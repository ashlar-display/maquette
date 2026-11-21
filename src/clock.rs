//! `SimClock`: hdmi-hal's `Clock` and `Delay` on simulated time.

use core::cell::Cell;
use core::fmt;
use core::time::Duration;

use critical_section::Mutex;
use hdmi_hal::time::{Clock, Delay, Timestamp};

/// A clock and a delay on simulated time.
///
/// `now` starts at zero and moves only when something advances it: a [`delay`](Delay::delay)
/// on the clock, or the test, with [`advance`](Self::advance). It never reads the host's
/// time, never moves backwards, and saturates instead of overflowing.
///
/// Two settings make it behave like a coarser or slower platform:
///
/// - **a tick** ([`with_tick`](Self::with_tick)): `now` reads in whole ticks, rounded down,
///   and a delay is rounded up to whole ticks, as hdmi-hal's `Delay` allows;
/// - **an overshoot** ([`with_overshoot`](Self::with_overshoot)): every delay waits that much
///   longer than asked, as a real delay may, so deadline arithmetic is tested against it.
///
/// `now` read after `delay(d)` is always at least `d` later than before it.
///
/// The time sits behind a [`critical_section::Mutex`], so the clock is `Sync`: one clock can
/// be shared between threads and held across an `.await` in `Send` code. A host needs the
/// `std` feature (or another critical-section implementation); a bare target supplies its own.
///
/// `Clock` and `Delay` are implemented for the clock and for a reference to it, so one clock
/// can be passed as both where a procedure takes them as separate values:
///
/// ```
/// use core::time::Duration;
/// use maquette::{Clock, Delay, SimClock, Timestamp};
///
/// fn wait_for<C: Clock, D: Delay>(clock: &C, delay: &mut D, d: Duration) -> Duration {
///     let start = clock.now();
///     delay.delay(d);
///     clock.now().saturating_duration_since(start)
/// }
///
/// let clock = SimClock::new().with_overshoot(Duration::from_micros(250));
/// let waited = wait_for(&clock, &mut &clock, Duration::from_millis(1));
/// assert_eq!(waited, Duration::from_micros(1_250));
///
/// clock.advance(Duration::from_millis(10));
/// assert_eq!(clock.now(), Timestamp::from_micros(11_250));
/// ```
pub struct SimClock {
    /// Microseconds since the epoch, exactly; `now` rounds it down to the tick.
    micros: Mutex<Cell<u64>>,
    /// The tick, in microseconds; at least 1.
    tick_us: u64,
    /// Added to every delay, in microseconds.
    overshoot_us: u64,
}

impl SimClock {
    /// A clock at zero with a 1 µs tick and no overshoot.
    pub const fn new() -> Self {
        Self {
            micros: Mutex::new(Cell::new(0)),
            tick_us: 1,
            overshoot_us: 0,
        }
    }

    /// The clock with a resolution of `tick`, rounded up to whole microseconds; zero means
    /// 1 µs, the finest a [`Timestamp`] holds.
    #[must_use]
    pub const fn with_tick(mut self, tick: Duration) -> Self {
        let us = micros_rounded_up(tick);
        self.tick_us = if us == 0 { 1 } else { us };
        self
    }

    /// The clock with every delay waiting `overshoot` longer than asked, rounded up to whole
    /// microseconds.
    #[must_use]
    pub const fn with_overshoot(mut self, overshoot: Duration) -> Self {
        self.overshoot_us = micros_rounded_up(overshoot);
        self
    }

    /// The clock's resolution.
    pub const fn tick(&self) -> Duration {
        Duration::from_micros(self.tick_us)
    }

    /// How much longer than asked every delay waits.
    pub const fn overshoot(&self) -> Duration {
        Duration::from_micros(self.overshoot_us)
    }

    /// Moves the clock `duration` forward, rounded up to whole microseconds; it stops at the
    /// largest time a [`Timestamp`] holds.
    pub fn advance(&self, duration: Duration) {
        self.advance_micros(micros_rounded_up(duration));
    }

    fn advance_micros(&self, micros: u64) {
        critical_section::with(|cs| {
            let now = self.micros.borrow(cs);
            now.set(now.get().saturating_add(micros));
        });
    }

