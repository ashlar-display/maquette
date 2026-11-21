//! The built-in SCDC target: the sink's 256-byte SCDC register file at 0x54.
//!
//! Register addresses, bits and who writes each register, from public implementations of HDMI
//! sources (no maquette code shares them with the crates it tests):
//!
//! - Linux, github.com/torvalds/linux `e9954f3fbddd`: `include/drm/display/drm_scdc.h:27–85`
//!   (addresses and bits);
//! - amdgpu, the same tree: `drivers/gpu/drm/amd/display/dc/dc_hdmi_types.h:58–65` (address 0x54
//!   and the registers), `dc/link/protocols/link_ddc.c:602–609` (`TMDS_Config` 3 and 1);
//! - Xilinx embeddedsw `97f2baf7f69e`:
//!   `XilinxProcessorIPLib/drivers/v_hdmitx1/src/xv_hdmitx1.c:41–49` (the source's fields),
//!   `645–669`, `719–743` (`TMDS_Config` bits 0 and 1), `790` (`Source_Version`), `870`
//!   (status at 0x21);
//! - Amlogic, github.com/CoreELEC/common_drivers `8ca9b6c3b715`:
//!   `drivers/media/vout/hdmitx21/hw/hdmi_tx_ddc.h:16–56` (address 0xA8, the 8-bit form of
//!   0x54; the registers, the bits, the registers the source writes marked `RW`);
//!   `hdmitx21/hdmi_tx_scdc.c:106–107` (update flags cleared by writing them back).

use core::time::Duration;

use hdmi_hal::ddc::{DdcErrorKind, NoAcknowledgeSource};
use hdmi_hal::phy::TmdsClockRatio;
use hdmi_hal::time::Timestamp;

use crate::config::{
    CedRead, CedUpdate, DuringHpdLow, DuringRead, FrlPass, HpdReset, OwnedWrite, Refusal,
    RefusalKind, RegisterGroup, ScdcPastEnd, SinkConfig, StatusUpdate, Trigger,
};
use crate::hook::{HookWrite, ScdcHook, ScdcRegisters};
use crate::line::{ErrorRates, Line, LinkLanes, RATIO_40_ABOVE_KHZ};
use crate::target::{Env, Operation, Target};
use crate::transcript::{Actor, Event, Ignored};
use hdmi_hal::HdmiForumFrl;
use hdmi_hal::phy::{EqParams, LanePatterns, LtpPattern, TxFfeLevel};

/// `TMDS_Config` bit 0.
const SCRAMBLING_ENABLE: u8 = 1 << 0;
/// `TMDS_Config` bit 1: the ratio is 1/40.
const RATIO_40: u8 = 1 << 1;
/// `Scrambling_Status` bit 0.
const STATUS: u8 = 1 << 0;
/// `Update_0` bit 0: `Status_Update`.
const STATUS_UPDATE: u8 = 1 << 0;
/// `Update_0` bit 1: `CED_Update`.
const CED_UPDATE: u8 = 1 << 1;
/// How often the sink checks its counters for `CED_Update`.
const CED_CHECK: Duration = Duration::from_secs(1);
/// `Status_Flags_0` bit 0: `Clock_Detected`.
const CLOCK_DETECTED: u8 = 1 << 0;
/// `Status_Flags_0` bits 1–3: lanes 0–2 locked.
const LANES_LOCKED: u8 = 0b1110;
/// `Status_Flags_0` bits 1–4: lanes 0–3 locked, in 4-lane FRL.
const FOUR_LANES_LOCKED: u8 = 0b1_1110;
/// `Status_Flags_0` bit 6: `FLT_ready`.
const FLT_READY: u8 = 1 << 6;
/// `Update_0` bit 4: `FRL_start`.
const FRL_START: u8 = 1 << 4;
/// `Update_0` bit 5: `FLT_update`.
const FLT_UPDATE: u8 = 1 << 5;
/// The LTP requests: lanes 0 and 1 in `Status_Flags_1`, lanes 2 and 3 in `Status_Flags_2`,
/// the lower lane in bits 3:0.
const LTP_REQ_01: u8 = 0x41;
const LTP_REQ_23: u8 = 0x42;
/// The request for a lower FRL rate.
const LTP_RATE_CHANGE: u8 = 0xF;
/// The request for another TxFFE level.
const LTP_FFE_CHANGE: u8 = 0xE;
/// How long a sink with [`FrlPass::Unchecked`] waits for the source to clear `FLT_update`
/// before it goes on: Amlogic's `HZ / 10` (`hdmi_rx_hw.c:864–879`).
const FLT_CLEAR_WAIT: Duration = Duration::from_millis(100);
/// The most `FFE_Levels` (`Config_1` bits 7:4) the LTS:3 timeouts count.
const FFE_LEVELS_MAX: u8 = 3;
/// `Source_Test_Configuration` bit 5: `FLT_no_timeout`.
const FLT_NO_TIMEOUT: u8 = 1 << 5;
/// `Config_0` bit 1: `FLT_no_retrain`.
const FLT_NO_RETRAIN: u8 = 1 << 1;
/// The highest `FRL_Rate` value that names a rate (12 Gbps, 4 lanes).
const FRL_RATE_MAX: u8 = 6;
/// The highest `FRL_Rate` value at 3 lanes (6 Gbps, 3 lanes).
const FRL_3_LANES_MAX: u8 = 2;

/// The least time from a write that changes `TMDS_Bit_Clock_Ratio` to the TMDS clock and
/// data resuming: the source suspends them, writes the bit, and resumes 1 to 100 ms after.
/// Linux, github.com/torvalds/linux `e9954f3fbddd`:
/// `drivers/gpu/drm/display/drm_scdc_helper.c:300–305` (the SCDC helper's wait) and
/// `drivers/gpu/drm/bridge/synopsys/dw-hdmi.c:1407–1419` (dw-hdmi's procedure); Amlogic,
/// github.com/CoreELEC/common_drivers `8ca9b6c3b715`:
/// `drivers/media/vout/hdmitx21/hdmi_tx_main.c:510–517`.
const RATIO_RESUME_MIN: Duration = Duration::from_millis(1);

/// The SCDC target's address.
pub(crate) const SCDC: u8 = 0x54;

/// `Sink_Version`: the sink's, read by the source.
pub(crate) const SINK_VERSION: u8 = 0x01;
/// `Source_Version`: written by the source.
pub(crate) const SOURCE_VERSION: u8 = 0x02;
/// `Update_0`: flags the sink sets and the source clears by writing them back as 1.
pub(crate) const UPDATE_0: u8 = 0x10;
/// `Update_1`: as `Update_0`.
pub(crate) const UPDATE_1: u8 = 0x11;
/// `TMDS_Config`: written by the source; bit 0 `Scrambling_Enable`, bit 1
/// `TMDS_Bit_Clock_Ratio`.
pub(crate) const TMDS_CONFIG: u8 = 0x20;
/// `Scrambling_Status`: the sink's; bit 0, scrambled TMDS detected.
pub(crate) const SCRAMBLING_STATUS: u8 = 0x21;
/// `Config_0`: written by the source.
pub(crate) const CONFIG_0: u8 = 0x30;
/// `Config_1`: written by the source; `FRL_Rate` in bits 3:0, `FFE_Levels` in 7:4.
pub(crate) const CONFIG_1: u8 = 0x31;
/// `Source_Test_Configuration`: the sink's test requests to the source, read by the source.
const SOURCE_TEST_CONFIGURATION: u8 = 0x35;
/// The first character error counter, lane 0's low byte; each lane's is two bytes, low
/// then high, lanes 0–2 at 0x50–0x55, and the checksum at 0x56.
const CED: u8 = 0x50;
/// The checksum of the counters of lanes 0–2, and of lane 3's at 4 lanes.
const CED_CHECKSUM: u8 = 0x56;
/// The last register of the counters: the Reed-Solomon corrections' high byte.
const CED_LAST: u8 = 0x5A;
/// Each counter's low byte, high byte following: lanes 0–3, then the Reed-Solomon
/// corrections.
const COUNTERS: [u8; 5] = [0x50, 0x52, 0x54, 0x57, 0x59];
/// The Reed-Solomon corrections' index in [`COUNTERS`].
const RS: usize = 4;
/// `Update_0` bit 6: `RSED_Update`.
const RSED_UPDATE: u8 = 1 << 6;
/// A counter's high byte, bit 7: the count is valid.
const CED_VALID: u8 = 1 << 7;
/// A counter's largest count, where it stays.
const CED_MAX: u64 = 0x7FFF;
/// A count in the unit the sink keeps it: millionths of an error, so that a rate in errors
/// a second times microseconds adds up exactly.
const CED_UNIT: u64 = 1_000_000;

