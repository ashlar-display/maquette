//! `Coupled<P>`: any PHY driving the bench's line.

use core::time::Duration;

use maquette::hdmi_hal::ddc::SCDC;
use maquette::{
    Bench, ColorBitDepth, ColorFormat, Coupled, DdcBus, EqParams, Event, FrlOutput, FrlPhy,
    HdmiForumFrl, LanePatterns, Line, LtpPattern, PhyCall, PhyCallKind, PhyErrorKind, PhyFault,
    PhyPower, SinkConfig, TmdsClockRatio, TmdsPhy, TmdsPhyCapabilities, TmdsPhyConfig, TmdsVideo,
};

/// The double's own error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Refused(&'static str);

/// A PHY that does what it is asked, keeps its scrambler across `configure`, records each
/// call, and fails the calls named in `fail`.
#[derive(Debug, Default)]
struct Double {
    calls: Vec<&'static str>,
    fail: Option<&'static str>,
    scrambler: bool,
}

impl Double {
    fn call(&mut self, name: &'static str) -> Result<(), Refused> {
        self.calls.push(name);
        match self.fail {
            Some(failing) if failing == name => Err(Refused(name)),
            _ => Ok(()),
        }
    }
}

impl TmdsPhy for Double {
    type Error = Refused;
    fn capabilities(&self) -> TmdsPhyCapabilities {
        TmdsPhyCapabilities::new(600_000).with_scrambling(true)
    }
    fn configure(&mut self, _config: TmdsPhyConfig) -> Result<(), Refused> {
        self.call("configure")
    }
    fn set_scrambling(&mut self, enabled: bool) -> Result<(), Refused> {
        self.call("set_scrambling")?;
        self.scrambler = enabled;
        Ok(())
    }
    fn start_output(&mut self) -> Result<(), Refused> {
        self.call("start_output")
    }
    fn stop_output(&mut self) -> Result<(), Refused> {
        self.call("stop_output")
    }
}

impl PhyPower for Double {
    type Error = Refused;
    fn power_on(&mut self) -> Result<(), Refused> {
        self.call("power_on")
    }
    fn power_off(&mut self) -> Result<(), Refused> {
        self.call("power_off")
    }
}

impl FrlPhy for Double {
    type Error = Refused;
    fn set_frl_rate(&mut self, _rate: HdmiForumFrl) -> Result<(), Refused> {
        self.call("set_frl_rate")
    }
    fn send_ltp(&mut self, _patterns: LanePatterns) -> Result<(), Refused> {
        self.call("send_ltp")
    }
    fn set_frl_output(&mut self, _output: FrlOutput) -> Result<(), Refused> {
        self.call("set_frl_output")
    }
    fn adjust_equalization(&mut self, _params: EqParams) -> Result<(), Refused> {
        self.call("adjust_equalization")
    }
}

fn high() -> TmdsPhyConfig {
    let video = TmdsVideo::new(594_000, ColorBitDepth::Depth8, ColorFormat::Rgb444);
    TmdsPhyConfig::new(594_000, TmdsClockRatio::Div40, video)
}

fn tmds(scrambled: bool) -> Line {
    Line::Tmds {
        character_rate_khz: 594_000,
        ratio: TmdsClockRatio::Div40,
        scrambled,
    }
}

fn bench() -> Bench<'static> {
    Bench::new(SinkConfig::new(), &[]).unwrap()
}

#[test]
fn a_bring_up_through_a_coupled_phy_locks_the_sink() {
    let bench = bench();
    let mut phy = Coupled::new(Double::default(), &bench);
    bench.ddc().write(SCDC, &[0x20, 0x03]).unwrap();
    phy.configure(high()).unwrap();
    assert_eq!(bench.line(), Line::Off);
    phy.set_scrambling(true).unwrap();
    phy.start_output().unwrap();
    assert_eq!(bench.line(), tmds(true));
    bench.advance(Duration::from_millis(1));
    assert_eq!(bench.sink().scrambling_status(), Some(1));
    assert_eq!(
        phy.get_ref().calls,
        ["configure", "set_scrambling", "start_output"]
    );
}

