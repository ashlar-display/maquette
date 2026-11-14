//! The built-in SCDC target: the sink's 256-byte SCDC register file at 0x54.
//!
//! Register addresses, bits and who writes each register, from public implementations of HDMI
//! sources (no maquette code shares them with the crates it tests):
//!
//! - Linux, github.com/torvalds/linux `e9954f3fbddd`: `include/drm/display/drm_scdc.h:27–53`
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
    DuringHpdLow, OwnedWrite, Refusal, RefusalKind, ScdcPastEnd, SinkConfig, Trigger,
};
use crate::line::Line;
use crate::target::{Env, Operation, Target};
use crate::transcript::{Actor, Event, Ignored};

/// `TMDS_Config` bit 0.
const SCRAMBLING_ENABLE: u8 = 1 << 0;
/// `TMDS_Config` bit 1: the ratio is 1/40.
const RATIO_40: u8 = 1 << 1;
/// `Scrambling_Status` bit 0.
const STATUS: u8 = 1 << 0;

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

/// The role of `reg`. The sink's: `Sink_Version`, `Scrambling_Status`, the status flags
/// (0x40–0x42), the error counters (0x50–0x5A) and the manufacturer's block (0xD0–0xFF),
/// which Amlogic does not mark `RW` and which sources only read. A register outside the
/// modelled set is stored as written (*chosen*).
fn role(reg: u8) -> Role {
    match reg {
        UPDATE_0 | UPDATE_1 => Role::WriteOneToClear,
        SINK_VERSION | SCRAMBLING_STATUS | 0x40..=0x42 | 0x50..=0x5A | 0xD0..=0xFF => Role::Sink,
        _ => Role::Source,
    }
}

/// The SCDC register file, and the sink's scrambling lock.
///
/// The lock: `Scrambling_Status` is set when `Scrambling_Enable` is set, the line is
/// scrambled TMDS at the ratio `TMDS_Bit_Clock_Ratio` names, and the trigger has passed since
/// the later of the write that set `Scrambling_Enable` (from 0 to 1) and the line becoming
/// what it is. It is cleared when any condition fails. The sink changes the status only when
/// its lock is gained or lost, so a test's preset holds until then. Evaluated at every
/// event: each transaction, each byte written, each read of the status, and every update.
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
    /// When `Scrambling_Enable` was last set from 0 to 1; `None` while it is 0.
    enable_at: Option<Timestamp>,
    /// Whether scrambled TMDS at the matching ratio was seen since `enable_at`.
    seen_since_enable: bool,
    /// Attempts so far, and whether the last one is withheld.
    attempts: u32,
    withheld: bool,
    /// The time the current lock conditions hold from, and reads of the status since.
    lock_from: Option<Timestamp>,
    reads: u32,
    locked: bool,
}

