//! The PHY check: hdmi-hal's `TmdsPhy`, `PhyPower` and `FrlPhy`, with a probe for what the
//! traits cannot show.

use core::fmt;

use hdmi_hal::HdmiForumFrl;
use hdmi_hal::phy::TmdsPhyConfig;

use super::Report;
use crate::bench::Bench;
use crate::line::{Line, RATIO_40_ABOVE_KHZ};

/// What a PHY transmits: what the traits cannot show.
///
/// The probe is handed the PHY, so a PHY that knows its own output is probed by a type that
/// reads it.
pub trait OutputProbe<P: ?Sized> {
    /// What `phy` transmits: `Some(Line::Off)` when nothing, `None` when the probe cannot
    /// tell; the points that need it are then skipped.
    fn line(&self, phy: &P) -> Option<Line>;
}

/// The bench's line, whichever PHY drives it.
impl<P: ?Sized, const LOG: usize, const FAULTS: usize> OutputProbe<P> for Bench<'_, LOG, FAULTS> {
    fn line(&self, _phy: &P) -> Option<Line> {
        Some(self.line())
    }
}

/// A probe that cannot tell: the points that need one are skipped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoProbe;

impl<P: ?Sized> OutputProbe<P> for NoProbe {
    fn line(&self, _phy: &P) -> Option<Line> {
        None
    }
}

/// A point of the [`PhyCheck`]: a call, in the state the check put the PHY in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PhyPoint {
    /// `stop_output` on a fresh PHY.
    StopFresh,
    /// `start_output` before any `configure`.
    StartUnconfigured,
    /// `configure`, with the output stopped.
    Configure,
    /// `set_scrambling(false)` with the output stopped.
    ScramblerOffStopped,
    /// `set_scrambling(true)` with the output stopped.
    ScramblerOnStopped,
    /// `start_output` after `configure`.
    Start,
    /// `set_scrambling(false)` with the output running.
    ScramblerOffRunning,
    /// `set_scrambling(true)` with the output running.
    ScramblerOnRunning,
    /// `configure` with the output running: it fails, or stops the output.
    ConfigureRunning,
    /// `stop_output` after the output ran.
    Stop,
    /// `stop_output` on a stopped output.
    StopStopped,
    /// `power_off`.
    PowerOff,
    /// `stop_output` while powered off.
    StopPoweredOff,
    /// `configure` while powered off.
    ConfigurePoweredOff,
    /// `power_on`.
    PowerOn,
    /// `power_on` on a PHY that is on.
    PowerOnAgain,
    /// `start_output` after a power-on, before `configure`.
    StartAfterPowerOn,
    /// `set_frl_rate` with a rate.
    EnterFrl,
    /// `start_output` in FRL, before `configure`.
    StartInFrl,
    /// `configure` in FRL: it leaves FRL.
    ConfigureLeavesFrl,
    /// `stop_output` in FRL: it leaves FRL.
    StopLeavesFrl,
    /// `set_frl_rate(NotSupported)`.
    FrlOff,
}

/// A failed point of the [`PhyCheck`]. A PHY's errors are only `Debug`, so a failure says
/// which point failed and how, not why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PhyFailure {
    /// The call failed where the contract says it succeeds.
    Failed {
        /// The point.
        point: PhyPoint,
    },
    /// The call succeeded where the contract says it fails.
    Succeeded {
        /// The point.
        point: PhyPoint,
    },
    /// After the call, the probe saw the output running when it should not, or not when it
    /// should.
    Running {
        /// The point.
        point: PhyPoint,
        /// Whether it should run.
        expected: bool,
        /// Whether it ran.
        found: bool,
    },
    /// After the call, the output ran but not as the configuration and the calls ask: TMDS
    /// at another rate or ratio, scrambled or not against `set_scrambling`, or FRL at
    /// another rate.
    Line {
        /// The point.
        point: PhyPoint,
        /// What the probe saw.
        found: Line,
    },
}

impl fmt::Display for PhyFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed { point } => write!(f, "{point:?}: the call failed"),
            Self::Succeeded { point } => write!(f, "{point:?}: the call should have failed"),
            Self::Running {
                point,
                expected,
                found,
            } => {
                let state = |running: &bool| if *running { "running" } else { "stopped" };
                write!(
                    f,
                    "{point:?}: the output was {}, not {}",
                    state(found),
                    state(expected)
                )
            }
            Self::Line { point, found } => write!(
                f,
                "{point:?}: transmits {found:?}, not what the configuration and the calls ask"
            ),
        }
    }
}

