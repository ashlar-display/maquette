//! The bench: the sink, the clock and the line as one simulation, and its handles.

use core::cell::RefCell;
use core::fmt;
use core::time::Duration;

use critical_section::Mutex;
use hdmi_hal::ddc::{DdcCapabilities, DdcErrorKind, NoAcknowledgeSource};
use hdmi_hal::phy::TmdsPhyCapabilities;
use hdmi_hal::time::{Clock, Delay, Timestamp};

use crate::action::{Action, When};
use crate::clock::SimClock;
use crate::config::SinkConfig;
use crate::ddc::{BusTiming, Ddc};
use crate::eddc::Eddc;
use crate::fault::{Effect, Fault, FaultId, Moment, Schedule, ScheduleFull, Strike};
use crate::hook::ScdcHook;
use crate::line::{ErrorRates, Line, LinkLanes};
use crate::phy::{Phy, PhyCall, PhyErrorKind, PhyFault, PhySettings, PhyState};
use crate::scdc::Scdc;
use crate::target::{Env, Operation, Target};
use crate::transcript::{BusRecord, Event, Record, Transcript};

/// The targets in routing order: the test's, in the order given, then the built-in ones.
fn routed<'s>(
    targets: &'s mut Targets<'_>,
    eddc: &'s mut Eddc<'_>,
    scdc: &'s mut Option<Scdc>,
) -> impl Iterator<Item = &'s mut dyn Target> {
    targets
        .iter_mut()
        .map(|target| &mut **target as &mut dyn Target)
        .chain(core::iter::once(eddc as &mut dyn Target))
        .chain(scdc.as_mut().map(|scdc| scdc as &mut dyn Target))
}

/// The test's own targets, as given to the bench.
type Targets<'a> = &'a mut [&'a mut (dyn Target + Send)];

/// The largest EDID or DisplayID store: 256 blocks of 128 bytes.
const MAX_STORE: usize = 256 * 128;

/// The default operation budget (*chosen*): far more than a bring-up, a training run or a
/// long session of polling takes, and spent within seconds by a loop that never ends.
pub const DEFAULT_BUDGET: u64 = 1_000_000;

/// The simulated PHY's capabilities by default: 600 000 kHz, with a scrambler that also
/// scrambles at or below 340 Mcsc.
pub const DEFAULT_PHY_CAPABILITIES: TmdsPhyCapabilities = TmdsPhyCapabilities::new(600_000)
    .with_scrambling(true)
    .with_low_rate_scrambling(true);

/// Checks the stores' lengths: whole 128-byte blocks, up to 256.
/// The line as the sink receives it: nothing while it is unplugged.
const fn sink_line(plugged: bool, line: Line) -> Line {
    if plugged { line } else { Line::Off }
}

/// The character errors as the sink receives them: none while it is unplugged.
const fn sink_errors(plugged: bool, errors: ErrorRates) -> ErrorRates {
    if plugged { errors } else { ErrorRates::NONE }
}

fn check_stores(edid: &[u8], displayid: Option<&[u8]>) -> Result<(), BenchError> {
    let whole_blocks = |len: usize| len % 128 == 0 && len <= MAX_STORE;
    if !whole_blocks(edid.len()) {
        return Err(BenchError::EdidLength(edid.len()));
    }
    match displayid {
        Some(store) if !whole_blocks(store.len()) => Err(BenchError::DisplayIdLength(store.len())),
        _ => Ok(()),
    }
}

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
/// Every operation through the handles counts against the bench's operation budget
/// ([`BenchBuilder::budget`]); past it the bench panics, so a procedure that loops forever
/// fails its test instead of hanging it.
///
/// ```
/// use maquette::{Bench, DdcBus, SinkConfig};
///
/// let edid = [0u8; 256];
/// let bench = Bench::new(SinkConfig::new(), &edid).unwrap();
/// let bus = bench.ddc();
/// // Nothing answers at 0x37: no target there.
/// assert!(bus.write(0x37, &[0x51]).is_err());
/// assert_eq!(bench.transcript().count(), 1);
/// ```
pub struct Bench<'a, const LOG: usize = 256, const FAULTS: usize = 16> {
    clock: SimClock,
    capabilities: DdcCapabilities,
    timing: BusTiming,
    budget: Option<u64>,
    state: Mutex<RefCell<State<'a, LOG, FAULTS>>>,
}

/// What the lock guards.
struct State<'a, const LOG: usize, const FAULTS: usize> {
    config: SinkConfig,
    eddc: Eddc<'a>,
    targets: Targets<'a>,
    scdc: Option<Scdc>,
    /// The test's SCDC hook: it outlives a plug, which makes a fresh `scdc`.
    scdc_hook: Option<Hook<'a>>,
    line: Line,
    /// The character errors on the link, and its lanes' limits.
    errors: ErrorRates,
    lanes: LinkLanes,
    hpd: bool,
    plugged: bool,
    attached: bool,
    spent: u64,
    transcript: Transcript<LOG>,
    faults: Schedule<'a, FAULTS>,
    /// The fault a check armed through `FailNext`, until it withdraws it.
    armed: Option<FaultId>,
    phy: PhyState,
    /// What the PHY last put on the line: it drives the line only when this changes, so a
    /// test's own `set_line` holds until the PHY's output changes.
    phy_line: Line,
}

