//! The built-in SCDC target, its settings, presets and the `Sink` view.

use maquette::hdmi_hal::ddc::SCDC;
use maquette::hdmi_hal::scdc::{ScdcOverDdc, ScdcTransport};
use maquette::{
    Actor, Bench, DdcBus, DdcErrorKind, Env, Event, Ignored, NoAcknowledgeSource, Operation,
    OwnedWrite, ScdcPastEnd, SinkConfig, Target,
};

const NACK_DATA: DdcErrorKind = DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Data);
const NACK_ADDRESS: DdcErrorKind = DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Address);

fn bench(config: SinkConfig) -> Bench<'static> {
    Bench::new(config, &[]).unwrap()
}

/// The transcript without its bus records.
/// What these tests compare of an event at 0x54: the transcript's variants are matched, not
/// built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seen {
    Register {
        reg: u8,
        from: u8,
        to: u8,
        by: Actor,
    },
    Ignored {
        reg: u8,
        value: u8,
        why: Ignored,
    },
    Other,
}

fn seen(event: Event) -> Seen {
    match event {
        Event::Register {
            addr: SCDC,
            reg,
            from,
            to,
            by,
            ..
        } => Seen::Register { reg, from, to, by },
        Event::IgnoredWrite {
            addr: SCDC,
            reg,
            value,
            why,
            ..
        } => Seen::Ignored { reg, value, why },
        _ => Seen::Other,
    }
}

fn sink_events(bench: &Bench<'_>) -> Vec<Seen> {
    bench
        .transcript()
        .map(|record| record.event)
        .filter(|event| !matches!(event, Event::Bus(_)))
        .map(seen)
        .collect()
}

fn register(reg: u8, from: u8, to: u8, by: Actor) -> Seen {
    Seen::Register { reg, from, to, by }
}

#[test]
fn the_default_sink_reports_sink_version_1_and_zeros() {
    let bench = bench(SinkConfig::new());
    let mut regs = [0xEE; 3];
    bench.ddc().write_read(SCDC, &[0x00], &mut regs).unwrap();
    assert_eq!(regs, [0, 1, 0]);
    let sink = bench.sink();
    assert_eq!(sink.sink_version(), Some(1));
    assert_eq!(sink.source_version(), Some(0));
    assert_eq!(sink.tmds_config(), Some(0));
    assert_eq!(sink.scrambling_status(), Some(0));
    assert_eq!(sink.update_0(), Some(0));
    assert_eq!(sink.config_0(), Some(0));
    assert_eq!(sink.config_1(), Some(0));
    assert!(
        sink_events(&bench).is_empty(),
        "reading records no register event"
    );
}

#[test]
fn source_writes_are_stored_and_recorded() {
    let bench = bench(SinkConfig::new());
    let bus = bench.ddc();
    bus.write(SCDC, &[0x02, 0x01]).unwrap();
    bus.write(SCDC, &[0x20, 0x03]).unwrap();
    // A burst: Config_0 and Config_1 in one write.
    bus.write(SCDC, &[0x30, 0x02, 0x35]).unwrap();
    // Writing the same value again is still recorded: it is a write the source made.
    bus.write(SCDC, &[0x20, 0x03]).unwrap();
    let sink = bench.sink();
    assert_eq!(sink.source_version(), Some(1));
    assert_eq!(sink.tmds_config(), Some(3));
    assert_eq!(sink.config_0(), Some(0x02));
    assert_eq!(sink.config_1(), Some(0x35));
    assert_eq!(
        sink_events(&bench),
        [
            register(0x02, 0, 1, Actor::Source),
            register(0x20, 0, 3, Actor::Source),
            register(0x30, 0, 0x02, Actor::Source),
            register(0x31, 0, 0x35, Actor::Source),
            register(0x20, 3, 3, Actor::Source),
        ]
    );
}

#[test]
fn register_events_follow_their_bus_record_at_its_time() {
    let bench = bench(SinkConfig::new());
    bench.advance(core::time::Duration::from_micros(700));
    bench.ddc().write(SCDC, &[0x20, 0x01]).unwrap();
    let records: Vec<_> = bench.transcript().collect();
    assert_eq!(records.len(), 2);
    assert!(matches!(records[0].event, Event::Bus(_)));
    assert_eq!(seen(records[1].event), register(0x20, 0, 1, Actor::Source));
    assert_eq!(records[1].at, records[0].at);
    assert_eq!(records[1].at.as_micros(), 700);
}

#[test]
fn the_offset_persists_and_advances() {
    let bench = bench(SinkConfig::new());
    let bus = bench.ddc();
    bench.preset_scdc(0x21, 0x01);
    bus.write(SCDC, &[0x20, 0x03]).unwrap();
    // The write left the offset at 0x21: a read with no offset reads from there.
    let mut buf = [0u8; 2];
    bus.read(SCDC, &mut buf).unwrap();
    assert_eq!(buf, [0x01, 0x00]);
    // An offset alone sets it.
    bus.write(SCDC, &[0x20]).unwrap();
    bus.read(SCDC, &mut buf).unwrap();
    assert_eq!(buf, [0x03, 0x01]);
}

