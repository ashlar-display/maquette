//! The simulated PHY: hdmi-hal's `TmdsPhy`, `FrlPhy` and `PhyPower` on the bench, driving the
//! line.

use core::fmt;
use core::time::Duration;

use hdmi_hal::HdmiForumFrl;
use hdmi_hal::phy::{
    EqParams, FrlOutput, FrlPhy, LanePatterns, PhyPower, TmdsPhy, TmdsPhyCapabilities,
    TmdsPhyConfig,
};

use crate::bench::Core;
use crate::fault::Times;
use crate::line::{Line, RATIO_40_ABOVE_KHZ as LOW_RATE_KHZ};

/// How the simulated PHY behaves where hdmi-hal's contract allows several behaviours. Each
/// defaults to the least forgiving one: a procedure must be correct under any behaviour the
/// contract allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct PhySettings {
    /// Whether the PHY is powered on at creation; on by default (*chosen*: `PhyPower`'s
    /// documentation says procedures never change the power, so a procedure test expects a
    /// powered PHY). Creation in the on state counts as a power-on.
    pub powered: bool,
    /// What `configure` does with the output running.
    pub configure_while_running: ConfigureWhileRunning,
    /// The scrambler after `configure`.
    pub scrambler_after_configure: ScramblerAfterConfigure,
    /// How long `start_output` takes before it returns, on the bench's clock.
    pub start_latency: Duration,
}

impl PhySettings {
    /// Powered on; `configure` fails with the output running; the scrambler the opposite of
    /// what the rate needs after `configure`; `start_output` takes 1 ms (*chosen*).
    pub const DEFAULT: Self = Self {
        powered: true,
        configure_while_running: ConfigureWhileRunning::Fail,
        scrambler_after_configure: ScramblerAfterConfigure::Opposite,
        start_latency: Duration::from_millis(1),
    };

    /// The settings with the PHY powered on at creation or not.
    #[must_use]
    pub const fn with_powered(self, powered: bool) -> Self {
        Self { powered, ..self }
    }

    /// The settings with `configure` doing `behaviour` with the output running.
    #[must_use]
    pub const fn with_configure_while_running(self, behaviour: ConfigureWhileRunning) -> Self {
        Self {
            configure_while_running: behaviour,
            ..self
        }
    }

    /// The settings with the scrambler after `configure` as `behaviour` says.
    #[must_use]
    pub const fn with_scrambler_after_configure(self, behaviour: ScramblerAfterConfigure) -> Self {
        Self {
            scrambler_after_configure: behaviour,
            ..self
        }
    }

    /// The settings with `start_output` taking `latency`.
    #[must_use]
    pub const fn with_start_latency(self, latency: Duration) -> Self {
        Self {
            start_latency: latency,
            ..self
        }
    }
}

impl Default for PhySettings {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// What `configure` does with the output running: the contract allows either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigureWhileRunning {
    /// Fails with `PhyErrorKind::Running` (the default).
    Fail,
    /// Stops the output, then configures.
    StopFirst,
}

/// The scrambler after `configure`: the contract leaves it unspecified. Each rule stays within
/// the PHY's capabilities: the scrambler is never on without one, nor on at or below
/// 340 000 kHz where it cannot scramble there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScramblerAfterConfigure {
    /// The opposite of what the rate needs: on at or below 340 000 kHz, off above (the
    /// default), so a procedure that never calls `set_scrambling` fails at every rate.
    Opposite,
    /// As it was.
    Keep,
    /// Off.
    Off,
    /// What the rate needs: on above 340 000 kHz, off at or below.
    FromRate,
}

