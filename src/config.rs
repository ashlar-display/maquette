//! The sink's settings.

/// The sink's settings.
///
/// [`new`](Self::new) is the default sink: an SCDC sink reporting `Sink_Version` 1, with the
/// defaults of each setting. Each setting has a `const` `with_*` (or `without_*`) builder;
/// there are no named sink models, since a name would suggest a real product whose behaviour
/// no source backs. Where no public source settles what a sink does, the setting's default
/// is marked *chosen*.
///
/// ```
/// use maquette::{OwnedWrite, SinkConfig};
///
/// const STRICT: SinkConfig = SinkConfig::new()
///     .with_sink_version(1)
///     .with_owned_scdc_writes(OwnedWrite::Nack);
/// const NO_SCDC: SinkConfig = SinkConfig::new().without_scdc();
/// # let _ = (STRICT, NO_SCDC);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct SinkConfig {
    pub(crate) scdc: bool,
    pub(crate) sink_version: u8,
    pub(crate) owned_scdc_write: OwnedWrite,
    pub(crate) scdc_past_end: ScdcPastEnd,
}

impl SinkConfig {
    /// The default sink.
    pub const fn new() -> Self {
        Self {
            scdc: true,
            sink_version: 1,
            owned_scdc_write: OwnedWrite::Ignore,
            scdc_past_end: ScdcPastEnd::Wrap,
        }
    }

    /// A sink without SCDC: nothing answers 0x54 (*chosen*: no source says how such a sink
    /// answers).
    pub const fn without_scdc(self) -> Self {
        Self {
            scdc: false,
            ..self
        }
    }

    /// The `Sink_Version` the sink reports; 1 by default.
    pub const fn with_sink_version(self, sink_version: u8) -> Self {
        Self {
            sink_version,
            ..self
        }
    }

    /// What a write to an SCDC register the sink owns does; by default
    /// [`OwnedWrite::Ignore`].
    pub const fn with_owned_scdc_writes(self, owned_scdc_write: OwnedWrite) -> Self {
        Self {
            owned_scdc_write,
            ..self
        }
    }

    /// What an SCDC read or write past 0xFF does; by default [`ScdcPastEnd::Wrap`].
    pub const fn with_scdc_past_end(self, scdc_past_end: ScdcPastEnd) -> Self {
        Self {
            scdc_past_end,
            ..self
        }
    }

    /// Whether the sink has SCDC.
    pub const fn has_scdc(&self) -> bool {
        self.scdc
    }

    /// The `Sink_Version` the sink reports.
    pub const fn sink_version(&self) -> u8 {
        self.sink_version
    }

    /// What a write to an SCDC register the sink owns does.
    pub const fn owned_scdc_writes(&self) -> OwnedWrite {
        self.owned_scdc_write
    }

    /// What an SCDC read or write past 0xFF does.
    pub const fn scdc_past_end(&self) -> ScdcPastEnd {
        self.scdc_past_end
    }
}

impl Default for SinkConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// What a write to an SCDC register the sink owns (`Sink_Version`, `Scrambling_Status`, the
/// status flags, the error counters, the manufacturer's block) does. No public source says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OwnedWrite {
    /// Acknowledged and ignored, recorded as an `IgnoredWrite` (*chosen* default).
    Ignore,
    /// Not acknowledged: `NoAcknowledge(Data)` at that byte, the bytes before it written.
    Nack,
}

/// What an SCDC read or write that runs past 0xFF in one burst does. hdmi-hal's
/// `ScdcTransport` leaves it to the sink, and no public source says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScdcPastEnd {
    /// The offset wraps to 0x00, as an 8-bit offset register does (*chosen* default).
    Wrap,
    /// The byte past 0xFF fails `NoAcknowledge(Data)`, the bytes before it done.
    Nack,
}