/// The PHY check: the points of hdmi-hal's PHY contracts, with a configuration the PHY can
/// do and a probe for what it transmits: whether its output runs, and, once it runs, TMDS at
/// the configuration's rate and ratio, scrambled as `set_scrambling` last said, or FRL at
/// the rate entered.
///
/// - **`run_tmds`**, `TmdsPhy` alone, on a PHY fresh from creation: `stop_output` before
///   anything; `start_output` refused before `configure`; `configure` leaving the output
///   stopped; `set_scrambling` with the output stopped and running (`true` only where the
///   capabilities report a scrambler for the rate, and refused without one); `start_output`
///   running the output; `configure` with the output running failing or stopping it;
///   `stop_output`, twice.
/// - **`run_powered`**, with `PhyPower`: a power cycle first, so any PHY is checked from the
///   contract's known state, then the TMDS points, then `power_off` stopping the output,
///   `stop_output` and a refused `configure` while off, `power_on` leaving the output stopped
///   and changing nothing a second time, and `configure` needed again before `start_output`.
/// - **`run_frl`**, with `FrlPhy` too and [`with_frl_rate`](Self::with_frl_rate): the mode
///   rule — a rate enters FRL and leaves TMDS (`start_output` refused until `configure`),
///   `configure` leaves FRL, `stop_output` leaves FRL, `NotSupported` enters nothing.
pub struct PhyCheck<'p, R: ?Sized> {
    config: TmdsPhyConfig,
    probe: &'p R,
    frl_rate: Option<HdmiForumFrl>,
}

impl<R: ?Sized> Clone for PhyCheck<'_, R> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<R: ?Sized> Copy for PhyCheck<'_, R> {}

impl<R: ?Sized> fmt::Debug for PhyCheck<'_, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PhyCheck")
            .field("config", &self.config)
            .field("frl_rate", &self.frl_rate)
            .finish_non_exhaustive()
    }
}

impl<'p, R: ?Sized> PhyCheck<'p, R> {
    /// The check with `config`, one the PHY can do, and `probe` for what it transmits
    /// ([`NoProbe`] when nothing can tell).
    pub const fn new(config: TmdsPhyConfig, probe: &'p R) -> Self {
        Self {
            config,
            probe,
            frl_rate: None,
        }
    }

    /// The check entering FRL at `rate`, one the PHY can do, for the mode rule.
    pub const fn with_frl_rate(self, rate: HdmiForumFrl) -> Self {
        Self {
            frl_rate: Some(rate),
            ..self
        }
    }
}

/// Records the check's points.
struct Points<'r> {
    report: &'r mut Report<PhyFailure>,
}

impl Points<'_> {
    /// A call the contract says succeeds.
    fn ok<E>(&mut self, point: PhyPoint, result: Result<(), E>) -> bool {
        let ok = result.is_ok();
        self.report
            .record((!ok).then_some(PhyFailure::Failed { point }));
        ok
    }

    /// A call the contract says fails.
    fn refused<E>(&mut self, point: PhyPoint, result: Result<(), E>) {
        self.report
            .record(result.is_ok().then_some(PhyFailure::Succeeded { point }));
    }

    /// Whether the output runs after `point`, as the probe sees `line`.
    fn running(&mut self, point: PhyPoint, line: Option<Line>, expected: bool) {
        if let Some(line) = line {
            let found = line != Line::Off;
            self.report
                .record((found != expected).then_some(PhyFailure::Running {
                    point,
                    expected,
                    found,
                }));
        }
    }

    /// Whether the output runs TMDS at `config`'s rate and ratio after `point`, scrambled
    /// as `scrambled` says where it says.
    fn tmds(
        &mut self,
        point: PhyPoint,
        line: Option<Line>,
        config: TmdsPhyConfig,
        scrambled: Option<bool>,
    ) {
        let Some(line) = line else { return };
        self.report.record(match line {
            Line::Off => Some(PhyFailure::Running {
                point,
                expected: true,
                found: false,
            }),
            Line::Tmds {
                character_rate_khz,
                ratio,
                scrambled: found,
            } if character_rate_khz == config.character_rate_khz
                && ratio == config.clock_ratio
                && scrambled.is_none_or(|expected| expected == found) =>
            {
                None
            }
            found => Some(PhyFailure::Line { point, found }),
        });
    }

    /// Whether the output runs FRL at `rate` after `point`.
    fn frl(&mut self, point: PhyPoint, line: Option<Line>, rate: HdmiForumFrl) {
        let Some(line) = line else { return };
        self.report.record(match line {
            Line::Off => Some(PhyFailure::Running {
                point,
                expected: true,
                found: false,
            }),
            Line::Frl { rate: found } if found == rate => None,
            found => Some(PhyFailure::Line { point, found }),
        });
    }
}