impl<'a, const LOG: usize, const FAULTS: usize> State<'a, LOG, FAULTS> {
    fn record(&mut self, at: Timestamp, event: Event) {
        self.transcript.push(Record { at, event });
    }

    /// Lets every target see the bench at `at`, in routing order.
    fn update(&mut self, at: Timestamp) {
        let State {
            targets,
            eddc,
            scdc,
            transcript,
            hpd,
            line,
            errors,
            lanes,
            plugged,
            ..
        } = self;
        let mut env = Env::new(at, *hpd, sink_line(*plugged, *line), transcript)
            .with_errors(sink_errors(*plugged, *errors))
            .with_lanes(*lanes);
        for target in routed(targets, eddc, scdc) {
            target.update(&mut env);
        }
        self.faults.time_actions(at);
        self.run_actions(at, Moment::Time);
    }

    /// Counts `record`'s operation for the scheduled faults and returns the one that
    /// strikes, if any. While detached nothing goes out, so nothing is counted.
    fn strike(&mut self, at: Timestamp, record: &BusRecord) -> Option<Strike> {
        self.faults.time_actions(at);
        if self.attached {
            self.faults.strike(record, at)
        } else {
            None
        }
    }

    /// Records `record` failed by a fault that struck before the sink saw it.
    fn struck_before(
        &mut self,
        at: Timestamp,
        record: BusRecord,
        strike: Strike,
        kind: DdcErrorKind,
    ) {
        self.record(
            at,
            Event::Bus(BusRecord {
                result: Err(kind),
                ..record
            }),
        );
        self.record(at, fault_event(strike));
    }

    /// Records `record` at `at` and routes `ops` as one transaction; a fault that strikes
    /// after or partway replaces its result.
    fn transaction(
        &mut self,
        at: Timestamp,
        per_byte: Duration,
        record: BusRecord,
        ops: &mut [Operation<'_>],
        fault: Option<Strike>,
    ) -> (Result<(), DdcErrorKind>, Reach) {
        if !self.attached {
            let result = Err(DdcErrorKind::Detached);
            self.record(at, Event::Bus(BusRecord { result, ..record }));
            return (result, Reach::Detached);
        }
        let index = self.transcript.push(Record {
            at,
            event: Event::Bus(record),
        });
        let unanswered = (
            Err(DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Address)),
            Reach::Unanswered,
        );
        let (result, reach) = if self.plugged {
            let State {
                targets,
                eddc,
                scdc,
                scdc_hook,
                transcript,
                hpd,
                line,
                errors,
                lanes,
                ..
            } = self;
            let addr = ops.first().map_or(0, Operation::addr);
            let mut env = Env::new(at, *hpd, *line, transcript)
                .with_errors(*errors)
                .with_lanes(*lanes)
                .with_byte_time(per_byte);
            let first = targets
                .iter_mut()
                .map(|target| &mut **target as &mut dyn Target)
                .chain(core::iter::once(eddc as &mut dyn Target))
                .find(|t| t.answers(addr, &env));
            match (first, scdc) {
                (Some(target), _) => (target.transaction(ops, &mut env), Reach::Answered),
                // The built-in SCDC target, last in routing order, with the test's hook.
                (None, Some(scdc)) if scdc.answers(addr, &env) => (
                    scdc.hooked_transaction(
                        ops,
                        &mut env,
                        scdc_hook
                            .as_deref_mut()
                            .map(|hook| hook as &mut (dyn ScdcHook + Send)),
                    ),
                    Reach::Answered,
                ),
                _ => unanswered,
            }
        } else {
            unanswered
        };
        let result = match fault {
            Some(Strike {
                effect: Effect::Fail(kind),
                ..
            }) => Err(kind),
            _ => result,
        };
        self.transcript.set_result(index, result);
        if let Some(strike) = fault {
            self.record(at, fault_event(strike));
        }
        (result, reach)
    }

    /// A call of the simulated PHY: counted for the PHY faults, applied, recorded, and a
    /// bench event; the line follows the PHY's output when it changes. Returns the result
    /// and whether the call took effect before a fault failed it.
    fn phy_call(&mut self, call: PhyCall, clock: &SimClock) -> (Result<(), PhyErrorKind>, bool) {
        let strike = self.faults.strike_phy(&call, clock.now());
        self.faults.phy_actions(&call);
        let was_running = self.phy.is_running();
        let (result, took_effect) = match strike {
            Some((_, false)) => (Err(PhyErrorKind::Fault), false),
            Some((_, true)) => match self.phy.apply(call) {
                Ok(()) => (Err(PhyErrorKind::Fault), true),
                Err(kind) => (Err(kind), false),
            },
            None => (self.phy.apply(call), false),
        };
        if !was_running && self.phy.is_running() {
            // `start_output` returns once the output is on the wire.
            clock.advance(self.phy.start_latency());
        }
        let at = clock.now();
        self.record(at, Event::Phy { call, result });
        let line = self.phy.line();
        if line != self.phy_line {
            self.phy_line = line;
            if line != self.line {
                self.line = line;
                self.record(at, Event::Line(line));
            }
        }
        self.update(at);
        self.run_actions(at, Moment::After);
        (result, took_effect)
    }

