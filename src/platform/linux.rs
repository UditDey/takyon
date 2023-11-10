use std::io;
use std::mem;
use std::ptr;
use std::pin::Pin;
use std::fs::File;
use std::path::Path;
use std::ffi::CString;
use std::future::Future;
use std::time::Duration;
use std::task::{Context, Poll};
use std::os::{fd::{AsRawFd, FromRawFd}, unix::ffi::OsStrExt};
use std::net::{TcpStream, SocketAddr, SocketAddrV4, SocketAddrV6, Ipv4Addr, Ipv6Addr, Shutdown};

use io_uring::{
    opcode,
    squeue,
    IoUring,
    Probe,
    types::{Timespec, Fd}
};

use nohash::IntMap;

use crate::{
    RUNTIME,
    fs::OpenOptions,
    runtime::TaskId,
    error::InitError
};

type IoKey = u32;

const MAX_LIBC_SOCKADDR_SIZE: usize = mem::size_of::<libc::sockaddr_in6>();

pub struct Platform {
    ring: IoUring,
    io_key_counter: IoKey,
    submissions: IntMap<IoKey, TaskId>,
    completions: IntMap<IoKey, i32>
}

impl Platform {
    pub fn new() -> Result<Self, InitError> {
        Ok(Self {
            ring: new_io_uring()?,
            io_key_counter: 1, // Io Keys start from 1 since 0 is reserved for fd close operations
            submissions: IntMap::default(),
            completions: IntMap::default()
        })
    }

    pub fn wait_for_io(&mut self, wakeups: &mut Vec<TaskId>) {
        self.ring
            .submit_and_wait(1)
            .expect("Failed to submit io_uring");

        for cqe in self.ring.completion() {
            let key = IoKey::from(cqe.user_data() as u32);

            if let Some(task_id) = self.submissions.remove(&key) {
                self.completions.insert(key, cqe.result());
                wakeups.push(task_id);
            }
        }
    }

    pub fn reset(&mut self) {
        // To get rid of pending IO we drop the current io_uring and
        // reset to our original state, we don't handle InitErrors
        // because since by this point `new()` has been called
        // successfully it is unlikely to return an error now
        *self = Self::new().unwrap();
    }

    fn new_io_key(&mut self) -> IoKey {
        let key = self.io_key_counter;
        self.io_key_counter = key.wrapping_add(1);

        // IoKey 0 is reserved for fd close operations
        if self.io_key_counter == 0 {
            self.io_key_counter = 1;
        }

        key
    }