/// `Status_Flags_0`: the sink's; bit 0 `Clock_Detected`, bits 1–4 lanes 0–3 locked, bit 6
/// `FLT_ready`. The bits: Linux, github.com/torvalds/linux `e9954f3fbddd`:
/// `include/drm/display/drm_scdc.h:55–64`; Amlogic's receiver,
/// github.com/CoreELEC/common_drivers `8ca9b6c3b715`:
/// `drivers/media/vin/tvin/hdmirx/hdmi_rx_hw_t3x.c:4647`, which takes bits 0–3 all set as
/// TMDS locked (`4736–4744`).
pub(crate) const STATUS_FLAGS_0: u8 = 0x40;

/// What a write to a register does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// The source writes it: stored as written.
    Source,
    /// Flags the sink sets: a 1 written clears that bit.
    WriteOneToClear,
    /// The sink's: the source only reads it.
    Sink,
}

/// The role of `reg`. The sink's: `Sink_Version`, `Scrambling_Status`,
/// `Source_Test_Configuration` (0x35), the status flags (0x40–0x42), the error counters
/// (0x50–0x5A) and the manufacturer's identification (0xD0–0xDD: the IEEE OUI, the device
/// ID string, the revisions), which Amlogic does not mark `RW` and which sources only read.
/// 0x35 the sources read under `Source_Test_Update` (amdgpu
/// `dc/link/protocols/link_hdmi_frl.c:58`, `98`, `220`, `310`; Xilinx
/// `v_hdmitx1/src/xv_hdmitx1_frl.c:360`, `538`; Amlogic `hdmitx21/hdmi_tx_scdc.c:198–200`).
/// The rest of the manufacturer's block, 0xDE–0xFF, Amlogic marks `RW`
/// (`hw/hdmi_tx_ddc.h:93`): stored as written, as is a register outside the modelled set
/// (*chosen*).
fn role(reg: u8) -> Role {
    match reg {
        UPDATE_0 | UPDATE_1 => Role::WriteOneToClear,
        SINK_VERSION
        | SCRAMBLING_STATUS
        | SOURCE_TEST_CONFIGURATION
        | 0x40..=0x42
        | 0x50..=0x5A
        | 0xD0..=0xDD => Role::Sink,
        _ => Role::Source,
    }
}

