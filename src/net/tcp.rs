use std::io::Result;
use std::mem::ManuallyDrop;
use std::net::{SocketAddr, ToSocketAddrs, TcpStream as StdTcpStream};

use crate::{
    util::try_zip,
    platform::{
        socket_create,
        socket_close,
        socket_connect,
        socket_recv,
        socket_send,
        socket_accept
    }
};

pub struct TcpStream(ManuallyDrop<std::net::TcpStream>);

impl TcpStream {
    pub async fn connect<A: ToSocketAddrs>(addr: A) -> Result<Self> {
        let addr_iter = addr.to_socket_addrs().expect("Couldn't get address iterator");

        // Since each address can be either IPv4 or IPv6, we create one socket for each type
        let (sock_v4, sock_v6) = try_zip(
            async {
                let stream = socket_create::<StdTcpStream>(false, false).await;
                stream.map(|stream| ManuallyDrop::new(stream))
            },
            
            async {
                let stream = socket_create::<StdTcpStream>(true, false).await;
                stream.map(|stream| ManuallyDrop::new(stream))
            },
        ).await?;

        let mut res = None;

        for addr in addr_iter {
            match addr {
                SocketAddr::V4(_) => {
                    match socket_connect(&*sock_v4, &addr).await {
                        Ok(()) => {
                            socket_close(&*sock_v6);
                            sock_v4.set_nonblocking(true)?;
                            return Ok(Self(sock_v4));
                        },

                        Err(err) => res = Some(err)
                    }
                },

                SocketAddr::V6(_) => {
                    match socket_connect(&*sock_v6, &addr).await {
                        Ok(()) => {
                            socket_close(&*sock_v4);
                            sock_v6.set_nonblocking(true)?;
                            return Ok(Self(sock_v6));
                        },

                        Err(err) => res = Some(err)
                    }
                }
            }
        }

        match res {
            Some(err) => Err(err),
            None => panic!("Address iterator didn't provide any addresses")
        }
    }

    pub fn std(&self) -> &std::net::TcpStream {
        &self.0
    }

    pub async fn read(&self, buf: &mut [u8]) -> Result<usize> {
        socket_recv(&*self.0, buf, false).await
    }

    pub async fn write(&self, buf: &[u8]) -> Result<usize> {
        socket_send(&*self.0, buf).await
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        socket_close(&*self.0);
    }
}

pub struct TcpListener(ManuallyDrop<std::net::TcpListener>);

impl TcpListener {
    pub async fn bind<A: ToSocketAddrs>(addr: A) -> Result<Self> {
        let listener = std::net::TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;

        Ok(Self(ManuallyDrop::new(listener)))
    }

    pub fn std(&self) -> &std::net::TcpListener {
        &self.0
    }

    pub async fn accept(&self) -> Result<(TcpStream, SocketAddr)> {
        let res = socket_accept(&*self.0).await;

        // Map from std TcpStream to our own TcpStream type
        res.map(|(stream, addr)| (TcpStream(ManuallyDrop::new(stream)), addr))
    }
}

impl Drop for TcpListener {
    fn drop(&mut self) {
        socket_close(&*self.0);
    }
}