impl Scdc {
    /// The register file of a sink with `config`'s settings; `None` for a sink without SCDC.
    pub(crate) fn new(config: &SinkConfig) -> Option<Self> {
        if !config.scdc {
            return None;
        }
        let mut regs = [0; 256];
        regs[usize::from(SINK_VERSION)] = config.sink_version;
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
            enable_at: None,
            seen_since_enable: false,
            attempts: 0,
            withheld: false,
            lock_from: None,
            reads: 0,
            locked: false,
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
            match (from & SCRAMBLING_ENABLE != 0, to & SCRAMBLING_ENABLE != 0) {
                (false, true) => {
                    self.enable_at = Some(env.now());
                    self.seen_since_enable = false;
                }
                (true, false) => self.enable_at = None,
                _ => {}
            }
        }
    }

    /// The sink evaluates its behaviour at `env`'s time; `reading_status` when the source is
    /// about to read `Scrambling_Status`, which counts toward `Trigger::AfterReads`.
    fn evaluate(&mut self, env: &mut Env<'_>, reading_status: bool) {
        let now = env.now();
        let line = env.line();
        if line != self.line {
            self.line = line;
            self.line_since = now;
        }
        let config = self.get(TMDS_CONFIG);
        let matching = match line {
            Line::Tmds {
                ratio,
                scrambled: true,
                ..
            } => (ratio == TmdsClockRatio::Div40) == (config & RATIO_40 != 0),
            _ => false,
        };
        // Giving up: no scrambled TMDS at the matching ratio within the timeout.
        if let Some(at) = self.enable_at {
            self.seen_since_enable |= matching;
            let expired = self
                .enable_timeout
                .and_then(|timeout| at.checked_add(timeout))
                .is_some_and(|deadline| now >= deadline);
            if expired && !self.seen_since_enable {
                self.set(TMDS_CONFIG, config & !SCRAMBLING_ENABLE, Actor::Sink, env);
            }
        }
        let lock_from = match self.enable_at {
            Some(at) if matching && !self.withheld && !(self.strict && self.line_since < at) => {
                Some(at.max(self.line_since))
            }
            _ => None,
        };
        if lock_from != self.lock_from {
            self.lock_from = lock_from;
            self.reads = 0;
        }
        if reading_status && lock_from.is_some() {
            self.reads = self.reads.saturating_add(1);
        }
        let locked = lock_from.is_some_and(|from| match self.trigger {
            Trigger::After(delay) => from.checked_add(delay).is_some_and(|due| now >= due),
            Trigger::AfterReads(n) => self.reads >= n.max(1),
        });
        if locked != self.locked {
            self.locked = locked;
            let status = self.get(SCRAMBLING_STATUS);
            let to = if locked {
                status | STATUS
            } else {
                status & !STATUS
            };
            self.set(SCRAMBLING_STATUS, to, Actor::Sink, env);
        }
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

const NACK_DATA: DdcErrorKind = DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Data);

impl Target for Scdc {
    fn answers(&self, addr: u8, env: &Env<'_>) -> bool {
        addr == SCDC && (env.hpd() || self.during_hpd_low == DuringHpdLow::Answers)
    }

    fn update(&mut self, env: &mut Env<'_>) {
        self.evaluate(env, false);
    }

    /// A write sets the offset with its first byte, then writes from there; a read reads from
    /// the offset. The offset advances with every byte and persists between transactions.
    /// Past 0xFF a burst wraps to 0x00, or with `ScdcPastEnd::Nack` fails on the next byte.
    fn transaction(
        &mut self,
        ops: &mut [Operation<'_>],
        env: &mut Env<'_>,
    ) -> Result<(), DdcErrorKind> {
        self.evaluate(env, false);
        let mut past_end = false;
        let refuse_past_end = self.past_end == ScdcPastEnd::Nack;
        for op in ops.iter_mut() {
            match op {
                Operation::Write { addr: SCDC, bytes } => {
                    let Some((&offset, data)) = bytes.split_first() else {
                        continue;
                    };
                    self.offset = offset;
                    past_end = false;
                    for &value in data {
                        if past_end && refuse_past_end {
                            return Err(NACK_DATA);
                        }
                        self.write_byte(value, env)?;
                        self.evaluate(env, false);
                        past_end |= self.advance();
                    }
                }
                Operation::Read { addr: SCDC, buf } => {
                    for byte in buf.iter_mut() {
                        if past_end && refuse_past_end {
                            return Err(NACK_DATA);
                        }
                        if self.offset == SCRAMBLING_STATUS {
                            self.evaluate(env, true);
                        }
                        *byte = self.get(self.offset);
                        past_end |= self.advance();
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
        for reg in [SOURCE_VERSION, TMDS_CONFIG, CONFIG_0, CONFIG_1, 0x35, 0xC0] {
            assert_eq!(role(reg), Role::Source, "{reg:#04x}");
        }
        for reg in [UPDATE_0, UPDATE_1] {
            assert_eq!(role(reg), Role::WriteOneToClear, "{reg:#04x}");
        }
        for reg in [
            SINK_VERSION,
            SCRAMBLING_STATUS,
            0x40,
            0x42,
            0x50,
            0x5A,
            0xD0,
            0xFF,
        ] {
            assert_eq!(role(reg), Role::Sink, "{reg:#04x}");
        }
    }

    #[test]
    fn no_register_file_without_scdc() {
        assert!(Scdc::new(&SinkConfig::new().without_scdc()).is_none());
    }
}