/// Whether `set_scrambling(true)` is the contract's to accept for `config` on a PHY with these
/// capabilities: a scrambler, and one that works at the rate.
fn scrambles(config: TmdsPhyConfig, caps: hdmi_hal::phy::TmdsPhyCapabilities) -> bool {
    caps.scrambling
        && (config.character_rate_khz > RATIO_40_ABOVE_KHZ || caps.scrambles_at_low_rate())
}

/// The TMDS points, the power points and the FRL points, once for the sync traits and once,
/// with `.await`, for the async ones.
macro_rules! tmds {
    ($check:ident, $points:ident, $phy:ident, $($await:tt)*) => {{
        use PhyPoint as P;
        let config = $check.config;
        let caps = $phy.capabilities();
        $points.ok(P::StopFresh, $phy.stop_output() $($await)*);
        $points.running(P::StopFresh, $check.probe.line(&*$phy), false);
        $points.refused(P::StartUnconfigured, $phy.start_output() $($await)*);
        $points.running(P::StartUnconfigured, $check.probe.line(&*$phy), false);
        if $points.ok(P::Configure, $phy.configure(config) $($await)*) {
            $points.running(P::Configure, $check.probe.line(&*$phy), false);
            $points.ok(P::ScramblerOffStopped, $phy.set_scrambling(false) $($await)*);
            if scrambles(config, caps) {
                $points.ok(P::ScramblerOnStopped, $phy.set_scrambling(true) $($await)*);
            } else if !caps.scrambling {
                $points.refused(P::ScramblerOnStopped, $phy.set_scrambling(true) $($await)*);
            }
            if $points.ok(P::Start, $phy.start_output() $($await)*) {
                $points.tmds(P::Start, $check.probe.line(&*$phy), config, None);
                if $points.ok(P::ScramblerOffRunning, $phy.set_scrambling(false) $($await)*) {
                    let line = $check.probe.line(&*$phy);
                    $points.tmds(P::ScramblerOffRunning, line, config, Some(false));
                }
                if scrambles(config, caps)
                    && $points.ok(P::ScramblerOnRunning, $phy.set_scrambling(true) $($await)*)
                {
                    let line = $check.probe.line(&*$phy);
                    $points.tmds(P::ScramblerOnRunning, line, config, Some(true));
                }
                // Either behaviour is the contract's; a success stops the output.
                if $phy.configure(config) $($await)*.is_ok() {
                    $points.running(P::ConfigureRunning, $check.probe.line(&*$phy), false);
                }
            }
        }
        $points.ok(P::Stop, $phy.stop_output() $($await)*);
        $points.running(P::Stop, $check.probe.line(&*$phy), false);
        $points.ok(P::StopStopped, $phy.stop_output() $($await)*);
    }};
}

macro_rules! power {
    ($check:ident, $points:ident, $phy:ident, $($await:tt)*) => {{
        use PhyPoint as P;
        let config = $check.config;
        if $points.ok(P::Configure, $phy.configure(config) $($await)*)
            && $points.ok(P::Start, $phy.start_output() $($await)*)
        {
            $points.tmds(P::Start, $check.probe.line(&*$phy), config, None);
            $points.ok(P::PowerOnAgain, $phy.power_on() $($await)*);
            $points.tmds(P::PowerOnAgain, $check.probe.line(&*$phy), config, None);
        }
        $points.ok(P::PowerOff, $phy.power_off() $($await)*);
        $points.running(P::PowerOff, $check.probe.line(&*$phy), false);
        $points.ok(P::StopPoweredOff, $phy.stop_output() $($await)*);
        $points.refused(P::ConfigurePoweredOff, $phy.configure(config) $($await)*);
        $points.ok(P::PowerOn, $phy.power_on() $($await)*);
        $points.running(P::PowerOn, $check.probe.line(&*$phy), false);
        $points.refused(P::StartAfterPowerOn, $phy.start_output() $($await)*);
    }};
}