    fn submit_sqe(&mut self, sqe: squeue::Entry) {
        loop {
            // Try and push the sqe
            let res = unsafe {
                self.ring
                    .submission()
                    .push(&sqe)
            };

            match res {
                // Push successful, return
                Ok(()) => return,
                
                // No space left in submission queue
                // Submit it to make space and try again
                Err(_) => {
                    self.ring
                        .submit()
                        .expect("Failed to submit io_uring");
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum FutState {
    NotSubmitted,
    Submitted(IoKey),
    Done
}

struct IoUringFut {
    sqe: squeue::Entry,
    state: FutState
}

impl IoUringFut {
    pub fn new(sqe: squeue::Entry) -> Self {
        Self { sqe, state: FutState::NotSubmitted }
    }
}

impl Future for IoUringFut {
    type Output = i32;

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.state {
            // sqe not submitted yet
            FutState::NotSubmitted => RUNTIME.with_borrow_mut(|rt| {
                let key = rt.plat.new_io_key();
                let sqe = self.sqe.clone().user_data(key as u64);
                
                rt.plat.submit_sqe(sqe);
                rt.plat.submissions.insert(key, rt.current_task);
                self.state = FutState::Submitted(key);

                Poll::Pending
            }),

            // sqe submitted, query it
            FutState::Submitted(key) => RUNTIME.with_borrow_mut(|rt| {
                match rt.plat.completions.remove(&key) {
                    Some(res) => {
                        self.state = FutState::Done;
                        Poll::Ready(res)
                    },
                    None => Poll::Pending
                }
            }),

            FutState::Done => panic!("IoRingFut polled even after completing")
        }
    }
}

impl Drop for IoUringFut {
    fn drop(&mut self) {
        if let FutState::Submitted(key) = &self.state {
            RUNTIME.with_borrow_mut(|rt| {
                if rt.plat.submissions.remove(key).is_some() {
                    let sqe = opcode::AsyncCancel::new(*key as u64).build();
                    rt.plat.submit_sqe(sqe);
                }
            });
        }
    }
}

pub async fn sleep(dur: Duration) {
    let timespec = Timespec::from(dur);
    let sqe = opcode::Timeout::new(&timespec).build();

    IoUringFut::new(sqe).await;
}

pub async fn file_open(path: &Path, opts: &OpenOptions) -> io::Result<File> {
    let mut flags = match (opts.read, opts.write) {
        (true, false) => libc::O_RDONLY,
        (false, true) => libc::O_WRONLY,
        (true, true) => libc::O_RDWR,
        (false, false) => 0
    };

    if opts.append {
        flags |= libc::O_APPEND;
    }

    if opts.truncate {
        flags |= libc::O_TRUNC;
    }

    if opts.create || opts.create_new {
        flags |= libc::O_CREAT;
    }

    if opts.create_new {
        flags |= libc::O_EXCL;
    }

    let dirfd = Fd(libc::AT_FDCWD);
    let path = CString::new(path.as_os_str().as_bytes()).expect("Provided path contains internal null char");

    let sqe = opcode::OpenAt::new(dirfd, path.as_ptr() as *const _)
        .flags(flags)
        .mode(0o666)
        .build();

    let res = IoUringFut::new(sqe).await;

    libc_result_to_std(res).map(|fd| unsafe { File::from_raw_fd(fd) })
}

pub async fn file_read(file: &File, buf: &mut [u8]) -> io::Result<usize> {
    let sqe = opcode::Read::new(Fd(file.as_raw_fd()), buf.as_mut_ptr(), buf.len() as u32).build();
    let res = IoUringFut::new(sqe).await;

    libc_result_to_std(res).map(|bytes| bytes as usize)
}

pub async fn file_write(file: &File, buf: &[u8]) -> io::Result<usize> {
    let sqe = opcode::Write::new(Fd(file.as_raw_fd()), buf.as_ptr(), buf.len() as u32).build();
    let res = IoUringFut::new(sqe).await;

    libc_result_to_std(res).map(|bytes| bytes as usize)
}

pub fn file_close(file: &File) {
    RUNTIME.with_borrow_mut(|rt| {
        let sqe = opcode::Close::new(Fd(file.as_raw_fd()))
            .build()
            .user_data(0); // IoKey 0 reserved for fd closes

        rt.plat.submit_sqe(sqe);   
    });
}

pub async fn socket_create<T: FromRawFd>(ipv6: bool, udp: bool) -> io::Result<T> {
    let domain = if ipv6 { libc::AF_INET6 } else { libc::AF_INET };
    let socket_type = if udp { libc::SOCK_DGRAM } else { libc::SOCK_STREAM };
    let protocol = if udp { libc::IPPROTO_UDP } else { libc::IPPROTO_TCP };

    let sqe = opcode::Socket::new(domain, socket_type, protocol).build();
    let res = IoUringFut::new(sqe).await;

    let fd = libc_result_to_std(res);

    fd.map(|fd| unsafe { T::from_raw_fd(fd) })
}

pub fn socket_close<T: AsRawFd>(sock: &T) {
    RUNTIME.with_borrow_mut(|rt| {
        let sqe = opcode::Close::new(Fd(sock.as_raw_fd()))
            .build()
            .user_data(0); // IoKey 0 reserved for fd closes

        rt.plat.submit_sqe(sqe);   
    });
}

pub async fn socket_connect<T: AsRawFd>(sock: &T, addr: &SocketAddr) -> io::Result<()> {
    let addr = std_addr_to_libc(addr);

    let sqe = opcode::Connect::new(Fd(sock.as_raw_fd()), addr.as_ptr() as *const libc::sockaddr, addr.len() as u32).build();
    let res = IoUringFut::new(sqe).await;

    libc_result_to_std(res).map(|_| ())
}

pub async fn socket_shutdown<T: AsRawFd>(sock: &T, how: Shutdown) -> io::Result<()> {
    let how = match how {
        Shutdown::Read => libc::SHUT_RD,
        Shutdown::Write => libc::SHUT_WR,
        Shutdown::Both => libc::SHUT_RDWR
    };

    let sqe = opcode::Shutdown::new(Fd(sock.as_raw_fd()), how).build();
    let res = IoUringFut::new(sqe).await;

    libc_result_to_std(res).map(|_| ())
}

pub async fn socket_recv<T: AsRawFd>(sock: &T, buf: &mut [u8], peek: bool) -> io::Result<usize> {
    let sqe = opcode::Recv::new(Fd(sock.as_raw_fd()), buf.as_mut_ptr(), buf.len() as u32)
        .flags(if peek { libc::MSG_PEEK } else { 0 })
        .build();

    let res = IoUringFut::new(sqe).await;

    libc_result_to_std(res).map(|bytes| bytes as usize)
}

pub async fn socket_recv_from<T: AsRawFd>(sock: &T, buf: &mut [u8], peek: bool) -> io::Result<(usize, SocketAddr)> {
    // Since a future is always pinned before use, these variable will have
    // a stable address that we can pass to the kernel without boxing
    // This approach saves us a heap allocation
    let mut iovec = libc::iovec {
        iov_base: buf.as_ptr() as *mut _,
        iov_len: buf.len()
    };

    // Create buffer with sufficient space to hold the largest sockaddr that we're expecting
    let mut src_addr = [0u8; MAX_LIBC_SOCKADDR_SIZE];

    let mut msghdr = libc::msghdr {
        msg_name: src_addr.as_mut_ptr() as *mut _,
        msg_namelen: src_addr.len() as u32,
        msg_iov: &mut iovec,
        msg_iovlen: 1,
        msg_control: ptr::null_mut(),
        msg_controllen: 0,
        msg_flags: if peek { libc::MSG_PEEK } else { 0 }
    };
    
    let sqe = opcode::RecvMsg::new(Fd(sock.as_raw_fd()), &mut msghdr).build();
    let res = IoUringFut::new(sqe).await;

    libc_result_to_std(res).map(|bytes| {
        let src_addr = unsafe { &*(src_addr.as_ptr() as *const _) };
        let src_addr = libc_addr_to_std(src_addr);

        (bytes as usize, src_addr)
    })
}

pub async fn socket_send<T: AsRawFd>(sock: &T, buf: &[u8]) -> io::Result<usize> {
    let sqe = opcode::Send::new(Fd(sock.as_raw_fd()), buf.as_ptr(), buf.len() as u32).build();
    let res = IoUringFut::new(sqe).await;

    libc_result_to_std(res).map(|bytes| bytes as usize)
}

pub async fn socket_send_to<T: AsRawFd>(sock: &T, buf: &[u8], addr: &SocketAddr) -> io::Result<usize> {
    // Since a future is always pinned before use, these variable will have
    // a stable address that we can pass to the kernel without boxing
    // This approach saves us a heap allocation
    //
    // We also can't include these variables in the SendMsgFut since they
    // refer to each other, so we punt the work of managing a self referential
    // future to the compiler using a seperate async block
    let mut iovec = libc::iovec {
        iov_base: buf.as_ptr() as *mut _,
        iov_len: buf.len()
    };

    let mut addr = std_addr_to_libc(&addr);

    let mut msghdr = libc::msghdr {
        msg_name: addr.as_mut_ptr() as *mut _,
        msg_namelen: addr.len() as u32,
        msg_iov: &mut iovec,
        msg_iovlen: 1,
        msg_control: ptr::null_mut(),
        msg_controllen: 0,
        msg_flags: 0
    };

    let sqe = opcode::SendMsg::new(Fd(sock.as_raw_fd()), &mut msghdr).build();
    let res = IoUringFut::new(sqe).await;

    libc_result_to_std(res).map(|bytes| bytes as usize)
}

pub async fn socket_accept<T: AsRawFd>(sock: &T) -> io::Result<(TcpStream, SocketAddr)> {
    // Create buffer with sufficient space to hold the largest sockaddr that we're expecting
    let mut sockaddr = [0u8; MAX_LIBC_SOCKADDR_SIZE];
    let mut addrlen = MAX_LIBC_SOCKADDR_SIZE as libc::socklen_t;

    let libc_addr = sockaddr.as_mut_ptr() as *mut libc::sockaddr;

    let sqe = opcode::Accept::new(Fd(sock.as_raw_fd()), libc_addr, &mut addrlen).build();
    let res = IoUringFut::new(sqe).await;

    let fd = libc_result_to_std(res);

    fd.map(|fd| {
        let stream = unsafe { TcpStream::from_raw_fd(fd) };

        let peer_addr = unsafe { &*libc_addr };
        let peer_addr = libc_addr_to_std(peer_addr);

        (stream, peer_addr)
    })
}

fn new_io_uring() -> Result<IoUring, InitError> {
    let ring = IoUring::new(128).map_err(|err| InitError::IoUringCreationFailed(err))?;

    // Check required features
    if !ring.params().is_feature_nodrop() {
        return Err(InitError::IoUringFeatureNotPresent("no_drop"));
    }

    // Probe supported opcodes
    let mut probe = Probe::new();

    ring.submitter()
        .register_probe(&mut probe)
        .map_err(|err| InitError::IoUringProbeFailed(err))?;

    // Check required opcodes
    let req_opcodes = [
        ("AsyncCancel", opcode::AsyncCancel::CODE),
        ("Timeout", opcode::Timeout::CODE),
        ("Socket", opcode::Socket::CODE),
        ("Connect", opcode::Connect::CODE),
        ("RecvMsg", opcode::RecvMsg::CODE),
        ("SendMsg", opcode::SendMsg::CODE),
        ("Shutdown", opcode::Shutdown::CODE),
        ("OpenAt", opcode::OpenAt::CODE),
        ("Read", opcode::Read::CODE),
        ("Write", opcode::Write::CODE),
        ("Close", opcode::Close::CODE)
    ];

    for (name, code) in req_opcodes {
        if !probe.is_supported(code) {
            return Err(InitError::IoUringOpcodeUnsupported(name));
        }
    }

    Ok(ring)
}

fn libc_addr_to_std(addr: &libc::sockaddr) -> SocketAddr {
    // IPv4 address
    if addr.sa_family == libc::AF_INET as libc::sa_family_t {
        // Reinterpret as sockaddr_in
        let addr = unsafe { &*(addr as *const libc::sockaddr as *const libc::sockaddr_in) };

        // Get address params, converting from network to host endianness
        let ip = Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr));
        let port = u16::from_be(addr.sin_port);
        
        SocketAddr::V4(SocketAddrV4::new(ip, port))
    }

    // IPv6 address
    else if addr.sa_family == libc::AF_INET6 as libc::sa_family_t {
        // Reinterpret as sockaddr_in6
        let addr = unsafe { &*(addr as *const libc::sockaddr as *const libc::sockaddr_in6) };

        // Get address params, converting from network to host endianness
        let ip = Ipv6Addr::from(addr.sin6_addr.s6_addr);
        let port = u16::from_be(addr.sin6_port);
        let flowinfo = u32::from_be(addr.sin6_flowinfo);
        let scope_id = u32::from_be(addr.sin6_scope_id);

        SocketAddr::V6(SocketAddrV6::new(ip, port, flowinfo, scope_id))
    }

    // Unknown
    else {
        panic!("addr.sa_family has unexpected value `{}`", addr.sa_family);
    }
}

fn std_addr_to_libc(addr: &SocketAddr) -> [u8; MAX_LIBC_SOCKADDR_SIZE] {
    let mut buf = [0u8; MAX_LIBC_SOCKADDR_SIZE];

    match addr {
        // IPv4 address
        SocketAddr::V4(addr) => {
            // Interpret buf as sockaddr_in
            let out_addr = unsafe { &mut *(buf.as_mut_ptr() as *mut libc::sockaddr_in) };

            // Write address params, converting from host to network endianness
            out_addr.sin_family = libc::AF_INET as libc::sa_family_t;
            out_addr.sin_port = u16::to_be(addr.port());
            out_addr.sin_addr.s_addr = u32::to_be(u32::from(*addr.ip()));
        },

        // IPv6 address
        SocketAddr::V6(addr) => {
            // Interpret buf as sockaddr_in6
            let out_addr = unsafe { &mut *(buf.as_mut_ptr() as *mut libc::sockaddr_in6) };

            // Write address params, converting from host to network endianness
            out_addr.sin6_family = libc::AF_INET6 as libc::sa_family_t;
            out_addr.sin6_port = u16::to_be(addr.port());
            out_addr.sin6_flowinfo = u32::to_be(addr.flowinfo());

            // These octets together are in host endianness
            // See the implementation of the Ipv6Addr::segments() fn for proof
            out_addr.sin6_addr.s6_addr = addr.ip().octets();

            out_addr.sin6_scope_id = u32::to_be(addr.scope_id());
        }
    }
    
    buf
}

fn libc_result_to_std(res: i32) -> io::Result<i32> {
    // Positive res means okay, negative means error and is equal
    // to the negated error code
    if res >= 0 {
        Ok(res)
    }
    else {
        Err(io::Error::from_raw_os_error(-res))
    }
}