//! The line: what the source transmits, as the sink sees it.

use hdmi_hal::HdmiForumFrl;
use hdmi_hal::phy::TmdsClockRatio;

/// The TMDS character rate above which the bit clock ratio is 1/40, and at or below which
/// it is 1/10, in kHz. Above it the characters are scrambled; at or below it scrambling is
/// optional.
///
/// Settled by public sources: Linux, github.com/torvalds/linux `e9954f3fbddd`,
/// `drivers/gpu/drm/display/drm_scdc_helper.c:262–267` (the rule) and
/// `drivers/gpu/drm/i915/display/intel_hdmi.c:2437–2440` (`port_clock > 340000` sets
/// scrambling and the 1/40 ratio); amdgpu, the same tree,
/// `drivers/gpu/drm/amd/display/dc/link/protocols/link_ddc.c:575`, `604–605`
/// (`pix_clk > 340000` writes `TMDS_Config` 3); Xilinx embeddedsw `97f2baf7f69e`,
/// `XilinxProcessorIPLib/drivers/v_hdmitx1/src/xv_hdmitx1.c:740–743` (the ratio bit above
/// 3.4 Gbps).
pub(crate) const RATIO_40_ABOVE_KHZ: u32 = 340_000;

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
    /// FRL at `rate`; what it carries comes with the sink's FRL responses, so it is built
    /// with [`Line::frl`] and matched with `..`.
    #[non_exhaustive]
    Frl {
        /// The FRL rate.
        rate: HdmiForumFrl,
    },
}

impl Line {
    /// FRL at `rate`.
    pub const fn frl(rate: HdmiForumFrl) -> Self {
        Self::Frl { rate }
    }
}

impl Default for Line {
    /// [`Line::Off`]: nothing transmitted, as at a bench's creation.
    fn default() -> Self {
        Self::Off
    }
}

/// Character errors on the link, per lane, in errors a second: what the sink's character
/// error counters count while the lane is locked. Lane 3 carries data in 4-lane FRL only.
///
/// The test sets them with
/// [`Bench::set_character_errors`](crate::Bench::set_character_errors); a target sees them
/// with [`Env::character_errors`](crate::Env::character_errors). At creation there are none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ErrorRates {
    /// Errors a second on lane 0.
    pub lane0: u32,
    /// Errors a second on lane 1.
    pub lane1: u32,
    /// Errors a second on lane 2.
    pub lane2: u32,
    /// Errors a second on lane 3.
    pub lane3: u32,
}

impl ErrorRates {
    /// No errors on any lane.
    pub const NONE: Self = Self {
        lane0: 0,
        lane1: 0,
        lane2: 0,
        lane3: 0,
    };

    /// `per_second` errors a second on every lane.
    pub const fn all(per_second: u32) -> Self {
        Self {
            lane0: per_second,
            lane1: per_second,
            lane2: per_second,
            lane3: per_second,
        }
    }

    /// The rates in lane order.
    pub(crate) const fn lanes(self) -> [u32; 4] {
        [self.lane0, self.lane1, self.lane2, self.lane3]
    }
}