#[test]
fn the_line_is_unscrambled_after_configure_until_set_scrambling() {
    let bench = bench();
    let mut phy = Coupled::new(Double::default(), &bench);
    phy.set_scrambling(true).unwrap();
    // The double keeps its scrambler, but the bench cannot see that.
    phy.configure(high()).unwrap();
    phy.start_output().unwrap();
    assert!(phy.get_ref().scrambler);
    assert_eq!(bench.line(), tmds(false));
    phy.set_scrambling(true).unwrap();
    assert_eq!(bench.line(), tmds(true));
}

#[test]
fn errors_come_back_unchanged_and_a_failed_call_changes_nothing() {
    let bench = bench();
    let mut phy = Coupled::new(Double::default(), &bench);
    phy.configure(high()).unwrap();
    phy.get_mut().fail = Some("start_output");
    assert_eq!(phy.start_output(), Err(Refused("start_output")));
    assert_eq!(bench.line(), Line::Off);
    let events: Vec<_> = bench
        .transcript()
        .filter_map(|record| match record.event {
            Event::Phy { call, result, .. } => Some((call, result)),
            _ => None,
        })
        .collect();
    assert_eq!(
        events,
        [
            (PhyCall::Configure(high()), Ok(())),
            (PhyCall::StartOutput, Err(PhyErrorKind::Coupled)),
        ]
    );
}

#[test]
fn calls_count_against_the_budget_and_capabilities_do_not() {
    let bench = bench();
    let mut phy = Coupled::new(Double::default(), &bench);
    assert!(phy.capabilities().scrambling);
    phy.stop_output().unwrap();
    phy.power_on().unwrap();
    assert_eq!(bench.operations(), 2);
}

#[test]
fn frl_and_power_pass_through_and_follow_the_contract() {
    let bench = bench();
    let mut phy = Coupled::new(Double::default(), &bench);
    phy.set_frl_rate(HdmiForumFrl::Rate6Gbps4Lanes).unwrap();
    let frl = Line::frl(HdmiForumFrl::Rate6Gbps4Lanes);
    assert_eq!(bench.line(), frl);
    // The patterns, the output and the equalization go on the line.
    let patterns = LanePatterns {
        lane0: Some(LtpPattern::Lfsr0),
        ..LanePatterns::default()
    };
    phy.send_ltp(patterns).unwrap();
    phy.set_frl_output(FrlOutput::Active).unwrap();
    phy.adjust_equalization(EqParams::new()).unwrap();
    assert_eq!(
        bench.line(),
        Line::frl_with(HdmiForumFrl::Rate6Gbps4Lanes, patterns, FrlOutput::Active)
            .with_equalization(EqParams::new())
    );
    // Entering a rate again starts from no patterns and gap characters.
    phy.set_frl_rate(HdmiForumFrl::Rate6Gbps4Lanes).unwrap();
    assert_eq!(bench.line(), frl);
    phy.set_frl_rate(HdmiForumFrl::NotSupported).unwrap();
    assert_eq!(bench.line(), Line::Off);
    phy.set_frl_rate(HdmiForumFrl::Rate6Gbps4Lanes).unwrap();
    phy.stop_output().unwrap();
    assert_eq!(bench.line(), Line::Off);
    // NotSupported outside FRL leaves TMDS alone.
    phy.configure(high()).unwrap();
    phy.set_frl_rate(HdmiForumFrl::NotSupported).unwrap();
    phy.start_output().unwrap();
    assert_eq!(bench.line(), tmds(false));
    // Power-off leaves every mode; power-on from off enters none.
    phy.power_off().unwrap();
    assert_eq!(bench.line(), Line::Off);
    phy.power_on().unwrap();
    phy.start_output().unwrap();
    assert_eq!(bench.line(), Line::Off, "nothing configured since power-on");
    // Starting with nothing configured keeps the line off too.
    let bench = self::bench();
    let mut phy = Coupled::new(Double::default(), &bench);
    phy.start_output().unwrap();
    assert_eq!(bench.line(), Line::Off);
}

