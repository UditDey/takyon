use std::pin::Pin;
use std::future::Future;
use std::time::Duration;
use std::task::{Context, Poll};

use crate::{RUNTIME, key::IoKey};

/// Future which waits for a certain duration of time
pub struct Sleep {
    dur: Duration,
    key: Option<IoKey>
}

impl Sleep {
    pub fn new(dur: Duration) -> Self {
        Self { dur, key: None }
    }
}

impl Future for Sleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.key {
            // Timeout has not been pushed yet
            None => {
                self.key = Some(RUNTIME.with_borrow_mut(|rt| rt.push_timeout(self.dur)));
                Poll::Pending
            },

            // Timeout has been pushed, try to pop it to see if it's completed
            Some(key) => {
                if RUNTIME.with_borrow_mut(|rt| rt.pop_timeout(key)) {
                    self.key = None;
                    Poll::Ready(())
                }
                else {
                    Poll::Pending
                }
            }
        }
    }
}

impl Drop for Sleep {
    fn drop(&mut self) {
        if let Some(key) = self.key {
            RUNTIME.with_borrow_mut(|rt| rt.cancel_timeout(key));
        }
    }
}

/// Wait for the specified number of milliseconds
/// 
/// Convenience function for [`Sleep`]
pub async fn sleep_millis(dur: u64) {
    Sleep::new(Duration::from_millis(dur)).await
}

/// Wait for the specified number of milliseconds
/// 
/// Convenience function for [`Sleep`]
pub async fn sleep_micros(dur: u64) {
    Sleep::new(Duration::from_micros(dur)).await
}

/// Wait for the specified number of milliseconds
/// 
/// Convenience function for [`Sleep`]
pub async fn sleep_secs(dur: u64) {
    Sleep::new(Duration::from_secs(dur)).await
}