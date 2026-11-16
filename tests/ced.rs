//! The character error counters: counted while the lane is locked, valid bits, saturation,
//! the checksum and resets.

use core::time::Duration;

use maquette::hdmi_hal::ddc::SCDC;
use maquette::{
    Action, Bench, DdcBus, ErrorRates, Event, HpdReset, Line, RegisterGroup, SinkConfig,
    TmdsClockRatio, When,
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
