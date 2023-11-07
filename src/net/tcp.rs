use std::io::Result;
use std::net::{SocketAddr, ToSocketAddrs};

use crate::{platform, runtime::SocketHandle};

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
        platform::recv(SocketHandle::from(&self.0), buf, false).await
    }

    pub async fn write(&self, buf: &[u8]) -> Result<usize> {
        platform::send_to(SocketHandle::from(&self.0), buf, None).await
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
        let res = platform::accept(SocketHandle::from(&self.0)).await;

        // Map from std TcpStream to our own TcpStream type
        res.map(|(stream, addr)| (TcpStream(stream), addr))
    }
}