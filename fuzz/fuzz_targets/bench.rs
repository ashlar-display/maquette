//! Drives a bench with arbitrary sequences of bus calls, PHY calls, line changes, HPD, plug
//! and adapter changes, presets, faults and clock advances, on a sink whose settings and bus capabilities
//! the input chooses, and checks the invariants a simulator must keep after every action:
//!
//! - the same input gives the same transcript (each input runs twice);
//! - nothing panics and no arithmetic overflows (the budget is off; the input bounds the
//!   actions);
//! - `Scrambling_Status` is set only while `Scrambling_Enable` is set and the line is
//!   scrambled TMDS at the matching ratio, and that ratio fits the rate (no action presets
//!   the status);
//! - no `Register` event changes a register the sink owns by `Actor::Source`;
//! - the segment pointer never outlives its command sequence: every EDID read at 0x50 that
//!   succeeds reads segment 0;
//! - the transcript's times never go backwards.

#![no_main]

use core::time::Duration;

use libfuzzer_sys::fuzz_target;
use maquette::{
    Actor, Bench, BlockBoundary, Clock, ColorBitDepth, ColorFormat, DdcBus, DdcCapabilities,
    DdcErrorKind, Delay, DuringHpdLow, EddcTarget, Effect, Event, Fault, Filter, FrlPhy,
    HdmiForumFrl, Line, MissingSegment, NoAcknowledgeSource, OpKind, OwnedWrite, PastStore,
    PhyCallKind, PhyFault, PhyPower, Point, Record, Refusal, RefusalKind, ScdcPastEnd, SinkConfig,
    Times, TmdsClockRatio, TmdsPhy, TmdsPhyConfig, TmdsVideo, Trigger,
};

/// The most actions one input runs.
const MAX_ACTIONS: usize = 256;

/// Two segments of EDID: segment 0 holds 0x00–0x7F, segment 1 holds 0x80–0xBF. A byte read
/// from segment 1 cannot pass for one of segment 0, and the 0xFF read past a store is
/// neither.
static EDID: [u8; 512] = {
    let mut store = [0u8; 512];
    let mut i = 0;
    while i < 512 {
        store[i] = if i < 256 {
            (i as u8) & 0x7F
        } else {
            0x80 | ((i as u8) & 0x3F)
        };
        i += 1;
    }
    store
};

static DISPLAYID: [u8; 128] = [0x5A; 128];

/// The addresses the actions use most: the pointer, DDC/CI, the EDID, the DisplayID, SCDC.
const ADDRS: [u8; 5] = [0x30, 0x37, 0x50, 0x52, 0x54];

/// The SCDC registers the sink owns.
fn sink_owned(reg: u8) -> bool {
    matches!(reg, 0x01 | 0x21 | 0x40..=0x42 | 0x50..=0x5A | 0xD0..=0xFF)
}

/// Reads the input a byte at a time; past its end, zeros.
struct Input<'d> {
    data: &'d [u8],
}

