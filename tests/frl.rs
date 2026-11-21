//! The sink's FRL link training: `FLT_ready`, the requests and `FLT_update`, the pass, the
//! lane locks and `FRL_start`.

use core::time::Duration;

use maquette::hdmi_hal::ddc::SCDC;
use maquette::{
    Action, Bench, CedRead, DdcBus, DuringRead, EqParams, ErrorRates, Event, FrlOutput, FrlPass,
    FrlPhy, HdmiForumFrl, HpdReset, LaneLimits, LanePatterns, LinkLanes, LtpPattern, RegisterGroup,
    SinkConfig, Trigger, TxFfeLevel, When,
};

const MS: Duration = Duration::from_millis(1);

const FRL_SINK: SinkConfig =
    SinkConfig::new().with_max_frl_rate(Some(HdmiForumFrl::Rate12Gbps4Lanes));

/// The patterns the default sink requests: LFSR 0–3 on lanes 0–3.
const REQUESTED: LanePatterns = LanePatterns {
    lane0: Some(LtpPattern::Lfsr0),
    lane1: Some(LtpPattern::Lfsr1),
    lane2: Some(LtpPattern::Lfsr2),
    lane3: Some(LtpPattern::Lfsr3),
};

/// `Update_0`, and the requests in 0x41 and 0x42.
fn training(bench: &Bench<'_>) -> (u8, u8, u8) {
    let sink = bench.sink();
    (
        sink.update_0().unwrap(),
        sink.scdc(0x41).unwrap(),
        sink.scdc(0x42).unwrap(),
    )
}

/// The source clears `FLT_update`, as a source does once it has read the requests.
fn clear_flt_update(bench: &Bench<'_>) {
    bench.ddc().write(SCDC, &[0x10, 0x20]).unwrap();
}

#[test]
fn a_sink_trains_and_starts_frl() {
    let bench = Bench::new(FRL_SINK, &[]).unwrap();
    assert_eq!(bench.sink().status_flags_0(), Some(0x40), "FLT_ready");
    let bus = bench.ddc();
    let mut phy = bench.phy();
    // LTS:3 at 6 Gbps, 4 lanes: the requests, with FLT_update.
    bus.write(SCDC, &[0x31, 0x03]).unwrap();
    assert_eq!(training(&bench), (0x20, 0x65, 0x87));
    phy.set_frl_rate(HdmiForumFrl::Rate6Gbps4Lanes).unwrap();
    phy.send_ltp(REQUESTED).unwrap();
    // Nothing before the source clears FLT_update.
    bench.advance(MS);
    assert_eq!(training(&bench), (0x20, 0x65, 0x87));
    clear_flt_update(&bench);
    bench.advance(MS);
    // The pass: no pattern on any lane, with FLT_update.
    assert_eq!(training(&bench), (0x20, 0, 0));
    phy.send_ltp(LanePatterns::default()).unwrap();
    clear_flt_update(&bench);
    bench.advance(MS);
    // LTS:P: four lanes locked (with Status_Update), then FRL_start.
    assert_eq!(bench.sink().status_flags_0(), Some(0x5E));
    assert_eq!(training(&bench).0, 0x11);
    phy.set_frl_output(FrlOutput::Active).unwrap();
    assert_eq!(bench.sink().status_flags_0(), Some(0x5E));
}

#[test]
fn a_rate_above_the_maximum_is_answered_with_a_lower_rate_request() {
    let config = SinkConfig::new().with_max_frl_rate(Some(HdmiForumFrl::Rate6Gbps3Lanes));
    assert_eq!(config.max_frl_rate(), Some(HdmiForumFrl::Rate6Gbps3Lanes));
    let bench = Bench::new(config, &[]).unwrap();
    bench.ddc().write(SCDC, &[0x31, 0x03]).unwrap();
    assert_eq!(training(&bench), (0x20, 0xFF, 0xFF));
    // So is a value that names no rate.
    clear_flt_update(&bench);
    bench.ddc().write(SCDC, &[0x31, 0x09]).unwrap();
    assert_eq!(training(&bench), (0x20, 0xFF, 0xFF));
    // A rate it takes, next.
    clear_flt_update(&bench);
    bench.ddc().write(SCDC, &[0x31, 0x02]).unwrap();
    assert_eq!(training(&bench), (0x20, 0x65, 0x87));
}

