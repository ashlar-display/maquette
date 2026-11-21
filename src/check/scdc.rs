//! The `ScdcTransport` check: hdmi-hal's `ScdcTransport`, over a known register image.

use core::fmt;

use hdmi_hal::ddc::{DdcError, DdcErrorKind, NoAcknowledgeSource};

use super::Report;
use crate::bench::Bench;
use crate::fault::{Effect, Fault, Filter};
use crate::scdc::SCDC;

/// The SCDC registers whose values the check knows and that hold still while it runs: the
/// caller's word, not discovered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScdcImage {
    regs: [Option<u8>; 256],
}

impl ScdcImage {
    /// An image that knows no register.
    pub const fn new() -> Self {
        Self { regs: [None; 256] }
    }

    /// The image with `reg` known to hold `value`.
    #[must_use]
    pub const fn with(mut self, reg: u8, value: u8) -> Self {
        self.regs[reg as usize] = Some(value);
        self
    }

    /// The bench's SCDC registers, but the ones that change by themselves: `Update_0`,
    /// `Update_1`, `Scrambling_Status`, the status flags (0x40–0x42) and the error counters
    /// (0x50–0x5A). Empty for a sink without SCDC.
    pub fn of<const LOG: usize, const FAULTS: usize>(bench: &Bench<'_, LOG, FAULTS>) -> Self {
        let sink = bench.sink();
        let mut image = Self::new();
        for reg in 0..=255u8 {
            let moves = matches!(reg, 0x10 | 0x11 | 0x21 | 0x40..=0x42 | 0x50..=0x5A);
            if !moves {
                image.regs[usize::from(reg)] = sink.scdc(reg);
            }
        }
        image
    }

    /// The value `reg` is known to hold.
    pub fn get(&self, reg: u8) -> Option<u8> {
        self.regs[usize::from(reg)]
    }

    /// How many registers the image knows.
    pub fn known(&self) -> usize {
        self.regs.iter().flatten().count()
    }
}

impl Default for ScdcImage {
    fn default() -> Self {
        Self::new()
    }
}

/// Makes the next transfer fail, so a check can see what follows an error.
pub trait FailNext {
    /// Makes the next transfer fail with `kind`; whether it could. A check that cannot arm
    /// a failure records that, and blames the setup, not the implementation.
    fn fail_next(&self, kind: DdcErrorKind) -> bool;

    /// Withdraws a failure [`fail_next`](Self::fail_next) armed that no transfer has taken,
    /// so it does not strike a later one. By default nothing: a failure that cannot outlast
    /// its point.
    fn withdraw(&self) {}
}

/// The next SCDC operation on the bench's bus fails: a scheduled fault, before the sink sees
/// it. Needs room for one fault in the schedule: without it `fail_next` is `false`.
impl<const LOG: usize, const FAULTS: usize> FailNext for Bench<'_, LOG, FAULTS> {
    fn fail_next(&self, kind: DdcErrorKind) -> bool {
        self.arm(Fault::new(Effect::Fail(kind)).on(Filter::ANY.with_addr(SCDC)))
    }

    fn withdraw(&self) {
        self.withdraw_armed();
    }
}

/// An operation of `ScdcTransport`, as a failure names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScdcOp {
    /// `read`.
    Read,
    /// `read_block`.
    ReadBlock,
    /// `write`: for a value, the one read back after it.
    Write,
}

/// A failed point of the [`ScdcTransportCheck`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScdcTransportFailure {
    /// A register read differs from the image: for `read_block`, the first that does.
    Value {
        /// The operation.
        op: ScdcOp,
        /// The register.
        reg: u8,
        /// The image's value.
        expected: u8,
        /// The value read.
        found: u8,
    },
    /// An operation that should succeed failed.
    Failed {
        /// The operation.
        op: ScdcOp,
        /// Its register, the first for `read_block`.
        reg: u8,
        /// The error's kind.
        kind: DdcErrorKind,
    },
    /// A transfer failed by the hook came back `Ok`, or with another kind.
    Hidden {
        /// The kind the transfer failed with.
        expected: DdcErrorKind,
        /// What the caller got.
        found: Result<(), DdcErrorKind>,
    },
    /// The hook could not arm a failure with `kind`: the setup's failure, not the
    /// transport's (for the bench, its fault schedule is full).
    NotArmed {
        /// The kind.
        kind: DdcErrorKind,
    },
}

