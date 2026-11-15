//! The `DdcBus` check: hdmi-hal's `DdcBus`, at the trait level.

use core::fmt;

use hdmi_hal::ddc::{
    DdcBus, DdcCapabilities, DdcError, DdcErrorKind, EDID, EddcTarget, MAX_SEGMENT,
    is_usable_address,
};

use super::Report;
use crate::fault::OpKind;

/// An address in I²C's reserved range: refused before bus activity.
const UNUSABLE: u8 = 0x7F;
/// The longest transfer the check drives; a limit above it is not checked.
const MAX_LEN: usize = 512;
/// What the check writes: zeros, to the silent address only, or an offset.
static ZEROS: [u8; MAX_LEN] = [0; MAX_LEN];

/// What is behind a DDC bus, as far as the [`DdcBusCheck`] needs to know: the caller's word,
/// not discovered. A point whose description is missing is skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct DdcDevice<'d> {
    /// The EDID the device serves, if the check should read and compare it.
    pub edid: Option<&'d [u8]>,
    /// Whether the device has an E-DDC segment pointer; without one, segment 0 is read without
    /// it (VESA E-DDC 1.3, 8.1.2).
    pub segment_pointer: bool,
    /// An address the capabilities reach where nothing answers.
    pub silent: Option<u8>,
}

impl<'d> DdcDevice<'d> {
    /// A device with a segment pointer, no EDID to compare and no silent address given.
    pub const fn new() -> Self {
        Self {
            edid: None,
            segment_pointer: true,
            silent: None,
        }
    }

    /// The device serving `edid`.
    #[must_use]
    pub const fn with_edid(self, edid: &'d [u8]) -> Self {
        Self {
            edid: Some(edid),
            ..self
        }
    }

    /// A device without a segment pointer.
    #[must_use]
    pub const fn without_segment_pointer(self) -> Self {
        Self {
            segment_pointer: false,
            ..self
        }
    }

    /// `addr`, reachable, answers nothing.
    #[must_use]
    pub const fn with_silent(self, addr: u8) -> Self {
        Self {
            silent: Some(addr),
            ..self
        }
    }
}

impl Default for DdcDevice<'_> {
    fn default() -> Self {
        Self::new()
    }
}

/// A failed point of the [`DdcBusCheck`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DdcBusFailure {
    /// A call the arguments or the capabilities rule out was not refused with `Unsupported`.
    NotRefused {
        /// The operation.
        op: OpKind,
        /// Its address.
        addr: u8,
        /// How it ended.
        found: Result<(), DdcErrorKind>,
    },
    /// A call that should succeed failed.
    Failed {
        /// The operation.
        op: OpKind,
        /// Its address.
        addr: u8,
        /// The error's kind.
        kind: DdcErrorKind,
    },
    /// A call to the address where nothing answers did not fail `NoAcknowledge`.
    Answered {
        /// The operation.
        op: OpKind,
        /// The silent address.
        addr: u8,
        /// How it ended.
        found: Result<(), DdcErrorKind>,
    },
    /// A byte of the EDID read differs from the device's: the first that does.
    Data {
        /// The segment read.
        segment: u8,
        /// The byte's offset in the segment.
        offset: u8,
        /// The device's byte.
        expected: u8,
        /// The byte read.
        found: u8,
    },
    /// A thread of [`DdcBusCheck::run_threads`] panicked.
    Panicked,
}

impl fmt::Display for DdcBusFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRefused { op, addr, found } => {
                write!(f, "{op:?} at {addr:#04x} was not refused: {found:?}")
            }
            Self::Failed { op, addr, kind } => write!(f, "{op:?} at {addr:#04x} failed: {kind:?}"),
            Self::Answered { op, addr, found } => write!(
                f,
                "{op:?} at {addr:#04x}, where nothing answers, ended {found:?}"
            ),
            Self::Data {
                segment,
                offset,
                expected,
                found,
            } => write!(
                f,
                "EDID segment {segment} byte {offset:#04x} read {found:#04x}, not {expected:#04x}"
            ),
            Self::Panicked => f.write_str("a thread panicked"),
        }
    }
}

