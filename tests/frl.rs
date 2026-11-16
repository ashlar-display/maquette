//! The sink's FRL link training: `FLT_ready`, the requests and `FLT_update`, the pass, the
//! lane locks and `FRL_start`.

use core::time::Duration;

use maquette::hdmi_hal::ddc::SCDC;
use maquette::{
    Bench, DdcBus, FrlOutput, FrlPhy, HdmiForumFrl, HpdReset, LanePatterns, LtpPattern,
    RegisterGroup, SinkConfig, Trigger,
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
