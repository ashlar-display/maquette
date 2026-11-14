//! A simulated HDMI sink for tests.
//!
//! `maquette` simulates what an HDMI source can observe of a sink: its SCDC registers, its
//! EDID and DisplayID behind the DDC bus, the source's PHY and the line between them, and a
//! clock, all under deterministic simulated time. Code written over hdmi-hal's traits runs
//! against it as against a sink, with the sink's delays, refusals and faults, and a test
//! asserts on one ordered transcript of everything that happened. It also carries
//! conformance checks that an implementation of hdmi-hal's traits runs in its own tests.
//!
//! Built so far: [`SimClock`], hdmi-hal's `Clock` and `Delay` on simulated time, and the
//! conformance check for `Clock` and `Delay` implementations ([`check::clock`]). The rest of
//! the design is in `doc/architecture.md`.
//!
//! # Features
//!
//! - **`std`** — the critical-section implementation for hosts, and the checks that need
//!   threads. Without it, the application supplies its own critical section, as embedded
//!   code does.
//! - **`async`** — hdmi-hal-async's traits, and their `Send` twins, on the same handles.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod check;
mod clock;

// Compiles and runs the README's examples as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

pub use clock::SimClock;

/// The hdmi-hal time types `SimClock` implements and returns, re-exported so a test names
/// the same versions maquette does without depending on hdmi-hal itself.
pub use hdmi_hal::time::{Clock, Delay, Timestamp};