    /// Unplugs the sink at `at`, if it is plugged.
    fn unplug(&mut self, at: Timestamp) {
        if !self.plugged {
            return;
        }
        self.plugged = false;
        self.record(at, Event::Plug { plugged: false });
        if self.hpd {
            self.hpd = false;
            self.record(at, Event::Hpd { high: false });
        }
        self.update(at);
    }

    /// Plugs a sink with checked stores in at `at`.
    fn plug(
        &mut self,
        at: Timestamp,
        edid: &'a [u8],
        displayid: Option<&'a [u8]>,
        config: SinkConfig,
    ) {
        self.unplug(at);
        self.config = config;
        self.eddc = Eddc::new(&config, edid, displayid);
        self.scdc = Scdc::new(&config);
        self.plugged = true;
        self.record(at, Event::Plug { plugged: true });
        self.hpd = true;
        self.record(at, Event::Hpd { high: true });
        self.update(at);
    }

    /// Sets the line at `at`; the same line changes nothing.
    fn set_line(&mut self, at: Timestamp, line: Line) {
        if self.line != line {
            self.line = line;
            self.record(at, Event::Line(line));
            self.update(at);
        }
    }

    /// Sets the character errors at `at`; the same rates change nothing.
    fn set_character_errors(&mut self, at: Timestamp, errors: ErrorRates) {
        if self.errors != errors {
            self.errors = errors;
            self.record(at, Event::CharacterErrors(errors));
            self.update(at);
        }
    }

    /// Sets the lane limits at `at`; the same limits change nothing.
    fn set_lane_limits(&mut self, at: Timestamp, lanes: LinkLanes) {
        if self.lanes != lanes {
            self.lanes = lanes;
            self.record(at, Event::LaneLimits(lanes));
            self.update(at);
        }
    }

    /// Sets HPD at `at`; false when raising it on an unplugged sink, which changes nothing.
    fn set_hpd(&mut self, at: Timestamp, high: bool) -> bool {
        if high && !self.plugged {
            return false;
        }
        if self.hpd != high {
            self.hpd = high;
            self.record(at, Event::Hpd { high });
            self.update(at);
        }
        true
    }

    /// Attaches or detaches the adapter at `at`.
    fn set_attached(&mut self, at: Timestamp, attached: bool) {
        if self.attached != attached {
            self.attached = attached;
            self.record(at, Event::Adapter { attached });
            self.update(at);
        }
    }

    /// Presets an SCDC register at `at`; false on a sink without SCDC, which changes nothing.
    fn preset(&mut self, at: Timestamp, reg: u8, value: u8) -> bool {
        let State {
            scdc,
            transcript,
            hpd,
            line,
            errors,
            lanes,
            plugged,
            ..
        } = self;
        let Some(scdc) = scdc else {
            return false;
        };
        let line = sink_line(*plugged, *line);
        let mut env = Env::new(at, *hpd, line, transcript)
            .with_errors(sink_errors(*plugged, *errors))
            .with_lanes(*lanes);
        scdc.preset(reg, value, &mut env);
        self.update(at);
        true
    }

    /// Runs the due actions of the moment (`before` an operation, or after one), each
    /// recorded first.
    fn run_actions(&mut self, at: Timestamp, moment: Moment) {
        while let Some((id, action)) = self.faults.take_due(moment) {
            self.record(at, Event::Action { id });
            match action {
                Action::Preset { reg, value } => {
                    self.preset(at, reg, value);
                }
                Action::Hpd(high) => {
                    self.set_hpd(at, high);
                }
                Action::Unplug => self.unplug(at),
                Action::Plug {
                    edid,
                    displayid,
                    config,
                } => self.plug(at, edid, displayid, config),
                Action::Line(line) => self.set_line(at, line),
                Action::CharacterErrors(errors) => self.set_character_errors(at, errors),
                Action::LaneLimits(lanes) => self.set_lane_limits(at, lanes),
                Action::Detach => self.set_attached(at, false),
                Action::Reattach => self.set_attached(at, true),
            }
        }
    }
}

/// The transcript's record of a fault that struck.
fn fault_event(strike: Strike) -> Event {
    Event::Fault {
        id: strike.id,
        effect: strike.effect,
        point: strike.point,
    }
}

/// How far a transaction got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reach {
    /// A target took it.
    Answered,
    /// Nothing answered its first address: only that address byte went out.
    Unanswered,
    /// The adapter is gone: nothing went out.
    Detached,
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
            displayid: None,
            config: SinkConfig::new(),
            capabilities: DdcCapabilities::general(),
            timing: BusTiming::STANDARD_MODE,
            clock: SimClock::new(),
            budget: Some(DEFAULT_BUDGET),
            targets: Default::default(),
            scdc_hook: None,
            phy_capabilities: DEFAULT_PHY_CAPABILITIES,
            phy_settings: PhySettings::DEFAULT,
            keep_newest: false,
        }
    }
}

