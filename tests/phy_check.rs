//! The PHY check: it passes the bench's PHY and a PHY coupled to the bench, and catches PHYs
//! broken in one way each.

use maquette::check::{NoProbe, OutputProbe, PhyCheck, PhyFailure, PhyPoint};
use maquette::{
    Bench, ColorBitDepth, ColorFormat, ConfigureWhileRunning, Coupled, EqParams, FrlOutput, FrlPhy,
    HdmiForumFrl, LanePatterns, Line, Phy, PhyErrorKind, PhyPower, PhySettings, SinkConfig,
    TmdsClockRatio, TmdsPhy, TmdsPhyCapabilities, TmdsPhyConfig, TmdsVideo,
};

const RATE: HdmiForumFrl = HdmiForumFrl::Rate6Gbps4Lanes;

fn config(high: bool) -> TmdsPhyConfig {
    let (rate, ratio) = if high {
        (594_000, TmdsClockRatio::Div40)
    } else {
        (148_500, TmdsClockRatio::Div10)
    };
    let video = TmdsVideo::new(rate, ColorBitDepth::Depth8, ColorFormat::Rgb444);
    TmdsPhyConfig::new(rate, ratio, video)
}

fn bench_with(settings: PhySettings) -> Bench<'static> {
    Bench::builder(&[]).phy(settings).build().unwrap()
}

#[test]
fn the_bench_phy_passes_every_run() {
    for behaviour in [
        ConfigureWhileRunning::Fail,
        ConfigureWhileRunning::StopFirst,
    ] {
        for high in [true, false] {
            let settings = PhySettings::DEFAULT.with_configure_while_running(behaviour);
            let bench = bench_with(settings);
            let check = PhyCheck::new(config(high), &bench).with_frl_rate(RATE);
            check.run_tmds(&mut bench.phy()).assert_ok();
            let bench = bench_with(settings);
            let check = PhyCheck::new(config(high), &bench).with_frl_rate(RATE);
            let powered = check.run_powered(&mut bench.phy());
            powered.assert_ok();
            let bench = bench_with(settings);
            let check = PhyCheck::new(config(high), &bench).with_frl_rate(RATE);
            let frl = check.run_frl(&mut bench.phy());
            frl.assert_ok();
            assert!(frl.points() > powered.points());
        }
    }
}

#[test]
fn a_phy_coupled_to_the_bench_passes() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    let mut coupled = Coupled::new(bench.phy(), &bench);
    PhyCheck::new(config(true), &bench)
        .with_frl_rate(RATE)
        .run_frl(&mut coupled)
        .assert_ok();
}

#[test]
fn what_the_capabilities_rule_out_is_refused_or_skipped() {
    // No scrambler: set_scrambling(true) is refused.
    let bench = Bench::builder(&[])
        .phy_capabilities(TmdsPhyCapabilities::new(600_000))
        .build()
        .unwrap();
    PhyCheck::new(config(true), &bench)
        .run_powered(&mut bench.phy())
        .assert_ok();
    // A scrambler, but not at low rates: the low rate's set_scrambling(true) is not a point.
    let caps = TmdsPhyCapabilities::new(600_000).with_scrambling(true);
    let bench = Bench::builder(&[]).phy_capabilities(caps).build().unwrap();
    PhyCheck::new(config(false), &bench)
        .run_powered(&mut bench.phy())
        .assert_ok();
}

#[test]
fn a_probe_that_cannot_tell_skips_its_points() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    let blind = PhyCheck::new(config(true), &NoProbe).run_powered(&mut bench.phy());
    blind.assert_ok();
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    let seeing = PhyCheck::new(config(true), &bench).run_powered(&mut bench.phy());
    assert!(blind.points() < seeing.points());
    assert!(format!("{:?}", PhyCheck::new(config(true), &NoProbe)).starts_with("PhyCheck"));
}

#[cfg(feature = "async")]
#[test]
fn the_async_forms_pass_the_async_phy() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    let check = PhyCheck::new(config(true), &bench).with_frl_rate(RATE);
    pollster::block_on(check.run_tmds_async(&mut bench.phy())).assert_ok();
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    let check = PhyCheck::new(config(true), &bench).with_frl_rate(RATE);
    let report = pollster::block_on(check.run_frl_async(&mut bench.phy()));
    report.assert_ok();
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    let check = PhyCheck::new(config(true), &bench).with_frl_rate(RATE);
    assert_eq!(report.points(), check.run_frl(&mut bench.phy()).points());
}