impl fmt::Display for ScdcTransportFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Value {
                op,
                reg,
                expected,
                found,
            } => write!(
                f,
                "{op:?} of register {reg:#04x} gave {found:#04x}, not {expected:#04x}"
            ),
            Self::Failed { op, reg, kind } => {
                write!(f, "{op:?} at register {reg:#04x} failed: {kind:?}")
            }
            Self::Hidden { expected, found } => {
                write!(f, "a transfer failed with {expected:?} came back {found:?}")
            }
            Self::NotArmed { kind } => write!(
                f,
                "could not make a transfer fail with {kind:?}: the setup, not the transport"
            ),
        }
    }
}

/// The kinds the check fails a transfer with, then checks the transport recovers from.
const RECOVERABLE: [DdcErrorKind; 3] = [
    DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Data),
    DdcErrorKind::Busy,
    DdcErrorKind::Timeout,
];

/// The `ScdcTransport` check: hdmi-hal's `ScdcTransport` contract, over a register image
/// the caller knows.
///
/// - **`read`** of every known register gives the image's value.
/// - **`read_block`** of 16 registers, at every start where they fit (0x00 to 0xF0), succeeds
///   and agrees with the image register by register; an empty buffer is `Ok`; a read past
///   0xFF returns, `Ok` or an error.
/// - **`write`,** with [`with_write`](Self::with_write): the value written reads back, and
///   the image's value written back after it reads back too.
/// - **After an error,** with [`with_faults`](Self::with_faults): a transfer failed with a
///   recoverable kind reaches the caller with that kind, and the next read succeeds. A
///   failure the hook cannot arm is [`ScdcTransportFailure::NotArmed`]; one the transport
///   never takes is withdrawn before the next point.
///
/// Whether a block read is one transaction on the wire cannot be seen from outside.
#[derive(Clone, Copy)]
pub struct ScdcTransportCheck<'i> {
    image: &'i ScdcImage,
    faults: Option<&'i dyn FailNext>,
    write: Option<(u8, u8)>,
}

impl<'i> ScdcTransportCheck<'i> {
    /// The check over `image`.
    pub const fn new(image: &'i ScdcImage) -> Self {
        Self {
            image,
            faults: None,
            write: None,
        }
    }

    /// The check writing `value` to `reg`, a register the caller knows is safe to write,
    /// and reading it back; then, where the image knows `reg` and holds another value,
    /// writing that back and reading it. For a sink, `Source_Version` (0x02) and 1, which a
    /// source writes at every bring-up. A `value` other than the one `reg` holds shows a
    /// write that does nothing.
    #[must_use]
    pub const fn with_write(self, reg: u8, value: u8) -> Self {
        Self {
            write: Some((reg, value)),
            ..self
        }
    }

    /// The check with `faults` making transfers fail, for the points after an error.
    #[must_use]
    pub const fn with_faults(self, faults: &'i dyn FailNext) -> Self {
        Self {
            faults: Some(faults),
            ..self
        }
    }
}

impl fmt::Debug for ScdcTransportCheck<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScdcTransportCheck")
            .field("known", &self.image.known())
            .field("faults", &self.faults.is_some())
            .field("write", &self.write)
            .finish()
    }
}

/// Records whether `buf`, read from `start`, agrees with the image.
fn compare_block(
    report: &mut Report<ScdcTransportFailure>,
    image: &ScdcImage,
    start: u8,
    buf: &[u8],
) {
    let mismatch = buf.iter().enumerate().find_map(|(i, &found)| {
        let reg = start.wrapping_add(i as u8);
        image
            .get(reg)
            .filter(|&expected| expected != found)
            .map(|expected| ScdcTransportFailure::Value {
                op: ScdcOp::ReadBlock,
                reg,
                expected,
                found,
            })
    });
    report.record(mismatch);
}