impl<'a, const LOG: usize, const FAULTS: usize> Bench<'a, LOG, FAULTS> {
    /// Runs `f` on the state under the lock, with the bench's time read under it.
    ///
    /// # Panics
    ///
    /// When the bench is already locked: a target, hook or check called back into the bench
    /// through its handles or controls while the bench ran it.
    fn locked<R>(&self, f: impl FnOnce(&mut State<'a, LOG, FAULTS>, Timestamp) -> R) -> R {
        critical_section::with(|cs| {
            let Ok(mut state) = self.state.borrow(cs).try_borrow_mut() else {
                panic!(
                    "the bench is locked: a target or hook used the bench's handles or \
                     controls while the bench ran it (use Env instead)"
                );
            };
            f(&mut state, self.clock.now())
        })
    }

    /// The sink's DDC bus.
    pub fn ddc(&self) -> Ddc<'_> {
        Ddc::new(self)
    }

    /// The source's simulated PHY: `TmdsPhy`, `FrlPhy` and `PhyPower`, driving the line.
    pub fn phy(&self) -> Phy<'_> {
        Phy::new(self)
    }

    /// The bench's clock, as a `Clock` and a `Delay`: one value passed as both.
    pub fn time(&self) -> Time<'_> {
        Time { core: self }
    }

    /// The sink as the test sees it: its registers and HPD, read without a bus operation.
    pub fn sink(&self) -> Sink<'_> {
        Sink { core: self }
    }

    /// Moves the bench's time `duration` forward, then lets every target see the new time
    /// ([`Target::update`]), in routing order.
    pub fn advance(&self, duration: Duration) {
        self.clock.advance(duration);
        self.locked(|state, at| state.update(at));
    }

    /// The bench's time.
    pub fn now(&self) -> Timestamp {
        self.clock.now()
    }

    /// Sets what the source transmits: a bench event, recorded as `Event::Line`. Setting the
    /// line it already has changes nothing and records nothing.
    pub fn set_line(&self, line: Line) {
        self.locked(|state, at| state.set_line(at, line));
    }

    /// What the source transmits.
    pub fn line(&self) -> Line {
        self.locked(|state, _| state.line)
    }

    /// Sets the character errors on the link, per lane: a bench event, recorded as
    /// `Event::CharacterErrors`. The sink's counters count them while the lane is locked;
    /// setting the rates the link already has changes nothing and records nothing.
    pub fn set_character_errors(&self, errors: ErrorRates) {
        self.locked(|state, at| state.set_character_errors(at, errors));
    }

    /// The character errors on the link.
    pub fn character_errors(&self) -> ErrorRates {
        self.locked(|state, _| state.errors)
    }

    /// Sets what each lane of the link can carry in FRL link training: a bench event,
    /// recorded as `Event::LaneLimits`. Setting the limits the link already has changes
    /// nothing and records nothing.
    pub fn set_lane_limits(&self, lanes: LinkLanes) {
        self.locked(|state, at| state.set_lane_limits(at, lanes));
    }

    /// What each lane of the link can carry in FRL link training.
    pub fn lane_limits(&self) -> LinkLanes {
        self.locked(|state, _| state.lanes)
    }

    /// Sets HPD: a bench event, recorded as `Event::Hpd`; setting the level it already has
    /// records nothing. The sink stays on the bus; what answers while HPD is low is each
    /// target's.
    ///
    /// # Panics
    ///
    /// When raising HPD on an unplugged sink: a mistake in the test, which plugs it instead.
    pub fn set_hpd(&self, high: bool) {
        let unplugged = !self.locked(|state, at| state.set_hpd(at, high));
        if unplugged {
            panic!("HPD raised on an unplugged sink: plug it first");
        }
    }

    /// HPD low, `low_for` on the bench's clock ([`advance`](Self::advance)), then HPD high;
    /// HPD stays low if a scheduled action unplugged the sink during the low.
    pub fn hpd_pulse(&self, low_for: Duration) {
        self.set_hpd(false);
        self.advance(low_for);
        self.locked(|state, at| state.set_hpd(at, true));
    }

    /// Unplugs the sink: HPD goes low, and nothing answers, the test's own targets included,
    /// until [`plug`](Self::plug). Recorded as `Event::Plug`; unplugging an unplugged sink
    /// records nothing.
    pub fn unplug(&self) {
        self.locked(|state, at| state.unplug(at));
    }

    /// Plugs a sink in: `edid`, `displayid` and `config`'s settings, its registers fresh, HPD
    /// high. On a plugged sink it is an unplug first: a different sink, or the same one
    /// after a power cycle. The test's own targets stay. Recorded as `Event::Plug` and
    /// `Event::Hpd`.
    pub fn plug(
        &self,
        edid: &'a [u8],
        displayid: Option<&'a [u8]>,
        config: SinkConfig,
    ) -> Result<(), BenchError> {
        check_stores(edid, displayid)?;
        self.locked(|state, at| state.plug(at, edid, displayid, config));
        Ok(())
    }

    /// Detaches the adapter: every `DdcBus` call that passes the argument and capability
    /// checks fails `Detached`, reaches no target and takes no time, until
    /// [`reattach`](Self::reattach). Recorded as `Event::Adapter`.
    pub fn detach(&self) {
        self.set_attached(false);
    }

    /// Reattaches the adapter.
    pub fn reattach(&self) {
        self.set_attached(true);
    }

    fn set_attached(&self, attached: bool) {
        self.locked(|state, at| state.set_attached(at, attached));
    }

    /// Whether the adapter is attached.
    pub fn is_attached(&self) -> bool {
        self.locked(|state, _| state.attached)
    }

    /// How many operations the handles have made: what the budget counts.
    pub fn operations(&self) -> u64 {
        self.locked(|state, _| state.spent)
    }

    /// Schedules `fault`; it counts the operations from now. `ScheduleFull` when the bench's
    /// `FAULTS` faults are scheduled.
    pub fn schedule(&self, fault: Fault) -> Result<FaultId, ScheduleFull> {
        self.locked(|state, _| state.faults.add(fault))
    }

    /// Schedules a fault of the simulated PHY, in the same schedule as the bus's faults; it
    /// counts the calls from now. `ScheduleFull` when the bench's `FAULTS` faults are
    /// scheduled.
    pub fn schedule_phy(&self, fault: PhyFault) -> Result<FaultId, ScheduleFull> {
        self.locked(|state, _| state.faults.add_phy(fault))
    }

    /// Schedules `action` to happen by itself `when` the test says, during a procedure; it
    /// counts from now, and shares the schedule (and its `FAULTS` slots) with the faults.
    /// [`clear_fault`](Self::clear_fault) with the id it returns removes it before it
    /// comes. `ScheduleFull` when the bench's `FAULTS` slots are taken.
    ///
    /// # Panics
    ///
    /// For an [`Action::Plug`] whose stores are not whole 128-byte blocks, up to 256: a
    /// mistake in the test, which [`plug`](Self::plug) would refuse.
    pub fn schedule_action(&self, action: Action<'a>, when: When) -> Result<FaultId, ScheduleFull> {
        if let Action::Plug {
            edid, displayid, ..
        } = action
        {
            if let Err(error) = check_stores(edid, displayid) {
                panic!("a scheduled plug's stores: {error}");
            }
        }
        self.locked(|state, _| state.faults.add_action(action, when))
    }

    /// Removes a scheduled fault; whether it was still scheduled.
    pub fn clear_fault(&self, id: FaultId) -> bool {
        self.locked(|state, _| state.faults.remove(id))
    }

    /// Schedules `fault` for a check, remembered until [`withdraw_armed`]; whether there was
    /// room. A fault armed before and not withdrawn is withdrawn first.
    ///
    /// [`withdraw_armed`]: Self::withdraw_armed
    pub(crate) fn arm(&self, fault: Fault) -> bool {
        self.locked(|state, _| {
            if let Some(id) = state.armed.take() {
                state.faults.remove(id);
            }
            state.armed = state.faults.add(fault).ok();
            state.armed.is_some()
        })
    }

    /// Removes the fault [`arm`](Self::arm) scheduled, if no operation has taken it yet.
    pub(crate) fn withdraw_armed(&self) {
        self.locked(|state, _| {
            if let Some(id) = state.armed.take() {
                state.faults.remove(id);
            }
        });
    }

    /// Removes every scheduled fault.
    pub fn clear_faults(&self) {
        self.locked(|state, _| state.faults.clear());
    }

    /// How many faults are scheduled.
    pub fn scheduled_faults(&self) -> usize {
        self.locked(|state, _| state.faults.len())
    }

    /// Sets the SCDC register `reg` to `value`, whatever its role, the sink's own registers
    /// included; recorded as a `Register` event by `Actor::Test`. A bench event: the sink
    /// evaluates its behaviour after it, so a preset of `TMDS_Config` can lose or start the
    /// scrambling lock; a preset of the status holds until the lock is gained or lost.
    ///
    /// # Panics
    ///
    /// On a sink without SCDC: a preset there is a mistake in the test.
    pub fn preset_scdc(&self, reg: u8, value: u8) {
        let missing = !self.locked(|state, at| state.preset(at, reg, value));
        if missing {
            panic!("preset of SCDC register {reg:#04x} on a sink without SCDC");
        }
    }

    /// The transcript's records, oldest first; each is copied out under the lock, so the
    /// bench can be used while iterating.
    pub fn transcript(&self) -> impl Iterator<Item = Record> + '_ {
        let mut index = 0;
        core::iter::from_fn(move || {
            let record = self.locked(|state, _| state.transcript.get(index));
            index += 1;
            record
        })
    }

    /// How many records the transcript holds, up to `LOG`.
    pub fn transcript_len(&self) -> usize {
        self.locked(|state, _| state.transcript.len())
    }

    /// How many records the transcript did not keep, past `LOG`: the newest by default, the
    /// oldest with [`BenchBuilder::keep_newest_records`].
    pub fn transcript_overflow(&self) -> usize {
        self.locked(|state, _| state.transcript.overflow())
    }

    /// Empties the transcript, and its overflow count.
    pub fn clear_transcript(&self) {
        self.locked(|state, _| state.transcript.clear());
    }
}