/// A call of the simulated or a coupled PHY, with its arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PhyCall {
    /// `TmdsPhy::configure`.
    Configure(TmdsPhyConfig),
    /// `TmdsPhy::set_scrambling`.
    SetScrambling(bool),
    /// `TmdsPhy::start_output`.
    StartOutput,
    /// `TmdsPhy::stop_output`.
    StopOutput,
    /// `PhyPower::power_on`.
    PowerOn,
    /// `PhyPower::power_off`.
    PowerOff,
    /// `FrlPhy::set_frl_rate`.
    SetFrlRate(HdmiForumFrl),
    /// `FrlPhy::send_ltp`.
    SendLtp(LanePatterns),
    /// `FrlPhy::set_frl_output`.
    SetFrlOutput(FrlOutput),
    /// `FrlPhy::adjust_equalization`.
    AdjustEqualization(EqParams),
}

impl PhyCall {
    /// The call's kind, as a [`PhyFault`] names it.
    pub fn kind(&self) -> PhyCallKind {
        match self {
            Self::Configure(_) => PhyCallKind::Configure,
            Self::SetScrambling(_) => PhyCallKind::SetScrambling,
            Self::StartOutput => PhyCallKind::StartOutput,
            Self::StopOutput => PhyCallKind::StopOutput,
            Self::PowerOn => PhyCallKind::PowerOn,
            Self::PowerOff => PhyCallKind::PowerOff,
            Self::SetFrlRate(_) => PhyCallKind::SetFrlRate,
            Self::SendLtp(_) => PhyCallKind::SendLtp,
            Self::SetFrlOutput(_) => PhyCallKind::SetFrlOutput,
            Self::AdjustEqualization(_) => PhyCallKind::AdjustEqualization,
        }
    }
}

/// The kind of a PHY call, without its arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PhyCallKind {
    /// `TmdsPhy::configure`.
    Configure,
    /// `TmdsPhy::set_scrambling`.
    SetScrambling,
    /// `TmdsPhy::start_output`.
    StartOutput,
    /// `TmdsPhy::stop_output`.
    StopOutput,
    /// `PhyPower::power_on`.
    PowerOn,
    /// `PhyPower::power_off`.
    PowerOff,
    /// `FrlPhy::set_frl_rate`.
    SetFrlRate,
    /// `FrlPhy::send_ltp`.
    SendLtp,
    /// `FrlPhy::set_frl_output`.
    SetFrlOutput,
    /// `FrlPhy::adjust_equalization`.
    AdjustEqualization,
}

/// Why a call of the simulated PHY failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PhyErrorKind {
    /// The PHY is powered off.
    PoweredOff,
    /// `start_output` without a `configure` since the last power-on.
    NotConfigured,
    /// `configure` with the output running, under [`ConfigureWhileRunning::Fail`].
    Running,
    /// Beyond the PHY's capabilities: the rate, the scrambler, low-rate scrambling.
    Unsupported,
    /// An FRL call outside FRL.
    NotInFrl,
    /// A scheduled PHY fault.
    Fault,
    /// A coupled PHY's own error, which [`Coupled`](crate::Coupled) returns unchanged; the
    /// transcript records only that it failed.
    Coupled,
}

/// The method's name.
impl fmt::Display for PhyCallKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Configure => "configure",
            Self::SetScrambling => "set_scrambling",
            Self::StartOutput => "start_output",
            Self::StopOutput => "stop_output",
            Self::PowerOn => "power_on",
            Self::PowerOff => "power_off",
            Self::SetFrlRate => "set_frl_rate",
            Self::SendLtp => "send_ltp",
            Self::SetFrlOutput => "set_frl_output",
            Self::AdjustEqualization => "adjust_equalization",
        })
    }
}

/// Why, in words.
impl fmt::Display for PhyErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::PoweredOff => "the PHY is powered off",
            Self::NotConfigured => "not configured since the last power-on",
            Self::Running => "the output is running",
            Self::Unsupported => "beyond the PHY's capabilities",
            Self::NotInFrl => "not in FRL",
            Self::Fault => "a scheduled fault",
            Self::Coupled => "the coupled PHY's own error",
        })
    }
}

/// A failed call of the simulated PHY.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct PhyError {
    /// Why it failed.
    pub kind: PhyErrorKind,
    /// The call.
    pub call: PhyCall,
    /// Whether the call took effect before it failed: only a fault that strikes after it.
    pub took_effect: bool,
}