impl Input<'_> {
    fn byte(&mut self) -> u8 {
        match self.data.split_first() {
            Some((&first, rest)) => {
                self.data = rest;
                first
            }
            None => 0,
        }
    }

    fn bit(&mut self) -> bool {
        self.byte() & 1 == 1
    }

    fn addr(&mut self) -> u8 {
        let byte = self.byte();
        match ADDRS.get(usize::from(byte % 8)) {
            Some(&addr) => addr,
            None => byte >> 1,
        }
    }

    fn bytes<'b>(&mut self, buf: &'b mut [u8; 8]) -> &'b [u8] {
        let len = usize::from(self.byte() % 5);
        for slot in buf.iter_mut().take(len) {
            *slot = self.byte();
        }
        &buf[..len]
    }

    fn micros(&mut self) -> Duration {
        Duration::from_micros(u64::from(u16::from_le_bytes([self.byte(), self.byte()])))
    }

    fn line(&mut self) -> Line {
        match self.byte() % 8 {
            0 => Line::Off,
            1 => tmds(594_000, TmdsClockRatio::Div40, true),
            2 => tmds(594_000, TmdsClockRatio::Div40, false),
            3 => tmds(297_000, TmdsClockRatio::Div10, true),
            4 => tmds(297_000, TmdsClockRatio::Div10, false),
            // Ratios that do not fit the rate.
            5 => tmds(594_000, TmdsClockRatio::Div10, true),
            6 => tmds(297_000, TmdsClockRatio::Div40, true),
            _ => Line::Frl {
                rate: HdmiForumFrl::Rate6Gbps4Lanes,
            },
        }
    }

    fn config(&mut self) -> SinkConfig {
        let a = self.byte();
        let b = self.byte();
        let mut config = SinkConfig::new();
        if a & 0x01 != 0 {
            config = config.without_segment_pointer();
        }
        if a & 0x02 != 0 {
            config = config.with_strict_lock(true);
        }
        if a & 0x04 != 0 {
            config = config.with_owned_scdc_writes(OwnedWrite::Nack);
        }
        if a & 0x08 != 0 {
            config = config.with_scdc_past_end(ScdcPastEnd::Nack);
        }
        if a & 0x10 != 0 {
            config = config.with_missing_segment(MissingSegment::Ack);
        }
        if a & 0x20 != 0 {
            config = config.with_block_boundary(BlockBoundary::Wrap);
        }
        if a & 0x40 != 0 {
            config = config
                .with_scdc_during_hpd_low(DuringHpdLow::Silent)
                .with_edid_during_hpd_low(DuringHpdLow::Silent);
        }
        if a & 0x80 != 0 {
            config = config.with_enable_timeout(Some(Duration::from_millis(100)));
        }
        config = config.with_past_store(match b % 3 {
            0 => PastStore::Fill,
            1 => PastStore::Wrap,
            _ => PastStore::Nack,
        });
        config = config.with_scrambling_lock(match (b >> 2) % 3 {
            0 => Trigger::After(Duration::from_millis(1)),
            1 => Trigger::After(Duration::ZERO),
            _ => Trigger::AfterReads(u32::from((b >> 4) % 4)),
        });
        let kind = if b & 0x40 != 0 {
            RefusalKind::DropEnable
        } else {
            RefusalKind::WithholdStatus
        };
        config.with_scrambling_refusals(Refusal::new(kind, u32::from(b >> 7) * 2))
    }

    fn capabilities(&mut self) -> DdcCapabilities {
        let byte = self.byte();
        let piece = match byte % 4 {
            0 => None,
            1 => Some(1),
            2 => Some(32),
            _ => Some(128),
        };
        DdcCapabilities::general()
            .with_segmented_piece(piece)
            .with_max_segment(if byte & 0x04 != 0 {
                Some(0)
            } else {
                Some(0x7F)
            })
            .with_stretch_timeout_us(if byte & 0x08 != 0 { Some(2_000) } else { None })
    }

    fn fault(&mut self) -> Fault {
        let kinds = [
            DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Address),
            DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Data),
            DdcErrorKind::Timeout,
            DdcErrorKind::Busy,
            DdcErrorKind::Detached,
            DdcErrorKind::Other,
        ];
        let a = self.byte();
        let effect = match kinds.get(usize::from(a % 8)) {
            Some(&kind) => Effect::Fail(kind),
            None => Effect::Slow(Duration::from_micros(u64::from(a))),
        };
        let point = match (a >> 3) % 3 {
            0 => Point::Before,
            1 => Point::After,
            _ => Point::Partway {
                byte: u16::from(self.byte() % 8),
            },
        };
        let b = self.byte();
        let mut filter = Filter::ANY;
        if b & 0x01 != 0 {
            filter = filter.with_addr(self.addr());
        }
        if b & 0x02 != 0 {
            filter = filter.with_op(match self.byte() % 4 {
                0 => OpKind::Write,
                1 => OpKind::WriteRead,
                2 => OpKind::Read,
                _ => OpKind::ReadSegmented,
            });
        }
        if b & 0x04 != 0 {
            filter = filter.with_first(self.byte());
        }
        let times = if b & 0x08 != 0 {
            Times::UntilCleared
        } else {
            Times::Count(u32::from((b >> 4) % 4))
        };
        Fault::new(effect)
            .on(filter)
            .nth(u32::from(self.byte() % 4))
            .times(times)
            .at(point)
    }
}

/// A TMDS configuration: 594 000 kHz at 1/40, or 148 500 kHz at 1/10.
fn phy_config(high: bool) -> TmdsPhyConfig {
    let (rate, ratio) = if high {
        (594_000, TmdsClockRatio::Div40)
    } else {
        (148_500, TmdsClockRatio::Div10)
    };
    let video = TmdsVideo::new(rate, ColorBitDepth::Depth8, ColorFormat::Rgb444);
    TmdsPhyConfig::new(rate, ratio, video)
}

fn tmds(character_rate_khz: u32, ratio: TmdsClockRatio, scrambled: bool) -> Line {
    Line::Tmds {
        character_rate_khz,
        ratio,
        scrambled,
    }
}

/// Checks the invariants that hold after every action.
fn check_state(bench: &Bench<'_>) {
    let sink = bench.sink();
    let (Some(status), Some(config)) = (sink.scrambling_status(), sink.tmds_config()) else {
        return;
    };
    if status & 1 != 0 {
        assert!(config & 1 != 0, "status set without Scrambling_Enable");
        match bench.line() {
            Line::Tmds {
                character_rate_khz,
                ratio,
                scrambled: true,
            } => {
                let div40 = ratio == TmdsClockRatio::Div40;
                assert_eq!(div40, config & 2 != 0, "status set at the wrong ratio");
                assert_eq!(
                    div40,
                    character_rate_khz > 340_000,
                    "status set at a ratio that does not fit the rate"
                );
            }
            line => panic!("status set without scrambled TMDS: {line:?}"),
        }
    }
}

