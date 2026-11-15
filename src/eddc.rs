//! The built-in E-DDC target: the EDID at 0x50 and the DisplayID at 0x52, behind the segment
//! pointer at 0x30 (VESA E-DDC 1.3).

use hdmi_hal::ddc::{DISPLAYID, DdcErrorKind, EDID, NoAcknowledgeSource, SEGMENT_POINTER};

use crate::config::{BlockBoundary, DuringHpdLow, MissingSegment, PastStore, SinkConfig};
use crate::target::{Env, Operation, Target};
use crate::transcript::{Event, Ignored};

const NACK_ADDRESS: DdcErrorKind = DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Address);
const NACK_DATA: DdcErrorKind = DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Data);

/// What a read past the store returns under [`PastStore::Fill`].
const FILL: u8 = 0xFF;

/// A segment's size: 256 bytes (E-DDC 1.3, 2.2.5).
const SEGMENT: usize = 256;

/// The EDID and DisplayID stores, the segment pointer and the word offsets.
///
/// - The pointer is 0 at the start of every transaction: the sink resets it on a NACK or STOP
///   (8.1.1, Table 9-1) and at the end of each command sequence (2.2.5), so a write to 0x30
///   matters only to the reads in the same transaction. A read at 0x30 is refused
///   (*chosen*: 2.2.5 calls the pointer effectively write-only because it resets after each
///   command sequence, and does not say what a read gets); of several bytes written to it,
///   the first counts (*chosen*).
/// - Each structure's word offset persists between transactions and advances with every byte
///   read: a read with no offset written reads at the current address (6.1, 6.3).
/// - For segment 1 and higher the sink acknowledges the pointer value (6.4, 6.5); for a
///   segment neither store has bytes in, [`MissingSegment`] says.
/// - A write to 0x50 or 0x52 sets the word offset with its first byte; the bytes after it are
///   acknowledged and ignored, without moving the offset, and recorded: E-DDC does not require
///   a display to support writes there (6).
/// - A sink without a segment pointer does not answer 0x30 (8.1.2).
/// - An empty store does not answer its address (*chosen*: a sink without that structure).
#[derive(Debug)]
pub(crate) struct Eddc<'a> {
    edid: &'a [u8],
    displayid: Option<&'a [u8]>,
    segment_pointer: bool,
    /// The word offsets, of the EDID and of the DisplayID.
    offsets: [u8; 2],
    past_store: PastStore,
    missing_segment: MissingSegment,
    block_boundary: BlockBoundary,
    during_hpd_low: DuringHpdLow,
}

impl<'a> Eddc<'a> {
    pub(crate) fn new(config: &SinkConfig, edid: &'a [u8], displayid: Option<&'a [u8]>) -> Self {
        Self {
            edid,
            displayid,
            segment_pointer: config.segment_pointer,
            offsets: [0; 2],
            past_store: config.past_store,
            missing_segment: config.missing_segment,
            block_boundary: config.block_boundary,
            during_hpd_low: config.edid_during_hpd_low,
        }
    }

    pub(crate) fn edid_len(&self) -> usize {
        self.edid.len()
    }

    pub(crate) fn displayid_len(&self) -> Option<usize> {
        self.displayid.map(<[u8]>::len)
    }

    /// The store at `addr` and the index of its word offset, if it answers.
    fn store(&self, addr: u8) -> Option<(&'a [u8], usize)> {
        let (store, index) = match addr {
            EDID => (self.edid, 0),
            DISPLAYID => (self.displayid?, 1),
            _ => return None,
        };
        (!store.is_empty()).then_some((store, index))
    }

    /// Whether either store has bytes in `segment`.
    fn has_segment(&self, segment: u8) -> bool {
        let start = usize::from(segment) * SEGMENT;
        start < self.edid.len() || self.displayid.is_some_and(|store| start < store.len())
    }

    /// The byte at `offset` in `segment` of `store`.
    fn byte(&self, store: &[u8], segment: u8, offset: u8) -> Result<u8, DdcErrorKind> {
        let index = usize::from(segment) * SEGMENT + usize::from(offset);
        match store.get(index) {
            Some(&byte) => Ok(byte),
            None => match self.past_store {
                PastStore::Fill => Ok(FILL),
                PastStore::Wrap => Ok(store[index % store.len()]),
                PastStore::Nack => Err(NACK_DATA),
            },
        }
    }

    /// The word offset after `offset`.
    fn next(&self, offset: u8) -> u8 {
        match self.block_boundary {
            BlockBoundary::Continue => offset.wrapping_add(1),
            BlockBoundary::Wrap => (offset & 0x80) | (offset.wrapping_add(1) & 0x7F),
        }
    }
}

