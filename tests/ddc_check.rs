//! The `DdcBus` check: it passes the bench's bus in its shapes, and catches buses broken in
//! one way each.

use maquette::check::{DdcBusCheck, DdcBusFailure, DdcDevice};
use maquette::hdmi_hal::ddc::SEGMENT_POINTER;
use maquette::{
    Bench, Ddc, DdcBus, DdcCapabilities, DdcError, DdcErrorKind, EddcTarget, OpKind, SinkConfig,
};

/// Four EDID blocks, each byte distinct from its neighbours.
fn edid() -> Vec<u8> {
    (0..512).map(|i| (i % 251) as u8).collect()
}

const SILENT: u8 = 0x37;

#[test]
fn the_bench_bus_passes_with_a_described_device() {
    let store = edid();
    let bench = Bench::new(SinkConfig::new(), &store).unwrap();
    let device = DdcDevice::new().with_edid(&store).with_silent(SILENT);
    let report = DdcBusCheck::new(device).run(&bench.ddc());
    report.assert_ok();
    assert!(report.points() >= 10, "{} points", report.points());
}

#[test]
fn a_narrow_bus_passes_and_has_more_to_refuse() {
    let store = edid();
    let smbus = DdcCapabilities::general()
        .with_write_max(33)
        .with_write_read_max(1, 32)
        .with_read_max(1)
        .with_max_segment(Some(0))
        .with_segmented_piece(Some(32))
        .with_eddc_target(EddcTarget::DisplayId, false);
    let bench = Bench::builder(&store)
        .ddc_capabilities(smbus)
        .build()
        .unwrap();
    let device = DdcDevice::new().with_edid(&store).with_silent(SILENT);
    let narrow = DdcBusCheck::new(device).run(&bench.ddc());
    narrow.assert_ok();
    let general = Bench::new(SinkConfig::new(), &store).unwrap();
    let wide = DdcBusCheck::new(device).run(&general.ddc());
    assert!(narrow.points() > wide.points() - 1);
}

#[test]
fn a_device_without_a_segment_pointer_is_read_at_segment_0() {
    let store = edid();
    let bench = Bench::new(SinkConfig::new().without_segment_pointer(), &store).unwrap();
    let device = DdcDevice::new()
        .with_edid(&store)
        .without_segment_pointer()
        .with_silent(SILENT);
    DdcBusCheck::new(device).run(&bench.ddc()).assert_ok();
}

#[test]
fn points_without_a_description_are_skipped() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    let report = DdcBusCheck::new(DdcDevice::default()).run(&bench.ddc());
    report.assert_ok();
    let store = edid();
    let full = Bench::new(SinkConfig::new(), &store).unwrap();
    let described =
        DdcBusCheck::new(DdcDevice::new().with_edid(&store).with_silent(SILENT)).run(&full.ddc());
    assert!(report.points() < described.points());
}

#[cfg(feature = "std")]
#[test]
fn concurrent_calls_pass_from_several_threads() {
    let store = edid();
    let bench = Bench::new(SinkConfig::new(), &store).unwrap();
    let check = DdcBusCheck::new(DdcDevice::new().with_edid(&store).with_silent(SILENT));
    // Each thread runs every point but the read at the current address.
    let single = check.run(&bench.ddc()).points() - 1;
    let report = check.run_threads(&bench.ddc(), 4);
    report.assert_ok();
    assert_eq!(report.points(), 4 * single);
    // As many threads as asked for, past any fixed number.
    let report = check.run_threads(&bench.ddc(), 40);
    report.assert_ok();
    assert_eq!(report.points(), 40 * single);
}

#[cfg(feature = "std")]
#[test]
#[should_panic(expected = "at least one thread")]
fn run_threads_needs_a_thread() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    DdcBusCheck::new(DdcDevice::new()).run_threads(&bench.ddc(), 0);
}

#[cfg(feature = "async")]
#[test]
fn the_async_form_passes_the_async_bus() {
    let store = edid();
    let bench = Bench::new(SinkConfig::new(), &store).unwrap();
    let check = DdcBusCheck::new(DdcDevice::new().with_edid(&store).with_silent(SILENT));
    let report = pollster::block_on(check.run_async(&bench.ddc()));
    report.assert_ok();
    assert_eq!(report.points(), check.run(&bench.ddc()).points());
}

