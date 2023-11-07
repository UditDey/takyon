use std::io::Result;
use std::mem::ManuallyDrop;
use std::net::{SocketAddr, ToSocketAddrs, Shutdown, TcpStream as StdTcpStream};

use crate::{
    util::try_zip,
    platform::{
        socket_create,
        socket_close,
        socket_connect,
        socket_shutdown,
        socket_recv,
        socket_send,
        socket_accept
    }
};

/// A TCP stream between a local and a remote socket
/// 
/// This internally contains a regular [`std::net::TcpStream`], additionally providing async
/// IO functions for it. The provided async functions work identically to their sync counterparts,
/// so refer to their docs for more details
/// 
/// The underlying [`std::net::TcpStream`] can be accessed using [`std()`](TcpStream::std), which
/// is useful for accessing the various configuration options that are not exposed by this type.
/// 
/// Do not accidentally use any of the underlying blocking functions!
pub struct TcpStream(ManuallyDrop<std::net::TcpStream>);

impl TcpStream {
    /// Opens a TCP connection to a remote host
    /// 
    /// See the [`std` docs](std::net::TcpStream::connect) for more info
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

    /// Get a reference to the underlying [`std::net::TcpStream`]
    /// 
    /// This is useful to access the various configuration options not exposed by this type.
    /// Be careful not to accidentally call any blocking functions on the underlying stream,
    /// since it will block other tasks from executing
    pub fn std(&self) -> &std::net::TcpStream {
        &self.0
    }

    /// Pull some bytes from this stream into the specified buffer, returning how many bytes
    /// were read
    /// 
    /// If the returned value is zero, it means that the remote host gracefully closed the
    /// connection
    /// 
    /// This is equivalent to the [`Read`](std::io::Read) impl on [`std::net::TcpStream`].
    /// See the [`std` docs](std::io::Read::read) for more info
    pub async fn read(&self, buf: &mut [u8]) -> Result<usize> {
        socket_recv(&*self.0, buf, false).await
    }

    /// Write a buffer into this stream, returning how many bytes were written
    /// 
    /// This is equivalent to the [`Write`](std::io::Write) impl on [`std::net::TcpStream`].
    /// See the [`std` docs](std::io::Write::write) for more info
    pub async fn write(&self, buf: &[u8]) -> Result<usize> {
        socket_send(&*self.0, buf).await
    }

    /// Shuts down the read, write, or both halves of this connection
    /// 
    /// See the [`std` docs](std::net::TcpStream::shutdown) for more info
    pub async fn shutdown(&self, how: Shutdown) -> Result<()> {
        socket_shutdown(&*self.0, how).await
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        socket_close(&*self.0);
    }
}

/// A TCP socket server, listening for connections
/// 
/// This internally contains a regular [`std::net::TcpListener`], additionally providing async
/// IO functions for it. The provided async functions work identically to their sync counterparts,
/// so refer to their docs for more details
/// 
/// The underlying [`std::net::TcpListener`] can be accessed using [`std()`](TcpListener::std), which
/// is useful for accessing the various configuration options that are not exposed by this type.
/// 
/// Do not accidentally use any of the underlying blocking functions!
pub struct TcpListener(ManuallyDrop<std::net::TcpListener>);

impl TcpListener {
    /// Creates a new [`TcpListener`] which will be bound to the specified address
    /// 
    /// The returned listener is ready for accepting connections
    /// 
    /// See the [`std` docs](std::net::TcpListener::bind) for more info
    pub async fn bind<A: ToSocketAddrs>(addr: A) -> Result<Self> {
        let listener = std::net::TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;

        Ok(Self(ManuallyDrop::new(listener)))
    }

    /// Get a reference to the underlying [`std::net::TcpListener`]
    /// 
    /// This is useful to access the various configuration options not exposed by this type.
    /// Be careful not to accidentally call any blocking functions on the underlying listener,
    /// since it will block other tasks from executing
    pub fn std(&self) -> &std::net::TcpListener {
        &self.0
    }

    /// Accept a new incoming connection from this listener, returning the corresponding
    /// [`TcpStream`] and the remote peer's address
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