/// One call the check makes.
#[derive(Debug, Clone, Copy)]
enum Call {
    Write {
        addr: u8,
        len: usize,
    },
    WriteRead {
        addr: u8,
        write: usize,
        read: usize,
    },
    Read {
        addr: u8,
        len: usize,
    },
    Segmented {
        target: EddcTarget,
        segment: u8,
        offset: u8,
        len: usize,
    },
    /// `write(addr, [offset])`: sets a word offset.
    SetOffset {
        addr: u8,
        offset: u8,
    },
    /// `write_read(addr, [offset], len)`: a read at a word offset.
    ReadAt {
        addr: u8,
        offset: u8,
        len: usize,
    },
}

impl Call {
    fn op(self) -> OpKind {
        match self {
            Self::Write { .. } | Self::SetOffset { .. } => OpKind::Write,
            Self::WriteRead { .. } | Self::ReadAt { .. } => OpKind::WriteRead,
            Self::Read { .. } => OpKind::Read,
            Self::Segmented { .. } => OpKind::ReadSegmented,
        }
    }

    fn addr(self) -> u8 {
        match self {
            Self::Write { addr, .. }
            | Self::WriteRead { addr, .. }
            | Self::Read { addr, .. }
            | Self::SetOffset { addr, .. }
            | Self::ReadAt { addr, .. } => addr,
            Self::Segmented { target, .. } => target.address(),
        }
    }
}

/// What a call should do.
#[derive(Debug, Clone, Copy)]
enum Expect<'d> {
    Ok,
    Refused,
    Silent,
    /// The device's `bytes` from `offset` of `segment`.
    Data {
        segment: u8,
        offset: u8,
        bytes: &'d [u8],
    },
}

/// The calls the check makes for a device and the bus's capabilities: at most `PLAN` points.
const PLAN: usize = 32;

/// A point: its call, what it should do, and whether it depends on the call before it, so
/// it runs only with no other thread on the bus.
type Point<'d> = (Call, Expect<'d>, bool);

struct Plan<'d> {
    points: [Option<Point<'d>>; PLAN],
    len: usize,
}

impl<'d> Plan<'d> {
    fn push(&mut self, call: Call, expect: Expect<'d>) {
        self.push_point((call, expect, false));
    }

    /// A point that depends on the one pushed before it.
    fn push_sequenced(&mut self, call: Call, expect: Expect<'d>) {
        self.push_point((call, expect, true));
    }

    fn push_point(&mut self, point: Point<'d>) {
        if let Some(slot) = self.points.get_mut(self.len) {
            *slot = Some(point);
            self.len += 1;
        }
    }

