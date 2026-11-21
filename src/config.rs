//! The sink's settings.

use core::time::Duration;

use hdmi_hal::HdmiForumFrl;
use hdmi_hal::phy::{LanePatterns, LtpPattern};

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
    pub(crate) hpd_resets: [HpdReset; RegisterGroup::COUNT],
    pub(crate) status_update: StatusUpdate,
    pub(crate) ced_update: CedUpdate,
    pub(crate) ced_read: CedRead,
    pub(crate) during_read: DuringRead,
    pub(crate) max_frl_rate: Option<HdmiForumFrl>,
    pub(crate) ltp_requests: LanePatterns,
    pub(crate) lts3_timeout: Option<Duration>,
    pub(crate) frl_pass: FrlPass,
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
            hpd_resets: [HpdReset::OnHpdLow; RegisterGroup::COUNT],
            status_update: StatusUpdate::OnChange,
            ced_update: CedUpdate::EverySecond { rise: 4 },
            ced_read: CedRead::Keep,
            during_read: DuringRead::Moving,
            max_frl_rate: None,
            ltp_requests: LanePatterns {
                lane0: Some(LtpPattern::Lfsr0),
                lane1: Some(LtpPattern::Lfsr1),
                lane2: Some(LtpPattern::Lfsr2),
                lane3: Some(LtpPattern::Lfsr3),
            },
            lts3_timeout: Some(Duration::from_millis(180)),
            frl_pass: FrlPass::Patterns,
        }
    }

    /// A sink without SCDC: nothing answers 0x54 (*chosen* on one source: Xilinx takes a
    /// failed `Source_Version` write for a sink without SCDC, embeddedsw `97f2baf7f69e`,
    /// `XilinxProcessorIPLib/drivers/v_hdmitx1/src/xv_hdmitx1.c:789–807`).
    #[must_use]
    pub const fn without_scdc(self) -> Self {
        Self {
            scdc: false,
            ..self
        }
    }

    /// The `Sink_Version` the sink reports; 1 by default.
    #[must_use]
    pub const fn with_sink_version(self, sink_version: u8) -> Self {
        Self {
            sink_version,
            ..self
        }
    }

    /// What a write to an SCDC register the sink owns does; by default
    /// [`OwnedWrite::Ignore`].
    #[must_use]
    pub const fn with_owned_scdc_writes(self, owned_scdc_write: OwnedWrite) -> Self {
        Self {
            owned_scdc_write,
            ..self
        }
    }

    /// What an SCDC read or write past 0xFF does; by default [`ScdcPastEnd::Wrap`].
    #[must_use]
    pub const fn with_scdc_past_end(self, scdc_past_end: ScdcPastEnd) -> Self {
        Self {
            scdc_past_end,
            ..self
        }
    }

    /// A sink without the E-DDC segment pointer: nothing answers 0x30, and the sink answers
    /// segment 0 reads (VESA E-DDC 1.3, 8.1.2), so a read of segment 1 or higher fails
    /// `NoAcknowledge(Address)` at the pointer.
    #[must_use]
    pub const fn without_segment_pointer(self) -> Self {
        Self {
            segment_pointer: false,
            ..self
        }
    }

    /// What a read past the end of the EDID or DisplayID store does, in a segment the sink
    /// has; by default [`PastStore::Fill`].
    #[must_use]
    pub const fn with_past_store(self, past_store: PastStore) -> Self {
        Self { past_store, ..self }
    }

    /// What a segment pointer value for a segment the sink does not have does; by default
    /// [`MissingSegment::Nack`].
    #[must_use]
    pub const fn with_missing_segment(self, missing_segment: MissingSegment) -> Self {
        Self {
            missing_segment,
            ..self
        }
    }

    /// What a read across a 128-byte block boundary at 0x50 or 0x52 does; by default
    /// [`BlockBoundary::Continue`].
    #[must_use]
    pub const fn with_block_boundary(self, block_boundary: BlockBoundary) -> Self {
        Self {
            block_boundary,
            ..self
        }
    }

    /// Whether SCDC answers while HPD is low; by default [`DuringHpdLow::Answers`]
    /// (*chosen*: no source says a sink's SCDC stops answering).
    #[must_use]
    pub const fn with_scdc_during_hpd_low(self, scdc_during_hpd_low: DuringHpdLow) -> Self {
        Self {
            scdc_during_hpd_low,
            ..self
        }
    }

    /// Whether the EDID, the DisplayID and the segment pointer answer while HPD is low; by
    /// default [`DuringHpdLow::Answers`]: VESA E-DDC 1.3 makes the EDID available whenever
    /// +5 V is present (§4.2.2), and readable with the display powered off (§9.2, §9.5.3).
    /// [`DuringHpdLow::Silent`] is a sink whose EDID is unavailable while HPD is low (DVI
    /// 1.0 has HPD high only while the EDID can be read, §2.2.9.2).
    #[must_use]
    pub const fn with_edid_during_hpd_low(self, edid_during_hpd_low: DuringHpdLow) -> Self {
        Self {
            edid_during_hpd_low,
            ..self
        }
    }

    /// When the sink sets `Scrambling_Status`, and the lane locks in `Status_Flags_0`, once
    /// their conditions hold; by default `Trigger::After` 1 ms (*chosen*: no source gives the
    /// delay). `Trigger::AfterReads` counts the reads of the register that shows each.
    #[must_use]
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
    #[must_use]
    pub const fn with_strict_lock(self, strict_lock: bool) -> Self {
        Self {
            strict_lock,
            ..self
        }
    }

    /// A sink that refuses to set the scrambling bit right away, for the first attempts; by
    /// default [`Refusal::NONE`] (*chosen*).
    #[must_use]
    pub const fn with_scrambling_refusals(self, refusal: Refusal) -> Self {
        Self { refusal, ..self }
    }

    /// A sink that clears `Scrambling_Enable` itself when no scrambled TMDS at the matching
    /// ratio arrives within this time of the write that set it, or of a write that changed
    /// `TMDS_Bit_Clock_Ratio` while it was set; `None` never gives up. 100 ms by default:
    /// i915 notes that a sink may disable scrambling without a scrambled clock within 100 ms,
    /// and a source resumes within 100 ms of a ratio change (the least forgiving default).
    #[must_use]
    pub const fn with_enable_timeout(self, enable_timeout: Option<Duration>) -> Self {
        Self {
            enable_timeout,
            ..self
        }
    }

    /// When `group`'s registers reset to 0 around an HPD low. By default every group resets
    /// as HPD goes low, the least forgiving rule: sources restore `TMDS_Config` after a
    /// reconnect, and the public sinks reset it as HPD falls. Xilinx's receiver clears
    /// `TMDS_Config` bits 1:0 as it lowers HPD and `FRL_Rate` as it raises it, and Amlogic's
    /// clears its whole SCDC with HPD low (on T7 and later chips); for the status, the
    /// update flags and `Source_Version`, only Amlogic's says (*chosen* after it). A sink
    /// whose 5 V goes away loses everything: [`Bench::unplug`](crate::Bench::unplug).
    #[must_use]
    pub const fn with_hpd_reset(mut self, group: RegisterGroup, reset: HpdReset) -> Self {
        self.hpd_resets[group.index()] = reset;
        self
    }

    /// Whether the sink sets `Status_Update` when it changes the status flags; by default
    /// [`StatusUpdate::OnChange`] (*chosen*).
    #[must_use]
    pub const fn with_status_update(self, status_update: StatusUpdate) -> Self {
        Self {
            status_update,
            ..self
        }
    }

    /// When the sink sets `CED_Update`; by default [`CedUpdate::EverySecond`] with a rise of
    /// 4 (*chosen*, after one receiver).
    #[must_use]
    pub const fn with_ced_update(self, ced_update: CedUpdate) -> Self {
        Self { ced_update, ..self }
    }

    /// Whether a read of a character error counter clears it; by default [`CedRead::Keep`]
    /// (*chosen*).
    #[must_use]
    pub const fn with_ced_read(self, ced_read: CedRead) -> Self {
        Self { ced_read, ..self }
    }

    /// Whether the registers that change by themselves hold still during a read; by default
    /// [`DuringRead::Moving`] (*chosen*: the least forgiving, as only it can tear).
    #[must_use]
    pub const fn with_during_read(self, during_read: DuringRead) -> Self {
        Self {
            during_read,
            ..self
        }
    }

    /// A sink that takes FRL, up to `max_frl_rate`; `None` (the default), or
    /// `HdmiForumFrl::NotSupported`, for a TMDS-only sink. An FRL sink sets `FLT_ready` and
    /// answers link training on a write of `FRL_Rate`, as the public receivers do; a rate
    /// above its maximum it answers with a request for a lower rate on every lane (*chosen*,
    /// after one receiver: Xilinx embeddedsw `97f2baf7f69e`,
    /// `XilinxProcessorIPLib/drivers/v_hdmirx1/src/xv_hdmirx1_frl.c:1122–1126`, `775–782`).
    #[must_use]
    pub const fn with_max_frl_rate(self, max_frl_rate: Option<HdmiForumFrl>) -> Self {
        Self {
            max_frl_rate,
            ..self
        }
    }

    /// The link training patterns an FRL sink first requests on each lane; `None` posts 0,
    /// no pattern. By default LFSR 0–3 on lanes 0–3, as Amlogic's receiver
    /// (github.com/CoreELEC/common_drivers `8ca9b6c3b715`,
    /// `drivers/media/vin/tvin/hdmirx/hdmi_rx_hw_t3x.c:4775–4776`); Xilinx's takes any LFSR
    /// per lane (`xv_hdmirx1_frl.c:147–160`).
    #[must_use]
    pub const fn with_ltp_requests(self, ltp_requests: LanePatterns) -> Self {
        Self {
            ltp_requests,
            ..self
        }
    }

    /// How long an FRL sink gives the lanes at a rate before it asks for a lower one; `None`
    /// never. By default 180 ms (*chosen*, after one receiver: Xilinx embeddedsw
    /// `97f2baf7f69e`, `XilinxProcessorIPLib/drivers/v_hdmirx1/src/xv_hdmirx1_frl.c:49–58`,
    /// `440–547`). The time is split into `FFE_Levels` + 1 equal attempts (180, 90, 60 or
    /// 45 ms, as Xilinx's table): at the end of each but the last, the lanes not passing are
    /// asked for another TxFFE level (0xE), and the patterns requested again once the source
    /// clears `FLT_update`; at the end of the last, every lane is asked for a lower rate
    /// (0xF). `FLT_no_timeout` (0x35 bit 5) or `FLT_no_retrain` (`Config_0` bit 1) stops
    /// them.
    #[must_use]
    pub const fn with_lts3_timeout(self, lts3_timeout: Option<Duration>) -> Self {
        Self {
            lts3_timeout,
            ..self
        }
    }

    /// What an FRL sink checks before it passes link training and sets `FRL_start`; by
    /// default [`FrlPass::Patterns`] (*chosen*: the two public receivers differ, and it is
    /// the least forgiving).
    #[must_use]
    pub const fn with_frl_pass(self, frl_pass: FrlPass) -> Self {
        Self { frl_pass, ..self }
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

    /// Whether the sink sets `Status_Update` when it changes the status flags.
    pub const fn status_update(&self) -> StatusUpdate {
        self.status_update
    }

    /// When the sink sets `CED_Update`.
    pub const fn ced_update(&self) -> CedUpdate {
        self.ced_update
    }

    /// Whether a read of a character error counter clears it.
    pub const fn ced_read(&self) -> CedRead {
        self.ced_read
    }

    /// Whether the registers that change by themselves hold still during a read.
    pub const fn during_read(&self) -> DuringRead {
        self.during_read
    }

    /// The highest FRL rate the sink takes; `None` for a TMDS-only sink.
    pub const fn max_frl_rate(&self) -> Option<HdmiForumFrl> {
        self.max_frl_rate
    }

    /// The link training patterns an FRL sink first requests.
    pub const fn ltp_requests(&self) -> LanePatterns {
        self.ltp_requests
    }

    /// How long an FRL sink gives the lanes at a rate.
    pub const fn lts3_timeout(&self) -> Option<Duration> {
        self.lts3_timeout
    }

    /// What an FRL sink checks before it passes link training and sets `FRL_start`.
    pub const fn frl_pass(&self) -> FrlPass {
        self.frl_pass
    }
}

