//! Exact boundaries: the budget, the PHY's start latency and rate, the segments and the
//! segment pointer.

use core::time::Duration;

use maquette::hdmi_hal::ddc::SEGMENT_POINTER;
use maquette::{
    Bench, BusOp, Clock, ColorBitDepth, ColorFormat, DdcBus, DdcErrorKind, EddcTarget, Event,
    PhyErrorKind, SinkConfig, TmdsClockRatio, TmdsPhy, TmdsPhyCapabilities, TmdsPhyConfig,
    TmdsVideo,
};

#[test]
fn a_budget_of_n_allows_n_operations() {
    let bench = Bench::builder(&[]).budget(Some(3)).build().unwrap();
    let time = bench.time();
    for _ in 0..3 {
        let _ = time.now();
    }
    assert_eq!(bench.operations(), 3);
}

#[test]
#[should_panic(expected = "operation budget of 3 operations is spent")]
fn the_operation_past_the_budget_panics() {
    let bench = Bench::builder(&[]).budget(Some(3)).build().unwrap();
    let time = bench.time();
    for _ in 0..4 {
        let _ = time.now();
    }
}

fn config(rate: u32) -> TmdsPhyConfig {
    let video = TmdsVideo::new(rate, ColorBitDepth::Depth8, ColorFormat::Rgb444);
    TmdsPhyConfig::new(rate, TmdsClockRatio::Div40, video)
}

#[test]
fn only_starting_the_output_takes_the_start_latency() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    let mut phy = bench.phy();
    phy.configure(config(594_000)).unwrap();
    let before = bench.now();
    phy.start_output().unwrap();
    assert_eq!(
        bench.now(),
        before.checked_add(Duration::from_millis(1)).unwrap()
    );
    // The calls on a running output take no time.
    let running = bench.now();
    phy.set_scrambling(true).unwrap();
    phy.stop_output().unwrap();
    assert_eq!(bench.now(), running);
}

#[test]
fn the_phy_takes_its_maximum_rate_and_not_above() {
    let caps = TmdsPhyCapabilities::new(600_000).with_scrambling(true);
    let bench = Bench::builder(&[]).phy_capabilities(caps).build().unwrap();
    let mut phy = bench.phy();
    phy.configure(config(600_000)).unwrap();
    assert_eq!(
        phy.configure(config(600_001)).unwrap_err().kind,
        PhyErrorKind::Unsupported
    );
}

#[test]
fn segment_0x7f_is_not_refused() {
    let edid = [0u8; 128];
    let bench = Bench::new(SinkConfig::new(), &edid).unwrap();
    let error = bench
        .ddc()
        .read_segmented(EddcTarget::Edid, 0x7F, 0, &mut [0])
        .unwrap_err();
    // The sink has no segment 0x7F; the bus took the call.
    assert_ne!(error.kind, DdcErrorKind::Unsupported);
}

#[test]
fn a_segment_0_read_writes_no_pointer() {
    let edid = [0u8; 384];
    let bench = Bench::new(SinkConfig::new(), &edid).unwrap();
    bench
        .ddc()
        .read_segmented(EddcTarget::Edid, 0, 0, &mut [0])
        .unwrap();
    bench
        .ddc()
        .read_segmented(EddcTarget::Edid, 1, 0, &mut [0])
        .unwrap();
    let written: Vec<(BusOp, u16)> = bench
        .transcript()
        .filter_map(|record| match record.event {
            Event::Bus(bus) => Some((bus.op, bus.written)),
            _ => None,
        })
        .collect();
    // Segment 0: the offset; segment 1: the pointer and the offset.
    assert_eq!(written[0].1, 1);
    assert_eq!(written[1].1, 2);
}

#[test]
fn a_segment_exists_only_where_a_store_has_bytes() {
    // Three EDID blocks: segment 1 holds the third, segment 2 nothing.
    let edid = [0u8; 384];
    let bench = Bench::new(SinkConfig::new(), &edid).unwrap();
    let bus = bench.ddc();
    bus.read_segmented(EddcTarget::Edid, 1, 0, &mut [0])
        .unwrap();
    assert!(
        bus.read_segmented(EddcTarget::Edid, 2, 0, &mut [0])
            .is_err()
    );
    // A DisplayID of exactly one segment has no segment 1.
    let edid = [0u8; 128];
    let displayid = [0u8; 256];
    let bench = Bench::builder(&edid).displayid(&displayid).build().unwrap();
    let bus = bench.ddc();
    assert!(
        bus.read_segmented(EddcTarget::DisplayId, 1, 0, &mut [0])
            .is_err()
    );
}

#[test]
fn a_pointer_of_0_is_taken_without_a_store() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    bench.ddc().write(SEGMENT_POINTER, &[0]).unwrap();
}