    /// The points; with `concurrent`, those that do not depend on the call before them.
    fn iter(&self, concurrent: bool) -> impl Iterator<Item = (Call, Expect<'d>)> + '_ {
        self.points[..self.len]
            .iter()
            .flatten()
            .filter(move |&&(_, _, sequenced)| !(concurrent && sequenced))
            .map(|&(call, expect, _)| (call, expect))
    }

    fn new(device: &DdcDevice<'d>, caps: DdcCapabilities) -> Self {
        let mut plan = Self {
            points: [None; PLAN],
            len: 0,
        };
        let some = |len: u16| usize::from(len) < MAX_LEN;
        // The arguments.
        if !is_usable_address(UNUSABLE) {
            plan.push(
                Call::Read {
                    addr: UNUSABLE,
                    len: 1,
                },
                Expect::Refused,
            );
        }
        let quiet = device.silent.unwrap_or(EDID);
        plan.push(
            Call::Write {
                addr: quiet,
                len: 0,
            },
            Expect::Refused,
        );
        plan.push(Call::Read { addr: EDID, len: 0 }, Expect::Ok);
        plan.push(
            Call::WriteRead {
                addr: EDID,
                write: 1,
                read: 0,
            },
            Expect::Ok,
        );
        let segmented = |segment, offset, len| Call::Segmented {
            target: EddcTarget::Edid,
            segment,
            offset,
            len,
        };
        plan.push(segmented(0, 0, 0), Expect::Ok);
        plan.push(segmented(MAX_SEGMENT + 1, 0, 1), Expect::Refused);
        plan.push(segmented(0, 0xFF, 2), Expect::Refused);
        // The capabilities.
        if let Some(addr) = (0x08..=0x77).find(|&addr| !caps.reaches(addr)) {
            plan.push(Call::Read { addr, len: 1 }, Expect::Refused);
        }
        if caps.reaches(EDID) && some(caps.read_max()) {
            let len = usize::from(caps.read_max()) + 1;
            plan.push(Call::Read { addr: EDID, len }, Expect::Refused);
        }
        let (write_part, read_part) = caps.write_read_max();
        if caps.reaches(EDID) && some(read_part) {
            let read = usize::from(read_part) + 1;
            plan.push(
                Call::WriteRead {
                    addr: EDID,
                    write: 1,
                    read,
                },
                Expect::Refused,
            );
        }
        if let Some(silent) = device.silent.filter(|&addr| caps.reaches(addr)) {
            if some(write_part) {
                let write = usize::from(write_part) + 1;
                plan.push(
                    Call::WriteRead {
                        addr: silent,
                        write,
                        read: 1,
                    },
                    Expect::Refused,
                );
            }
            if some(caps.write_max()) {
                let len = usize::from(caps.write_max()) + 1;
                plan.push(Call::Write { addr: silent, len }, Expect::Refused);
            }
        }
        for target in [EddcTarget::Edid, EddcTarget::DisplayId] {
            if !caps.reads_target(target) {
                plan.push(
                    Call::Segmented {
                        target,
                        segment: 0,
                        offset: 0,
                        len: 1,
                    },
                    Expect::Refused,
                );
            }
        }
        match caps.max_segment() {
            None => plan.push(segmented(0, 0, 1), Expect::Refused),
            Some(max) if max < MAX_SEGMENT => {
                plan.push(segmented(max + 1, 0, 1), Expect::Refused);
            }
            Some(_) => {}
        }
        // The device.
        if let Some(edid) = device.edid {
            let data = |segment, offset: u8, len: usize| {
                let start = usize::from(segment) * 256 + usize::from(offset);
                let bytes = edid.get(start..start + len)?;
                Some(Expect::Data {
                    segment,
                    offset,
                    bytes,
                })
            };
            if caps.reads_target(EddcTarget::Edid) && caps.max_segment().is_some() {
                let first = &edid[..edid.len().min(128)];
                if !first.is_empty() {
                    plan.push(
                        segmented(0, 0, first.len()),
                        Expect::Data {
                            segment: 0,
                            offset: 0,
                            bytes: first,
                        },
                    );
                }
                // The second block, and an odd offset: the offset reaches the device.
                for (offset, len) in [(128, 128), (0x11, 16)] {
                    if let Some(expect) = data(0, offset, len) {
                        plan.push(segmented(0, offset, len), expect);
                    }
                }
                let reaches_1 = caps.max_segment().is_some_and(|max| max >= 1);
                if device.segment_pointer && reaches_1 && edid.len() > 256 {
                    let block = &edid[256..edid.len().min(384)];
                    plan.push(
                        segmented(1, 0, block.len()),
                        Expect::Data {
                            segment: 1,
                            offset: 0,
                            bytes: block,
                        },
                    );
                }
            }
            // A plain write_read at 0x50 reads segment 0 from the offset it writes.
            let (write_part, read_part) = caps.write_read_max();
            if caps.reaches(EDID) && write_part >= 1 && read_part >= 16 {
                if let Some(expect) = data(0, 0x20, 16) {
                    plan.push(
                        Call::ReadAt {
                            addr: EDID,
                            offset: 0x20,
                            len: 16,
                        },
                        expect,
                    );
                }
            }
            // A write of the offset, then a read at the current address (E-DDC 1.3, 6.1):
            // the write reaches the device. Not with other threads on the bus.
            if caps.reaches(EDID) && caps.write_max() >= 1 && caps.read_max() >= 16 {
                if let Some(expect) = data(0, 0x40, 16) {
                    plan.push(
                        Call::SetOffset {
                            addr: EDID,
                            offset: 0x40,
                        },
                        Expect::Ok,
                    );
                    plan.push_sequenced(
                        Call::Read {
                            addr: EDID,
                            len: 16,
                        },
                        expect,
                    );
                }
            }
        }
        if let Some(silent) = device.silent.filter(|&addr| caps.reaches(addr)) {
            if caps.read_max() >= 1 {
                plan.push(
                    Call::Read {
                        addr: silent,
                        len: 1,
                    },
                    Expect::Silent,
                );
            }
            if caps.write_max() >= 1 {
                plan.push(
                    Call::Write {
                        addr: silent,
                        len: 1,
                    },
                    Expect::Silent,
                );
            }
        }
        plan
    }
}

/// Whether a call ended as expected.
fn judge(
    call: Call,
    expect: Expect<'_>,
    result: Result<(), DdcErrorKind>,
    read: &[u8],
) -> Option<DdcBusFailure> {
    let (op, addr) = (call.op(), call.addr());
    match expect {
        Expect::Ok => result
            .err()
            .map(|kind| DdcBusFailure::Failed { op, addr, kind }),
        Expect::Refused => {
            (result != Err(DdcErrorKind::Unsupported)).then_some(DdcBusFailure::NotRefused {
                op,
                addr,
                found: result,
            })
        }
        Expect::Silent => (!matches!(result, Err(DdcErrorKind::NoAcknowledge(_)))).then_some(
            DdcBusFailure::Answered {
                op,
                addr,
                found: result,
            },
        ),
        Expect::Data {
            segment,
            offset,
            bytes,
        } => match result {
            Err(kind) => Some(DdcBusFailure::Failed { op, addr, kind }),
            Ok(()) => bytes
                .iter()
                .zip(read)
                .position(|(expected, found)| expected != found)
                .map(|i| DdcBusFailure::Data {
                    segment,
                    offset: offset.wrapping_add(i as u8),
                    expected: bytes[i],
                    found: read[i],
                }),
        },
    }
}

/// Makes one call over a sync bus.
fn call<B: DdcBus + ?Sized>(
    bus: &B,
    call: Call,
    buf: &mut [u8; MAX_LEN],
) -> (Result<(), DdcErrorKind>, usize) {
    let kind = |result: Result<(), B::Error>| result.map_err(|error| error.kind());
    match call {
        Call::Write { addr, len } => (kind(bus.write(addr, &ZEROS[..len])), 0),
        Call::WriteRead { addr, write, read } => (
            kind(bus.write_read(addr, &ZEROS[..write], &mut buf[..read])),
            read,
        ),
        Call::Read { addr, len } => (kind(bus.read(addr, &mut buf[..len])), len),
        Call::Segmented {
            target,
            segment,
            offset,
            len,
        } => (
            kind(bus.read_segmented(target, segment, offset, &mut buf[..len])),
            len,
        ),
        Call::SetOffset { addr, offset } => (kind(bus.write(addr, &[offset])), 0),
        Call::ReadAt { addr, offset, len } => {
            (kind(bus.write_read(addr, &[offset], &mut buf[..len])), len)
        }
    }
}

/// The `DdcBus` check: the points of hdmi-hal's `DdcBus` contract a caller can see from
/// outside, against a device it describes.
///
/// - **Arguments:** an unusable address, an empty write, a segment above `MAX_SEGMENT` and a
///   read past 256 bytes of a segment are refused with `Unsupported`; empty buffers are `Ok`.
/// - **Capabilities:** an address they do not reach, a read past `read_max` or past
///   `write_read`'s read part, a write past `write_max` or past its write part (to the
///   silent address only), a target they do not read, a segment past `max_segment`: each
///   refused with `Unsupported`.
/// - **The device:** the EDID's first block read from segment 0, without a segment pointer
///   too (E-DDC 1.3, 8.1.2), its second block and 16 bytes at an odd offset, and its third
///   block from segment 1 where the device has a pointer; 16 bytes by `write_read` at the
///   offset it writes; a `write` of the offset and a plain `read` at the current address
///   (6.1); the silent address failing `NoAcknowledge`.
///
/// Every call is safe on a real device: the refusals are reads, or writes to the silent
/// address. Points whose description is missing are skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DdcBusCheck<'d> {
    device: DdcDevice<'d>,
}

