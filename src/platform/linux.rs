use std::io;
use std::mem;
use std::ptr;
use std::array;
use std::pin::Pin;
use std::os::fd::RawFd;
use std::future::Future;
use std::time::Duration;
use std::task::{Context, Poll};
use std::net::{SocketAddr, Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};

use io_uring::{
    opcode,
    squeue,
    IoUring,
    Probe,
    types::{Timespec, Fd}
};

use nohash::{IntSet, IntMap};

use crate::{
    RUNTIME,
    runtime::{TaskId, SocketHandle},
    error::InitError
};

type IoKey = u32;

pub struct Platform {
    ring: IoUring,
    io_key_counter: IoKey,

    submitted_timeouts: IntMap<IoKey, TaskId>,
    completed_timeouts: IntSet<IoKey>,
    timespec_store: Vec<Box<Timespec>>,

    submitted_recv_msgs: IntMap<IoKey, TaskId>,
    completed_recv_msgs: IntMap<IoKey, io::Result<usize>>,

    submitted_send_msgs: IntMap<IoKey, TaskId>,
    completed_send_msgs: IntMap<IoKey, io::Result<usize>>
}

impl Platform {
    pub fn new() -> Result<Self, InitError> {
        Ok(Self {
            ring: new_io_uring()?,
            io_key_counter: 0,

            submitted_timeouts: IntMap::default(),
            completed_timeouts: IntSet::default(),
            timespec_store: Vec::new(),

            submitted_recv_msgs: IntMap::default(),
            completed_recv_msgs: IntMap::default(),

            submitted_send_msgs: IntMap::default(),
            completed_send_msgs: IntMap::default()
        })
    }