impl fmt::Display for PhyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} failed: {}", self.call.kind(), self.kind)?;
        if self.took_effect {
            f.write_str(" (after it took effect)")?;
        }
        Ok(())
    }
}

impl core::error::Error for PhyError {}

/// A fault of the simulated PHY: it fails the `nth` call matching its kind, for `times`
/// calls, with [`PhyErrorKind::Fault`]. Scheduled with
/// [`Bench::schedule_phy`](crate::Bench::schedule_phy), in the same schedule as the bus's
/// faults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct PhyFault {
    /// Which calls it counts; `None` for every call.
    pub call: Option<PhyCallKind>,
    /// The first matching call it fails, counted from 1 from when it is scheduled (0 is
    /// taken as 1).
    pub nth: u32,
    /// How many calls it fails, from the `nth`.
    pub times: Times,
    /// Whether the call takes effect before it fails (true) or not at all (false).
    pub took_effect: bool,
}

impl PhyFault {
    /// A fault that fails the first call, once, before it takes effect.
    pub const fn new() -> Self {
        Self {
            call: None,
            nth: 1,
            times: Times::Count(1),
            took_effect: false,
        }
    }

    /// The fault counting only calls of `kind`.
    #[must_use]
    pub const fn on(self, kind: PhyCallKind) -> Self {
        Self {
            call: Some(kind),
            ..self
        }
    }

    /// The fault failing the `n`-th matching call first.
    #[must_use]
    pub const fn nth(self, n: u32) -> Self {
        Self { nth: n, ..self }
    }

    /// The fault failing `times` calls from the `nth`.
    #[must_use]
    pub const fn times(self, times: Times) -> Self {
        Self { times, ..self }
    }

    /// The fault failing the call after it took effect.
    #[must_use]
    pub const fn after_effect(self) -> Self {
        Self {
            took_effect: true,
            ..self
        }
    }

    pub(crate) fn matches(&self, call: &PhyCall) -> bool {
        self.call.is_none_or(|kind| kind == call.kind())
    }
}

impl Default for PhyFault {
    fn default() -> Self {
        Self::new()
    }
}

/// The mode a PHY is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    None,
    Tmds(TmdsPhyConfig),
    Frl(FrlMode),
}

/// What a PHY in FRL sends: its rate, the training patterns and the output without one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrlMode {
    rate: HdmiForumFrl,
    patterns: LanePatterns,
    output: FrlOutput,
    equalization: Option<EqParams>,
}

impl FrlMode {
    /// A PHY that has just entered `rate`: no patterns, gap characters only.
    pub(crate) fn new(rate: HdmiForumFrl) -> Self {
        Self {
            rate,
            patterns: LanePatterns::default(),
            output: FrlOutput::GapOnly,
            equalization: None,
        }
    }
}

/// What a successful `send_ltp`, `set_frl_output` or `adjust_equalization` leaves in FRL;
/// other calls, and a PHY not in FRL, are left as they are.
pub(crate) fn apply_frl(mode: &mut Mode, call: PhyCall) {
    if let Mode::Frl(frl) = mode {
        match call {
            PhyCall::SendLtp(patterns) => frl.patterns = patterns,
            PhyCall::SetFrlOutput(output) => frl.output = output,
            PhyCall::AdjustEqualization(equalization) => frl.equalization = Some(equalization),
            _ => {}
        }
    }
}

/// What a PHY in `mode`, with its TMDS output `running` or not and its scrambler as given,
/// puts on the line: running TMDS from the configuration and the scrambler, FRL from its
/// rate, nothing otherwise.
pub(crate) fn line_of(mode: Mode, running: bool, scrambler: bool) -> Line {
    match mode {
        Mode::Tmds(config) if running => Line::Tmds {
            character_rate_khz: config.character_rate_khz,
            ratio: config.clock_ratio,
            scrambled: scrambler,
        },
        Mode::Frl(frl) => {
            let line = Line::frl_with(frl.rate, frl.patterns, frl.output);
            match frl.equalization {
                Some(equalization) => line.with_equalization(equalization),
                None => line,
            }
        }
        _ => Line::Off,
    }
}

