//! TCP and UDP networking
//! 
//! This module provides functionality for async IO over TCP and UDP sockets, and aims
//! to provide APIs identical to the equivalent `std` types

mod udp;
mod tcp;

pub use udp::UdpSocket;
pub use tcp::{TcpListener, TcpStream};