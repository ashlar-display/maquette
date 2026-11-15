//! The sink's settings.

use core::time::Duration;

/// The sink's settings.
///
/// [`new`](Self::new) is the default sink: an SCDC sink reporting `Sink_Version` 1, with an
/// E-DDC segment pointer, and the defaults of each setting. Each setting has a `const`
/// `with_*` (or `without_*`) builder; there are no named sink models, since a name would
/// suggest a real product whose behaviour no source backs. Where no public source settles
/// what a sink does, the setting's default is marked *chosen*.
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
    pub(crate) scdc_during_hpd_low: DuringHpdLow,
    pub(crate) edid_during_hpd_low: DuringHpdLow,
    pub(crate) scrambling_lock: Trigger,
    pub(crate) strict_lock: bool,
    pub(crate) refusal: Refusal,
    pub(crate) enable_timeout: Option<Duration>,
    pub(crate) hpd_resets: [HpdReset; 4],
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
            scdc_during_hpd_low: DuringHpdLow::Answers,
            edid_during_hpd_low: DuringHpdLow::Answers,
            scrambling_lock: Trigger::After(Duration::from_millis(1)),
            strict_lock: false,
            refusal: Refusal::NONE,
            enable_timeout: Some(Duration::from_millis(100)),
            hpd_resets: [
                HpdReset::After(Duration::from_millis(100)),
                HpdReset::After(Duration::from_millis(100)),
                HpdReset::After(Duration::from_millis(100)),
                HpdReset::OnHpdLow,
            ],
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

    /// Whether SCDC answers while HPD is low; by default [`DuringHpdLow::Answers`]
    /// (*chosen*: no source says a sink's SCDC stops answering, and its registers are kept
    /// until a long HPD low resets them).
    pub const fn with_scdc_during_hpd_low(self, scdc_during_hpd_low: DuringHpdLow) -> Self {
        Self {
            scdc_during_hpd_low,
            ..self
        }
    }

    /// Whether the EDID, the DisplayID and the segment pointer answer while HPD is low; by
    /// default [`DuringHpdLow::Answers`] (*chosen*, after VESA E-DDC 1.3's rule that the EDID
    /// is readable with the display powered off, 9.2, 9.5.3).
    pub const fn with_edid_during_hpd_low(self, edid_during_hpd_low: DuringHpdLow) -> Self {
        Self {
            edid_during_hpd_low,
            ..self
        }
    }

    /// When the sink sets `Scrambling_Status` once its conditions hold; by default
    /// `Trigger::After` 1 ms (*chosen*: no source gives the delay).
    pub const fn with_scrambling_lock(self, scrambling_lock: Trigger) -> Self {
        Self {
            scrambling_lock,
            ..self
        }
    }

    /// A strict sink: the output must start after the write that set `Scrambling_Enable` or
    /// changed `TMDS_Bit_Clock_Ratio`, and at least 1 ms after the last change of the ratio
    /// (the source suspends the TMDS clock and data, writes the bit, and resumes 1 to 100 ms
    /// after). Output already running at the write, or resumed sooner, is not taken until it
    /// stops and starts again. Off by default: sources that redo the whole mode set say some
    /// sinks would accept the SCDC writes alone, and others restore with the output running.
    pub const fn with_strict_lock(self, strict_lock: bool) -> Self {
        Self {
            strict_lock,
            ..self
        }
    }

    /// A sink that refuses to set the scrambling bit right away, for the first attempts; by
    /// default [`Refusal::NONE`] (*chosen*).
    pub const fn with_scrambling_refusals(self, refusal: Refusal) -> Self {
        Self { refusal, ..self }
    }

    /// A sink that clears `Scrambling_Enable` itself when no scrambled TMDS at the matching
    /// ratio arrives within this time of the write that set it, or of a write that changed
    /// `TMDS_Bit_Clock_Ratio` while it was set; `None` never gives up. 100 ms by default:
    /// i915 notes that a sink may disable scrambling without a scrambled clock within 100 ms,
    /// and a source resumes within 100 ms of a ratio change (the least forgiving default).
    pub const fn with_enable_timeout(self, enable_timeout: Option<Duration>) -> Self {
        Self {
            enable_timeout,
            ..self
        }
    }

    /// When `group`'s registers reset to 0 while HPD is low. By default (*chosen*: sources
    /// restore `TMDS_Config` after a reconnect, but no public source says when a sink
    /// resets it) after 100 ms of HPD low for `TMDS_Config`, the status and update flags and
    /// `Source_Version`, and on every HPD low for `Config_0` and `Config_1`.
    pub const fn with_hpd_reset(mut self, group: RegisterGroup, reset: HpdReset) -> Self {
        self.hpd_resets[group.index()] = reset;
        self
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

    /// When the sink sets `Scrambling_Status` once its conditions hold.
    pub const fn scrambling_lock(&self) -> Trigger {
        self.scrambling_lock
    }

    /// Whether the sink is strict about scrambled output arriving before the write.
    pub const fn strict_lock(&self) -> bool {
        self.strict_lock
    }

    /// How the sink refuses to set the scrambling bit right away.
    pub const fn scrambling_refusals(&self) -> Refusal {
        self.refusal
    }

    /// How long the sink waits for scrambled TMDS before clearing `Scrambling_Enable`.
    pub const fn enable_timeout(&self) -> Option<Duration> {
        self.enable_timeout
    }

    /// When `group`'s registers reset while HPD is low.
    pub const fn hpd_reset(&self, group: RegisterGroup) -> HpdReset {
        self.hpd_resets[group.index()]
    }

    /// Whether SCDC answers while HPD is low.
    pub const fn scdc_during_hpd_low(&self) -> DuringHpdLow {
        self.scdc_during_hpd_low
    }

    /// Whether the EDID, the DisplayID and the segment pointer answer while HPD is low.
    pub const fn edid_during_hpd_low(&self) -> DuringHpdLow {
        self.edid_during_hpd_low
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

/// Whether a built-in target answers while HPD is low. No public source covers HPD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DuringHpdLow {
    /// It answers as with HPD high (*chosen* default).
    Answers,
    /// Nothing answers its addresses: `NoAcknowledge(Address)`.
    Silent,
}

/// When a sink's behaviour happens once its conditions hold: after a time, or on a count of
/// operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Trigger {
    /// Once this long has passed on the bench's clock.
    After(Duration),
    /// On the source's `n`-th read of the register that shows it (0 and 1: the first).
    AfterReads(u32),
}

