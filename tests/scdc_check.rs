//! The `ScdcTransport` check: it passes hdmi-hal's `ScdcOverDdc` over the bench, and catches
//! transports broken in one way each.

use core::cell::Cell;

use maquette::check::{ScdcImage, ScdcOp, ScdcTransportCheck, ScdcTransportFailure};
use maquette::hdmi_hal::scdc::{ScdcOverDdc, ScdcTransport};
use maquette::{Bench, Ddc, DdcCapabilities, DdcError, DdcErrorKind, SinkConfig};

/// A bench whose SCDC registers each hold a value of their own: `reg ^ 0x5A`.
fn bench() -> Bench<'static> {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    for reg in 0..=255u8 {
        bench.preset_scdc(reg, reg ^ 0x5A);
    }
    bench
}

#[test]
fn scdc_over_the_bench_bus_passes() {
    let bench = bench();
    let bus = bench.ddc();
    let image = ScdcImage::of(&bench);
    assert_eq!(image.known(), 256 - 2 - 1 - 3 - 11);
    let mut transport = ScdcOverDdc::new(&bus).unwrap();
    let check = ScdcTransportCheck::new(&image).with_faults(&bench);
    let report = check.run(&mut transport);
    report.assert_ok();
    // Every known register, 241 blocks, 3 empty, 2 past 0xFF, 3 errors and recoveries.
    assert_eq!(report.points(), image.known() + 241 + 3 + 2 + 6);
}

#[test]
fn the_bytewise_transport_passes_too() {
    let bench = bench();
    let bus = bench.ddc();
    let image = ScdcImage::of(&bench);
    let mut transport = ScdcOverDdc::bytewise(&bus).unwrap();
    ScdcTransportCheck::new(&image)
        .with_faults(&bench)
        .run(&mut transport)
        .assert_ok();
}

#[test]
fn a_bus_that_reads_16_bytes_at_most_passes() {
    let caps = DdcCapabilities::general().with_write_read_max(1, 16);
    let bench = Bench::builder(&[]).ddc_capabilities(caps).build().unwrap();
    let image = ScdcImage::of(&bench);
    let bus = bench.ddc();
    let mut transport = ScdcOverDdc::new(&bus).unwrap();
    ScdcTransportCheck::new(&image)
        .run(&mut transport)
        .assert_ok();
}

#[test]
fn without_faults_the_points_after_an_error_are_skipped() {
    let bench = bench();
    let bus = bench.ddc();
    let image = ScdcImage::of(&bench);
    let mut transport = ScdcOverDdc::new(&bus).unwrap();
    let report = ScdcTransportCheck::new(&image).run(&mut transport);
    report.assert_ok();
    assert_eq!(report.points(), image.known() + 241 + 3 + 2);
    // An image the caller states works the same.
    let stated = ScdcImage::new()
        .with(0x01, 0x01 ^ 0x5A)
        .with(0x20, 0x20 ^ 0x5A);
    assert_eq!(stated.known(), 2);
    assert_eq!(stated.get(0x20), Some(0x7A));
    let report = ScdcTransportCheck::new(&stated).run(&mut transport);
    report.assert_ok();
    assert_eq!(ScdcImage::default().known(), 0);
}

#[cfg(feature = "async")]
#[test]
fn the_async_form_passes_the_async_adapter() {
    use maquette::hdmi_hal_async::scdc::SendScdcOverDdc;
    let bench = bench();
    let bus = bench.ddc();
    let image = ScdcImage::of(&bench);
    let mut transport = SendScdcOverDdc::new(&bus).unwrap();
    let check = ScdcTransportCheck::new(&image).with_faults(&bench);
    let report = pollster::block_on(check.run_async(&mut transport));
    report.assert_ok();
    assert_eq!(report.points(), image.known() + 241 + 3 + 2 + 6);
}

/// How a [`Broken`] transport is broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flaw {
    /// `read_block` starts one register late.
    OffByOne,
    /// `read_block` refuses more than 8 bytes.
    EightAtMost,
    /// A failed `read` returns 0.
    HidesErrors,
    /// After its first error, every call fails.
    StuckAfterError,
    /// An empty `read_block` fails.
    EmptyFails,
}

/// hdmi-hal's adapter over the bench's bus, with one flaw.
struct Broken<'b> {
    inner: ScdcOverDdc<&'b Ddc<'b>>,
    flaw: Flaw,
    stuck: Cell<bool>,
}

impl Broken<'_> {
    fn watch<T>(&self, result: Result<T, DdcErrorKind>) -> Result<T, DdcErrorKind> {
        if self.flaw == Flaw::StuckAfterError {
            if self.stuck.get() {
                return Err(DdcErrorKind::Other);
            }
            if result.is_err() {
                self.stuck.set(true);
            }
        }
        result
    }
}