/// The simulated PHY's state, held by the bench.
#[derive(Debug)]
pub(crate) struct PhyState {
    settings: PhySettings,
    capabilities: TmdsPhyCapabilities,
    powered: bool,
    mode: Mode,
    running: bool,
    scrambler: bool,
}

impl PhyState {
    pub(crate) fn new(settings: PhySettings, capabilities: TmdsPhyCapabilities) -> Self {
        Self {
            settings,
            capabilities,
            powered: settings.powered,
            mode: Mode::None,
            running: false,
            scrambler: false,
        }
    }

    pub(crate) fn capabilities(&self) -> TmdsPhyCapabilities {
        self.capabilities
    }

    pub(crate) fn start_latency(&self) -> Duration {
        self.settings.start_latency
    }

    /// What the PHY puts on the line.
    pub(crate) fn line(&self) -> Line {
        line_of(self.mode, self.running, self.scrambler)
    }

    /// Whether the TMDS output is running.
    pub(crate) fn is_running(&self) -> bool {
        self.running
    }

    fn leave_modes(&mut self) {
        self.mode = Mode::None;
        self.running = false;
    }

    /// Applies `call`.
    pub(crate) fn apply(&mut self, call: PhyCall) -> Result<(), PhyErrorKind> {
        match call {
            PhyCall::Configure(config) => {
                if !self.powered {
                    return Err(PhyErrorKind::PoweredOff);
                }
                if config.character_rate_khz > self.capabilities.max_character_rate_khz {
                    return Err(PhyErrorKind::Unsupported);
                }
                if self.running
                    && self.settings.configure_while_running == ConfigureWhileRunning::Fail
                {
                    return Err(PhyErrorKind::Running);
                }
                let high_rate = config.character_rate_khz > LOW_RATE_KHZ;
                let can = self.capabilities.scrambling
                    && (high_rate || self.capabilities.scrambles_at_low_rate());
                self.scrambler = can
                    && match self.settings.scrambler_after_configure {
                        ScramblerAfterConfigure::Opposite => !high_rate,
                        ScramblerAfterConfigure::Keep => self.scrambler,
                        ScramblerAfterConfigure::Off => false,
                        ScramblerAfterConfigure::FromRate => high_rate,
                    };
                self.mode = Mode::Tmds(config);
                self.running = false;
                Ok(())
            }
            PhyCall::SetScrambling(enabled) => {
                if enabled {
                    if !self.capabilities.scrambling {
                        return Err(PhyErrorKind::Unsupported);
                    }
                    let low_rate = matches!(
                        self.mode,
                        Mode::Tmds(config) if config.character_rate_khz <= LOW_RATE_KHZ
                    );
                    if low_rate && !self.capabilities.scrambles_at_low_rate() {
                        return Err(PhyErrorKind::Unsupported);
                    }
                }
                self.scrambler = enabled;
                Ok(())
            }
            PhyCall::StartOutput => {
                if !self.powered {
                    return Err(PhyErrorKind::PoweredOff);
                }
                if !matches!(self.mode, Mode::Tmds(_)) {
                    return Err(PhyErrorKind::NotConfigured);
                }
                self.running = true;
                Ok(())
            }
            PhyCall::StopOutput => {
                self.running = false;
                if matches!(self.mode, Mode::Frl(_)) {
                    self.mode = Mode::None;
                }
                Ok(())
            }
            PhyCall::PowerOn => {
                if !self.powered {
                    self.powered = true;
                    self.leave_modes();
                }
                Ok(())
            }
            PhyCall::PowerOff => {
                self.powered = false;
                self.leave_modes();
                Ok(())
            }
            PhyCall::SetFrlRate(rate) => {
                if !self.powered {
                    return Err(PhyErrorKind::PoweredOff);
                }
                if rate == HdmiForumFrl::NotSupported {
                    if matches!(self.mode, Mode::Frl(_)) {
                        self.mode = Mode::None;
                    }
                } else {
                    self.running = false;
                    self.mode = Mode::Frl(FrlMode::new(rate));
                }
                Ok(())
            }
            PhyCall::SendLtp(_) | PhyCall::SetFrlOutput(_) | PhyCall::AdjustEqualization(_) => {
                if !self.powered {
                    Err(PhyErrorKind::PoweredOff)
                } else if !matches!(self.mode, Mode::Frl(_)) {
                    Err(PhyErrorKind::NotInFrl)
                } else {
                    apply_frl(&mut self.mode, call);
                    Ok(())
                }
            }
        }
    }
}

