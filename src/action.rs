//! Scheduled actions: bench events the bench makes by itself during a procedure, at a moment
//! the test chose.

use crate::config::SinkConfig;
use crate::fault::Filter;
use crate::line::Line;
use crate::phy::PhyCallKind;

/// Something the bench does by itself during a procedure, as the test scheduled it with
/// [`Bench::schedule_action`](crate::Bench::schedule_action): what a test could do between
/// two calls, done between two steps of the code under test.
///
/// Each is recorded as the events it makes, after an `Event::Action` naming it. An action
/// that cannot apply when it comes (a preset on a sink without SCDC, HPD raised on an
/// unplugged sink) does nothing but that record.
///
/// ```
/// use maquette::{Action, Bench, DdcBus, Filter, SinkConfig, When};
///
/// let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
/// // The sink sets FLT_update after the source's second write to Config_1.
/// bench
///     .schedule_action(
///         Action::Preset { reg: 0x10, value: 0x20 },
///         When::AfterOp {
///             filter: Filter::ANY.with_addr(0x54).with_first(0x31),
///             nth: 2,
///         },
///     )
///     .unwrap();
/// let bus = bench.ddc();
/// bus.write(0x54, &[0x31, 0x01]).unwrap();
/// assert_eq!(bench.sink().update_0(), Some(0));
/// bus.write(0x54, &[0x31, 0x03]).unwrap();
/// assert_eq!(bench.sink().update_0(), Some(0x20));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Action<'a> {
    /// Sets the SCDC register `reg` to `value`, as
    /// [`Bench::preset_scdc`](crate::Bench::preset_scdc).
    Preset {
        /// The register.
        reg: u8,
        /// Its value.
        value: u8,
    },
    /// Sets HPD, as [`Bench::set_hpd`](crate::Bench::set_hpd).
    Hpd(bool),
    /// Unplugs the sink, as [`Bench::unplug`](crate::Bench::unplug).
    Unplug,
    /// Plugs a sink in, as [`Bench::plug`](crate::Bench::plug).
    Plug {
        /// The EDID store: whole 128-byte blocks, up to 256.
        edid: &'a [u8],
        /// The DisplayID store, if any.
        displayid: Option<&'a [u8]>,
        /// The sink's settings.
        config: SinkConfig,
    },
    /// Sets what the source transmits, as [`Bench::set_line`](crate::Bench::set_line).
    Line(Line),
    /// Detaches the adapter, as [`Bench::detach`](crate::Bench::detach).
    Detach,
    /// Reattaches the adapter, as [`Bench::reattach`](crate::Bench::reattach).
    Reattach,
}

/// When a scheduled [`Action`] happens: once, then it leaves the schedule. Counts start when
/// it is scheduled, from 1 (an `nth` of 0 is taken as 1), and count what a fault would.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum When {
    /// Before the `nth` bus operation matching `filter`: the sink sees the action first.
    BeforeOp {
        /// Which operations it counts.
        filter: Filter,
        /// The operation it comes before.
        nth: u32,
    },
    /// After the `nth` bus operation matching `filter`: the sink has applied the operation,
    /// and the action comes before the operation's time is charged.
    AfterOp {
        /// Which operations it counts.
        filter: Filter,
        /// The operation it comes after.
        nth: u32,
    },
    /// After the `nth` PHY call of `kind` (any call, with `None`) returns: the simulated
    /// PHY's, or a coupled one's. A call through [`Coupled`](crate::Coupled) counts once;
    /// coupling the bench's own PHY counts its inner call too.
    PhyCall {
        /// Which calls it counts.
        kind: Option<PhyCallKind>,
        /// The call it comes after.
        nth: u32,
    },
}