/// The check's body, once for the sync trait and once, with `.await`, for the async one.
macro_rules! body {
    ($check:ident, $transport:ident, $($await:tt)*) => {{
        let mut report = Report::new();
        let image = $check.image;
        for reg in 0..=255u8 {
            let Some(expected) = image.get(reg) else { continue };
            report.record(match $transport.read(reg) $($await)* {
                Ok(found) if found == expected => None,
                Ok(found) => Some(ScdcTransportFailure::Value {
                    op: ScdcOp::Read,
                    reg,
                    expected,
                    found,
                }),
                Err(error) => Some(ScdcTransportFailure::Failed {
                    op: ScdcOp::Read,
                    reg,
                    kind: error.kind(),
                }),
            });
        }
        let mut buf = [0u8; 16];
        for start in 0..=0xF0u8 {
            match $transport.read_block(start, &mut buf) $($await)* {
                Ok(()) => compare_block(&mut report, image, start, &buf),
                Err(error) => report.record(Some(ScdcTransportFailure::Failed {
                    op: ScdcOp::ReadBlock,
                    reg: start,
                    kind: error.kind(),
                })),
            }
        }
        for start in [0x00, 0x20, 0xFF] {
            let result = $transport.read_block(start, &mut []) $($await)*;
            report.record(result.err().map(|error| ScdcTransportFailure::Failed {
                op: ScdcOp::ReadBlock,
                reg: start,
                kind: error.kind(),
            }));
        }
        // Past 0xFF the result is the sink's: the point is that it returns.
        for (start, len) in [(0xF8u8, 16usize), (0xFF, 2)] {
            let _ = $transport.read_block(start, &mut buf[..len]) $($await)*;
            report.record(None);
        }
        if let Some((reg, value)) = $check.write {
            let mut values = [Some(value), image.get(reg).filter(|&held| held != value)];
            for value in values.iter_mut().filter_map(Option::take) {
                match $transport.write(reg, value) $($await)* {
                    Err(error) => report.record(Some(ScdcTransportFailure::Failed {
                        op: ScdcOp::Write,
                        reg,
                        kind: error.kind(),
                    })),
                    Ok(()) => report.record(match $transport.read(reg) $($await)* {
                        Ok(found) if found == value => None,
                        Ok(found) => Some(ScdcTransportFailure::Value {
                            op: ScdcOp::Write,
                            reg,
                            expected: value,
                            found,
                        }),
                        Err(error) => Some(ScdcTransportFailure::Failed {
                            op: ScdcOp::Read,
                            reg,
                            kind: error.kind(),
                        }),
                    }),
                }
            }
        }
        let known = (0..=255u8).find_map(|reg| image.get(reg).map(|value| (reg, value)));
        if let (Some(faults), Some((reg, value))) = ($check.faults, known) {
            for kind in RECOVERABLE {
                if !faults.fail_next(kind) {
                    report.record(Some(ScdcTransportFailure::NotArmed { kind }));
                    continue;
                }
                let found = $transport.read(reg) $($await)*;
                faults.withdraw();
                let found = found.map(|_| ()).map_err(|error| error.kind());
                report.record((found != Err(kind)).then_some(ScdcTransportFailure::Hidden {
                    expected: kind,
                    found,
                }));
                report.record(match $transport.read(reg) $($await)* {
                    Ok(found) if found == value => None,
                    Ok(found) => Some(ScdcTransportFailure::Value {
                        op: ScdcOp::Read,
                        reg,
                        expected: value,
                        found,
                    }),
                    Err(error) => Some(ScdcTransportFailure::Failed {
                        op: ScdcOp::Read,
                        reg,
                        kind: error.kind(),
                    }),
                });
            }
        }
        report
    }};
}

impl ScdcTransportCheck<'_> {
    /// Runs the check against `transport`.
    pub fn run<T>(&self, transport: &mut T) -> Report<ScdcTransportFailure>
    where
        T: hdmi_hal::scdc::ScdcTransport + ?Sized,
    {
        let check = self;
        body!(check, transport,)
    }

    /// Runs the check against an async `transport`: the same points as [`run`](Self::run).
    #[cfg(feature = "async")]
    pub async fn run_async<T>(&self, transport: &mut T) -> Report<ScdcTransportFailure>
    where
        T: hdmi_hal_async::scdc::ScdcTransport + ?Sized,
    {
        let check = self;
        body!(check, transport, .await)
    }
}
