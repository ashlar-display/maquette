//! The line: what the source transmits, as the sink sees it.

use hdmi_hal::HdmiForumFrl;
use hdmi_hal::phy::TmdsClockRatio;

/// What the source transmits, as the sink sees it.
///
/// The test sets it with [`Bench::set_line`](crate::Bench::set_line); a target sees it with
/// [`Env::line`](crate::Env::line). At creation nothing is transmitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Line {
    /// Nothing transmitted.
    Off,
    /// TMDS at `character_rate_khz`, the clock at `ratio`, scrambled or not.
    Tmds {
        /// The TMDS character rate, in kHz.
        character_rate_khz: u32,
        /// The TMDS bit clock ratio.
        ratio: TmdsClockRatio,
        /// Whether the characters are scrambled.
        scrambled: bool,
    },
    /// FRL at `rate`; what it carries comes with the sink's FRL responses.
    Frl {
        /// The FRL rate.
        rate: HdmiForumFrl,
    },
}
