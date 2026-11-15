//! The transcript: one ordered, typed record of everything the bench and the sink did.

use core::fmt;

use hdmi_hal::ddc::{DdcErrorKind, EddcTarget, NoAcknowledgeSource};
use hdmi_hal::time::Timestamp;

use crate::config::RefusalKind;
use crate::fault::{Effect, FaultId, Point};
use crate::line::Line;
use crate::phy::{PhyCall, PhyErrorKind};

/// One entry of the transcript: what happened, and when.
///
/// The bench makes records and a test reads them: like [`Event`] and [`BusRecord`], it is
/// `#[non_exhaustive]`, so a test compares the fields it cares about, not a built value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Record {
    /// When it happened: for a bus operation, when it started.
    pub at: Timestamp,
    /// What happened.
    pub event: Event,
}

/// What a [`Record`] records.
///
/// The bench makes the records; a test reads them. Each variant with fields is
/// `#[non_exhaustive]`, so it may gain one without a breaking change: match it with `..`,
/// and compare what a test cares about rather than a whole built `Event`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Event {
    /// A `DdcBus` call, or one piece of a split `read_segmented`: what was asked and how it
    /// ended, refused calls included.
    Bus(BusRecord),
    /// A register of a built-in target changed, by the source or by the sink.
    #[non_exhaustive]
    Register {
        /// The target's address.
        addr: u8,
        /// The register.
        reg: u8,
        /// Its value before.
        from: u8,
        /// Its value after.
        to: u8,
        /// Who changed it.
        by: Actor,
    },
    /// A built-in target ignored a write, and why.
    #[non_exhaustive]
    IgnoredWrite {
        /// The target's address.
        addr: u8,
        /// The register or offset written.
        reg: u8,
        /// The value written.
        value: u8,
        /// Why it was ignored.
        why: Ignored,
    },
    /// What the source transmits changed ([`Bench::set_line`](crate::Bench::set_line)).
    Line(Line),
    /// HPD changed.
    #[non_exhaustive]
    Hpd {
        /// Whether HPD is now high.
        high: bool,
    },
    /// The sink was unplugged or plugged in.
    #[non_exhaustive]
    Plug {
        /// Whether the sink is now plugged in.
        plugged: bool,
    },
    /// The adapter was detached or reattached.
    #[non_exhaustive]
    Adapter {
        /// Whether the adapter is now attached.
        attached: bool,
    },
    /// The sink refused to set the scrambling bit right away (`SinkConfig`'s refusals).
    #[non_exhaustive]
    Refusal {
        /// Which attempt, counted from 1: a write of `TMDS_Config` with `Scrambling_Enable`
        /// set.
        attempt: u32,
        /// How it refused.
        kind: RefusalKind,
    },
    /// A scheduled fault struck the bus operation recorded before it.
    #[non_exhaustive]
    Fault {
        /// The fault.
        id: FaultId,
        /// What it did.
        effect: Effect,
        /// Where it struck.
        point: Point,
    },
    /// A call of the simulated PHY, and how it ended, recorded when it returned.
    #[non_exhaustive]
    Phy {
        /// The call, with its arguments.
        call: PhyCall,
        /// How it ended.
        result: Result<(), PhyErrorKind>,
    },
    /// A test's target's own event, recorded with [`Env::note`](crate::Env::note).
    #[non_exhaustive]
    Note {
        /// The address the target noted it for.
        addr: u8,
        /// The target's own code for the event.
        code: u32,
        /// A value that goes with it.
        value: u32,
    },
    /// A scheduled action came: the events it makes follow.
    #[non_exhaustive]
    Action {
        /// The id [`Bench::schedule_action`](crate::Bench::schedule_action) returned.
        id: FaultId,
    },
}

/// A bus operation as the transcript records it: read, not built (see [`Record`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct BusRecord {
    /// The operation.
    pub op: BusOp,
    /// The address of the operation: for `read_segmented`, the target's.
    pub addr: u8,
    /// The first byte written (an offset or a register), if any: for `read_segmented`, the
    /// offset of this piece.
    pub first: Option<u8>,
    /// How many bytes were written, the segment pointer's included; saturates at
    /// `u16::MAX`.
    pub written: u16,
    /// How many bytes were read; saturates at `u16::MAX`.
    pub read: u16,
    /// How it ended.
    pub result: Result<(), DdcErrorKind>,
}

/// The kind of a bus operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BusOp {
    /// `DdcBus::write`.
    Write,
    /// `DdcBus::write_read` with something to write; one with nothing to write is a `Read`.
    WriteRead,
    /// `DdcBus::read`.
    Read,
    /// `DdcBus::read_segmented`, or one piece of it.
    ReadSegmented {
        /// The structure read.
        target: EddcTarget,
        /// The segment read.
        segment: u8,
    },
}

/// The method's name; for a `read_segmented`, the structure and the segment too.
impl fmt::Display for BusOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Write => f.write_str("write"),
            Self::WriteRead => f.write_str("write_read"),
            Self::Read => f.write_str("read"),
            Self::ReadSegmented { target, segment } => {
                let target = match target {
                    EddcTarget::Edid => "the EDID",
                    EddcTarget::DisplayId => "the DisplayID",
                    _ => "a structure",
                };
                write!(f, "read_segmented of {target}, segment {segment}")
            }
        }
    }
}

