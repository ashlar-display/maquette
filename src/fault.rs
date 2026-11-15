//! Faults: scheduled failures of the bench's bus operations.

use core::fmt;
use core::time::Duration;

use hdmi_hal::ddc::DdcErrorKind;

use crate::phy::{PhyCall, PhyFault};
use crate::transcript::{BusOp, BusRecord};

/// A fault: it strikes bus operations that match its filter, at a point of the operation,
/// with an effect. Scheduled with [`Bench::schedule`](crate::Bench::schedule).
///
/// An operation is one transaction: a `DdcBus` call, or one piece of a split
/// `read_segmented`. Calls refused by the argument or capability checks, empty buffers and
/// calls while detached do not count; calls while unplugged do.
///
/// ```
/// use maquette::{
///     Bench, DdcBus, DdcErrorKind, Effect, Fault, Filter, NoAcknowledgeSource, Point,
///     SinkConfig,
/// };
///
/// let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
/// // The second write to TMDS_Config is not acknowledged at its value byte.
/// let nack = DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Data);
/// bench
///     .schedule(
///         Fault::new(Effect::Fail(nack))
///             .on(Filter::ANY.with_addr(0x54).with_first(0x20))
///             .nth(2)
///             .at(Point::Partway { byte: 1 }),
///     )
///     .unwrap();
/// let bus = bench.ddc();
/// bus.write(0x54, &[0x20, 0x01]).unwrap();
/// assert_eq!(bus.write(0x54, &[0x20, 0x03]).unwrap_err().kind, nack);
/// assert_eq!(bench.sink().tmds_config(), Some(0x01));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Fault {
    /// Which operations it counts.
    pub filter: Filter,
    /// The first matching operation it strikes, counted from 1 from when it is scheduled
    /// (0 is taken as 1).
    pub nth: u32,
    /// How many operations it strikes, from the `nth`.
    pub times: Times,
    /// What it does.
    pub effect: Effect,
    /// Where in the operation it strikes.
    pub point: Point,
}

impl Fault {
    /// A fault with `effect` that strikes the first operation, once, before the sink sees
    /// it.
    pub const fn new(effect: Effect) -> Self {
        Self {
            filter: Filter::ANY,
            nth: 1,
            times: Times::Count(1),
            effect,
            point: Point::Before,
        }
    }

    /// The fault counting only operations that match `filter`.
    #[must_use]
    pub const fn on(self, filter: Filter) -> Self {
        Self { filter, ..self }
    }

    /// The fault striking the `n`-th matching operation first.
    #[must_use]
    pub const fn nth(self, n: u32) -> Self {
        Self { nth: n, ..self }
    }

    /// The fault striking `times` operations from the `nth`.
    #[must_use]
    pub const fn times(self, times: Times) -> Self {
        Self { times, ..self }
    }

    /// The fault striking at `point`.
    #[must_use]
    pub const fn at(self, point: Point) -> Self {
        Self { point, ..self }
    }

    /// Whether this fault counts `record`'s operation.
    fn matches(&self, record: &BusRecord) -> bool {
        let filter = &self.filter;
        filter.addr.is_none_or(|addr| addr == record.addr)
            && filter.op.is_none_or(|op| op == OpKind::of(record.op))
            && filter.first.is_none_or(|first| Some(first) == record.first)
    }
}

/// Which operations a fault counts: every field that is set must match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Filter {
    /// The operation's address; for `read_segmented`, the target's.
    pub addr: Option<u8>,
    /// The kind of operation.
    pub op: Option<OpKind>,
    /// The first byte written: the register or offset; for `read_segmented`, the piece's
    /// offset.
    pub first: Option<u8>,
}

impl Filter {
    /// Every operation.
    pub const ANY: Self = Self {
        addr: None,
        op: None,
        first: None,
    };

    /// The filter matching only operations at `addr`.
    #[must_use]
    pub const fn with_addr(self, addr: u8) -> Self {
        Self {
            addr: Some(addr),
            ..self
        }
    }

    /// The filter matching only operations of kind `op`.
    #[must_use]
    pub const fn with_op(self, op: OpKind) -> Self {
        Self {
            op: Some(op),
            ..self
        }
    }

    /// The filter matching only operations whose first byte written is `first`.
    #[must_use]
    pub const fn with_first(self, first: u8) -> Self {
        Self {
            first: Some(first),
            ..self
        }
    }
}

/// The kind of a bus operation, as a [`Filter`] names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OpKind {
    /// `DdcBus::write`.
    Write,
    /// `DdcBus::write_read` with something to write.
    WriteRead,
    /// `DdcBus::read`, or `write_read` with nothing to write.
    Read,
    /// `DdcBus::read_segmented`, any piece of it.
    ReadSegmented,
}

impl OpKind {
    fn of(op: BusOp) -> Self {
        match op {
            BusOp::Write => Self::Write,
            BusOp::WriteRead => Self::WriteRead,
            BusOp::Read => Self::Read,
            BusOp::ReadSegmented { .. } => Self::ReadSegmented,
        }
    }
}

/// How many operations a fault strikes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Times {
    /// This many matching operations from the `nth`; once the last has passed, the fault
    /// leaves the schedule. 0 never strikes.
    Count(u32),
    /// Every matching operation from the `nth`, until the test clears it.
    UntilCleared,
}

