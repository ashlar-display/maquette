//! The sink's settings.

/// The sink's settings.
///
/// [`new`](Self::new) is the default sink: an SCDC sink reporting `Sink_Version` 1, with an
/// E-DDC segment pointer, and the defaults of each setting. Each setting has a `const` `with_*` (or `without_*`) builder;
/// there are no named sink models, since a name would suggest a real product whose behaviour
/// no source backs. Where no public source settles what a sink does, the setting's default
/// is marked *chosen*.
///
/// ```
/// use maquette::{OwnedWrite, SinkConfig};
///
/// const STRICT: SinkConfig = SinkConfig::new()
///     .with_sink_version(1)
///     .with_owned_scdc_writes(OwnedWrite::Nack);
/// const NO_SCDC: SinkConfig = SinkConfig::new().without_scdc();
/// # let _ = (STRICT, NO_SCDC);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct SinkConfig {
    pub(crate) scdc: bool,
    pub(crate) sink_version: u8,
    pub(crate) owned_scdc_write: OwnedWrite,
    pub(crate) scdc_past_end: ScdcPastEnd,
    pub(crate) segment_pointer: bool,
    pub(crate) past_store: PastStore,
    pub(crate) missing_segment: MissingSegment,
    pub(crate) block_boundary: BlockBoundary,
}

impl SinkConfig {
    /// The default sink.
    pub const fn new() -> Self {
        Self {
            scdc: true,
            sink_version: 1,
            owned_scdc_write: OwnedWrite::Ignore,
            scdc_past_end: ScdcPastEnd::Wrap,
            segment_pointer: true,
            past_store: PastStore::Fill,
            missing_segment: MissingSegment::Nack,
            block_boundary: BlockBoundary::Continue,
        }
    }

    /// A sink without SCDC: nothing answers 0x54 (*chosen*: no source says how such a sink
    /// answers).
    pub const fn without_scdc(self) -> Self {
        Self {
            scdc: false,
            ..self
        }
    }

    /// The `Sink_Version` the sink reports; 1 by default.
    pub const fn with_sink_version(self, sink_version: u8) -> Self {
        Self {
            sink_version,
            ..self
        }
    }

    /// What a write to an SCDC register the sink owns does; by default
    /// [`OwnedWrite::Ignore`].
    pub const fn with_owned_scdc_writes(self, owned_scdc_write: OwnedWrite) -> Self {
        Self {
            owned_scdc_write,
            ..self
        }
    }

    /// What an SCDC read or write past 0xFF does; by default [`ScdcPastEnd::Wrap`].
    pub const fn with_scdc_past_end(self, scdc_past_end: ScdcPastEnd) -> Self {
        Self {
            scdc_past_end,
            ..self
        }
    }

    /// A sink without the E-DDC segment pointer: nothing answers 0x30, and the sink answers
    /// segment 0 reads (VESA E-DDC 1.3, 8.1.2), so a read of segment 1 or higher fails
    /// `NoAcknowledge(Address)` at the pointer.
    pub const fn without_segment_pointer(self) -> Self {
        Self {
            segment_pointer: false,
            ..self
        }
    }

    /// What a read past the end of the EDID or DisplayID store does, in a segment the sink
    /// has; by default [`PastStore::Fill`].
    pub const fn with_past_store(self, past_store: PastStore) -> Self {
        Self { past_store, ..self }
    }

    /// What a segment pointer value for a segment the sink does not have does; by default
    /// [`MissingSegment::Nack`].
    pub const fn with_missing_segment(self, missing_segment: MissingSegment) -> Self {
        Self {
            missing_segment,
            ..self
        }
    }

    /// What a read across a 128-byte block boundary at 0x50 or 0x52 does; by default
    /// [`BlockBoundary::Continue`].
    pub const fn with_block_boundary(self, block_boundary: BlockBoundary) -> Self {
        Self {
            block_boundary,
            ..self
        }
    }

    /// Whether the sink has SCDC.
    pub const fn has_scdc(&self) -> bool {
        self.scdc
    }

    /// The `Sink_Version` the sink reports.
    pub const fn sink_version(&self) -> u8 {
        self.sink_version
    }

    /// What a write to an SCDC register the sink owns does.
    pub const fn owned_scdc_writes(&self) -> OwnedWrite {
        self.owned_scdc_write
    }

    /// What an SCDC read or write past 0xFF does.
    pub const fn scdc_past_end(&self) -> ScdcPastEnd {
        self.scdc_past_end
    }

    /// Whether the sink has the E-DDC segment pointer.
    pub const fn has_segment_pointer(&self) -> bool {
        self.segment_pointer
    }

    /// What a read past the end of the EDID or DisplayID store does.
    pub const fn past_store(&self) -> PastStore {
        self.past_store
    }

    /// What a segment pointer value for a segment the sink does not have does.
    pub const fn missing_segment(&self) -> MissingSegment {
        self.missing_segment
    }

    /// What a read across a 128-byte block boundary does.
    pub const fn block_boundary(&self) -> BlockBoundary {
        self.block_boundary
    }
}

impl Default for SinkConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// What a write to an SCDC register the sink owns (`Sink_Version`, `Scrambling_Status`, the
/// status flags, the error counters, the manufacturer's block) does. No public source says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OwnedWrite {
    /// Acknowledged and ignored, recorded as an `IgnoredWrite` (*chosen* default).
    Ignore,
    /// Not acknowledged: `NoAcknowledge(Data)` at that byte, the bytes before it written.
    Nack,
}

/// What an SCDC read or write that runs past 0xFF in one burst does. hdmi-hal's
/// `ScdcTransport` leaves it to the sink, and no public source says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScdcPastEnd {
    /// The offset wraps to 0x00, as an 8-bit offset register does (*chosen* default).
    Wrap,
    /// The byte past 0xFF fails `NoAcknowledge(Data)`, the bytes before it done.
    Nack,
}

/// What a read past the end of the EDID or DisplayID store does, in a segment the sink has.
/// VESA E-DDC 1.3 covers only the data a display has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PastStore {
    /// Reads 0xFF (*chosen* default).
    Fill,
    /// Wraps to the start of the store.
    Wrap,
    /// The first byte past the store fails `NoAcknowledge(Data)`, the bytes before it read.
    Nack,
}

/// What the sink does with a segment pointer value for a segment it does not have. VESA
/// E-DDC 1.3 requires an acknowledge of the pointer for segments 1 and higher that exist
/// (6.4, 6.5), not what follows for others. A segment exists when the EDID or the DisplayID
/// store has bytes in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MissingSegment {
    /// `NoAcknowledge(Data)` at the pointer value (*chosen* default, as hdmi-hal's `DdcBus`
    /// contract reads E-DDC).
    Nack,
    /// Acknowledged; the read that follows reads past the store ([`PastStore`]).
    Ack,
}

/// What a read across a 128-byte block boundary at 0x50 or 0x52 does: "some implementations
/// may support" continuing (VESA E-DDC 1.3, 2.2.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BlockBoundary {
    /// Continues into the next block (*chosen* default).
    Continue,
    /// Wraps to the start of the same block.
    Wrap,
}
