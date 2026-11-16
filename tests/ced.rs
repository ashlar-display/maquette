//! The character error counters: counted while the lane is locked, valid bits, saturation,
//! the checksum, resets and `CED_Update`.

use core::time::Duration;

use maquette::hdmi_hal::ddc::SCDC;
use maquette::{
    Action, Actor, Bench, CedUpdate, DdcBus, ErrorRates, Event, HpdReset, Line, RegisterGroup,
    SinkConfig, TmdsClockRatio, When,
};

const MS: Duration = Duration::from_millis(1);

const TMDS_297: Line = Line::Tmds {
    character_rate_khz: 297_000,
    ratio: TmdsClockRatio::Div10,
    scrambled: false,
};

/// A bench whose lanes are locked, from the time it returns.
fn locked(config: SinkConfig) -> Bench<'static> {
    let bench = Bench::new(config, &[]).unwrap();
    bench.set_line(TMDS_297);
    bench.advance(MS);
    assert_eq!(bench.sink().status_flags_0(), Some(0x0F));
    bench
}

/// The counter bytes 0x50–0x56, as the sink holds them.
fn counters(bench: &Bench<'_>) -> [u8; 7] {
    core::array::from_fn(|i| bench.sink().scdc(0x50 + i as u8).unwrap())
}

/// Lane `lane`'s count and whether it is valid.
fn count(bench: &Bench<'_>, lane: usize) -> (u16, bool) {
    let bytes = counters(bench);
    let high = bytes[2 * lane + 1];
    (
        u16::from_le_bytes([bytes[2 * lane], high & 0x7F]),
        high & 0x80 != 0,
    )
}

fn sums_to_zero(bytes: [u8; 7]) -> bool {
    bytes.iter().fold(0u8, |sum, &byte| sum.wrapping_add(byte)) == 0
}

#[test]
fn an_error_free_locked_link_reads_valid_zero_counts() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    assert_eq!(counters(&bench), [0; 7]);
    bench.set_line(TMDS_297);
    bench.advance(MS);
    // As a real sink read: 0x56 = 0x80 with lanes 0-2 valid at 0.
    assert_eq!(counters(&bench), [0, 0x80, 0, 0x80, 0, 0x80, 0x80]);
}

#[test]
fn errors_are_counted_per_lane_with_a_checksum() {
    let bench = locked(SinkConfig::new());
    bench.set_character_errors(ErrorRates {
        lane1: 1_000,
        lane2: 300_000,
        ..ErrorRates::NONE
    });
    bench.advance(Duration::from_millis(10));
    assert_eq!(count(&bench, 0), (0, true));
    assert_eq!(count(&bench, 1), (10, true));
    assert_eq!(count(&bench, 2), (3_000, true));
    assert!(sums_to_zero(counters(&bench)));
    // The source reads the same in one burst.
    let mut read = [0u8; 7];
    bench.ddc().write_read(SCDC, &[0x50], &mut read).unwrap();
    assert_eq!(read, counters(&bench));
}

#[test]
fn errors_add_up_across_events_shorter_than_one_error() {
    let bench = locked(SinkConfig::new());
    bench.set_character_errors(ErrorRates::all(1));
    for _ in 0..10 {
        bench.advance(Duration::from_millis(100));
    }
    assert_eq!(count(&bench, 0), (1, true));
}

#[test]
fn a_lane_not_locked_counts_nothing_and_is_not_valid() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    bench.set_character_errors(ErrorRates::all(1_000));
    // The clock without a decodable line: 594 Mcsc unscrambled.
    bench.set_line(Line::Tmds {
        character_rate_khz: 594_000,
        ratio: TmdsClockRatio::Div40,
        scrambled: false,
    });
    bench.advance(Duration::from_millis(10));
    assert_eq!(bench.sink().status_flags_0(), Some(0x01));
    assert_eq!(counters(&bench), [0; 7]);
    // Once the line is decodable, the count starts when the lanes lock.
    bench.set_line(TMDS_297);
    bench.advance(MS);
    bench.advance(Duration::from_millis(2));
    assert_eq!(count(&bench, 0), (2, true));
    // Lost again: the count holds, no longer valid.
    bench.set_line(Line::Off);
    bench.advance(Duration::from_millis(10));
    assert_eq!(count(&bench, 0), (2, false));
    assert!(sums_to_zero(counters(&bench)));
}

#[test]
fn a_counter_stays_at_0x7fff() {
    let bench = locked(SinkConfig::new());
    bench.set_character_errors(ErrorRates {
        lane0: u32::MAX,
        ..ErrorRates::NONE
    });
    bench.advance(Duration::from_secs(1));
    assert_eq!(count(&bench, 0), (0x7FFF, true));
    assert_eq!(&counters(&bench)[..2], [0xFF, 0xFF]);
    bench.advance(Duration::from_secs(3_600));
    assert_eq!(count(&bench, 0), (0x7FFF, true));
    assert!(sums_to_zero(counters(&bench)));
}

#[test]
fn the_counters_reset_as_hpd_falls_by_default() {
    let bench = locked(SinkConfig::new());
    bench.set_character_errors(ErrorRates::all(1_000));
    bench.advance(Duration::from_millis(5));
    bench.set_character_errors(ErrorRates::NONE);
    bench.set_hpd(false);
    bench.set_hpd(true);
    bench.advance(MS);
    assert_eq!(count(&bench, 0), (0, true));
    // A sink that keeps them.
    let config = SinkConfig::new().with_hpd_reset(RegisterGroup::ErrorCounters, HpdReset::Never);
    let bench = locked(config);
    bench.set_character_errors(ErrorRates::all(1_000));
    bench.advance(Duration::from_millis(5));
    bench.set_character_errors(ErrorRates::NONE);
    bench.set_hpd(false);
    bench.set_hpd(true);
    bench.advance(MS);
    assert_eq!(count(&bench, 0), (5, true));
}

