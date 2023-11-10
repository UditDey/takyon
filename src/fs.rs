//! Filesystem manipulation operations
//! 
//! This module provides functionality for async IO for filesystem operations,
//! and aims to provide APIs identical to the equivalent `std` types

use std::path::Path;
use std::io::Result;
use std::mem::ManuallyDrop;

use crate::platform::{file_open, file_read, file_write, file_close};

/// Options and flags which can be used to configure how a file is opened
/// 
/// To use this, call [`OpenOptions::new()`], use the builder API to set the
/// configuration options and then pass into [`File::open()`]. This provides
/// similar options as [`std::fs::OpenOptions`]
pub struct OpenOptions {
    pub(crate) read: bool,
    pub(crate) write: bool,
    pub(crate) append: bool,
    pub(crate) truncate: bool,
    pub(crate) create: bool,
    pub(crate) create_new: bool
}


impl OpenOptions {
    /// Creates a blank set of options ready for configuration
    /// 
    /// All options are initially set to `false`
    pub fn new() -> Self {
        Self {
            read: false,
            write: false,
            append: false,
            truncate: false,
            create: false,
            create_new: false,
        }
    }

    /// Set the option for read access
    /// 
    /// This option, when true, will indicate that the file should be read-able if opened
    pub fn read(mut self, read: bool) -> Self {
        self.read = read;
        self
    }

    /// Sets the option for write access
    /// 
    /// This option, when true, will indicate that the file should be write-able if opened
    /// 
    /// If the file already exists, any write calls on it will overwrite its contents, without truncating it
    pub fn write(mut self, write: bool) -> Self {
        self.write = write;
        self
    }
    
    /// Sets the option for the append mode
    /// 
    /// This option, when true, means that writes will append to a file instead of overwriting previous
    /// contents. Note that setting `.write(true).append(true)` has the same effect as setting only
    /// `.append(true)`
    /// 
    /// See the [`std` docs](std::fs::OpenOptions::append) for more info
    pub fn append(self, append: bool) -> Self {
        let mut this = self.write(true);
        this.append = append;
        this
    }
    
    /// Sets the option for truncating a previous file
    ///
    /// If a file is successfully opened with this option set it will truncate the file to 0 length
    /// if it already exists
    ///
    /// The file must be opened with write access for truncate to work
    pub fn truncate(mut self, truncate: bool) -> Self {
        self.truncate = truncate;
        self
    }
    
    /// Sets the option to create a new file, or open it if it already exists
    ///
    /// In order for the file to be created, `OpenOptions::write` or `OpenOptions::append` access must
    /// be used
    pub fn create(mut self, create: bool) -> Self {
        self.create = create;
        self
    }
    
    /// Sets the option to create a new file, failing if it already exists
    /// 
    /// No file is allowed to exist at the target location, also no (dangling) symlink. In this way,
    /// if the call succeeds, the file returned is guaranteed to be new
    /// 
    /// See the [`std` docs](std::fs::OpenOptions::create_new) for more info
    pub fn create_new(mut self, create_new: bool) -> Self {
        self.create_new = create_new;
        self
    }
}

/// An object providing access to an open file on the filesystem
/// 
/// This internally contains a regular [`std::fs::File`], additionally providing async
/// IO functions for it. The provided async functions work identically to their sync counterparts,
/// so refer to their docs for more details
/// 
/// The underlying [`std::fs::File`] can be accessed using [`std()`](File::std), which
/// is useful for accessing the various configuration options that are not exposed by this type.
/// 
/// Do not accidentally use any of the underlying blocking functions!
pub struct File(ManuallyDrop<std::fs::File>);

impl File {
    /// Attempts to open a file at the given path with the given [`OpenOptions`]
    pub async fn open<T: AsRef<Path>>(path: T, opts: &OpenOptions) -> Result<Self> {
        file_open(path.as_ref(), opts)
            .await
            .map(|file| Self(ManuallyDrop::new(file)))
    }

    /// Get a reference to the underlying [`std::fs::File`]
    /// 
    /// This is useful to access the various configuration options not exposed by this type.
    /// Be careful not to accidentally call any blocking functions on the underlying file,
    /// since it will block other tasks from executing
    pub fn std(&self) -> &std::fs::File {
        &self.0
    }

    /// Pull some bytes from this stream into the specified buffer, returning how many bytes
    /// were read
    /// 
    /// If the returned value is zero, it means that the end of file was reached and no new
    /// bytes are available to read. This may change if more data is appended to it in the
    /// meantime
    /// 
    /// This is equivalent to the [`Read`](std::io::Read) impl on [`std::fs::File`].
    /// See the [`std` docs](std::io::Read::read) for more info
    pub async fn read(&self, buf: &mut [u8]) -> Result<usize> {
        file_read(&self.0, buf).await
    }

    /// Write a buffer into this stream, returning how many bytes were written
    /// 
    /// This is equivalent to the [`Write`](std::io::Write) impl on [`std::fs::File`].
    /// See the [`std` docs](std::io::Write::write) for more info
    pub async fn write(&self, buf: &[u8]) -> Result<usize> {
        file_write(&self.0, buf).await
    }
}

impl Drop for File {
    fn drop(&mut self) {
        file_close(&self.0);
    }
}