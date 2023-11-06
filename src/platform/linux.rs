use std::io;
use std::mem;
use std::ptr;
use std::array;
use std::pin::Pin;
use std::future::Future;
use std::time::Duration;
use std::task::{Context, Poll};
use std::os::fd::{RawFd, FromRawFd};
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

#[derive(Clone, Copy)]
enum FutState {
    NotSubmitted,
    Submitted(IoKey),
    Done
}

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

    // Returns future for sleeping
    pub fn sleep_fut(&self, dur: Duration) -> impl Future<Output = ()> {
        struct TimeoutFut {
            timespec: Timespec,
            state: FutState
        }

        impl Future for TimeoutFut {
            type Output = ();

            fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                match self.state {
                    // Timeout not submitted yet
                    FutState::NotSubmitted => RUNTIME.with_borrow_mut(|rt| {
                        let key = rt.plat.new_io_key();
                        self.state = FutState::Submitted(key);

                        let sqe = opcode::Timeout::new(&self.timespec)
                            .build()
                            .user_data(key as u64);

                        rt.plat.submit_sqe(sqe);
                        rt.plat.submissions.insert(key, rt.current_task);

                        Poll::Pending
                    }),

                    // Timeout submitted, query it
                    FutState::Submitted(key) => RUNTIME.with_borrow_mut(|rt| {
                        if rt.plat.completions.remove(&key).is_some() {
                            self.state = FutState::Done;
                            Poll::Ready(())
                        }
                        else {
                            Poll::Pending
                        }
                    }),

                    FutState::Done => panic!("SleepFut polled even after completing")
                }
            }
        }

        impl Drop for TimeoutFut {
            fn drop(&mut self) {
                if let FutState::Submitted(key) = &self.state {
                    RUNTIME.with_borrow_mut(|rt| {
                        if rt.plat.submissions.remove(key).is_some() {
                            let sqe = opcode::TimeoutRemove::new(*key as u64).build();
                            rt.plat.submit_sqe(sqe);   
                        }
                    });
                }
            }
        }

        TimeoutFut {
            timespec: Timespec::from(dur),
            state: FutState::NotSubmitted
        }
    }

    // Returns future for socket recvs and peeks
    pub fn recv_fut<'a>(&self, sock: SocketHandle, buf: &'a mut [u8], peek: bool) -> impl Future<Output = io::Result<usize>> + 'a {
        struct RecvFut{
            sock: RawFd,
            buf: *mut u8,
            len: u32,
            peek: bool,
            state: FutState
        }

        impl Future for RecvFut {
            type Output = io::Result<usize>;

            fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                match self.state {
                    // Recv not submitted yet
                    FutState::NotSubmitted => RUNTIME.with_borrow_mut(|rt| {
                        let key = rt.plat.new_io_key();
                        self.state = FutState::Submitted(key);

                        let sqe = opcode::Recv::new(Fd(self.sock), self.buf, self.len)
                            .flags(if self.peek { libc::MSG_PEEK } else { 0 })
                            .build()
                            .user_data(key as u64);

                        rt.plat.submit_sqe(sqe);
                        rt.plat.submissions.insert(key, rt.current_task);

                        Poll::Pending
                    }),

                    // Recv submitted, query it
                    FutState::Submitted(key) => RUNTIME.with_borrow_mut(|rt| {
                        match rt.plat.completions.remove(&key) {
                            Some(res) => {
                                self.state = FutState::Done;

                                let bytes = libc_result_to_std(res).map(|bytes| bytes as usize);
                                Poll::Ready(bytes)
                            },

                            None => Poll::Pending
                        }
                    }),

                    FutState::Done => panic!("RecvFut polled even after completing")
                }
            }
        }

        impl Drop for RecvFut {
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

        RecvFut {
            sock: sock.0,
            buf: buf.as_mut_ptr(),
            len: buf.len() as u32,
            peek,
            state: FutState::NotSubmitted
        }
    }

    // Returns future for socket recv_froms and peek_froms
    pub fn recv_from_fut<'a>(&self, sock: SocketHandle, buf: &'a mut [u8], peek: bool) -> impl Future<Output = io::Result<(usize, SocketAddr)>> + 'a {
        struct RecvMsgFut {
            sock: RawFd,
            src_addr: *mut libc::sockaddr,
            msghdr: *mut libc::msghdr,
            state: FutState
        }

        impl Future for RecvMsgFut {
            type Output = io::Result<(usize, SocketAddr)>;

            fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                match self.state {
                    // RecvMsg not submitted yet
                    FutState::NotSubmitted => RUNTIME.with_borrow_mut(|rt| {
                        let key = rt.plat.new_io_key();
                        self.state = FutState::Submitted(key);

                        let sqe = opcode::RecvMsg::new(Fd(self.sock), self.msghdr)
                            .build()
                            .user_data(key as u64);

                        rt.plat.submit_sqe(sqe);
                        rt.plat.submissions.insert(key, rt.current_task);

                        Poll::Pending
                    }),

                    // RecvMsg submitted, query it
                    FutState::Submitted(key) => RUNTIME.with_borrow_mut(|rt| {
                        match rt.plat.completions.remove(&key) {
                            Some(res) => {
                                self.state = FutState::Done;

                                let bytes = libc_result_to_std(res).map(|bytes| bytes as usize);

                                let res = bytes.map(|bytes| {
                                    let src_addr = unsafe { &*self.src_addr };
                                    let src_addr = libc_addr_to_std(src_addr);

                                    (bytes, src_addr)
                                });

                                Poll::Ready(res)
                            },

                            None => Poll::Pending
                        }
                    }),

                    FutState::Done => panic!("RecvMsgFut polled even after completing"),
                }
            }
        }

        impl Drop for RecvMsgFut {
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

        async move {
            // Since a future is always pinned before use, these variable will have
            // a stable address that we can pass to the kernel without boxing
            // This approach saves us a heap allocation
            //
            // We also can't include these variables in the RecvMsgFut since they
            // refer to each other, so we punt the work of managing a self referential
            // future to the compiler using a seperate async block
            let mut iovec = libc::iovec {
                iov_base: buf.as_ptr() as *mut _,
                iov_len: buf.len()
            };

            // Create buffer with sufficient space to hold the largest sockaddr that we're expecting
            let mut src_addr = [0u8; MAX_LIBC_SOCKADDR_SIZE];

            let mut msghdr = libc::msghdr {
                msg_name: src_addr.as_mut_ptr() as *mut _,
                msg_namelen: mem::size_of::<libc::sockaddr>() as u32,
                msg_iov: &mut iovec,
                msg_iovlen: 1,
                msg_control: ptr::null_mut(),
                msg_controllen: 0,
                msg_flags: if peek { libc::MSG_PEEK } else { 0 }
            };

            RecvMsgFut {
                sock: sock.0,
                src_addr: src_addr.as_mut_ptr() as *mut _,
                msghdr: &mut msghdr,
                state: FutState::NotSubmitted
            }.await
        }
    }

    // Returns a future for socket sends and send_tos
    pub fn send_to_fut<'a>(&self, sock: SocketHandle, buf: &'a [u8], addr: Option<SocketAddr>) -> impl Future<Output = io::Result<usize>> + 'a {
        struct SendMsgFut {
            sock: RawFd,
            msghdr: *mut libc::msghdr,
            state: FutState
        }

        impl Future for SendMsgFut {
            type Output = io::Result<usize>;

            fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                match self.state {
                    // SendMsg not submitted yet
                    FutState::NotSubmitted => RUNTIME.with_borrow_mut(|rt| {
                        let key = rt.plat.new_io_key();
                        self.state = FutState::Submitted(key);

                        let sqe = opcode::SendMsg::new(Fd(self.sock), self.msghdr)
                            .build()
                            .user_data(key as u64);

                        rt.plat.submit_sqe(sqe);
                        rt.plat.submissions.insert(key, rt.current_task);

                        Poll::Pending
                    }),

                    // SendMsg submitted, query it
                    FutState::Submitted(key) => RUNTIME.with_borrow_mut(|rt| {
                        match rt.plat.completions.remove(&key) {
                            Some(res) => {
                                self.state = FutState::Done;

                                let bytes = libc_result_to_std(res).map(|bytes| bytes as usize);
                                Poll::Ready(bytes)
                            },

                            None => Poll::Pending
                        }
                    }),

                    FutState::Done => panic!("SendMsgFut polled even after completing"),
                }
            }
        }

        impl Drop for SendMsgFut {
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

        async move {
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

            SendMsgFut {
                sock: sock.0,
                msghdr: &mut msghdr,
                state: FutState::NotSubmitted
            }.await
        }
    }

    pub fn accept_fut(&self, sock: SocketHandle) -> impl Future<Output = io::Result<(TcpStream, SocketAddr)>> {
        struct AcceptFut {
            sock: RawFd,
            sockaddr: [u8; MAX_LIBC_SOCKADDR_SIZE], // Buffer with sufficient space to hold the largest sockaddr that we're expecting
            addrlen: libc::socklen_t,
            state: FutState
        }

        impl Future for AcceptFut {
            type Output = io::Result<(TcpStream, SocketAddr)>;

            fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                match self.state {
                    // Accept not submitted yet
                    FutState::NotSubmitted => RUNTIME.with_borrow_mut(|rt| {
                        let key = rt.plat.new_io_key();
                        self.state = FutState::Submitted(key);

                        let addr = self.sockaddr.as_mut_ptr() as *mut libc::sockaddr;

                        let sqe = opcode::Accept::new(Fd(self.sock), addr, &mut self.addrlen)
                            .build()
                            .user_data(key as u64);

                        rt.plat.submit_sqe(sqe);
                        rt.plat.submissions.insert(key, rt.current_task);

                        Poll::Pending
                    }),

                    // Accept submitted, query it
                    FutState::Submitted(key) => RUNTIME.with_borrow_mut(|rt| {
                        match rt.plat.completions.remove(&key) {
                            Some(res) => {
                                self.state = FutState::Done;

                                let fd = libc_result_to_std(res);

                                let res = fd.map(|fd| {
                                    let stream = unsafe { TcpStream::from_raw_fd(fd) };

                                    let peer_addr = unsafe { &*(self.sockaddr.as_ptr() as *const libc::sockaddr) };
                                    let peer_addr = libc_addr_to_std(peer_addr);

                                    (stream, peer_addr)
                                });

                                Poll::Ready(res)
                            },

                            None => Poll::Pending
                        }
                    }),

                    FutState::Done => panic!("AcceptFut polled even after completing"),
                }
            }
        }

        impl Drop for AcceptFut {
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

        AcceptFut {
            sock: sock.0,
            sockaddr: [0u8; MAX_LIBC_SOCKADDR_SIZE],
            addrlen: MAX_LIBC_SOCKADDR_SIZE as libc::socklen_t,
            state: FutState::NotSubmitted
        }
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