#[test]
fn the_pass_needs_every_active_lane_to_carry_its_pattern() {
    let bench = Bench::new(FRL_SINK, &[]).unwrap();
    let mut phy = bench.phy();
    bench.ddc().write(SCDC, &[0x31, 0x03]).unwrap();
    clear_flt_update(&bench);
    phy.set_frl_rate(HdmiForumFrl::Rate6Gbps4Lanes).unwrap();
    // Lane 3 without its pattern.
    phy.send_ltp(LanePatterns {
        lane3: None,
        ..REQUESTED
    })
    .unwrap();
    bench.advance(Duration::from_millis(10));
    assert_eq!(training(&bench), (0, 0x65, 0x87));
    // At another rate.
    phy.set_frl_rate(HdmiForumFrl::Rate8Gbps4Lanes).unwrap();
    phy.send_ltp(REQUESTED).unwrap();
    bench.advance(Duration::from_millis(10));
    assert_eq!(training(&bench), (0, 0x65, 0x87));
    phy.set_frl_rate(HdmiForumFrl::Rate6Gbps4Lanes).unwrap();
    phy.send_ltp(REQUESTED).unwrap();
    bench.advance(MS);
    assert_eq!(training(&bench), (0x20, 0, 0));
}

#[test]
fn at_3_lanes_lane_3_is_not_trained_or_locked() {
    let bench = Bench::new(FRL_SINK, &[]).unwrap();
    let mut phy = bench.phy();
    bench.ddc().write(SCDC, &[0x31, 0x02]).unwrap();
    clear_flt_update(&bench);
    phy.set_frl_rate(HdmiForumFrl::Rate6Gbps3Lanes).unwrap();
    phy.send_ltp(LanePatterns {
        lane3: None,
        ..REQUESTED
    })
    .unwrap();
    bench.advance(MS);
    assert_eq!(training(&bench), (0x20, 0, 0));
    phy.send_ltp(LanePatterns::default()).unwrap();
    clear_flt_update(&bench);
    bench.advance(MS);
    assert_eq!(bench.sink().status_flags_0(), Some(0x4E));
    assert_eq!(training(&bench).0 & 0x10, 0x10);
}

#[test]
fn the_requests_are_a_setting_and_a_lane_requested_0_needs_nothing() {
    let requests = LanePatterns {
        lane0: Some(LtpPattern::Lfsr3),
        lane1: Some(LtpPattern::Lfsr3),
        lane2: Some(LtpPattern::Lfsr3),
        lane3: None,
    };
    let config = FRL_SINK.with_ltp_requests(requests);
    assert_eq!(config.ltp_requests(), requests);
    let bench = Bench::new(config, &[]).unwrap();
    let mut phy = bench.phy();
    bench.ddc().write(SCDC, &[0x31, 0x06]).unwrap();
    assert_eq!(training(&bench), (0x20, 0x88, 0x08));
    clear_flt_update(&bench);
    phy.set_frl_rate(HdmiForumFrl::Rate12Gbps4Lanes).unwrap();
    phy.send_ltp(LanePatterns {
        lane3: Some(LtpPattern::AllOnes),
        ..requests
    })
    .unwrap();
    bench.advance(MS);
    assert_eq!(training(&bench), (0x20, 0, 0));
}

#[test]
fn frl_rate_0_returns_to_legacy_and_a_new_rate_restarts() {
    let bench = Bench::new(FRL_SINK, &[]).unwrap();
    let mut phy = bench.phy();
    let bus = bench.ddc();
    bus.write(SCDC, &[0x31, 0x03]).unwrap();
    clear_flt_update(&bench);
    phy.set_frl_rate(HdmiForumFrl::Rate6Gbps4Lanes).unwrap();
    phy.send_ltp(REQUESTED).unwrap();
    bench.advance(MS);
    phy.send_ltp(LanePatterns::default()).unwrap();
    clear_flt_update(&bench);
    bench.advance(MS);
    assert_eq!(training(&bench).0 & 0x10, 0x10);
    // A new rate: FRL_start cleared, the requests posted again.
    bus.write(SCDC, &[0x31, 0x04]).unwrap();
    let (update, lanes_01, lanes_23) = training(&bench);
    assert_eq!(update & 0x30, 0x20);
    assert_eq!((lanes_01, lanes_23), (0x65, 0x87));
    // FRL_Rate 0: the requests are cleared, and no FLT_update.
    clear_flt_update(&bench);
    bus.write(SCDC, &[0x31, 0x00]).unwrap();
    let (update, lanes_01, lanes_23) = training(&bench);
    assert_eq!(update & 0x30, 0);
    assert_eq!((lanes_01, lanes_23), (0, 0));
}

