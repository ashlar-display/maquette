//! The sink's settings.

/// The sink's settings.
///
/// [`new`](Self::new) is the default sink: an SCDC sink with a segment pointer. Each setting
/// has a `const` `with_*` builder, added with the behaviour it sets; there are no named sink
/// models, since a name would suggest a real product whose behaviour no source backs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct SinkConfig {}

impl SinkConfig {
    /// The default sink.
    pub const fn new() -> Self {
        Self {}
    }
}
