use std::{collections::BTreeMap, net::IpAddr};

use crate::LockoutConfig;

#[derive(Debug)]
pub(crate) struct LockoutStore {
    config: LockoutConfig,
    entries: BTreeMap<IpAddr, LockoutEntry>,
}

#[derive(Debug, Clone, Copy)]
struct LockoutEntry {
    failures: u32,
    first_failure: u64,
    last_seen: u64,
    locked_until: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockState {
    Allowed,
    Locked { retry_after: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FailureState {
    pub(crate) remaining_before_lock: u32,
    pub(crate) lock_state: LockState,
}

impl LockoutStore {
    pub(crate) const fn new(config: LockoutConfig) -> Self {
        Self {
            config,
            entries: BTreeMap::new(),
        }
    }

    pub(crate) fn check(&mut self, peer: IpAddr, now: u64) -> LockState {
        self.prune(now);
        self.entries
            .get(&peer)
            .and_then(|entry| entry.locked_until)
            .filter(|locked_until| *locked_until > now)
            .map_or(LockState::Allowed, |locked_until| LockState::Locked {
                retry_after: locked_until.saturating_sub(now),
            })
    }

    pub(crate) fn record_failure(&mut self, peer: IpAddr, now: u64) -> FailureState {
        self.prune(now);
        if !self.entries.contains_key(&peer) && self.entries.len() >= self.config.capacity {
            self.evict_oldest();
        }
        let entry = self.entries.entry(peer).or_insert(LockoutEntry {
            failures: 0,
            first_failure: now,
            last_seen: now,
            locked_until: None,
        });
        if now.saturating_sub(entry.first_failure) >= self.config.window.as_secs() {
            *entry = LockoutEntry {
                failures: 0,
                first_failure: now,
                last_seen: now,
                locked_until: None,
            };
        }
        entry.failures = entry.failures.saturating_add(1);
        entry.last_seen = now;
        if entry.failures >= self.config.threshold {
            let locked_until = now.saturating_add(self.config.lock_duration.as_secs());
            entry.locked_until = Some(locked_until);
            return FailureState {
                remaining_before_lock: 0,
                lock_state: LockState::Locked {
                    retry_after: locked_until.saturating_sub(now),
                },
            };
        }
        FailureState {
            remaining_before_lock: self.config.threshold.saturating_sub(entry.failures),
            lock_state: LockState::Allowed,
        }
    }

    pub(crate) fn record_success(&mut self, peer: IpAddr) {
        self.entries.remove(&peer);
    }

    fn prune(&mut self, now: u64) {
        let window = self.config.window.as_secs();
        self.entries.retain(|_, entry| match entry.locked_until {
            Some(locked_until) => locked_until > now,
            None => now.saturating_sub(entry.first_failure) < window,
        });
    }

    fn evict_oldest(&mut self) {
        let oldest = self
            .entries
            .iter()
            .min_by_key(|(peer, entry)| (entry.last_seen, **peer))
            .map(|(peer, _)| *peer);
        if let Some(peer) = oldest {
            self.entries.remove(&peer);
        }
    }
}