impl ScdcTransport for Broken<'_> {
    type Error = DdcErrorKind;

    fn read(&self, reg: u8) -> Result<u8, DdcErrorKind> {
        let result = self.inner.read(reg).map_err(|error| error.kind());
        match (self.flaw, result) {
            (Flaw::HidesErrors, Err(_)) => Ok(0),
            (_, result) => self.watch(result),
        }
    }

    fn write(&mut self, reg: u8, value: u8) -> Result<(), DdcErrorKind> {
        self.inner.write(reg, value).map_err(|error| error.kind())
    }

    fn read_block(&self, reg: u8, buf: &mut [u8]) -> Result<(), DdcErrorKind> {
        match self.flaw {
            Flaw::EightAtMost if buf.len() > 8 => return Err(DdcErrorKind::Unsupported),
            Flaw::EmptyFails if buf.is_empty() => return Err(DdcErrorKind::Other),
            _ => {}
        }
        let start = if self.flaw == Flaw::OffByOne {
            reg.wrapping_add(1)
        } else {
            reg
        };
        let result = self
            .inner
            .read_block(start, buf)
            .map_err(|error| error.kind());
        self.watch(result)
    }
}

fn failures(flaw: Flaw) -> (usize, Vec<ScdcTransportFailure>) {
    let bench = bench();
    let bus = bench.ddc();
    let image = ScdcImage::of(&bench);
    let mut transport = Broken {
        inner: ScdcOverDdc::new(&bus).unwrap(),
        flaw,
        stuck: Cell::new(false),
    };
    let report = ScdcTransportCheck::new(&image)
        .with_faults(&bench)
        .run(&mut transport);
    (report.failed(), report.failures().copied().collect())
}

#[test]
fn a_block_read_from_the_wrong_register_is_caught() {
    let (failed, failures) = failures(Flaw::OffByOne);
    assert_eq!(failed, 241, "every block");
    assert_eq!(
        failures[0],
        ScdcTransportFailure::Value {
            op: ScdcOp::ReadBlock,
            reg: 0x00,
            expected: 0x5A,
            found: 0x01 ^ 0x5A
        }
    );
}

#[test]
fn a_transport_that_cannot_read_16_bytes_is_caught() {
    let (failed, failures) = failures(Flaw::EightAtMost);
    // Every 16-byte block, and the 16 bytes past 0xFF are not a point.
    assert_eq!(failed, 241);
    assert!(failures.iter().all(|failure| matches!(
        failure,
        ScdcTransportFailure::Failed {
            op: ScdcOp::ReadBlock,
            kind: DdcErrorKind::Unsupported,
            ..
        }
    )));
}

#[test]
fn a_transport_that_hides_errors_is_caught() {
    let (failed, failures) = failures(Flaw::HidesErrors);
    assert_eq!(failed, 3);
    assert!(
        failures
            .iter()
            .all(|failure| matches!(failure, ScdcTransportFailure::Hidden { found: Ok(()), .. }))
    );
}

#[test]
fn a_transport_stuck_after_an_error_is_caught() {
    let (failed, failures) = failures(Flaw::StuckAfterError);
    // The first injected error reaches the caller; everything after it fails.
    assert_eq!(failed, 5);
    assert!(failures.contains(&ScdcTransportFailure::Failed {
        op: ScdcOp::Read,
        reg: 0x00,
        kind: DdcErrorKind::Other
    }));
}

#[test]
fn a_transport_that_fails_an_empty_buffer_is_caught() {
    let (failed, _) = failures(Flaw::EmptyFails);
    assert_eq!(failed, 3);
}

#[test]
fn a_sink_without_scdc_fails_every_read() {
    let bench = Bench::new(SinkConfig::new().without_scdc(), &[]).unwrap();
    let bus = bench.ddc();
    let stated = ScdcImage::new().with(0x01, 1);
    let mut transport = ScdcOverDdc::new(&bus).unwrap();
    let report = ScdcTransportCheck::new(&stated).run(&mut transport);
    assert_eq!(report.failed(), 1 + 241);
}

#[test]
fn failures_print_what_went_wrong() {
    let printed = [
        ScdcTransportFailure::Value {
            op: ScdcOp::Read,
            reg: 0x20,
            expected: 3,
            found: 0,
        },
        ScdcTransportFailure::Failed {
            op: ScdcOp::ReadBlock,
            reg: 0xF0,
            kind: DdcErrorKind::Busy,
        },
        ScdcTransportFailure::Hidden {
            expected: DdcErrorKind::Timeout,
            found: Ok(()),
        },
    ]
    .map(|failure| failure.to_string());
    assert_eq!(
        printed,
        [
            "Read of register 0x20 gave 0x00, not 0x03",
            "ReadBlock at register 0xf0 failed: Busy",
            "a transfer failed with Timeout came back Ok(())",
        ]
    );
    let image = ScdcImage::new();
    assert_eq!(
        format!("{:?}", ScdcTransportCheck::new(&image)),
        "ScdcTransportCheck { known: 0, faults: false }"
    );
}
