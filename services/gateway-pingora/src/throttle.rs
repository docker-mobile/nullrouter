//! Per-address request throttling for the control plane.
//!
//! Only `/api/*` is throttled. `/v1/*` is the inference path, where a legitimate caller may issue
//! thousands of requests a minute and a limit would break the product rather than protect it. The
//! control plane is the opposite: a human clicking a dashboard generates a handful of requests a
//! second, so a ceiling well above that is invisible to real use and still bounds a scripted attack
//! on the credential-bearing surface.
//!
//! Off unless configured. Enabling a limit silently would break whatever deployment happens to
//! exceed it, and an operator who has not chosen a number has not agreed to be throttled.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How many distinct addresses are tracked at once.
///
/// Bounded because the map is keyed by remote address: an attacker rotating source addresses would
/// otherwise grow it without limit, turning the defence into the exhaustion it exists to prevent.
/// At the cap the least-recently-seen entry is evicted, which is the one least likely to be mid-burst.
const MAX_TRACKED_ADDRESSES: usize = 8_192;

/// A refill-on-read token bucket.
///
/// Tokens accrue continuously rather than resetting on a window boundary. A fixed window lets a
/// caller spend the whole allowance at the end of one window and again at the start of the next,
/// which is twice the intended rate at the moment it matters least.
#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    last_seen: Instant,
}

/// What the limiter decided about one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Within the allowance.
    Allow,
    /// Over it. Carries how long until one token is available, for `Retry-After`.
    Throttle { retry_after: Duration },
}

/// Requests permitted per second, and the burst a caller may spend at once.
#[derive(Debug, Clone, Copy)]
pub struct ThrottleConfig {
    pub per_second: f64,
    pub burst: f64,
}

impl ThrottleConfig {
    /// Read the limit from the environment, or `None` when unset.
    ///
    /// `NULLROUTER_API_RATE_LIMIT` is requests per second; `NULLROUTER_API_RATE_BURST` defaults to
    /// one second's worth so a single click that fires several requests is never refused.
    ///
    /// A value that does not parse, or is not positive, is treated as unset rather than as zero.
    /// Zero would refuse every request, and a typo in a deployment variable should not lock an
    /// operator out of their own dashboard.
    pub fn from_env() -> Option<Self> {
        let per_second = std::env::var("NULLROUTER_API_RATE_LIMIT")
            .ok()?
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|value| *value > 0.0 && value.is_finite())?;
        let burst = std::env::var("NULLROUTER_API_RATE_BURST")
            .ok()
            .and_then(|value| value.trim().parse::<f64>().ok())
            .filter(|value| *value > 0.0 && value.is_finite())
            .unwrap_or(per_second);
        Some(Self { per_second, burst })
    }
}

/// Tracks recent request rates per address.
#[derive(Debug)]
pub struct Throttle {
    config: ThrottleConfig,
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
}

impl Throttle {
    pub fn new(config: ThrottleConfig) -> Self {
        Self {
            config,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Whether this path is subject to throttling.
    ///
    /// `/api/auth/*` is included deliberately: the auth service has its own per-address lockout, but
    /// that counts failed passwords, so it does nothing about a flood of well-formed requests.
    pub fn governs(path: &str) -> bool {
        path.starts_with("/api/")
    }

    /// Charge one request against `peer`, at `now`.
    ///
    /// `now` is a parameter so the behaviour over time is testable without sleeping; the caller
    /// passes `Instant::now()`.
    pub fn check(&self, peer: IpAddr, now: Instant) -> Verdict {
        let Ok(mut buckets) = self.buckets.lock() else {
            // A poisoned lock means another thread panicked holding it. Allowing the request is the
            // right failure direction: a throttle that starts refusing everything because of an
            // internal fault is a worse outage than the one it was guarding against.
            return Verdict::Allow;
        };

        if buckets.len() >= MAX_TRACKED_ADDRESSES && !buckets.contains_key(&peer) {
            Self::evict_oldest(&mut buckets);
        }

        let bucket = buckets.entry(peer).or_insert(Bucket {
            tokens: self.config.burst,
            last_seen: now,
        });

        let elapsed = now
            .saturating_duration_since(bucket.last_seen)
            .as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.config.per_second).min(self.config.burst);
        bucket.last_seen = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            return Verdict::Allow;
        }

        // Rounded up to whole seconds: `Retry-After` is expressed in seconds, and rounding down
        // would invite a retry that is still too early.
        let deficit = 1.0 - bucket.tokens;
        let wait = (deficit / self.config.per_second).ceil().max(1.0);
        Verdict::Throttle {
            retry_after: Duration::from_secs(wait as u64),
        }
    }

    fn evict_oldest(buckets: &mut HashMap<IpAddr, Bucket>) {
        let oldest = buckets
            .iter()
            .min_by_key(|(_, bucket)| bucket.last_seen)
            .map(|(address, _)| *address);
        if let Some(address) = oldest {
            buckets.remove(&address);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Throttle, ThrottleConfig, Verdict};
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::{Duration, Instant};

    fn peer(octet: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(203, 0, 113, octet))
    }

    fn throttle(per_second: f64, burst: f64) -> Throttle {
        Throttle::new(ThrottleConfig { per_second, burst })
    }

