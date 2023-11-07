#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub use linux::{
    sleep,
    socket_create,
    socket_close,
    socket_connect,
    socket_shutdown,
    socket_recv,
    socket_recv_from,
    socket_send,
    socket_send_to,
    socket_accept,
    Platform
};