/// Writes what `kind` means, in words: hdmi-hal's kinds have no `Display`.
pub(crate) fn describe_ddc(kind: DdcErrorKind, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(match kind {
        DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Address) => {
            "the address was not acknowledged"
        }
        DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Data) => {
            "a data byte was not acknowledged"
        }
        DdcErrorKind::NoAcknowledge(_) => "a byte was not acknowledged",
        DdcErrorKind::Timeout => "it timed out",
        DdcErrorKind::Busy => "the bus was busy",
        DdcErrorKind::Unsupported => "unsupported",
        DdcErrorKind::Detached => "the adapter is detached",
        DdcErrorKind::Other => "it failed",
        other => return write!(f, "{other:?}"),
    })
}

/// Who changed a register.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Actor {
    /// The source, by a bus write.
    Source,
    /// The sink, on its own.
    Sink,
    /// The test, by a preset.
    Test,
}

/// Why a built-in target ignored a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Ignored {
    /// The register is the sink's to set, not the source's.
    SinkOwned,
    /// The structure is not writable: the EDID and DisplayID after the offset byte.
    ReadOnly,
}

/// The transcript's storage: `LOG` records, then a count of those that did not fit.
#[derive(Debug)]
pub(crate) struct Transcript<const LOG: usize> {
    records: [Option<Record>; LOG],
    len: usize,
    overflow: usize,
}

impl<const LOG: usize> Transcript<LOG> {
    pub(crate) const fn new() -> Self {
        Self {
            records: [None; LOG],
            len: 0,
            overflow: 0,
        }
    }

    /// Appends `record`, returning its index, or counts it when the transcript is full.
    pub(crate) fn push(&mut self, record: Record) -> Option<usize> {
        match self.records.get_mut(self.len) {
            Some(slot) => {
                *slot = Some(record);
                self.len += 1;
                Some(self.len - 1)
            }
            None => {
                self.overflow = self.overflow.saturating_add(1);
                None
            }
        }
    }

    /// Sets the result of the bus record at `index`.
    pub(crate) fn set_result(&mut self, index: Option<usize>, result: Result<(), DdcErrorKind>) {
        if let Some(Some(Record {
            event: Event::Bus(bus),
            ..
        })) = index.and_then(|index| self.records.get_mut(index))
        {
            bus.result = result;
        }
    }

    pub(crate) fn get(&self, index: usize) -> Option<Record> {
        self.records[..self.len].get(index).copied().flatten()
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn overflow(&self) -> usize {
        self.overflow
    }

    pub(crate) fn clear(&mut self) {
        self.records = [None; LOG];
        self.len = 0;
        self.overflow = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(code: u32) -> Record {
        Record {
            at: Timestamp::from_micros(u64::from(code)),
            event: Event::Note {
                addr: 0x37,
                code,
                value: 0,
            },
        }
    }

    fn bus() -> Record {
        Record {
            at: Timestamp::from_micros(0),
            event: Event::Bus(BusRecord {
                op: BusOp::Write,
                addr: 0x54,
                first: Some(0x20),
                written: 2,
                read: 0,
                result: Ok(()),
            }),
        }
    }

    #[test]
    fn keeps_records_in_order_and_counts_the_overflow() {
        let mut transcript = Transcript::<2>::new();
        assert_eq!(transcript.push(note(1)), Some(0));
        assert_eq!(transcript.push(note(2)), Some(1));
        assert_eq!(transcript.push(note(3)), None);
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript.overflow(), 1);
        assert_eq!(transcript.get(0), Some(note(1)));
        assert_eq!(transcript.get(1), Some(note(2)));
        assert_eq!(transcript.get(2), None);
        transcript.clear();
        assert_eq!((transcript.len(), transcript.overflow()), (0, 0));
        assert_eq!(transcript.get(0), None);
    }

    #[test]
    fn sets_a_bus_records_result_only() {
        let mut transcript = Transcript::<2>::new();
        let index = transcript.push(bus());
        transcript.set_result(index, Err(DdcErrorKind::Busy));
        assert!(matches!(
            transcript.get(0),
            Some(Record {
                event: Event::Bus(BusRecord {
                    result: Err(DdcErrorKind::Busy),
                    ..
                }),
                ..
            })
        ));
        // Not a bus record, an overflowed one, an index out of range: nothing changes.
        let index = transcript.push(note(1));
        transcript.set_result(index, Err(DdcErrorKind::Busy));
        transcript.set_result(None, Err(DdcErrorKind::Busy));
        transcript.set_result(Some(7), Err(DdcErrorKind::Busy));
        assert_eq!(transcript.get(1), Some(note(1)));
    }

    #[test]
    fn a_transcript_of_no_capacity_counts_everything() {
        let mut transcript = Transcript::<0>::new();
        assert_eq!(transcript.push(note(1)), None);
        assert_eq!(transcript.overflow(), 1);
    }
}
