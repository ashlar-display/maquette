//! Bench events: the line, HPD, unplug and plug, detach, delays, and the operation budget.

use core::time::Duration;

use maquette::hdmi_hal::ddc::{EDID, SCDC};
use maquette::{
    Bench, BenchError, Clock, DEFAULT_BUDGET, DdcBus, DdcErrorKind, Delay, DuringHpdLow,
    EddcTarget, Env, Event, HdmiForumFrl, Line, NoAcknowledgeSource, Operation, SinkConfig, Target,
    TmdsClockRatio,
};

const NACK_ADDRESS: DdcErrorKind = DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Address);
const TMDS_594: Line = Line::Tmds {
    character_rate_khz: 594_000,
    ratio: TmdsClockRatio::Div40,
    scrambled: true,
};

fn us(micros: u64) -> Duration {
    Duration::from_micros(micros)
}

/// The transcript's events other than bus records, with their times.
fn events(bench: &Bench<'_>) -> Vec<(u64, Event)> {
    bench
        .transcript()
        .filter(|record| !matches!(record.event, Event::Bus(_)))
        .map(|record| (record.at.as_micros(), record.event))
        .collect()
}

/// A target at 0x37 that notes every update with what it saw, and answers only with HPD high.
struct Watcher;

impl Target for Watcher {
    fn answers(&self, addr: u8, env: &Env<'_>) -> bool {
        addr == 0x37 && env.hpd()
    }
    fn transaction(
        &mut self,
        _ops: &mut [Operation<'_>],
        _env: &mut Env<'_>,
    ) -> Result<(), DdcErrorKind> {
        Ok(())
    }
    fn update(&mut self, env: &mut Env<'_>) {
        let line = match env.line() {
            Line::Off => 0,
            Line::Tmds {
                character_rate_khz, ..
            } => character_rate_khz,
            _ => 1,
        };
        env.note(0x37, u32::from(env.hpd()), line);
    }
}

fn note(hpd: bool, line: u32) -> Event {
    Event::Note {
        addr: 0x37,
        code: u32::from(hpd),
        value: line,
    }
}

#[test]
fn the_line_starts_off_and_changes_are_events() {
    let mut watcher = Watcher;
    let mut targets: [&mut (dyn Target + Send); 1] = [&mut watcher];
    let bench = Bench::builder(&[]).targets(&mut targets).build().unwrap();
    assert_eq!(bench.line(), Line::Off);
    bench.advance(us(10));
    bench.set_line(TMDS_594);
    // The same line again is not a change.
    bench.set_line(TMDS_594);
    let frl = Line::Frl {
        rate: HdmiForumFrl::Rate12Gbps4Lanes,
    };
    bench.set_line(frl);
    assert_eq!(bench.line(), frl);
    assert_eq!(
        events(&bench),
        [
            (10, note(true, 0)),
            (10, Event::Line(TMDS_594)),
            (10, note(true, 594_000)),
            (10, Event::Line(frl)),
            (10, note(true, 1)),
        ]
    );
}

#[test]
fn a_delay_on_the_time_handle_is_an_event() {
    let mut watcher = Watcher;
    let mut targets: [&mut (dyn Target + Send); 1] = [&mut watcher];
    let bench = Bench::builder(&[]).targets(&mut targets).build().unwrap();
    let mut time = bench.time();
    time.delay(us(250));
    assert_eq!(events(&bench), [(250, note(true, 0))]);
}

#[test]
fn hpd_changes_are_events_and_the_built_in_targets_answer_while_it_is_low() {
    let edid = [0x11u8; 128];
    let bench = Bench::new(SinkConfig::new(), &edid).unwrap();
    assert!(bench.sink().hpd());
    bench.hpd(false);
    bench.hpd(false);
    assert!(!bench.sink().hpd());
    let mut buf = [0u8; 2];
    bench
        .ddc()
        .read_segmented(EddcTarget::Edid, 0, 0, &mut buf)
        .unwrap();
    assert_eq!(buf, [0x11; 2]);
    bench
        .ddc()
        .write_read(SCDC, &[0x01], &mut buf[..1])
        .unwrap();
    assert_eq!(buf[0], 1);
    bench.hpd(true);
    let hpd: Vec<_> = events(&bench).into_iter().map(|(_, e)| e).collect();
    assert_eq!(hpd, [Event::Hpd { high: false }, Event::Hpd { high: true }]);
}

#[test]
fn built_in_targets_can_be_silent_while_hpd_is_low() {
    let edid = [0u8; 128];
    let config = SinkConfig::new()
        .with_scdc_during_hpd_low(DuringHpdLow::Silent)
        .with_edid_during_hpd_low(DuringHpdLow::Silent);
    assert_eq!(config.scdc_during_hpd_low(), DuringHpdLow::Silent);
    assert_eq!(config.edid_during_hpd_low(), DuringHpdLow::Silent);
    let bench = Bench::new(config, &edid).unwrap();
    let bus = bench.ddc();
    bus.write(SCDC, &[0x20, 0x03]).unwrap();
    bench.hpd(false);
    assert_eq!(
        bus.write(SCDC, &[0x20, 0x00]).unwrap_err().kind,
        NACK_ADDRESS
    );
    assert_eq!(bus.read(EDID, &mut [0]).unwrap_err().kind, NACK_ADDRESS);
    assert_eq!(bus.write(0x30, &[1]).unwrap_err().kind, NACK_ADDRESS);
    bench.hpd(true);
    bus.write(SCDC, &[0x20, 0x00]).unwrap();
    assert_eq!(bench.sink().tmds_config(), Some(0));
    let defaults = SinkConfig::new();
    assert_eq!(defaults.scdc_during_hpd_low(), DuringHpdLow::Answers);
    assert_eq!(defaults.edid_during_hpd_low(), DuringHpdLow::Answers);
}

#[test]
fn a_tests_target_decides_with_hpd() {
    let mut watcher = Watcher;
    let mut targets: [&mut (dyn Target + Send); 1] = [&mut watcher];
    let bench = Bench::builder(&[]).targets(&mut targets).build().unwrap();
    bench.ddc().write(0x37, &[0]).unwrap();
    bench.hpd(false);
    assert_eq!(
        bench.ddc().write(0x37, &[0]).unwrap_err().kind,
        NACK_ADDRESS
    );
}

#[test]
fn an_hpd_pulse_is_low_for_its_duration() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    bench.advance(us(5));
    bench.hpd_pulse(Duration::from_millis(500));
    assert_eq!(
        events(&bench),
        [
            (5, Event::Hpd { high: false }),
            (500_005, Event::Hpd { high: true }),
        ]
    );
}

