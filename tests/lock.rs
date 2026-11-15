//! The sink's scrambling lock: `Scrambling_Status` coupled to `TMDS_Config` and the line,
//! and its settings.

use core::time::Duration;

use maquette::hdmi_hal::ddc::SCDC;
use maquette::{
    Actor, Bench, DdcBus, Delay, Event, Line, Refusal, RefusalKind, SinkConfig, TmdsClockRatio,
    Trigger,
};

const SCRAMBLED_40: Line = Line::Tmds {
    character_rate_khz: 594_000,
    ratio: TmdsClockRatio::Div40,
    scrambled: true,
};
const SCRAMBLED_10: Line = Line::Tmds {
    character_rate_khz: 297_000,
    ratio: TmdsClockRatio::Div10,
    scrambled: true,
};
const PLAIN_10: Line = Line::Tmds {
    character_rate_khz: 297_000,
    ratio: TmdsClockRatio::Div10,
    scrambled: false,
};

fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

fn bench(config: SinkConfig) -> Bench<'static> {
    Bench::builder(&[])
        .sink(config)
        .budget(None)
        .build()
        .unwrap()
}

/// Writes `TMDS_Config` over the bus.
fn write_config(bench: &Bench<'_>, value: u8) {
    bench.ddc().write(SCDC, &[0x20, value]).unwrap();
}

/// Reads `Scrambling_Status` over the bus.
fn read_status(bench: &Bench<'_>) -> u8 {
    let mut status = [0];
    bench.ddc().write_read(SCDC, &[0x21], &mut status).unwrap();
    status[0]
}