#[test]
fn scdc_over_ddc_reads_and_writes_through_the_target() {
    let bench = bench(SinkConfig::new());
    let bus = bench.ddc();
    let mut scdc = ScdcOverDdc::new(&bus).expect("the bench's bus serves SCDC");
    scdc.write(0x20, 0x02).unwrap();
    assert_eq!(scdc.read(0x01), Ok(1));
    let mut block = [0u8; 2];
    scdc.read_block(0x20, &mut block).unwrap();
    assert_eq!(block, [0x02, 0x00]);
    assert_eq!(bench.sink().tmds_config(), Some(0x02));
}

#[test]
fn update_flags_clear_when_written_back_as_1() {
    let bench = bench(SinkConfig::new());
    let bus = bench.ddc();
    bench.preset_scdc(0x10, 0b0010_0011); // FLT_update, CED_update, Status_update
    bench.preset_scdc(0x11, 0b0000_0001);
    bus.write(SCDC, &[0x10, 0b0010_0000]).unwrap();
    assert_eq!(bench.sink().update_0(), Some(0b0000_0011));
    // A 0 leaves a flag set; the other flags are cleared together.
    bus.write(SCDC, &[0x10, 0b0000_0011, 0b0000_0001]).unwrap();
    assert_eq!(bench.sink().update_0(), Some(0));
    assert_eq!(bench.sink().scdc(0x11), Some(0));
    let events = sink_events(&bench);
    assert_eq!(
        events[2..],
        [
            register(0x10, 0b0010_0011, 0b0000_0011, Actor::Source),
            register(0x10, 0b0000_0011, 0, Actor::Source),
            register(0x11, 1, 0, Actor::Source),
        ]
    );
}

#[test]
fn writes_to_the_sinks_registers_are_ignored_by_default() {
    let bench = bench(SinkConfig::new());
    let bus = bench.ddc();
    for reg in [0x01, 0x21, 0x35, 0x40, 0x42, 0x50, 0x5A, 0xD0, 0xDD] {
        bus.write(SCDC, &[reg, 0x7F]).unwrap();
        assert_eq!(
            bench.sink().scdc(reg),
            Some(if reg == 0x01 { 1 } else { 0 })
        );
    }
    let events = sink_events(&bench);
    assert_eq!(events.len(), 9);
    assert_eq!(
        events[1],
        Seen::Ignored {
            reg: 0x21,
            value: 0x7F,
            why: Ignored::SinkOwned
        }
    );
    // The burst carries on past an ignored register.
    bus.write(SCDC, &[0x20, 0x03, 0x01, 0x00]).unwrap();
    assert_eq!(bench.sink().tmds_config(), Some(3));
    assert_eq!(bench.sink().scdc(0x22), Some(0));
}

#[test]
fn the_manufacturers_block_past_its_identification_is_stored() {
    let bench = bench(SinkConfig::new());
    let bus = bench.ddc();
    for reg in [0xDE, 0xFF] {
        bus.write(SCDC, &[reg, 0x7F]).unwrap();
        assert_eq!(bench.sink().scdc(reg), Some(0x7F));
    }
}

#[test]
fn writes_to_the_sinks_registers_can_be_refused() {
    let bench = bench(SinkConfig::new().with_owned_scdc_writes(OwnedWrite::Nack));
    let bus = bench.ddc();
    // TMDS_Config is written, then the status register refuses the next byte.
    let error = bus.write(SCDC, &[0x20, 0x03, 0x01]).unwrap_err();
    assert_eq!(error.kind, NACK_DATA);
    assert_eq!(bench.sink().tmds_config(), Some(3));
    assert_eq!(bench.sink().scrambling_status(), Some(0));
    assert_eq!(sink_events(&bench), [register(0x20, 0, 3, Actor::Source)]);
}

#[test]
fn registers_outside_the_modelled_set_are_stored() {
    let bench = bench(SinkConfig::new());
    bench.ddc().write(SCDC, &[0x60, 0xAB, 0xCD]).unwrap();
    assert_eq!(bench.sink().scdc(0x60), Some(0xAB));
    assert_eq!(bench.sink().scdc(0x61), Some(0xCD));
}

#[test]
fn bursts_past_0xff_wrap_by_default() {
    let bench = bench(SinkConfig::new());
    let bus = bench.ddc();
    bench.preset_scdc(0xFF, 0x07);
    bench.preset_scdc(0x00, 0x09);
    let mut buf = [0u8; 3];
    bus.write_read(SCDC, &[0xFE], &mut buf).unwrap();
    assert_eq!(buf, [0x00, 0x07, 0x09]);
    // 0xFF is the sink's, 0x00 is not: the burst wraps and writes it.
    bus.write(SCDC, &[0xFF, 0x11, 0x22]).unwrap();
    assert_eq!(bench.sink().scdc(0x00), Some(0x22));
    // The offset wrapped with it.
    bus.read(SCDC, &mut buf[..1]).unwrap();
    assert_eq!(buf[0], 0x01);
}

