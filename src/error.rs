use std::io;
use std::error::Error;
use std::fmt::{self, Display, Formatter};

#[cfg(target_os = "linux")]
#[derive(Debug)]
pub enum InitError {
    IoUringCreationFailed(io::Error),
    IoUringFeatureNotPresent(&'static str),
    IoUringProbeFailed(io::Error),
    IoUringOpcodeUnsupported(&'static str)
}

#[cfg(target_os = "linux")]
impl Display for InitError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::IoUringCreationFailed(err) => write!(f, "Failed to create io_uring ({0})", err),
            Self::IoUringFeatureNotPresent(msg) => write!(f, "io_uring feature `{0}` is required but not supported on current kernel", msg),
            Self::IoUringProbeFailed(err) => write!(f, "Failed to probe supported io_uring opcodes ({0})", err),
            Self::IoUringOpcodeUnsupported(msg) => write!(f, "io_uring opcode  `{0}` is required but not supported on current kernel", msg)
        }
    }
}

impl Error for InitError {}