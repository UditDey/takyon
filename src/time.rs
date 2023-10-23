use std::pin::Pin;
use std::time::Duration;
use std::future::Future;
use std::task::{Context, Poll};

use crate::{
    Error,
    executor::{Executor, IoKey}
};

pub struct Sleep<'a> {
    key: Option<IoKey>,
    dur: Duration,
    exec: &'a Executor
}

impl<'a> Sleep<'a> {
    pub fn new(exec: &'a Executor, dur: Duration) -> Self {
        Self { key: None, dur, exec }
    }
}

impl<'a> Future for Sleep<'a> {
    type Output = Result<(), Error>;

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        match &self.key {
            None => {
                let res = self.exec.push_timeout(self.dur);

                match res {
                    Ok(key) => {
                        self.key = Some(key);
                        Poll::Pending
                    },
                    Err(err) => Poll::Ready(Err(err))
                }
            },

            Some(key) => {
                if self.exec.pop_timeout(*key) {
                    self.key = None;
                    Poll::Ready(Ok(()))
                }
                else {
                    Poll::Pending
                }
            }
        }
    }
}

impl<'a> Drop for Sleep<'a> {
    fn drop(&mut self) {
        if let Some(io_key) = self.key {
            self.exec.cancel_timeout(io_key).expect("Failed to cancel timeout");
        }
    }
}

pub async fn sleep(exec: &Executor, ms: u64) -> Result<(), Error> {
    Sleep::new(exec, Duration::from_millis(ms)).await
}