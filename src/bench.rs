//! The bench: the sink, the clock and the line as one simulation, and its handles.

use core::cell::RefCell;
use core::fmt;
use core::time::Duration;

use critical_section::Mutex;
use hdmi_hal::ddc::{DdcCapabilities, DdcErrorKind, NoAcknowledgeSource};
use hdmi_hal::time::{Clock, Delay, Timestamp};

use crate::clock::SimClock;
use crate::config::SinkConfig;
use crate::ddc::{BusTiming, Ddc};
use crate::target::{Env, Operation, Target};
use crate::transcript::{BusRecord, Event, Record, Transcript};

/// The test's own targets, as given to the bench.
type Targets<'a> = &'a mut [&'a mut (dyn Target + Send)];

/// The largest EDID or DisplayID store: 256 blocks of 128 bytes.
const MAX_STORE: usize = 256 * 128;

/// A simulated sink, its clock and its line, and the handles a procedure runs over.
///
/// The bench holds the whole simulation; the trait implementations are `Copy` handles
/// borrowed from it ([`ddc`](Self::ddc), [`time`](Self::time)), so one value goes to each
/// argument of a procedure, and the test's controls work while the handles are lent out.
///
/// `LOG` is the transcript's capacity in records and `FAULTS` the fault schedule's; the
/// records are stored inline, so the bench needs no `alloc`. [`new`](Bench::new) and
/// [`builder`](Bench::builder) use the defaults; a bare target with a small stack picks
/// smaller ones with [`BenchBuilder::capacities`].
///
/// The state sits behind a [`critical_section::Mutex`], so the bench is `Sync` and every
/// handle `Send + Sync`; a host needs the `std` feature (or another critical-section
/// implementation).
///
/// ```
/// use maquette::{Bench, DdcBus, SinkConfig};
///
/// let edid = [0u8; 256];
/// let bench = Bench::new(SinkConfig::new(), &edid).unwrap();
/// let bus = bench.ddc();
/// // Nothing answers yet at 0x37: no target there.
/// assert!(bus.write(0x37, &[0x51]).is_err());
/// assert_eq!(bench.transcript().count(), 1);
/// ```
pub struct Bench<'a, const LOG: usize = 256, const FAULTS: usize = 16> {
    clock: SimClock,
    capabilities: DdcCapabilities,
    timing: BusTiming,
    state: Mutex<RefCell<State<'a, LOG>>>,
}

/// What the lock guards.
struct State<'a, const LOG: usize> {
    config: SinkConfig,
    edid: &'a [u8],
    targets: Targets<'a>,
    hpd: bool,
    transcript: Transcript<LOG>,
}

impl<'a> Bench<'a> {
    /// A bench with the default capacities: `config`'s sink with `edid`, a bus with
    /// `DdcCapabilities::general()` and standard-mode timing, and a clock at zero.
    pub fn new(config: SinkConfig, edid: &'a [u8]) -> Result<Self, BenchError> {
        Self::builder(edid).sink(config).build()
    }

    /// A builder for everything else, starting from the defaults [`new`](Self::new) uses.
    pub fn builder(edid: &'a [u8]) -> BenchBuilder<'a> {
        BenchBuilder {
            edid,
            config: SinkConfig::new(),
            capabilities: DdcCapabilities::general(),
            timing: BusTiming::STANDARD_MODE,
            clock: SimClock::new(),
            targets: Default::default(),
        }
    }
}

impl<'a, const LOG: usize, const FAULTS: usize> Bench<'a, LOG, FAULTS> {
    /// The sink's DDC bus.
    pub fn ddc(&self) -> Ddc<'_> {
        Ddc::new(self)
    }

