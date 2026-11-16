//! Drives a bench with arbitrary sequences of bus calls, PHY calls, line changes, HPD, plug
//! and adapter changes, presets, faults and clock advances, on a sink whose settings and bus capabilities
//! the input chooses, and checks the invariants a simulator must keep after every action:
//!
//! - the same input gives the same transcript (each input runs twice);
//! - nothing panics and no arithmetic overflows (the budget is off; the input bounds the
//!   actions);
//! - the status flags follow the line, and the character error counters keep their checksum
//!   and are valid exactly while their lanes are locked (no action presets either);
//! - `Scrambling_Status` is set only while `Scrambling_Enable` is set and the line is
//!   scrambled TMDS at the matching ratio, and that ratio fits the rate (no action presets
//!   the status);
//! - once the lock's conditions have held for a time trigger, the status is set (for sinks
//!   neither strict nor refusing, checked after an advance or a delay);
//! - no `Register` event changes a register the sink owns by `Actor::Source`;
//! - the segment pointer never outlives its command sequence: every EDID read at 0x50 that
//!   succeeds reads segment 0;
//! - the transcript's times never go backwards.

#![no_main]

use core::time::Duration;

use libfuzzer_sys::fuzz_target;
use maquette::{
    Action, Actor, Bench, BlockBoundary, CedRead, CedUpdate, Clock, ColorBitDepth, ColorFormat,
    DdcBus, DdcCapabilities, DdcErrorKind, Delay, DuringHpdLow, DuringRead, EddcTarget, Effect,
    ErrorRates, Event, Fault, Filter, FrlOutput, FrlPhy, HdmiForumFrl, HpdReset, LanePatterns,
    Line, LtpPattern, MissingSegment, NoAcknowledgeSource, OpKind, OwnedWrite, PastStore,
    PhyCallKind, PhyFault, PhyPower, Point, Record, Refusal, RefusalKind, RegisterGroup,
    ScdcPastEnd, SinkConfig, StatusUpdate, Times, Timestamp, TmdsClockRatio, TmdsPhy,
    TmdsPhyConfig, TmdsVideo, Trigger, When,
};

/// The most actions one input runs.
const MAX_ACTIONS: usize = 256;

/// The fuzz bench's transcript, so the invariants see every record: an input's actions stop
/// once fewer than [`ROOM`] records are left, and `run` asserts none overflowed.
const LOG: usize = 4096;

