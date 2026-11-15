//! The clock check: hdmi-hal's `Clock` and `Delay`.

use core::fmt;
use core::time::Duration;

use hdmi_hal::time::{Clock, Delay, Timestamp};

use super::Report;

/// Runs the clock check with its default settings ([`ClockCheck::DEFAULT`]).
///
/// ```
/// use maquette::{SimClock, check};
///
/// let clock = SimClock::new();
/// check::clock(&clock, &mut &clock).assert_ok();
/// ```
pub fn clock<C: Clock + ?Sized, D: Delay + ?Sized>(
    clock: &C,
    delay: &mut D,
) -> Report<ClockFailure> {
    ClockCheck::DEFAULT.run(clock, delay)
}

/// The clock check's settings: the delays it runs and how many reads it makes in a row.
///
/// The points: each read of the clock compared with the read before it, over the reads in a
/// row and around every delay ([`ClockFailure::WentBackwards`]); each delay measured on the
/// clock, from the read before it to the read after it ([`ClockFailure::DelayTooShort`]). A
/// clock that never advances fails every delay but a zero one.
///
/// The delays run in the order given. On a real timer they take real time, so a platform
/// whose delays should be few or short sets its own with [`with_delays`](Self::with_delays),
/// for instance the durations its procedures use.
///
/// The clock alone cannot show that it runs at the right rate: a wrong timer frequency
/// scales the clock and the delays measured on it alike. With a clock the test trusts,
/// [`run_with_reference`](Self::run_with_reference) measures each delay on it too
/// ([`ClockFailure::DelayTooShortOnReference`], [`ClockFailure::Drift`]); on a host, the
/// `HostClock` the `std` feature adds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ClockCheck<'a> {
    /// The delays run, each between two reads of the clock.
    pub delays: &'a [Duration],
    /// The reads of the clock in a row, with no delay between them.
    pub reads: u32,
}

const DEFAULT_DELAYS: [Duration; 7] = [
    Duration::ZERO,
    Duration::from_nanos(1),
    Duration::from_micros(1),
    Duration::from_micros(999),
    Duration::from_millis(1),
    Duration::from_micros(1_001),
    Duration::from_millis(10),
];

impl ClockCheck<'static> {
    /// Delays of 0, 1 ns, 1 µs, 999 µs, 1 ms, 1.001 ms and 10 ms (about 13 ms in all on a
    /// real timer), and 1000 reads in a row.
    pub const DEFAULT: Self = Self {
        delays: &DEFAULT_DELAYS,
        reads: 1000,
    };
}