    // Returns future for sleeping
    pub fn sleep_fut(&self, dur: Duration) -> impl Future<Output = ()> {
        enum FutState {
            NotSubmitted(Duration),
            Submitted(IoKey),
            Done
        }

        struct SleepFut(FutState);

        impl Future for SleepFut {
            type Output = ();

            fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                match self.0 {
                    // Timeout not submitted yet
                    FutState::NotSubmitted(dur) => RUNTIME.with_borrow_mut(|rt| {
                        let key = rt.plat.new_io_key();
                        self.0 = FutState::Submitted(key);

                        rt.plat.submitted_timeouts.insert(key, rt.current_task);

                        let timespec = Box::new(Timespec::from(dur));
                        let sqe = opcode::Timeout::new(timespec.as_ref())
                            .build()
                            .user_data(key as u64);

                        rt.plat.timespec_store.push(timespec);
                        rt.plat.submit_sqe(sqe);

                        Poll::Pending
                    }),

                    // Timeout submitted, query it
                    FutState::Submitted(key) => RUNTIME.with_borrow_mut(|rt| {
                        if rt.plat.completed_timeouts.remove(&key) {
                            self.0 = FutState::Done;
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

        impl Drop for SleepFut {
            fn drop(&mut self) {
                if let FutState::Submitted(key) = &self.0 {
                    RUNTIME.with_borrow_mut(|rt| {
                        if rt.plat.submitted_timeouts.remove(key).is_some() {
                            let sqe = opcode::TimeoutRemove::new(*key as u64).build();
                            rt.plat.submit_sqe(sqe);   
                        }
                    });
                }
            }
        }

        SleepFut(FutState::NotSubmitted(dur))
    }

    // Returns future for socket recv_froms and peek_froms
    pub fn recv_from_fut<'a>(&self, sock: SocketHandle, buf: &'a mut [u8], peek: bool) -> impl Future<Output = io::Result<(usize, SocketAddr)>> + 'a {
        #[derive(Clone, Copy)]
        enum FutState {
            NotSubmitted(RawFd),
            Submitted(IoKey),
            Done
        }

        struct RecvMsgFut {
            src_addr: *mut libc::sockaddr,
            msghdr: *mut libc::msghdr,
            state: FutState
        }

        impl Future for RecvMsgFut {
            type Output = io::Result<(usize, SocketAddr)>;

            fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                match self.state {
                    // RecvMsg not submitted yet
                    FutState::NotSubmitted(sock) => RUNTIME.with_borrow_mut(|rt| {
                        let key = rt.plat.new_io_key();
                        self.state = FutState::Submitted(key);

                        rt.plat.submitted_recv_msgs.insert(key, rt.current_task);

                        let sqe = opcode::RecvMsg::new(Fd(sock), self.msghdr)
                                .build()
                                .user_data(key as u64);

                        rt.plat.submit_sqe(sqe);
                        Poll::Pending
                    }),

                    // RecvMsg submitted, query it
                    FutState::Submitted(key) => RUNTIME.with_borrow_mut(|rt| {
                        match rt.plat.completed_recv_msgs.remove(&key) {
                            Some(bytes) => {
                                self.state = FutState::Done;

                                let src_addr = unsafe { &*self.src_addr };
                                let src_addr = libc_addr_to_std(src_addr);

                                let res = bytes.map(|bytes| (bytes, src_addr));
                                Poll::Ready(res)
                            },

                            None => Poll::Pending
                        }
                    }),

                    FutState::Done => panic!("RecvFromFut polled even after completing"),
                }
            }
        }

        impl Drop for RecvMsgFut {
            fn drop(&mut self) {
                if let FutState::Submitted(key) = &self.state {
                    RUNTIME.with_borrow_mut(|rt| {
                        if rt.plat.submitted_recv_msgs.remove(key).is_some() {
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
            let mut iovec = libc::iovec {
                iov_base: buf.as_ptr() as *mut _,
                iov_len: buf.len()
            };

            let mut src_addr = libc::sockaddr {
                sa_data: [0; 14],
                sa_family: 0
            };

            let mut msghdr = libc::msghdr {
                msg_name: &mut src_addr as *mut libc::sockaddr as *mut _,
                msg_namelen: mem::size_of::<libc::sockaddr>() as u32,
                msg_iov: &mut iovec,
                msg_iovlen: 1,
                msg_control: ptr::null_mut(),
                msg_controllen: 0,
                msg_flags: if peek { libc::MSG_PEEK } else { 0 }
            };

            RecvMsgFut {
                src_addr: &mut src_addr,
                msghdr: &mut msghdr,
                state: FutState::NotSubmitted(sock.0)
            }.await
        }
    }

    // Returns a future for socket sends and send_tos
    pub fn send_to_fut<'a>(&self, sock: SocketHandle, buf: &'a [u8], addr: Option<SocketAddr>) -> impl Future<Output = io::Result<usize>> + 'a {
        #[derive(Clone, Copy)]
        enum FutState {
            NotSubmitted(RawFd),
            Submitted(IoKey),
            Done
        }

        struct SendMsgFut {
            msghdr: *mut libc::msghdr,
            state: FutState
        }

        impl Future for SendMsgFut {
            type Output = io::Result<usize>;

            fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                match self.state {
                    // SendMsg not submitted yet
                    FutState::NotSubmitted(sock) => RUNTIME.with_borrow_mut(|rt| {
                        let key = rt.plat.new_io_key();
                        self.state = FutState::Submitted(key);

                        rt.plat.submitted_send_msgs.insert(key, rt.current_task);

                        let sqe = opcode::SendMsg::new(Fd(sock), self.msghdr)
                                .build()
                                .user_data(key as u64);

                        rt.plat.submit_sqe(sqe);
                        Poll::Pending
                    }),

                    // SendMsg submitted, query it
                    FutState::Submitted(key) => RUNTIME.with_borrow_mut(|rt| {
                        match rt.plat.completed_send_msgs.remove(&key) {
                            Some(bytes) => {
                                self.state = FutState::Done;
                                Poll::Ready(bytes)
                            },

                            None => Poll::Pending
                        }
                    }),

                    FutState::Done => panic!("SendToFut polled even after completing"),
                }
            }
        }

        impl Drop for SendMsgFut {
            fn drop(&mut self) {
                if let FutState::Submitted(key) = &self.state {
                    RUNTIME.with_borrow_mut(|rt| {
                        if rt.plat.submitted_send_msgs.remove(key).is_some() {
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
            let mut iovec = libc::iovec {
                iov_base: buf.as_ptr() as *mut _,
                iov_len: buf.len()
            };

            let mut dst_addr = match &addr {
                // send_to operation for unconnected socket
                // Address needs to be specified
                Some(addr) => std_addr_to_libc(addr),

                // send operation for connected socket
                // No address needs to be specified
                None => [0; MAX_LIBC_SOCKADDR_SIZE]
            };

            let mut msghdr = libc::msghdr {
                msg_name: dst_addr.as_mut_ptr() as *mut _,
                msg_namelen: if addr.is_some() { dst_addr.len() as u32 } else { 0 },
                msg_iov: &mut iovec,
                msg_iovlen: 1,
                msg_control: ptr::null_mut(),
                msg_controllen: 0,
                msg_flags: 0
            };

            SendMsgFut { msghdr: &mut msghdr, state: FutState::NotSubmitted(sock.0) }.await
        }
    }

    pub fn wait_for_io(&mut self, wakeups: &mut Vec<TaskId>) {
        self.ring
            .submit_and_wait(1)
            .expect("Failed to submit io_uring");

        self.clear_stores();

        for cqe in self.ring.completion() {
            let key = IoKey::from(cqe.user_data() as u32);

            // Is this a Timeout completion?
            if let Some(task_id) = self.submitted_timeouts.remove(&key) {
                self.completed_timeouts.insert(key);
                wakeups.push(task_id);
            }

            // Is this a RecvMsg completion?
            else if let Some(task_id) = self.submitted_recv_msgs.remove(&key) {
                let res = if cqe.result() >= 0 {
                    Ok(cqe.result() as usize)
                }
                else {
                    Err(io::Error::from_raw_os_error(-cqe.result()))
                };

                self.completed_recv_msgs.insert(key, res);
                wakeups.push(task_id);
            }

            // Is this a SendMsg completion?
            else if let Some(task_id) = self.submitted_send_msgs.remove(&key) {
                let res = if cqe.result() >= 0 {
                    Ok(cqe.result() as usize)
                }
                else {
                    Err(io::Error::from_raw_os_error(-cqe.result()))
                };

                self.completed_send_msgs.insert(key, res);
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

                    // Stores not needed after submission (submit_stable feature)
                    self.clear_stores();
                }
            }
        }
    }

    fn clear_stores(&mut self) {
        self.timespec_store.clear();
    }
}

fn new_io_uring() -> Result<IoUring, InitError> {
    let ring = IoUring::new(128).map_err(|err| InitError::IoUringCreationFailed(err))?;

    // Check required features
    if !ring.params().is_feature_submit_stable() {
        return Err(InitError::IoUringFeatureNotPresent("submit_stable"));
    }

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

        // Get address params, converting from network endianness to host endianness
        let ip = Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr));
        let port = u16::from_be(addr.sin_port);
        
        SocketAddr::V4(SocketAddrV4::new(ip, port))
    }

    // IPv6 address
    else if addr.sa_family == libc::AF_INET6 as libc::sa_family_t {
        // Reinterpret as sockaddr_in6
        let addr = unsafe { &*(addr as *const libc::sockaddr as *const libc::sockaddr_in6) };

        // Get address params, converting from network to host endianness
        let segments = array::from_fn(|i| {
            let octet_1 = addr.sin6_addr.s6_addr[i * 2] as u16;
            let octet_2 = addr.sin6_addr.s6_addr[(i * 2) + 1] as u16;

            u16::from_be(octet_1 << 8 | octet_2)
        });

        let ip = Ipv6Addr::from(segments);
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

const MAX_LIBC_SOCKADDR_SIZE: usize = mem::size_of::<libc::sockaddr_in6>();

fn std_addr_to_libc(addr: &SocketAddr) -> [u8; MAX_LIBC_SOCKADDR_SIZE] {
    let mut buf = [0; MAX_LIBC_SOCKADDR_SIZE];

    match addr {
        // IPv4 address
        SocketAddr::V4(addr) => {
            // Interpret buf as sockaddr_in
            let out_addr = unsafe { &mut *(buf.as_mut_ptr() as *mut libc::sockaddr_in) };

            // Write address params, converting from host to network endianness
            out_addr.sin_family = libc::AF_INET as libc::sa_family_t;
            out_addr.sin_port = u16::to_be(addr.port());
            out_addr.sin_addr.s_addr = u32::to_be(u32::from(*addr.ip()));
            out_addr.sin_zero = [0; 8];
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