#[test]
fn a_tmds_only_sink_answers_nothing() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    assert_eq!(SinkConfig::new().max_frl_rate(), None);
    assert_eq!(bench.sink().status_flags_0(), Some(0));
    bench.ddc().write(SCDC, &[0x31, 0x03]).unwrap();
    assert_eq!(bench.sink().config_1(), Some(0x03));
    assert_eq!(training(&bench), (0, 0, 0));
    // NotSupported as the maximum is the same.
    let config = SinkConfig::new().with_max_frl_rate(Some(HdmiForumFrl::NotSupported));
    let bench = Bench::new(config, &[]).unwrap();
    bench.ddc().write(SCDC, &[0x31, 0x03]).unwrap();
    assert_eq!(training(&bench), (0, 0, 0));
}

#[test]
fn an_hpd_low_returns_to_legacy_and_flt_ready_is_set_again() {
    let bench = Bench::new(FRL_SINK, &[]).unwrap();
    bench.ddc().write(SCDC, &[0x31, 0x03]).unwrap();
    bench.set_hpd(false);
    assert_eq!(training(&bench), (0, 0, 0));
    assert_eq!(bench.sink().status_flags_0(), Some(0x40));
    assert_eq!(bench.sink().config_1(), Some(0));
    // With the configuration kept, the requests are still lost.
    let config = FRL_SINK.with_hpd_reset(RegisterGroup::Config, HpdReset::Never);
    let bench = Bench::new(config, &[]).unwrap();
    bench.ddc().write(SCDC, &[0x31, 0x03]).unwrap();
    bench.set_hpd(false);
    assert_eq!(bench.sink().config_1(), Some(0x03));
    assert_eq!(training(&bench), (0, 0, 0));
}

#[test]
fn the_pass_can_come_on_the_sources_polls() {
    let config = FRL_SINK.with_scrambling_lock(Trigger::AfterReads(2));
    let bench = Bench::new(config, &[]).unwrap();
    let mut phy = bench.phy();
    let bus = bench.ddc();
    bus.write(SCDC, &[0x31, 0x03]).unwrap();
    clear_flt_update(&bench);
    phy.set_frl_rate(HdmiForumFrl::Rate6Gbps4Lanes).unwrap();
    phy.send_ltp(REQUESTED).unwrap();
    let mut update = [0];
    bus.write_read(SCDC, &[0x10], &mut update).unwrap();
    assert_eq!(update, [0]);
    bus.write_read(SCDC, &[0x10], &mut update).unwrap();
    assert_eq!(update, [0x20]);
}

#[test]
fn the_maximum_rate_is_taken_and_the_next_is_not() {
    let config = SinkConfig::new().with_max_frl_rate(Some(HdmiForumFrl::Rate10Gbps4Lanes));
    let bench = Bench::new(config, &[]).unwrap();
    bench.ddc().write(SCDC, &[0x31, 0x05]).unwrap();
    assert_eq!(training(&bench), (0x20, 0x65, 0x87));
    clear_flt_update(&bench);
    bench.ddc().write(SCDC, &[0x31, 0x06]).unwrap();
    assert_eq!(training(&bench), (0x20, 0xFF, 0xFF));
}

/// Starts LTS:3 at `rate` and has the source send the requested patterns, `FLT_update`
/// cleared.
fn train_to_patterns(bench: &Bench<'_>, rate: HdmiForumFrl, value: u8) {
    let mut phy = bench.phy();
    bench.ddc().write(SCDC, &[0x31, value]).unwrap();
    clear_flt_update(bench);
    phy.set_frl_rate(rate).unwrap();
    phy.send_ltp(REQUESTED).unwrap();
}

fn tx_ffe(level: u8) -> TxFfeLevel {
    TxFfeLevel::new(level).unwrap()
}

#[test]
fn a_lane_that_needs_a_txffe_level_passes_once_the_source_applies_it() {
    let bench = Bench::new(FRL_SINK, &[]).unwrap();
    let limits = LinkLanes {
        lane1: LaneLimits::NONE.with_min_tx_ffe(tx_ffe(2)),
        ..LinkLanes::NONE
    };
    bench.set_lane_limits(limits);
    assert_eq!(bench.lane_limits(), limits);
    train_to_patterns(&bench, HdmiForumFrl::Rate6Gbps4Lanes, 0x03);
    bench.advance(Duration::from_millis(10));
    assert_eq!(
        training(&bench),
        (0, 0x65, 0x87),
        "lane 1 below its TxFFE level"
    );
    // Level 1 is not enough; level 2 is.
    let mut eq = EqParams::new();
    eq.lane1.tx_ffe_level = tx_ffe(1);
    bench.phy().adjust_equalization(eq).unwrap();
    bench.advance(Duration::from_millis(10));
    assert_eq!(training(&bench).0, 0);
    eq.lane1.tx_ffe_level = tx_ffe(2);
    bench.phy().adjust_equalization(eq).unwrap();
    bench.advance(MS);
    assert_eq!(training(&bench), (0x20, 0, 0));
}

