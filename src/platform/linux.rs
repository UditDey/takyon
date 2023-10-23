use std::io;
use std::path::PathBuf;
use std::time::Duration;
use std::os::unix::ffi::OsStringExt;

use io_uring::{
    opcode,
    squeue,
    IoUring,
    Probe,
    types::{Timespec, Fd}
};

use slotmap::{SlotMap, Key, KeyData};

use crate::{
    error::Error,
    fs::OpenOptions,
    executor::{IoKey, FileHandle}
};

enum IoMapEntry {
    Timeout(bool),
    FileOpen(Option<Result<FileHandle, io::Error>>)
}

pub struct Platform {
    ring: IoUring,
    io_map: SlotMap<IoKey, IoMapEntry>,
    timespec_store: Vec<Box<Timespec>>,
    path_store: Vec<Vec<u8>>
}

impl Platform {
    pub fn new() -> Result<Self, Error> {
        // Create io_uring
        let ring = IoUring::new(128).map_err(|err| Error::IoUringCreationFailed(err))?;

        // Check required features
        if !ring.params().is_feature_submit_stable() {
            return Err(Error::IoUringFeatureNotPresent("submit_stable"));
        }

        if !ring.params().is_feature_nodrop() {
            return Err(Error::IoUringFeatureNotPresent("no_drop"));
        }

        // Probe supported opcodes
        let mut probe = Probe::new();

        ring.submitter()
            .register_probe(&mut probe)
            .map_err(|err| Error::IoUringProbeFailed(err))?;

        // Check required opcodes
        let req_opcodes = [
            ("Timeout", opcode::Timeout::CODE),
            ("OpenAt", opcode::OpenAt::CODE)
        ];

        for (name, code) in req_opcodes {
            if !probe.is_supported(code) {
                return Err(Error::IoUringOpcodeUnsupported(name));
            }
        }

        Ok(Self {
            ring,
            io_map: SlotMap::with_key(),
            timespec_store: Vec::new(),
            path_store: Vec::new()
        })
    }

    fn submit_sqe(&mut self, sqe: squeue::Entry) -> Result<(), Error> {
        loop {
            let res = unsafe {
                self.ring
                    .submission()
                    .push(&sqe)
            };

            match res {
                Ok(()) => break Ok(()),
                Err(_) => {
                    self.ring
                        .submit()
                        .map_err(|err| Error::IoUringSubmitFailed(err))?;

                    self.clear_stores();
                }
            }
        }
    }

    fn clear_stores(&mut self) {
        self.timespec_store.clear();
        self.path_store.clear();
    }


    // Timeouts
    pub fn push_timeout(&mut self, dur: Duration) -> Result<IoKey, Error> {
        let key = self.io_map.insert(IoMapEntry::Timeout(false));
        let timespec = Box::new(Timespec::from(dur));

        let sqe = opcode::Timeout::new(timespec.as_ref())
            .build()
            .user_data(key.data().as_ffi());

        self.timespec_store.push(timespec);
        self.submit_sqe(sqe)?;

        Ok(key)
    }

    pub fn cancel_timeout(&mut self, key: IoKey) -> Result<(), Error> {
        let sqe = opcode::TimeoutRemove::new(key.data().as_ffi()).build();

        self.submit_sqe(sqe)?;
        self.io_map.remove(key);

        Ok(())
    }

    pub fn pop_timeout(&mut self, key: IoKey) -> bool {
        self.io_map
            .remove(key)
            .map(|entry| match entry {
                IoMapEntry::Timeout(state) => state,
                _ => panic!("IoMapEntry in unexpected state")
            })
            .expect("Couldn't find IoMapEntry for this key")
    }


    // File Opens
    pub fn push_file_open(&mut self, opt: &OpenOptions, path: PathBuf) -> Result<IoKey, Error> {
        let key = self.io_map.insert(IoMapEntry::FileOpen(None));
        
        let mut flags = if opt.read && !opt.write {
            libc::O_RDONLY
        }
        else if !opt.read && opt.write {
            libc::O_WRONLY
        }
        else if opt.read && opt.write {
            libc::O_RDWR
        }
        else {
            return Err(Error::IoError(io::Error::from(io::ErrorKind::InvalidInput)));
        };

        if opt.append {
            flags |= libc::O_APPEND;
        }

        if opt.truncate {
            flags |= libc::O_TRUNC;
        }

        if opt.create {
            flags |= libc::O_CREAT;
        }

        let path = path
            .into_os_string()
            .into_vec();

        let dirfd = Fd(libc::AT_FDCWD);

        let sqe = opcode::OpenAt::new(dirfd, path.as_ptr() as *const _)
            .flags(flags)
            .mode(0o666)
            .build()
            .user_data(key.data().as_ffi());

        self.path_store.push(path);
        self.submit_sqe(sqe)?;

        Ok(key)
    }

    pub fn pop_file_open(&mut self, key: IoKey) -> Option<Result<FileHandle, Error>> {
        self.io_map
            .remove(key)
            .map(|entry| match entry {
                IoMapEntry::FileOpen(res) => res.map(|res| res.map_err(|err| Error::IoError(err))),
                _ => panic!("IoMapEntry in unexpected state")
            })
            .expect("Couldn't find IoMapEntry for this key")
    }


    pub fn wait_for_events(&mut self) -> Result<(), Error> {
        self.ring
            .submit_and_wait(1)
            .map_err(|err| Error::IoUringSubmitFailed(err))?;

        self.clear_stores();

        for cqe in self.ring.completion() {
            let key = IoKey::from(KeyData::from_ffi(cqe.user_data()));
            let entry = self.io_map.get_mut(key);

            if let Some(entry) = entry {
                match entry {
                    IoMapEntry::Timeout(state) => *state = true,
                    IoMapEntry::FileOpen(res) => *res = Some(fd_to_result(cqe.result()))
                }
            }
        }

        Ok(())
    }
}

fn fd_to_result(fd: i32) -> Result<i32, io::Error> {
    if fd >= 0 {
        Ok(fd)
    }
    else {
        Err(io::Error::from_raw_os_error(-fd))
    }
}