#[test]
fn write_bursts_past_0xff_can_be_refused_and_reads_wrap() {
    let bench = bench(SinkConfig::new().with_scdc_past_end(ScdcPastEnd::Nack));
    let bus = bench.ddc();
    bench.preset_scdc(0xFF, 0x07);
    bench.preset_scdc(0x00, 0x09);
    // A read wraps: the sink cannot refuse a byte it sends.
    let mut buf = [0u8; 3];
    bus.write_read(SCDC, &[0xFE], &mut buf).unwrap();
    assert_eq!(buf, [0x00, 0x07, 0x09]);
    let error = bus.write(SCDC, &[0xFF, 0x11, 0x22]).unwrap_err();
    assert_eq!(error.kind, NACK_DATA);
    assert_eq!(bench.sink().scdc(0x00), Some(0x09));
    // A new transaction starts again at its own offset.
    bus.write(SCDC, &[0x00, 0x33]).unwrap();
    assert_eq!(bench.sink().scdc(0x00), Some(0x33));
}

#[test]
fn presets_set_any_register_and_are_recorded() {
    let bench = bench(SinkConfig::new());
    bench.preset_scdc(0x21, 0x01);
    bench.preset_scdc(0x01, 0x02);
    assert_eq!(bench.sink().scrambling_status(), Some(1));
    assert_eq!(bench.sink().sink_version(), Some(2));
    assert_eq!(
        sink_events(&bench),
        [
            register(0x21, 0, 1, Actor::Test),
            register(0x01, 1, 2, Actor::Test),
        ]
    );
    // A preset is not a bus operation: no time, no bus record.
    assert_eq!(bench.now().as_micros(), 0);
}

#[test]
fn the_sink_version_is_a_setting() {
    let config = SinkConfig::new().with_sink_version(0);
    assert_eq!(config.sink_version(), 0);
    let bench = bench(config);
    let mut version = [0xEE];
    bench.ddc().write_read(SCDC, &[0x01], &mut version).unwrap();
    assert_eq!(version, [0]);
}

#[test]
fn a_sink_without_scdc_does_not_answer() {
    let config = SinkConfig::new().without_scdc();
    assert!(!config.has_scdc());
    let bench = bench(config);
    let error = bench.ddc().write(SCDC, &[0x20, 0x03]).unwrap_err();
    assert_eq!(error.kind, NACK_ADDRESS);
    assert_eq!(bench.sink().scdc(0x01), None);
    assert_eq!(bench.sink().tmds_config(), None);
    assert!(
        ScdcOverDdc::new(&bench.ddc()).is_ok(),
        "the bus still reaches 0x54"
    );
}

#[test]
#[should_panic(expected = "preset of SCDC register 0x20 on a sink without SCDC")]
fn a_preset_without_scdc_is_a_mistake() {
    bench(SinkConfig::new().without_scdc()).preset_scdc(0x20, 0x03);
}

#[test]
fn a_tests_target_at_0x54_replaces_the_built_in_one() {
    /// Reads 0x42 everywhere.
    struct Quirk;
    impl Target for Quirk {
        fn answers(&self, addr: u8, _env: &Env<'_>) -> bool {
            addr == SCDC
        }
        fn transaction(
            &mut self,
            ops: &mut [Operation<'_>],
            _env: &mut Env<'_>,
        ) -> Result<(), DdcErrorKind> {
            for op in ops {
                if let Operation::Read { buf, .. } = op {
                    buf.fill(0x42);
                }
            }
            Ok(())
        }
    }
    let mut quirk = Quirk;
    let mut targets: [&mut (dyn Target + Send); 1] = [&mut quirk];
    let bench = Bench::builder(&[]).targets(&mut targets).build().unwrap();
    let mut version = [0];
    bench.ddc().write_read(SCDC, &[0x01], &mut version).unwrap();
    assert_eq!(version, [0x42]);
    bench.ddc().write(SCDC, &[0x20, 0x03]).unwrap();
    // The built-in register file is still there, untouched.
    assert_eq!(bench.sink().tmds_config(), Some(0));
}

#[test]
fn settings_read_back() {
    let config = SinkConfig::default();
    assert_eq!(config, SinkConfig::new());
    assert!(config.has_scdc());
    assert_eq!(config.sink_version(), 1);
    assert_eq!(config.owned_scdc_writes(), OwnedWrite::Ignore);
    assert_eq!(config.scdc_past_end(), ScdcPastEnd::Wrap);
    let config = config
        .with_owned_scdc_writes(OwnedWrite::Nack)
        .with_scdc_past_end(ScdcPastEnd::Nack);
    assert_eq!(config.owned_scdc_writes(), OwnedWrite::Nack);
    assert_eq!(config.scdc_past_end(), ScdcPastEnd::Nack);
}

#[test]
fn debug_output_shows_the_sink() {
    let bench = bench(SinkConfig::new());
    bench.preset_scdc(0x20, 0x03);
    assert_eq!(
        format!("{:?}", bench.sink()),
        "Sink { tmds_config: Some(3), scrambling_status: Some(0), .. }"
    );
    assert!(format!("{bench:?}").contains("scdc: true"));
}
