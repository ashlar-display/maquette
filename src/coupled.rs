//! `Coupled<P>`: any PHY driving the bench's line.

use core::fmt;

use hdmi_hal::HdmiForumFrl;
use hdmi_hal::phy::{
    EqParams, FrlOutput, FrlPhy, LanePatterns, PhyPower, TmdsPhy, TmdsPhyCapabilities,
    TmdsPhyConfig,
};

use crate::bench::{Bench, Core};
use crate::line::Line;
use crate::phy::{FrlMode, Mode, PhyCall, PhyErrorKind, apply_frl, line_of};

/// Any PHY driving the bench's line: a third party's, a forwarding stub, a firmware PHY under
/// hardware-in-the-loop.
///
/// It implements `TmdsPhy` with `P`'s error, and `PhyPower` and `FrlPhy` where `P` does.
/// Each call goes to `P` unchanged and returns what `P` returned; what succeeded drives the
/// bench's line, so the sink answers what `P` transmits. Every call is recorded as an
/// `Event::Phy` (a failure as `PhyErrorKind::Coupled`, `P`'s own error being returned to the
/// caller), is a bench event, and counts against the budget, as the simulated PHY's calls
/// are. Scheduled PHY faults are the simulated PHY's; `P` has its own.
///
/// The line follows the contract's guarantees only: `configure` enters TMDS with the output
/// stopped, `start_output` starts it, `stop_output` stops it and leaves FRL, a FRL rate
/// enters FRL, `power_off` leaves every mode. What `P` does to its scrambler in `configure`
/// is its own and the bench cannot see it, so the line is unscrambled after `configure` until
/// a `set_scrambling` says otherwise (*chosen*). A failed call changes nothing.
///
/// ```
/// use maquette::{Bench, Coupled, SinkConfig, TmdsPhy, TmdsPhyCapabilities, TmdsPhyConfig};
///
/// /// A PHY that does whatever it is asked.
/// struct Stub;
///
/// impl TmdsPhy for Stub {
///     type Error = ();
///     fn capabilities(&self) -> TmdsPhyCapabilities {
///         TmdsPhyCapabilities::new(600_000).with_scrambling(true)
///     }
///     fn configure(&mut self, _config: TmdsPhyConfig) -> Result<(), ()> { Ok(()) }
///     fn set_scrambling(&mut self, _enabled: bool) -> Result<(), ()> { Ok(()) }
///     fn start_output(&mut self) -> Result<(), ()> { Ok(()) }
///     fn stop_output(&mut self) -> Result<(), ()> { Ok(()) }
/// }
///
/// let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
/// let mut phy = Coupled::new(Stub, &bench);
/// phy.start_output().unwrap(); // nothing configured: the line stays off
/// assert_eq!(bench.line(), maquette::Line::Off);
/// ```
pub struct Coupled<'b, P> {
    phy: P,
    core: &'b (dyn Core + Sync + 'b),
    powered: bool,
    mode: Mode,
    running: bool,
    scrambler: bool,
    /// What this PHY last put on the line.
    line: Line,
}

impl<'b, P> Coupled<'b, P> {
    /// Couples `phy` to `bench`'s line. The PHY is taken to be powered and transmitting
    /// nothing.
    pub fn new<'a: 'b, const LOG: usize, const FAULTS: usize>(
        phy: P,
        bench: &'b Bench<'a, LOG, FAULTS>,
    ) -> Self {
        Self {
            phy,
            core: bench,
            powered: true,
            mode: Mode::None,
            running: false,
            scrambler: false,
            line: Line::Off,
        }
    }

    /// The coupled PHY.
    pub fn get_ref(&self) -> &P {
        &self.phy
    }

    /// The coupled PHY, mutably: calls made through it bypass the coupling.
    pub fn get_mut(&mut self) -> &mut P {
        &mut self.phy
    }

    /// The coupled PHY, uncoupled.
    pub fn into_inner(self) -> P {
        self.phy
    }