impl<const LOG: usize, const FAULTS: usize> fmt::Debug for Bench<'_, LOG, FAULTS> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.locked(|state, now| {
            f.debug_struct("Bench")
                .field("now", &now)
                .field("config", &state.config)
                .field("edid_len", &state.eddc.edid_len())
                .field("displayid_len", &state.eddc.displayid_len())
                .field("targets", &state.targets.len())
                .field("scdc", &state.scdc.is_some())
                .field("line", &state.line)
                .field("hpd", &state.hpd)
                .field("plugged", &state.plugged)
                .field("attached", &state.attached)
                .field("operations", &state.spent)
                .field("faults", &state.faults.len())
                .field("phy", &state.phy)
                .field("budget", &self.budget)
                .field("transcript_len", &state.transcript.len())
                .field("transcript_overflow", &state.transcript.overflow())
                .finish_non_exhaustive()
        })
    }
}

/// What the handles reach the bench through, whatever its capacities.
pub(crate) trait Core {
    /// Runs `f` as one step no other handle's call interleaves with: a bus call, or a delay
    /// and the event after it, takes the lock several times, and its records, the fault
    /// counts and the clock must follow the calls' order. A critical section, which nests.
    fn serialized(&self, f: &mut dyn FnMut());
    fn capabilities(&self) -> DdcCapabilities;
    fn timing(&self) -> BusTiming;
    fn clock(&self) -> &SimClock;
    /// Counts one operation against the budget; panics past it.
    fn spend(&self);
    /// Counts the operation `record` describes for the scheduled faults; the one that
    /// strikes, if any.
    fn strike(&self, record: &BusRecord) -> Option<Strike>;
    /// Records `record` failed with `kind` by `strike`, before the sink saw it.
    fn struck_before(&self, record: BusRecord, strike: Strike, kind: DdcErrorKind);
    /// Records `record` at the bench's time, routes `ops` as one transaction and completes
    /// the record with the result, which `fault` replaces when it fails the call; returns
    /// the result and how far the transaction got.
    fn transaction(
        &self,
        record: BusRecord,
        ops: &mut [Operation<'_>],
        fault: Option<Strike>,
    ) -> (Result<(), DdcErrorKind>, Reach);
    /// Records a call that reached no target.
    fn record(&self, record: BusRecord);
    /// Runs the scheduled actions due at this moment of a bus operation: `before` it, or
    /// after it.
    fn run_actions(&self, before: bool);
    /// A delay on the bench's clock ended: a bench event.
    fn after_delay(&self);
    /// The SCDC register `reg`; `None` for a sink without SCDC.
    fn scdc(&self, reg: u8) -> Option<u8>;
    /// HPD, and whether the sink is plugged.
    fn hpd(&self) -> (bool, bool);
    /// What the simulated PHY can do.
    fn phy_capabilities(&self) -> TmdsPhyCapabilities;
    /// A call of the simulated PHY: the result, and whether it took effect before a fault
    /// failed it.
    fn phy_call(&self, call: PhyCall) -> (Result<(), PhyErrorKind>, bool);
    /// A call a coupled PHY made: recorded with its result, the line set when the PHY's
    /// output changed, and a bench event.
    fn coupled_call(&self, call: PhyCall, result: Result<(), PhyErrorKind>, line: Option<Line>);
}

impl<const LOG: usize, const FAULTS: usize> Core for Bench<'_, LOG, FAULTS> {
    fn capabilities(&self) -> DdcCapabilities {
        self.capabilities
    }

