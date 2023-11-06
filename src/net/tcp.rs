use std::io::Result;
use std::net::{SocketAddr, ToSocketAddrs};

use crate::{RUNTIME, runtime::SocketHandle};

pub struct TcpStream(std::net::TcpStream);

impl TcpStream {
    pub fn connect<A: ToSocketAddrs>(addr: A) -> Result<Self> {
        let stream = std::net::TcpStream::connect(addr)?;
        stream.set_nonblocking(true)?;

        Ok(Self(stream))
    }

    pub fn std(&self) -> &std::net::TcpStream {
        &self.0
    }

    pub async fn read(&self, buf: &mut [u8]) -> Result<usize> {
        RUNTIME.with_borrow_mut(|rt| rt.recv_fut(SocketHandle::from(&self.0), buf, false)).await
    }

    pub async fn write(&self, buf: &[u8]) -> Result<usize> {
        RUNTIME.with_borrow_mut(|rt| rt.send_fut(SocketHandle::from(&self.0), buf)).await
    }
}

pub struct TcpListener(std::net::TcpListener);

impl TcpListener {
    pub fn bind<A: ToSocketAddrs>(addr: A) -> Result<Self> {
        let listener = std::net::TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;

        Ok(Self(listener))
    }

    pub fn std(&self) -> &std::net::TcpListener {
        &self.0
    }

    pub async fn accept(&self) -> Result<(TcpStream, SocketAddr)> {
        let res = RUNTIME.with_borrow_mut(|rt| rt.accept_fut(SocketHandle::from(&self.0))).await;

        // Map from std TcpStream to our own TcpStream type
        res.map(|(stream, addr)| (TcpStream(stream), addr))
    }
}