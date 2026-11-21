//! The character error counters: counted while the lane is locked, valid bits, saturation,
//! the checksum, resets, `CED_Update`, clear on read and torn reads.

use core::time::Duration;

use maquette::hdmi_hal::ddc::SCDC;
use maquette::{
    Action, Actor, Bench, CedRead, CedUpdate, DdcBus, DuringRead, ErrorRates, Event, HpdReset,
    Line, RegisterGroup, SinkConfig, TmdsClockRatio, When,
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

/// Reads the counters 0x50–0x56 in one burst: the offset written, then seven bytes, the
/// first of them the fourth byte on the wire (two for the write, the read's address).
fn read_counters(bench: &Bench<'_>) -> [u8; 7] {
    let mut read = [0u8; 7];
    bench.ddc().write_read(SCDC, &[0x50], &mut read).unwrap();
    read
}

/// One error a microsecond on lane 0: at the default 90 µs a byte, 90 more each byte.
const FAST: ErrorRates = ErrorRates {
    lane0: 1_000_000,
    ..ErrorRates::NONE
};

#[test]
fn a_held_burst_reads_the_counters_as_they_were_at_its_start() {
    let bench = locked(SinkConfig::new());
    bench.set_character_errors(FAST);
    let read = read_counters(&bench);
    // The burst started as the errors did: every count 0, consistent with its checksum.
    assert_eq!(read, [0, 0x80, 0, 0x80, 0, 0x80, 0x80]);
    assert!(sums_to_zero(read));
}

#[test]
fn a_moving_burst_reads_each_byte_at_its_moment_and_can_tear() {
    let config = SinkConfig::new().with_during_read(DuringRead::Moving);
    assert_eq!(config.during_read(), DuringRead::Moving);
    let bench = locked(config);
    bench.set_character_errors(FAST);
    let read = read_counters(&bench);
    // Lane 0's low byte at 270 µs (count 270), its high byte at 360 µs (count 360).
    assert_eq!(read[0], (270u16 & 0xFF) as u8);
    assert_eq!(read[1], 0x80 | (360u16 >> 8) as u8);
    // The checksum, at 810 µs, is for counts the burst did not read.
    assert!(!sums_to_zero(read));
    // Still: the sink's own bytes are consistent.
    assert!(sums_to_zero(counters(&bench)));
}

#[test]
fn a_counter_read_clears_it_with_clear_on_read() {
    let config = SinkConfig::new().with_ced_read(CedRead::ClearOnRead);
    assert_eq!(config.ced_read(), CedRead::ClearOnRead);
    let bench = locked(config);
    bench.set_character_errors(ErrorRates::all(1_000));
    bench.advance(Duration::from_millis(10));
    bench.set_character_errors(ErrorRates::NONE);
    // A low byte alone clears nothing.
    let mut low = [0];
    bench.ddc().write_read(SCDC, &[0x50], &mut low).unwrap();
    assert_eq!(low, [10]);
    // A held burst reads the counts and its checksum together, then they clear.
    let read = read_counters(&bench);
    assert_eq!(read, [10, 0x80, 10, 0x80, 10, 0x80, 0x62]);
    assert!(sums_to_zero(read));
    assert_eq!(read_counters(&bench), [0, 0x80, 0, 0x80, 0, 0x80, 0x80]);
}

#[test]
fn a_moving_burst_with_clear_on_read_sees_its_own_clears_in_the_checksum() {
    let config = SinkConfig::new()
        .with_ced_read(CedRead::ClearOnRead)
        .with_during_read(DuringRead::Moving);
    let bench = locked(config);
    bench.set_character_errors(ErrorRates::all(1_000));
    bench.advance(Duration::from_millis(10));
    bench.set_character_errors(ErrorRates::NONE);
    let read = read_counters(&bench);
    // The counts as read, and the checksum after all three cleared.
    assert_eq!(read, [10, 0x80, 10, 0x80, 10, 0x80, 0x80]);
    assert!(!sums_to_zero(read));
}

#[test]
fn after_a_clear_ced_update_measures_the_rise_from_zero() {
    let config = SinkConfig::new().with_ced_read(CedRead::ClearOnRead);
    let bench = locked(config);
    bench.set_character_errors(ErrorRates::all(1_000));
    bench.advance(SECOND);
    bench.set_character_errors(ErrorRates::NONE);
    read_counters(&bench);
    bench.ddc().write(SCDC, &[0x10, 0x02]).unwrap();
    // Four errors by the next check, at 2 s: a rise of 4 from 0, though far below the
    // count the check before the clear saw. Asserted right after that check: a later one
    // would see a rise from the 2 s count whatever the clear did.
    bench.set_character_errors(ErrorRates {
        lane0: 5,
        ..ErrorRates::NONE
    });
    let at = maquette::Timestamp::from_micros(2_100_000);
    bench.advance(at.saturating_duration_since(bench.now()));
    assert_eq!(count(&bench, 0).0, 5);
    assert!(ced_update(&bench));
}

#[test]
fn errors_count_from_the_lock_even_when_one_advance_spans_it() {
    // The line starts and the bench advances 2 s in one step: the lanes lock 1 ms in, and
    // count from there, though the flags change only at the advance's end.
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    bench.set_character_errors(ErrorRates::all(1_000));
    bench.set_line(TMDS_297);
    bench.advance(Duration::from_secs(2));
    assert_eq!(count(&bench, 0), (1_999, true));
    // The checks at 1 s and 2 s saw the counts as they were at those seconds.
    assert!(ced_update(&bench));
}

#[test]
fn a_check_within_a_moving_read_sets_its_flag_at_the_next_event() {
    let config = SinkConfig::new().with_during_read(DuringRead::Moving);
    let bench = locked(config);
    bench.set_character_errors(FAST);
    // The read starts just before the 1 s check, which falls within it.
    let start = maquette::Timestamp::from_micros(999_700);
    bench.advance(start.saturating_duration_since(bench.now()));
    read_counters(&bench);
    assert!(!ced_update(&bench), "not set within the read");
    bench.advance(MS);
    assert!(ced_update(&bench));
    // Recorded at that event, at or after the check's second, never before it.
    let set_at: Vec<u64> = bench
        .transcript()
        .filter_map(|record| match record.event {
            Event::Register {
                reg: 0x10,
                from,
                to,
                by: Actor::Sink,
                ..
            } if from & 0x02 == 0 && to & 0x02 != 0 => Some(record.at.as_micros()),
            _ => None,
        })
        .collect();
    assert_eq!(set_at.len(), 1);
    assert!(set_at[0] >= 1_000_000, "{set_at:?}");
}