#[test]
fn the_last_change_to_the_line_wins() {
    let bench = bench();
    let mut coupled = Coupled::new(Double::default(), &bench);
    let mut sim = bench.phy();
    coupled.configure(high()).unwrap();
    coupled.start_output().unwrap();
    assert_eq!(bench.line(), tmds(false));
    bench.set_line(Line::Off);
    // A call that does not change the coupled PHY's output leaves the test's line.
    coupled.set_frl_output(FrlOutput::GapOnly).unwrap();
    assert_eq!(bench.line(), Line::Off);
    coupled.set_scrambling(true).unwrap();
    assert_eq!(bench.line(), tmds(true));
    // The simulated PHY drives the same line.
    sim.set_frl_rate(HdmiForumFrl::Rate3Gbps3Lanes).unwrap();
    assert_eq!(bench.line(), Line::frl(HdmiForumFrl::Rate3Gbps3Lanes));
}

#[test]
fn scheduled_phy_faults_are_the_simulated_phys() {
    let bench = bench();
    bench
        .schedule_phy(PhyFault::new().on(PhyCallKind::Configure))
        .unwrap();
    let mut phy = Coupled::new(Double::default(), &bench);
    phy.configure(high()).unwrap();
    assert_eq!(bench.scheduled_faults(), 1);
}

#[test]
fn the_coupled_phy_can_be_reached_and_taken_back() {
    let bench = bench();
    let mut phy = Coupled::new(Double::default(), &bench);
    phy.stop_output().unwrap();
    // A call through get_mut bypasses the coupling: not recorded, not counted.
    phy.get_mut().start_output().unwrap();
    assert_eq!(bench.operations(), 1);
    assert!(format!("{phy:?}").starts_with("Coupled { phy: Double"));
    let double = phy.into_inner();
    assert_eq!(double.calls, ["stop_output", "start_output"]);
}

#[test]
fn a_tmds_only_phy_couples_too() {
    struct TmdsOnly;
    impl TmdsPhy for TmdsOnly {
        type Error = core::convert::Infallible;
        fn capabilities(&self) -> TmdsPhyCapabilities {
            TmdsPhyCapabilities::new(340_000)
        }
        fn configure(&mut self, _: TmdsPhyConfig) -> Result<(), Self::Error> {
            Ok(())
        }
        fn set_scrambling(&mut self, _: bool) -> Result<(), Self::Error> {
            Ok(())
        }
        fn start_output(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        fn stop_output(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }
    fn bring_up<P: TmdsPhy>(phy: &mut P, config: TmdsPhyConfig) -> Result<(), P::Error> {
        phy.configure(config)?;
        phy.start_output()
    }
    let bench = bench();
    let mut phy = Coupled::new(TmdsOnly, &bench);
    let video = TmdsVideo::new(148_500, ColorBitDepth::Depth8, ColorFormat::Rgb444);
    bring_up(
        &mut phy,
        TmdsPhyConfig::new(148_500, TmdsClockRatio::Div10, video),
    )
    .unwrap();
    assert_eq!(
        bench.line(),
        Line::Tmds {
            character_rate_khz: 148_500,
            ratio: TmdsClockRatio::Div10,
            scrambled: false
        }
    );
}

#[test]
fn every_frl_and_power_call_reaches_the_coupled_phy() {
    let bench = bench();
    let mut phy = Coupled::new(Double::default(), &bench);
    phy.set_frl_rate(HdmiForumFrl::Rate6Gbps4Lanes).unwrap();
    phy.send_ltp(LanePatterns::default()).unwrap();
    phy.set_frl_output(FrlOutput::Active).unwrap();
    phy.adjust_equalization(EqParams::new()).unwrap();
    phy.power_off().unwrap();
    phy.power_on().unwrap();
    assert_eq!(
        phy.get_ref().calls,
        [
            "set_frl_rate",
            "send_ltp",
            "set_frl_output",
            "adjust_equalization",
            "power_off",
            "power_on",
        ]
    );
}

#[test]
fn power_on_while_powered_keeps_the_mode() {
    let bench = bench();
    let mut phy = Coupled::new(Double::default(), &bench);
    phy.configure(high()).unwrap();
    phy.start_output().unwrap();
    assert_eq!(bench.line(), tmds(false));
    phy.power_on().unwrap();
    assert_eq!(bench.line(), tmds(false));
}