    fn serialized(&self, f: &mut dyn FnMut()) {
        critical_section::with(|_| f());
    }

    fn timing(&self) -> BusTiming {
        self.timing
    }

    fn clock(&self) -> &SimClock {
        &self.clock
    }

    fn spend(&self) {
        let over = self.locked(|state, _| {
            state.spent = state.spent.saturating_add(1);
            self.budget.filter(|&budget| state.spent > budget)
        });
        if let Some(budget) = over {
            panic!(
                "the bench's operation budget of {budget} operations is spent: the code under \
                 test may be looping (BenchBuilder::budget sets the budget)"
            );
        }
    }

    fn strike(&self, record: &BusRecord) -> Option<Strike> {
        self.locked(|state, at| state.strike(at, record))
    }

    fn struck_before(&self, record: BusRecord, strike: Strike, kind: DdcErrorKind) {
        self.locked(|state, at| state.struck_before(at, record, strike, kind));
    }

    fn transaction(
        &self,
        record: BusRecord,
        ops: &mut [Operation<'_>],
        fault: Option<Strike>,
    ) -> (Result<(), DdcErrorKind>, Reach) {
        let per_byte = self.timing.per_byte;
        self.locked(|state, at| state.transaction(at, per_byte, record, ops, fault))
    }

    fn record(&self, record: BusRecord) {
        self.locked(|state, at| state.record(at, Event::Bus(record)));
    }

    fn run_actions(&self, before: bool) {
        let moment = if before {
            Moment::Before
        } else {
            Moment::After
        };
        self.locked(|state, at| state.run_actions(at, moment));
    }

    fn after_delay(&self) {
        self.locked(|state, at| state.update(at));
    }

    fn scdc(&self, reg: u8) -> Option<u8> {
        self.locked(|state, _| state.scdc.as_ref().map(|scdc| scdc.get(reg)))
    }

    fn hpd(&self) -> (bool, bool) {
        self.locked(|state, _| (state.hpd, state.plugged))
    }

    fn phy_capabilities(&self) -> TmdsPhyCapabilities {
        self.locked(|state, _| state.phy.capabilities())
    }

    fn phy_call(&self, call: PhyCall) -> (Result<(), PhyErrorKind>, bool) {
        self.locked(|state, _| state.phy_call(call, &self.clock))
    }

    fn coupled_call(&self, call: PhyCall, result: Result<(), PhyErrorKind>, line: Option<Line>) {
        self.locked(|state, at| {
            state.faults.phy_actions(&call);
            state.record(at, Event::Phy { call, result });
            if let Some(line) = line {
                if line != state.line {
                    state.line = line;
                    state.record(at, Event::Line(line));
                }
            }
            state.update(at);
            state.run_actions(at, Moment::After);
        });
    }
}

/// Builds a [`Bench`]: [`Bench::builder`], its setters, then
/// [`build`](Self::build).
#[must_use]
pub struct BenchBuilder<'a, const LOG: usize = 256, const FAULTS: usize = 16> {
    edid: &'a [u8],
    displayid: Option<&'a [u8]>,
    config: SinkConfig,
    capabilities: DdcCapabilities,
    timing: BusTiming,
    clock: SimClock,
    budget: Option<u64>,
    targets: Targets<'a>,
    scdc_hook: Option<Hook<'a>>,
    phy_capabilities: TmdsPhyCapabilities,
    phy_settings: PhySettings,
    keep_newest: bool,
}