/// How a [`Broken`] PHY is broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flaw {
    /// `start_output` reports success when the PHY refused.
    StartsUnconfigured,
    /// `configure` with the output running restarts it afterwards.
    ConfigureKeepsRunning,
    /// `stop_output` fails while powered off.
    StopFailsWhenOff,
    /// `configure` in FRL claims success and leaves FRL running.
    ConfigureKeepsFrl,
    /// `power_on` restores the configuration it had.
    PowerOnRemembers,
    /// `set_scrambling(true)` fails above 340 Mcsc, where scrambling is needed.
    RefusesScrambling,
    /// `set_scrambling` reports success and does nothing.
    IgnoresScrambling,
    /// `configure` takes 148 500 kHz at 1/10, whatever it is asked.
    IgnoresRate,
}

/// The bench's PHY with one flaw.
struct Broken<'b> {
    bench: &'b Bench<'static>,
    inner: Phy<'b>,
    flaw: Flaw,
    in_frl: bool,
    powered: bool,
    last: Option<TmdsPhyConfig>,
}

fn broken<'b>(bench: &'b Bench<'static>, flaw: Flaw) -> Broken<'b> {
    Broken {
        bench,
        inner: bench.phy(),
        flaw,
        in_frl: false,
        powered: true,
        last: None,
    }
}

type Kind = PhyErrorKind;

fn kind(result: Result<(), maquette::PhyError>) -> Result<(), Kind> {
    result.map_err(|error| error.kind)
}

impl TmdsPhy for Broken<'_> {
    type Error = Kind;
    fn capabilities(&self) -> TmdsPhyCapabilities {
        self.inner.capabilities()
    }
    fn configure(&mut self, config: TmdsPhyConfig) -> Result<(), Kind> {
        if self.flaw == Flaw::ConfigureKeepsFrl && self.in_frl {
            return Ok(());
        }
        let config = if self.flaw == Flaw::IgnoresRate {
            self::config(false)
        } else {
            config
        };
        let result = kind(self.inner.configure(config));
        if result.is_ok() {
            self.in_frl = false;
            self.last = Some(config);
        }
        if self.flaw == Flaw::ConfigureKeepsRunning && result.is_ok() {
            let _ = self.inner.start_output();
        }
        result
    }
    fn set_scrambling(&mut self, enabled: bool) -> Result<(), Kind> {
        let high = self
            .last
            .is_some_and(|config| config.character_rate_khz > 340_000);
        if self.flaw == Flaw::RefusesScrambling && enabled && high {
            return Err(PhyErrorKind::Unsupported);
        }
        if self.flaw == Flaw::IgnoresScrambling {
            return Ok(());
        }
        kind(self.inner.set_scrambling(enabled))
    }
    fn start_output(&mut self) -> Result<(), Kind> {
        let result = kind(self.inner.start_output());
        if self.flaw == Flaw::StartsUnconfigured {
            return Ok(());
        }
        result
    }
    fn stop_output(&mut self) -> Result<(), Kind> {
        if self.flaw == Flaw::StopFailsWhenOff && !self.powered {
            return Err(PhyErrorKind::PoweredOff);
        }
        let result = kind(self.inner.stop_output());
        self.in_frl = false;
        result
    }
}

impl PhyPower for Broken<'_> {
    type Error = Kind;
    fn power_on(&mut self) -> Result<(), Kind> {
        let was_off = !self.powered;
        let result = kind(self.inner.power_on());
        self.powered = true;
        if self.flaw == Flaw::PowerOnRemembers && was_off {
            if let Some(config) = self.last {
                let _ = self.inner.configure(config);
            }
        }
        result
    }
    fn power_off(&mut self) -> Result<(), Kind> {
        self.powered = false;
        self.in_frl = false;
        kind(self.inner.power_off())
    }
}

impl FrlPhy for Broken<'_> {
    type Error = Kind;
    fn set_frl_rate(&mut self, rate: HdmiForumFrl) -> Result<(), Kind> {
        let result = kind(self.inner.set_frl_rate(rate));
        self.in_frl = result.is_ok() && rate != HdmiForumFrl::NotSupported;
        result
    }
    fn send_ltp(&mut self, patterns: LanePatterns) -> Result<(), Kind> {
        kind(self.inner.send_ltp(patterns))
    }
    fn set_frl_output(&mut self, output: FrlOutput) -> Result<(), Kind> {
        kind(self.inner.set_frl_output(output))
    }
    fn adjust_equalization(&mut self, params: EqParams) -> Result<(), Kind> {
        kind(self.inner.adjust_equalization(params))
    }
}

