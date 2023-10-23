use std::path::PathBuf;
use std::cell::RefCell;
use std::time::Duration;

use slotmap::new_key_type;

use crate::{
    error::Error,
    platform::Platform,
    fs::OpenOptions
};

new_key_type! {
    pub struct IoKey;
}

#[cfg(target_os = "linux")]
pub type FileHandle = std::os::fd::RawFd;

pub struct Executor {
    plat: RefCell<Platform>
}

impl Executor {
    pub fn new() -> Result<Self, Error> {
        let plat = Platform::new()?;

        Ok(Self { plat: RefCell::new(plat) })
    }


    // Timeouts
    pub fn push_timeout(&self, dur: Duration) -> Result<IoKey, Error> {
        self.plat.borrow_mut().push_timeout(dur)
    }

    pub fn cancel_timeout(&self, key: IoKey) -> Result<(), Error> {
        self.plat.borrow_mut().cancel_timeout(key)
    }

    pub fn pop_timeout(&self, key: IoKey) -> bool {
        self.plat.borrow_mut().pop_timeout(key)
    }


    // File Opens
    pub fn push_file_open(&self, opt: &OpenOptions, path: PathBuf) -> Result<IoKey, Error> {
        self.plat.borrow_mut().push_file_open(opt, path)
    }

    pub fn pop_file_open(&self, key: IoKey) -> Option<Result<FileHandle, Error>> {
        self.plat.borrow_mut().pop_file_open(key)
    }


    pub fn wait_for_events(&self) -> Result<(), Error> {
        self.plat.borrow_mut().wait_for_events()
    }
}