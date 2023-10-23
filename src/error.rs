use std::io;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    // Internal errors
    // Linux errors  
    #[cfg(target_os = "linux")]
    #[error("Failed to create io_uring ({0})")]
    IoUringCreationFailed(io::Error),

    #[cfg(target_os = "linux")]
    #[error("io_uring feature `{0}` is required but not supported on current kernel")]
    IoUringFeatureNotPresent(&'static str),

    #[cfg(target_os = "linux")]
    #[error("Failed to probe supported io_uring opcodes ({0})")]
    IoUringProbeFailed(io::Error),

    #[cfg(target_os = "linux")]
    #[error("io_uring opcode `{0}` is required but not supported on current kernel")]
    IoUringOpcodeUnsupported(&'static str),

    #[cfg(target_os = "linux")]
    #[error("Failed to submit io_uring submission queue ({0})")]
    IoUringSubmitFailed(io::Error),

    // IO Errors
    #[error("IO Error ({0})")]
    IoError(io::Error)
}