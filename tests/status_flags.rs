//! The status flags: `Clock_Detected` and the lane locks in `Status_Flags_0`, and
//! `Status_Update`.

use core::time::Duration;

use maquette::hdmi_hal::ddc::SCDC;
use maquette::{
    Actor, Bench, DdcBus, Event, Line, SinkConfig, StatusUpdate, TmdsClockRatio, Trigger,
};

const MS: Duration = Duration::from_millis(1);

const TMDS_297: Line = Line::Tmds {
    character_rate_khz: 297_000,
    ratio: TmdsClockRatio::Div10,
    scrambled: false,
};

const TMDS_594: Line = Line::Tmds {
    character_rate_khz: 594_000,
    ratio: TmdsClockRatio::Div40,
    scrambled: true,
};

/// The sink's changes to `Status_Flags_0` and `Update_0`, in order.
fn sink_changes(bench: &Bench<'_>) -> Vec<(u8, u8)> {
    bench
        .transcript()
        .filter_map(|record| match record.event {
            Event::Register {
                reg: reg @ (0x10 | 0x40),
                to,
                by: Actor::Sink,
                ..
            } => Some((reg, to)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_clock_is_detected_at_once_and_the_lanes_lock_after_the_trigger() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    bench.set_line(TMDS_297);
    assert_eq!(bench.sink().status_flags_0(), Some(0x01));
    bench.advance(MS);
    assert_eq!(bench.sink().status_flags_0(), Some(0x0F));
    // Status_Update is set at the first change and kept until the source clears it.
    assert_eq!(
        sink_changes(&bench),
        [(0x40, 0x01), (0x10, 0x01), (0x40, 0x0F)]
    );
}

#[test]
fn the_source_clears_status_update_and_the_next_change_sets_it_again() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    let bus = bench.ddc();
    bench.set_line(TMDS_297);
    let mut update = [0];
    bus.write_read(SCDC, &[0x10], &mut update).unwrap();
    assert_eq!(update, [0x01]);
    bus.write(SCDC, &[0x10, 0x01]).unwrap();
    assert_eq!(bench.sink().update_0(), Some(0));
    bench.advance(MS);
    assert_eq!(bench.sink().update_0(), Some(0x01));
    // The line stops: every flag clears.
    bus.write(SCDC, &[0x10, 0x01]).unwrap();
    bench.set_line(Line::Off);
    assert_eq!(bench.sink().status_flags_0(), Some(0));
    assert_eq!(bench.sink().update_0(), Some(0x01));
}

#[test]
fn above_340_mcsc_the_lanes_lock_only_with_the_matching_configuration() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    let bus = bench.ddc();
    bench.set_line(TMDS_594);
    bench.advance(MS);
    // TMDS_Config is 0: the sink sees the clock, but cannot decode.
    assert_eq!(bench.sink().status_flags_0(), Some(0x01));
    // The ratio alone is not enough: the line is scrambled.
    bus.write(SCDC, &[0x20, 0x02]).unwrap();
    bench.advance(MS);
    assert_eq!(bench.sink().status_flags_0(), Some(0x01));
    bus.write(SCDC, &[0x20, 0x03]).unwrap();
    bench.advance(MS);
    assert_eq!(bench.sink().status_flags_0(), Some(0x0F));
    // Unscrambled above 340 Mcsc is not decodable, whatever TMDS_Config says.
    bench.set_line(Line::Tmds {
        character_rate_khz: 594_000,
        ratio: TmdsClockRatio::Div40,
        scrambled: false,
    });
    bus.write(SCDC, &[0x20, 0x02]).unwrap();
    bench.advance(MS);
    assert_eq!(bench.sink().status_flags_0(), Some(0x01));
}

#[test]
fn the_lanes_unlock_when_the_configuration_stops_matching() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    let bus = bench.ddc();
    bus.write(SCDC, &[0x20, 0x03]).unwrap();
    bench.set_line(TMDS_594);
    bench.advance(MS);
    assert_eq!(bench.sink().status_flags_0(), Some(0x0F));
    bus.write(SCDC, &[0x20, 0x00]).unwrap();
    assert_eq!(bench.sink().status_flags_0(), Some(0x01));
}

#[test]
fn a_sink_that_never_sets_status_update() {
    let config = SinkConfig::new().with_status_update(StatusUpdate::Never);
    assert_eq!(config.status_update(), StatusUpdate::Never);
    let bench = Bench::new(config, &[]).unwrap();
    bench.set_line(TMDS_297);
    bench.advance(MS);
    assert_eq!(bench.sink().status_flags_0(), Some(0x0F));
    assert_eq!(bench.sink().update_0(), Some(0));
}

#[test]
fn the_lanes_can_lock_on_the_sources_reads_of_the_flags() {
    let config = SinkConfig::new().with_scrambling_lock(Trigger::AfterReads(2));
    let bench = Bench::new(config, &[]).unwrap();
    let bus = bench.ddc();
    bench.set_line(TMDS_297);
    bench.advance(MS);
    let mut flags = [0];
    bus.write_read(SCDC, &[0x40], &mut flags).unwrap();
    assert_eq!(flags, [0x01]);
    bus.write_read(SCDC, &[0x40], &mut flags).unwrap();
    assert_eq!(flags, [0x0F]);
}

#[test]
fn a_preset_holds_until_the_flags_change_and_other_bits_are_kept() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    bench.set_line(TMDS_297);
    // A preset that hides the clock and sets FLT_ready.
    bench.preset_scdc(0x40, 0x40);
    bench.advance(Duration::from_micros(10));
    assert_eq!(bench.sink().status_flags_0(), Some(0x40));
    // The lanes lock: the sink sets its flags, and FLT_ready stays.
    bench.advance(MS);
    assert_eq!(bench.sink().status_flags_0(), Some(0x4F));
}

#[test]
fn an_hpd_low_resets_the_flags_and_a_running_line_is_seen_again() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    bench.set_line(TMDS_297);
    bench.advance(MS);
    bench.set_hpd(false);
    // Reset with the status group, then the clock is detected again; the lanes relock
    // after the trigger.
    assert_eq!(bench.sink().status_flags_0(), Some(0x01));
    bench.advance(MS);
    assert_eq!(bench.sink().status_flags_0(), Some(0x0F));
}

#[test]
fn an_unplugged_sink_receives_nothing() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    bench.set_line(TMDS_297);
    bench.advance(MS);
    bench.unplug();
    bench.advance(MS);
    assert_eq!(bench.sink().status_flags_0(), Some(0));
}