impl Default for ClockCheck<'static> {
    /// [`ClockCheck::DEFAULT`].
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl<'a> ClockCheck<'a> {
    /// The check with `delays` run instead.
    #[must_use]
    pub const fn with_delays<'b>(self, delays: &'b [Duration]) -> ClockCheck<'b> {
        ClockCheck {
            delays,
            reads: self.reads,
        }
    }

    /// The check with `reads` reads in a row.
    #[must_use]
    pub const fn with_reads(self, reads: u32) -> Self {
        Self { reads, ..self }
    }

    /// Runs the check against `clock` and `delay`.
    pub fn run<C: Clock + ?Sized, D: Delay + ?Sized>(
        &self,
        clock: &C,
        delay: &mut D,
    ) -> Report<ClockFailure> {
        self.run_measured(clock, delay, None)
    }

    /// Runs the check against `clock` and `delay`, measuring each delay on `reference` too,
    /// a clock the test trusts: the delay must take at least the time asked on it, and
    /// `clock` must advance as far as it does, each within `tolerance` (the reads around a
    /// delay take time, and a reference has its own resolution).
    pub fn run_with_reference<C, D, R>(
        &self,
        clock: &C,
        delay: &mut D,
        reference: &R,
        tolerance: Duration,
    ) -> Report<ClockFailure>
    where
        C: Clock + ?Sized,
        D: Delay + ?Sized,
        R: Clock,
    {
        self.run_measured(clock, delay, Some((reference, tolerance)))
    }

    fn run_measured<C: Clock + ?Sized, D: Delay + ?Sized>(
        &self,
        clock: &C,
        delay: &mut D,
        reference: Option<(&dyn Clock, Duration)>,
    ) -> Report<ClockFailure> {
        let mut run = Run::start(clock, reference);
        for _ in 0..self.reads {
            run.read(clock);
        }
        for &duration in self.delays {
            run.before_delay(clock);
            delay.delay(duration);
            run.after_delay(clock, duration);
        }
        run.report
    }

    /// Runs the check against `clock` and an async `delay`: the same points as
    /// [`run`](Self::run).
    ///
    /// `delay` may implement hdmi-hal-async's `Delay` or its `Send` twin, `SendDelay`, which
    /// implements `Delay` too. The future is not promised to be `Send`: a test drives it to
    /// completion where it is.
    #[cfg(feature = "async")]
    pub async fn run_async<C, D>(&self, clock: &C, delay: &mut D) -> Report<ClockFailure>
    where
        C: Clock + ?Sized,
        D: hdmi_hal_async::time::Delay + ?Sized,
    {
        self.run_measured_async(clock, delay, None).await
    }

    /// [`run_with_reference`](Self::run_with_reference) with an async `delay`.
    #[cfg(feature = "async")]
    pub async fn run_with_reference_async<C, D, R>(
        &self,
        clock: &C,
        delay: &mut D,
        reference: &R,
        tolerance: Duration,
    ) -> Report<ClockFailure>
    where
        C: Clock + ?Sized,
        D: hdmi_hal_async::time::Delay + ?Sized,
        R: Clock,
    {
        self.run_measured_async(clock, delay, Some((reference, tolerance)))
            .await
    }

    #[cfg(feature = "async")]
    async fn run_measured_async<C, D>(
        &self,
        clock: &C,
        delay: &mut D,
        reference: Option<(&dyn Clock, Duration)>,
    ) -> Report<ClockFailure>
    where
        C: Clock + ?Sized,
        D: hdmi_hal_async::time::Delay + ?Sized,
    {
        let mut run = Run::start(clock, reference);
        for _ in 0..self.reads {
            run.read(clock);
        }
        for &duration in self.delays {
            run.before_delay(clock);
            delay.delay(duration).await;
            run.after_delay(clock, duration);
        }
        run.report
    }
}

/// A failed point of the clock check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClockFailure {
    /// A read of the clock was earlier than the read before it.
    WentBackwards {
        /// The read before.
        earlier: Timestamp,
        /// The read after it, earlier than `earlier`.
        later: Timestamp,
    },
    /// Across `delay(requested)`, the clock advanced only `elapsed`.
    DelayTooShort {
        /// The duration asked of the delay.
        requested: Duration,
        /// How far the clock advanced across it.
        elapsed: Duration,
    },
    /// Across `delay(requested)`, the reference advanced only `elapsed`, short of it by
    /// more than the tolerance.
    DelayTooShortOnReference {
        /// The duration asked of the delay.
        requested: Duration,
        /// How far the reference advanced across it.
        elapsed: Duration,
    },
    /// Across `delay(requested)`, the clock and the reference advanced by amounts further
    /// apart than the tolerance: the clock runs at the wrong rate.
    Drift {
        /// The duration asked of the delay.
        requested: Duration,
        /// How far the clock advanced.
        clock: Duration,
        /// How far the reference advanced.
        reference: Duration,
    },
}

impl fmt::Display for ClockFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WentBackwards { earlier, later } => write!(
                f,
                "the clock went backwards: it read {} µs after reading {} µs",
                later.as_micros(),
                earlier.as_micros()
            ),
            Self::DelayTooShort { requested, elapsed } => write!(
                f,
                "delay({requested:?}) advanced the clock only {elapsed:?}"
            ),
            Self::DelayTooShortOnReference { requested, elapsed } => write!(
                f,
                "delay({requested:?}) took only {elapsed:?} on the reference"
            ),
            Self::Drift {
                requested,
                clock,
                reference,
            } => write!(
                f,
                "across delay({requested:?}) the clock advanced {clock:?}, the reference {reference:?}"
            ),
        }
    }
}

