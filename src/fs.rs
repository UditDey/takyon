use std::pin::Pin;
use std::path::{Path, PathBuf};
use std::future::Future;
use std::task::{Context, Poll};

use crate::{
    Error,
    executor::{Executor, IoKey, FileHandle}
};

#[derive(Clone, Copy)]
pub struct OpenOptions {
    pub(crate) read: bool,
    pub(crate) write: bool,
    pub(crate) append: bool,
    pub(crate) truncate: bool,
    pub(crate) create: bool
}

impl OpenOptions {
    pub fn new() -> Self {
        Self {
            read: false,
            write: false,
            append: false,
            truncate: false,
            create: false
        }
    }

    pub fn read(mut self, val: bool) -> Self {
        self.read = val;
        self
    }

    pub fn write(mut self, val: bool) -> Self {
        self.write = val;
        self
    }
    
    pub fn append(mut self, val: bool) -> Self {
        self.append = val;
        self
    }
    
    pub fn truncate(mut self, val: bool) -> Self {
        self.truncate = val;
        self
    }

    pub fn create(mut self, val: bool) -> Self {
        self.create = val;
        self
    }
}

pub struct File {
    hnd: FileHandle
}

impl File {
    pub async fn open_with(exec: &Executor, opt: OpenOptions, path: impl AsRef<Path>) -> Result<Self, Error> {
        struct OpenFut<'a> {
            key: Option<IoKey>,
            path: Option<PathBuf>,
            opt: OpenOptions,
            exec: &'a Executor
        }
        
        impl<'a> OpenFut<'a> {
            pub fn new(exec: &'a Executor, opt: OpenOptions, path: PathBuf) -> Self {
                Self {
                    key: None,
                    path: Some(path),
                    opt,
                    exec
                }
            }
        }
        
        impl<'a> Future for OpenFut<'a> {
            type Output = Result<FileHandle, Error>;
        
            fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                match &self.key {
                    None => {
                        let path = self.path.take().unwrap();
                        let res = self.exec.push_file_open(&self.opt, path);

                        match res {
                            Ok(key) => {
                                self.key = Some(key);
                                Poll::Pending
                            },
                            Err(err) => Poll::Ready(Err(err))
                        }
                    },
        
                    Some(key) => {
                        match self.exec.pop_file_open(*key) {
                            Some(hnd) => Poll::Ready(hnd),
                            None => Poll::Pending
                        }
                    }
                }
            }
        }

        OpenFut::new(exec, opt, path.as_ref().to_path_buf())
            .await
            .map(|hnd| Self { hnd })
    }

    pub async fn open(exec: &Executor, path: impl AsRef<Path>) -> Result<Self, Error> {
        let opt = OpenOptions::new().read(true);
        Self::open_with(exec, opt, path).await
    }

    pub async fn create(exec: &Executor, path: impl AsRef<Path>) -> Result<Self, Error> {
        let opt = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true);

        Self::open_with(exec, opt, path).await
    }
}