/// How a [`Broken`] bus is broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flaw {
    /// Reports `read_max` 1 but reads any length.
    ClaimsLess,
    /// Accepts an empty write.
    EmptyWrite,
    /// Writes the segment pointer for segment 0 too, as its own transaction.
    PointerAlways,
    /// Corrupts byte 5 of every read.
    Corrupts,
    /// Answers at the silent address.
    AnswersSilent,
    /// Panics on every read.
    #[cfg_attr(not(feature = "std"), allow(dead_code))]
    Panics,
    /// `read_segmented` reads from offset 0, but where it must refuse.
    IgnoresOffset,
    /// `write_read` drops its write: a plain read.
    DropsWriteOfWriteRead,
    /// `write` sends nothing and returns `Ok`.
    DropsWrite,
}

/// The bench's bus with one flaw.
struct Broken<'b> {
    inner: Ddc<'b>,
    flaw: Flaw,
}

fn kind<E: DdcError>(result: Result<(), E>) -> Result<(), DdcErrorKind> {
    result.map_err(|error| error.kind())
}

impl DdcBus for Broken<'_> {
    type Error = DdcErrorKind;

    fn capabilities(&self) -> DdcCapabilities {
        let caps = self.inner.capabilities();
        match self.flaw {
            Flaw::ClaimsLess => caps.with_read_max(1),
            _ => caps,
        }
    }

    fn write(&self, addr: u8, bytes: &[u8]) -> Result<(), DdcErrorKind> {
        match self.flaw {
            Flaw::EmptyWrite if bytes.is_empty() => Ok(()),
            Flaw::AnswersSilent if addr == SILENT && !bytes.is_empty() => Ok(()),
            Flaw::DropsWrite if !bytes.is_empty() => Ok(()),
            _ => kind(self.inner.write(addr, bytes)),
        }
    }

    fn write_read(&self, addr: u8, bytes: &[u8], buf: &mut [u8]) -> Result<(), DdcErrorKind> {
        match self.flaw {
            Flaw::DropsWriteOfWriteRead if !bytes.is_empty() => kind(self.inner.read(addr, buf)),
            _ => kind(self.inner.write_read(addr, bytes, buf)),
        }
    }

    fn read(&self, addr: u8, buf: &mut [u8]) -> Result<(), DdcErrorKind> {
        match self.flaw {
            Flaw::Panics => panic!("a broken bus"),
            Flaw::AnswersSilent if addr == SILENT => Ok(()),
            Flaw::ClaimsLess => {
                // Bypasses its own refusal: reads through a bus that reaches more.
                kind(self.inner.read(addr, buf))
            }
            _ => kind(self.inner.read(addr, buf)),
        }
    }

    fn read_segmented(
        &self,
        target: EddcTarget,
        segment: u8,
        offset: u8,
        buf: &mut [u8],
    ) -> Result<(), DdcErrorKind> {
        if self.flaw == Flaw::PointerAlways && segment == 0 && !buf.is_empty() {
            kind(self.inner.write(SEGMENT_POINTER, &[0]))?;
        }
        let fits = usize::from(offset) + buf.len() <= 256;
        let offset = if self.flaw == Flaw::IgnoresOffset && fits {
            0
        } else {
            offset
        };
        kind(self.inner.read_segmented(target, segment, offset, buf))?;
        if self.flaw == Flaw::Corrupts {
            if let Some(byte) = buf.get_mut(5) {
                *byte ^= 0xFF;
            }
        }
        Ok(())
    }
}

fn failures(flaw: Flaw, config: SinkConfig) -> Vec<DdcBusFailure> {
    let store = edid();
    let bench = Bench::new(config, &store).unwrap();
    let mut device = DdcDevice::new().with_edid(&store).with_silent(SILENT);
    if !config.has_segment_pointer() {
        device = device.without_segment_pointer();
    }
    let bus = Broken {
        inner: bench.ddc(),
        flaw,
    };
    DdcBusCheck::new(device)
        .run(&bus)
        .failures()
        .copied()
        .collect()
}