    #[test]
    fn the_inference_path_is_never_throttled() {
        // The load-bearing scope decision. A limit here would break a legitimate high-volume caller,
        // which is the product's whole purpose.
        assert!(!Throttle::governs("/v1/chat/completions"));
        assert!(!Throttle::governs("/v1/messages"));
        assert!(Throttle::governs("/api/settings"));
        assert!(Throttle::governs("/api/auth/login"));
    }

    #[test]
    fn a_burst_is_allowed_then_refused() {
        let limiter = throttle(1.0, 3.0);
        let now = Instant::now();
        for attempt in 0..3 {
            assert_eq!(
                limiter.check(peer(1), now),
                Verdict::Allow,
                "attempt {attempt}"
            );
        }
        assert!(matches!(
            limiter.check(peer(1), now),
            Verdict::Throttle { .. }
        ));
    }

    #[test]
    fn tokens_refill_over_time() {
        let limiter = throttle(2.0, 2.0);
        let start = Instant::now();
        assert_eq!(limiter.check(peer(2), start), Verdict::Allow);
        assert_eq!(limiter.check(peer(2), start), Verdict::Allow);
        assert!(matches!(
            limiter.check(peer(2), start),
            Verdict::Throttle { .. }
        ));

        // Half a second at two per second is one token.
        let later = start + Duration::from_millis(500);
        assert_eq!(limiter.check(peer(2), later), Verdict::Allow);
    }

    #[test]
    fn refill_does_not_exceed_the_burst() {
        // A caller idle for an hour must not accumulate an hour of allowance and then spend it at
        // once, which would make the burst ceiling meaningless.
        let limiter = throttle(1.0, 2.0);
        let start = Instant::now();
        assert_eq!(limiter.check(peer(3), start), Verdict::Allow);

        let much_later = start + Duration::from_secs(3_600);
        assert_eq!(limiter.check(peer(3), much_later), Verdict::Allow);
        assert_eq!(limiter.check(peer(3), much_later), Verdict::Allow);
        assert!(matches!(
            limiter.check(peer(3), much_later),
            Verdict::Throttle { .. }
        ));
    }

    #[test]
    fn addresses_are_counted_separately() {
        // One noisy client must not throttle everyone else.
        let limiter = throttle(1.0, 1.0);
        let now = Instant::now();
        assert_eq!(limiter.check(peer(4), now), Verdict::Allow);
        assert!(matches!(
            limiter.check(peer(4), now),
            Verdict::Throttle { .. }
        ));
        assert_eq!(limiter.check(peer(5), now), Verdict::Allow);
    }

    #[test]
    fn retry_after_is_at_least_one_second() {
        // `Retry-After` is whole seconds. A zero would invite an immediate retry that is still over.
        let limiter = throttle(0.5, 1.0);
        let now = Instant::now();
        assert_eq!(limiter.check(peer(6), now), Verdict::Allow);
        match limiter.check(peer(6), now) {
            Verdict::Throttle { retry_after } => {
                assert!(retry_after >= Duration::from_secs(1), "{retry_after:?}");
            }
            Verdict::Allow => panic!("expected a throttle"),
        }
    }

    #[test]
    fn tracking_is_bounded() {
        // Keyed by remote address, so an attacker rotating addresses must not grow the map without
        // limit -- that would turn the defence into the exhaustion it guards against.
        let limiter = throttle(1_000.0, 1_000.0);
        let now = Instant::now();
        for index in 0..20_000_u32 {
            let address = IpAddr::V4(Ipv4Addr::from(index.to_be_bytes()));
            limiter.check(address, now);
        }
        let tracked = limiter.buckets.lock().map(|guard| guard.len()).unwrap_or(0);
        assert!(
            tracked <= super::MAX_TRACKED_ADDRESSES,
            "tracked {tracked} addresses, cap is {}",
            super::MAX_TRACKED_ADDRESSES
        );
    }

    #[test]
    fn an_unusable_configuration_is_treated_as_unset() {
        // A typo in a deployment variable must not lock an operator out of their own dashboard, so
        // zero and garbage mean "no limit" rather than "refuse everything".
        let saved = (
            std::env::var_os("NULLROUTER_API_RATE_LIMIT"),
            std::env::var_os("NULLROUTER_API_RATE_BURST"),
        );

        for value in ["0", "-5", "abc", ""] {
            // SAFETY: this module's cases run in one thread and restore both variables below.
            unsafe { std::env::set_var("NULLROUTER_API_RATE_LIMIT", value) };
            assert!(
                ThrottleConfig::from_env().is_none(),
                "{value:?} should read as unset"
            );
        }

        // SAFETY: as above.
        unsafe { std::env::set_var("NULLROUTER_API_RATE_LIMIT", "10") };
        let config = ThrottleConfig::from_env().expect("a positive rate must configure a limit");
        assert!((config.per_second - 10.0).abs() < f64::EPSILON);
        // Burst defaults to one second's worth, so a click firing several requests is never refused.
        assert!((config.burst - 10.0).abs() < f64::EPSILON);

        for (name, value) in [
            ("NULLROUTER_API_RATE_LIMIT", saved.0),
            ("NULLROUTER_API_RATE_BURST", saved.1),
        ] {
            match value {
                // SAFETY: as above.
                Some(previous) => unsafe { std::env::set_var(name, previous) },
                // SAFETY: as above.
                None => unsafe { std::env::remove_var(name) },
            }
        }
    }
}