#[test]
fn an_unplugged_sink_counts_nothing() {
    let bench = locked(SinkConfig::new());
    bench.set_character_errors(ErrorRates::all(1_000));
    bench.unplug();
    bench.advance(Duration::from_millis(10));
    assert_eq!(bench.character_errors(), ErrorRates::all(1_000));
    bench.plug(&[], None, SinkConfig::new()).unwrap();
    assert_eq!(counters(&bench), [0; 7]);
}

#[test]
fn a_preset_holds_until_the_counts_change() {
    let bench = locked(SinkConfig::new());
    bench.preset_scdc(0x50, 0x12);
    bench.advance(MS);
    assert_eq!(bench.sink().scdc(0x50), Some(0x12));
    bench.set_character_errors(ErrorRates::all(1_000));
    bench.advance(MS);
    assert_eq!(count(&bench, 0), (1, true));
}

#[test]
fn the_rates_are_a_bench_event_and_an_action() {
    let bench = locked(SinkConfig::new());
    let rates = ErrorRates::all(1_000);
    bench
        .schedule_action(
            Action::CharacterErrors(rates),
            When::At(bench.now().checked_add(MS).unwrap()),
        )
        .unwrap();
    // Taken at the first bench event at or after its time.
    bench.advance(MS);
    bench.advance(Duration::from_millis(2));
    assert_eq!(bench.character_errors(), rates);
    assert_eq!(count(&bench, 0), (2, true));
    // The same rates again record nothing.
    bench.set_character_errors(rates);
    let changes: Vec<_> = bench
        .transcript()
        .filter_map(|record| match record.event {
            Event::CharacterErrors(rates) => Some(rates),
            _ => None,
        })
        .collect();
    assert_eq!(changes, [rates]);
}

const SECOND: Duration = Duration::from_secs(1);

fn ced_update(bench: &Bench<'_>) -> bool {
    bench.sink().update_0().unwrap() & 0x02 != 0
}

#[test]
fn ced_update_is_set_by_a_rise_of_4_within_a_second_check() {
    // Locked at 1 ms; the checks fall at whole seconds of the bench's clock.
    let bench = locked(SinkConfig::new());
    bench.set_character_errors(ErrorRates {
        lane2: 4,
        ..ErrorRates::NONE
    });
    // At 1 s the count is 3: a rise of 3.
    bench.advance(Duration::from_millis(999));
    assert!(!ced_update(&bench));
    // At 2 s it is 7: a rise of 4.
    bench.advance(SECOND);
    assert!(ced_update(&bench));
    let sets: Vec<_> = bench
        .transcript()
        .filter_map(|record| match record.event {
            Event::Register {
                reg: 0x10,
                to,
                by: Actor::Sink,
                ..
            } => Some((record.at.as_micros(), to)),
            _ => None,
        })
        .collect();
    // Status_Update when the clock was detected and the lanes locked, then CED_Update.
    assert_eq!(sets, [(0, 0x01), (2_000_000, 0x03)]);
}

#[test]
fn a_slower_rise_sets_nothing_until_the_count_saturates() {
    let bench = locked(SinkConfig::new());
    bench.set_character_errors(ErrorRates::all(3));
    bench.advance(Duration::from_secs(10));
    assert!(!ced_update(&bench));
    // 0x7FFF errors at 3 a second take 10 923 s: the check that sees it sets the flag.
    bench.advance(Duration::from_secs(10_000));
    assert!(!ced_update(&bench));
    bench.advance(Duration::from_secs(1_000));
    assert!(ced_update(&bench));
    assert_eq!(count(&bench, 0), (0x7FFF, true));
}

#[test]
fn the_source_clears_ced_update_and_a_later_rise_sets_it_again() {
    let bench = locked(SinkConfig::new());
    let bus = bench.ddc();
    bench.set_character_errors(ErrorRates::all(5));
    // A long advance: the flag is set at the first second, and the checks after it are
    // skipped, keeping the counts at the last.
    bench.advance(Duration::from_secs(100));
    assert!(ced_update(&bench));
    bench.set_character_errors(ErrorRates::NONE);
    bus.write(SCDC, &[0x10, 0x02]).unwrap();
    bench.advance(SECOND);
    assert!(!ced_update(&bench), "no rise since the last check");
    bench.set_character_errors(ErrorRates::all(5));
    bench.advance(SECOND);
    assert!(ced_update(&bench));
}

#[test]
fn a_lane_not_locked_sets_nothing() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    bench.set_character_errors(ErrorRates::all(1_000));
    bench.set_line(Line::Off);
    bench.advance(Duration::from_secs(5));
    assert_eq!(bench.sink().update_0(), Some(0));
}

#[test]
fn a_sink_that_never_sets_ced_update() {
    let config = SinkConfig::new().with_ced_update(CedUpdate::Never);
    assert_eq!(config.ced_update(), CedUpdate::Never);
    let bench = locked(config);
    bench.set_character_errors(ErrorRates::all(u32::MAX));
    bench.advance(Duration::from_secs(5));
    assert!(!ced_update(&bench));
    assert_eq!(count(&bench, 0), (0x7FFF, true));
}

#[test]
fn a_rise_of_0_is_taken_as_1() {
    let config = SinkConfig::new().with_ced_update(CedUpdate::EverySecond { rise: 0 });
    let bench = locked(config);
    bench.advance(Duration::from_secs(3));
    assert!(!ced_update(&bench), "no rise, no flag");
    bench.set_character_errors(ErrorRates {
        lane0: 1,
        ..ErrorRates::NONE
    });
    bench.advance(Duration::from_secs(2));
    assert!(ced_update(&bench));
}
