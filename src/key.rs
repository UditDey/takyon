use std::marker::PhantomData;
use std::cmp::{Eq, PartialEq};
use std::hash::{Hash, Hasher};

// Provides the IoKey and TaskId types
// The idea behind these types (and why they are not simply type aliases to u32) is that
// they are !Send + !Sync, which will propagate to all futures in this crate
// Its achieved by storing a PhantomData<*const ()> in both types (*const is !Send + !Sync)
// This is important since we're a single threaded runtime and using a future from one
// thread in another will cause issues

#[derive(Clone, Copy)]
pub struct IoKey {
    pub inner: u32,
    phantom: PhantomData<*const ()>
}

impl From<u32> for IoKey {
    fn from(value: u32) -> Self {
        Self { inner: value, phantom: PhantomData }
    }
}

impl PartialEq for IoKey {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl Eq for IoKey {}

impl Hash for IoKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u32(self.inner);
    }
}

impl nohash::IsEnabled for IoKey {}


#[derive(Clone, Copy)]
pub struct TaskId {
    pub inner: u32,
    phantom: PhantomData<*const ()>
}

impl From<u32> for TaskId {
    fn from(value: u32) -> Self {
        Self { inner: value, phantom: PhantomData }
    }
}

impl PartialEq for TaskId {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl Eq for TaskId {}

impl Hash for TaskId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u32(self.inner);
    }
}

impl nohash::IsEnabled for TaskId {}