#[test]
fn a_bus_that_does_more_than_it_claims_is_caught() {
    let failures = failures(Flaw::ClaimsLess, SinkConfig::new());
    assert!(failures.iter().any(|failure| matches!(
        failure,
        DdcBusFailure::NotRefused {
            op: OpKind::Read,
            found: Ok(()),
            ..
        }
    )));
}

#[test]
fn a_bus_that_accepts_an_empty_write_is_caught() {
    let failures = failures(Flaw::EmptyWrite, SinkConfig::new());
    assert_eq!(
        failures,
        [DdcBusFailure::NotRefused {
            op: OpKind::Write,
            addr: SILENT,
            found: Ok(())
        }]
    );
}

#[test]
fn writing_the_pointer_for_segment_0_fails_on_a_device_without_one() {
    // On a device with a pointer it does no harm.
    assert_eq!(failures(Flaw::PointerAlways, SinkConfig::new()), []);
    let failures = failures(
        Flaw::PointerAlways,
        SinkConfig::new().without_segment_pointer(),
    );
    assert!(failures.iter().any(|failure| matches!(
        failure,
        DdcBusFailure::Failed {
            op: OpKind::ReadSegmented,
            kind: DdcErrorKind::NoAcknowledge(_),
            ..
        }
    )));
}

#[test]
fn a_bus_that_corrupts_what_it_reads_is_caught() {
    let failures = failures(Flaw::Corrupts, SinkConfig::new());
    let store = edid();
    assert!(failures.contains(&DdcBusFailure::Data {
        segment: 0,
        offset: 5,
        expected: store[5],
        found: store[5] ^ 0xFF
    }));
}

#[test]
fn a_bus_that_answers_where_nothing_does_is_caught() {
    let failures = failures(Flaw::AnswersSilent, SinkConfig::new());
    assert_eq!(failures.len(), 2, "{failures:?}");
    assert!(
        failures
            .iter()
            .all(|failure| matches!(failure, DdcBusFailure::Answered { addr: SILENT, .. }))
    );
}

#[cfg(feature = "std")]
#[test]
fn a_panicking_thread_is_a_failure() {
    let store = edid();
    let bench = Bench::new(SinkConfig::new(), &store).unwrap();
    let bus = Broken {
        inner: bench.ddc(),
        flaw: Flaw::Panics,
    };
    let report = DdcBusCheck::new(DdcDevice::new()).run_threads(&bus, 2);
    assert_eq!(report.failed(), 2);
    assert!(
        report
            .failures()
            .all(|failure| *failure == DdcBusFailure::Panicked)
    );
}

/// The failures of a bus with `flaw`, before a device described without a silent address:
/// only the data points can catch what it does wrong.
fn data_failures(flaw: Flaw) -> Vec<DdcBusFailure> {
    let store = edid();
    let bench = Bench::new(SinkConfig::new(), &store).unwrap();
    let bus = Broken {
        inner: bench.ddc(),
        flaw,
    };
    DdcBusCheck::new(DdcDevice::new().with_edid(&store))
        .run(&bus)
        .failures()
        .copied()
        .collect()
}

#[test]
fn a_bus_that_ignores_the_offset_is_caught() {
    // The store's byte at n is n % 251: offset 0 reads 0 where 128 and 0x11 are expected.
    assert_eq!(
        data_failures(Flaw::IgnoresOffset),
        [
            DdcBusFailure::Data {
                segment: 0,
                offset: 128,
                expected: 128,
                found: 0
            },
            DdcBusFailure::Data {
                segment: 0,
                offset: 0x11,
                expected: 0x11,
                found: 0
            },
        ]
    );
}

#[test]
fn a_write_read_that_drops_its_write_is_caught() {
    let failures = data_failures(Flaw::DropsWriteOfWriteRead);
    assert!(
        failures.iter().any(|failure| matches!(
            failure,
            DdcBusFailure::Data {
                segment: 0,
                offset: 0x20,
                ..
            }
        )),
        "{failures:?}"
    );
}