impl<'d> DdcBusCheck<'d> {
    /// The check for a bus in front of `device`.
    pub const fn new(device: DdcDevice<'d>) -> Self {
        Self { device }
    }

    /// Runs the check against `bus`.
    pub fn run<B: DdcBus + ?Sized>(&self, bus: &B) -> Report<DdcBusFailure> {
        self.run_points(bus, false)
    }

    /// Runs the points against `bus`; with `concurrent`, those that do not depend on the
    /// call before them.
    fn run_points<B: DdcBus + ?Sized>(&self, bus: &B, concurrent: bool) -> Report<DdcBusFailure> {
        let mut report = Report::new();
        let plan = Plan::new(&self.device, bus.capabilities());
        let mut buf = [0u8; MAX_LEN];
        for (point, expect) in plan.iter(concurrent) {
            let (result, read) = call(bus, point, &mut buf);
            report.record(judge(point, expect, result, &buf[..read]));
        }
        report
    }

    /// Runs the check from `threads` threads at once against `bus`, which must take
    /// concurrent calls without a panic: every thread's points, and a panicked thread as a
    /// failure. The read at the current address is left out: another thread's call may move
    /// the offset between the write and the read.
    ///
    /// # Panics
    ///
    /// If `threads` is 0: a run with no thread checks nothing.
    #[cfg(feature = "std")]
    pub fn run_threads<B: DdcBus + Sync + ?Sized>(
        &self,
        bus: &B,
        threads: usize,
    ) -> Report<DdcBusFailure> {
        assert!(threads > 0, "run_threads needs at least one thread");
        let mut report = Report::new();
        std::thread::scope(|scope| {
            let handles: std::vec::Vec<_> = (0..threads)
                .map(|_| scope.spawn(|| self.run_points(bus, true)))
                .collect();
            for handle in handles {
                match handle.join() {
                    Ok(thread) => report.absorb(thread),
                    Err(_) => report.record(Some(DdcBusFailure::Panicked)),
                }
            }
        });
        report
    }

