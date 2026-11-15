//! The simulated PHY: its contract, its settings, the line it drives, its records and faults.

use core::time::Duration;

use maquette::hdmi_hal::ddc::SCDC;
use maquette::{
    Bench, ColorBitDepth, ColorFormat, ConfigureWhileRunning, DEFAULT_PHY_CAPABILITIES, DdcBus,
    DdcErrorKind, EqParams, Event, Fault, FrlOutput, FrlPhy, HdmiForumFrl, LanePatterns, Line,
    PhyCall, PhyCallKind, PhyError, PhyErrorKind, PhyFault, PhyPower, PhySettings, ScheduleFull,
    ScramblerAfterConfigure, SinkConfig, Times, TmdsClockRatio, TmdsPhy, TmdsPhyCapabilities,
    TmdsPhyConfig, TmdsVideo,
};

/// 4K60 RGB 8-bit: 594 000 kHz at 1/40.
fn high() -> TmdsPhyConfig {
    let video = TmdsVideo::new(594_000, ColorBitDepth::Depth8, ColorFormat::Rgb444);
    TmdsPhyConfig::new(594_000, TmdsClockRatio::Div40, video)
}

/// 1080p60 RGB 8-bit: 148 500 kHz at 1/10.
fn low() -> TmdsPhyConfig {
    let video = TmdsVideo::new(148_500, ColorBitDepth::Depth8, ColorFormat::Rgb444);
    TmdsPhyConfig::new(148_500, TmdsClockRatio::Div10, video)
}

fn line(config: TmdsPhyConfig, scrambled: bool) -> Line {
    Line::Tmds {
        character_rate_khz: config.character_rate_khz,
        ratio: config.clock_ratio,
        scrambled,
    }
}

fn bench() -> Bench<'static> {
    Bench::new(SinkConfig::new(), &[]).unwrap()
}

fn bench_with(settings: PhySettings) -> Bench<'static> {
    Bench::builder(&[]).phy(settings).build().unwrap()
}

fn kind(result: Result<(), PhyError>) -> PhyErrorKind {
    result.unwrap_err().kind
}