impl Default for SinkConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// What a write to an SCDC register the sink owns (`Sink_Version`, `Scrambling_Status`,
/// `Source_Test_Configuration`, the status flags, the error counters, the manufacturer's
/// identification at 0xD0–0xDD) does. No public source says.
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
    /// A write's byte past 0xFF fails `NoAcknowledge(Data)`, the bytes before it done; a
    /// read wraps, since the sink cannot refuse a byte it sends (DDC/CI 1.1 §1.6).
    Nack,
}

/// What a read past the end of the EDID or DisplayID store does, in a segment the sink has.
/// VESA E-DDC 1.3 covers only the data a display has. A read goes on with data: the host
/// acknowledges each byte it reads, so the display cannot refuse one (DDC/CI 1.1 §1.6: the
/// acknowledge bit goes the other way to the data).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PastStore {
    /// Reads 0xFF (*chosen* default).
    Fill,
    /// Wraps to the start of the store.
    Wrap,
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

/// Whether a built-in target answers while HPD is low. For the EDID VESA E-DDC 1.3 §4.2.2
/// settles it; for SCDC no public source says.
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

impl Default for Refusal {
    /// [`Refusal::NONE`].
    fn default() -> Self {
        Self::NONE
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

/// Whether the sink sets `Status_Update` (`Update_0` bit 0) when it changes `Clock_Detected`
/// or a lane lock in `Status_Flags_0`. The public receivers set it in their SCDC hardware,
/// and none shows when; one source family reads the flags only when it is set and clears it
/// by writing it back (Amlogic, github.com/CoreELEC/common_drivers `8ca9b6c3b715`:
/// `drivers/media/vout/hdmitx20/hdmi_tx_scdc.c:88–112`), others read them regardless (Linux,
/// github.com/torvalds/linux `e9954f3fbddd`: `drivers/gpu/drm/display/drm_scdc_helper.c:440–478`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StatusUpdate {
    /// Set at every such change, kept until the source clears it (*chosen* default).
    OnChange,
    /// Never set: a source that reads the flags only when it is set sees no change.
    Never,
}

/// When the sink sets `CED_Update` (`Update_0` bit 1), and `RSED_Update` (bit 6) for the
/// Reed-Solomon corrections, which it keeps until the source clears them. One public
/// receiver sets both from a check once a second: when a valid count has risen by at least
/// 4 since the last check, or has just reached 0x7FFF (Xilinx embeddedsw `97f2baf7f69e`,
/// `XilinxProcessorIPLib/drivers/v_hdmirx1/src/xv_hdmirx1.c:2303–2345`, `2410–2436`);
/// Intel documents a receiver that did not set it on errors (knowledge base 000087546).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CedUpdate {
    /// Checked at each whole second on the bench's clock (*chosen*): set when the count of
    /// a lane locked since the last check has risen by at least `rise` since then, or has
    /// just reached 0x7FFF. The default, with a `rise` of 4.
    EverySecond {
        /// The least rise in a second that sets it; 0 is taken as 1.
        rise: u16,
    },
    /// Never set: a source that reads the counters only when it is set sees no errors.
    Never,
}

