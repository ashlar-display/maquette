//! Targets: the devices of the sink at their DDC addresses.

use core::time::Duration;
use hdmi_hal::ddc::DdcErrorKind;

use hdmi_hal::time::Timestamp;

use crate::line::{ErrorRates, Line};
use crate::transcript::{Event, Record, Transcript};

/// A device on the sink's DDC bus, as the bench routes transactions to it.
///
/// A target runs with the bench locked: it sees the bench through [`Env`] only. Using the
/// bench's handles or controls from a target panics, naming the cause.
///
/// A target sees what a device on the bus sees: transactions, each a sequence of writes and
/// reads to addresses, ending in STOP. The bench turns each `DdcBus` call into one
/// transaction: `write` and `read` into one operation, `write_read` into a write and a read,
/// and a piece of `read_segmented` into a write of the segment to 0x30 (for segment 1 and
/// higher), a write of the offset and a read.
///
/// A transaction goes to the first target that [answers](Self::answers) the address of its
/// first operation, the test's targets first, in the order given to the bench; when none
/// answers, it fails `NoAcknowledge(Address)`. The target that takes a transaction takes all
/// of it: a later operation to an address it does not answer is its to refuse.
///
/// ```
/// use maquette::{Env, Operation, Target};
/// use maquette::hdmi_hal::ddc::{DdcErrorKind, NoAcknowledgeSource};
///
/// /// A DDC/CI device that answers every read with the same byte.
/// struct Constant(u8);
///
/// impl Target for Constant {
///     fn answers(&self, addr: u8, _env: &Env<'_>) -> bool {
///         addr == 0x37
///     }
///
///     fn transaction(
///         &mut self,
///         ops: &mut [Operation<'_>],
///         _env: &mut Env<'_>,
///     ) -> Result<(), DdcErrorKind> {
///         for op in ops {
///             match op {
///                 Operation::Read { addr: 0x37, buf } => buf.fill(self.0),
///                 Operation::Write { addr: 0x37, .. } => {}
///                 _ => return Err(DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Address)),
///             }
///         }
///         Ok(())
///     }
/// }
/// ```
pub trait Target {
    /// Whether this target answers at `addr` now.
    fn answers(&self, addr: u8, env: &Env<'_>) -> bool;

    /// One transaction: `ops` in order, ending in STOP.
    ///
    /// The target applies what it accepts and fills every read; an error says how the
    /// transaction failed, and what the target applied before it stays applied.
    fn transaction(
        &mut self,
        ops: &mut [Operation<'_>],
        env: &mut Env<'_>,
    ) -> Result<(), DdcErrorKind>;

    /// A bench event that is not a transaction: time moved, HPD or the line changed. Does
    /// nothing by default.
    fn update(&mut self, env: &mut Env<'_>) {
        let _ = env;
    }
}

/// One operation of a transaction.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Operation<'b> {
    /// START (or repeated START), the address with W, `bytes`.
    Write {
        /// The 7-bit address.
        addr: u8,
        /// The bytes written.
        bytes: &'b [u8],
    },
    /// START (or repeated START), the address with R, `buf.len()` bytes read.
    Read {
        /// The 7-bit address.
        addr: u8,
        /// Where the bytes read go.
        buf: &'b mut [u8],
    },
}

impl Operation<'_> {
    /// The operation's address.
    pub fn addr(&self) -> u8 {
        match self {
            Self::Write { addr, .. } | Self::Read { addr, .. } => *addr,
        }
    }

    /// How many data bytes the operation carries.
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Write { bytes, .. } => bytes.len(),
            Self::Read { buf, .. } => buf.len(),
        }
    }
}

/// The bench as a target sees it, for the length of one call.
pub struct Env<'e> {
    now: Timestamp,
    hpd: bool,
    line: Line,
    errors: ErrorRates,
    byte_time: Duration,
    log: &'e mut dyn NoteSink,
}

impl<'e> Env<'e> {
    pub(crate) fn new(now: Timestamp, hpd: bool, line: Line, log: &'e mut dyn NoteSink) -> Self {
        Self {
            now,
            hpd,
            line,
            errors: ErrorRates::NONE,
            byte_time: Duration::ZERO,
            log,
        }
    }

    /// The bench as before, with the link's character errors.
    pub(crate) fn with_errors(self, errors: ErrorRates) -> Self {
        Self { errors, ..self }
    }

    /// The bench's time: for a transaction, when it started.
    pub fn now(&self) -> Timestamp {
        self.now
    }

    /// Whether HPD is high.
    pub fn hpd(&self) -> bool {
        self.hpd
    }

    /// What the source transmits, as the sink receives it: [`Line::Off`] while the sink is
    /// unplugged.
    pub fn line(&self) -> Line {
        self.line
    }

    /// The bench as before, each byte of a transaction taking `byte_time` on the wire.
    pub(crate) fn with_byte_time(self, byte_time: Duration) -> Self {
        Self { byte_time, ..self }
    }

    /// How long each byte of the transaction takes on the wire: zero outside one.
    pub(crate) fn byte_time(&self) -> Duration {
        self.byte_time
    }

    /// The character errors on the link, as the sink receives them: none while it is
    /// unplugged.
    pub fn character_errors(&self) -> ErrorRates {
        self.errors
    }

    /// Records a built-in target's event at [`now`](Self::now).
    pub(crate) fn record(&mut self, event: Event) {
        self.log.note(Record {
            at: self.now,
            event,
        });
    }

    /// Records one of the target's own events in the transcript, as an
    /// [`Event::Note`](crate::Event::Note) at [`now`](Self::now).
    pub fn note(&mut self, addr: u8, code: u32, value: u32) {
        self.log.note(Record {
            at: self.now,
            event: Event::Note { addr, code, value },
        });
    }
}

impl core::fmt::Debug for Env<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Env")
            .field("now", &self.now)
            .field("hpd", &self.hpd)
            .field("line", &self.line)
            .finish_non_exhaustive()
    }
}

/// Where a target's events go: the transcript, whatever its capacity.
pub(crate) trait NoteSink {
    fn note(&mut self, record: Record);
}

impl<const LOG: usize> NoteSink for Transcript<LOG> {
    fn note(&mut self, record: Record) {
        self.push(record);
    }
}