    /// Runs the check against an async `bus`: the same points as [`run`](Self::run).
    #[cfg(feature = "async")]
    pub async fn run_async<B>(&self, bus: &B) -> Report<DdcBusFailure>
    where
        B: hdmi_hal_async::ddc::DdcBus + ?Sized,
    {
        let mut report = Report::new();
        let plan = Plan::new(&self.device, bus.capabilities());
        let mut buf = [0u8; MAX_LEN];
        for (point, expect) in plan.iter(false) {
            let kind = |result: Result<(), B::Error>| result.map_err(|error| error.kind());
            let (result, read) = match point {
                Call::Write { addr, len } => (kind(bus.write(addr, &ZEROS[..len]).await), 0),
                Call::WriteRead { addr, write, read } => (
                    kind(
                        bus.write_read(addr, &ZEROS[..write], &mut buf[..read])
                            .await,
                    ),
                    read,
                ),
                Call::Read { addr, len } => (kind(bus.read(addr, &mut buf[..len]).await), len),
                Call::Segmented {
                    target,
                    segment,
                    offset,
                    len,
                } => (
                    kind(
                        bus.read_segmented(target, segment, offset, &mut buf[..len])
                            .await,
                    ),
                    len,
                ),
                Call::SetOffset { addr, offset } => (kind(bus.write(addr, &[offset]).await), 0),
                Call::ReadAt { addr, offset, len } => (
                    kind(bus.write_read(addr, &[offset], &mut buf[..len]).await),
                    len,
                ),
            };
            report.record(judge(point, expect, result, &buf[..read]));
        }
        report
    }
}