/// Checks that a successful plain read at 0x50 read segment 0.
fn check_edid_read(result: &Result<(), maquette::BusError>, addr: u8, buf: &[u8]) {
    if result.is_ok() && addr == 0x50 {
        assert!(
            !buf.iter().any(|byte| (0x80..=0xBF).contains(byte)),
            "a plain EDID read returned segment 1: {buf:02x?}"
        );
    }
}

/// Runs the input on a fresh bench and returns its transcript.
fn run(data: &[u8]) -> (Vec<Record>, usize) {
    let mut input = Input { data };
    let config = input.config();
    let displayid: Option<&'static [u8]> = if input.bit() { Some(&DISPLAYID) } else { None };
    let mut builder = Bench::builder(&EDID)
        .sink(config)
        .ddc_capabilities(input.capabilities())
        .budget(None);
    if let Some(store) = displayid {
        builder = builder.displayid(store);
    }
    let bench = builder.build().expect("the stores are whole blocks");
    let bus = bench.ddc();
    let clock = bench.time();
    let mut delay = bench.time();
    let mut phy = bench.phy();

    for _ in 0..MAX_ACTIONS {
        if input.data.is_empty() {
            break;
        }
        let mut bytes = [0u8; 8];
        let mut buf = [0u8; 64];
        match input.byte() % 21 {
            0 => {
                let addr = input.addr();
                let _ = bus.write(addr, input.bytes(&mut bytes));
            }
            1 => {
                let addr = input.addr();
                let written = input.bytes(&mut bytes);
                let len = usize::from(input.byte() % 9);
                let result = bus.write_read(addr, written, &mut buf[..len]);
                check_edid_read(&result, addr, &buf[..len]);
            }
            2 => {
                let addr = input.addr();
                let len = usize::from(input.byte() % 9);
                let result = bus.read(addr, &mut buf[..len]);
                check_edid_read(&result, addr, &buf[..len]);
            }
            3 => {
                let target = if input.bit() {
                    EddcTarget::DisplayId
                } else {
                    EddcTarget::Edid
                };
                let segment = input.byte() % 4;
                let offset = input.byte();
                let len = usize::from(input.byte() % 65);
                let _ = bus.read_segmented(target, segment, offset, &mut buf[..len]);
            }
            4 => bench.set_line(input.line()),
            5 => {
                let high = input.bit();
                // Raising HPD on an unplugged sink is a mistake the bench panics on.
                if !high || bench.sink().is_plugged() {
                    bench.hpd(high);
                }
            }
            6 => {
                let low_for = input.micros() * 4;
                if bench.sink().is_plugged() {
                    bench.hpd_pulse(low_for);
                }
            }
            7 => bench.unplug(),
            8 => {
                let config = input.config();
                bench
                    .plug(&EDID, displayid, config)
                    .expect("the stores are whole blocks");
            }
            9 => bench.detach(),
            10 => bench.reattach(),
            11 => bench.advance(input.micros()),
            12 => delay.delay(input.micros()),
            13 => {
                let reg = input.byte();
                let value = input.byte();
                // Presets of the status would set it outside the lock's conditions.
                if reg != 0x21 && bench.sink().scdc(0x01).is_some() {
                    bench.preset_scdc(reg, value);
                }
            }
            14 => {
                let _ = bench.schedule(input.fault());
            }
            15 => bench.clear_faults(),
            16 => {
                let _ = phy.configure(phy_config(input.bit()));
            }
            17 => {
                let _ = phy.set_scrambling(input.bit());
            }
            18 => {
                let _ = match input.byte() % 6 {
                    0 => phy.start_output(),
                    1 => phy.stop_output(),
                    2 => phy.power_on(),
                    3 => phy.power_off(),
                    4 => phy.set_frl_rate(HdmiForumFrl::Rate6Gbps4Lanes),
                    _ => phy.set_frl_rate(HdmiForumFrl::NotSupported),
                };
            }
            19 => {
                let kinds = [
                    PhyCallKind::Configure,
                    PhyCallKind::SetScrambling,
                    PhyCallKind::StartOutput,
                    PhyCallKind::StopOutput,
                ];
                let byte = input.byte();
                let mut fault = PhyFault::new().nth(u32::from(byte % 3));
                if let Some(&kind) = kinds.get(usize::from(byte >> 2) % 8) {
                    fault = fault.on(kind);
                }
                if byte & 0x80 != 0 {
                    fault = fault.after_effect();
                }
                let _ = bench.schedule_phy(fault);
            }
            _ => {
                let _ = clock.now();
            }
        }
        check_state(&bench);
    }

    let records: Vec<Record> = bench.transcript().collect();
    for record in &records {
        if let Event::Register {
            reg,
            by: Actor::Source,
            addr: 0x54,
            ..
        } = record.event
        {
            assert!(!sink_owned(reg), "the source changed register {reg:#04x}");
        }
    }
    assert!(
        records.windows(2).all(|pair| pair[0].at <= pair[1].at),
        "the transcript's times went backwards"
    );
    (records, bench.transcript_overflow())
}

fuzz_target!(|data: &[u8]| {
    let first = run(data);
    let second = run(data);
    assert_eq!(first, second, "the same input gave another transcript");
});