/// The SCDC register file, and the sink's scrambling lock.
///
/// The lock: `Scrambling_Status` is set when `Scrambling_Enable` is set, the line is
/// scrambled TMDS at the ratio `TMDS_Bit_Clock_Ratio` names, that ratio fits the rate (1/40
/// above 340 Mcsc, 1/10 at or below), no refusal withholds it, and the trigger has passed
/// since these conditions last began to hold: the write that set `Scrambling_Enable` (from
/// 0 to 1), a write that made the ratio match, an attempt accepted after a withheld one, or
/// the line becoming what it is, whichever came last. It is cleared when any condition
/// fails. The sink changes the status only when its lock is gained or lost, so a test's
/// preset holds until then; the sink's own reset of the status loses the lock, and the
/// conditions begin to hold again at the reset.
///
/// The status flags: `Clock_Detected` is set while the line is TMDS, at any rate (*chosen*:
/// no source gives the delay). The lane locks (lanes 0–2) are set once the sink can decode
/// the line and the trigger has passed since it could: TMDS at a ratio that fits the rate
/// and matches `TMDS_Bit_Clock_Ratio`, scrambled exactly when `Scrambling_Enable` is set,
/// and scrambled above 340 Mcsc (*chosen*: no source shows what a receiver fails to decode);
/// cleared when it cannot. The receivers set them in hardware
/// as the lanes lock, and no source shows when (*chosen*: the scrambling lock's trigger,
/// `AfterReads` counting reads of `Status_Flags_0`). Lane 3 and `FLT_ready` are FRL's. Each
/// change sets `Status_Update`, unless [`StatusUpdate::Never`]. As with the status, the sink
/// changes the flags only when what it sees changes, so a preset holds until then.
///
/// The character error counters (0x50–0x56): each of lanes 0–2 counts the link's character
/// errors on it ([`ErrorRates`]) while it is locked, up to 0x7FFF, where it stays; its high
/// byte's bit 7 says the count is valid, set while the lane is locked (*chosen*: no source
/// says what makes it valid; with it, an error-free locked link reads 0x80 at 0x56, as a
/// real sink did). 0x56 is the checksum: the bytes 0x50–0x56 sum to zero. Settled by Linux,
/// github.com/torvalds/linux `e9954f3fbddd`: `drivers/gpu/drm/display/drm_scdc_helper.c:383–412`
/// (the checksum over 0x50–0x56 at 3 lanes, the valid bit); and Amlogic,
/// github.com/CoreELEC/common_drivers `8ca9b6c3b715`:
/// `drivers/media/vout/hdmitx20/hdmi_tx_scdc.c:38–77`. The count saturating at 0x7FFF
/// (*chosen*, after one receiver: Xilinx's, embeddedsw `97f2baf7f69e`,
/// `XilinxProcessorIPLib/drivers/v_hdmirx1/src/xv_hdmirx1.c:2306–2345`). Their changes are
/// not recorded: they move with time; a test reads them through the bench's `Sink` view.
/// `CED_Update` is set by [`CedUpdate`]'s rule, its checks at whole seconds on the bench's
/// clock, each seeing the counts as they were at that second; the flag set is recorded at
/// the bench event at or after that second, as a scheduled action's time is. A read of the
/// counters sees them at the transaction's start, or with [`DuringRead::Moving`] each byte
/// at its own moment; with [`CedRead::ClearOnRead`] reading a lane's high byte clears it.
///
/// FRL link training, for a sink with a maximum FRL rate, after the two public receivers
/// (Xilinx embeddedsw `97f2baf7f69e`, `XilinxProcessorIPLib/drivers/v_hdmirx1/src/`
/// `xv_hdmirx1_frl.c` and `xv_hdmirx1_intr.c`; Amlogic, github.com/CoreELEC/common_drivers
/// `8ca9b6c3b715`, `drivers/media/vin/tvin/hdmirx/hdmi_rx_hw_t3x.c`, `hdmi_rx_hw.c` and
/// `hdmi_rx_wrapper.c`). Settled by both:
///
/// - `FLT_ready` is set from the start, and again after a reset clears it (Xilinx
///   `xv_hdmirx1_frl.c:183–254`; Amlogic `hdmi_rx_hw_t3x.c:4302–4303`).
/// - A write of a non-zero `FRL_Rate` starts LTS:3 (Xilinx `xv_hdmirx1_intr.c:2088–2100`;
///   Amlogic `hdmi_rx_wrapper.c:6692–6703`); every write of `Config_1` restarts it
///   (*chosen*). The sink clears `FRL_start` (Xilinx `xv_hdmirx1_frl.c:557–559`; Amlogic
///   `hdmi_rx_hw_t3x.c:4774`), posts its requests, one per lane in 0x41–0x42, and sets
///   `FLT_update` (Xilinx `1112–1175`; Amlogic `4775–4777`). A rate above its maximum, or a
///   value naming no rate, it answers with 0xF on every lane, a request for a lower rate
///   (*chosen*, after Xilinx).
/// - The pass is 0 on every lane, with `FLT_update` (Xilinx `398–412`; Amlogic
///   `4591–4601`, `4792–4794`); `FRL_start` follows it in LTS:P (Xilinx `616–640`; Amlogic
///   `hdmi_rx_hw_t3x.c:4717–4724`, `hdmi_rx_wrapper.c:6710–6719`).
///
/// What comes before the pass and `FRL_start`, the two differ on: [`FrlPass`], a setting.
/// With [`FrlPass::Patterns`], the default after Xilinx (*chosen*: the least forgiving):
///
/// - Once the source has cleared `FLT_update` (the sink posts nothing before: Xilinx
///   `588–596`), and each active lane has carried its requested pattern on the line at the
///   rate, within the link's [`LaneLimits`](crate::LaneLimits), for the lock trigger
///   (*chosen*: no source gives the time), the sink posts the pass. A lane requested 0 needs
///   nothing.
/// - After the pass, the lanes lock once the line is FRL at the rate without patterns, each
///   active lane within the link's limits, and the lock trigger has passed:
///   `Status_Flags_0` bits 1–3, and bit 4 at 4 lanes. A lane leaving its limits loses the
///   locks, as a link that degrades (*chosen*: no source shows the lanes' margins). Once
///   they are locked and the source has cleared `FLT_update`, the sink sets `FRL_start`
///   (Xilinx `xv_hdmirx1_intr.c:2153–2165`).
///
/// With [`FrlPass::Unchecked`], after Amlogic, the sink waits up to 100 ms for each clear
/// (`hdmi_rx_hw.c:864–879`) and checks nothing else: the pass after the lock trigger
/// whatever the lanes carry (its BIST counts only logged, `hdmi_rx_hw_t3x.c:4454–4576`),
/// `FRL_start` whatever the line carries (`hdmi_rx_wrapper.c:6711–6719`). The lanes still
/// lock as above, and there are no LTS:3 timeouts. Further:
///
/// - With `Patterns`, lanes that do not pass in time: [`SinkConfig::with_lts3_timeout`]'s
///   attempts, 0xE on the lanes not passing, then 0xF on every lane (*chosen*, after Xilinx
///   `440–547`; once 0xE is cleared, the patterns are requested again: `1134–1139`).
/// - `FRL_Rate` 0 returns to legacy: the requests are cleared (Xilinx `183–224`). So does a
///   reset of the status group, which loses the requests (*chosen*).
///
/// `Clock_Detected` stays clear in FRL (*chosen*: the bit is TMDS's); a change of the lane
/// locks sets `Status_Update` as in TMDS.
///
/// Evaluated at every event: each transaction, each byte written, each read of the status,
/// the status flags or `Update_0`, and every update. The line and the registers change only at events, so what an
/// evaluation sees has held since the one before: a condition that begins to hold is seen
/// when it does, and one seen late, past a deadline, was late.
#[derive(Debug)]
pub(crate) struct Scdc {
    regs: [u8; 256],
    offset: u8,
    owned_write: OwnedWrite,
    past_end: ScdcPastEnd,
    during_hpd_low: DuringHpdLow,
    trigger: Trigger,
    strict: bool,
    refusal: Refusal,
    enable_timeout: Option<Duration>,
    /// The line as last seen, and since when it is so.
    line: Line,
    line_since: Timestamp,
    /// When the sink last began to wait for scrambled TMDS: the write that set
    /// `Scrambling_Enable` from 0 to 1, or one that changed `TMDS_Bit_Clock_Ratio` while it
    /// was set; `None` while it is 0.
    armed_at: Option<Timestamp>,
    /// When `TMDS_Bit_Clock_Ratio` last changed.
    ratio_at: Option<Timestamp>,
    /// Whether scrambled TMDS at the matching ratio was seen since `armed_at`.
    seen_since_enable: bool,
    /// Attempts so far, and whether the last one is withheld.
    attempts: u32,
    withheld: bool,
    /// Since when the lock's conditions hold, `None` while they do not; and reads of the
    /// status since.
    lock_from: Option<Timestamp>,
    reads: u32,
    locked: bool,
    hpd_resets: [HpdReset; RegisterGroup::COUNT],
    status_update: StatusUpdate,
    /// Since when the sink can decode the line, `None` while it cannot; reads of
    /// `Status_Flags_0` since; and the flags as the sink last set them.
    lanes_from: Option<Timestamp>,
    flag_reads: u32,
    flags: u8,
    /// Errors counted per lane, in [`CED_UNIT`]s, through `counted_to`; the rates as last
    /// seen; and the counter bytes 0x50–0x56 as the sink last set them.
    ced: [u64; 5],
    counted_to: Timestamp,
    errors: ErrorRates,
    ced_shown: [u8; 11],
    ced_update: CedUpdate,
    ced_read: CedRead,
    during_read: DuringRead,
    /// The last `CED_Update` check, and the counts it saw; and the flags a check past the
    /// event's time set (a byte's, under `DuringRead::Moving`), set at the next evaluation.
    checked_at: Timestamp,
    pending_update: u8,
    ced_checked: [u64; 5],
    /// The highest `FRL_Rate` value the sink takes, 0 for none; the requests it posts first.
    max_frl: u8,
    ltp_requests: [u8; 4],
    /// Where training is, whether `FLT_ready` is shown, and since when the line has matched
    /// the requests (with reads of `Update_0` since).
    training: Training,
    flt_ready_shown: bool,
    match_from: Option<Timestamp>,
    match_reads: u32,
    /// The requests the sink wants posted; the LTS:3 budget, when the training began, the
    /// timeouts so far and the source's `FFE_Levels`; the lane limits as last seen.
    wanted: [u8; 4],
    lts3_timeout: Option<Duration>,
    lts3_from: Timestamp,
    timeouts: u32,
    ffe_levels: u8,
    lanes_seen: LinkLanes,
    /// What the sink checks before it passes, and when it last set `FLT_update`.
    frl_pass: FrlPass,
    update_set_at: Timestamp,
    /// When HPD went low, while it is low, and which groups this low has reset.
    hpd_low_since: Option<Timestamp>,
    reset_done: [bool; RegisterGroup::COUNT],
}

impl Scdc {
    /// The register file of a sink with `config`'s settings; `None` for a sink without SCDC.
    pub(crate) fn new(config: &SinkConfig) -> Option<Self> {
        if !config.scdc {
            return None;
        }
        let mut regs = [0; 256];
        regs[usize::from(SINK_VERSION)] = config.sink_version;
        let max_frl = config.max_frl_rate.map_or(0, frl_value);
        if max_frl != 0 {
            regs[usize::from(STATUS_FLAGS_0)] = FLT_READY;
        }
        Some(Self {
            regs,
            offset: 0,
            owned_write: config.owned_scdc_write,
            past_end: config.scdc_past_end,
            during_hpd_low: config.scdc_during_hpd_low,
            trigger: config.scrambling_lock,
            strict: config.strict_lock,
            refusal: config.refusal,
            enable_timeout: config.enable_timeout,
            line: Line::Off,
            line_since: Timestamp::from_micros(0),
            armed_at: None,
            ratio_at: None,
            seen_since_enable: false,
            attempts: 0,
            withheld: false,
            lock_from: None,
            reads: 0,
            locked: false,
            hpd_resets: config.hpd_resets,
            status_update: config.status_update,
            lanes_from: None,
            flag_reads: 0,
            flags: 0,
            ced: [0; 5],
            counted_to: Timestamp::from_micros(0),
            errors: ErrorRates::NONE,
            ced_shown: [0; 11],
            ced_update: config.ced_update,
            ced_read: config.ced_read,
            during_read: config.during_read,
            checked_at: Timestamp::from_micros(0),
            pending_update: 0,
            ced_checked: [0; 5],
            max_frl,
            ltp_requests: {
                let lanes = config.ltp_requests;
                [lanes.lane0, lanes.lane1, lanes.lane2, lanes.lane3]
                    .map(|pattern| pattern.map_or(0, LtpPattern::value))
            },
            training: Training::Legacy,
            flt_ready_shown: max_frl != 0,
            match_from: None,
            match_reads: 0,
            wanted: [0; 4],
            lts3_timeout: config.lts3_timeout,
            lts3_from: Timestamp::from_micros(0),
            timeouts: 0,
            ffe_levels: 0,
            lanes_seen: LinkLanes::NONE,
            frl_pass: config.frl_pass,
            update_set_at: Timestamp::from_micros(0),
            hpd_low_since: None,
            reset_done: [false; RegisterGroup::COUNT],
        })
    }

