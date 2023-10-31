use std::io::Result;
use std::net::SocketAddr;
use std::net::ToSocketAddrs;
use std::net::UdpSocket as StdUdpSocket;
use std::os::fd::AsRawFd;

use crate::RUNTIME;

pub struct UdpSocket(StdUdpSocket);

impl UdpSocket {
    pub fn bind<A: ToSocketAddrs>(addr: A) -> Result<Self> {
        let socket = StdUdpSocket::bind(addr)?;
        socket.set_nonblocking(true)?;

        Ok(Self(socket))
    }

    pub async fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr)> {
        RUNTIME.with_borrow_mut(|rt| rt.recv_from_fut(self.0.as_raw_fd(), buf)).await
    }

    pub fn broadcast(&self) -> Result<bool> {
        self.0.broadcast()
    }

    pub fn connect<A: ToSocketAddrs>(&self, addr: A) -> Result<()> {
        self.0.connect(addr)
    }
}