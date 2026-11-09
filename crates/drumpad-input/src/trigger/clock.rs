//! Monotonic, testable timestamp sources.
//!
//! Every captured event carries a `timestamp_us` measured on the same clock as
//! the queue consumer, so the audio engine can order keyboard and MIDI events
//! without trusting their arrival order.

use std::cell::Cell;
use std::time::{Duration, Instant};

/// Monotonic microsecond clock used by capture paths.
///
/// Implementations must never move backwards. [`MonotonicClock`] guarantees
/// this by construction; [`ManualClock`] is the deterministic test double.
pub trait EventClock {
    /// Microseconds since an arbitrary but fixed origin.
    fn now_us(&self) -> u64;
}

/// Wall-clock-backed monotonic clock.
///
/// The origin is chosen when the clock is created; only differences are
/// meaningful. `Instant` is monotonic on Windows, so repeated reads never
/// decrease.
#[derive(Debug, Clone, Copy)]
pub struct MonotonicClock {
    origin: Instant,
}

impl MonotonicClock {
    /// Start a clock at the current instant.
    #[must_use]
    pub fn new() -> Self {
        Self::started_at(Instant::now())
    }

    /// Start a clock at a caller-supplied instant.
    ///
    /// Useful for probes that straddle an event boundary.
    #[must_use]
    pub fn started_at(origin: Instant) -> Self {
        Self { origin }
    }

    /// Instant the clock started from.
    #[must_use]
    pub fn origin(&self) -> Instant {
        self.origin
    }

    /// Duration since the clock origin.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.origin.elapsed()
    }
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl EventClock for MonotonicClock {
    fn now_us(&self) -> u64 {
        self.origin.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
    }
}

/// Deterministic clock for tests and replay.
///
/// Shared by reference (`&ManualClock`) so fixtures can advance time between
/// events while the producer owns it directly.
#[derive(Debug, Default, Clone)]
pub struct ManualClock {
    now_us: Cell<u64>,
}

impl ManualClock {
    /// Create a clock at `start_us`.
    #[must_use]
    pub const fn new(start_us: u64) -> Self {
        Self {
            now_us: Cell::new(start_us),
        }
    }

    /// Current value without consuming it.
    #[must_use]
    pub fn value(&self) -> u64 {
        self.now_us.get()
    }

    /// Advance by `delta_us`, saturating at `u64::MAX`.
    pub fn advance_us(&self, delta_us: u64) {
        self.now_us.set(self.now_us.get().saturating_add(delta_us));
    }

    /// Jump to an absolute value; values below the current time are ignored so
    /// the clock stays monotonic.
    pub fn set_us(&self, value: u64) {
        self.now_us.set(self.now_us.get().max(value));
    }
}

impl EventClock for ManualClock {
    fn now_us(&self) -> u64 {
        self.now_us.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotonic_clock_never_moves_backwards() {
        let clock = MonotonicClock::new();
        let mut previous = 0;
        for _ in 0..100 {
            let now = clock.now_us();
            assert!(now >= previous, "{now} < {previous}");
            previous = now;
        }
    }

    #[test]
    fn monotonic_clock_measures_duration() {
        let origin = Instant::now() - Duration::from_millis(5);
        let clock = MonotonicClock::started_at(origin);
        assert!(clock.now_us() >= 5_000);
        assert!(clock.elapsed() >= Duration::from_millis(5));
    }

    #[test]
    fn manual_clock_is_deterministic_and_monotonic() {
        let clock = ManualClock::new(100);
        assert_eq!(clock.now_us(), 100);
        clock.advance_us(25);
        assert_eq!(clock.now_us(), 125);
        clock.set_us(10); // backwards writes are ignored
        assert_eq!(clock.now_us(), 125);
        clock.set_us(200);
        assert_eq!(clock.now_us(), 200);
    }

    #[test]
    fn manual_clock_saturates() {
        let clock = ManualClock::new(u64::MAX - 1);
        clock.advance_us(10);
        assert_eq!(clock.now_us(), u64::MAX);
    }

    #[test]
    fn clock_accessors_expose_their_origin_and_value() {
        let origin = Instant::now();
        let clock = MonotonicClock::started_at(origin);
        assert_eq!(clock.origin(), origin);

        let manual = ManualClock::new(77);
        assert_eq!(manual.value(), 77);
        let default_clock = MonotonicClock::default();
        assert!(default_clock.now_us() < u64::MAX);
    }
}
