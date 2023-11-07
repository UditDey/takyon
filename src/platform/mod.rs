#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub use linux::{sleep, recv, recv_from, send_to, accept, Platform};