    pub(crate) fn get(&self, reg: u8) -> u8 {
        self.regs[usize::from(reg)]
    }

    /// Sets `reg` to `value` as the test's preset, whatever its role.
    pub(crate) fn preset(&mut self, reg: u8, value: u8, env: &mut Env<'_>) {
        self.set(reg, value, Actor::Test, env);
    }

    fn set(&mut self, reg: u8, to: u8, by: Actor, env: &mut Env<'_>) {
        let from = self.regs[usize::from(reg)];
        self.regs[usize::from(reg)] = to;
        env.record(Event::Register {
            addr: SCDC,
            reg,
            from,
            to,
            by,
        });
        if reg == TMDS_CONFIG {
            let now = env.now();
            let ratio_changed = (from ^ to) & RATIO_40 != 0;
            if ratio_changed {
                self.ratio_at = Some(now);
            }
            let (was, is) = (from & SCRAMBLING_ENABLE != 0, to & SCRAMBLING_ENABLE != 0);
            if is && (!was || ratio_changed) {
                self.armed_at = Some(now);
                self.seen_since_enable = false;
            } else if !is {
                self.armed_at = None;
            }
        }
        if reg == CONFIG_1 && self.max_frl != 0 {
            self.start_training(to, env);
        }
    }

    /// A write of `Config_1` holding `config`: LTS:3 at its `FRL_Rate`, or legacy for 0.
    fn start_training(&mut self, config: u8, env: &mut Env<'_>) {
        let rate = config & 0x0F;
        self.match_from = None;
        if rate == 0 {
            self.training = Training::Legacy;
            self.post_requests([0; 4], false, env);
            return;
        }
        let update = self.get(UPDATE_0);
        if update & FRL_START != 0 {
            self.set(UPDATE_0, update & !FRL_START, Actor::Sink, env);
        }
        self.training = Training::Requesting { rate };
        self.lts3_from = env.now();
        self.timeouts = 0;
        self.ffe_levels = (config >> 4).min(FFE_LEVELS_MAX);
        self.wanted = if rate > self.max_frl {
            [LTP_RATE_CHANGE; 4]
        } else {
            self.ltp_requests
        };
        self.post_requests(self.wanted, true, env);
    }

    /// Writes `requests` to 0x41–0x42 where they change, then sets `FLT_update` if `update`.
    fn post_requests(&mut self, requests: [u8; 4], update: bool, env: &mut Env<'_>) {
        for (reg, [low, high]) in [
            (LTP_REQ_01, [requests[0], requests[1]]),
            (LTP_REQ_23, [requests[2], requests[3]]),
        ] {
            let value = (low & 0x0F) | (high << 4);
            if self.get(reg) != value {
                self.set(reg, value, Actor::Sink, env);
            }
        }
        let flags = self.get(UPDATE_0);
        if update {
            self.update_set_at = env.now();
            if flags & FLT_UPDATE == 0 {
                self.set(UPDATE_0, flags | FLT_UPDATE, Actor::Sink, env);
            }
        }
    }

    /// Whether the sink goes on past the `FLT_update` it last set, at `now`: once the source
    /// has cleared it, or with [`FrlPass::Unchecked`], once the wait for it is over. The
    /// moment it went on, if it was the wait's end.
    fn past_flt_update(&self, now: Timestamp) -> (bool, Option<Timestamp>) {
        if self.get(UPDATE_0) & FLT_UPDATE == 0 {
            return (true, None);
        }
        let waited = self.update_set_at.checked_add(FLT_CLEAR_WAIT);
        match waited {
            Some(end) if self.frl_pass == FrlPass::Unchecked && end <= now => (true, Some(end)),
            _ => (false, None),
        }
    }

    /// The requests posted on each lane, from 0x41–0x42.
    fn posted(&self) -> [u8; 4] {
        let (a, b) = (self.get(LTP_REQ_01), self.get(LTP_REQ_23));
        [a & 0x0F, a >> 4, b & 0x0F, b >> 4]
    }

    /// Whether `lane` passes at `rate` on `line` within `limits`: requested nothing, or
    /// carrying its requested pattern within its limits.
    fn lane_passes(lane: usize, rate: u8, posted: [u8; 4], line: Line, limits: LinkLanes) -> bool {
        if posted[lane] == 0 {
            return true;
        }
        let Line::Frl {
            rate: line_rate,
            patterns,
            equalization,
            ..
        } = line
        else {
            return false;
        };
        let carried = [
            patterns.lane0,
            patterns.lane1,
            patterns.lane2,
            patterns.lane3,
        ]
        .map(|pattern| pattern.map_or(0, LtpPattern::value));
        frl_value(line_rate) == rate
            && posted[lane] == carried[lane]
            && within_limits(lane, rate, equalization, limits)
    }

    /// The LTS:3 timeouts due by `now`, seen with the line and the lane limits as they were
    /// since the last evaluation. Each of the first `FFE_Levels` asks for another TxFFE level
    /// (0xE) on the lanes not passing; any later one asks for a lower rate (0xF) on every
    /// lane, if a lane is not passing. The requests wait for `FLT_update` to be cleared.
    fn run_timeouts(&mut self, now: Timestamp) {
        let Training::Requesting { rate } = self.training else {
            return;
        };
        let Some(budget) = self.lts3_timeout else {
            return;
        };
        if self.frl_pass == FrlPass::Unchecked {
            return;
        }
        let no_timeout = self.get(SOURCE_TEST_CONFIGURATION) & FLT_NO_TIMEOUT != 0;
        let no_retrain = self.get(CONFIG_0) & FLT_NO_RETRAIN != 0;
        if no_timeout || no_retrain {
            return;
        }
        let step = budget / (u32::from(self.ffe_levels) + 1);
        let elapsed = now.saturating_duration_since(self.lts3_from);
        let due = if step.is_zero() {
            u128::from(u32::MAX)
        } else {
            elapsed.as_micros() / step.as_micros().max(1)
        };
        let due = u32::try_from(due).unwrap_or(u32::MAX);
        if due <= self.timeouts {
            return;
        }
        let posted = self.posted();
        let failing: [bool; 4] = core::array::from_fn(|lane| {
            lane < frl_lanes(rate)
                && !Self::lane_passes(lane, rate, posted, self.line, self.lanes_seen)
        });
        let levels = u32::from(self.ffe_levels);
        if self.timeouts < levels {
            for (wanted, &failing) in self.wanted.iter_mut().zip(&failing) {
                if failing {
                    *wanted = LTP_FFE_CHANGE;
                }
            }
        }
        if due > levels && failing.contains(&true) {
            self.wanted = [LTP_RATE_CHANGE; 4];
        }
        self.timeouts = due;
    }