    /// The bench's clock, as a `Clock` and a `Delay`: one value passed as both.
    pub fn time(&self) -> Time<'_> {
        Time { clock: &self.clock }
    }

    /// Moves the bench's time `duration` forward, then lets every target see the new time
    /// ([`Target::update`]), in routing order.
    pub fn advance(&self, duration: Duration) {
        self.clock.advance(duration);
        let at = self.clock.now();
        critical_section::with(|cs| {
            let mut state = self.state.borrow_ref_mut(cs);
            let State {
                targets,
                transcript,
                hpd,
                ..
            } = &mut *state;
            let mut env = Env::new(at, *hpd, transcript);
            for target in targets.iter_mut() {
                target.update(&mut env);
            }
        });
    }

    /// The bench's time.
    pub fn now(&self) -> Timestamp {
        self.clock.now()
    }

    /// The transcript's records, oldest first; each is copied out under the lock, so the
    /// bench can be used while iterating.
    pub fn transcript(&self) -> impl Iterator<Item = Record> + '_ {
        let mut index = 0;
        core::iter::from_fn(move || {
            let record =
                critical_section::with(|cs| self.state.borrow_ref(cs).transcript.get(index));
            index += 1;
            record
        })
    }

    /// How many records did not fit in the transcript, past `LOG`.
    pub fn transcript_overflow(&self) -> usize {
        critical_section::with(|cs| self.state.borrow_ref(cs).transcript.overflow())
    }

    /// Empties the transcript, and its overflow count.
    pub fn clear_transcript(&self) {
        critical_section::with(|cs| self.state.borrow_ref_mut(cs).transcript.clear());
    }
}

impl<const LOG: usize, const FAULTS: usize> fmt::Debug for Bench<'_, LOG, FAULTS> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        critical_section::with(|cs| {
            let state = self.state.borrow_ref(cs);
            f.debug_struct("Bench")
                .field("now", &self.clock.now())
                .field("config", &state.config)
                .field("edid_len", &state.edid.len())
                .field("targets", &state.targets.len())
                .field("hpd", &state.hpd)
                .field("transcript_len", &state.transcript.len())
                .field("transcript_overflow", &state.transcript.overflow())
                .finish_non_exhaustive()
        })
    }
}

/// What the handles reach the bench through, whatever its capacities.
pub(crate) trait Core {
    fn capabilities(&self) -> DdcCapabilities;
    fn timing(&self) -> BusTiming;
    fn clock(&self) -> &SimClock;
    /// Records `record` at the bench's time, routes `ops` as one transaction and completes
    /// the record with the result; returns the result and whether a target answered.
    fn transaction(
        &self,
        record: BusRecord,
        ops: &mut [Operation<'_>],
    ) -> (Result<(), DdcErrorKind>, bool);
    /// Records a call that reached no target.
    fn record(&self, record: BusRecord);
}

impl<const LOG: usize, const FAULTS: usize> Core for Bench<'_, LOG, FAULTS> {
    fn capabilities(&self) -> DdcCapabilities {
        self.capabilities
    }

    fn timing(&self) -> BusTiming {
        self.timing
    }

    fn clock(&self) -> &SimClock {
        &self.clock
    }

    fn transaction(
        &self,
        record: BusRecord,
        ops: &mut [Operation<'_>],
    ) -> (Result<(), DdcErrorKind>, bool) {
        let at = self.clock.now();
        critical_section::with(|cs| {
            let mut state = self.state.borrow_ref_mut(cs);
            let State {
                targets,
                transcript,
                hpd,
                ..
            } = &mut *state;
            let index = transcript.push(Record {
                at,
                event: Event::Bus(record),
            });
            let addr = ops.first().map_or(0, Operation::addr);
            let mut env = Env::new(at, *hpd, transcript);
            let (result, answered) = match targets.iter_mut().find(|t| t.answers(addr, &env)) {
                Some(target) => (target.transaction(ops, &mut env), true),
                None => (
                    Err(DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Address)),
                    false,
                ),
            };
            transcript.set_result(index, result);
            (result, answered)
        })
    }

    fn record(&self, record: BusRecord) {
        let at = self.clock.now();
        critical_section::with(|cs| {
            self.state.borrow_ref_mut(cs).transcript.push(Record {
                at,
                event: Event::Bus(record),
            });
        });
    }
}

