//! The line: what the source transmits, as the sink sees it.

use hdmi_hal::HdmiForumFrl;
use hdmi_hal::phy::{EqParams, FrlOutput, LanePatterns, TmdsClockRatio, TxFfeLevel};

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
    /// FRL at `rate`: the link training pattern on each lane, and what the lanes carry
    /// once the patterns stop. Built with [`Line::frl`] or [`Line::frl_with`] and matched
    /// with `..`, so what FRL carries can grow.
    #[non_exhaustive]
    Frl {
        /// The FRL rate.
        rate: HdmiForumFrl,
        /// The link training pattern on each lane, as `FrlPhy::send_ltp` set it.
        patterns: LanePatterns,
        /// What the lanes carry without a pattern, as `FrlPhy::set_frl_output` set it.
        output: FrlOutput,
        /// The equalization `FrlPhy::adjust_equalization` last set at this rate; `None`
        /// since the PHY entered it: every lane at TxFFE level 0.
        equalization: Option<EqParams>,
    },
}

impl Line {
    /// FRL at `rate`, as a PHY leaves it when it enters the rate: no patterns, gap
    /// characters only.
    pub const fn frl(rate: HdmiForumFrl) -> Self {
        Self::frl_with(
            rate,
            LanePatterns {
                lane0: None,
                lane1: None,
                lane2: None,
                lane3: None,
            },
            FrlOutput::GapOnly,
        )
    }

    /// FRL at `rate`, with `patterns` on the lanes and `output` without one.
    pub const fn frl_with(rate: HdmiForumFrl, patterns: LanePatterns, output: FrlOutput) -> Self {
        Self::Frl {
            rate,
            patterns,
            output,
            equalization: None,
        }
    }

    /// The line with `equalization` on its FRL lanes; a line not in FRL is left as it is.
    #[must_use]
    pub const fn with_equalization(self, equalization: EqParams) -> Self {
        match self {
            Self::Frl {
                rate,
                patterns,
                output,
                ..
            } => Self::Frl {
                rate,
                patterns,
                output,
                equalization: Some(equalization),
            },
            other => other,
        }
    }
}

impl Default for Line {
    /// [`Line::Off`]: nothing transmitted, as at a bench's creation.
    fn default() -> Self {
        Self::Off
    }
}

/// Character errors on the link, per lane, in errors a second: what the sink's character
/// error counters count while the lane is locked. Lane 3 carries data in 4-lane FRL only;
/// the Reed-Solomon corrections are FRL's.
///
/// The test sets them with
/// [`Bench::set_character_errors`](crate::Bench::set_character_errors); a target sees them
/// with [`Env::character_errors`](crate::Env::character_errors). At creation there are none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct ErrorRates {
    /// Errors a second on lane 0.
    pub lane0: u32,
    /// Errors a second on lane 1.
    pub lane1: u32,
    /// Errors a second on lane 2.
    pub lane2: u32,
    /// Errors a second on lane 3.
    pub lane3: u32,
    /// Reed-Solomon corrections a second, in FRL.
    pub rs_corrections: u32,
}

impl ErrorRates {
    /// No errors on any lane.
    pub const NONE: Self = Self {
        lane0: 0,
        lane1: 0,
        lane2: 0,
        lane3: 0,
        rs_corrections: 0,
    };

    /// `per_second` errors a second on every lane, and no corrections.
    pub const fn all(per_second: u32) -> Self {
        Self {
            lane0: per_second,
            lane1: per_second,
            lane2: per_second,
            lane3: per_second,
            rs_corrections: 0,
        }
    }

    /// The rates with `per_second` errors a second on lane 0.
    #[must_use]
    pub const fn with_lane0(self, per_second: u32) -> Self {
        Self {
            lane0: per_second,
            ..self
        }
    }

    /// The rates with `per_second` errors a second on lane 1.
    #[must_use]
    pub const fn with_lane1(self, per_second: u32) -> Self {
        Self {
            lane1: per_second,
            ..self
        }
    }

    /// The rates with `per_second` errors a second on lane 2.
    #[must_use]
    pub const fn with_lane2(self, per_second: u32) -> Self {
        Self {
            lane2: per_second,
            ..self
        }
    }

    /// The rates with `per_second` errors a second on lane 3.
    #[must_use]
    pub const fn with_lane3(self, per_second: u32) -> Self {
        Self {
            lane3: per_second,
            ..self
        }
    }

    /// The rates with `per_second` Reed-Solomon corrections a second.
    #[must_use]
    pub const fn with_rs_corrections(self, per_second: u32) -> Self {
        Self {
            rs_corrections: per_second,
            ..self
        }
    }

    /// The rates in lane order.
    pub(crate) const fn lanes(self) -> [u32; 4] {
        [self.lane0, self.lane1, self.lane2, self.lane3]
    }
}

/// What one lane of the link can carry in FRL link training: the highest rate it trains at,
/// and the least TxFFE level the source must apply on it. A lane outside its limits does
/// not carry the training patterns the sink asks for, so training does not pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct LaneLimits {
    /// The highest FRL rate the lane trains at; `None` for any.
    pub max_rate: Option<HdmiForumFrl>,
    /// The least TxFFE level it needs, at every rate it trains at.
    pub min_tx_ffe: TxFfeLevel,
}

impl LaneLimits {
    /// No limits: any rate, at TxFFE level 0.
    pub const NONE: Self = Self {
        max_rate: None,
        min_tx_ffe: match TxFfeLevel::new(0) {
            Some(level) => level,
            None => unreachable!(),
        },
    };

    /// The limits with the lane training at `max_rate` at most; `None` for any.
    #[must_use]
    pub const fn with_max_rate(self, max_rate: Option<HdmiForumFrl>) -> Self {
        Self { max_rate, ..self }
    }

    /// The limits with the lane needing TxFFE level `min_tx_ffe` at least.
    #[must_use]
    pub const fn with_min_tx_ffe(self, min_tx_ffe: TxFfeLevel) -> Self {
        Self { min_tx_ffe, ..self }
    }
}

impl Default for LaneLimits {
    /// [`LaneLimits::NONE`].
    fn default() -> Self {
        Self::NONE
    }
}

/// Each lane's [`LaneLimits`]: what the link can carry in FRL link training.
///
/// The test sets them with [`Bench::set_lane_limits`](crate::Bench::set_lane_limits); a
/// target sees them with [`Env::lane_limits`](crate::Env::lane_limits). At creation there
/// are none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LinkLanes {
    /// Lane 0's limits.
    pub lane0: LaneLimits,
    /// Lane 1's limits.
    pub lane1: LaneLimits,
    /// Lane 2's limits.
    pub lane2: LaneLimits,
    /// Lane 3's limits.
    pub lane3: LaneLimits,
}

impl LinkLanes {
    /// No limits on any lane.
    pub const NONE: Self = Self::all(LaneLimits::NONE);

    /// `limits` on every lane.
    pub const fn all(limits: LaneLimits) -> Self {
        Self {
            lane0: limits,
            lane1: limits,
            lane2: limits,
            lane3: limits,
        }
    }

    /// The limits in lane order.
    pub(crate) const fn lanes(self) -> [LaneLimits; 4] {
        [self.lane0, self.lane1, self.lane2, self.lane3]
    }
}
