use std::io::Result;
use std::mem::ManuallyDrop;
use std::net::{SocketAddr, ToSocketAddrs};

use crate::platform::{
    socket_connect,
    socket_close,
    socket_recv,
    socket_recv_from,
    socket_send,
    socket_send_to
};

/// A UDP socket
/// 
/// This internally contains a regular [`std::net::UdpSocket`], additionally providing async
/// IO functions for it. The provided async functions work identically to their sync counterparts,
/// so refer to their docs for more details
/// 
/// The underlying [`std::net::UdpSocket`] can be accessed using [`std()`](UdpSocket::std), which
/// is useful for accessing the various configuration options that are not exposed by this type.
/// 
/// Do not accidentally use any of the underlying blocking functions!
pub struct UdpSocket(ManuallyDrop<std::net::UdpSocket>);

impl UdpSocket {
    /// Creates a UDP socket from the given address
    /// 
    /// This internally creates a [`std::net::UdpSocket`] and calls
    /// [`set_nonblocking(true)`](std::net::UdpSocket::set_nonblocking) on it.
    /// 
    /// See the [`std` docs](std::net::UdpSocket::bind) for more info
    pub async fn bind<A: ToSocketAddrs>(addr: A) -> Result<Self> {
        let sock = std::net::UdpSocket::bind(addr)?;
        sock.set_nonblocking(true)?;

        Ok(Self(ManuallyDrop::new(sock)))
    }

    /// Connects this UDP socket to a remote address, allowing the send and recv syscalls to be used
    /// to send data and also applies filters to only receive data from the specified address
    /// 
    /// See the [`std` docs](std::net::UdpSocket::connect) for more info
    pub async fn connect<A: ToSocketAddrs>(&self, addr: A) -> Result<()> {
        let addr_iter = addr.to_socket_addrs().expect("Couldn't get address iterator");

        let mut res = None;

        for addr in addr_iter {
            match socket_connect(&*self.0, &addr).await {
                Ok(()) => return Ok(()),
                Err(err) => res = Some(err)
            }
        }

        match res {
            Some(err) => Err(err),
            None => panic!("Address iterator didn't provide any addresses")
        }
    }

    /// Get a reference to the underlying [`std::net::UdpSocket`]
    /// 
    /// This is useful for accessing the various configuration options not exposed by this type.
    /// Be careful not to accidentally call any blocking functions on the underlying socket,
    /// since it will block other tasks from executing
    pub fn std(&self) -> &std::net::UdpSocket {
        &self.0
    }

    /// Receives a single datagram message on the socket from the remote address to which it
    /// is connected. On success, returns the number of bytes read
    /// 
    /// This socket needs to be first connected to a remote host using [`connect()`](Self::connect),
    /// otherwise this function will fail
    /// 
    /// See the [`std` docs](std::net::UdpSocket::recv) for more info
    pub async fn recv(&self, buf: &mut [u8]) -> Result<usize> {
        socket_recv(&*self.0, buf, false).await
    }

    /// Receives a single datagram message on the socket. On success, returns the number of bytes
    /// read and the origin
    /// 
    /// See the [`std` docs](std::net::UdpSocket::recv_from) for more info
    pub async fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr)> {
        socket_recv_from(&*self.0, buf, false).await
    }

    /// Receives single datagram on the socket from the remote address to which it is connected,
    /// without removing the message from input queue. On success, returns the number of bytes peeked
    /// 
    /// This socket needs to be first connected to a remote host using [`connect()`](Self::connect),
    /// otherwise this function will fail
    /// 
    /// See the [`std` docs](std::net::UdpSocket::peek) for more info
    pub async fn peek(&self, buf: &mut [u8]) -> Result<usize> {
        socket_recv(&*self.0, buf, true).await
    }

    /// Receives a single datagram message on the socket, without removing it from the queue.
    /// On success, returns the number of bytes read and the origin
    /// 
    /// See the [`std` docs](std::net::UdpSocket::peek_from) for more info
    pub async fn peek_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr)> {
        socket_recv_from(&*self.0, buf, true).await
    }

    /// Sends data on the socket to the remote address to which it is connected
    /// 
    /// This socket needs to be first connected to a remote host using [`connect()`](Self::connect),
    /// otherwise this function will fail
    /// 
    /// See the [`std` docs](std::net::UdpSocket::send) for more info
    pub async fn send(&self, buf: &[u8]) -> Result<usize> {
        socket_send(&*self.0, buf).await
    }

    /// Sends data on the socket to the given address. On success, returns the number of bytes written
    /// 
    /// See the [`std` docs](std::net::UdpSocket::send_to) for more info
    pub async fn send_to<A: ToSocketAddrs>(&self, buf: &[u8], addr: A) -> Result<usize> {
        let addr = addr
            .to_socket_addrs()
            .expect("Couldn't get address iterator")
            .next()
            .expect("Address iterator didn't provide any addresses");

        socket_send_to(&*self.0, buf, &addr).await
    }
}

impl Drop for UdpSocket {
    fn drop(&mut self) {
        socket_close(&*self.0);
    }
}