/// Builds a [`Bench`]: [`Bench::builder`], the `with`-style setters, then
/// [`build`](Self::build).
#[must_use]
pub struct BenchBuilder<'a, const LOG: usize = 256, const FAULTS: usize = 16> {
    edid: &'a [u8],
    config: SinkConfig,
    capabilities: DdcCapabilities,
    timing: BusTiming,
    clock: SimClock,
    targets: Targets<'a>,
}

impl<'a, const LOG: usize, const FAULTS: usize> BenchBuilder<'a, LOG, FAULTS> {
    /// The sink's settings.
    pub fn sink(self, config: SinkConfig) -> Self {
        Self { config, ..self }
    }

    /// What the bench's DDC bus can do; by default `DdcCapabilities::general()`.
    pub fn ddc_capabilities(self, capabilities: DdcCapabilities) -> Self {
        Self {
            capabilities,
            ..self
        }
    }

    /// The bench's clock; by default `SimClock::new()`.
    pub fn clock(self, clock: SimClock) -> Self {
        Self { clock, ..self }
    }

    /// How long bus operations take; by default [`BusTiming::STANDARD_MODE`].
    pub fn bus_timing(self, timing: BusTiming) -> Self {
        Self { timing, ..self }
    }

    /// The test's own targets, routed to before the built-in ones, in this order.
    pub fn targets(self, targets: &'a mut [&'a mut (dyn Target + Send)]) -> Self {
        Self { targets, ..self }
    }

    /// The builder for a bench with a transcript of `L` records and a fault schedule of `F`.
    pub fn capacities<const L: usize, const F: usize>(self) -> BenchBuilder<'a, L, F> {
        BenchBuilder {
            edid: self.edid,
            config: self.config,
            capabilities: self.capabilities,
            timing: self.timing,
            clock: self.clock,
            targets: self.targets,
        }
    }

    /// The bench, or why the stores cannot be its sink's.
    pub fn build(self) -> Result<Bench<'a, LOG, FAULTS>, BenchError> {
        let len = self.edid.len();
        if len % 128 != 0 || len > MAX_STORE {
            return Err(BenchError::EdidLength(len));
        }
        Ok(Bench {
            clock: self.clock,
            capabilities: self.capabilities,
            timing: self.timing,
            state: Mutex::new(RefCell::new(State {
                config: self.config,
                edid: self.edid,
                targets: self.targets,
                hpd: true,
                transcript: Transcript::new(),
            })),
        })
    }
}

impl<const LOG: usize, const FAULTS: usize> fmt::Debug for BenchBuilder<'_, LOG, FAULTS> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BenchBuilder")
            .field("edid_len", &self.edid.len())
            .field("config", &self.config)
            .field("capabilities", &self.capabilities)
            .field("timing", &self.timing)
            .field("clock", &self.clock)
            .field("targets", &self.targets.len())
            .finish()
    }
}

/// Why a bench cannot be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BenchError {
    /// The EDID store is not a whole number of 128-byte blocks, or is longer than 256; the
    /// length given.
    EdidLength(usize),
    /// The DisplayID store is not a whole number of 128-byte blocks, or is longer than 256;
    /// the length given.
    DisplayIdLength(usize),
}

impl fmt::Display for BenchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (store, len) = match self {
            Self::EdidLength(len) => ("EDID", len),
            Self::DisplayIdLength(len) => ("DisplayID", len),
        };
        write!(
            f,
            "an {store} store of {len} bytes: a store is a whole number of 128-byte blocks, \
             at most 256"
        )
    }
}

impl core::error::Error for BenchError {}

/// The bench's clock as a `Clock` and a `Delay`, a `Copy` handle borrowed from the bench
/// ([`Bench::time`]): one value passed as both.
#[derive(Debug, Clone, Copy)]
pub struct Time<'b> {
    clock: &'b SimClock,
}

impl Clock for Time<'_> {
    fn now(&self) -> Timestamp {
        self.clock.now()
    }
}

impl Delay for Time<'_> {
    fn delay(&mut self, duration: Duration) {
        Delay::delay(&mut self.clock, duration);
    }
}
