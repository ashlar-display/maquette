//! The SCDC registers a long HPD low resets, by register group.

use core::time::Duration;

use maquette::hdmi_hal::ddc::SCDC;
use maquette::{
    Actor, Bench, BusTiming, DdcBus, Event, HpdReset, Line, RegisterGroup, SinkConfig,
    TmdsClockRatio,
};

fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

/// A bench whose sink never gives up on `Scrambling_Enable`: most of these tests set it
/// with no output, and are about the resets.
fn bench(config: SinkConfig) -> Bench<'static> {
    Bench::new(config.with_enable_timeout(None), &[]).unwrap()
}

/// Sets the registers each group holds, over the bus where the source writes them.
fn fill(bench: &Bench<'_>) {
    let bus = bench.ddc();
    bus.write(SCDC, &[0x02, 0x01]).unwrap(); // Source_Version
    bus.write(SCDC, &[0x20, 0x03]).unwrap(); // TMDS_Config
    bus.write(SCDC, &[0x30, 0x01, 0x35]).unwrap(); // Config_0, Config_1
    bench.preset_scdc(0x10, 0x22); // update flags
    bench.preset_scdc(0x21, 0x01); // Scrambling_Status
}

/// The sink's resets: (time in µs, register), in order.
fn resets(bench: &Bench<'_>) -> Vec<(u64, u8)> {
    bench
        .transcript()
        .filter_map(|record| match record.event {
            Event::Register {
                reg,
                to: 0,
                by: Actor::Sink,
                ..
            } => Some((record.at.as_micros(), reg)),
            _ => None,
        })
        .collect()
}

#[test]
fn config_resets_on_any_hpd_low_and_the_rest_after_100_ms() {
    let bench = bench(SinkConfig::new());
    fill(&bench);
    let start = bench.now().as_micros();
    bench.hpd_pulse(ms(50));
    assert_eq!(resets(&bench), [(start, 0x30), (start, 0x31)]);
    let sink = bench.sink();
    assert_eq!(sink.tmds_config(), Some(0x03));
    assert_eq!(sink.source_version(), Some(0x01));
    assert_eq!(sink.update_0(), Some(0x22));

    bench.clear_transcript();
    let low = bench.now().as_micros();
    bench.hpd_pulse(ms(100));
    let due = low + 100_000;
    assert_eq!(
        resets(&bench),
        [(due, 0x20), (due, 0x21), (due, 0x10), (due, 0x02)]
    );
    assert_eq!(sink.tmds_config(), Some(0));
    assert_eq!(sink.scrambling_status(), Some(0));
    assert_eq!(sink.update_0(), Some(0));
    assert_eq!(sink.source_version(), Some(0));
    assert_eq!(
        sink.sink_version(),
        Some(1),
        "Sink_Version is not in a group"
    );
}

#[test]
fn a_reset_is_taken_at_the_first_event_after_it_is_due() {
    let bench = bench(SinkConfig::new());
    fill(&bench);
    bench.hpd(false);
    let low = bench.now().as_micros();
    // SCDC answers while HPD is low; reads at 60 ms and 150 ms are events.
    bench.advance(ms(60));
    let mut config = [0];
    bench.ddc().write_read(SCDC, &[0x20], &mut config).unwrap();
    assert_eq!(config, [0x03]);
    bench.advance(ms(90));
    let read_at = bench.now().as_micros();
    bench.ddc().write_read(SCDC, &[0x20], &mut config).unwrap();
    assert_eq!(config, [0], "reset before the read");
    let late: Vec<_> = resets(&bench)
        .into_iter()
        .filter(|&(_, reg)| reg == 0x20)
        .collect();
    // `advance` to 150 ms was the first event at or after 100 ms.
    assert_eq!(late, [(read_at, 0x20)]);
    assert!(read_at >= low + 100_000);
}

#[test]
fn a_reset_due_while_nothing_happened_is_taken_when_hpd_rises() {
    // A long transaction moves the clock without an event: it is charged after it ran.
    let timing = BusTiming::STANDARD_MODE.with_per_transaction(ms(200));
    let bench = Bench::builder(&[]).bus_timing(timing).build().unwrap();
    bench.preset_scdc(0x20, 0x03);
    bench.hpd(false);
    bench.ddc().write(0x37, &[0]).unwrap_err();
    assert_eq!(bench.sink().tmds_config(), Some(0x03));
    bench.hpd(true);
    let risen = bench.now().as_micros();
    assert!(resets(&bench).contains(&(risen, 0x20)));
    assert_eq!(bench.sink().tmds_config(), Some(0));
}

#[test]
fn a_group_can_survive_any_hpd_low() {
    let config = SinkConfig::new()
        .with_hpd_reset(RegisterGroup::TmdsConfig, HpdReset::Never)
        .with_hpd_reset(RegisterGroup::Config, HpdReset::Never);
    assert_eq!(config.hpd_reset(RegisterGroup::TmdsConfig), HpdReset::Never);
    let bench = bench(config);
    fill(&bench);
    bench.hpd_pulse(Duration::from_secs(1));
    assert_eq!(bench.sink().tmds_config(), Some(0x03));
    assert_eq!(bench.sink().config_1(), Some(0x35));
    assert_eq!(bench.sink().source_version(), Some(0));
}

