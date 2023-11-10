use std::path::Path;
use std::io::Result;
use std::fs::File as StdFile;

use crate::platform::file_open;

pub struct OpenOptions {
    pub(crate) read: bool,
    pub(crate) write: bool,
    pub(crate) append: bool,
    pub(crate) truncate: bool,
    pub(crate) create: bool,
    pub(crate) create_new: bool
}

impl OpenOptions {
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

    pub fn read(mut self, read: bool) -> Self {
        self.read = read;
        self
    }

    pub fn write(mut self, write: bool) -> Self {
        self.write = write;
        self
    }
    
    pub fn append(self, append: bool) -> Self {
        let mut this = self.write(true);
        this.append = append;
        this
    }
    
    pub fn truncate(mut self, truncate: bool) -> Self {
        self.truncate = truncate;
        self
    }
    
    pub fn create(mut self, create: bool) -> Self {
        self.create = create;
        self
    }
    
    pub fn create_new(mut self, create_new: bool) -> Self {
        self.create_new = create_new;
        self
    }
}

pub struct File(StdFile);

impl File {
    pub async fn open<T: AsRef<Path>>(path: T, opts: &OpenOptions) -> Result<Self> {
        file_open::<StdFile>(path.as_ref(), opts).await.map(|file| Self(file))
    }
}