    /// LTS:3: the sink posts what it wants once the source has cleared `FLT_update`, the
    /// patterns again after a TxFFE request; and once the line has carried the requested
    /// patterns for the lock trigger, it posts the pass.
    fn evaluate_training(&mut self, env: &mut Env<'_>, reading_update: bool) {
        let Training::Requesting { rate } = self.training else {
            return;
        };
        let now = env.now();
        let cleared = self.get(UPDATE_0) & FLT_UPDATE == 0;
        if cleared && self.wanted != self.posted() {
            self.post_requests(self.wanted, true, env);
            for (wanted, &first) in self.wanted.iter_mut().zip(&self.ltp_requests) {
                if *wanted == LTP_FFE_CHANGE {
                    *wanted = first;
                }
            }
            self.match_from = None;
            return;
        }
        let posted = self.posted();
        let limits = env.lane_limits();
        let (past, waited_until) = self.past_flt_update(now);
        let lanes_pass = match self.frl_pass {
            FrlPass::Unchecked => true,
            _ => (0..frl_lanes(rate))
                .all(|lane| Self::lane_passes(lane, rate, posted, self.line, limits)),
        };
        let matches = past
            && !posted
                .iter()
                .any(|&request| matches!(request, LTP_FFE_CHANGE | LTP_RATE_CHANGE))
            && lanes_pass;
        let match_from = matches.then(|| self.match_from.or(waited_until).unwrap_or(now));
        if match_from != self.match_from {
            self.match_from = match_from;
            self.match_reads = 0;
        }
        if reading_update && match_from.is_some() {
            self.match_reads = self.match_reads.saturating_add(1);
        }
        if match_from.is_some_and(|from| self.passed(from, now, self.match_reads)) {
            self.match_from = None;
            self.training = Training::Passed { rate };
            self.wanted = [0; 4];
            self.post_requests([0; 4], true, env);
        }
    }

    /// LTS:P: once the lanes are locked and the source has cleared `FLT_update`, the sink
    /// sets `FRL_start`; and `FLT_ready` whenever a reset has cleared it.
    fn evaluate_start(&mut self, env: &mut Env<'_>) {
        if self.max_frl == 0 {
            return;
        }
        if !self.flt_ready_shown {
            self.flt_ready_shown = true;
            let flags = self.get(STATUS_FLAGS_0);
            if flags & FLT_READY == 0 {
                self.set(STATUS_FLAGS_0, flags | FLT_READY, Actor::Sink, env);
            }
        }
        let Training::Passed { rate } = self.training else {
            return;
        };
        let update = self.get(UPDATE_0);
        let start = match self.frl_pass {
            FrlPass::Unchecked => self.past_flt_update(env.now()).0,
            _ => update & FLT_UPDATE == 0 && self.flags & LANES_LOCKED != 0,
        };
        if start {
            self.training = Training::Started { rate };
            self.set(UPDATE_0, update | FRL_START, Actor::Sink, env);
        }
    }

    /// The sink loses its lock by itself: the status clears, and the conditions begin to hold
    /// again from now.
    fn lose_lock(&mut self, env: &mut Env<'_>) {
        let status = self.get(SCRAMBLING_STATUS);
        if status & STATUS != 0 {
            self.set(SCRAMBLING_STATUS, status & !STATUS, Actor::Sink, env);
        }
        self.locked = false;
        self.lock_from = None;
    }

    /// The resets around an HPD low: each group once per low, when its rule says.
    ///
    /// The public sinks: Xilinx embeddedsw `97f2baf7f69e`,
    /// `XilinxProcessorIPLib/drivers/v_hdmirx1/src/xv_hdmirx1.c:740–765` (lowering HPD
    /// resets FRL, raising it clears `FRL_Rate`), `xv_hdmirx1_frl.c:76`, `1480–1502` (the
    /// reset clears `TMDS_Config` bits 1:0), `xv_hdmirx1_intr.c:1168–1182` (the cable's 5 V
    /// lost clears all of SCDC); Amlogic, github.com/CoreELEC/common_drivers `8ca9b6c3b715`,
    /// `drivers/media/vin/tvin/hdmirx/hdmi_rx_hw.c:2045–2069`, `2854–2870` (SCDC cleared, or
    /// the HPD state handed to the IP's SCDC block), called with HPD low from
    /// `hdmi_rx_wrapper.c:3663`, `4794–4797`.
    fn hpd_resets(&mut self, env: &mut Env<'_>) {
        let now = env.now();
        let low_since = match (self.hpd_low_since, env.hpd()) {
            (None, true) => return,
            (None, false) => {
                self.hpd_low_since = Some(now);
                self.reset_done = [false; RegisterGroup::COUNT];
                now
            }
            (Some(since), _) => since,
        };
        for &group in RegisterGroup::ALL {
            let due = match self.hpd_resets[group.index()] {
                HpdReset::OnHpdLow => true,
                HpdReset::OnHpdRise => env.hpd(),
                HpdReset::After(low_for) => {
                    low_since.checked_add(low_for).is_some_and(|due| now >= due)
                }
                HpdReset::Never => false,
            };
            if due && !self.reset_done[group.index()] {
                self.reset_done[group.index()] = true;
                for &reg in group.registers() {
                    if self.get(reg) != 0 {
                        self.set(reg, 0, Actor::Sink, env);
                    }
                    if reg == SCRAMBLING_STATUS {
                        self.locked = false;
                        self.lock_from = None;
                    }
                    if reg == STATUS_FLAGS_0 {
                        self.flags = 0;
                        self.lanes_from = None;
                        self.flt_ready_shown = false;
                        self.training = Training::Legacy;
                        self.match_from = None;
                    }
                    if reg == CED {
                        self.ced = [0; 5];
                        self.ced_shown = [0; 11];
                        self.ced_checked = [0; 5];
                    }
                }
            }
        }
        if env.hpd() {
            self.hpd_low_since = None;
        }
    }

    /// The sink evaluates its behaviour at `env`'s time; `reading` the register the source
    /// is about to read, which for `Scrambling_Status` and `Status_Flags_0` counts toward
    /// `Trigger::AfterReads`.
    fn evaluate(&mut self, env: &mut Env<'_>, reading: Option<u8>) {
        self.count_and_check(env);
        self.run_timeouts(env.now());
        self.hpd_resets(env);
        let now = env.now();
        let line = env.line();
        if line != self.line {
            if !same_lanes(self.line, line) {
                self.lanes_from = None;
            }
            self.line = line;
            self.line_since = now;
            self.lock_from = None;
        }
        let config = self.get(TMDS_CONFIG);
        let matching = match line {
            Line::Tmds {
                character_rate_khz,
                ratio,
                scrambled: true,
            } => {
                let div40 = ratio == TmdsClockRatio::Div40;
                let fits = div40 == (character_rate_khz > RATIO_40_ABOVE_KHZ);
                fits && div40 == (config & RATIO_40 != 0)
            }
            _ => false,
        };
        // Giving up: no scrambled TMDS at the matching ratio before the deadline. Seen at or
        // past it, it came too late.
        if let Some(at) = self.armed_at {
            let deadline = self
                .enable_timeout
                .and_then(|timeout| at.checked_add(timeout));
            let in_time = deadline.is_none_or(|deadline| now < deadline);
            self.seen_since_enable |= matching && in_time;
            if !in_time && !self.seen_since_enable {
                self.set(TMDS_CONFIG, config & !SCRAMBLING_ENABLE, Actor::Sink, env);
            }
        }
        let holds = self
            .armed_at
            .is_some_and(|at| matching && !self.withheld && !(self.strict && self.too_early(at)));
        let lock_from = holds.then(|| self.lock_from.unwrap_or(now));
        if lock_from != self.lock_from {
            self.lock_from = lock_from;
            self.reads = 0;
        }
        if reading == Some(SCRAMBLING_STATUS) && lock_from.is_some() {
            self.reads = self.reads.saturating_add(1);
        }
        let locked = lock_from.is_some_and(|from| self.passed(from, now, self.reads));
        if locked != self.locked {
            self.locked = locked;
            let status = self.get(SCRAMBLING_STATUS);
            let to = if locked {
                status | STATUS
            } else {
                status & !STATUS
            };
            if to != status {
                self.set(SCRAMBLING_STATUS, to, Actor::Sink, env);
            }
        }
        self.evaluate_training(env, reading == Some(UPDATE_0));
        self.evaluate_flags(env, reading == Some(STATUS_FLAGS_0));
        self.evaluate_start(env);
        self.errors = env.character_errors();
        self.lanes_seen = env.lane_limits();
        self.show_errors();
    }