/// What a fault does to the operation it strikes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Effect {
    /// The call fails with this kind. A NACK carries its source; a timeout also charges the
    /// bus's `stretch_timeout_us` where its capabilities give one; `Detached` fails this
    /// operation only. `Unsupported` breaks the `DdcBus` contract on purpose (it means
    /// refused before bus activity), to test code against a misbehaving backend.
    Fail(DdcErrorKind),
    /// The operation succeeds and takes this much longer.
    Slow(Duration),
}

/// Where in an operation a fault strikes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Point {
    /// Before the sink sees it: no target sees it, and only its address byte goes out.
    Before,
    /// After the sink applied it: the transaction runs as usual, and the fault's error is
    /// returned in place of its result.
    After,
    /// At data byte `byte` of the transaction, counted from 0 across its operations: the
    /// target sees the transaction cut there, then the fault's error is returned. A
    /// transaction of `byte` data bytes or fewer is struck after.
    Partway {
        /// The data byte that fails; the bytes before it go through.
        byte: u16,
    },
}

/// A scheduled fault, as [`Bench::schedule`](crate::Bench::schedule) names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FaultId(pub(crate) u32);

/// The fault schedule is full: the bench's `FAULTS` faults are scheduled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduleFull;

impl fmt::Display for ScheduleFull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the fault schedule is full; clear a fault or give the bench more")
    }
}

impl core::error::Error for ScheduleFull {}

/// A fault that strikes, as the bus applies it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Strike {
    pub(crate) id: FaultId,
    pub(crate) effect: Effect,
    pub(crate) point: Point,
}

/// A scheduled fault: the bus's or the PHY's.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Bus(Fault),
    Phy(PhyFault),
}

/// A scheduled fault and how many matching operations it has counted.
#[derive(Debug, Clone, Copy)]
struct Scheduled {
    id: FaultId,
    kind: Kind,
    seen: u32,
}

/// Counts one more matching operation; whether it is struck, and whether the fault's last
/// operation has now passed.
fn count(seen: &mut u32, nth: u32, times: Times) -> (bool, bool) {
    *seen = seen.saturating_add(1);
    let first = nth.max(1);
    match times {
        Times::Count(count) => {
            let end = first.saturating_add(count);
            (*seen >= first && *seen < end, seen.saturating_add(1) >= end)
        }
        Times::UntilCleared => (*seen >= first, false),
    }
}

/// The fault schedule: `FAULTS` slots, shared by the bus's faults and the PHY's.
#[derive(Debug)]
pub(crate) struct Schedule<const FAULTS: usize> {
    slots: [Option<Scheduled>; FAULTS],
    next_id: u32,
}

impl<const FAULTS: usize> Schedule<FAULTS> {
    pub(crate) const fn new() -> Self {
        Self {
            slots: [None; FAULTS],
            next_id: 0,
        }
    }

    fn insert(&mut self, kind: Kind) -> Result<FaultId, ScheduleFull> {
        let slot = self
            .slots
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(ScheduleFull)?;
        let id = FaultId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        *slot = Some(Scheduled { id, kind, seen: 0 });
        Ok(id)
    }

    pub(crate) fn add(&mut self, fault: Fault) -> Result<FaultId, ScheduleFull> {
        self.insert(Kind::Bus(fault))
    }

    pub(crate) fn add_phy(&mut self, fault: PhyFault) -> Result<FaultId, ScheduleFull> {
        self.insert(Kind::Phy(fault))
    }

    pub(crate) fn remove(&mut self, id: FaultId) -> bool {
        match self
            .slots
            .iter_mut()
            .find(|slot| matches!(slot, Some(s) if s.id == id))
        {
            Some(slot) => {
                *slot = None;
                true
            }
            None => false,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.slots = [None; FAULTS];
    }

    pub(crate) fn len(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    /// Counts `record`'s operation for every bus fault it matches, in the order scheduled,
    /// and returns the first that strikes; a fault whose last operation has passed leaves,
    /// struck or not (an earlier fault may have taken it).
    pub(crate) fn strike(&mut self, record: &BusRecord) -> Option<Strike> {
        let mut strike = None;
        for slot in self.slots.iter_mut() {
            let Some(scheduled) = slot else {
                continue;
            };
            let Kind::Bus(fault) = scheduled.kind else {
                continue;
            };
            if !fault.matches(record) {
                continue;
            }
            let (strikes, last) = count(&mut scheduled.seen, fault.nth, fault.times);
            if strikes && strike.is_none() {
                strike = Some(Strike {
                    id: scheduled.id,
                    effect: fault.effect,
                    point: fault.point,
                });
            }
            if last {
                *slot = None;
            }
        }
        strike
    }

    /// Counts `call` for every PHY fault it matches, as [`strike`](Self::strike) does for
    /// the bus; the fault that strikes, and whether the call takes effect first.
    pub(crate) fn strike_phy(&mut self, call: &PhyCall) -> Option<(FaultId, bool)> {
        let mut strike = None;
        for slot in self.slots.iter_mut() {
            let Some(scheduled) = slot else {
                continue;
            };
            let Kind::Phy(fault) = scheduled.kind else {
                continue;
            };
            if !fault.matches(call) {
                continue;
            }
            let (strikes, last) = count(&mut scheduled.seen, fault.nth, fault.times);
            if strikes && strike.is_none() {
                strike = Some((scheduled.id, fault.took_effect));
            }
            if last {
                *slot = None;
            }
        }
        strike
    }
}