/// The test's SCDC hook, as given to the bench.
type Hook<'a> = &'a mut (dyn ScdcHook + Send);

impl<'a, const LOG: usize, const FAULTS: usize> BenchBuilder<'a, LOG, FAULTS> {
    /// The sink's settings.
    pub fn sink(self, config: SinkConfig) -> Self {
        Self { config, ..self }
    }

    /// The sink's DisplayID store, at 0x52: a whole number of 128-byte blocks, up to 256.
    /// Without one, or with an empty one, nothing answers 0x52.
    pub fn displayid(self, store: &'a [u8]) -> Self {
        Self {
            displayid: Some(store),
            ..self
        }
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

    /// The operation budget: how many operations the handles may make (bus calls, refused
    /// ones included, PHY calls, clock reads and delays) before the bench panics; by default
    /// [`DEFAULT_BUDGET`], `None` for none.
    pub fn budget(self, budget: Option<u64>) -> Self {
        Self { budget, ..self }
    }

    /// What the simulated PHY can do; by default [`DEFAULT_PHY_CAPABILITIES`].
    pub fn phy_capabilities(self, phy_capabilities: TmdsPhyCapabilities) -> Self {
        Self {
            phy_capabilities,
            ..self
        }
    }

    /// How the simulated PHY behaves where hdmi-hal's contract allows several behaviours;
    /// by default [`PhySettings::DEFAULT`].
    pub fn phy(self, phy_settings: PhySettings) -> Self {
        Self {
            phy_settings,
            ..self
        }
    }

    /// The test's own targets, routed to before the built-in ones, in this order.
    pub fn targets(self, targets: &'a mut [&'a mut (dyn Target + Send)]) -> Self {
        Self { targets, ..self }
    }

    /// A transcript that keeps its newest `LOG` records once full, dropping the oldest, for a
    /// session-length test that asks about what came late. By default it keeps the oldest,
    /// which a test of one procedure asks about. Either way
    /// [`Bench::transcript_overflow`] counts what it did not keep.
    pub fn keep_newest_records(self) -> Self {
        Self {
            keep_newest: true,
            ..self
        }
    }

    /// The test's quirk on top of the built-in SCDC sink: it decides each byte the source
    /// writes and what each read returns. Kept across a [`Bench::plug`].
    pub fn scdc_hook(self, hook: &'a mut (dyn ScdcHook + Send)) -> Self {
        Self {
            scdc_hook: Some(hook),
            ..self
        }
    }

    /// The builder for a bench with a transcript of `L` records and a fault schedule of `F`.
    pub fn capacities<const L: usize, const F: usize>(self) -> BenchBuilder<'a, L, F> {
        BenchBuilder {
            edid: self.edid,
            displayid: self.displayid,
            config: self.config,
            capabilities: self.capabilities,
            timing: self.timing,
            clock: self.clock,
            budget: self.budget,
            targets: self.targets,
            scdc_hook: self.scdc_hook,
            phy_capabilities: self.phy_capabilities,
            phy_settings: self.phy_settings,
            keep_newest: self.keep_newest,
        }
    }

    /// The bench, or why the stores cannot be its sink's. The sink is plugged in with HPD
    /// high, the adapter attached, and nothing on the line.
    pub fn build(self) -> Result<Bench<'a, LOG, FAULTS>, BenchError> {
        check_stores(self.edid, self.displayid)?;
        Ok(Bench {
            clock: self.clock,
            capabilities: self.capabilities,
            timing: self.timing,
            budget: self.budget,
            state: Mutex::new(RefCell::new(State {
                config: self.config,
                eddc: Eddc::new(&self.config, self.edid, self.displayid),
                scdc: Scdc::new(&self.config),
                targets: self.targets,
                scdc_hook: self.scdc_hook,
                line: Line::Off,
                errors: ErrorRates::NONE,
                lanes: LinkLanes::NONE,
                hpd: true,
                plugged: true,
                attached: true,
                spent: 0,
                transcript: if self.keep_newest {
                    Transcript::new().keeping_newest()
                } else {
                    Transcript::new()
                },
                faults: Schedule::new(),
                armed: None,
                phy: PhyState::new(self.phy_settings, self.phy_capabilities),
                phy_line: Line::Off,
            })),
        })
    }
}