    /// Counts the errors since the last evaluation, with the `CED_Update` checks that fell
    /// in between, each at its second. Rates do not change between events, and the locks
    /// only where one comes due, so every count in between is known. After a check from which no later one can set a
    /// flag (each rising counter's flag is set, or none rises), later checks change nothing
    /// but the counts they keep: they are skipped to the last.
    fn count_and_check(&mut self, env: &mut Env<'_>) {
        if self.pending_update != 0 {
            let update = self.get(UPDATE_0);
            if update | self.pending_update != update {
                self.set(UPDATE_0, update | self.pending_update, Actor::Sink, env);
            }
            self.pending_update = 0;
        }
        self.count_and_check_to(env.now(), env);
    }

    /// As [`count_and_check`](Self::count_and_check), to `now`: the event's time, or a byte's
    /// within a transaction.
    fn count_and_check_to(&mut self, now: Timestamp, env: &mut Env<'_>) {
        let CedUpdate::EverySecond { rise } = self.ced_update else {
            self.count_errors(now);
            return;
        };
        let rise = u64::from(rise.max(1));
        while let Some(next) = self.checked_at.checked_add(CED_CHECK) {
            if next > now {
                break;
            }
            self.count_errors(next);
            self.see_counts(next, rise, env);
            if !self.check_can_set(next) {
                let seconds = now.saturating_duration_since(next).as_secs();
                let seconds = u32::try_from(seconds).unwrap_or(u32::MAX);
                if let Some(last) = next.checked_add(CED_CHECK.saturating_mul(seconds)) {
                    self.count_errors(last);
                    self.see_counts(last, rise, env);
                }
                break;
            }
        }
        self.count_errors(now);
    }

    /// Whether a check after `at`, before the next event, can set a flag: a counter counting,
    /// with errors, below 0x7FFF, whose flag (`CED_Update` for a lane, `RSED_Update` for the
    /// corrections) is not set yet.
    fn check_can_set(&self, at: Timestamp) -> bool {
        let rates = self.counter_rates();
        let flags = self.flags_at(at);
        let update = self.get(UPDATE_0) | self.pending_update;
        (0..COUNTERS.len()).any(|i| {
            let flag = if i == RS { RSED_UPDATE } else { CED_UPDATE };
            self.counting(i, flags)
                && rates[i] > 0
                && self.ced[i] < CED_MAX * CED_UNIT
                && update & flag == 0
        })
    }

    /// Whether counter `i` counts and is valid with the status flags `flags`: a lane while
    /// it is locked; the Reed-Solomon corrections while the FRL lanes are locked (*chosen*:
    /// Linux reads them only in FRL).
    fn counting(&self, i: usize, flags: u8) -> bool {
        if i == RS {
            matches!(self.line, Line::Frl { .. }) && flags & LANES_LOCKED != 0
        } else {
            flags & (1 << (i + 1)) != 0
        }
    }

    /// When the lanes lock, between the last evaluation and the next, if they do: their
    /// conditions hold since `lanes_from`, and a time trigger names the moment. The flags
    /// change at the next event; the counters count from this moment.
    fn lock_due(&self) -> Option<Timestamp> {
        if self.flags & LANES_LOCKED != 0 {
            return None;
        }
        let Trigger::After(delay) = self.trigger else {
            return None;
        };
        self.lanes_from?.checked_add(delay)
    }

    /// The status flags as they are at `at`, since the last evaluation: with the lanes locked
    /// once their lock is due.
    fn flags_at(&self, at: Timestamp) -> u8 {
        match self.lock_due() {
            Some(due) if due <= at => {
                let lanes = match self.line {
                    Line::Frl { rate, .. } if frl_lanes(frl_value(rate)) == 4 => FOUR_LANES_LOCKED,
                    _ => LANES_LOCKED,
                };
                self.flags | lanes
            }
            _ => self.flags,
        }
    }

    /// Each counter's rate: the lanes' character errors, then the corrections.
    fn counter_rates(&self) -> [u32; 5] {
        let [a, b, c, d] = self.errors.lanes();
        [a, b, c, d, self.errors.rs_corrections]
    }

    /// The check at `at`: sets `CED_Update` when a counting lane's count rose by `rise`
    /// since the last check, or just reached 0x7FFF, and `RSED_Update` when the corrections
    /// did; and keeps the counts for the next check.
    fn see_counts(&mut self, at: Timestamp, rise: u64, env: &mut Env<'_>) {
        self.checked_at = at;
        let flags = self.flags_at(at);
        let mut set = 0;
        for i in 0..COUNTERS.len() {
            if !self.counting(i, flags) {
                continue;
            }
            let count = (self.ced[i] / CED_UNIT).min(CED_MAX);
            let checked = self.ced_checked[i];
            if count >= checked.saturating_add(rise) || (count != checked && count == CED_MAX) {
                set |= if i == RS { RSED_UPDATE } else { CED_UPDATE };
            }
            self.ced_checked[i] = count;
        }
        // A check past the event's time (a byte's within a read) sets its flags at the next
        // evaluation, so no record runs ahead of the transaction's.
        if at > env.now() {
            self.pending_update |= set;
            return;
        }
        let update = self.get(UPDATE_0);
        if update | set != update {
            self.set(UPDATE_0, update | set, Actor::Sink, env);
        }
    }

    /// Counts the errors up to `now`, at the rates and on the counters counting since the
    /// last count: the rates do not change between events, and the counters counting only
    /// where the lanes lock between them, which splits the count there.
    /// A time before the last count (a byte's time runs ahead of the transaction's start)
    /// counts nothing.
    fn count_errors(&mut self, now: Timestamp) {
        if let Some(due) = self.lock_due()
            && self.counted_to < due
            && due <= now
        {
            self.count_span(due, self.flags);
        }
        self.count_span(now, self.flags_at(now));
    }

    /// Counts the errors from the last count up to `to`, on the counters counting with the
    /// status flags `flags`.
    fn count_span(&mut self, to: Timestamp, flags: u8) {
        if to <= self.counted_to {
            return;
        }
        let micros = to.saturating_duration_since(self.counted_to).as_micros();
        let micros = u64::try_from(micros).unwrap_or(u64::MAX);
        self.counted_to = to;
        let rates = self.counter_rates();
        for (i, rate) in rates.into_iter().enumerate() {
            if self.counting(i, flags) {
                let more = u64::from(rate).saturating_mul(micros);
                self.ced[i] = self.ced[i].saturating_add(more).min(CED_MAX * CED_UNIT);
            }
        }
    }

    /// A read of `reg` with [`CedRead::ClearOnRead`]: a counter's high byte clears it.
    fn clear_on_read(&mut self, reg: u8) {
        if let Some(i) = COUNTERS.iter().position(|&low| low + 1 == reg) {
            self.ced[i] = 0;
            self.ced_checked[i] = 0;
        }
    }

