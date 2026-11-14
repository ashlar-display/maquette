//! A simulated HDMI sink for tests.
//!
//! `maquette` simulates what an HDMI source can observe of a sink: its SCDC registers, its
//! EDID and DisplayID behind the DDC bus, the source's PHY and the line between them, and a
//! clock, all under deterministic simulated time. Code written over hdmi-hal's traits runs
//! against it as against a sink, with the sink's delays, refusals and faults, and a test
//! asserts on one ordered transcript of everything that happened. It also carries
//! conformance checks that an implementation of hdmi-hal's traits runs in its own tests.
//!
//! Built so far: [`SimClock`], hdmi-hal's `Clock` and `Delay` on simulated time; the
//! conformance check for `Clock` and `Delay` implementations ([`check::clock`]); and the
//! [`Bench`]. Its DDC bus ([`Ddc`]) routes each call to a test's own [`Target`]s or the
//! sink's SCDC registers, EDID and DisplayID, and records it in the transcript. The line,
//! HPD, unplug and detach are bench events; the sink keeps a scrambling lock and resets its
//! registers after a long HPD low; register presets and a view of the registers ([`Sink`])
//! serve the test; the simulated [`Phy`] drives the line; scheduled [`Fault`]s fail
//! operations; an operation budget ends a test
//! that loops forever. The rest of the design is in `doc/architecture.md`.
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

mod bench;
pub mod check;
mod clock;
mod config;
mod ddc;
mod eddc;
mod fault;
mod line;
mod phy;
mod scdc;
mod target;
mod transcript;

// Compiles and runs the README's examples as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

pub use bench::{
    Bench, BenchBuilder, BenchError, DEFAULT_BUDGET, DEFAULT_PHY_CAPABILITIES, Sink, Time,
};
pub use clock::SimClock;
pub use config::{
    BlockBoundary, DuringHpdLow, HpdReset, MissingSegment, OwnedWrite, PastStore, Refusal,
    RefusalKind, RegisterGroup, ScdcPastEnd, SinkConfig, Trigger,
};
pub use ddc::{BusError, BusTiming, Ddc};
pub use fault::{Effect, Fault, FaultId, Filter, OpKind, Point, ScheduleFull, Times};
pub use line::Line;
pub use phy::{
    ConfigureWhileRunning, Phy, PhyCall, PhyCallKind, PhyError, PhyErrorKind, PhyFault,
    PhySettings, ScramblerAfterConfigure,
};
pub use target::{Env, Operation, Target};
pub use transcript::{Actor, BusOp, BusRecord, Event, Ignored, Record};

/// hdmi-hal, whose types maquette's API is made of: a test names the same version through
/// this re-export without depending on hdmi-hal itself.
pub use hdmi_hal;
pub use hdmi_hal::ddc::{
    DdcBus, DdcCapabilities, DdcError, DdcErrorKind, EddcTarget, NoAcknowledgeSource,
};
/// The hdmi-hal DDC types the bench's bus implements and uses.
/// The hdmi-hal types the line and the PHY are made of.
pub use hdmi_hal::phy::{
    EqParams, FrlOutput, FrlPhy, LanePatterns, PhyPower, TmdsClockRatio, TmdsPhy,
    TmdsPhyCapabilities, TmdsPhyConfig, TmdsVideo,
};
pub use hdmi_hal::{ColorBitDepth, ColorFormat, HdmiForumFrl};

/// The hdmi-hal time types `SimClock` implements and returns, re-exported so a test names
/// the same versions maquette does without depending on hdmi-hal itself.
pub use hdmi_hal::time::{Clock, Delay, Timestamp};
