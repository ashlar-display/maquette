//! The bench's DDC bus: `DdcBus` over the sink's targets.

use core::fmt;
use core::time::Duration;

use hdmi_hal::ddc::{
    DdcBus, DdcCapabilities, DdcError, DdcErrorKind, EddcTarget, MAX_SEGMENT, SEGMENT_POINTER,
    is_usable_address,
};

use crate::bench::{Core, Reach};
use crate::target::Operation;
use crate::transcript::{BusOp, BusRecord};

/// What a failed read leaves in its buffer: `DdcBus` leaves the contents unspecified, so a
/// caller that uses them after an error reads this pattern, not plausible data.
pub(crate) const FAILED_READ_FILL: u8 = 0xA5;

/// The bench's DDC bus, a `Copy` handle borrowed from the bench ([`Bench::ddc`]).
///
/// It implements `DdcBus` over the sink's targets, with the capabilities given to the bench
/// (by default `DdcCapabilities::general()`):
///
/// - **Before the sink:** `DdcBus`'s argument rules (an address outside
///   `is_usable_address`, an empty `write`, a segment above `MAX_SEGMENT` or a read past 256
///   bytes of a segment: `Unsupported`; an empty buffer: `Ok`), then the capabilities. A
///   refused call touches no target, takes no time and is recorded with its result.
/// - **Each call is one transaction**, routed to a target; `read_segmented` is split into
///   pieces of `segmented_piece` bytes where the capabilities set one, each piece its own
///   transaction with the pointer and offset written again. Segment 0 writes no pointer.
/// - **Time:** each transaction charges the bench's clock after it is evaluated, by the
///   bench's [`BusTiming`]: every byte of it, address bytes included, as if it all went out,
///   except a transaction nothing answered, which charges its first address byte.
/// - **A failed read** fills its buffer with `0xA5`.
///
/// [`Bench::ddc`]: crate::Bench::ddc
#[derive(Clone, Copy)]
pub struct Ddc<'b> {
    core: &'b (dyn Core + Sync + 'b),
}

impl<'b> Ddc<'b> {
    pub(crate) fn new(core: &'b (dyn Core + Sync + 'b)) -> Self {
        Self { core }
    }

    /// Records a call refused before the sink.
    fn refuse(&self, record: BusRecord) -> Result<(), BusError> {
        let record = BusRecord {
            result: Err(DdcErrorKind::Unsupported),
            ..record
        };
        self.core.record(record);
        Err(BusError::from_record(&record, DdcErrorKind::Unsupported))
    }

    /// Runs one transaction, charges it, and returns its result as the call's.
    fn run(&self, record: BusRecord, ops: &mut [Operation<'_>]) -> Result<(), BusError> {
        let (result, reach) = self.core.transaction(record, ops);
        let timing = self.core.timing();
        let wire_bytes = match reach {
            Reach::Answered => ops
                .iter()
                .map(|op| op.len().saturating_add(1))
                .sum::<usize>(),
            Reach::Unanswered => 1,
            Reach::Detached => 0,
        };
        if reach != Reach::Detached {
            let bytes = u32::try_from(wire_bytes).unwrap_or(u32::MAX);
            self.core.clock().advance(
                timing
                    .per_transaction
                    .saturating_add(timing.per_byte.saturating_mul(bytes)),
            );
        }
        result.map_err(|kind| {
            for op in ops.iter_mut() {
                if let Operation::Read { buf, .. } = op {
                    buf.fill(FAILED_READ_FILL);
                }
            }
            BusError::from_record(&record, kind)
        })
    }
}

impl fmt::Debug for Ddc<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ddc")
            .field("capabilities", &self.core.capabilities())
            .finish_non_exhaustive()
    }
}

fn saturating_len(len: usize) -> u16 {
    u16::try_from(len).unwrap_or(u16::MAX)
}