fn failures(flaw: Flaw, settings: PhySettings) -> Vec<PhyFailure> {
    let bench = bench_with(settings);
    let mut phy = broken(&bench, flaw);
    PhyCheck::new(config(true), &bench)
        .with_frl_rate(RATE)
        .run_frl(&mut phy)
        .failures()
        .copied()
        .collect()
}

#[test]
fn a_phy_that_starts_unconfigured_is_caught() {
    let failures = failures(Flaw::StartsUnconfigured, PhySettings::DEFAULT);
    for point in [
        PhyPoint::StartUnconfigured,
        PhyPoint::StartAfterPowerOn,
        PhyPoint::StartInFrl,
    ] {
        assert!(
            failures.contains(&PhyFailure::Succeeded { point }),
            "{point:?}: {failures:?}"
        );
    }
}

#[test]
fn a_configure_that_leaves_the_output_running_is_caught() {
    let settings =
        PhySettings::DEFAULT.with_configure_while_running(ConfigureWhileRunning::StopFirst);
    let failures = failures(Flaw::ConfigureKeepsRunning, settings);
    assert!(failures.contains(&PhyFailure::Running {
        point: PhyPoint::Configure,
        expected: false,
        found: true,
    }));
    assert!(failures.contains(&PhyFailure::Running {
        point: PhyPoint::ConfigureRunning,
        expected: false,
        found: true,
    }));
}

#[test]
fn a_stop_that_fails_while_off_is_caught() {
    let failures = failures(Flaw::StopFailsWhenOff, PhySettings::DEFAULT);
    assert_eq!(
        failures,
        [PhyFailure::Failed {
            point: PhyPoint::StopPoweredOff
        }]
    );
}

#[test]
fn a_configure_that_does_not_leave_frl_is_caught() {
    let failures = failures(Flaw::ConfigureKeepsFrl, PhySettings::DEFAULT);
    assert_eq!(
        failures,
        [PhyFailure::Running {
            point: PhyPoint::ConfigureLeavesFrl,
            expected: false,
            found: true,
        }]
    );
}

#[test]
fn a_power_on_that_remembers_the_configuration_is_caught() {
    let failures = failures(Flaw::PowerOnRemembers, PhySettings::DEFAULT);
    assert_eq!(
        failures,
        [PhyFailure::Succeeded {
            point: PhyPoint::StartAfterPowerOn
        }]
    );
}

#[test]
fn a_phy_that_refuses_to_scramble_at_high_rates_is_caught() {
    let bench = bench_with(PhySettings::DEFAULT);
    let mut phy = broken(&bench, Flaw::RefusesScrambling);
    let report = PhyCheck::new(config(true), &bench).run_tmds(&mut phy);
    let failures: Vec<PhyFailure> = report.failures().copied().collect();
    for point in [PhyPoint::ScramblerOnStopped, PhyPoint::ScramblerOnRunning] {
        assert!(
            failures.contains(&PhyFailure::Failed { point }),
            "{point:?}: {failures:?}"
        );
    }
}

#[test]
fn at_340_000_khz_a_scrambler_for_high_rates_only_may_refuse() {
    let caps = TmdsPhyCapabilities::new(600_000).with_scrambling(true);
    for (rate, ratio) in [
        (340_000, TmdsClockRatio::Div10),
        (340_001, TmdsClockRatio::Div40),
    ] {
        let bench = Bench::builder(&[]).phy_capabilities(caps).build().unwrap();
        let video = TmdsVideo::new(rate, ColorBitDepth::Depth8, ColorFormat::Rgb444);
        let config = TmdsPhyConfig::new(rate, ratio, video);
        PhyCheck::new(config, &bench)
            .run_powered(&mut bench.phy())
            .assert_ok();
    }
}

/// A probe that reads the PHY it is handed: here, the bench behind the double.
struct ThroughThePhy;

impl OutputProbe<Broken<'_>> for ThroughThePhy {
    fn line(&self, phy: &Broken<'_>) -> Option<Line> {
        Some(phy.bench.line())
    }
}

