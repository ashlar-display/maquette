//! Scheduled actions: bench events the bench makes by itself during a procedure.

use core::time::Duration;

use maquette::hdmi_hal::ddc::SCDC;
use maquette::{
    Action, Bench, ColorBitDepth, ColorFormat, Coupled, DdcBus, DdcErrorKind, Effect, Event, Fault,
    Filter, Line, NoAcknowledgeSource, PhyCallKind, Point, ScheduleFull, SinkConfig, Timestamp,
    TmdsClockRatio, TmdsPhy, TmdsPhyConfig, TmdsVideo, When,
};

/// What these tests compare of a record: the transcript's variants are matched, not built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seen {
    Bus(Result<(), DdcErrorKind>),
    Action,
    Register(u8, u8),
    Hpd(bool),
    Plug(bool),
    Adapter(bool),
    Line,
    Phy(PhyCallKind),
    Fault,
}

fn seen(bench: &Bench<'_>) -> Vec<Seen> {
    bench
        .transcript()
        .filter_map(|record| {
            Some(match record.event {
                Event::Bus(bus) => Seen::Bus(bus.result),
                Event::Action { .. } => Seen::Action,
                Event::Register { reg, to, .. } => Seen::Register(reg, to),
                Event::Hpd { high, .. } => Seen::Hpd(high),
                Event::Plug { plugged, .. } => Seen::Plug(plugged),
                Event::Adapter { attached, .. } => Seen::Adapter(attached),
                Event::Line(_) => Seen::Line,
                Event::Phy { call, .. } => Seen::Phy(call.kind()),
                Event::Fault { .. } => Seen::Fault,
                _ => return None,
            })
        })
        .collect()
}

fn bench() -> Bench<'static> {
    Bench::new(SinkConfig::new(), &[]).unwrap()
}

/// Reads `Update_0`.
fn update_0(bench: &Bench<'_>) -> u8 {
    let mut value = [0];
    bench.ddc().write_read(SCDC, &[0x10], &mut value).unwrap();
    value[0]
}

const UPDATE_0_READS: Filter = Filter::ANY.with_addr(SCDC).with_first(0x10);

#[test]
fn an_action_before_an_operation_is_seen_by_it() {
    let bench = bench();
    // CED_update set on the third poll of Update_0.
    bench
        .schedule_action(
            Action::Preset {
                reg: 0x10,
                value: 0x02,
            },
            When::BeforeOp {
                filter: UPDATE_0_READS,
                nth: 3,
            },
        )
        .unwrap();
    assert_eq!([update_0(&bench), update_0(&bench)], [0, 0]);
    assert_eq!(update_0(&bench), 0x02);
    assert_eq!(
        seen(&bench),
        [
            Seen::Bus(Ok(())),
            Seen::Bus(Ok(())),
            Seen::Action,
            Seen::Register(0x10, 0x02),
            Seen::Bus(Ok(())),
        ]
    );
    // It came once and left the schedule.
    assert_eq!(bench.scheduled_faults(), 0);
    assert_eq!(update_0(&bench), 0x02);
}

#[test]
fn an_action_after_an_operation_follows_its_record() {
    let bench = bench();
    bench
        .schedule_action(
            Action::Hpd(false),
            When::AfterOp {
                filter: Filter::ANY.with_addr(SCDC),
                nth: 1,
            },
        )
        .unwrap();
    bench.ddc().write(SCDC, &[0x20, 0x03]).unwrap();
    assert!(!bench.sink().hpd());
    assert_eq!(
        seen(&bench),
        [
            Seen::Bus(Ok(())),
            Seen::Register(0x20, 0x03),
            Seen::Action,
            Seen::Hpd(false),
            // TMDS_Config resets as HPD falls, by default.
            Seen::Register(0x20, 0),
        ]
    );
}

fn high() -> TmdsPhyConfig {
    let video = TmdsVideo::new(594_000, ColorBitDepth::Depth8, ColorFormat::Rgb444);
    TmdsPhyConfig::new(594_000, TmdsClockRatio::Div40, video)
}

#[test]
fn an_action_after_a_phy_call_comes_as_it_returns() {
    // An unplug as the output starts, from the simulated PHY and from a coupled one.
    let bench = bench();
    bench
        .schedule_action(
            Action::Unplug,
            When::PhyCall {
                kind: Some(PhyCallKind::StartOutput),
                nth: 1,
            },
        )
        .unwrap();
    let mut phy = bench.phy();
    phy.configure(high()).unwrap();
    assert!(bench.sink().is_plugged());
    phy.start_output().unwrap();
    assert!(!bench.sink().is_plugged());
    let tail: Vec<_> = seen(&bench).into_iter().skip(1).collect();
    assert_eq!(
        tail,
        [
            Seen::Phy(PhyCallKind::StartOutput),
            Seen::Line,
            // The sink detects the clock, and says so.
            Seen::Register(0x40, 0x01),
            Seen::Register(0x10, 0x01),
            Seen::Action,
            Seen::Plug(false),
            Seen::Hpd(false),
            // The status resets as HPD falls; the unplugged sink receives nothing.
            Seen::Register(0x10, 0),
            Seen::Register(0x40, 0),
        ]
    );

    // A coupled PHY's calls count once each.
    let bench = self::bench();
    bench
        .schedule_action(Action::Unplug, When::PhyCall { kind: None, nth: 2 })
        .unwrap();
    let mut coupled = Coupled::new(Stub, &bench);
    coupled.configure(high()).unwrap();
    assert!(bench.sink().is_plugged());
    coupled.start_output().unwrap();
    assert!(!bench.sink().is_plugged());
}

