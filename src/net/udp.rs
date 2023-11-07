use std::io::Result;
use std::net::{SocketAddr, ToSocketAddrs};

use crate::platform::{
    socket_recv,
    socket_recv_from,
    socket_send,
    socket_send_to
};

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
        socket_recv(&self.0, buf, false).await
    }

    pub async fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr)> {
        socket_recv_from(&self.0, buf, false).await
    }

    pub async fn peek(&self, buf: &mut [u8]) -> Result<usize> {
        socket_recv(&self.0, buf, true).await
    }

    pub async fn peek_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr)> {
        socket_recv_from(&self.0, buf, true).await
    }

    pub async fn send(&self, buf: &[u8]) -> Result<usize> {
        socket_send(&self.0, buf).await
    }

    pub async fn send_to<A: ToSocketAddrs>(&self, buf: &[u8], addr: A) -> Result<usize> {
        let addr = addr
            .to_socket_addrs()
            .expect("Couldn't get address iterator")
            .next()
            .expect("Address iterator didn't provide any addresses");

        socket_send_to(&self.0, buf, &addr).await
    }
}