#[test]
fn a_lane_does_not_train_above_its_maximum_rate() {
    let bench = Bench::new(FRL_SINK, &[]).unwrap();
    bench.set_lane_limits(LinkLanes {
        lane3: LaneLimits::NONE.with_max_rate(Some(HdmiForumFrl::Rate6Gbps4Lanes)),
        ..LinkLanes::NONE
    });
    train_to_patterns(&bench, HdmiForumFrl::Rate8Gbps4Lanes, 0x04);
    bench.advance(Duration::from_millis(10));
    assert_eq!(training(&bench), (0, 0x65, 0x87));
    // At its maximum it trains.
    train_to_patterns(&bench, HdmiForumFrl::Rate6Gbps4Lanes, 0x03);
    bench.advance(MS);
    assert_eq!(training(&bench), (0x20, 0, 0));
}

#[test]
fn a_lane_not_used_at_3_lanes_does_not_hold_training_back() {
    let bench = Bench::new(FRL_SINK, &[]).unwrap();
    bench.set_lane_limits(LinkLanes {
        lane3: LaneLimits::NONE
            .with_max_rate(Some(HdmiForumFrl::Rate3Gbps3Lanes))
            .with_min_tx_ffe(tx_ffe(7)),
        ..LinkLanes::NONE
    });
    train_to_patterns(&bench, HdmiForumFrl::Rate6Gbps3Lanes, 0x02);
    bench.advance(MS);
    assert_eq!(training(&bench), (0x20, 0, 0));
}

#[test]
fn the_lane_limits_are_a_bench_event_and_an_action() {
    let bench = Bench::new(FRL_SINK, &[]).unwrap();
    let limits = LinkLanes::all(LaneLimits::NONE.with_min_tx_ffe(tx_ffe(3)));
    bench
        .schedule_action(Action::LaneLimits(limits), When::At(bench.now()))
        .unwrap();
    bench.advance(MS);
    assert_eq!(bench.lane_limits(), limits);
    bench.set_lane_limits(limits);
    let changes: Vec<_> = bench
        .transcript()
        .filter_map(|record| match record.event {
            Event::LaneLimits(lanes) => Some(lanes),
            _ => None,
        })
        .collect();
    assert_eq!(changes, [limits]);
    assert_eq!(LinkLanes::default(), LinkLanes::NONE);
}

/// Advances the bench to `ms` milliseconds after `from`.
fn advance_to(bench: &Bench<'_>, from: maquette::Timestamp, ms: u64) {
    let at = from.checked_add(Duration::from_millis(ms)).unwrap();
    bench.advance(at.saturating_duration_since(bench.now()));
}

/// An FRL sink training at 6 Gbps, 4 lanes, with `FFE_Levels` `levels`, lane 1 needing
/// TxFFE level `needs`; the source sends the requested patterns. Returns when LTS:3 began.
fn failing_lane_1(
    config: SinkConfig,
    levels: u8,
    needs: u8,
) -> (Bench<'static>, maquette::Timestamp) {
    let bench = Bench::new(config, &[]).unwrap();
    bench.set_lane_limits(LinkLanes {
        lane1: LaneLimits::NONE.with_min_tx_ffe(tx_ffe(needs)),
        ..LinkLanes::NONE
    });
    let start = bench.now();
    train_to_patterns(&bench, HdmiForumFrl::Rate6Gbps4Lanes, (levels << 4) | 0x03);
    (bench, start)
}

#[test]
fn without_ffe_levels_a_failing_lane_gets_a_lower_rate_request_after_180_ms() {
    let (bench, start) = failing_lane_1(FRL_SINK, 0, 1);
    assert_eq!(FRL_SINK.lts3_timeout(), Some(Duration::from_millis(180)));
    advance_to(&bench, start, 175);
    assert_eq!(training(&bench), (0, 0x65, 0x87));
    advance_to(&bench, start, 185);
    assert_eq!(training(&bench), (0x20, 0xFF, 0xFF));
}