    fn micros(&self) -> u64 {
        critical_section::with(|cs| self.micros.borrow(cs).get())
    }
}

impl Default for SimClock {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for SimClock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SimClock")
            .field("now", &self.now())
            .field("tick", &self.tick())
            .field("overshoot", &self.overshoot())
            .finish()
    }
}

impl Clock for SimClock {
    /// The simulated time, rounded down to the tick.
    fn now(&self) -> Timestamp {
        let micros = self.micros();
        Timestamp::from_micros(micros - micros % self.tick_us)
    }
}

impl Delay for SimClock {
    /// Moves the clock `duration` forward, rounded up to whole ticks, plus the overshoot.
    ///
    /// A whole number of ticks moves `now`, which reads in ticks, by exactly that much, so
    /// `now` read after the delay is at least `duration` later than before it.
    fn delay(&mut self, duration: Duration) {
        Delay::delay(&mut &*self, duration);
    }
}

impl Clock for &SimClock {
    fn now(&self) -> Timestamp {
        (**self).now()
    }
}

impl Delay for &SimClock {
    fn delay(&mut self, duration: Duration) {
        let ticks = micros_rounded_up(duration).div_ceil(self.tick_us);
        let micros = ticks.saturating_mul(self.tick_us);
        self.advance_micros(micros.saturating_add(self.overshoot_us));
    }
}

