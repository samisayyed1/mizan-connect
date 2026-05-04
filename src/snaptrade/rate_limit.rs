//! Per-user rate limit for the SnapTrade `/login-portal` endpoint.
//!
//! 10 requests per rolling hour per local user. In-memory; single-instance
//! safe only. When the deployment scales beyond one Fly machine
//! (Chunk 5 prereq), this needs to move to Redis or a Postgres-backed
//! token bucket.
//!
//! The limiter is a simple [`HashMap<Uuid, Vec<OffsetDateTime>>`] —
//! perfectly fine at our scale and avoids a dependency on a generic
//! token-bucket crate. Sweep on each call to drop entries older than the
//! window.

use std::sync::Arc;

use parking_lot::Mutex;
use std::collections::HashMap;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const WINDOW: Duration = Duration::hours(1);
const MAX_PER_WINDOW: usize = 10;

/// Per-user login-portal rate limiter. Cheap to clone (wraps `Arc`).
#[derive(Clone, Default)]
pub struct LoginPortalLimiter {
    inner: Arc<Mutex<HashMap<Uuid, Vec<OffsetDateTime>>>>,
}

impl LoginPortalLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Try to consume one slot for `user_id`. Returns `Ok(())` if allowed,
    /// `Err(retry_after)` with the duration until the next slot frees up.
    pub fn check(&self, user_id: Uuid) -> Result<(), Duration> {
        let now = OffsetDateTime::now_utc();
        let cutoff = now - WINDOW;

        let mut map = self.inner.lock();
        let slots = map.entry(user_id).or_default();

        // Drop entries outside the window.
        slots.retain(|t| *t > cutoff);

        if slots.len() >= MAX_PER_WINDOW {
            // Oldest entry will roll out of the window first; the user can
            // retry after that timestamp.
            let oldest = slots.first().copied().unwrap_or(now);
            let retry_after = (oldest + WINDOW) - now;
            return Err(retry_after.max(Duration::ZERO));
        }

        slots.push(now);
        Ok(())
    }

    /// Drop entries for users that haven't called in `2 * WINDOW`. Call
    /// from a background sweeper if memory growth becomes a concern.
    /// Safe to call concurrently with [`Self::check`].
    #[allow(dead_code)]
    pub fn sweep(&self) {
        let cutoff = OffsetDateTime::now_utc() - WINDOW;
        let mut map = self.inner.lock();
        map.retain(|_, slots| {
            slots.retain(|t| *t > cutoff);
            !slots.is_empty()
        });
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn ten_calls_pass_eleventh_fails() {
        let l = LoginPortalLimiter::new();
        let user = Uuid::new_v4();
        for i in 0..10 {
            l.check(user)
                .unwrap_or_else(|d| panic!("call {i} should pass, got retry-after {d}"));
        }
        let err = l.check(user).expect_err("11th call must be rate limited");
        assert!(
            err > Duration::ZERO,
            "retry-after must be positive, got {err}"
        );
    }

    #[test]
    fn separate_users_have_independent_buckets() {
        let l = LoginPortalLimiter::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        for _ in 0..10 {
            l.check(a).expect("a allowed");
        }
        l.check(b).expect("b is fresh");
    }
}