#[test]
#[should_panic(expected = "HPD raised on an unplugged sink: plug it first")]
fn hpd_cannot_rise_on_an_unplugged_sink() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    bench.unplug();
    bench.hpd(true);
}

#[test]
fn while_unplugged_nothing_answers() {
    let edid = [0u8; 128];
    let mut watcher = Watcher;
    let mut targets: [&mut (dyn Target + Send); 1] = [&mut watcher];
    let bench = Bench::builder(&edid).targets(&mut targets).build().unwrap();
    bench.unplug();
    bench.unplug();
    assert!(!bench.sink().is_plugged());
    assert!(!bench.sink().hpd());
    let bus = bench.ddc();
    for addr in [0x37, EDID, SCDC, 0x30] {
        assert_eq!(bus.write(addr, &[0]).unwrap_err().kind, NACK_ADDRESS);
    }
    // Each charged its address byte only.
    assert_eq!(bench.now().as_micros(), 4 * 90);
    let changes: Vec<_> = events(&bench)
        .into_iter()
        .map(|(_, e)| e)
        .filter(|e| !matches!(e, Event::Note { .. }))
        .collect();
    assert_eq!(
        changes,
        [Event::Plug { plugged: false }, Event::Hpd { high: false }]
    );
    // An HPD low on an unplugged sink is no change.
    bench.hpd(false);
}

#[test]
fn plug_brings_a_fresh_sink_back() {
    let first = [0x11u8; 128];
    let second = [0x22u8; 256];
    let bench = Bench::new(SinkConfig::new(), &first).unwrap();
    bench.preset_scdc(0x20, 0x03);
    bench.unplug();
    bench
        .plug(&second, None, SinkConfig::new().with_sink_version(2))
        .unwrap();
    assert!(bench.sink().is_plugged());
    assert!(bench.sink().hpd());
    let mut buf = [0u8; 2];
    bench
        .ddc()
        .read_segmented(EddcTarget::Edid, 0, 0x80, &mut buf)
        .unwrap();
    assert_eq!(buf, [0x22; 2]);
    assert_eq!(bench.sink().tmds_config(), Some(0));
    assert_eq!(bench.sink().sink_version(), Some(2));
    let plugs: Vec<_> = events(&bench)
        .into_iter()
        .map(|(_, e)| e)
        .filter(|e| !matches!(e, Event::Register { .. }))
        .collect();
    assert_eq!(
        plugs,
        [
            Event::Plug { plugged: false },
            Event::Hpd { high: false },
            Event::Plug { plugged: true },
            Event::Hpd { high: true },
        ]
    );
}