/// The state of one run: the last read, the reference with its tolerance and its read before
/// the delay running, and the report so far.
struct Run<'r> {
    last: Timestamp,
    reference: Option<(&'r dyn Clock, Duration)>,
    reference_before: Timestamp,
    report: Report<ClockFailure>,
}

impl<'r> Run<'r> {
    fn start<C: Clock + ?Sized>(clock: &C, reference: Option<(&'r dyn Clock, Duration)>) -> Self {
        Self {
            last: clock.now(),
            reference,
            reference_before: Timestamp::from_micros(0),
            report: Report::new(),
        }
    }

    /// Reads the clock, then the reference, before a delay.
    fn before_delay<C: Clock + ?Sized>(&mut self, clock: &C) {
        self.read(clock);
        if let Some((reference, _)) = self.reference {
            self.reference_before = reference.now();
        }
    }

    /// Reads the clock, checking it against the read before.
    fn read<C: Clock + ?Sized>(&mut self, clock: &C) -> Timestamp {
        let earlier = self.last;
        let later = clock.now();
        self.report
            .record((later < earlier).then_some(ClockFailure::WentBackwards { earlier, later }));
        self.last = later;
        later
    }

    /// Reads the clock after a delay of `requested`, checking the read and the time passed
    /// since the read before the delay.
    fn after_delay<C: Clock + ?Sized>(&mut self, clock: &C, requested: Duration) {
        let before = self.last;
        let reference_after = self.reference.map(|(reference, _)| reference.now());
        let after = self.read(clock);
        let elapsed = after.saturating_duration_since(before);
        self.report.record(
            (elapsed < requested).then_some(ClockFailure::DelayTooShort { requested, elapsed }),
        );
        if let (Some((_, tolerance)), Some(reference_after)) = (self.reference, reference_after) {
            let measured = reference_after.saturating_duration_since(self.reference_before);
            self.report
                .record((measured.saturating_add(tolerance) < requested).then_some(
                    ClockFailure::DelayTooShortOnReference {
                        requested,
                        elapsed: measured,
                    },
                ));
            let apart = elapsed.abs_diff(measured);
            self.report
                .record((apart > tolerance).then_some(ClockFailure::Drift {
                    requested,
                    clock: elapsed,
                    reference: measured,
                }));
        }
    }
}

/// The host's clock, `std::time::Instant` from its creation: a reference for
/// [`ClockCheck::run_with_reference`].
#[cfg(feature = "std")]
#[derive(Debug, Clone, Copy)]
pub struct HostClock {
    start: std::time::Instant,
}

#[cfg(feature = "std")]
impl HostClock {
    /// A clock reading 0 now.
    pub fn new() -> Self {
        Self {
            start: std::time::Instant::now(),
        }
    }
}

#[cfg(feature = "std")]
impl Default for HostClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "std")]
impl Clock for HostClock {
    fn now(&self) -> Timestamp {
        let micros = self.start.elapsed().as_micros();
        Timestamp::from_micros(u64::try_from(micros).unwrap_or(u64::MAX))
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use core::cell::Cell;
    use std::string::ToString;

    use super::*;
    use crate::SimClock;

    fn us(micros: u64) -> Duration {
        Duration::from_micros(micros)
    }

    /// The default run's points: each read in a row, and per delay the read before it, the
    /// read after it and the delay.
    const DEFAULT_POINTS: usize = 1000 + 3 * 7;

    /// A clock and delay with a fault: `now` reads `reads(time)`, a delay advances the time by
    /// `delays(requested)`.
    struct Broken {
        time: Cell<u64>,
        reads: fn(u64) -> u64,
        delays: fn(u64) -> u64,
    }

    impl Broken {
        fn new(reads: fn(u64) -> u64, delays: fn(u64) -> u64) -> Self {
            Self {
                time: Cell::new(1_000_000),
                reads,
                delays,
            }
        }
    }

    impl Clock for Broken {
        fn now(&self) -> Timestamp {
            Timestamp::from_micros((self.reads)(self.time.get()))
        }
    }

    impl Delay for &Broken {
        fn delay(&mut self, duration: Duration) {
            let requested = duration.as_nanos().div_ceil(1_000) as u64;
            self.time.set(self.time.get() + (self.delays)(requested));
        }
    }

    #[test]
    fn sim_clock_passes() {
        let clock = SimClock::new();
        let report = clock_check(&clock);
        report.assert_ok();
        assert_eq!(report.points(), DEFAULT_POINTS);
    }

    fn clock_check(clock: &SimClock) -> Report<ClockFailure> {
        super::clock(clock, &mut &*clock)
    }

    #[test]
    fn sim_clock_passes_with_a_coarse_tick_and_overshoot() {
        for tick in [us(1), us(7), us(1_000), Duration::from_millis(16)] {
            for overshoot in [Duration::ZERO, us(3), us(1_500)] {
                let clock = SimClock::new().with_tick(tick).with_overshoot(overshoot);
                clock.advance(us(123));
                clock_check(&clock).assert_ok();
            }
        }
    }

    #[test]
    fn a_frozen_clock_fails_every_delay_but_zero() {
        let frozen = Broken::new(|t| t, |_| 0);
        let report = super::clock(&frozen, &mut &frozen);
        assert_eq!(report.points(), DEFAULT_POINTS);
        assert_eq!(report.failed(), 6);
        assert!(report.failures().all(|f| matches!(
            f,
            ClockFailure::DelayTooShort { elapsed, .. } if *elapsed == Duration::ZERO
        )));
        let first = report.failures().next();
        assert_eq!(
            first,
            Some(&ClockFailure::DelayTooShort {
                requested: Duration::from_nanos(1),
                elapsed: Duration::ZERO
            })
        );
    }

    #[test]
    fn a_delay_that_returns_early_fails() {
        // Waits half of what is asked.
        let hasty = Broken::new(|t| t, |requested| requested / 2);
        let report = super::clock(&hasty, &mut &hasty);
        assert_eq!(report.failed(), 6);
        assert!(report.failures().any(|f| *f
            == ClockFailure::DelayTooShort {
                requested: Duration::from_millis(10),
                elapsed: Duration::from_millis(5)
            }));
    }

    #[test]
    fn a_clock_that_goes_backwards_fails() {
        // Each read is earlier than the last: the time counts down on every read.
        struct Backwards(Cell<u64>);
        impl Clock for Backwards {
            fn now(&self) -> Timestamp {
                self.0.set(self.0.get() - 1);
                Timestamp::from_micros(self.0.get())
            }
        }
        impl Delay for &Backwards {
            fn delay(&mut self, duration: Duration) {
                // The duration, rounded up, plus the step the next read takes back.
                let micros = duration.as_nanos().div_ceil(1_000) as u64;
                self.0.set(self.0.get() + micros + 1);
            }
        }
        let clock = Backwards(Cell::new(1_000_000));
        let report = super::clock(&clock, &mut &clock);
        assert_eq!(
            report.failures().next(),
            Some(&ClockFailure::WentBackwards {
                earlier: Timestamp::from_micros(999_999),
                later: Timestamp::from_micros(999_998),
            })
        );
        // Every read in a row and every read before a delay goes backwards; the delays move
        // the clock forward by their duration plus the read's step, so they pass.
        assert_eq!(report.failed(), 1000 + 7);
        assert_eq!(report.overflow(), 1000 + 7 - 16);
    }

    #[test]
    fn a_clock_that_jumps_back_across_a_delay_fails_both_points() {
        let jumpy = Broken::new(|t| t, |_| 0);
        // The delay moves the clock back by a millisecond.
        struct Back<'a>(&'a Broken);
        impl Delay for Back<'_> {
            fn delay(&mut self, _: Duration) {
                self.0.time.set(self.0.time.get() - 1_000);
            }
        }
        let report = ClockCheck::DEFAULT
            .with_reads(0)
            .with_delays(&[Duration::from_millis(1)])
            .run(&jumpy, &mut Back(&jumpy));
        assert_eq!(report.points(), 3);
        assert!(
            report.failures().eq([
                ClockFailure::WentBackwards {
                    earlier: Timestamp::from_micros(1_000_000),
                    later: Timestamp::from_micros(999_000),
                },
                ClockFailure::DelayTooShort {
                    requested: Duration::from_millis(1),
                    elapsed: Duration::ZERO,
                },
            ]
            .iter())
        );
    }

    #[test]
    fn settings_choose_the_delays_and_reads() {
        let clock = SimClock::new();
        let delays = [us(5), us(50)];
        let check = ClockCheck::DEFAULT.with_reads(3).with_delays(&delays);
        assert_eq!(check.reads, 3);
        assert_eq!(check.delays, &delays);
        let report = check.run(&clock, &mut &clock);
        report.assert_ok();
        assert_eq!(report.points(), 3 + 3 * 2);
        // The check waited what it asked, on the clock it was given.
        assert_eq!(clock.now(), Timestamp::from_micros(55));
    }

    #[test]
    fn nothing_to_run_is_ok() {
        let clock = SimClock::new();
        let report = ClockCheck::DEFAULT
            .with_reads(0)
            .with_delays(&[])
            .run(&clock, &mut &clock);
        assert!(report.is_ok());
        assert_eq!(report.points(), 0);
    }

    #[test]
    fn the_default_runs_the_documented_delays() {
        assert_eq!(ClockCheck::DEFAULT.reads, 1000);
        assert_eq!(
            ClockCheck::DEFAULT.delays.iter().sum::<Duration>(),
            Duration::from_nanos(13_001_001)
        );
    }

    #[test]
    fn failures_print_what_was_seen() {
        let backwards = ClockFailure::WentBackwards {
            earlier: Timestamp::from_micros(10),
            later: Timestamp::from_micros(4),
        };
        assert_eq!(
            backwards.to_string(),
            "the clock went backwards: it read 4 µs after reading 10 µs"
        );
        let short = ClockFailure::DelayTooShort {
            requested: Duration::from_millis(1),
            elapsed: us(500),
        };
        assert_eq!(
            short.to_string(),
            "delay(1ms) advanced the clock only 500µs"
        );
    }

    #[cfg(feature = "async")]
    mod async_form {
        use super::*;

        /// The async delay over a `SimClock`, as the bench's async handles will be.
        struct AsyncDelay<'a>(&'a SimClock);

        impl hdmi_hal_async::time::Delay for AsyncDelay<'_> {
            async fn delay(&mut self, duration: Duration) {
                let mut clock = self.0;
                Delay::delay(&mut clock, duration);
            }
        }

        /// The same through the `Send` twin, which implements `Delay` by its blanket
        /// implementation.
        struct SendAsyncDelay<'a>(&'a SimClock);

        impl hdmi_hal_async::time::SendDelay for SendAsyncDelay<'_> {
            fn delay(&mut self, duration: Duration) -> impl Future<Output = ()> + Send {
                let mut clock = self.0;
                Delay::delay(&mut clock, duration);
                core::future::ready(())
            }
        }

        #[test]
        fn sim_clock_passes_async() {
            let clock = SimClock::new().with_tick(us(1_000));
            let report =
                pollster::block_on(ClockCheck::DEFAULT.run_async(&clock, &mut AsyncDelay(&clock)));
            report.assert_ok();
            assert_eq!(report.points(), DEFAULT_POINTS);
        }

        #[test]
        fn send_twins_are_checked_too() {
            let clock = SimClock::new();
            let report = pollster::block_on(
                ClockCheck::DEFAULT.run_async(&clock, &mut SendAsyncDelay(&clock)),
            );
            report.assert_ok();
        }

        #[test]
        fn a_frozen_async_delay_fails() {
            struct Frozen;
            impl hdmi_hal_async::time::Delay for Frozen {
                async fn delay(&mut self, _: Duration) {}
            }
            let clock = SimClock::new();
            let report = pollster::block_on(ClockCheck::DEFAULT.run_async(&clock, &mut Frozen));
            assert_eq!(report.failed(), 6);
        }
    }

    #[cfg(feature = "std")]
    mod reference {
        use super::*;
        use std::time::Instant;

        /// The host's clock, and a delay that sleeps on it; reading nanoseconds as
        /// microseconds when `fast`, a timer with the wrong frequency.
        struct Host {
            start: Instant,
            fast: bool,
        }

        impl Clock for Host {
            fn now(&self) -> Timestamp {
                let elapsed = self.start.elapsed();
                let micros = if self.fast {
                    elapsed.as_nanos()
                } else {
                    elapsed.as_micros()
                };
                Timestamp::from_micros(micros as u64)
            }
        }

        impl Delay for &Host {
            fn delay(&mut self, duration: Duration) {
                let until = self.now().checked_add(duration).unwrap();
                while self.now() < until {
                    std::thread::yield_now();
                }
            }
        }

        fn host(fast: bool) -> Host {
            Host {
                start: Instant::now(),
                fast,
            }
        }

        /// Wide enough for a busy host between two reads; the cases caught miss by ~10 ms.
        const TOLERANCE: Duration = Duration::from_millis(5);

        #[test]
        fn a_host_clock_passes_against_the_host() {
            let clock = host(false);
            let report = ClockCheck::DEFAULT.run_with_reference(
                &clock,
                &mut &clock,
                &HostClock::new(),
                TOLERANCE,
            );
            report.assert_ok();
            // Each delay adds the reference's two points.
            assert_eq!(report.points(), DEFAULT_POINTS + 2 * 7);
        }

        #[test]
        fn a_clock_1000_times_fast_is_caught() {
            let clock = host(true);
            let failures: std::vec::Vec<_> = ClockCheck::DEFAULT
                .run_with_reference(&clock, &mut &clock, &HostClock::new(), TOLERANCE)
                .failures()
                .copied()
                .collect();
            // Its 10 ms delay takes about 10 µs.
            assert!(failures.iter().any(|failure| matches!(
                failure,
                ClockFailure::DelayTooShortOnReference { requested, .. }
                    if *requested == Duration::from_millis(10)
            )));
            assert!(failures.iter().any(|failure| matches!(
                failure,
                ClockFailure::Drift { requested, .. } if *requested == Duration::from_millis(10)
            )));
        }

        #[cfg(feature = "async")]
        #[test]
        fn the_async_form_measures_on_the_reference_too() {
            let bench = crate::Bench::new(crate::SinkConfig::new(), &[]).unwrap();
            let delays = [Duration::from_millis(10)];
            let check = ClockCheck::DEFAULT.with_delays(&delays);
            let report = pollster::block_on(check.run_with_reference_async(
                &bench.time(),
                &mut bench.time(),
                &HostClock::new(),
                TOLERANCE,
            ));
            // The bench's time is simulated: its delay takes no host time.
            assert!(
                report.failures().any(|failure| matches!(
                    failure,
                    ClockFailure::DelayTooShortOnReference { .. }
                ))
            );
        }

        #[test]
        fn simulated_time_is_not_host_time() {
            let clock = SimClock::new();
            let report = ClockCheck::DEFAULT
                .with_delays(&[Duration::from_millis(10)])
                .run_with_reference(&clock, &mut &clock, &HostClock::new(), TOLERANCE);
            assert!(
                report.failures().any(|failure| matches!(
                    failure,
                    ClockFailure::DelayTooShortOnReference { .. }
                ))
            );
            assert!(HostClock::default().now() < Timestamp::from_micros(1_000_000));
        }
    }

    #[test]
    fn reference_failures_print_what_went_wrong() {
        let short = ClockFailure::DelayTooShortOnReference {
            requested: us(10_000),
            elapsed: us(10),
        };
        assert_eq!(
            short.to_string(),
            "delay(10ms) took only 10µs on the reference"
        );
        let drift = ClockFailure::Drift {
            requested: us(10_000),
            clock: us(10_000),
            reference: us(10),
        };
        assert_eq!(
            drift.to_string(),
            "across delay(10ms) the clock advanced 10ms, the reference 10µs"
        );
    }
}