/// More records than one action makes: the most is a segmented read in 64 one-byte pieces,
/// a bus record each, with the events of the faults and scheduled actions it sets off.
const ROOM: usize = 512;

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
    matches!(reg, 0x01 | 0x21 | 0x35 | 0x40..=0x42 | 0x50..=0x5A | 0xD0..=0xDD)
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
        match self.byte() % 9 {
            0 => Line::Off,
            1 => tmds(594_000, TmdsClockRatio::Div40, true),
            2 => tmds(594_000, TmdsClockRatio::Div40, false),
            3 => tmds(297_000, TmdsClockRatio::Div10, true),
            4 => tmds(297_000, TmdsClockRatio::Div10, false),
            // Ratios that do not fit the rate.
            5 => tmds(594_000, TmdsClockRatio::Div10, true),
            6 => tmds(297_000, TmdsClockRatio::Div40, true),
            7 => Line::frl(HdmiForumFrl::Rate6Gbps4Lanes),
            _ => Line::frl_with(
                HdmiForumFrl::Rate6Gbps4Lanes,
                LanePatterns {
                    lane0: Some(LtpPattern::Lfsr0),
                    lane1: Some(LtpPattern::Lfsr1),
                    lane2: Some(LtpPattern::Lfsr2),
                    lane3: None,
                },
                FrlOutput::GapOnly,
            ),
        }
    }

    /// Error rates from none to past saturating a counter within a second.
    fn errors(&mut self) -> ErrorRates {
        let rate = |byte: u8| match byte % 4 {
            0 => 0,
            1 => 1,
            2 => 1_000 * u32::from(byte),
            _ => u32::MAX,
        };
        ErrorRates {
            lane0: rate(self.byte()),
            lane1: rate(self.byte()),
            lane2: rate(self.byte()),
            lane3: rate(self.byte()),
        }
    }

    fn config(&mut self) -> SinkConfig {
        let a = self.byte();
        let b = self.byte();
        // Two bits for each group's HPD reset (bits 0-9), then the update flags' rules.
        let c = u16::from_le_bytes([self.byte(), self.byte()]);
        let mut config = SinkConfig::new();
        // Each group's HPD reset: the default (on any low), never, as HPD rises, or after
        // 50 ms.
        for (i, &group) in RegisterGroup::ALL.iter().enumerate() {
            let reset = match (c >> (2 * i)) & 0b11 {
                0 => continue,
                1 => HpdReset::Never,
                2 => HpdReset::OnHpdRise,
                _ => HpdReset::After(Duration::from_millis(50)),
            };
            config = config.with_hpd_reset(group, reset);
        }
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
            config = config.with_enable_timeout(None);
        }
        config = config.with_ced_update(match (c >> 10) % 4 {
            0 => config.ced_update(),
            1 => CedUpdate::Never,
            2 => CedUpdate::EverySecond { rise: 0 },
            _ => CedUpdate::EverySecond { rise: 1_000 },
        });
        if c & (1 << 12) != 0 {
            config = config.with_status_update(StatusUpdate::Never);
        }
        if c & (1 << 13) != 0 {
            config = config.with_ced_read(CedRead::ClearOnRead);
        }
        if c & (1 << 14) != 0 {
            config = config.with_during_read(DuringRead::Moving);
        }
        config = config.with_past_store(if b % 2 == 0 {
            PastStore::Fill
        } else {
            PastStore::Wrap
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

    /// A filter, the fields `b` sets read from the input.
    fn filter(&mut self, b: u8) -> Filter {
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
        filter
    }

    /// A scheduled action and its moment. No plug: it would change the sink's settings at a
    /// moment `check_lock_comes` does not know. No preset of the status or the status flags,
    /// as for presets.
    fn action(&mut self, now: Timestamp) -> (Action<'static>, When) {
        let a = self.byte();
        let action = match a % 7 {
            0 => {
                let reg = self.byte();
                Action::Preset {
                    reg: if matches!(reg, 0x21 | 0x40 | 0x50..=0x56) {
                        0x20
                    } else {
                        reg
                    },
                    value: self.byte(),
                }
            }
            1 => Action::Hpd(self.bit()),
            2 => Action::Unplug,
            3 => Action::Line(self.line()),
            4 => Action::Detach,
            5 => Action::CharacterErrors(self.errors()),
            _ => Action::Reattach,
        };
        let nth = u32::from(self.byte() % 4);
        let when = match (a >> 3) % 4 {
            0 => {
                let b = self.byte();
                When::BeforeOp {
                    filter: self.filter(b),
                    nth,
                }
            }
            1 => {
                let b = self.byte();
                When::AfterOp {
                    filter: self.filter(b),
                    nth,
                }
            }
            2 => When::PhyCall { kind: None, nth },
            _ => When::At(now.checked_add(self.micros()).unwrap_or(now)),
        };
        (action, when)
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
            None if a % 8 == 6 => Effect::Slow(Duration::from_micros(u64::from(a))),
            None => Effect::Corrupt {
                byte: u16::from(self.byte() % 16),
                xor: self.byte() | 1,
            },
        };
        let point = match (a >> 3) % 3 {
            0 => Point::Before,
            1 => Point::After,
            _ => Point::Partway {
                byte: u16::from(self.byte() % 8),
            },
        };
        let b = self.byte();
        let filter = self.filter(b);
        let times = match (b & 0x08 != 0, b & 0x80 != 0) {
            (true, false) => Times::UntilCleared,
            (true, true) => Times::For(self.micros()),
            _ => Times::Count(u32::from((b >> 4) % 4)),
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
fn check_state(bench: &Bench<'_, LOG>) {
    let sink = bench.sink();
    check_flags(bench);
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

/// Checks the status flags against what the sink receives: `Clock_Detected` only on TMDS,
/// the lane locks only on a line it can decode.
fn check_flags(bench: &Bench<'_, LOG>) {
    let sink = bench.sink();
    let (Some(flags), Some(config)) = (sink.status_flags_0(), sink.tmds_config()) else {
        return;
    };
    // The counters: their checksum, and valid exactly while the lanes are locked.
    let counters: Vec<u8> = (0x50..=0x56).filter_map(|reg| sink.scdc(reg)).collect();
    let sum = counters
        .iter()
        .fold(0u8, |sum, &byte| sum.wrapping_add(byte));
    assert_eq!(sum, 0, "the counters' checksum is wrong: {counters:02x?}");
    for high in [counters[1], counters[3], counters[5]] {
        assert_eq!(
            high & 0x80 != 0,
            flags & 0x0E != 0,
            "a counter valid without its lane locked, or not with it"
        );
    }
    let line = if sink.is_plugged() {
        bench.line()
    } else {
        Line::Off
    };
    let tmds = matches!(line, Line::Tmds { .. });
    assert!(
        flags & 1 == 0 || tmds,
        "clock detected without TMDS: {line:?}"
    );
    if flags & 0x0E != 0 {
        assert_eq!(flags & 0x0E, 0x0E, "lanes 0-2 lock together");
        let Line::Tmds {
            character_rate_khz,
            ratio,
            scrambled,
        } = line
        else {
            panic!("lanes locked without TMDS: {line:?}");
        };
        let div40 = ratio == TmdsClockRatio::Div40;
        let high = character_rate_khz > 340_000;
        assert!(flags & 1 != 0, "lanes locked without the clock");
        assert_eq!(
            div40, high,
            "lanes locked at a ratio that does not fit the rate"
        );
        assert_eq!(div40, config & 2 != 0, "lanes locked at the wrong ratio");
        assert_eq!(
            scrambled,
            config & 1 != 0,
            "lanes locked against Scrambling_Enable"
        );
        assert!(
            scrambled || !high,
            "lanes locked on unscrambled TMDS above 340 Mcsc"
        );
    }
}

/// Checks that the sink is locked once the lock's conditions have held for the trigger: for
/// the sinks whose conditions the transcript shows (a time trigger, not strict, no
/// refusals), after an advance or a delay, at which the sink evaluates. The conditions may
/// have begun to hold at a change of `TMDS_Config`'s two bits, a change of the line, a plug,
/// any change of HPD (a long low resets the status, recorded only if it changes it) or the
/// sink's own clearing of the status; while HPD is low a reset may still come.
fn check_lock_comes(bench: &Bench<'_, LOG>, config: &SinkConfig) {
    let Trigger::After(delay) = config.scrambling_lock() else {
        return;
    };
    if config.strict_lock() || config.scrambling_refusals().attempts > 0 {
        return;
    }
    let sink = bench.sink();
    let (Some(status), Some(tmds_config), true) =
        (sink.scrambling_status(), sink.tmds_config(), sink.hpd())
    else {
        return;
    };
    let holds = tmds_config & 1 != 0
        && match bench.line() {
            Line::Tmds {
                character_rate_khz,
                ratio,
                scrambled: true,
            } => {
                let div40 = ratio == TmdsClockRatio::Div40;
                div40 == (character_rate_khz > 340_000) && div40 == (tmds_config & 2 != 0)
            }
            _ => false,
        };
    let since = bench
        .transcript()
        .filter_map(|record| match record.event {
            Event::Register {
                addr: 0x54,
                reg: 0x20,
                from,
                to,
                ..
            } if (from ^ to) & 0b11 != 0 => Some(record.at),
            Event::Register {
                addr: 0x54,
                reg: 0x21,
                from,
                to,
                by: Actor::Sink,
                ..
            } if from & 1 != 0 && to & 1 == 0 => Some(record.at),
            Event::Line(_) | Event::Plug { .. } | Event::Hpd { .. } => Some(record.at),
            _ => None,
        })
        .max();
    let (true, Some(since)) = (holds, since) else {
        return;
    };
    if since
        .checked_add(delay)
        .is_some_and(|due| bench.now() >= due)
    {
        assert!(
            status & 1 != 0,
            "the conditions held since {since:?} for the trigger, but the status is not set"
        );
    }
}

/// Checks that a successful plain read at 0x50 read segment 0.
/// A read a `Corrupt` fault struck is not checked: its bytes are garbled on purpose. The
/// fault is recorded after the read's bus record, the last one, with a scheduled action's
/// records possibly after it.
fn check_edid_read(
    bench: &Bench<'_, LOG>,
    result: &Result<(), maquette::BusError>,
    addr: u8,
    buf: &[u8],
) {
    let records: Vec<Record> = bench.transcript().collect();
    let read = records
        .iter()
        .rposition(|record| matches!(record.event, Event::Bus(_)));
    let corrupted = read.is_some_and(|read| {
        records[read..].iter().any(|record| {
            matches!(
                record.event,
                Event::Fault {
                    effect: Effect::Corrupt { .. },
                    ..
                }
            )
        })
    });
    if result.is_ok() && addr == 0x50 && !corrupted {
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
        .capacities::<LOG, 16>()
        .sink(config)
        .ddc_capabilities(input.capabilities())
        .budget(None);
    if let Some(store) = displayid {
        builder = builder.displayid(store);
    }
    let bench = builder.build().expect("the stores are whole blocks");
    let mut sink = config;
    let bus = bench.ddc();
    let clock = bench.time();
    let mut delay = bench.time();
    let mut phy = bench.phy();

    for _ in 0..MAX_ACTIONS {
        if input.data.is_empty() || bench.transcript_len() > LOG - ROOM {
            break;
        }
        let mut bytes = [0u8; 8];
        let mut buf = [0u8; 64];
        match input.byte() % 23 {
            0 => {
                let addr = input.addr();
                let _ = bus.write(addr, input.bytes(&mut bytes));
            }
            1 => {
                let addr = input.addr();
                let written = input.bytes(&mut bytes);
                let len = usize::from(input.byte() % 9);
                let result = bus.write_read(addr, written, &mut buf[..len]);
                check_edid_read(&bench, &result, addr, &buf[..len]);
            }
            2 => {
                let addr = input.addr();
                let len = usize::from(input.byte() % 9);
                let result = bus.read(addr, &mut buf[..len]);
                check_edid_read(&bench, &result, addr, &buf[..len]);
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
                    bench.set_hpd(high);
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
                sink = config;
            }
            9 => bench.detach(),
            10 => bench.reattach(),
            11 => {
                bench.advance(input.micros());
                check_lock_comes(&bench, &sink);
            }
            12 => {
                delay.delay(input.micros());
                check_lock_comes(&bench, &sink);
            }
            13 => {
                let reg = input.byte();
                let value = input.byte();
                // Presets of the status, the flags or the counters would set them outside
                // their conditions.
                if !matches!(reg, 0x21 | 0x40 | 0x50..=0x56) && bench.sink().scdc(0x01).is_some() {
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
            20 => {
                let (action, when) = input.action(bench.now());
                let _ = bench.schedule_action(action, when);
            }
            21 => bench.set_character_errors(input.errors()),
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
    assert_eq!(
        bench.transcript_overflow(),
        0,
        "the fuzz bench's transcript must keep every record"
    );
    (records, bench.transcript_overflow())
}

fuzz_target!(|data: &[u8]| {
    let first = run(data);
    let second = run(data);
    assert_eq!(first, second, "the same input gave another transcript");
});