/// A PHY that does as asked: no bench behind it.
struct Stub;

impl TmdsPhy for Stub {
    type Error = ();
    fn capabilities(&self) -> maquette::TmdsPhyCapabilities {
        maquette::DEFAULT_PHY_CAPABILITIES
    }
    fn configure(&mut self, _: TmdsPhyConfig) -> Result<(), ()> {
        Ok(())
    }
    fn set_scrambling(&mut self, _: bool) -> Result<(), ()> {
        Ok(())
    }
    fn start_output(&mut self) -> Result<(), ()> {
        Ok(())
    }
    fn stop_output(&mut self) -> Result<(), ()> {
        Ok(())
    }
}

#[test]
fn every_action_does_what_its_control_does() {
    static EDID: [u8; 128] = [0x22; 128];
    let bench = bench();
    let line = Line::Tmds {
        character_rate_khz: 594_000,
        ratio: TmdsClockRatio::Div40,
        scrambled: true,
    };
    // Each comes after the next PHY call: bus operations would not count while detached.
    let next_call = || When::PhyCall { kind: None, nth: 1 };
    let actions = [
        Action::Line(line),
        Action::Unplug,
        Action::Plug {
            edid: &EDID,
            displayid: None,
            config: SinkConfig::new().with_sink_version(2),
        },
        Action::Detach,
        Action::Reattach,
    ];
    for action in actions {
        bench.schedule_action(action, next_call()).unwrap();
        bench.phy().stop_output().unwrap();
    }
    assert_eq!(bench.line(), line);
    assert!(bench.sink().is_plugged());
    assert_eq!(bench.sink().sink_version(), Some(2));
    assert!(bench.is_attached());
    let changes: Vec<_> = seen(&bench)
        .into_iter()
        .filter(|seen| !matches!(seen, Seen::Phy(_) | Seen::Action))
        .collect();
    assert_eq!(
        changes,
        [
            Seen::Line,
            Seen::Register(0x40, 0x01),
            Seen::Register(0x10, 0x01),
            Seen::Plug(false),
            Seen::Hpd(false),
            Seen::Register(0x10, 0),
            Seen::Register(0x40, 0),
            Seen::Plug(true),
            Seen::Hpd(true),
            // The new sink detects the clock.
            Seen::Register(0x40, 0x01),
            Seen::Register(0x10, 0x01),
            Seen::Adapter(false),
            Seen::Adapter(true),
        ]
    );
}

#[test]
fn a_detach_before_an_operation_fails_it() {
    let bench = bench();
    bench
        .schedule_action(
            Action::Detach,
            When::BeforeOp {
                filter: Filter::ANY,
                nth: 2,
            },
        )
        .unwrap();
    bench.ddc().write(SCDC, &[0x20, 0x01]).unwrap();
    let error = bench.ddc().write(SCDC, &[0x20, 0x03]).unwrap_err();
    assert_eq!(error.kind, DdcErrorKind::Detached);
    assert_eq!(bench.sink().tmds_config(), Some(0x01));
}

#[test]
fn operations_while_detached_do_not_count() {
    let bench = bench();
    bench
        .schedule_action(
            Action::Preset {
                reg: 0x10,
                value: 0x01,
            },
            When::BeforeOp {
                filter: UPDATE_0_READS,
                nth: 1,
            },
        )
        .unwrap();
    bench.detach();
    let _ = bench.ddc().write_read(SCDC, &[0x10], &mut [0]);
    bench.reattach();
    assert_eq!(bench.scheduled_faults(), 1);
    assert_eq!(update_0(&bench), 0x01);
}

#[test]
fn an_action_that_cannot_apply_is_only_recorded() {
    // A preset on a sink without SCDC, and HPD raised on an unplugged sink.
    let bench = Bench::new(SinkConfig::new().without_scdc(), &[]).unwrap();
    let first = || When::AfterOp {
        filter: Filter::ANY,
        nth: 1,
    };
    bench
        .schedule_action(
            Action::Preset {
                reg: 0x20,
                value: 3,
            },
            first(),
        )
        .unwrap();
    bench.unplug();
    bench.schedule_action(Action::Hpd(true), first()).unwrap();
    let _ = bench.ddc().write(0x37, &[0]);
    assert!(!bench.sink().hpd());
    let tail: Vec<_> = seen(&bench).into_iter().skip(2).collect();
    assert_eq!(
        tail,
        [
            Seen::Bus(Err(DdcErrorKind::NoAcknowledge(
                NoAcknowledgeSource::Address
            ))),
            Seen::Action,
            Seen::Action,
        ]
    );
}