/// Whether reading a character error counter clears it. Unresolved in public sources: one
/// source's comments say reading clears the counts at the sink (Xilinx embeddedsw
/// `97f2baf7f69e`, `XilinxProcessorIPLib/drivers/v_hdmitx1/src/xv_hdmitx1.c:2979`), while
/// the same vendor's receiver keeps them, and Linux reads the whole map for debugging with
/// no word of losing them (github.com/torvalds/linux `e9954f3fbddd`,
/// `drivers/gpu/drm/display/drm_scdc_helper.c:440–478`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CedRead {
    /// The counts are kept (*chosen* default).
    Keep,
    /// A read of a lane's high byte, the second of its two, clears the lane's count, after
    /// the byte is sent (*chosen*: the byte a source reads last); the next `CED_Update`
    /// check measures its rise from 0.
    ClearOnRead,
}

/// Whether the registers that change by themselves (the character error counters) hold
/// still while a read is under way. No public source says whether a sink holds them; the
/// counters' checksum is the source's protection against a torn read, whatever the
/// transport does. On a link without errors nothing moves, so the default tears only where
/// a test sets [`ErrorRates`](crate::ErrorRates).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DuringRead {
    /// A read returns them as they were when the transaction started; what the read
    /// changes (a clear on read) shows from the next bench event.
    Held,
    /// Each byte is taken at its own moment, the transaction's start plus the bench's time
    /// per byte for the bytes before it on the wire (its time per transaction comes after
    /// them): a burst can see a count rise, or a clear, between a lane's bytes or before the
    /// checksum, and read a torn value. A `CED_Update` check whose second falls within the
    /// read sees the counts as they were at that second, and its flag is set at the next
    /// bench event, as a check between events is. The default (*chosen*).
    Moving,
}

