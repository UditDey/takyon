use std::io::Result;
use std::net::{SocketAddr, ToSocketAddrs};

use crate::{RUNTIME, runtime::SocketHandle};

pub struct UdpSocket(std::net::UdpSocket);

impl UdpSocket {
    pub fn bind<A: ToSocketAddrs>(addr: A) -> Result<Self> {
        let socket = std::net::UdpSocket::bind(addr)?;
        socket.set_nonblocking(true)?;

        Ok(Self(socket))
    }

    pub fn std(&self) -> &std::net::UdpSocket {
        &self.0
    }

    pub async fn recv(&self, buf: &mut [u8]) -> Result<usize> {
        RUNTIME.with_borrow_mut(|rt| rt.recv_fut(SocketHandle::from(&self.0), buf, false)).await
    }

    pub async fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr)> {
        RUNTIME.with_borrow_mut(|rt| rt.recv_from_fut(SocketHandle::from(&self.0), buf, false)).await
    }

    pub async fn peek(&self, buf: &mut [u8]) -> Result<usize> {
        RUNTIME.with_borrow_mut(|rt| rt.recv_fut(SocketHandle::from(&self.0), buf, true)).await
    }

    pub async fn peek_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr)> {
        RUNTIME.with_borrow_mut(|rt| rt.recv_from_fut(SocketHandle::from(&self.0), buf, true)).await
    }

    pub async fn send(&self, buf: &[u8]) -> Result<usize> {
        RUNTIME.with_borrow_mut(|rt| rt.send_fut(SocketHandle::from(&self.0), buf)).await
    }

    pub async fn send_to<A: ToSocketAddrs>(&self, buf: &[u8], addr: A) -> Result<usize> {
        RUNTIME.with_borrow_mut(|rt| rt.send_to_fut(SocketHandle::from(&self.0), buf, addr)).await
    }
}