    /// Forwards one call: counts it, runs it on `P`, applies what the contract guarantees
    /// after a success, and records it with the line change it made.
    fn forward<E>(
        &mut self,
        call: PhyCall,
        run: impl FnOnce(&mut P) -> Result<(), E>,
    ) -> Result<(), E> {
        self.core.spend();
        let result = run(&mut self.phy);
        if result.is_ok() {
            self.apply(call);
        }
        let line = line_of(self.mode, self.running, self.scrambler);
        let changed = (line != self.line).then_some(line);
        self.line = line;
        let recorded = result
            .as_ref()
            .map(|_| ())
            .map_err(|_| PhyErrorKind::Coupled);
        self.core.coupled_call(call, recorded, changed);
        result
    }

    /// What a successful `call` leaves, by the contract.
    fn apply(&mut self, call: PhyCall) {
        match call {
            PhyCall::Configure(config) => {
                self.mode = Mode::Tmds(config);
                self.running = false;
                self.scrambler = false;
            }
            PhyCall::SetScrambling(enabled) => self.scrambler = enabled,
            PhyCall::StartOutput => self.running = matches!(self.mode, Mode::Tmds(_)),
            PhyCall::StopOutput => {
                self.running = false;
                if matches!(self.mode, Mode::Frl(_)) {
                    self.mode = Mode::None;
                }
            }
            PhyCall::PowerOn => {
                if !self.powered {
                    self.powered = true;
                    self.mode = Mode::None;
                    self.running = false;
                }
            }
            PhyCall::PowerOff => {
                self.powered = false;
                self.mode = Mode::None;
                self.running = false;
            }
            PhyCall::SetFrlRate(HdmiForumFrl::NotSupported) => {
                if matches!(self.mode, Mode::Frl(_)) {
                    self.mode = Mode::None;
                }
            }
            PhyCall::SetFrlRate(rate) => {
                self.mode = Mode::Frl(FrlMode::new(rate));
                self.running = false;
            }
            PhyCall::SendLtp(_) | PhyCall::SetFrlOutput(_) | PhyCall::AdjustEqualization(_) => {
                apply_frl(&mut self.mode, call);
            }
        }
    }
}

impl<P: fmt::Debug> fmt::Debug for Coupled<'_, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Coupled")
            .field("phy", &self.phy)
            .field("line", &self.line)
            .finish_non_exhaustive()
    }
}

impl<P: TmdsPhy> TmdsPhy for Coupled<'_, P> {
    type Error = P::Error;

    fn capabilities(&self) -> TmdsPhyCapabilities {
        self.phy.capabilities()
    }

    fn configure(&mut self, config: TmdsPhyConfig) -> Result<(), P::Error> {
        self.forward(PhyCall::Configure(config), |phy| phy.configure(config))
    }

    fn set_scrambling(&mut self, enabled: bool) -> Result<(), P::Error> {
        self.forward(PhyCall::SetScrambling(enabled), |phy| {
            phy.set_scrambling(enabled)
        })
    }

    fn start_output(&mut self) -> Result<(), P::Error> {
        self.forward(PhyCall::StartOutput, P::start_output)
    }

    fn stop_output(&mut self) -> Result<(), P::Error> {
        self.forward(PhyCall::StopOutput, P::stop_output)
    }
}

impl<P: PhyPower> PhyPower for Coupled<'_, P> {
    type Error = P::Error;

    fn power_on(&mut self) -> Result<(), P::Error> {
        self.forward(PhyCall::PowerOn, P::power_on)
    }

    fn power_off(&mut self) -> Result<(), P::Error> {
        self.forward(PhyCall::PowerOff, P::power_off)
    }
}

impl<P: FrlPhy> FrlPhy for Coupled<'_, P> {
    type Error = P::Error;

    fn set_frl_rate(&mut self, rate: HdmiForumFrl) -> Result<(), P::Error> {
        self.forward(PhyCall::SetFrlRate(rate), |phy| phy.set_frl_rate(rate))
    }

    fn send_ltp(&mut self, patterns: LanePatterns) -> Result<(), P::Error> {
        self.forward(PhyCall::SendLtp(patterns), |phy| phy.send_ltp(patterns))
    }

    fn set_frl_output(&mut self, output: FrlOutput) -> Result<(), P::Error> {
        self.forward(PhyCall::SetFrlOutput(output), |phy| {
            phy.set_frl_output(output)
        })
    }

    fn adjust_equalization(&mut self, params: EqParams) -> Result<(), P::Error> {
        self.forward(PhyCall::AdjustEqualization(params), |phy| {
            phy.adjust_equalization(params)
        })
    }
}
