use std::time::{SystemTime, UNIX_EPOCH};

pub trait Clock: Send + Sync {
    fn now_seconds(&self) -> u64;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_seconds(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs())
    }
}
