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
//! A check's settings are a struct with a `DEFAULT` and `with_*` builders, with `run` (and
//! `run_async` with the `async` feature) on it; the function named after the check runs it
//! with the defaults.
//!
//! New check points come only in a breaking release, so an implementor's CI does not turn red
//! on `cargo update`. A panic inside the implementation under test is not reported: it ends
//! the check, as it ends the test.

mod clock;
mod report;

pub use clock::{ClockCheck, ClockFailure, clock};
pub use report::Report;
