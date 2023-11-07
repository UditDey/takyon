use std::io::Result;
use std::net::{SocketAddr, ToSocketAddrs};

use crate::{platform, runtime::SocketHandle};

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
        platform::recv(SocketHandle::from(&self.0), buf, false).await
    }

    pub async fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr)> {
        platform::recv_from(SocketHandle::from(&self.0), buf, false).await
    }

    pub async fn peek(&self, buf: &mut [u8]) -> Result<usize> {
        platform::recv(SocketHandle::from(&self.0), buf, true).await
    }

    pub async fn peek_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr)> {
        platform::recv_from(SocketHandle::from(&self.0), buf, true).await
    }

    pub async fn send(&self, buf: &[u8]) -> Result<usize> {
        platform::send_to(SocketHandle::from(&self.0), buf, None).await
    }

    pub async fn send_to<A: ToSocketAddrs>(&self, buf: &[u8], addr: A) -> Result<usize> {
        let addr = addr
            .to_socket_addrs()
            .expect("Could not get address iterator")
            .next()
            .expect("Address iterator didn't provide any addresses");

        platform::send_to(SocketHandle::from(&self.0), buf, Some(addr)).await
    }
}