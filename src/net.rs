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
        socket_recv_from,
        socket_send,
        socket_send_to,
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
    /// 
    /// # Blocking Warning
    /// When getting addresses from the provided `ToSocketAddrs` object, DNS lookups may be performed,
    /// which can block the thread and prevent other tasks from executing. To prevent this, pass only
    /// resolved exact IP addresses and not strings
    /// ```
    /// // Any other `SocketAddr::from()` or `ToSocketAddrs` impl can also be used as long as
    /// // it doesn't perform a DNS lookup
    /// TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], 5000))).await;
    /// ```
    pub async fn connect<A: ToSocketAddrs>(addr: A) -> Result<Self> {
        let addr_iter = addr.to_socket_addrs().expect("Couldn't get address iterator");

        // Since each address can be either IPv4 or IPv6, we create one stream for each type
        let (stream_v4, stream_v6) = try_zip(socket_create::<StdTcpStream>(false, false), socket_create::<StdTcpStream>(true, false)).await?;

        // Prevent the streams from being auto dropped since we close them manually and
        // don't want double closes
        let stream_v4 = ManuallyDrop::new(stream_v4);
        let stream_v6 = ManuallyDrop::new(stream_v6);

        let mut res = None;

        for addr in addr_iter {
            match addr {
                SocketAddr::V4(_) => {
                    match socket_connect(&*stream_v4, &addr).await {
                        Ok(()) => {
                            socket_close(&*stream_v6);
                            stream_v4.set_nonblocking(true)?;
                            return Ok(Self(stream_v4));
                        },

                        Err(err) => res = Some(err)
                    }
                },

                SocketAddr::V6(_) => {
                    match socket_connect(&*stream_v6, &addr).await {
                        Ok(()) => {
                            socket_close(&*stream_v4);
                            stream_v6.set_nonblocking(true)?;
                            return Ok(Self(stream_v6));
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
    /// 
    /// # Blocking Warning
    /// When getting addresses from the provided `ToSocketAddrs` object, DNS lookups may be performed,
    /// which can block the thread and prevent other tasks from executing. To prevent this, pass only
    /// resolved exact IP addresses and not strings
    /// ```
    /// // Any other `SocketAddr::from()` or `ToSocketAddrs` impl can also be used as long as
    /// // it doesn't perform a DNS lookup
    /// TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 5000))).await;
    /// ```
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
    /// 
    /// # Blocking Warning
    /// When getting addresses from the provided `ToSocketAddrs` object, DNS lookups may be performed,
    /// which can block the thread and prevent other tasks from executing. To prevent this, pass only
    /// resolved exact IP addresses and not strings
    /// ```
    /// // Any other `SocketAddr::from()` or `ToSocketAddrs` impl can also be used as long as
    /// // it doesn't perform a DNS lookup
    /// UdpSocket::bind(SocketAddr::from(([127, 0, 0, 1], 5000))).await;
    /// ```
    pub async fn bind<A: ToSocketAddrs>(addr: A) -> Result<Self> {
        let sock = std::net::UdpSocket::bind(addr)?;
        sock.set_nonblocking(true)?;

        Ok(Self(ManuallyDrop::new(sock)))
    }

    /// Connects this UDP socket to a remote address, allowing the send and recv syscalls to be used
    /// to send data and also applies filters to only receive data from the specified address
    /// 
    /// See the [`std` docs](std::net::UdpSocket::connect) for more info
    /// 
    /// # Blocking Warning
    /// When getting addresses from the provided `ToSocketAddrs` object, DNS lookups may be performed,
    /// which can block the thread and prevent other tasks from executing. To prevent this, pass only
    /// resolved exact IP addresses and not strings
    /// ```
    /// // Any other `SocketAddr::from()` or `ToSocketAddrs` impl can also be used as long as
    /// // it doesn't perform a DNS lookup
    /// socket.connect(SocketAddr::from(([127, 0, 0, 1], 5000))).await;
    /// ```
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