//! Conformance checks for implementations of hdmi-hal's traits.
//!
//! Each check takes the implementation under test as the trait values a procedure would get,
//! runs every point, and returns a [`Report`] of the points that failed, typed by the check's
//! own failure enum. [`Report::assert_ok`] is the one assertion a test needs:
//!
//! ```
//! use maquette::{SimClock, check};
//!
//! let clock = SimClock::new();
//! let report = check::clock(&clock, &mut &clock);
//! report.assert_ok();
//! assert_eq!(report.failed(), 0);
//! ```
//!
//! A check is a struct with `run` (and `run_async` with the `async` feature) on it, built
//! from what it needs: the clock check from settings with a `DEFAULT` (and [`clock`], which
//! runs it with them), the others from a description of what the implementation is in front
//! of (a device, a register image, a configuration and a probe) with `new`, and `with_*` for
//! the rest. The PHY check runs by the traits the PHY implements (`run_tmds`, `run_powered`,
//! `run_frl`).
//!
//! New check points come only in a breaking release, so an implementor's CI does not turn red
//! on `cargo update`. A panic inside the implementation under test is not reported: it ends
//! the check, as it ends the test; only `DdcBusCheck::run_threads` (with `std`) counts a
//! thread's panic as a failure.

mod clock;
mod ddc;
mod phy;
mod report;
mod scdc;

#[cfg(feature = "std")]
pub use clock::HostClock;
pub use clock::{ClockCheck, ClockFailure, clock};
pub use ddc::{DdcBusCheck, DdcBusFailure, DdcDevice};
pub use phy::{NoProbe, OutputProbe, PhyCheck, PhyFailure, PhyPoint};
pub use report::Report;
pub use scdc::{FailNext, ScdcImage, ScdcOp, ScdcTransportCheck, ScdcTransportFailure};