#[test]
fn a_reset_after_zero_is_a_reset_as_hpd_falls() {
    let config = SinkConfig::new().with_hpd_reset(
        RegisterGroup::SourceVersion,
        HpdReset::After(Duration::ZERO),
    );
    let bench = bench(config);
    fill(&bench);
    bench.hpd(false);
    assert_eq!(bench.sink().source_version(), Some(0));
}

#[test]
fn each_low_resets_a_group_once() {
    let bench = bench(SinkConfig::new());
    bench.hpd(false);
    bench.advance(ms(150));
    // Written again while HPD is still low: kept for the rest of this low.
    bench.ddc().write(SCDC, &[0x20, 0x03]).unwrap();
    bench.advance(ms(150));
    assert_eq!(bench.sink().tmds_config(), Some(0x03));
    bench.hpd(true);
    assert_eq!(bench.sink().tmds_config(), Some(0x03));
    // The next long low resets it again.
    bench.hpd_pulse(ms(100));
    assert_eq!(bench.sink().tmds_config(), Some(0));
}

#[test]
fn a_reset_of_tmds_config_loses_the_lock() {
    let config = SinkConfig::new().with_hpd_reset(RegisterGroup::Status, HpdReset::Never);
    let bench = bench(config);
    bench.ddc().write(SCDC, &[0x20, 0x03]).unwrap();
    bench.set_line(Line::Tmds {
        character_rate_khz: 594_000,
        ratio: TmdsClockRatio::Div40,
        scrambled: true,
    });
    bench.advance(ms(1));
    assert_eq!(bench.sink().scrambling_status(), Some(1));
    bench.hpd_pulse(ms(100));
    // The status group survives, but the lock is lost with Scrambling_Enable.
    assert_eq!(bench.sink().tmds_config(), Some(0));
    assert_eq!(bench.sink().scrambling_status(), Some(0));
}

/// A bench locked at 594 Mcsc, 1/40.
fn locked(config: SinkConfig) -> Bench<'static> {
    let bench = bench(config);
    bench.ddc().write(SCDC, &[0x20, 0x03]).unwrap();
    bench.set_line(Line::Tmds {
        character_rate_khz: 594_000,
        ratio: TmdsClockRatio::Div40,
        scrambled: true,
    });
    bench.advance(ms(1));
    assert_eq!(bench.sink().scrambling_status(), Some(1));
    bench
}

#[test]
fn the_sinks_own_reset_of_the_status_re_arms_the_lock() {
    let config = SinkConfig::new().with_hpd_reset(RegisterGroup::TmdsConfig, HpdReset::Never);
    let bench = locked(config);
    bench.hpd_pulse(ms(200));
    // The reset was taken at the end of the low; the lock holds again 1 ms after it.
    assert_eq!(bench.sink().tmds_config(), Some(0x03));
    assert_eq!(bench.sink().scrambling_status(), Some(0));
    bench.advance(Duration::from_micros(999));
    assert_eq!(bench.sink().scrambling_status(), Some(0));
    bench.advance(Duration::from_micros(1));
    assert_eq!(bench.sink().scrambling_status(), Some(1));
}

#[test]
fn a_preset_of_the_status_still_holds() {
    let bench = locked(SinkConfig::new());
    bench.preset_scdc(0x21, 0x00);
    bench.advance(ms(10));
    assert_eq!(bench.sink().scrambling_status(), Some(0));
}

#[test]
fn a_reset_records_each_change_of_the_status_once() {
    let bench = locked(SinkConfig::new());
    bench.hpd_pulse(ms(200));
    let status: Vec<(u8, u8)> = bench
        .transcript()
        .filter_map(|record| match record.event {
            Event::Register {
                reg: 0x21,
                from,
                to,
                ..
            } => Some((from, to)),
            _ => None,
        })
        .collect();
    assert_eq!(status, [(0, 1), (1, 0)]);
}

#[test]
fn the_groups_and_their_defaults() {
    assert_eq!(RegisterGroup::ALL.len(), 4);
    assert_eq!(RegisterGroup::TmdsConfig.registers(), [0x20]);
    assert_eq!(
        RegisterGroup::Status.registers(),
        [0x21, 0x10, 0x11, 0x40, 0x41, 0x42]
    );
    assert_eq!(RegisterGroup::SourceVersion.registers(), [0x02]);
    assert_eq!(RegisterGroup::Config.registers(), [0x30, 0x31]);
    let config = SinkConfig::new();
    for group in [
        RegisterGroup::TmdsConfig,
        RegisterGroup::Status,
        RegisterGroup::SourceVersion,
    ] {
        assert_eq!(config.hpd_reset(group), HpdReset::After(ms(100)));
    }
    assert_eq!(config.hpd_reset(RegisterGroup::Config), HpdReset::OnHpdLow);
}