/// The bench's simulated PHY, a `Copy` handle borrowed from the bench
/// ([`Bench::phy`](crate::Bench::phy)); two handles are one PHY.
///
/// It implements `TmdsPhy`, `FrlPhy` and `PhyPower` to hdmi-hal's contracts, the mode rule
/// included (entering one mode leaves the other), with the behaviours the contract leaves
/// open set by [`PhySettings`]. After every call it drives the line: running TMDS output from
/// the configuration and the scrambler, FRL from its rate, `Line::Off` otherwise. Every call
/// is a bench event, recorded as an `Event::Phy`, and counts against the budget.
#[derive(Clone, Copy)]
pub struct Phy<'b> {
    core: &'b (dyn Core + Sync + 'b),
}

impl<'b> Phy<'b> {
    pub(crate) fn new(core: &'b (dyn Core + Sync + 'b)) -> Self {
        Self { core }
    }

    fn call(&self, call: PhyCall) -> Result<(), PhyError> {
        self.core.spend();
        let (result, took_effect) = self.core.phy_call(call);
        result.map_err(|kind| PhyError {
            kind,
            call,
            took_effect,
        })
    }
}

impl fmt::Debug for Phy<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Phy")
            .field("capabilities", &self.core.phy_capabilities())
            .finish_non_exhaustive()
    }
}

impl TmdsPhy for Phy<'_> {
    type Error = PhyError;

    fn capabilities(&self) -> TmdsPhyCapabilities {
        self.core.phy_capabilities()
    }

    fn configure(&mut self, config: TmdsPhyConfig) -> Result<(), PhyError> {
        self.call(PhyCall::Configure(config))
    }

    fn set_scrambling(&mut self, enabled: bool) -> Result<(), PhyError> {
        self.call(PhyCall::SetScrambling(enabled))
    }

    fn start_output(&mut self) -> Result<(), PhyError> {
        self.call(PhyCall::StartOutput)
    }

    fn stop_output(&mut self) -> Result<(), PhyError> {
        self.call(PhyCall::StopOutput)
    }
}

impl PhyPower for Phy<'_> {
    type Error = PhyError;

    fn power_on(&mut self) -> Result<(), PhyError> {
        self.call(PhyCall::PowerOn)
    }

    fn power_off(&mut self) -> Result<(), PhyError> {
        self.call(PhyCall::PowerOff)
    }
}

impl FrlPhy for Phy<'_> {
    type Error = PhyError;

    fn set_frl_rate(&mut self, rate: HdmiForumFrl) -> Result<(), PhyError> {
        self.call(PhyCall::SetFrlRate(rate))
    }

    fn send_ltp(&mut self, patterns: LanePatterns) -> Result<(), PhyError> {
        self.call(PhyCall::SendLtp(patterns))
    }

    fn set_frl_output(&mut self, output: FrlOutput) -> Result<(), PhyError> {
        self.call(PhyCall::SetFrlOutput(output))
    }

    fn adjust_equalization(&mut self, params: EqParams) -> Result<(), PhyError> {
        self.call(PhyCall::AdjustEqualization(params))
    }
}
