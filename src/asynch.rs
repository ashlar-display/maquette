//! The async traits on the same handles: hdmi-hal-async's `Send` twins, which give the plain
//! async traits through hdmi-hal-async's blanket implementations.
//!
//! The bench is synchronous: each async method runs its sync counterpart when its future is
//! first polled, and the future is then ready. So an async procedure runs against the bench
//! with any executor, a single `block_on` included, and its futures are `Send`: it can be
//! spawned on a multi-threaded executor.

use core::time::Duration;

use hdmi_hal::HdmiForumFrl;
use hdmi_hal::ddc::{DdcCapabilities, EddcTarget};
use hdmi_hal::phy::{EqParams, FrlOutput, LanePatterns, TmdsPhyCapabilities, TmdsPhyConfig};
use hdmi_hal_async::ddc::{SendDdcBus, SendDdcHandle};
use hdmi_hal_async::phy::{SendFrlPhy, SendPhyPower, SendTmdsPhy};
use hdmi_hal_async::time::SendDelay;

use crate::bench::Time;
use crate::clock::SimClock;
use crate::ddc::{BusError, Ddc};
use crate::phy::{Phy, PhyError};

/// The bench's bus is its own async handle too, passed by value.
impl hdmi_hal_async::ddc::DdcHandle for Ddc<'_> {
    type Bus = Self;

    fn bus(&self) -> &Self {
        self
    }
}

impl SendDdcHandle for Ddc<'_> {
    type Bus = Self;

    fn bus(&self) -> &Self {
        self
    }
}

impl SendDdcBus for Ddc<'_> {
    type Error = BusError;

    fn capabilities(&self) -> DdcCapabilities {
        hdmi_hal::ddc::DdcBus::capabilities(self)
    }

    async fn write(&self, addr: u8, bytes: &[u8]) -> Result<(), BusError> {
        hdmi_hal::ddc::DdcBus::write(self, addr, bytes)
    }

    async fn write_read(&self, addr: u8, bytes: &[u8], buf: &mut [u8]) -> Result<(), BusError> {
        hdmi_hal::ddc::DdcBus::write_read(self, addr, bytes, buf)
    }

    async fn read(&self, addr: u8, buf: &mut [u8]) -> Result<(), BusError> {
        hdmi_hal::ddc::DdcBus::read(self, addr, buf)
    }

    async fn read_segmented(
        &self,
        target: EddcTarget,
        segment: u8,
        offset: u8,
        buf: &mut [u8],
    ) -> Result<(), BusError> {
        hdmi_hal::ddc::DdcBus::read_segmented(self, target, segment, offset, buf)
    }
}

impl SendDelay for Time<'_> {
    async fn delay(&mut self, duration: Duration) {
        hdmi_hal::time::Delay::delay(self, duration)
    }
}

/// A simulated clock's delay, for an async procedure that needs only time: no bench.
impl SendDelay for SimClock {
    async fn delay(&mut self, duration: Duration) {
        hdmi_hal::time::Delay::delay(self, duration)
    }
}

/// As for `SimClock`, through a shared reference, as the sync `Delay` is.
impl SendDelay for &SimClock {
    async fn delay(&mut self, duration: Duration) {
        hdmi_hal::time::Delay::delay(self, duration)
    }
}

impl SendTmdsPhy for Phy<'_> {
    type Error = PhyError;

    fn capabilities(&self) -> TmdsPhyCapabilities {
        hdmi_hal::phy::TmdsPhy::capabilities(self)
    }

    async fn configure(&mut self, config: TmdsPhyConfig) -> Result<(), PhyError> {
        hdmi_hal::phy::TmdsPhy::configure(self, config)
    }

    async fn set_scrambling(&mut self, enabled: bool) -> Result<(), PhyError> {
        hdmi_hal::phy::TmdsPhy::set_scrambling(self, enabled)
    }

    async fn start_output(&mut self) -> Result<(), PhyError> {
        hdmi_hal::phy::TmdsPhy::start_output(self)
    }

    async fn stop_output(&mut self) -> Result<(), PhyError> {
        hdmi_hal::phy::TmdsPhy::stop_output(self)
    }
}

impl SendPhyPower for Phy<'_> {
    type Error = PhyError;

    async fn power_on(&mut self) -> Result<(), PhyError> {
        hdmi_hal::phy::PhyPower::power_on(self)
    }

    async fn power_off(&mut self) -> Result<(), PhyError> {
        hdmi_hal::phy::PhyPower::power_off(self)
    }
}

impl SendFrlPhy for Phy<'_> {
    type Error = PhyError;

    async fn set_frl_rate(&mut self, rate: HdmiForumFrl) -> Result<(), PhyError> {
        hdmi_hal::phy::FrlPhy::set_frl_rate(self, rate)
    }

    async fn send_ltp(&mut self, patterns: LanePatterns) -> Result<(), PhyError> {
        hdmi_hal::phy::FrlPhy::send_ltp(self, patterns)
    }

    async fn set_frl_output(&mut self, output: FrlOutput) -> Result<(), PhyError> {
        hdmi_hal::phy::FrlPhy::set_frl_output(self, output)
    }

    async fn adjust_equalization(&mut self, params: EqParams) -> Result<(), PhyError> {
        hdmi_hal::phy::FrlPhy::adjust_equalization(self, params)
    }
}