#[test]
fn each_ffe_level_gets_a_txffe_request_on_the_failing_lanes() {
    // Two FFE levels: three attempts of 60 ms.
    let (bench, start) = failing_lane_1(FRL_SINK, 2, 2);
    let mut phy = bench.phy();
    advance_to(&bench, start, 55);
    assert_eq!(training(&bench), (0, 0x65, 0x87));
    advance_to(&bench, start, 65);
    // Lane 1 only: another TxFFE level.
    assert_eq!(training(&bench), (0x20, 0xE5, 0x87));
    let mut eq = EqParams::new();
    eq.lane1.tx_ffe_level = tx_ffe(1);
    phy.adjust_equalization(eq).unwrap();
    clear_flt_update(&bench);
    // Once cleared, the patterns are requested again.
    assert_eq!(training(&bench), (0x20, 0x65, 0x87));
    clear_flt_update(&bench);
    advance_to(&bench, start, 125);
    assert_eq!(training(&bench), (0x20, 0xE5, 0x87));
    eq.lane1.tx_ffe_level = tx_ffe(2);
    phy.adjust_equalization(eq).unwrap();
    clear_flt_update(&bench);
    clear_flt_update(&bench);
    advance_to(&bench, start, 130);
    // Lane 1 at its level: the pass, before the last attempt runs out.
    assert_eq!(training(&bench), (0x20, 0, 0));
    advance_to(&bench, start, 400);
    assert_eq!(training(&bench), (0x20, 0, 0));
}

#[test]
fn once_the_ffe_levels_are_used_every_lane_gets_a_lower_rate_request() {
    // One FFE level: two attempts of 90 ms; lane 1 never passes.
    let (bench, start) = failing_lane_1(FRL_SINK, 1, 7);
    advance_to(&bench, start, 95);
    assert_eq!(training(&bench), (0x20, 0xE5, 0x87));
    clear_flt_update(&bench);
    clear_flt_update(&bench);
    advance_to(&bench, start, 185);
    assert_eq!(training(&bench), (0x20, 0xFF, 0xFF));
}

#[test]
fn timeouts_due_together_end_with_the_lower_rate_request() {
    let (bench, start) = failing_lane_1(FRL_SINK, 3, 7);
    // Every attempt runs out within one advance: the last one's request is posted.
    advance_to(&bench, start, 500);
    assert_eq!(training(&bench), (0x20, 0xFF, 0xFF));
}

#[test]
fn requests_wait_for_flt_update_to_be_cleared() {
    let bench = Bench::new(FRL_SINK, &[]).unwrap();
    bench.set_lane_limits(LinkLanes::all(LaneLimits::NONE.with_min_tx_ffe(tx_ffe(7))));
    let start = bench.now();
    // FLT_update left set.
    bench.ddc().write(SCDC, &[0x31, 0x03]).unwrap();
    advance_to(&bench, start, 200);
    assert_eq!(training(&bench), (0x20, 0x65, 0x87));
    clear_flt_update(&bench);
    assert_eq!(training(&bench), (0x20, 0xFF, 0xFF));
}

#[test]
fn no_timeouts_with_flt_no_retrain_flt_no_timeout_or_none_set() {
    // FLT_no_retrain, from the source.
    let bench = Bench::new(FRL_SINK, &[]).unwrap();
    bench.ddc().write(SCDC, &[0x30, 0x02]).unwrap();
    let start = bench.now();
    train_to_patterns(&bench, HdmiForumFrl::Rate6Gbps4Lanes, 0x13);
    bench.set_lane_limits(LinkLanes::all(LaneLimits::NONE.with_min_tx_ffe(tx_ffe(7))));
    advance_to(&bench, start, 1_000);
    assert_eq!(training(&bench), (0, 0x65, 0x87));
    // FLT_no_timeout, the sink's test configuration.
    let (bench, start) = failing_lane_1(FRL_SINK, 1, 7);
    bench.preset_scdc(0x35, 0x20);
    advance_to(&bench, start, 1_000);
    assert_eq!(training(&bench), (0, 0x65, 0x87));
    // No timeout set.
    let config = FRL_SINK.with_lts3_timeout(None);
    assert_eq!(config.lts3_timeout(), None);
    let (bench, start) = failing_lane_1(config, 1, 7);
    advance_to(&bench, start, 1_000);
    assert_eq!(training(&bench), (0, 0x65, 0x87));
}