/// What an FRL sink checks before it passes link training and sets `FRL_start`. The two
/// public receivers differ: Xilinx's checks the patterns on every lane, waits for the
/// source to clear `FLT_update`, and sets `FRL_start` once the lanes lock (embeddedsw
/// `97f2baf7f69e`, `XilinxProcessorIPLib/drivers/v_hdmirx1/src/xv_hdmirx1_frl.c:398–412`,
/// `588–596`, `616–640`); Amlogic's posts the pass whatever the lanes carry (its BIST error
/// counts are only logged), gives up waiting for the clear after 100 ms, and sets
/// `FRL_start` without the lanes in sync (github.com/CoreELEC/common_drivers
/// `8ca9b6c3b715`, `drivers/media/vin/tvin/hdmirx/hdmi_rx_hw_t3x.c:4454–4576`, `4770–4800`,
/// `hdmi_rx_hw.c:864–879`, `hdmi_rx_wrapper.c:6711–6719`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FrlPass {
    /// The pass once the source has cleared `FLT_update` and each active lane has carried
    /// its requested pattern, within the link's lane limits, for the lock trigger;
    /// `FRL_start` once the lanes lock and the source has cleared the pass's `FLT_update`;
    /// the LTS:3 timeouts for lanes that do not pass. After Xilinx (*chosen* default).
    Patterns,
    /// The pass once the source has cleared `FLT_update`, or 100 ms after it was set, and
    /// the lock trigger has passed, whatever the lanes carry; `FRL_start` once the source
    /// has cleared the pass's `FLT_update`, or 100 ms after it, whatever the line carries;
    /// no LTS:3 timeouts. After Amlogic. A rate above the sink's maximum is still answered
    /// with a request for a lower one.
    Unchecked,
}