/// `duration` in whole microseconds, rounded up, saturating at `u64::MAX`.
const fn micros_rounded_up(duration: Duration) -> u64 {
    let micros = duration.as_nanos().div_ceil(1_000);
    if micros > u64::MAX as u128 {
        u64::MAX
    } else {
        micros as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: Timestamp = Timestamp::from_micros(u64::MAX);

    fn us(micros: u64) -> Duration {
        Duration::from_micros(micros)
    }

    #[test]
    fn starts_at_zero_and_moves_only_when_advanced() {
        let clock = SimClock::new();
        assert_eq!(clock.now(), Timestamp::from_micros(0));
        assert_eq!(clock.now(), Timestamp::from_micros(0));
        clock.advance(us(1_500));
        assert_eq!(clock.now(), Timestamp::from_micros(1_500));
    }

    #[test]
    fn default_is_new() {
        let clock = SimClock::default();
        assert_eq!(clock.tick(), us(1));
        assert_eq!(clock.overshoot(), Duration::ZERO);
        assert_eq!(clock.now(), Timestamp::from_micros(0));
    }

    #[test]
    fn delay_moves_the_clock_by_the_duration() {
        let mut clock = SimClock::new();
        clock.delay(us(700));
        assert_eq!(clock.now(), Timestamp::from_micros(700));
        let mut shared = &clock;
        shared.delay(us(300));
        assert_eq!(clock.now(), Timestamp::from_micros(1_000));
    }

    #[test]
    fn a_reference_is_the_same_clock() {
        fn read<C: Clock>(clock: C) -> Timestamp {
            clock.now()
        }
        let clock = SimClock::new().with_tick(us(10));
        clock.advance(us(25));
        assert_eq!(read(&clock), Timestamp::from_micros(20));
        assert_eq!(read(&clock), clock.now());
    }

    #[test]
    fn delay_of_zero_does_not_move_the_clock() {
        let mut clock = SimClock::new();
        clock.delay(Duration::ZERO);
        assert_eq!(clock.now(), Timestamp::from_micros(0));
    }

    #[test]
    fn sub_microsecond_durations_round_up() {
        let mut clock = SimClock::new();
        clock.delay(Duration::from_nanos(1));
        assert_eq!(clock.now(), Timestamp::from_micros(1));
        clock.advance(Duration::from_nanos(1_001));
        assert_eq!(clock.now(), Timestamp::from_micros(3));
    }

    #[test]
    fn overshoot_lengthens_every_delay() {
        let mut clock = SimClock::new().with_overshoot(us(250));
        assert_eq!(clock.overshoot(), us(250));
        clock.delay(us(1_000));
        assert_eq!(clock.now(), Timestamp::from_micros(1_250));
        clock.delay(Duration::ZERO);
        assert_eq!(clock.now(), Timestamp::from_micros(1_500));
        // The test's own advance is exact.
        clock.advance(us(100));
        assert_eq!(clock.now(), Timestamp::from_micros(1_600));
    }

    #[test]
    fn tick_rounds_now_down() {
        let clock = SimClock::new().with_tick(Duration::from_millis(1));
        assert_eq!(clock.tick(), us(1_000));
        clock.advance(us(1_999));
        assert_eq!(clock.now(), Timestamp::from_micros(1_000));
        clock.advance(us(1));
        assert_eq!(clock.now(), Timestamp::from_micros(2_000));
    }

    #[test]
    fn tick_rounds_delays_up_to_whole_ticks() {
        let mut clock = SimClock::new().with_tick(Duration::from_millis(1));
        clock.delay(us(1));
        assert_eq!(clock.now(), Timestamp::from_micros(1_000));
        clock.delay(us(1_000));
        assert_eq!(clock.now(), Timestamp::from_micros(2_000));
    }

    #[test]
    fn delay_waits_at_least_its_duration_from_any_point_in_a_tick() {
        for start in [0, 1, 999, 1_000, 1_001, 2_500] {
            for d in [0, 1, 999, 1_000, 1_001, 2_000, 3_333] {
                let mut clock = SimClock::new().with_tick(us(1_000)).with_overshoot(us(17));
                clock.advance(us(start));
                let before = clock.now();
                clock.delay(us(d));
                let waited = clock.now().saturating_duration_since(before);
                assert!(
                    waited >= us(d),
                    "start {start}, delay {d}: waited {waited:?}"
                );
            }
        }
    }

    #[test]
    fn zero_and_sub_microsecond_ticks_are_one_microsecond() {
        assert_eq!(SimClock::new().with_tick(Duration::ZERO).tick(), us(1));
        assert_eq!(
            SimClock::new().with_tick(Duration::from_nanos(1)).tick(),
            us(1)
        );
        assert_eq!(
            SimClock::new()
                .with_tick(Duration::from_nanos(1_500))
                .tick(),
            us(2)
        );
    }

    #[test]
    fn saturates_instead_of_overflowing() {
        let mut clock = SimClock::new().with_overshoot(Duration::MAX);
        assert_eq!(clock.overshoot(), us(u64::MAX));
        clock.advance(Duration::MAX);
        assert_eq!(clock.now(), MAX);
        clock.advance(us(1));
        clock.delay(Duration::MAX);
        assert_eq!(clock.now(), MAX);
    }

    #[test]
    fn saturated_now_is_rounded_down_to_the_tick() {
        let clock = SimClock::new().with_tick(us(1_000));
        clock.advance(Duration::MAX);
        assert_eq!(clock.now().as_micros(), u64::MAX - u64::MAX % 1_000);
    }

    #[test]
    fn now_never_moves_backwards() {
        let mut clock = SimClock::new().with_tick(us(7)).with_overshoot(us(3));
        let mut last = clock.now();
        for step in 0..200u64 {
            if step % 3 == 0 {
                clock.advance(us(step % 11));
            } else {
                clock.delay(us(step % 13));
            }
            let now = clock.now();
            assert!(now >= last);
            last = now;
        }
    }

    #[test]
    fn is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SimClock>();
        assert_send_sync::<&SimClock>();
    }

    #[test]
    fn shared_between_threads() {
        extern crate std;
        let clock = SimClock::new();
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    for _ in 0..100 {
                        (&clock).delay(us(1));
                    }
                });
            }
        });
        assert_eq!(clock.now(), Timestamp::from_micros(400));
    }

    #[test]
    fn debug_shows_the_time_and_settings() {
        extern crate std;
        let clock = SimClock::new().with_tick(us(10)).with_overshoot(us(2));
        clock.advance(us(25));
        assert_eq!(
            std::format!("{clock:?}"),
            "SimClock { now: Timestamp(20), tick: 10µs, overshoot: 2µs }"
        );
    }

    #[test]
    fn usable_in_const_and_static() {
        static CLOCK: SimClock = SimClock::new().with_tick(Duration::from_millis(1));
        CLOCK.advance(us(1_500));
        assert!(CLOCK.now() >= Timestamp::from_micros(1_000));
    }
}