#[test]
fn a_zero_timeout_asks_for_a_lower_rate_at_once() {
    let config = FRL_SINK.with_lts3_timeout(Some(Duration::ZERO));
    let (bench, _) = failing_lane_1(config, 2, 7);
    bench.advance(Duration::from_micros(1));
    assert_eq!(training(&bench), (0x20, 0xFF, 0xFF));
}

/// A sink with `config`, trained to `FRL_start` at `rate` (the `FRL_Rate` value `value`).
fn trained(config: SinkConfig, rate: HdmiForumFrl, value: u8) -> Bench<'static> {
    let bench = Bench::new(config, &[]).unwrap();
    let lanes = LanePatterns {
        lane3: if value >= 3 { REQUESTED.lane3 } else { None },
        ..REQUESTED
    };
    let mut phy = bench.phy();
    bench.ddc().write(SCDC, &[0x31, value]).unwrap();
    clear_flt_update(&bench);
    phy.set_frl_rate(rate).unwrap();
    phy.send_ltp(lanes).unwrap();
    bench.advance(MS);
    phy.send_ltp(LanePatterns::default()).unwrap();
    clear_flt_update(&bench);
    bench.advance(MS);
    assert_eq!(training(&bench).0 & 0x10, 0x10, "FRL_start");
    bench
}

/// The counter bytes 0x50–0x5A.
fn counters(bench: &Bench<'_>) -> [u8; 11] {
    core::array::from_fn(|i| bench.sink().scdc(0x50 + i as u8).unwrap())
}

fn sum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0, |sum, &byte| sum.wrapping_add(byte))
}

#[test]
fn at_4_lanes_lane_3_and_the_corrections_count_and_the_checksum_covers_lane_3() {
    // Held, so a burst reads the bytes the sink holds.
    let config = FRL_SINK.with_during_read(DuringRead::Held);
    let bench = trained(config, HdmiForumFrl::Rate6Gbps4Lanes, 0x03);
    // Error-free: four valid lanes and valid corrections; 4 × 0x80 sums to 0 by itself.
    assert_eq!(
        counters(&bench),
        [0, 0x80, 0, 0x80, 0, 0x80, 0x00, 0, 0x80, 0, 0x80]
    );
    bench.set_character_errors(
        ErrorRates::NONE
            .with_lane3(5_000)
            .with_rs_corrections(7_000),
    );
    bench.advance(Duration::from_millis(2));
    let bytes = counters(&bench);
    assert_eq!(&bytes[7..], [10, 0x80, 14, 0x80]);
    assert_eq!(sum(&bytes[..9]), 0, "0x50-0x58 sum to zero");
    // The source reads the same in one burst.
    let mut read = [0u8; 11];
    bench.ddc().write_read(SCDC, &[0x50], &mut read).unwrap();
    assert_eq!(read, counters(&bench));
}

#[test]
fn at_3_lanes_lane_3_is_not_valid_and_the_checksum_covers_lanes_0_to_2() {
    let bench = trained(FRL_SINK, HdmiForumFrl::Rate6Gbps3Lanes, 0x02);
    bench.set_character_errors(ErrorRates::all(1_000));
    bench.advance(Duration::from_millis(3));
    let bytes = counters(&bench);
    assert_eq!(&bytes[7..9], [0, 0], "lane 3 neither counts nor is valid");
    assert_eq!(bytes[10], 0x80, "the corrections are valid in FRL");
    assert_eq!(sum(&bytes[..7]), 0);
}

#[test]
fn the_corrections_count_only_in_frl() {
    let bench = Bench::new(FRL_SINK, &[]).unwrap();
    bench.set_line(maquette::Line::Tmds {
        character_rate_khz: 297_000,
        ratio: maquette::TmdsClockRatio::Div10,
        scrambled: false,
    });
    bench.set_character_errors(ErrorRates::NONE.with_rs_corrections(1_000_000));
    bench.advance(Duration::from_millis(5));
    assert_eq!(bench.sink().status_flags_0().unwrap() & 0x0E, 0x0E);
    assert_eq!(&counters(&bench)[9..], [0, 0]);
}

#[test]
fn rsed_update_is_set_by_the_corrections_and_ced_update_by_the_lanes() {
    let bench = trained(FRL_SINK, HdmiForumFrl::Rate6Gbps4Lanes, 0x03);
    bench.set_character_errors(ErrorRates::NONE.with_rs_corrections(10));
    bench.advance(Duration::from_secs(2));
    assert_eq!(
        training(&bench).0 & 0x42,
        0x40,
        "RSED_Update, not CED_Update"
    );
    // Lane 3 alone sets CED_Update.
    bench.ddc().write(SCDC, &[0x10, 0x40]).unwrap();
    bench.set_character_errors(ErrorRates::NONE.with_lane3(10));
    bench.advance(Duration::from_secs(2));
    assert_eq!(training(&bench).0 & 0x42, 0x02);
}