impl DdcBus for Ddc<'_> {
    type Error = BusError;

    fn capabilities(&self) -> DdcCapabilities {
        self.core.capabilities()
    }

    fn write(&self, addr: u8, bytes: &[u8]) -> Result<(), BusError> {
        self.core.spend();
        let record = BusRecord {
            op: BusOp::Write,
            addr,
            first: bytes.first().copied(),
            written: saturating_len(bytes.len()),
            read: 0,
            result: Ok(()),
        };
        let caps = self.capabilities();
        if !is_usable_address(addr)
            || bytes.is_empty()
            || !caps.reaches(addr)
            || bytes.len() > usize::from(caps.write_max())
        {
            return self.refuse(record);
        }
        self.run(record, &mut [Operation::Write { addr, bytes }])
    }

    fn write_read(&self, addr: u8, bytes: &[u8], buf: &mut [u8]) -> Result<(), BusError> {
        if bytes.is_empty() {
            return self.read(addr, buf);
        }
        self.core.spend();
        let record = BusRecord {
            op: BusOp::WriteRead,
            addr,
            first: bytes.first().copied(),
            written: saturating_len(bytes.len()),
            read: saturating_len(buf.len()),
            result: Ok(()),
        };
        if !is_usable_address(addr) {
            return self.refuse(record);
        }
        if buf.is_empty() {
            self.core.record(record);
            return Ok(());
        }
        let caps = self.capabilities();
        let (write_max, read_max) = caps.write_read_max();
        if !caps.reaches(addr)
            || bytes.len() > usize::from(write_max)
            || buf.len() > usize::from(read_max)
        {
            return self.refuse(record);
        }
        self.run(
            record,
            &mut [
                Operation::Write { addr, bytes },
                Operation::Read { addr, buf },
            ],
        )
    }

    fn read(&self, addr: u8, buf: &mut [u8]) -> Result<(), BusError> {
        self.core.spend();
        let record = BusRecord {
            op: BusOp::Read,
            addr,
            first: None,
            written: 0,
            read: saturating_len(buf.len()),
            result: Ok(()),
        };
        if !is_usable_address(addr) {
            return self.refuse(record);
        }
        if buf.is_empty() {
            self.core.record(record);
            return Ok(());
        }
        let caps = self.capabilities();
        if !caps.reaches(addr) || buf.len() > usize::from(caps.read_max()) {
            return self.refuse(record);
        }
        self.run(record, &mut [Operation::Read { addr, buf }])
    }

    fn read_segmented(
        &self,
        target: EddcTarget,
        segment: u8,
        offset: u8,
        buf: &mut [u8],
    ) -> Result<(), BusError> {
        self.core.spend();
        let addr = target.address();
        let pointer = u16::from(segment > 0);
        let record = BusRecord {
            op: BusOp::ReadSegmented { target, segment },
            addr,
            first: Some(offset),
            written: pointer + 1,
            read: saturating_len(buf.len()),
            result: Ok(()),
        };
        if buf.is_empty() {
            self.core.record(record);
            return Ok(());
        }
        let caps = self.capabilities();
        if segment > MAX_SEGMENT
            || usize::from(offset) + buf.len() > 256
            || !caps.reads_target(target)
            || caps.max_segment().is_none_or(|max| segment > max)
        {
            return self.refuse(record);
        }
        let piece = match caps.segmented_piece() {
            Some(piece) if piece > 0 => usize::from(piece),
            _ => buf.len(),
        };
        let mut start = 0;
        while start < buf.len() {
            let end = buf.len().min(start + piece);
            // `offset + buf.len()` is at most 256, so the piece's offset fits a byte.
            let piece_offset = offset.wrapping_add(start as u8);
            let record = BusRecord {
                first: Some(piece_offset),
                read: saturating_len(end - start),
                ..record
            };
            let chunk = &mut buf[start..end];
            let offset_byte = [piece_offset];
            let segment_byte = [segment];
            let result = if segment > 0 {
                self.run(
                    record,
                    &mut [
                        Operation::Write {
                            addr: SEGMENT_POINTER,
                            bytes: &segment_byte,
                        },
                        Operation::Write {
                            addr,
                            bytes: &offset_byte,
                        },
                        Operation::Read { addr, buf: chunk },
                    ],
                )
            } else {
                self.run(
                    record,
                    &mut [
                        Operation::Write {
                            addr,
                            bytes: &offset_byte,
                        },
                        Operation::Read { addr, buf: chunk },
                    ],
                )
            };
            if let Err(error) = result {
                buf.fill(FAILED_READ_FILL);
                return Err(error);
            }
            start = end;
        }
        Ok(())
    }
}

/// A failed call of the bench's DDC bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct BusError {
    /// How it failed.
    pub kind: DdcErrorKind,
    /// The call.
    pub op: BusOp,
    /// The address of the call: for `read_segmented`, the target's.
    pub addr: u8,
}

impl BusError {
    fn from_record(record: &BusRecord, kind: DdcErrorKind) -> Self {
        Self {
            kind,
            op: record.op,
            addr: record.addr,
        }
    }
}

impl DdcError for BusError {
    fn kind(&self) -> DdcErrorKind {
        self.kind
    }
}

impl fmt::Display for BusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} at {:#04x} failed: {:?}",
            self.op, self.addr, self.kind
        )
    }
}

impl core::error::Error for BusError {}

/// How long the bench's bus operations take on its clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct BusTiming {
    /// Each byte on the wire, address bytes included.
    pub per_byte: Duration,
    /// Each transaction, on top of its bytes.
    pub per_transaction: Duration,
}

impl BusTiming {
    /// 90 µs a byte, nothing per transaction: a byte is eight data bits and the acknowledge,
    /// nine clocks, and 100 kHz is I²C standard mode's maximum clock (DDC/CI 1.1), the
    /// fastest a DDC bus runs.
    pub const STANDARD_MODE: Self = Self {
        per_byte: Duration::from_micros(90),
        per_transaction: Duration::ZERO,
    };

    /// The timing with each byte taking `per_byte`.
    pub const fn with_per_byte(self, per_byte: Duration) -> Self {
        Self { per_byte, ..self }
    }

    /// The timing with each transaction taking `per_transaction` on top of its bytes.
    pub const fn with_per_transaction(self, per_transaction: Duration) -> Self {
        Self {
            per_transaction,
            ..self
        }
    }
}

impl Default for BusTiming {
    fn default() -> Self {
        Self::STANDARD_MODE
    }
}