/// The times, in µs, of the sink's own changes to `reg`, with the value it set.
fn sink_sets(bench: &Bench<'_>, reg: u8) -> Vec<(u64, u8)> {
    bench
        .transcript()
        .filter_map(|record| match record.event {
            Event::Register {
                reg: r,
                to,
                by: Actor::Sink,
                ..
            } if r == reg => Some((record.at.as_micros(), to)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_ratio_must_fit_the_rate() {
    use TmdsClockRatio::{Div10, Div40};
    // (character rate, ratio on the line and in TMDS_Config, locks)
    for (rate, ratio, locks) in [
        (594_000, Div10, false),
        (340_001, Div10, false),
        (340_001, Div40, true),
        (340_000, Div40, false),
        (340_000, Div10, true),
        (148_500, Div40, false),
    ] {
        let bench = bench(SinkConfig::new());
        write_config(&bench, if ratio == Div40 { 0b11 } else { 0b01 });
        bench.set_line(Line::Tmds {
            character_rate_khz: rate,
            ratio,
            scrambled: true,
        });
        bench.advance(ms(5));
        assert_eq!(
            read_status(&bench),
            u8::from(locks),
            "{rate} kHz at {ratio:?}"
        );
    }
}

#[test]
fn the_status_is_set_1_ms_after_the_write_and_scrambled_output() {
    let bench = bench(SinkConfig::new());
    write_config(&bench, 0b11);
    let written = bench.now();
    bench.advance(ms(3));
    assert_eq!(read_status(&bench), 0, "no scrambled output yet");
    bench.set_line(SCRAMBLED_40);
    let started = bench.now();
    bench.advance(Duration::from_micros(999));
    assert_eq!(bench.sink().scrambling_status(), Some(0));
    bench.advance(Duration::from_micros(1));
    assert_eq!(bench.sink().scrambling_status(), Some(1));
    assert!(started > written);
    assert_eq!(sink_sets(&bench, 0x21), [(started.as_micros() + 1_000, 1)]);
}

#[test]
fn scrambled_output_before_the_write_locks_from_the_write_by_default() {
    let bench = bench(SinkConfig::new());
    bench.set_line(SCRAMBLED_40);
    bench.advance(ms(5));
    write_config(&bench, 0b11);
    let written = bench.now();
    bench.advance(ms(1));
    assert_eq!(read_status(&bench), 1);
    assert_eq!(sink_sets(&bench, 0x21), [(written.as_micros() + 1_000, 1)]);
}

#[test]
fn a_strict_sink_waits_for_the_output_to_start_again() {
    let config = SinkConfig::new().with_strict_lock(true);
    assert!(config.strict_lock());
    let bench = bench(config);
    bench.set_line(SCRAMBLED_40);
    bench.advance(ms(5));
    write_config(&bench, 0b11);
    bench.advance(ms(100));
    assert_eq!(read_status(&bench), 0);
    bench.set_line(Line::Off);
    bench.set_line(SCRAMBLED_40);
    let restarted = bench.now();
    bench.advance(ms(1));
    assert_eq!(read_status(&bench), 1);
    assert_eq!(
        sink_sets(&bench, 0x21),
        [(restarted.as_micros() + 1_000, 1)]
    );
    // Output that starts after the write locks as with the lenient sink.
    let bench = self::bench(SinkConfig::new().with_strict_lock(true));
    write_config(&bench, 0b11);
    bench.set_line(SCRAMBLED_40);
    bench.advance(ms(1));
    assert_eq!(read_status(&bench), 1);
}

#[test]
fn the_ratio_must_match_and_scrambling_at_1_10_locks_too() {
    let bench = bench(SinkConfig::new());
    write_config(&bench, 0b01); // scrambling, ratio 1/10
    bench.set_line(SCRAMBLED_40);
    bench.advance(ms(10));
    assert_eq!(read_status(&bench), 0, "the line is at 1/40");
    bench.set_line(SCRAMBLED_10);
    bench.advance(ms(1));
    assert_eq!(read_status(&bench), 1);
    // Unscrambled output at the right ratio does not lock.
    let bench = self::bench(SinkConfig::new());
    write_config(&bench, 0b01);
    bench.set_line(PLAIN_10);
    bench.advance(ms(10));
    assert_eq!(read_status(&bench), 0);
}

#[test]
fn the_lock_is_lost_when_a_condition_fails() {
    let bench = bench(SinkConfig::new());
    write_config(&bench, 0b11);
    bench.set_line(SCRAMBLED_40);
    bench.advance(ms(1));
    assert_eq!(read_status(&bench), 1);
    // The output stops.
    bench.set_line(Line::Off);
    assert_eq!(bench.sink().scrambling_status(), Some(0));
    bench.set_line(SCRAMBLED_40);
    bench.advance(ms(1));
    assert_eq!(read_status(&bench), 1);
    // The source clears Scrambling_Enable: the status goes with it, in the same write.
    write_config(&bench, 0b10);
    assert_eq!(read_status(&bench), 0);
    let changes: Vec<_> = sink_sets(&bench, 0x21).iter().map(|&(_, v)| v).collect();
    assert_eq!(changes, [1, 0, 1, 0]);
}

#[test]
fn a_rewrite_that_keeps_scrambling_enabled_does_not_restart_the_lock() {
    let bench = bench(SinkConfig::new());
    write_config(&bench, 0b11);
    bench.set_line(SCRAMBLED_40);
    bench.advance(ms(1));
    assert_eq!(read_status(&bench), 1);
    write_config(&bench, 0b11);
    assert_eq!(read_status(&bench), 1);
    assert_eq!(sink_sets(&bench, 0x21).len(), 1);
}

#[test]
fn the_trigger_counts_from_the_write_that_makes_the_ratio_match() {
    let bench = bench(SinkConfig::new());
    bench.set_line(SCRAMBLED_40);
    write_config(&bench, 0b01);
    bench.advance(ms(50));
    assert_eq!(read_status(&bench), 0, "the ratio does not match");
    write_config(&bench, 0b11);
    assert_eq!(
        read_status(&bench),
        0,
        "the trigger has not passed since the ratio matched"
    );
    bench.advance(ms(1));
    assert_eq!(read_status(&bench), 1);
}

#[test]
fn the_lock_can_come_on_a_count_of_reads() {
    let config = SinkConfig::new().with_scrambling_lock(Trigger::AfterReads(3));
    assert_eq!(config.scrambling_lock(), Trigger::AfterReads(3));
    let bench = bench(config);
    write_config(&bench, 0b11);
    bench.set_line(SCRAMBLED_40);
    bench.advance(ms(100));
    assert_eq!(read_status(&bench), 0);
    assert_eq!(read_status(&bench), 0);
    assert_eq!(read_status(&bench), 1);
    // A condition failing and holding again restarts the count.
    let bench = self::bench(SinkConfig::new().with_scrambling_lock(Trigger::AfterReads(2)));
    write_config(&bench, 0b11);
    bench.set_line(SCRAMBLED_40);
    assert_eq!(read_status(&bench), 0);
    bench.set_line(Line::Off);
    bench.set_line(SCRAMBLED_40);
    assert_eq!(read_status(&bench), 0);
    assert_eq!(read_status(&bench), 1);
    // Reads before the conditions held do not count.
    let bench = self::bench(SinkConfig::new().with_scrambling_lock(Trigger::AfterReads(1)));
    assert_eq!(read_status(&bench), 0);
    write_config(&bench, 0b11);
    bench.set_line(SCRAMBLED_40);
    assert_eq!(read_status(&bench), 1);
}

#[test]
fn a_sink_can_withhold_the_status_for_its_first_attempts() {
    let refusal = Refusal::new(RefusalKind::WithholdStatus, 2);
    let config = SinkConfig::new().with_scrambling_refusals(refusal);
    assert_eq!(config.scrambling_refusals(), refusal);
    let bench = bench(config);
    bench.set_line(SCRAMBLED_40);
    write_config(&bench, 0b11);
    bench.advance(ms(10));
    assert_eq!(read_status(&bench), 0);
    write_config(&bench, 0b11);
    bench.advance(ms(10));
    assert_eq!(read_status(&bench), 0);
    // The third attempt is not refused; Scrambling_Enable has been set since the first, but
    // the lock's conditions hold only from the accepted attempt, so the trigger counts from it.
    write_config(&bench, 0b11);
    assert_eq!(read_status(&bench), 0);
    bench.advance(ms(1));
    assert_eq!(read_status(&bench), 1);
    let refusals: Vec<_> = bench
        .transcript()
        .filter_map(|record| match record.event {
            Event::Refusal { attempt, kind } => Some((attempt, kind)),
            _ => None,
        })
        .collect();
    assert_eq!(
        refusals,
        [
            (1, RefusalKind::WithholdStatus),
            (2, RefusalKind::WithholdStatus)
        ]
    );
}

#[test]
fn a_sink_can_drop_scrambling_enable_from_its_first_attempts() {
    let config =
        SinkConfig::new().with_scrambling_refusals(Refusal::new(RefusalKind::DropEnable, 1));
    let bench = bench(config);
    bench.set_line(SCRAMBLED_40);
    write_config(&bench, 0b11);
    assert_eq!(bench.sink().tmds_config(), Some(0b10));
    bench.advance(ms(10));
    assert_eq!(read_status(&bench), 0);
    write_config(&bench, 0b11);
    assert_eq!(bench.sink().tmds_config(), Some(0b11));
    bench.advance(ms(1));
    assert_eq!(read_status(&bench), 1);
    assert_eq!(Refusal::NONE.attempts, 0);
    assert_eq!(SinkConfig::new().scrambling_refusals(), Refusal::NONE);
}

#[test]
fn a_sink_can_give_up_on_scrambling_enable() {
    let config = SinkConfig::new().with_enable_timeout(Some(ms(100)));
    assert_eq!(config.enable_timeout(), Some(ms(100)));
    let bench = bench(config);
    write_config(&bench, 0b11);
    let written = bench.now();
    bench.advance(ms(99));
    assert_eq!(bench.sink().tmds_config(), Some(0b11));
    bench.advance(ms(1));
    assert_eq!(bench.sink().tmds_config(), Some(0b10));
    assert_eq!(
        sink_sets(&bench, 0x20),
        [(written.as_micros() + 100_000, 0b10)]
    );
    // Output that starts after the deadline is late, even when no SCDC event fell between:
    // here the time passes reading the EDID.
    static EDID: [u8; 128] = [0; 128];
    let bench = Bench::builder(&EDID)
        .sink(SinkConfig::new().with_enable_timeout(Some(ms(10))))
        .build()
        .unwrap();
    write_config(&bench, 0b11);
    let deadline = bench.now().as_micros() + 10_000;
    let mut block = [0; 128];
    while bench.now().as_micros() < deadline + 5_000 {
        bench.ddc().write_read(0x50, &[0], &mut block).unwrap();
    }
    bench.set_line(SCRAMBLED_40);
    bench.advance(ms(5));
    assert_eq!(bench.sink().tmds_config(), Some(0b10));
    assert_eq!(read_status(&bench), 0);
    // Scrambled output in time keeps it.
    let bench = self::bench(SinkConfig::new().with_enable_timeout(Some(ms(100))));
    write_config(&bench, 0b11);
    bench.advance(ms(50));
    bench.set_line(SCRAMBLED_40);
    bench.advance(ms(50));
    bench.set_line(Line::Off);
    bench.advance(ms(100));
    assert_eq!(bench.sink().tmds_config(), Some(0b11));
    assert_eq!(SinkConfig::new().enable_timeout(), None);
}

#[test]
fn a_delay_on_the_time_handle_is_when_the_status_comes() {
    let bench = bench(SinkConfig::new());
    write_config(&bench, 0b11);
    bench.set_line(SCRAMBLED_40);
    let from = bench.now();
    let mut delay = bench.time();
    delay.delay(Duration::from_micros(400));
    delay.delay(Duration::from_micros(700));
    // Due 1 ms after `from`, taken at the end of the second delay.
    assert_eq!(sink_sets(&bench, 0x21), [(from.as_micros() + 1_100, 1)]);
}

#[test]
fn a_preset_status_holds_until_the_lock_changes() {
    let bench = bench(SinkConfig::new());
    bench.preset_scdc(0x21, 0x01);
    bench.advance(ms(10));
    assert_eq!(read_status(&bench), 1);
    write_config(&bench, 0b11);
    bench.set_line(SCRAMBLED_40);
    bench.advance(ms(1));
    bench.set_line(Line::Off);
    assert_eq!(read_status(&bench), 0, "the lock was gained and lost");
}

#[test]
fn the_default_lock_is_after_1_ms_and_lenient() {
    let config = SinkConfig::new();
    assert_eq!(config.scrambling_lock(), Trigger::After(ms(1)));
    assert!(!config.strict_lock());
}