#[test]
fn clear_on_read_and_moving_reads_reach_lane_3_and_the_corrections() {
    let config = FRL_SINK
        .with_ced_read(CedRead::ClearOnRead)
        .with_during_read(DuringRead::Moving);
    let bench = trained(config, HdmiForumFrl::Rate6Gbps4Lanes, 0x03);
    bench.set_character_errors(
        ErrorRates::NONE
            .with_lane3(1_000)
            .with_rs_corrections(1_000),
    );
    bench.advance(Duration::from_millis(10));
    bench.set_character_errors(ErrorRates::NONE);
    let mut read = [0u8; 4];
    bench.ddc().write_read(SCDC, &[0x57], &mut read).unwrap();
    assert_eq!(read, [10, 0x80, 10, 0x80]);
    bench.ddc().write_read(SCDC, &[0x57], &mut read).unwrap();
    assert_eq!(read, [0, 0x80, 0, 0x80]);
}

#[test]
fn frl_errors_count_from_the_lock_even_when_one_advance_spans_it() {
    let bench = Bench::new(FRL_SINK, &[]).unwrap();
    let mut phy = bench.phy();
    bench.ddc().write(SCDC, &[0x31, 0x03]).unwrap();
    clear_flt_update(&bench);
    phy.set_frl_rate(HdmiForumFrl::Rate6Gbps4Lanes).unwrap();
    phy.send_ltp(REQUESTED).unwrap();
    bench.advance(MS);
    phy.send_ltp(LanePatterns::default()).unwrap();
    bench.set_character_errors(ErrorRates::NONE.with_lane3(1_000));
    // The lanes lock 1 ms after the patterns stop, within this advance.
    let stopped = bench.now();
    bench.advance(Duration::from_secs(3));
    // One error a millisecond, from the lock 1 ms after the patterns stopped.
    let locked_for = bench.now().saturating_duration_since(stopped) - MS;
    let bytes = counters(&bench);
    assert_eq!(bytes[8] & 0x80, 0x80, "lane 3 valid");
    let count = u16::from_le_bytes([bytes[7], bytes[8] & 0x7F]);
    assert_eq!(u128::from(count), locked_for.as_millis());
}

#[test]
fn rsed_update_comes_from_its_own_checks_while_ced_update_is_set() {
    // CED_Update set by the lanes; the corrections rise 1 a second, below the rise of 4.
    let bench = trained(FRL_SINK, HdmiForumFrl::Rate6Gbps4Lanes, 0x03);
    bench.set_character_errors(ErrorRates::NONE.with_lane0(100).with_rs_corrections(1));
    bench.advance(Duration::from_millis(1_500));
    assert_eq!(training(&bench).0 & 0x42, 0x02);
    // Ten more seconds in one advance: no one-second check saw a rise of 4.
    bench.advance(Duration::from_secs(10));
    assert_eq!(training(&bench).0 & 0x42, 0x02, "no RSED_Update");
    // At 10 a second, it comes.
    bench.set_character_errors(ErrorRates::NONE.with_lane0(100).with_rs_corrections(10));
    bench.advance(Duration::from_secs(3));
    assert_eq!(training(&bench).0 & 0x42, 0x42);
}

#[test]
fn frl_start_waits_for_the_source_to_clear_the_pass() {
    let bench = Bench::new(FRL_SINK, &[]).unwrap();
    let mut phy = bench.phy();
    bench.ddc().write(SCDC, &[0x31, 0x03]).unwrap();
    clear_flt_update(&bench);
    phy.set_frl_rate(HdmiForumFrl::Rate6Gbps4Lanes).unwrap();
    phy.send_ltp(REQUESTED).unwrap();
    bench.advance(MS);
    assert_eq!(training(&bench), (0x20, 0, 0), "the pass");
    // The source stops the patterns but leaves FLT_update set: the lanes lock, and no
    // FRL_start comes.
    phy.send_ltp(LanePatterns::default()).unwrap();
    bench.advance(Duration::from_millis(10));
    assert_eq!(bench.sink().status_flags_0(), Some(0x5E));
    assert_eq!(training(&bench).0 & 0x30, 0x20);
    clear_flt_update(&bench);
    assert_eq!(training(&bench).0 & 0x30, 0x10);
}