/// "Some displays refuse to set the scrambling bit right away": a sink that does, for its
/// first `attempts` attempts, an attempt being a write of `TMDS_Config` with
/// `Scrambling_Enable` set. One source, which reads two ways: the two kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Refusal {
    /// How the sink refuses.
    pub kind: RefusalKind,
    /// How many attempts it refuses; 0 for none.
    pub attempts: u32,
}

impl Refusal {
    /// No refusals.
    pub const NONE: Self = Self {
        kind: RefusalKind::WithholdStatus,
        attempts: 0,
    };

    /// Refuses the first `attempts` attempts, by `kind`.
    pub const fn new(kind: RefusalKind, attempts: u32) -> Self {
        Self { kind, attempts }
    }
}

/// How a sink refuses an attempt to enable scrambling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RefusalKind {
    /// The write is stored, but the sink does not lock until a later attempt.
    WithholdStatus,
    /// The write is stored without `Scrambling_Enable`.
    DropEnable,
}

/// A group of SCDC registers that a long HPD low resets together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RegisterGroup {
    /// `TMDS_Config` (0x20).
    TmdsConfig,
    /// The sink's status: `Scrambling_Status` (0x21), the update flags (0x10, 0x11) and the
    /// status flags (0x40–0x42).
    Status,
    /// `Source_Version` (0x02).
    SourceVersion,
    /// `Config_0` (0x30) and `Config_1` (0x31).
    Config,
}

impl RegisterGroup {
    /// Every group.
    pub const ALL: [Self; 4] = [
        Self::TmdsConfig,
        Self::Status,
        Self::SourceVersion,
        Self::Config,
    ];

    pub(crate) const fn index(self) -> usize {
        match self {
            Self::TmdsConfig => 0,
            Self::Status => 1,
            Self::SourceVersion => 2,
            Self::Config => 3,
        }
    }

    /// The group's registers.
    pub const fn registers(self) -> &'static [u8] {
        match self {
            Self::TmdsConfig => &[0x20],
            Self::Status => &[0x21, 0x10, 0x11, 0x40, 0x41, 0x42],
            Self::SourceVersion => &[0x02],
            Self::Config => &[0x30, 0x31],
        }
    }
}

/// When a group of registers resets to 0 while HPD is low.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum HpdReset {
    /// Once HPD has been low this long: at the first bench event at or after that moment
    /// while HPD is still low, or when HPD rises if it was low that long.
    After(Duration),
    /// As HPD goes low.
    OnHpdLow,
    /// Never: the registers survive any HPD low.
    Never,
}