    /// The counter bytes 0x50–0x5A for the counts, written where they changed since the
    /// sink last wrote them, so a preset holds until then. The checksum covers lane 3 when
    /// `Config_1`'s rate is a 4-lane one, as the sources reading it do.
    fn show_errors(&mut self) {
        let mut bytes = [0u8; 11];
        for (i, &low) in COUNTERS.iter().enumerate() {
            let [count_low, count_high, ..] = (self.ced[i] / CED_UNIT).to_le_bytes();
            let valid = if self.counting(i, self.flags) {
                CED_VALID
            } else {
                0
            };
            let at = usize::from(low - CED);
            bytes[at] = count_low;
            bytes[at + 1] = count_high | valid;
        }
        let rate = self.get(CONFIG_1) & 0x0F;
        let four_lanes = (1..=FRL_RATE_MAX).contains(&rate) && frl_lanes(rate) == 4;
        let mut summed: u8 = bytes[..6]
            .iter()
            .fold(0, |sum, &byte| sum.wrapping_add(byte));
        if four_lanes {
            summed = summed.wrapping_add(bytes[7]).wrapping_add(bytes[8]);
        }
        bytes[usize::from(CED_CHECKSUM - CED)] = summed.wrapping_neg();
        for (i, (&byte, shown)) in bytes.iter().zip(&mut self.ced_shown).enumerate() {
            if byte != *shown {
                *shown = byte;
                // `i` is below 11.
                self.regs[usize::from(CED) + i] = byte;
            }
        }
    }

    /// Whether the trigger has passed for conditions that hold since `from`, with `reads`
    /// reads of the register that shows them since.
    fn passed(&self, from: Timestamp, now: Timestamp, reads: u32) -> bool {
        match self.trigger {
            Trigger::After(delay) => from.checked_add(delay).is_some_and(|due| now >= due),
            Trigger::AfterReads(n) => reads >= n.max(1),
        }
    }

    /// The status flags, after the scrambling lock: `Scrambling_Enable` may just have been
    /// cleared by the sink.
    fn evaluate_flags(&mut self, env: &mut Env<'_>, reading_flags: bool) {
        let now = env.now();
        let config = self.get(TMDS_CONFIG);
        let (clock, decodable) = match self.line {
            Line::Tmds {
                character_rate_khz,
                ratio,
                scrambled,
            } => {
                let div40 = ratio == TmdsClockRatio::Div40;
                let high = character_rate_khz > RATIO_40_ABOVE_KHZ;
                let decodable = div40 == high
                    && div40 == (config & RATIO_40 != 0)
                    && scrambled == (config & SCRAMBLING_ENABLE != 0)
                    && (scrambled || !high);
                (true, decodable)
            }
            Line::Frl {
                rate,
                patterns,
                equalization,
                ..
            } => {
                let trained = match self.training {
                    Training::Passed { rate: at } | Training::Started { rate: at } => {
                        frl_value(rate) == at
                    }
                    _ => false,
                };
                // Lanes outside their limits after the pass lose their lock: a link that
                // degrades.
                let value = frl_value(rate);
                let limits = env.lane_limits();
                let within = (0..frl_lanes(value))
                    .all(|lane| within_limits(lane, value, equalization, limits));
                (
                    false,
                    trained && patterns == LanePatterns::default() && within,
                )
            }
            Line::Off => (false, false),
        };
        let lanes = match self.line {
            Line::Frl { rate, .. } if frl_lanes(frl_value(rate)) == 4 => FOUR_LANES_LOCKED,
            _ => LANES_LOCKED,
        };
        let lanes_from = decodable.then(|| self.lanes_from.unwrap_or(now));
        if lanes_from != self.lanes_from {
            self.lanes_from = lanes_from;
            self.flag_reads = 0;
        }
        if reading_flags && lanes_from.is_some() {
            self.flag_reads = self.flag_reads.saturating_add(1);
        }
        let locked = lanes_from.is_some_and(|from| self.passed(from, now, self.flag_reads));
        let flags = if clock { CLOCK_DETECTED } else { 0 } | if locked { lanes } else { 0 };
        if flags == self.flags {
            return;
        }
        self.flags = flags;
        let held = self.get(STATUS_FLAGS_0);
        let to = (held & !(CLOCK_DETECTED | FOUR_LANES_LOCKED)) | flags;
        if to == held {
            return;
        }
        self.set(STATUS_FLAGS_0, to, Actor::Sink, env);
        let update = self.get(UPDATE_0);
        if self.status_update == StatusUpdate::OnChange && update & STATUS_UPDATE == 0 {
            self.set(UPDATE_0, update | STATUS_UPDATE, Actor::Sink, env);
        }
    }

    /// For the strict sink: whether the line began too early for the write armed at `at`, so
    /// the output must stop and start again: before that write, or within
    /// [`RATIO_RESUME_MIN`] of the last change of `TMDS_Bit_Clock_Ratio`.
    fn too_early(&self, at: Timestamp) -> bool {
        let resume_from = self
            .ratio_at
            .and_then(|ratio_at| ratio_at.checked_add(RATIO_RESUME_MIN));
        self.line_since < at || resume_from.is_some_and(|from| self.line_since < from)
    }

    /// The next register in a burst: the offset advances, wrapping past 0xFF. Returns
    /// whether the burst has now run past 0xFF.
    fn advance(&mut self) -> bool {
        let (next, wrapped) = self.offset.overflowing_add(1);
        self.offset = next;
        wrapped
    }

    /// The source writes `value` at the current offset.
    fn write_byte(&mut self, value: u8, env: &mut Env<'_>) -> Result<(), DdcErrorKind> {
        let reg = self.offset;
        let mut value = value;
        if reg == TMDS_CONFIG && value & SCRAMBLING_ENABLE != 0 {
            self.attempts = self.attempts.saturating_add(1);
            self.withheld = false;
            if self.attempts <= self.refusal.attempts {
                env.record(Event::Refusal {
                    attempt: self.attempts,
                    kind: self.refusal.kind,
                });
                match self.refusal.kind {
                    RefusalKind::WithholdStatus => self.withheld = true,
                    RefusalKind::DropEnable => value &= !SCRAMBLING_ENABLE,
                }
            }
        }
        match role(reg) {
            Role::Source => self.set(reg, value, Actor::Source, env),
            Role::WriteOneToClear => {
                let to = self.get(reg) & !value;
                self.set(reg, to, Actor::Source, env);
            }
            Role::Sink => match self.owned_write {
                OwnedWrite::Ignore => env.record(Event::IgnoredWrite {
                    addr: SCDC,
                    reg,
                    value,
                    why: Ignored::SinkOwned,
                }),
                OwnedWrite::Nack => return Err(NACK_DATA),
            },
        }
        Ok(())
    }
}

/// Where an FRL sink's link training is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Training {
    /// Legacy (TMDS), or not started: `FRL_Rate` 0.
    Legacy,
    /// LTS:3 at `rate` (the `FRL_Rate` value): the requests are posted.
    Requesting { rate: u8 },
    /// The pass is posted; `FRL_start` comes once the lanes lock.
    Passed { rate: u8 },
    /// `FRL_start` is set.
    Started { rate: u8 },
}

/// The `FRL_Rate` value of `rate`.
const fn frl_value(rate: HdmiForumFrl) -> u8 {
    match rate {
        HdmiForumFrl::Rate3Gbps3Lanes => 1,
        HdmiForumFrl::Rate6Gbps3Lanes => 2,
        HdmiForumFrl::Rate6Gbps4Lanes => 3,
        HdmiForumFrl::Rate8Gbps4Lanes => 4,
        HdmiForumFrl::Rate10Gbps4Lanes => 5,
        HdmiForumFrl::Rate12Gbps4Lanes => FRL_RATE_MAX,
        _ => 0,
    }
}

/// Whether the lanes carry the same signal on `a` and on `b`, so their locks hold: in FRL,
/// the same rate and patterns, whatever the output (video starts on locked lanes).
fn same_lanes(a: Line, b: Line) -> bool {
    match (a, b) {
        (
            Line::Frl { rate, patterns, .. },
            Line::Frl {
                rate: other_rate,
                patterns: other_patterns,
                ..
            },
        ) => rate == other_rate && patterns == other_patterns,
        _ => a == b,
    }
}