#[test]
fn a_phy_whose_set_scrambling_does_nothing_is_caught() {
    // After configure the scrambler is off for the high rate; set_scrambling(true) leaves it.
    let failures = failures(Flaw::IgnoresScrambling, PhySettings::DEFAULT);
    assert_eq!(
        failures,
        [PhyFailure::Line {
            point: PhyPoint::ScramblerOnRunning,
            found: Line::Tmds {
                character_rate_khz: 594_000,
                ratio: TmdsClockRatio::Div40,
                scrambled: false
            }
        }]
    );
}

#[test]
fn a_phy_whose_configure_ignores_the_rate_is_caught() {
    // Asked for 594 000 kHz at 1/40, it transmits 148 500 kHz at 1/10.
    let failures = failures(Flaw::IgnoresRate, PhySettings::DEFAULT);
    for point in [PhyPoint::Start, PhyPoint::ScramblerOffRunning] {
        assert!(
            failures.iter().any(|&failure| matches!(
                failure,
                PhyFailure::Line {
                    point: p,
                    found: Line::Tmds {
                        character_rate_khz: 148_500,
                        ratio: TmdsClockRatio::Div10,
                        ..
                    },
                } if p == point
            )),
            "{point:?}: {failures:?}"
        );
    }
}

#[test]
fn a_probe_can_read_the_phy_it_is_handed() {
    let bench = bench_with(PhySettings::DEFAULT);
    let mut phy = broken(&bench, Flaw::IgnoresRate);
    let report = PhyCheck::new(config(true), &ThroughThePhy)
        .with_frl_rate(RATE)
        .run_frl(&mut phy);
    assert!(report.failures().any(|failure| matches!(
        failure,
        PhyFailure::Line {
            point: PhyPoint::Start,
            ..
        }
    )));
}

#[test]
fn run_tmds_catches_a_phy_that_starts_unconfigured() {
    let bench = bench_with(PhySettings::DEFAULT);
    let mut phy = broken(&bench, Flaw::StartsUnconfigured);
    let failures: Vec<PhyFailure> = PhyCheck::new(config(true), &bench)
        .run_tmds(&mut phy)
        .failures()
        .copied()
        .collect();
    assert!(failures.contains(&PhyFailure::Succeeded {
        point: PhyPoint::StartUnconfigured
    }));
}

#[cfg(feature = "async")]
mod asynch {
    use super::*;
    use maquette::hdmi_hal_async::phy::SendTmdsPhy;

    /// The broken PHY behind hdmi-hal-async's `TmdsPhy`.
    struct Async<'b>(Broken<'b>);

    impl SendTmdsPhy for Async<'_> {
        type Error = Kind;

        fn capabilities(&self) -> TmdsPhyCapabilities {
            TmdsPhy::capabilities(&self.0)
        }

        async fn configure(&mut self, config: TmdsPhyConfig) -> Result<(), Kind> {
            TmdsPhy::configure(&mut self.0, config)
        }

        async fn set_scrambling(&mut self, enabled: bool) -> Result<(), Kind> {
            TmdsPhy::set_scrambling(&mut self.0, enabled)
        }

        async fn start_output(&mut self) -> Result<(), Kind> {
            TmdsPhy::start_output(&mut self.0)
        }

        async fn stop_output(&mut self) -> Result<(), Kind> {
            TmdsPhy::stop_output(&mut self.0)
        }
    }

    #[test]
    fn run_tmds_async_catches_a_phy_that_starts_unconfigured() {
        let bench = bench_with(PhySettings::DEFAULT);
        let mut phy = Async(broken(&bench, Flaw::StartsUnconfigured));
        let report =
            pollster::block_on(PhyCheck::new(config(true), &bench).run_tmds_async(&mut phy));
        assert!(report.failures().any(|failure| *failure
            == PhyFailure::Succeeded {
                point: PhyPoint::StartUnconfigured
            }));
    }
}

#[test]
fn failures_print_what_went_wrong() {
    let printed = [
        PhyFailure::Failed {
            point: PhyPoint::Start,
        },
        PhyFailure::Succeeded {
            point: PhyPoint::StartUnconfigured,
        },
        PhyFailure::Running {
            point: PhyPoint::PowerOff,
            expected: false,
            found: true,
        },
        PhyFailure::Line {
            point: PhyPoint::Start,
            found: Line::Off,
        },
    ]
    .map(|failure| failure.to_string());
    assert_eq!(
        printed,
        [
            "Start: the call failed",
            "StartUnconfigured: the call should have failed",
            "PowerOff: the output was running, not stopped",
            "Start: transmits Off, not what the configuration and the calls ask",
        ]
    );
}