#[test]
fn a_write_that_sends_nothing_is_caught() {
    // The read at the current address reads where the write_read before it stopped, 0x30.
    assert_eq!(
        data_failures(Flaw::DropsWrite),
        [DdcBusFailure::Data {
            segment: 0,
            offset: 0x40,
            expected: 0x40,
            found: 0x30
        }]
    );
}

#[test]
fn failures_print_what_went_wrong() {
    let failure = DdcBusFailure::NotRefused {
        op: OpKind::Write,
        addr: 0x37,
        found: Ok(()),
    };
    assert_eq!(failure.to_string(), "Write at 0x37 was not refused: Ok(())");
    assert_eq!(DdcBusFailure::Panicked.to_string(), "a thread panicked");
}

#[test]
fn unreachable_addresses_and_no_segmented_reads_are_refused() {
    let caps = DdcCapabilities::general()
        .with_address_reachable(0x08, false)
        .with_max_segment(None);
    let bench = Bench::builder(&[]).ddc_capabilities(caps).build().unwrap();
    let report = DdcBusCheck::new(DdcDevice::new()).run(&bench.ddc());
    report.assert_ok();
    // The same points against a bus that claims those limits and ignores them.
    let general = Bench::new(SinkConfig::new(), &[]).unwrap();
    struct Claims<'b>(Ddc<'b>, DdcCapabilities);
    impl DdcBus for Claims<'_> {
        type Error = DdcErrorKind;
        fn capabilities(&self) -> DdcCapabilities {
            self.1
        }
        fn write(&self, addr: u8, bytes: &[u8]) -> Result<(), DdcErrorKind> {
            kind(self.0.write(addr, bytes))
        }
        fn write_read(&self, addr: u8, bytes: &[u8], buf: &mut [u8]) -> Result<(), DdcErrorKind> {
            kind(self.0.write_read(addr, bytes, buf))
        }
        fn read(&self, addr: u8, buf: &mut [u8]) -> Result<(), DdcErrorKind> {
            kind(self.0.read(addr, buf))
        }
        fn read_segmented(
            &self,
            target: EddcTarget,
            segment: u8,
            offset: u8,
            buf: &mut [u8],
        ) -> Result<(), DdcErrorKind> {
            kind(self.0.read_segmented(target, segment, offset, buf))
        }
    }
    let report = DdcBusCheck::new(DdcDevice::new()).run(&Claims(general.ddc(), caps));
    assert_eq!(report.failed(), 2, "{report}");
}

#[cfg(feature = "std")]
#[test]
fn failures_from_several_threads_are_all_counted() {
    let store = edid();
    let bench = Bench::new(SinkConfig::new(), &store).unwrap();
    let bus = Broken {
        inner: bench.ddc(),
        flaw: Flaw::EmptyWrite,
    };
    let check = DdcBusCheck::new(DdcDevice::new().with_silent(SILENT));
    let report = check.run_threads(&bus, 3);
    assert_eq!(report.failed(), 3);
    assert_eq!(report.failures().count(), 3);
    assert_eq!(report.points(), 3 * check.run(&bus).points());
}

#[test]
fn every_failure_prints() {
    let printed = [
        DdcBusFailure::Failed {
            op: OpKind::Read,
            addr: 0x50,
            kind: DdcErrorKind::Busy,
        },
        DdcBusFailure::Answered {
            op: OpKind::Read,
            addr: 0x37,
            found: Ok(()),
        },
        DdcBusFailure::Data {
            segment: 1,
            offset: 5,
            expected: 1,
            found: 2,
        },
    ]
    .map(|failure| failure.to_string());
    assert_eq!(
        printed,
        [
            "Read at 0x50 failed: Busy",
            "Read at 0x37, where nothing answers, ended Ok(())",
            "EDID segment 1 byte 0x05 read 0x02, not 0x01",
        ]
    );
    let empty: maquette::check::Report<DdcBusFailure> = Default::default();
    assert!(empty.is_ok());
}