macro_rules! frl {
    ($check:ident, $points:ident, $phy:ident, $($await:tt)*) => {{
        use PhyPoint as P;
        if let Some(rate) = $check.frl_rate {
            let config = $check.config;
            if $points.ok(P::EnterFrl, $phy.set_frl_rate(rate) $($await)*) {
                $points.frl(P::EnterFrl, $check.probe.line(&*$phy), rate);
                $points.refused(P::StartInFrl, $phy.start_output() $($await)*);
                $points.ok(P::ConfigureLeavesFrl, $phy.configure(config) $($await)*);
                $points.running(P::ConfigureLeavesFrl, $check.probe.line(&*$phy), false);
            }
            if $points.ok(P::EnterFrl, $phy.set_frl_rate(rate) $($await)*) {
                $points.ok(P::StopLeavesFrl, $phy.stop_output() $($await)*);
                $points.running(P::StopLeavesFrl, $check.probe.line(&*$phy), false);
            }
            if $points.ok(P::EnterFrl, $phy.set_frl_rate(rate) $($await)*) {
                $points.ok(P::FrlOff, $phy.set_frl_rate(HdmiForumFrl::NotSupported) $($await)*);
                $points.running(P::FrlOff, $check.probe.line(&*$phy), false);
                $points.refused(P::StartInFrl, $phy.start_output() $($await)*);
            }
        }
    }};
}

impl<R: ?Sized> PhyCheck<'_, R> {
    /// The TMDS points, against a PHY fresh from creation (`start_output` is refused before
    /// its first `configure`).
    pub fn run_tmds<P>(&self, phy: &mut P) -> Report<PhyFailure>
    where
        P: hdmi_hal::phy::TmdsPhy + ?Sized,
        R: OutputProbe<P>,
    {
        let mut report = Report::new();
        let mut points = Points {
            report: &mut report,
        };
        let check = self;
        tmds!(check, points, phy,);
        report
    }

    /// A power cycle, the TMDS points and the power points, against any PHY.
    pub fn run_powered<P>(&self, phy: &mut P) -> Report<PhyFailure>
    where
        P: hdmi_hal::phy::TmdsPhy + hdmi_hal::phy::PhyPower + ?Sized,
        R: OutputProbe<P>,
    {
        let mut report = Report::new();
        let mut points = Points {
            report: &mut report,
        };
        let check = self;
        points.ok(PhyPoint::PowerOff, phy.power_off());
        points.ok(PhyPoint::PowerOn, phy.power_on());
        tmds!(check, points, phy,);
        power!(check, points, phy,);
        report
    }

    /// The powered points and the mode rule (with [`with_frl_rate`](Self::with_frl_rate)).
    pub fn run_frl<P>(&self, phy: &mut P) -> Report<PhyFailure>
    where
        P: hdmi_hal::phy::TmdsPhy + hdmi_hal::phy::PhyPower + hdmi_hal::phy::FrlPhy + ?Sized,
        R: OutputProbe<P>,
    {
        let mut report = self.run_powered(phy);
        let mut points = Points {
            report: &mut report,
        };
        let check = self;
        frl!(check, points, phy,);
        report
    }

    /// [`run_tmds`](Self::run_tmds) over hdmi-hal-async's `TmdsPhy`.
    #[cfg(feature = "async")]
    pub async fn run_tmds_async<P>(&self, phy: &mut P) -> Report<PhyFailure>
    where
        P: hdmi_hal_async::phy::TmdsPhy + ?Sized,
        R: OutputProbe<P>,
    {
        let mut report = Report::new();
        let mut points = Points {
            report: &mut report,
        };
        let check = self;
        tmds!(check, points, phy, .await);
        report
    }

    /// [`run_powered`](Self::run_powered) over hdmi-hal-async's traits.
    #[cfg(feature = "async")]
    pub async fn run_powered_async<P>(&self, phy: &mut P) -> Report<PhyFailure>
    where
        P: hdmi_hal_async::phy::TmdsPhy + hdmi_hal_async::phy::PhyPower + ?Sized,
        R: OutputProbe<P>,
    {
        let mut report = Report::new();
        let mut points = Points {
            report: &mut report,
        };
        let check = self;
        points.ok(PhyPoint::PowerOff, phy.power_off().await);
        points.ok(PhyPoint::PowerOn, phy.power_on().await);
        tmds!(check, points, phy, .await);
        power!(check, points, phy, .await);
        report
    }

    /// [`run_frl`](Self::run_frl) over hdmi-hal-async's traits.
    #[cfg(feature = "async")]
    pub async fn run_frl_async<P>(&self, phy: &mut P) -> Report<PhyFailure>
    where
        P: hdmi_hal_async::phy::TmdsPhy
            + hdmi_hal_async::phy::PhyPower
            + hdmi_hal_async::phy::FrlPhy
            + ?Sized,
        R: OutputProbe<P>,
    {
        let mut report = self.run_powered_async(phy).await;
        let mut points = Points {
            report: &mut report,
        };
        let check = self;
        frl!(check, points, phy, .await);
        report
    }
}
