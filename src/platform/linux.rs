use std::io;
use std::mem;
use std::ptr;
use std::pin::Pin;
use std::future::Future;
use std::time::Duration;
use std::task::{Context, Poll};
use std::os::fd::FromRawFd;
use std::net::{TcpStream, SocketAddr, SocketAddrV4, SocketAddrV6, Ipv4Addr, Ipv6Addr};

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
    runtime::{TaskId, SocketHandle},
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
            io_key_counter: 0,
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

pub async fn recv(sock: SocketHandle, buf: &mut [u8], peek: bool) -> io::Result<usize> {
    let sqe = opcode::Recv::new(Fd(sock.0), buf.as_mut_ptr(), buf.len() as u32)
        .flags(if peek { libc::MSG_PEEK } else { 0 })
        .build();

    let res = IoUringFut::new(sqe).await;

    libc_result_to_std(res).map(|bytes| bytes as usize)
}

pub async fn recv_from(sock: SocketHandle, buf: &mut [u8], peek: bool) -> io::Result<(usize, SocketAddr)> {
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
    
    let sqe = opcode::RecvMsg::new(Fd(sock.0), &mut msghdr).build();
    let res = IoUringFut::new(sqe).await;

    libc_result_to_std(res).map(|bytes| {
        let src_addr = unsafe { &*(src_addr.as_ptr() as *const _) };
        let src_addr = libc_addr_to_std(src_addr);

        (bytes as usize, src_addr)
    })
}

pub async fn send_to(sock: SocketHandle, buf: &[u8], addr: Option<SocketAddr>) -> io::Result<usize> {
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

    // If addr is Some, then we're working with an unconnected socket and
    // a dst addr needs to be specified
    // Otherwise we're working with a connected socket and there is no dst
    // addr here
    let mut dst_addr = addr.map(|addr| std_addr_to_libc(&addr));

    let mut msghdr = libc::msghdr {
        msg_name: match &mut dst_addr {
            Some(dst_addr) => dst_addr.as_mut_ptr() as *mut _,
            None => ptr::null_mut()
        },

        msg_namelen: match dst_addr {
            Some(dst_addr) => dst_addr.len() as u32,
            None => 0
        },

        msg_iov: &mut iovec,
        msg_iovlen: 1,
        msg_control: ptr::null_mut(),
        msg_controllen: 0,
        msg_flags: 0
    };

    let sqe = opcode::SendMsg::new(Fd(sock.0), &mut msghdr).build();
    let res = IoUringFut::new(sqe).await;

    libc_result_to_std(res).map(|bytes| bytes as usize)
}

pub async fn accept(sock: SocketHandle) -> io::Result<(TcpStream, SocketAddr)> {
    // Create buffer with sufficient space to hold the largest sockaddr that we're expecting
    let mut sockaddr = [0u8; MAX_LIBC_SOCKADDR_SIZE];
    let mut addrlen = MAX_LIBC_SOCKADDR_SIZE as libc::socklen_t;

    let libc_addr = sockaddr.as_mut_ptr() as *mut libc::sockaddr;

    let sqe = opcode::Accept::new(Fd(sock.0), libc_addr, &mut addrlen).build();
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
        ("RecvMsg", opcode::RecvMsg::CODE),
        ("SendMsg", opcode::SendMsg::CODE)
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