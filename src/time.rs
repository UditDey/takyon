use std::time::Duration;

use crate::RUNTIME;

pub async fn sleep(dur: Duration) {
    RUNTIME.with_borrow_mut(|rt| rt.sleep_fut(dur)).await;
}

/// Wait for the specified number of milliseconds
pub async fn sleep_millis(dur: u64) {
    sleep(Duration::from_millis(dur)).await
}

/// Wait for the specified number of microseconds
pub async fn sleep_micros(dur: u64) {
    sleep(Duration::from_micros(dur)).await
}

/// Wait for the specified number of seconds
pub async fn sleep_secs(dur: u64) {
    sleep(Duration::from_secs(dur)).await
}