impl Target for Eddc<'_> {
    fn answers(&self, addr: u8, env: &Env<'_>) -> bool {
        let awake = env.hpd() || self.during_hpd_low == DuringHpdLow::Answers;
        awake && ((addr == SEGMENT_POINTER && self.segment_pointer) || self.store(addr).is_some())
    }

    fn transaction(
        &mut self,
        ops: &mut [Operation<'_>],
        env: &mut Env<'_>,
    ) -> Result<(), DdcErrorKind> {
        let mut segment = 0;
        for op in ops.iter_mut() {
            let addr = op.addr();
            if addr == SEGMENT_POINTER && self.segment_pointer {
                let Operation::Write { bytes, .. } = op else {
                    return Err(NACK_ADDRESS);
                };
                if let Some(&value) = bytes.first() {
                    let missing = value > 0 && !self.has_segment(value);
                    if missing && self.missing_segment == MissingSegment::Nack {
                        return Err(NACK_DATA);
                    }
                    segment = value;
                }
                continue;
            }
            let Some((store, index)) = self.store(addr) else {
                return Err(NACK_ADDRESS);
            };
            match op {
                Operation::Write { bytes, .. } => {
                    let Some((&offset, data)) = bytes.split_first() else {
                        continue;
                    };
                    self.offsets[index] = offset;
                    for &value in data {
                        env.record(Event::IgnoredWrite {
                            addr,
                            reg: offset,
                            value,
                            why: Ignored::ReadOnly,
                        });
                    }
                }
                Operation::Read { buf, .. } => {
                    for slot in buf.iter_mut() {
                        let offset = self.offsets[index];
                        *slot = self.byte(store, segment, offset)?;
                        self.offsets[index] = self.next(offset);
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line::Line;
    use crate::transcript::Transcript;
    use hdmi_hal::time::Timestamp;

    fn run(eddc: &mut Eddc<'_>, ops: &mut [Operation<'_>]) -> Result<(), DdcErrorKind> {
        let mut transcript = Transcript::<8>::new();
        let mut env = Env::new(Timestamp::from_micros(0), true, Line::Off, &mut transcript);
        eddc.transaction(ops, &mut env)
    }

    #[test]
    fn of_several_pointer_bytes_the_first_counts() {
        let edid: [u8; 512] = core::array::from_fn(|i| (i / 256) as u8);
        let mut eddc = Eddc::new(&SinkConfig::new(), &edid, None);
        let mut buf = [0xEE; 1];
        let result = run(
            &mut eddc,
            &mut [
                Operation::Write {
                    addr: SEGMENT_POINTER,
                    bytes: &[1, 0],
                },
                Operation::Write {
                    addr: EDID,
                    bytes: &[0],
                },
                Operation::Read {
                    addr: EDID,
                    buf: &mut buf,
                },
            ],
        );
        assert_eq!(result, Ok(()));
        assert_eq!(buf, [1]);
    }

    #[test]
    fn empty_writes_change_nothing() {
        let edid = [7u8; 128];
        let mut eddc = Eddc::new(&SinkConfig::new(), &edid, None);
        let mut buf = [0u8; 1];
        let result = run(
            &mut eddc,
            &mut [
                Operation::Write {
                    addr: SEGMENT_POINTER,
                    bytes: &[],
                },
                Operation::Write {
                    addr: EDID,
                    bytes: &[],
                },
                Operation::Read {
                    addr: EDID,
                    buf: &mut buf,
                },
            ],
        );
        assert_eq!(result, Ok(()));
        assert_eq!(buf, [7]);
        assert_eq!(eddc.offsets, [1, 0]);
    }

    #[test]
    fn another_address_in_its_transaction_is_not_acknowledged() {
        let edid = [0u8; 128];
        let mut eddc = Eddc::new(&SinkConfig::new(), &edid, None);
        let result = run(
            &mut eddc,
            &mut [
                Operation::Write {
                    addr: EDID,
                    bytes: &[0x10],
                },
                Operation::Write {
                    addr: 0x54,
                    bytes: &[0x20],
                },
            ],
        );
        assert_eq!(result, Err(NACK_ADDRESS));
        assert_eq!(eddc.offsets[0], 0x10, "what came before stays");
    }

    #[test]
    fn a_missing_displayid_is_not_a_store() {
        let edid = [0u8; 128];
        let eddc = Eddc::new(&SinkConfig::new(), &edid, None);
        assert!(eddc.store(DISPLAYID).is_none());
        assert_eq!(eddc.displayid_len(), None);
        let empty = Eddc::new(&SinkConfig::new(), &edid, Some(&[]));
        assert!(empty.store(DISPLAYID).is_none());
        assert_eq!(empty.displayid_len(), Some(0));
    }
}
