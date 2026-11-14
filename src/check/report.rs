//! `Report`: what a check found.

use core::fmt;

/// What a check found: the points that failed, in the order found.
///
/// A check runs every point rather than stopping at the first failure, so an implementor sees
/// every failure at once. The first `N` failures are recorded; past them, failures are counted,
/// so [`failed`](Self::failed) stays exact. [`assert_ok`](Self::assert_ok) is the one
/// assertion a test needs; [`Display`](fmt::Display) prints one line per recorded failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report<F, const N: usize = 16> {
    recorded: [Option<F>; N],
    len: usize,
    failed: usize,
    points: usize,
}

impl<F, const N: usize> Report<F, N> {
    /// An empty report: no points run.
    pub(crate) fn new() -> Self {
        Self {
            recorded: core::array::from_fn(|_| None),
            len: 0,
            failed: 0,
            points: 0,
        }
    }

    /// Records a point: passed when `failure` is `None`.
    pub(crate) fn point(&mut self, failure: Option<F>) {
        self.points = self.points.saturating_add(1);
        if let Some(failure) = failure {
            self.failed = self.failed.saturating_add(1);
            if let Some(slot) = self.recorded.get_mut(self.len) {
                *slot = Some(failure);
                self.len += 1;
            }
        }
    }

    /// Whether every point passed.
    pub fn is_ok(&self) -> bool {
        self.failed == 0
    }

    /// How many points the check ran.
    pub fn points(&self) -> usize {
        self.points
    }

    /// How many points failed, recorded or not.
    pub fn failed(&self) -> usize {
        self.failed
    }

    /// The failed points recorded: the first `N`, in the order found.
    pub fn failures(&self) -> impl Iterator<Item = &F> {
        self.recorded[..self.len].iter().flatten()
    }

    /// How many failed points were not recorded, past the first `N`.
    pub fn overflow(&self) -> usize {
        self.failed - self.len
    }

    /// Panics, listing every recorded failure and the overflow, unless every point passed.
    #[track_caller]
    pub fn assert_ok(&self)
    where
        F: fmt::Display,
    {
        if !self.is_ok() {
            panic!("{self}");
        }
    }
}

impl<F: fmt::Display, const N: usize> fmt::Display for Report<F, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_ok() {
            return write!(f, "all {} points passed", self.points);
        }
        write!(f, "{} of {} points failed:", self.failed, self.points)?;
        for failure in self.failures() {
            write!(f, "\n- {failure}")?;
        }
        if self.overflow() > 0 {
            write!(f, "\n- and {} more, not recorded", self.overflow())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::string::ToString;

    fn report<const N: usize>(results: &[Option<u32>]) -> Report<u32, N> {
        let mut report = Report::new();
        for &result in results {
            report.point(result);
        }
        report
    }

    #[test]
    fn an_empty_report_is_ok() {
        let report = report::<4>(&[]);
        assert!(report.is_ok());
        assert_eq!(report.points(), 0);
        assert_eq!(report.failed(), 0);
        assert_eq!(report.overflow(), 0);
        assert_eq!(report.failures().count(), 0);
        assert_eq!(report.to_string(), "all 0 points passed");
        report.assert_ok();
    }

    #[test]
    fn counts_points_and_records_failures_in_order() {
        let report = report::<4>(&[None, Some(1), None, Some(2), None]);
        assert!(!report.is_ok());
        assert_eq!(report.points(), 5);
        assert_eq!(report.failed(), 2);
        assert_eq!(report.overflow(), 0);
        assert!(report.failures().eq([1, 2].iter()));
        assert_eq!(report.to_string(), "2 of 5 points failed:\n- 1\n- 2");
    }

    #[test]
    fn counts_failures_past_its_capacity() {
        let report = report::<2>(&[Some(1), Some(2), None, Some(3), Some(4)]);
        assert_eq!(report.failed(), 4);
        assert_eq!(report.overflow(), 2);
        assert!(report.failures().eq([1, 2].iter()));
        assert_eq!(
            report.to_string(),
            "4 of 5 points failed:\n- 1\n- 2\n- and 2 more, not recorded"
        );
    }

    #[test]
    fn a_report_of_no_capacity_still_counts() {
        let report = report::<0>(&[Some(1), Some(2)]);
        assert_eq!(report.failed(), 2);
        assert_eq!(report.overflow(), 2);
        assert_eq!(report.failures().count(), 0);
    }

    #[test]
    fn the_default_capacity_is_16() {
        let mut report: Report<u32> = Report::new();
        for n in 0..20 {
            report.point(Some(n));
        }
        assert_eq!(report.failures().count(), 16);
        assert_eq!(report.overflow(), 4);
    }

    #[test]
    fn display_passes_on_a_writer_error() {
        /// A writer that fails once it has taken `left` writes.
        struct Failing {
            left: usize,
        }
        impl fmt::Write for Failing {
            fn write_str(&mut self, _: &str) -> fmt::Result {
                if self.left == 0 {
                    return Err(fmt::Error);
                }
                self.left -= 1;
                Ok(())
            }
        }
        let report = report::<1>(&[Some(1), Some(2)]);
        let full = report.to_string();
        let mut writes = 0;
        while fmt::write(&mut Failing { left: writes }, format_args!("{report}")).is_err() {
            writes += 1;
            assert!(writes < full.len(), "the report never finished");
        }
        // Every write before the last one was given the chance to fail.
        assert!(writes > 3);
    }

    #[test]
    #[should_panic(expected = "1 of 2 points failed:\n- 7")]
    fn assert_ok_panics_listing_the_failures() {
        report::<4>(&[None, Some(7)]).assert_ok();
    }
}