#[test]
fn plugging_a_plugged_sink_unplugs_it_first() {
    let edid = [0u8; 128];
    let displayid = [0x5Au8; 128];
    let bench = Bench::new(SinkConfig::new(), &edid).unwrap();
    bench
        .plug(&edid, Some(&displayid), SinkConfig::new().without_scdc())
        .unwrap();
    assert_eq!(events(&bench).len(), 4);
    assert_eq!(bench.sink().scdc(0x01), None);
    let mut buf = [0u8; 1];
    bench
        .ddc()
        .read_segmented(EddcTarget::DisplayId, 0, 0, &mut buf)
        .unwrap();
    assert_eq!(buf, [0x5A]);
}

#[test]
fn a_bad_store_is_refused_and_nothing_changes() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    bench.unplug();
    assert_eq!(
        bench.plug(&[0; 5], None, SinkConfig::new()),
        Err(BenchError::EdidLength(5))
    );
    assert_eq!(
        bench.plug(&[], Some(&[0; 3]), SinkConfig::new()),
        Err(BenchError::DisplayIdLength(3))
    );
    assert!(!bench.sink().is_plugged());
}

#[test]
fn while_detached_every_call_fails_and_takes_no_time() {
    let edid = [0u8; 128];
    let mut watcher = Watcher;
    let mut targets: [&mut (dyn Target + Send); 1] = [&mut watcher];
    let bench = Bench::builder(&edid).targets(&mut targets).build().unwrap();
    bench.detach();
    bench.detach();
    assert!(!bench.is_attached());
    let bus = bench.ddc();
    let mut buf = [0u8; 2];
    let error = bus.write_read(SCDC, &[0x01], &mut buf).unwrap_err();
    assert_eq!(error.kind, DdcErrorKind::Detached);
    assert_eq!(buf, [0xA5; 2]);
    assert_eq!(
        bus.read_segmented(EddcTarget::Edid, 0, 0, &mut buf)
            .unwrap_err()
            .kind,
        DdcErrorKind::Detached
    );
    // The argument checks still come first.
    assert_eq!(
        bus.write(SCDC, &[]).unwrap_err().kind,
        DdcErrorKind::Unsupported
    );
    assert_eq!(bench.now().as_micros(), 0);
    let results: Vec<_> = bench
        .transcript()
        .filter_map(|record| match record.event {
            Event::Bus(bus) => Some(bus.result),
            _ => None,
        })
        .collect();
    assert_eq!(
        results,
        [
            Err(DdcErrorKind::Detached),
            Err(DdcErrorKind::Detached),
            Err(DdcErrorKind::Unsupported),
        ]
    );
    bench.reattach();
    assert!(bench.is_attached());
    bus.write(0x37, &[0]).unwrap();
    let adapter: Vec<_> = events(&bench)
        .into_iter()
        .map(|(_, e)| e)
        .filter(|e| matches!(e, Event::Adapter { .. }))
        .collect();
    assert_eq!(
        adapter,
        [
            Event::Adapter { attached: false },
            Event::Adapter { attached: true },
        ]
    );
}

#[test]
fn the_budget_counts_every_operation_through_the_handles() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    let time = bench.time();
    let mut delay = bench.time();
    let _ = time.now();
    delay.delay(us(1));
    let _ = bench.ddc().write(SCDC, &[]); // refused, still counted
    bench.ddc().write(SCDC, &[0x20, 0]).unwrap();
    bench.ddc().write_read(SCDC, &[], &mut [0]).unwrap(); // one read, counted once
    bench
        .ddc()
        .read_segmented(EddcTarget::Edid, 0, 0, &mut [])
        .unwrap();
    // The test's own reads and controls are not operations of the code under test.
    let _ = bench.now();
    bench.advance(us(1));
    let _ = bench.sink().tmds_config();
    assert_eq!(bench.operations(), 6);
    assert!(format!("{bench:?}").contains("budget: Some(1000000)"));
    assert_eq!(DEFAULT_BUDGET, 1_000_000);
}

#[test]
#[should_panic(expected = "the bench's operation budget of 5 operations is spent")]
fn a_loop_that_never_ends_spends_the_budget() {
    let bench = Bench::builder(&[]).budget(Some(5)).build().unwrap();
    let time = bench.time();
    // Waits for a time that never comes: the clock does not move without a delay.
    let deadline = maquette::Timestamp::from_micros(1);
    while time.now() < deadline {}
}

#[test]
fn without_a_budget_nothing_is_counted_against() {
    let bench = Bench::builder(&[]).budget(None).build().unwrap();
    let time = bench.time();
    for _ in 0..10 {
        let _ = time.now();
    }
    assert_eq!(bench.operations(), 10);
}

#[test]
fn debug_shows_the_events_state() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    bench.set_line(TMDS_594);
    let text = format!("{bench:?}");
    assert!(
        text.contains("line: Tmds { character_rate_khz: 594000"),
        "{text}"
    );
    assert!(
        text.contains("hpd: true, plugged: true, attached: true"),
        "{text}"
    );
}