/// Whether `lane` at the `FRL_Rate` value `rate`, with `equalization` on the line, is
/// within the link's `limits`: the rate at most its maximum, the TxFFE level at least its
/// minimum.
fn within_limits(lane: usize, rate: u8, equalization: Option<EqParams>, limits: LinkLanes) -> bool {
    let limit = limits.lanes()[lane];
    limit.max_rate.is_none_or(|max| rate <= frl_value(max))
        && tx_ffe_levels(equalization)[lane] >= limit.min_tx_ffe
}

/// Each lane's TxFFE level on the line: 0 where the PHY has set none.
fn tx_ffe_levels(equalization: Option<EqParams>) -> [TxFfeLevel; 4] {
    equalization.map_or([TxFfeLevel::default(); 4], |eq| {
        [
            eq.lane0.tx_ffe_level,
            eq.lane1.tx_ffe_level,
            eq.lane2.tx_ffe_level,
            eq.lane3
                .map_or(TxFfeLevel::default(), |lane| lane.tx_ffe_level),
        ]
    })
}

/// How many lanes the `FRL_Rate` value `rate` uses.
const fn frl_lanes(rate: u8) -> usize {
    if rate <= FRL_3_LANES_MAX { 3 } else { 4 }
}

const NACK_DATA: DdcErrorKind = DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Data);

impl Target for Scdc {
    fn answers(&self, addr: u8, env: &Env<'_>) -> bool {
        addr == SCDC && (env.hpd() || self.during_hpd_low == DuringHpdLow::Answers)
    }

    fn update(&mut self, env: &mut Env<'_>) {
        self.evaluate(env, None);
    }

    /// A write sets the offset with its first byte, then writes from there; a read reads from
    /// the offset. The offset advances with every byte and persists between transactions,
    /// so a read with no offset written reads where the last transaction stopped (*chosen*:
    /// every source surveyed writes the offset first).
    /// Past 0xFF a burst wraps to 0x00, or with `ScdcPastEnd::Nack` a write fails on the next
    /// byte; a read always wraps.
    fn transaction(
        &mut self,
        ops: &mut [Operation<'_>],
        env: &mut Env<'_>,
    ) -> Result<(), DdcErrorKind> {
        self.hooked_transaction(ops, env, None)
    }
}

impl Scdc {
    /// The transaction, with the test's hook, if any, deciding each byte written and each
    /// byte read.
    pub(crate) fn hooked_transaction(
        &mut self,
        ops: &mut [Operation<'_>],
        env: &mut Env<'_>,
        mut hook: Option<&mut (dyn ScdcHook + Send)>,
    ) -> Result<(), DdcErrorKind> {
        self.evaluate(env, None);
        let refuse_past_end = self.past_end == ScdcPastEnd::Nack;
        // Bytes on the wire before the next: each operation's address byte, then its data.
        let mut wire: u32 = 0;
        for op in ops.iter_mut() {
            wire = wire.saturating_add(1).saturating_add(match op {
                Operation::Write { bytes, .. } => u32::try_from(bytes.len()).unwrap_or(u32::MAX),
                Operation::Read { .. } => 0,
            });
            match op {
                Operation::Write { addr: SCDC, bytes } => {
                    let Some((&offset, data)) = bytes.split_first() else {
                        continue;
                    };
                    self.offset = offset;
                    let mut past_end = false;
                    for &value in data {
                        if past_end && refuse_past_end {
                            return Err(NACK_DATA);
                        }
                        let reg = self.offset;
                        let decision = match hook.as_deref_mut() {
                            Some(hook) => {
                                hook.write(reg, value, &ScdcRegisters::new(&self.regs), env)
                            }
                            None => HookWrite::Store(value),
                        };
                        match decision {
                            HookWrite::Store(value) => self.write_byte(value, env)?,
                            HookWrite::StoreRelocking(value) => {
                                self.write_byte(value, env)?;
                                self.lose_lock(env);
                            }
                            HookWrite::Drop | HookWrite::Nack => {
                                env.record(Event::IgnoredWrite {
                                    addr: SCDC,
                                    reg,
                                    value,
                                    why: Ignored::Hook,
                                });
                                if decision == HookWrite::Nack {
                                    return Err(NACK_DATA);
                                }
                            }
                        }
                        self.evaluate(env, None);
                        past_end |= self.advance();
                    }
                }
                Operation::Read { addr: SCDC, buf } => {
                    for byte in buf.iter_mut() {
                        if matches!(self.offset, UPDATE_0 | SCRAMBLING_STATUS | STATUS_FLAGS_0) {
                            self.evaluate(env, Some(self.offset));
                        }
                        let counter = (CED..=CED_LAST).contains(&self.offset);
                        if counter && self.during_read == DuringRead::Moving {
                            let ahead = env.byte_time().saturating_mul(wire);
                            let at = env.now().checked_add(ahead).unwrap_or(env.now());
                            self.count_and_check_to(at, env);
                            self.show_errors();
                        }
                        let value = self.get(self.offset);
                        *byte = match hook.as_deref_mut() {
                            Some(hook) => {
                                hook.read(self.offset, value, &ScdcRegisters::new(&self.regs), env)
                            }
                            None => value,
                        };
                        if self.ced_read == CedRead::ClearOnRead {
                            self.clear_on_read(self.offset);
                        }
                        wire = wire.saturating_add(1);
                        // A read wraps past 0xFF: the sink cannot refuse a byte it sends.
                        self.advance();
                    }
                }
                _ => {
                    return Err(DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Address));
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

    /// Runs `ops` on a default register file.
    fn run(ops: &mut [Operation<'_>]) -> (Scdc, Result<(), DdcErrorKind>) {
        let mut scdc = Scdc::new(&SinkConfig::new()).unwrap();
        let mut transcript = Transcript::<8>::new();
        let mut env = Env::new(Timestamp::from_micros(0), true, Line::Off, &mut transcript);
        let result = scdc.transaction(ops, &mut env);
        (scdc, result)
    }

    #[test]
    fn an_empty_write_changes_nothing() {
        let mut buf = [0u8; 1];
        let (scdc, result) = run(&mut [
            Operation::Write {
                addr: SCDC,
                bytes: &[],
            },
            Operation::Read {
                addr: SCDC,
                buf: &mut buf,
            },
        ]);
        assert_eq!(result, Ok(()));
        assert_eq!(scdc.offset, 1);
        assert_eq!(buf, [0]);
    }

    #[test]
    fn another_address_in_its_transaction_is_not_acknowledged() {
        let (scdc, result) = run(&mut [
            Operation::Write {
                addr: SCDC,
                bytes: &[0x20, 0x03],
            },
            Operation::Write {
                addr: 0x30,
                bytes: &[1],
            },
        ]);
        assert_eq!(
            result,
            Err(DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Address))
        );
        assert_eq!(scdc.get(TMDS_CONFIG), 3, "what came before stays");
    }

    #[test]
    fn roles_follow_the_sources() {
        for reg in [
            SOURCE_VERSION,
            TMDS_CONFIG,
            CONFIG_0,
            CONFIG_1,
            0xC0,
            0xDE,
            0xFF,
        ] {
            assert_eq!(role(reg), Role::Source, "{reg:#04x}");
        }
        for reg in [UPDATE_0, UPDATE_1] {
            assert_eq!(role(reg), Role::WriteOneToClear, "{reg:#04x}");
        }
        for reg in [
            SINK_VERSION,
            SCRAMBLING_STATUS,
            SOURCE_TEST_CONFIGURATION,
            0x40,
            0x42,
            0x50,
            0x5A,
            0xD0,
            0xDD,
        ] {
            assert_eq!(role(reg), Role::Sink, "{reg:#04x}");
        }
    }

    #[test]
    fn no_register_file_without_scdc() {
        assert!(Scdc::new(&SinkConfig::new().without_scdc()).is_none());
    }
}