/// The PHY calls recorded, with their results.
fn phy_events(bench: &Bench<'_>) -> Vec<(PhyCall, Result<(), PhyErrorKind>)> {
    bench
        .transcript()
        .filter_map(|record| match record.event {
            Event::Phy { call, result, .. } => Some((call, result)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_bring_up_drives_the_line_and_the_sink_locks() {
    let bench = bench();
    let mut phy = bench.phy();
    assert_eq!(phy.capabilities(), DEFAULT_PHY_CAPABILITIES);
    bench.ddc().write(SCDC, &[0x20, 0x03]).unwrap();
    phy.configure(high()).unwrap();
    assert_eq!(
        bench.line(),
        Line::Off,
        "configure leaves the output stopped"
    );
    phy.set_scrambling(true).unwrap();
    let before = bench.now();
    phy.start_output().unwrap();
    // start_output returns once on the wire: 1 ms by default.
    assert_eq!(
        bench.now().as_micros() - before.as_micros(),
        1_000,
        "start latency"
    );
    assert_eq!(bench.line(), line(high(), true));
    bench.advance(Duration::from_millis(1));
    assert_eq!(bench.sink().scrambling_status(), Some(1));
    assert_eq!(
        phy_events(&bench),
        [
            (PhyCall::Configure(high()), Ok(())),
            (PhyCall::SetScrambling(true), Ok(())),
            (PhyCall::StartOutput, Ok(())),
        ]
    );
    phy.stop_output().unwrap();
    assert_eq!(bench.line(), Line::Off);
    assert_eq!(bench.sink().scrambling_status(), Some(0));
}

#[test]
fn a_procedure_that_never_sets_the_scrambler_fails_at_every_rate() {
    // The default scrambler after configure is the opposite of what the rate needs.
    let bench = bench();
    let mut phy = bench.phy();
    phy.configure(high()).unwrap();
    phy.start_output().unwrap();
    assert_eq!(bench.line(), line(high(), false));
    phy.stop_output().unwrap();
    phy.configure(low()).unwrap();
    phy.start_output().unwrap();
    assert_eq!(bench.line(), line(low(), true));
}

#[test]
fn the_scrambler_after_configure_is_a_setting() {
    for (behaviour, after_high, after_low) in [
        (ScramblerAfterConfigure::Opposite, false, true),
        (ScramblerAfterConfigure::Off, false, false),
        (ScramblerAfterConfigure::FromRate, true, false),
    ] {
        let bench = bench_with(PhySettings::DEFAULT.with_scrambler_after_configure(behaviour));
        let mut phy = bench.phy();
        phy.configure(high()).unwrap();
        phy.start_output().unwrap();
        assert_eq!(bench.line(), line(high(), after_high), "{behaviour:?}");
        phy.stop_output().unwrap();
        phy.configure(low()).unwrap();
        phy.start_output().unwrap();
        assert_eq!(bench.line(), line(low(), after_low), "{behaviour:?}");
    }
    let bench = bench_with(
        PhySettings::DEFAULT.with_scrambler_after_configure(ScramblerAfterConfigure::Keep),
    );
    let mut phy = bench.phy();
    phy.configure(high()).unwrap();
    phy.set_scrambling(true).unwrap();
    phy.configure(low()).unwrap();
    phy.start_output().unwrap();
    assert_eq!(bench.line(), line(low(), true), "kept");
}

#[test]
fn configure_with_the_output_running_fails_or_stops_it() {
    let bench = bench();
    let mut phy = bench.phy();
    phy.configure(high()).unwrap();
    phy.start_output().unwrap();
    assert_eq!(kind(phy.configure(low())), PhyErrorKind::Running);
    assert_eq!(bench.line(), line(high(), false), "nothing changed");

    let bench = bench_with(
        PhySettings::DEFAULT.with_configure_while_running(ConfigureWhileRunning::StopFirst),
    );
    let mut phy = bench.phy();
    phy.configure(high()).unwrap();
    phy.start_output().unwrap();
    phy.configure(low()).unwrap();
    assert_eq!(bench.line(), Line::Off, "stopped, and configured");
    phy.start_output().unwrap();
    assert_eq!(bench.line(), line(low(), true));
}

#[test]
fn output_needs_power_and_a_configure_since_the_last_power_on() {
    let bench = bench();
    let mut phy = bench.phy();
    assert_eq!(kind(phy.start_output()), PhyErrorKind::NotConfigured);
    phy.configure(high()).unwrap();
    phy.power_off().unwrap();
    phy.power_off().unwrap();
    assert_eq!(kind(phy.configure(high())), PhyErrorKind::PoweredOff);
    assert_eq!(kind(phy.start_output()), PhyErrorKind::PoweredOff);
    phy.stop_output().unwrap();
    phy.power_on().unwrap();
    // Power-off forgot the configuration.
    assert_eq!(kind(phy.start_output()), PhyErrorKind::NotConfigured);
    phy.configure(high()).unwrap();
    phy.start_output().unwrap();
    // Power-on on a powered PHY changes nothing.
    phy.power_on().unwrap();
    assert_eq!(bench.line(), line(high(), false));
    // Starting a running output changes nothing either.
    phy.start_output().unwrap();
    assert_eq!(bench.line(), line(high(), false));
}

#[test]
fn a_phy_can_start_powered_off() {
    let settings = PhySettings::DEFAULT.with_powered(false);
    assert!(!settings.powered);
    let bench = bench_with(settings);
    let mut phy = bench.phy();
    assert_eq!(kind(phy.configure(high())), PhyErrorKind::PoweredOff);
    phy.power_on().unwrap();
    phy.configure(high()).unwrap();
}

/// TMDS at `rate` kHz and `ratio`.
fn at(rate: u32, ratio: TmdsClockRatio) -> TmdsPhyConfig {
    let video = TmdsVideo::new(rate, ColorBitDepth::Depth8, ColorFormat::Rgb444);
    TmdsPhyConfig::new(rate, ratio, video)
}

#[test]
fn a_rate_of_340_000_khz_is_low() {
    let at_340 = at(340_000, TmdsClockRatio::Div10);
    let above = at(340_001, TmdsClockRatio::Div40);
    // The scrambler after configure: low rates on with Opposite, off with FromRate.
    for (behaviour, on_at_340, on_above) in [
        (ScramblerAfterConfigure::Opposite, true, false),
        (ScramblerAfterConfigure::FromRate, false, true),
    ] {
        let bench = bench_with(PhySettings::DEFAULT.with_scrambler_after_configure(behaviour));
        let mut phy = bench.phy();
        for (config, scrambled) in [(at_340, on_at_340), (above, on_above)] {
            phy.configure(config).unwrap();
            phy.start_output().unwrap();
            assert_eq!(bench.line(), line(config, scrambled), "{behaviour:?}");
            phy.stop_output().unwrap();
        }
    }
    // A scrambler that does not work at low rates refuses at 340 000 kHz, not above.
    let caps = TmdsPhyCapabilities::new(600_000).with_scrambling(true);
    let bench = Bench::builder(&[]).phy_capabilities(caps).build().unwrap();
    let mut phy = bench.phy();
    phy.configure(at_340).unwrap();
    assert_eq!(kind(phy.set_scrambling(true)), PhyErrorKind::Unsupported);
    phy.configure(above).unwrap();
    phy.set_scrambling(true).unwrap();
}

#[test]
fn what_the_phy_cannot_do_fails() {
    let caps = TmdsPhyCapabilities::new(340_000).with_scrambling(true);
    let bench = Bench::builder(&[]).phy_capabilities(caps).build().unwrap();
    let mut phy = bench.phy();
    assert_eq!(phy.capabilities(), caps);
    assert_eq!(kind(phy.configure(high())), PhyErrorKind::Unsupported);
    phy.configure(low()).unwrap();
    // A scrambler, but not at low rates.
    assert_eq!(kind(phy.set_scrambling(true)), PhyErrorKind::Unsupported);
    phy.set_scrambling(false).unwrap();

    let bench = Bench::builder(&[])
        .phy_capabilities(TmdsPhyCapabilities::new(600_000))
        .build()
        .unwrap();
    let mut phy = bench.phy();
    phy.configure(high()).unwrap();
    assert_eq!(kind(phy.set_scrambling(true)), PhyErrorKind::Unsupported);
    phy.set_scrambling(false).unwrap();
}

#[test]
fn entering_one_mode_leaves_the_other() {
    let bench = bench();
    let mut phy = bench.phy();
    phy.configure(high()).unwrap();
    phy.start_output().unwrap();
    phy.set_frl_rate(HdmiForumFrl::Rate6Gbps4Lanes).unwrap();
    assert_eq!(bench.line(), Line::frl(HdmiForumFrl::Rate6Gbps4Lanes));
    // FRL left TMDS: the configuration is gone.
    assert_eq!(kind(phy.start_output()), PhyErrorKind::NotConfigured);
    phy.send_ltp(LanePatterns::default()).unwrap();
    phy.set_frl_output(FrlOutput::Active).unwrap();
    phy.adjust_equalization(EqParams::new()).unwrap();
    // configure leaves FRL.
    phy.configure(high()).unwrap();
    assert_eq!(bench.line(), Line::Off);
    assert_eq!(
        kind(phy.send_ltp(LanePatterns::default())),
        PhyErrorKind::NotInFrl
    );
    // stop_output and NotSupported leave FRL too.
    phy.set_frl_rate(HdmiForumFrl::Rate3Gbps3Lanes).unwrap();
    phy.stop_output().unwrap();
    assert_eq!(bench.line(), Line::Off);
    assert_eq!(
        kind(phy.set_frl_output(FrlOutput::GapOnly)),
        PhyErrorKind::NotInFrl
    );
    phy.set_frl_rate(HdmiForumFrl::Rate3Gbps3Lanes).unwrap();
    phy.set_frl_rate(HdmiForumFrl::NotSupported).unwrap();
    assert_eq!(bench.line(), Line::Off);
    assert_eq!(
        kind(phy.adjust_equalization(EqParams::new())),
        PhyErrorKind::NotInFrl
    );
    // NotSupported outside FRL enters nothing and leaves TMDS alone.
    phy.configure(high()).unwrap();
    phy.set_frl_rate(HdmiForumFrl::NotSupported).unwrap();
    phy.start_output().unwrap();
    phy.power_off().unwrap();
    assert_eq!(
        kind(phy.set_frl_rate(HdmiForumFrl::Rate3Gbps3Lanes)),
        PhyErrorKind::PoweredOff
    );
    assert_eq!(
        kind(phy.send_ltp(LanePatterns::default())),
        PhyErrorKind::PoweredOff
    );
}

#[test]
fn a_tests_line_holds_until_the_phys_output_changes() {
    let bench = bench();
    let mut phy = bench.phy();
    let set = line(low(), true);
    bench.set_line(set);
    // Calls that change nothing on the PHY's output leave the test's line.
    phy.set_scrambling(true).unwrap();
    phy.configure(high()).unwrap();
    assert_eq!(bench.line(), set);
    phy.start_output().unwrap();
    assert_eq!(bench.line(), line(high(), false));
    // And the test's line wins again until the next change.
    bench.set_line(Line::Off);
    phy.set_scrambling(false).unwrap();
    assert_eq!(bench.line(), Line::Off);
    phy.set_scrambling(true).unwrap();
    assert_eq!(bench.line(), line(high(), true));
}

#[test]
fn every_call_is_recorded_with_its_result_and_line_change() {
    let bench = bench();
    let mut phy = bench.phy();
    let _ = phy.start_output();
    phy.configure(low()).unwrap();
    phy.start_output().unwrap();
    let events: Vec<Event> = bench.transcript().map(|record| record.event).collect();
    assert_eq!(events.len(), 4);
    assert_eq!(events[3], Event::Line(line(low(), true)));
    assert_eq!(
        phy_events(&bench),
        [
            (PhyCall::StartOutput, Err(PhyErrorKind::NotConfigured)),
            (PhyCall::Configure(low()), Ok(())),
            (PhyCall::StartOutput, Ok(())),
        ]
    );
}

#[test]
fn a_phy_fault_fails_a_call_before_or_after_it_takes_effect() {
    let bench = bench();
    let mut phy = bench.phy();
    bench
        .schedule_phy(PhyFault::new().on(PhyCallKind::Configure))
        .unwrap();
    let error = phy.configure(high()).unwrap_err();
    assert_eq!(
        (error.kind, error.call, error.took_effect),
        (PhyErrorKind::Fault, PhyCall::Configure(high()), false)
    );
    assert_eq!(kind(phy.start_output()), PhyErrorKind::NotConfigured);
    phy.configure(high()).unwrap();
    bench
        .schedule_phy(PhyFault::new().on(PhyCallKind::StartOutput).after_effect())
        .unwrap();
    let error = phy.start_output().unwrap_err();
    assert!(error.took_effect);
    assert_eq!(bench.line(), line(high(), false), "it took effect");
    assert_eq!(
        error.to_string(),
        "StartOutput failed: Fault (after it took effect)"
    );
}

#[test]
fn phy_faults_count_matching_calls_and_share_the_schedule() {
    let bench = Bench::builder(&[]).capacities::<16, 2>().build().unwrap();
    let mut phy = bench.phy();
    bench
        .schedule_phy(PhyFault::new().nth(2).times(Times::Count(2)))
        .unwrap();
    bench
        .schedule(Fault::new(maquette::Effect::Fail(DdcErrorKind::Busy)))
        .unwrap();
    assert_eq!(bench.schedule_phy(PhyFault::default()), Err(ScheduleFull));
    // Bus operations do not count for PHY faults, nor PHY calls for bus faults.
    assert!(bench.ddc().write(SCDC, &[0x02, 1]).is_err());
    let results: Vec<bool> = (0..4).map(|_| phy.stop_output().is_ok()).collect();
    assert_eq!(results, [true, false, false, true]);
    assert_eq!(bench.scheduled_faults(), 0);
}

#[test]
fn a_fault_after_a_call_that_fails_anyway_reports_the_calls_error() {
    let bench = bench();
    let mut phy = bench.phy();
    bench.schedule_phy(PhyFault::new().after_effect()).unwrap();
    let error = phy.start_output().unwrap_err();
    assert_eq!(error.kind, PhyErrorKind::NotConfigured);
    assert!(!error.took_effect);
    assert_eq!(error.to_string(), "StartOutput failed: NotConfigured");
}

#[test]
fn phy_faults_pass_over_bus_faults_and_freed_slots() {
    let bench = bench();
    let mut phy = bench.phy();
    let freed = bench.schedule_phy(PhyFault::new()).unwrap();
    bench
        .schedule(Fault::new(maquette::Effect::Fail(DdcErrorKind::Busy)))
        .unwrap();
    bench
        .schedule_phy(PhyFault::new().on(PhyCallKind::StopOutput))
        .unwrap();
    assert!(bench.clear_fault(freed));
    assert_eq!(kind(phy.stop_output()), PhyErrorKind::Fault);
    // The bus fault is still there for the bus.
    assert_eq!(bench.scheduled_faults(), 1);
}

#[test]
fn each_phy_call_counts_against_the_budget() {
    let bench = bench();
    let mut phy = bench.phy();
    let _ = phy.capabilities();
    phy.power_on().unwrap();
    phy.stop_output().unwrap();
    assert_eq!(
        bench.operations(),
        2,
        "capabilities is a report, not a call"
    );
}

#[test]
fn two_handles_are_one_phy() {
    let bench = bench();
    let mut first = bench.phy();
    let mut second = bench.phy();
    first.configure(high()).unwrap();
    second.start_output().unwrap();
    assert_eq!(bench.line(), line(high(), false));
    fn send_sync_copy<T: Send + Sync + Copy>() {}
    send_sync_copy::<maquette::Phy<'_>>();
    assert!(format!("{first:?}").starts_with("Phy { capabilities: "));
}

#[test]
fn generic_code_bounded_on_the_traits_runs_on_the_handle() {
    fn bring_up<P>(phy: &mut P, config: TmdsPhyConfig) -> Result<(), <P as TmdsPhy>::Error>
    where
        P: TmdsPhy + PhyPower<Error = <P as TmdsPhy>::Error>,
    {
        phy.power_on()?;
        phy.configure(config)?;
        phy.set_scrambling(true)?;
        phy.start_output()
    }
    let bench = bench();
    bring_up(&mut bench.phy(), high()).unwrap();
    assert_eq!(bench.line(), line(high(), true));
}

#[test]
fn calls_name_their_kind() {
    let calls = [
        (PhyCall::Configure(high()), PhyCallKind::Configure),
        (PhyCall::SetScrambling(true), PhyCallKind::SetScrambling),
        (PhyCall::StartOutput, PhyCallKind::StartOutput),
        (PhyCall::StopOutput, PhyCallKind::StopOutput),
        (PhyCall::PowerOn, PhyCallKind::PowerOn),
        (PhyCall::PowerOff, PhyCallKind::PowerOff),
        (
            PhyCall::SetFrlRate(HdmiForumFrl::NotSupported),
            PhyCallKind::SetFrlRate,
        ),
        (
            PhyCall::SendLtp(LanePatterns::default()),
            PhyCallKind::SendLtp,
        ),
        (
            PhyCall::SetFrlOutput(FrlOutput::GapOnly),
            PhyCallKind::SetFrlOutput,
        ),
        (
            PhyCall::AdjustEqualization(EqParams::new()),
            PhyCallKind::AdjustEqualization,
        ),
    ];
    for (call, kind) in calls {
        assert_eq!(call.kind(), kind);
    }
}

#[test]
fn settings_and_their_defaults() {
    let settings = PhySettings::default();
    assert_eq!(settings, PhySettings::DEFAULT);
    assert!(settings.powered);
    assert_eq!(
        settings.configure_while_running,
        ConfigureWhileRunning::Fail
    );
    assert_eq!(
        settings.scrambler_after_configure,
        ScramblerAfterConfigure::Opposite
    );
    assert_eq!(settings.start_latency, Duration::from_millis(1));
    let settings = settings.with_start_latency(Duration::ZERO);
    let bench = bench_with(settings);
    let mut phy = bench.phy();
    phy.configure(high()).unwrap();
    phy.start_output().unwrap();
    assert_eq!(bench.now().as_micros(), 0);
    assert!(format!("{:?}", Bench::builder(&[])).contains("phy_settings"));
}

#[test]
fn a_phy_fault_passes_over_other_calls() {
    let bench = bench();
    let mut phy = bench.phy();
    bench
        .schedule_phy(PhyFault::new().on(PhyCallKind::PowerOff))
        .unwrap();
    phy.stop_output().unwrap();
    phy.power_on().unwrap();
    assert_eq!(kind(phy.power_off()), PhyErrorKind::Fault);
}