#[test]
fn the_last_timeout_asks_for_no_lower_rate_when_every_lane_passes() {
    // The lanes carry their patterns, but the source leaves FLT_update set past the whole
    // LTS:3 time: the sink posts nothing, and once cleared, it passes at the rate.
    let bench = Bench::new(FRL_SINK, &[]).unwrap();
    let mut phy = bench.phy();
    let start = bench.now();
    bench.ddc().write(SCDC, &[0x31, 0x03]).unwrap();
    phy.set_frl_rate(HdmiForumFrl::Rate6Gbps4Lanes).unwrap();
    phy.send_ltp(REQUESTED).unwrap();
    advance_to(&bench, start, 400);
    assert_eq!(training(&bench), (0x20, 0x65, 0x87));
    clear_flt_update(&bench);
    assert_eq!(training(&bench), (0, 0x65, 0x87), "no lower-rate request");
    bench.advance(MS);
    assert_eq!(training(&bench), (0x20, 0, 0));
}

const UNCHECKED_SINK: SinkConfig = FRL_SINK.with_frl_pass(FrlPass::Unchecked);

#[test]
fn an_unchecked_sink_passes_whatever_the_lanes_carry() {
    assert_eq!(FRL_SINK.frl_pass(), FrlPass::Patterns);
    assert_eq!(UNCHECKED_SINK.frl_pass(), FrlPass::Unchecked);
    let bench = Bench::new(UNCHECKED_SINK, &[]).unwrap();
    let mut phy = bench.phy();
    bench.ddc().write(SCDC, &[0x31, 0x03]).unwrap();
    phy.set_frl_rate(HdmiForumFrl::Rate6Gbps4Lanes).unwrap();
    // No patterns sent; once the source clears FLT_update, the pass comes.
    clear_flt_update(&bench);
    bench.advance(MS);
    assert_eq!(training(&bench), (0x20, 0, 0));
    // FRL_start once cleared, though the lanes carry patterns and are not locked.
    phy.send_ltp(REQUESTED).unwrap();
    clear_flt_update(&bench);
    assert_eq!(training(&bench).0 & 0x30, 0x10);
    assert_eq!(bench.sink().status_flags_0().unwrap() & 0x1E, 0);
}

#[test]
fn an_unchecked_sink_gives_up_waiting_for_the_clear_after_100_ms() {
    let bench = Bench::new(UNCHECKED_SINK, &[]).unwrap();
    let start = bench.now();
    bench.ddc().write(SCDC, &[0x31, 0x03]).unwrap();
    // The source never clears FLT_update.
    advance_to(&bench, start, 95);
    assert_eq!(training(&bench), (0x20, 0x65, 0x87));
    advance_to(&bench, start, 105);
    // The pass, 100 ms after the requests plus the lock trigger, its FLT_update still set.
    assert_eq!(training(&bench), (0x20, 0, 0));
    // FRL_start 100 ms after the pass.
    advance_to(&bench, start, 195);
    assert_eq!(training(&bench).0 & 0x10, 0);
    advance_to(&bench, start, 210);
    assert_eq!(training(&bench).0 & 0x10, 0x10);
}

#[test]
fn an_unchecked_sink_has_no_lts3_timeouts_and_still_refuses_a_rate_above_its_maximum() {
    let bench = Bench::new(UNCHECKED_SINK, &[]).unwrap();
    bench.set_lane_limits(LinkLanes::all(
        LaneLimits::NONE.with_min_tx_ffe(TxFfeLevel::MAX),
    ));
    // Three FFE levels: a checking sink would ask for another TxFFE level at 45 ms; this
    // one waits out its 100 ms and passes.
    let start = bench.now();
    bench.ddc().write(SCDC, &[0x31, 0x33]).unwrap();
    advance_to(&bench, start, 60);
    assert_eq!(training(&bench), (0x20, 0x65, 0x87), "no 0xE");
    advance_to(&bench, start, 105);
    assert_eq!(training(&bench), (0x20, 0, 0), "the pass");
    let config = SinkConfig::new()
        .with_max_frl_rate(Some(HdmiForumFrl::Rate6Gbps4Lanes))
        .with_frl_pass(FrlPass::Unchecked);
    let bench = Bench::new(config, &[]).unwrap();
    bench.ddc().write(SCDC, &[0x31, 0x06]).unwrap();
    clear_flt_update(&bench);
    bench.advance(Duration::from_millis(300));
    assert_eq!(training(&bench), (0, 0xFF, 0xFF));
}
