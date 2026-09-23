//! Process-wide Spotify rate-limit cooldown ("circuit breaker").
//!
//! Spotify answers content-API calls with `429 Too Many Requests` plus a
//! `Retry-After` header that can be tens of minutes. Retrying *inside* that
//! window keeps the server's rolling limit saturated — the app then locks
//! itself out indefinitely (observed: a full day with 0 successful polls).
//!
//! Every background loop that talks to Spotify consults [`cooldown`] before
//! issuing a request and reports `Retry-After` values back into it, so one 429
//! pauses *all* callers instead of each one hammering independently.

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Upper bound for a single cooldown, so a bogus `Retry-After` cannot wedge the
/// app forever.
pub const MAX_COOLDOWN_SECS: u64 = 21_600; // 6 h

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A "do not call Spotify before" deadline.
///
/// The cooldown only ever *extends* — a shorter `Retry-After` never shortens an
/// active penalty — and is self-healing: once the deadline passes the type
/// reads as free again without an explicit reset.
#[derive(Debug)]
pub struct SpotifyCooldown {
    /// Absolute unix second until which requests must be withheld. `0` = free.
    until_unix: AtomicI64,
}

impl SpotifyCooldown {
    pub const fn new() -> Self {
        Self {
            until_unix: AtomicI64::new(0),
        }
    }

    /// Remaining cooldown in seconds, or `None` when requests are allowed.
    pub fn remaining_secs(&self) -> Option<u64> {
        let remaining = self.until_unix.load(Ordering::SeqCst) - now_unix();
        if remaining > 0 {
            Some(remaining as u64)
        } else {
            None
        }
    }

    /// Extend the cooldown to at least `now + secs`.
    ///
    /// `0` is ignored (some servers send an empty value), values are capped at
    /// [`MAX_COOLDOWN_SECS`], and the deadline never moves earlier than an
    /// already-active one.
    pub fn note_retry_after(&self, secs: u64) {
        if secs == 0 {
            return;
        }
        let deadline = now_unix().saturating_add(secs.min(MAX_COOLDOWN_SECS) as i64);
        let mut current = self.until_unix.load(Ordering::SeqCst);
        while deadline > current {
            match self.until_unix.compare_exchange(
                current,
                deadline,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return,
                Err(observed) => current = observed,
            }
        }
    }

    /// Clear the cooldown (used after a request proves the limit is gone).
    pub fn clear(&self) {
        self.until_unix.store(0, Ordering::SeqCst);
    }
}

impl Default for SpotifyCooldown {
    fn default() -> Self {
        Self::new()
    }
}

/// The process-wide cooldown shared by every Spotify-calling background loop.
pub static SPOTIFY_COOLDOWN: SpotifyCooldown = SpotifyCooldown::new();

/// Accessor for the process-wide [`SPOTIFY_COOLDOWN`].
pub fn cooldown() -> &'static SpotifyCooldown {
    &SPOTIFY_COOLDOWN
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_cooldown_is_free() {
        assert_eq!(SpotifyCooldown::new().remaining_secs(), None);
    }

    #[test]
    fn note_retry_after_blocks_for_that_long() {
        let c = SpotifyCooldown::new();
        c.note_retry_after(3600);
        let r = c.remaining_secs().expect("cooling down");
        assert!((3599..=3600).contains(&r), "got {r}");
    }

    #[test]
    fn shorter_retry_after_never_shortens_active_penalty() {
        let c = SpotifyCooldown::new();
        c.note_retry_after(3600);
        c.note_retry_after(5);
        let r = c.remaining_secs().expect("still cooling down");
        assert!(r > 3500, "active penalty was shortened: {r}");
    }

    #[test]
    fn zero_retry_after_is_ignored() {
        let c = SpotifyCooldown::new();
        c.note_retry_after(0);
        assert_eq!(c.remaining_secs(), None);
    }

    #[test]
    fn retry_after_is_capped() {
        let c = SpotifyCooldown::new();
        c.note_retry_after(u64::MAX);
        let r = c.remaining_secs().expect("cooling down");
        assert!(r <= MAX_COOLDOWN_SECS, "not capped: {r}");
        assert!(r > MAX_COOLDOWN_SECS - 5, "unexpectedly low: {r}");
    }

    #[test]
    fn clear_releases_the_cooldown() {
        let c = SpotifyCooldown::new();
        c.note_retry_after(3600);
        c.clear();
        assert_eq!(c.remaining_secs(), None);
    }
}