#[test]
fn actions_share_the_schedule_with_faults() {
    let bench = Bench::builder(&[]).capacities::<256, 1>().build().unwrap();
    let id = bench
        .schedule_action(Action::Unplug, When::PhyCall { kind: None, nth: 1 })
        .unwrap();
    assert_eq!(
        bench.schedule(Fault::new(Effect::Slow(core::time::Duration::ZERO))),
        Err(ScheduleFull)
    );
    // Cleared before it came, it never comes.
    assert!(bench.clear_fault(id));
    bench.phy().configure(high()).unwrap();
    assert!(bench.sink().is_plugged());
}

#[test]
fn a_fault_and_an_action_on_one_operation() {
    // The action comes before the operation, which the fault then fails before the sink.
    let bench = bench();
    let nack = DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Data);
    bench
        .schedule(Fault::new(Effect::Fail(nack)).at(Point::Before))
        .unwrap();
    bench
        .schedule_action(
            Action::Hpd(false),
            When::BeforeOp {
                filter: Filter::ANY,
                nth: 1,
            },
        )
        .unwrap();
    assert_eq!(
        bench.ddc().write(SCDC, &[0x20, 0x03]).unwrap_err().kind,
        nack
    );
    assert_eq!(
        seen(&bench),
        [
            Seen::Action,
            Seen::Hpd(false),
            Seen::Bus(Err(nack)),
            Seen::Fault
        ]
    );
}

#[test]
#[should_panic(expected = "a scheduled plug's stores")]
fn a_plug_with_broken_stores_is_refused_when_scheduled() {
    let bench = bench();
    let _ = bench.schedule_action(
        Action::Plug {
            edid: &[0; 5],
            displayid: None,
            config: SinkConfig::new(),
        },
        When::PhyCall { kind: None, nth: 1 },
    );
}

fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

#[test]
fn an_action_at_a_time_comes_at_the_first_event_at_or_after_it() {
    let bench = bench();
    bench
        .schedule_action(Action::Hpd(false), When::At(Timestamp::from_micros(5_000)))
        .unwrap();
    bench.advance(ms(3));
    assert!(bench.sink().hpd());
    bench.advance(ms(3));
    assert!(!bench.sink().hpd());
    // Recorded at the event that took it, at 6 ms.
    let action = bench
        .transcript()
        .find(|record| matches!(record.event, Event::Action { .. }))
        .unwrap();
    assert_eq!(action.at.as_micros(), 6_000);
}

#[test]
fn an_action_at_a_time_comes_before_a_bus_operation() {
    let bench = bench();
    bench
        .schedule_action(
            Action::Preset {
                reg: 0x10,
                value: 0x02,
            },
            When::At(Timestamp::from_micros(1_000)),
        )
        .unwrap();
    // Each poll takes time on the bus; the first at or after 1 ms sees the flag.
    let mut polls = 0;
    while update_0(&bench) == 0 {
        polls += 1;
    }
    assert!(polls > 0);
    let records: Vec<_> = bench.transcript().collect();
    let action = records
        .iter()
        .position(|record| matches!(record.event, Event::Action { .. }))
        .unwrap();
    assert!(records[action].at.as_micros() >= 1_000);
    assert!(matches!(records[action + 2].event, Event::Bus(_)));
    assert_eq!(records[action + 2].at, records[action].at);
}

#[test]
fn an_action_at_a_past_time_comes_at_the_next_event() {
    let bench = bench();
    bench.advance(ms(10));
    bench
        .schedule_action(Action::Unplug, When::At(Timestamp::from_micros(0)))
        .unwrap();
    assert!(bench.sink().is_plugged());
    bench.advance(Duration::ZERO);
    assert!(!bench.sink().is_plugged());
}

#[test]
fn an_update_inside_an_operation_takes_none_of_its_actions() {
    // The HPD low before the write is a bench update; the preset after it waits for the
    // write's transaction.
    let bench = bench();
    let first = Filter::ANY.with_addr(SCDC);
    bench
        .schedule_action(
            Action::Hpd(false),
            When::BeforeOp {
                filter: first,
                nth: 1,
            },
        )
        .unwrap();
    bench
        .schedule_action(
            Action::Preset {
                reg: 0x10,
                value: 0x01,
            },
            When::AfterOp {
                filter: first,
                nth: 1,
            },
        )
        .unwrap();
    bench.ddc().write(SCDC, &[0x02, 0x01]).unwrap();
    assert_eq!(
        seen(&bench),
        [
            Seen::Action,
            Seen::Hpd(false),
            Seen::Bus(Ok(())),
            Seen::Register(0x02, 0x01),
            Seen::Action,
            Seen::Register(0x10, 0x01),
        ]
    );
}

#[test]
fn an_unplug_during_an_hpd_pulse_leaves_hpd_low() {
    let bench = bench();
    bench
        .schedule_action(Action::Unplug, When::At(Timestamp::from_micros(1_000)))
        .unwrap();
    bench.hpd_pulse(ms(5));
    assert!(!bench.sink().is_plugged());
    assert!(!bench.sink().hpd());
}