/// A group of SCDC registers that an HPD low resets together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    /// The character error counters, their checksum and the Reed-Solomon corrections
    /// (0x50–0x5A). A receiver resets them on HPD low to high or power-off (LG,
    /// EP 3 160 152 A1, paragraph 0064), or with all of SCDC (Xilinx embeddedsw `97f2baf7f69e`,
    /// `XilinxProcessorIPLib/drivers/v_hdmirx1/src/xv_hdmirx1_intr.c:1181–1182`).
    ErrorCounters,
}

impl RegisterGroup {
    /// Every group, in the order the sink resets them. A slice, so a group added later
    /// changes no type.
    pub const ALL: &'static [Self] = &[
        Self::TmdsConfig,
        Self::Status,
        Self::SourceVersion,
        Self::Config,
        Self::ErrorCounters,
    ];

    /// How many groups there are.
    pub(crate) const COUNT: usize = 5;

    pub(crate) const fn index(self) -> usize {
        match self {
            Self::TmdsConfig => 0,
            Self::Status => 1,
            Self::SourceVersion => 2,
            Self::Config => 3,
            Self::ErrorCounters => 4,
        }
    }

    /// The group's registers.
    pub const fn registers(self) -> &'static [u8] {
        match self {
            Self::TmdsConfig => &[0x20],
            Self::Status => &[0x21, 0x10, 0x11, 0x40, 0x41, 0x42],
            Self::SourceVersion => &[0x02],
            Self::Config => &[0x30, 0x31],
            Self::ErrorCounters => &[
                0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A,
            ],
        }
    }
}

/// When a group of registers resets to 0 around an HPD low.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum HpdReset {
    /// Once HPD has been low this long: at the first bench event at or after that moment
    /// while HPD is still low, or when HPD rises if it was low that long.
    After(Duration),
    /// As HPD goes low.
    OnHpdLow,
    /// As HPD rises: what was written while it was low is lost too, as Xilinx's receiver
    /// does with `FRL_Rate`.
    OnHpdRise,
    /// Never: the registers survive any HPD low.
    Never,
}