impl<const LOG: usize, const FAULTS: usize> fmt::Debug for BenchBuilder<'_, LOG, FAULTS> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BenchBuilder")
            .field("edid_len", &self.edid.len())
            .field("displayid_len", &self.displayid.map(<[u8]>::len))
            .field("config", &self.config)
            .field("capabilities", &self.capabilities)
            .field("timing", &self.timing)
            .field("clock", &self.clock)
            .field("budget", &self.budget)
            .field("targets", &self.targets.len())
            .field("scdc_hook", &self.scdc_hook.is_some())
            .field("phy_capabilities", &self.phy_capabilities)
            .field("phy_settings", &self.phy_settings)
            .field("keep_newest", &self.keep_newest)
            .finish()
    }
}

/// Why a bench cannot be built, or a sink plugged in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BenchError {
    /// The EDID store is not a whole number of 128-byte blocks, or is longer than 256
    /// blocks; the length given, in bytes.
    EdidLength(usize),
    /// The DisplayID store is not a whole number of 128-byte blocks, or is longer than 256
    /// blocks; the length given, in bytes.
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
             at most 256 blocks"
        )
    }
}

impl core::error::Error for BenchError {}

/// The bench's clock as a `Clock` and a `Delay`, a `Copy` handle borrowed from the bench
/// ([`Bench::time`]): one value passed as both.
///
/// Each read and each delay counts against the bench's operation budget; a delay is a bench
/// event, after which every target sees the new time.
#[derive(Clone, Copy)]
pub struct Time<'b> {
    core: &'b (dyn Core + Sync + 'b),
}

impl Clock for Time<'_> {
    fn now(&self) -> Timestamp {
        self.core.spend();
        self.core.clock().now()
    }
}

impl Delay for Time<'_> {
    fn delay(&mut self, duration: Duration) {
        self.core.spend();
        let core = self.core;
        core.serialized(&mut || {
            let mut clock = core.clock();
            Delay::delay(&mut clock, duration);
            core.after_delay();
        });
    }
}

impl fmt::Debug for Time<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Time")
            .field("clock", self.core.clock())
            .finish()
    }
}

/// The sink as the test sees it, a `Copy` handle borrowed from the bench ([`Bench::sink`]):
/// its registers and HPD, read without a bus operation, so reading them changes nothing and
/// takes no time.
#[derive(Clone, Copy)]
pub struct Sink<'b> {
    core: &'b (dyn Core + Sync + 'b),
}

impl Sink<'_> {
    /// Whether HPD is high.
    pub fn hpd(&self) -> bool {
        self.core.hpd().0
    }

    /// Whether the sink is plugged in.
    pub fn is_plugged(&self) -> bool {
        self.core.hpd().1
    }

    /// The SCDC register `reg`; `None` for a sink without SCDC.
    pub fn scdc(&self, reg: u8) -> Option<u8> {
        self.core.scdc(reg)
    }

    /// `TMDS_Config` (0x20): bit 0 `Scrambling_Enable`, bit 1 `TMDS_Bit_Clock_Ratio`.
    pub fn tmds_config(&self) -> Option<u8> {
        self.scdc(crate::scdc::TMDS_CONFIG)
    }

    /// `Scrambling_Status` (0x21): bit 0, scrambled TMDS detected.
    pub fn scrambling_status(&self) -> Option<u8> {
        self.scdc(crate::scdc::SCRAMBLING_STATUS)
    }

    /// `Sink_Version` (0x01).
    pub fn sink_version(&self) -> Option<u8> {
        self.scdc(crate::scdc::SINK_VERSION)
    }

    /// `Source_Version` (0x02), as the source last wrote it.
    pub fn source_version(&self) -> Option<u8> {
        self.scdc(crate::scdc::SOURCE_VERSION)
    }

    /// `Update_0` (0x10): the update flags.
    pub fn update_0(&self) -> Option<u8> {
        self.scdc(crate::scdc::UPDATE_0)
    }

    /// `Status_Flags_0` (0x40): bit 0 `Clock_Detected`, bits 1–4 lanes 0–3 locked, bit 6
    /// `FLT_ready`.
    pub fn status_flags_0(&self) -> Option<u8> {
        self.scdc(crate::scdc::STATUS_FLAGS_0)
    }

    /// `Config_0` (0x30): bit 0 `RR_Enable`, bit 1 `FLT_no_retrain`.
    pub fn config_0(&self) -> Option<u8> {
        self.scdc(crate::scdc::CONFIG_0)
    }

    /// `Config_1` (0x31): `FRL_Rate` in bits 3:0, `FFE_Levels` in 7:4.
    pub fn config_1(&self) -> Option<u8> {
        self.scdc(crate::scdc::CONFIG_1)
    }
}

impl fmt::Debug for Sink<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sink")
            .field("tmds_config", &self.tmds_config())
            .field("scrambling_status", &self.scrambling_status())
            .finish_non_exhaustive()
    }
}
