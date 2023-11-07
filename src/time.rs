//! Time related functionality

use std::time::Duration;

use crate::platform;

/// Waits for the specified duration
pub async fn sleep(dur: Duration) {
    platform::sleep(dur).await;
}

/// Waits for the specified number of milliseconds
pub async fn sleep_millis(dur: u64) {
    sleep(Duration::from_millis(dur)).await
}

/// Waits for the specified number of microseconds
pub async fn sleep_micros(dur: u64) {
    sleep(Duration::from_micros(dur)).await
}

/// Waits for the specified number of seconds
pub async fn sleep_secs(dur: u64) {
    sleep(Duration::from_secs(dur)).await
}