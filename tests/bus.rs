//! The bench's DDC bus over a test's own targets: routing, refusals, `read_segmented`,
//! timing, errors and the transcript.

use core::time::Duration;

use maquette::hdmi_hal::ddc::{EDID, SCDC, SEGMENT_POINTER};
use maquette::hdmi_hal::scdc::{ScdcOverDdc, ScdcTransport};
use maquette::{
    Bench, BenchError, BusError, BusOp, BusRecord, BusTiming, Clock, DdcBus, DdcCapabilities,
    DdcError, DdcErrorKind, EddcTarget, Env, Event, NoAcknowledgeSource, Operation, Record,
    SinkConfig, Target, Timestamp, check,
};

const NACK_ADDRESS: DdcErrorKind = DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Address);
const BYTE: u64 = 90;
/// An address nothing on the bench answers: the HDCP port, which it does not model.
const HDCP: u8 = 0x3A;

/// What a [`Device`] saw of one operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seen {
    Write {
        addr: u8,
        len: usize,
        first: Option<u8>,
    },
    Read {
        addr: u8,
        len: usize,
    },
}

/// A register device at `addrs`: writes set the offset with their first byte and store the
/// rest; reads read from the offset, which advances. It keeps what it saw.
struct Device {
    addrs: &'static [u8],
    regs: [u8; 256],
    offset: u8,
    seen: [Option<Seen>; 16],
    transactions: usize,
    fail: Option<DdcErrorKind>,
}

impl Device {
    fn new(addrs: &'static [u8]) -> Self {
        Self {
            addrs,
            regs: core::array::from_fn(|i| i as u8),
            offset: 0,
            seen: [None; 16],
            transactions: 0,
            fail: None,
        }
    }

    fn seen(&self) -> impl Iterator<Item = Seen> + '_ {
        self.seen.iter().flatten().copied()
    }

    fn see(&mut self, seen: Seen) {
        if let Some(slot) = self.seen.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(seen);
        }
    }
}

impl Target for Device {
    fn answers(&self, addr: u8, _env: &Env<'_>) -> bool {
        self.addrs.contains(&addr)
    }

    fn transaction(
        &mut self,
        ops: &mut [Operation<'_>],
        env: &mut Env<'_>,
    ) -> Result<(), DdcErrorKind> {
        self.transactions += 1;
        env.note(ops[0].addr(), 1, self.transactions as u32);
        for op in ops.iter_mut() {
            match op {
                Operation::Write { addr, bytes } => {
                    self.see(Seen::Write {
                        addr: *addr,
                        len: bytes.len(),
                        first: bytes.first().copied(),
                    });
                    if let Some((&offset, data)) = bytes.split_first() {
                        self.offset = offset;
                        for &byte in data {
                            self.regs[usize::from(self.offset)] = byte;
                            self.offset = self.offset.wrapping_add(1);
                        }
                    }
                }
                Operation::Read { addr, buf } => {
                    self.see(Seen::Read {
                        addr: *addr,
                        len: buf.len(),
                    });
                    for byte in buf.iter_mut() {
                        *byte = self.regs[usize::from(self.offset)];
                        self.offset = self.offset.wrapping_add(1);
                    }
                }
                _ => unreachable!("an operation added after this test was written"),
            }
        }
        match self.fail {
            Some(kind) => Err(kind),
            None => Ok(()),
        }
    }
}

fn bus_records<const LOG: usize>(bench: &Bench<'_, LOG>) -> Vec<(Timestamp, BusRecord)> {
    bench
        .transcript()
        .filter_map(|record| match record.event {
            Event::Bus(bus) => Some((record.at, bus)),
            _ => None,
        })
        .collect()
}

fn us(micros: u64) -> Duration {
    Duration::from_micros(micros)
}

#[test]
fn routes_to_the_first_target_that_answers() {
    let mut first = Device::new(&[0x37]);
    let mut second = Device::new(&[0x37, 0x3A]);
    let mut targets: [&mut (dyn Target + Send); 2] = [&mut first, &mut second];
    let bench = Bench::builder(&[]).targets(&mut targets).build().unwrap();
    let bus = bench.ddc();

    bus.write(0x37, &[0x10, 0xAB]).unwrap();
    bus.write(0x3A, &[0x10, 0xCD]).unwrap();

    assert_eq!(first.transactions, 1);
    assert_eq!(first.regs[0x10], 0xAB);
    assert_eq!(second.transactions, 1);
    assert_eq!(second.regs[0x10], 0xCD);
    assert!(second.seen().eq([Seen::Write {
        addr: 0x3A,
        len: 2,
        first: Some(0x10)
    }]));
}

#[test]
fn nothing_answers_an_address_no_target_takes() {
    let mut device = Device::new(&[0x37]);
    let mut targets: [&mut (dyn Target + Send); 1] = [&mut device];
    let bench = Bench::builder(&[]).targets(&mut targets).build().unwrap();
    let mut buf = [0u8; 4];

    let error = bench.ddc().write_read(HDCP, &[0x01], &mut buf).unwrap_err();
    assert_eq!(
        (error.kind, error.op, error.addr),
        (NACK_ADDRESS, BusOp::WriteRead, HDCP)
    );
    assert_eq!(error.kind(), NACK_ADDRESS);
    assert_eq!(buf, [0xA5; 4], "a failed read is filled with the pattern");
    // Only the address byte went out.
    assert_eq!(bench.now(), Timestamp::from_micros(BYTE));
    let records = bus_records(&bench);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].1.result, Err(NACK_ADDRESS));
    assert_eq!(device.transactions, 0);
}

#[test]
fn the_target_that_takes_a_transaction_takes_all_of_it() {
    // 0x50 answers; 0x30 is not this target's, but it is given the whole transaction.
    let mut edid = Device::new(&[EDID]);
    let mut targets: [&mut (dyn Target + Send); 1] = [&mut edid];
    // A sink without a segment pointer: nothing else answers 0x30.
    let bench = Bench::builder(&[])
        .sink(SinkConfig::new().without_segment_pointer())
        .targets(&mut targets)
        .build()
        .unwrap();
    let mut buf = [0u8; 2];
    bench
        .ddc()
        .read_segmented(EddcTarget::Edid, 0, 0x80, &mut buf)
        .unwrap();
    assert_eq!(buf, [0x80, 0x81]);
    // Segment 1: the transaction starts at 0x30, which nothing answers.
    let error = bench
        .ddc()
        .read_segmented(EddcTarget::Edid, 1, 0, &mut buf)
        .unwrap_err();
    assert_eq!(error.kind, NACK_ADDRESS);
    assert_eq!(edid.transactions, 1);
}

#[test]
fn read_segmented_writes_the_pointer_from_segment_1() {
    let mut edid = Device::new(&[SEGMENT_POINTER, EDID]);
    let mut targets: [&mut (dyn Target + Send); 1] = [&mut edid];
    let bench = Bench::builder(&[]).targets(&mut targets).build().unwrap();
    let mut buf = [0u8; 3];
    bench
        .ddc()
        .read_segmented(EddcTarget::Edid, 0, 0x10, &mut buf)
        .unwrap();
    bench
        .ddc()
        .read_segmented(EddcTarget::Edid, 2, 0x20, &mut buf)
        .unwrap();
    assert_eq!(buf, [0x20, 0x21, 0x22]);
    // Segment 0: offset write and read, 2 + 4 bytes; segment 2: pointer too, 2 + 2 + 4.
    assert_eq!(bench.now(), Timestamp::from_micros(BYTE * (6 + 8)));
    let records = bus_records(&bench);
    assert_eq!(records[1].0, Timestamp::from_micros(BYTE * 6));
    let piece = records[1].1;
    assert_eq!(
        piece.op,
        BusOp::ReadSegmented {
            target: EddcTarget::Edid,
            segment: 2
        }
    );
    assert_eq!(
        (
            piece.addr,
            piece.first,
            piece.written,
            piece.read,
            piece.result
        ),
        (EDID, Some(0x20), 2, 3, Ok(()))
    );
    assert!(edid.seen().eq([
        Seen::Write {
            addr: EDID,
            len: 1,
            first: Some(0x10)
        },
        Seen::Read { addr: EDID, len: 3 },
        Seen::Write {
            addr: SEGMENT_POINTER,
            len: 1,
            first: Some(2)
        },
        Seen::Write {
            addr: EDID,
            len: 1,
            first: Some(0x20)
        },
        Seen::Read { addr: EDID, len: 3 },
    ]));
}

#[test]
fn read_segmented_splits_into_pieces() {
    let mut edid = Device::new(&[SEGMENT_POINTER, EDID]);
    let mut targets: [&mut (dyn Target + Send); 1] = [&mut edid];
    let caps = DdcCapabilities::general().with_segmented_piece(Some(32));
    let bench = Bench::builder(&[])
        .targets(&mut targets)
        .ddc_capabilities(caps)
        .build()
        .unwrap();
    let mut buf = [0u8; 80];
    bench
        .ddc()
        .read_segmented(EddcTarget::Edid, 1, 0x10, &mut buf)
        .unwrap();
    assert!(buf.iter().copied().eq(0x10..0x60));
    let pieces: Vec<_> = bus_records(&bench)
        .iter()
        .map(|(at, bus)| (at.as_micros(), bus.first, bus.read))
        .collect();
    // Each piece: pointer (2 bytes), offset (2), the read (1 + n).
    assert_eq!(
        pieces,
        [
            (0, Some(0x10), 32),
            (BYTE * 37, Some(0x30), 32),
            (BYTE * 74, Some(0x50), 16),
        ]
    );
    assert_eq!(edid.transactions, 3);
}

#[test]
fn a_piece_size_of_zero_reads_in_one_transaction() {
    let mut edid = Device::new(&[EDID]);
    let mut targets: [&mut (dyn Target + Send); 1] = [&mut edid];
    let caps = DdcCapabilities::general().with_segmented_piece(Some(0));
    let bench = Bench::builder(&[])
        .targets(&mut targets)
        .ddc_capabilities(caps)
        .build()
        .unwrap();
    let mut buf = [0u8; 64];
    bench
        .ddc()
        .read_segmented(EddcTarget::Edid, 0, 0, &mut buf)
        .unwrap();
    assert_eq!(edid.transactions, 1);
}

#[test]
fn refuses_what_the_arguments_and_capabilities_rule_out() {
    let mut device = Device::new(&[0x37, SCDC, SEGMENT_POINTER, EDID]);
    let mut targets: [&mut (dyn Target + Send); 1] = [&mut device];
    let caps = DdcCapabilities::general()
        .with_write_max(2)
        .with_write_read_max(1, 4)
        .with_read_max(3)
        .with_address_reachable(0x37, false)
        .with_eddc_target(EddcTarget::DisplayId, false)
        .with_max_segment(Some(1));
    let bench = Bench::builder(&[])
        .targets(&mut targets)
        .ddc_capabilities(caps)
        .build()
        .unwrap();
    let bus = bench.ddc();
    let mut buf = [0u8; 8];
    let unsupported = |result: Result<(), BusError>| {
        assert_eq!(result.map_err(|e| e.kind), Err(DdcErrorKind::Unsupported));
    };

    // Arguments: an unusable address, an empty write.
    unsupported(bus.write(0x00, &[1]));
    unsupported(bus.write(SCDC, &[]));
    unsupported(bus.read(0x7F, &mut buf[..1]));
    unsupported(bus.write_read(0x00, &[1], &mut buf[..1]));
    unsupported(bus.read_segmented(EddcTarget::Edid, 0x80, 0, &mut buf[..1]));
    unsupported(bus.read_segmented(EddcTarget::Edid, 0, 0xFF, &mut buf[..2]));
    // Capabilities: an address, lengths, a target, a segment.
    unsupported(bus.write(0x37, &[1]));
    unsupported(bus.write(SCDC, &[1, 2, 3]));
    unsupported(bus.write_read(SCDC, &[1, 2], &mut buf[..1]));
    unsupported(bus.write_read(SCDC, &[1], &mut buf[..5]));
    unsupported(bus.read(SCDC, &mut buf[..4]));
    unsupported(bus.read_segmented(EddcTarget::DisplayId, 0, 0, &mut buf[..1]));
    unsupported(bus.read_segmented(EddcTarget::Edid, 2, 0, &mut buf[..1]));

    // Refused calls take no time and are all recorded.
    assert_eq!(bench.now(), Timestamp::from_micros(0));
    let records = bus_records(&bench);
    assert_eq!(records.len(), 13);
    assert!(
        records
            .iter()
            .all(|(_, bus)| bus.result == Err(DdcErrorKind::Unsupported))
    );
    // And what fits still works.
    bus.write(SCDC, &[1, 2]).unwrap();
    bus.read(SCDC, &mut buf[..3]).unwrap();
    assert_eq!(device.transactions, 2, "refused calls reach no target");
}

#[test]
fn no_segmented_reads_without_a_max_segment() {
    let caps = DdcCapabilities::general().with_max_segment(None);
    let bench = Bench::builder(&[]).ddc_capabilities(caps).build().unwrap();
    let error = bench
        .ddc()
        .read_segmented(EddcTarget::Edid, 0, 0, &mut [0; 1])
        .unwrap_err();
    assert_eq!(error.kind, DdcErrorKind::Unsupported);
}

#[test]
fn empty_buffers_are_ok_and_reach_nothing() {
    let mut device = Device::new(&[SCDC, EDID]);
    let mut targets: [&mut (dyn Target + Send); 1] = [&mut device];
    // Even where the capabilities would refuse the address.
    let caps = DdcCapabilities::general().with_address_reachable(SCDC, false);
    let bench = Bench::builder(&[])
        .targets(&mut targets)
        .ddc_capabilities(caps)
        .build()
        .unwrap();
    let bus = bench.ddc();
    bus.read(SCDC, &mut []).unwrap();
    bus.write_read(SCDC, &[0x01], &mut []).unwrap();
    bus.read_segmented(EddcTarget::Edid, 0x80, 0, &mut [])
        .unwrap();
    assert_eq!(bench.now(), Timestamp::from_micros(0));
    assert_eq!(bus_records(&bench).len(), 3);
    assert_eq!(device.transactions, 0);
}

#[test]
fn write_read_with_nothing_to_write_is_a_read() {
    let mut device = Device::new(&[SCDC]);
    let mut targets: [&mut (dyn Target + Send); 1] = [&mut device];
    let bench = Bench::builder(&[]).targets(&mut targets).build().unwrap();
    let mut buf = [0u8; 2];
    bench.ddc().write_read(SCDC, &[], &mut buf).unwrap();
    assert_eq!(buf, [0, 1]);
    assert_eq!(bus_records(&bench)[0].1.op, BusOp::Read);
    assert!(device.seen().eq([Seen::Read { addr: SCDC, len: 2 }]));
}

#[test]
fn timing_charges_every_byte_after_the_transaction() {
    let mut device = Device::new(&[SCDC]);
    let mut targets: [&mut (dyn Target + Send); 1] = [&mut device];
    let timing = BusTiming::STANDARD_MODE
        .with_per_byte(us(10))
        .with_per_transaction(us(1_000));
    let bench = Bench::builder(&[])
        .targets(&mut targets)
        .bus_timing(timing)
        .build()
        .unwrap();
    let bus = bench.ddc();
    bus.write(SCDC, &[0x20, 0x03]).unwrap(); // 3 bytes
    assert_eq!(bench.now(), Timestamp::from_micros(1_030));
    bus.write_read(SCDC, &[0x21], &mut [0; 2]).unwrap(); // 2 + 3 bytes
    assert_eq!(bench.now(), Timestamp::from_micros(2_080));
    // A record's time is the transaction's start.
    let starts: Vec<_> = bus_records(&bench)
        .iter()
        .map(|(at, _)| at.as_micros())
        .collect();
    assert_eq!(starts, [0, 1_030]);
    assert_eq!(BusTiming::default(), BusTiming::STANDARD_MODE);
    assert_eq!(BusTiming::STANDARD_MODE.per_byte, us(90));
}

#[test]
fn a_target_error_is_the_calls_and_what_it_applied_stays() {
    let mut device = Device::new(&[SCDC]);
    device.fail = Some(DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Data));
    let mut targets: [&mut (dyn Target + Send); 1] = [&mut device];
    let bench = Bench::builder(&[]).targets(&mut targets).build().unwrap();
    let bus = bench.ddc();
    let error = bus.write(SCDC, &[0x20, 0x03]).unwrap_err();
    assert_eq!(
        error.kind,
        DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Data)
    );
    let mut buf = [0u8; 3];
    bus.read(SCDC, &mut buf).unwrap_err();
    assert_eq!(buf, [0xA5; 3]);
    // A transaction a target answered is charged in full, even when it failed.
    assert_eq!(bench.now(), Timestamp::from_micros(BYTE * (3 + 4)));
    let records = bus_records(&bench);
    assert!(
        records
            .iter()
            .all(|(_, bus)| bus.result
                == Err(DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Data)))
    );
    assert_eq!(device.regs[0x20], 0x03);
}

#[test]
fn a_targets_notes_follow_its_bus_record() {
    let mut device = Device::new(&[SCDC]);
    let mut targets: [&mut (dyn Target + Send); 1] = [&mut device];
    let bench = Bench::builder(&[]).targets(&mut targets).build().unwrap();
    bench.advance(us(500));
    bench.ddc().write(SCDC, &[0x02, 0x01]).unwrap();
    let records: Vec<Record> = bench.transcript().collect();
    assert_eq!(records.len(), 2);
    assert!(matches!(records[0].event, Event::Bus(_)));
    assert_eq!(
        records[1].event,
        Event::Note {
            addr: SCDC,
            code: 1,
            value: 1
        }
    );
    assert_eq!(records[1].at, Timestamp::from_micros(500));
}

#[test]
fn env_shows_the_start_time_and_hpd() {
    struct Probe(Option<(Timestamp, bool)>);
    impl Target for Probe {
        fn answers(&self, addr: u8, env: &Env<'_>) -> bool {
            addr == 0x37 && env.hpd()
        }
        fn transaction(
            &mut self,
            _ops: &mut [Operation<'_>],
            env: &mut Env<'_>,
        ) -> Result<(), DdcErrorKind> {
            self.0 = Some((env.now(), env.hpd()));
            assert!(format!("{env:?}").starts_with("Env { now: Timestamp(250), hpd: true"));
            Ok(())
        }
    }
    let mut probe = Probe(None);
    let mut targets: [&mut (dyn Target + Send); 1] = [&mut probe];
    let bench = Bench::builder(&[]).targets(&mut targets).build().unwrap();
    bench.advance(us(250));
    bench.ddc().write(0x37, &[0]).unwrap();
    assert_eq!(probe.0, Some((Timestamp::from_micros(250), true)));
}

#[test]
fn advance_lets_every_target_see_the_new_time() {
    /// Notes every update it is given.
    struct Watcher(u8);
    impl Target for Watcher {
        fn answers(&self, _addr: u8, _env: &Env<'_>) -> bool {
            false
        }
        fn transaction(
            &mut self,
            _ops: &mut [Operation<'_>],
            _env: &mut Env<'_>,
        ) -> Result<(), DdcErrorKind> {
            unreachable!("it answers nothing")
        }
        fn update(&mut self, env: &mut Env<'_>) {
            env.note(self.0, 2, env.now().as_micros() as u32);
        }
    }
    let mut first = Watcher(1);
    let mut second = Watcher(2);
    // A target that keeps the default update, which does nothing.
    let mut quiet = Device::new(&[SCDC]);
    let mut targets: [&mut (dyn Target + Send); 3] = [&mut first, &mut quiet, &mut second];
    let bench = Bench::builder(&[]).targets(&mut targets).build().unwrap();
    bench.advance(us(40));
    let notes: Vec<_> = bench
        .transcript()
        .map(|record| (record.at.as_micros(), record.event))
        .collect();
    assert_eq!(
        notes,
        [
            (
                40,
                Event::Note {
                    addr: 1,
                    code: 2,
                    value: 40
                }
            ),
            (
                40,
                Event::Note {
                    addr: 2,
                    code: 2,
                    value: 40
                }
            ),
        ]
    );
}

#[test]
fn the_bench_runs_on_the_clock_it_is_given() {
    let clock = maquette::SimClock::new().with_tick(us(1_000));
    clock.advance(us(2_500));
    let bench = Bench::builder(&[]).clock(clock).build().unwrap();
    assert_eq!(bench.now(), Timestamp::from_micros(2_000));
    // A refused-by-nobody write charges its address byte on that clock.
    bench.ddc().write(0x37, &[0]).unwrap_err();
    assert_eq!(bench.now(), Timestamp::from_micros(2_000));
    bench.advance(us(410));
    assert_eq!(bench.now(), Timestamp::from_micros(3_000));
}

#[test]
fn the_transcript_counts_what_does_not_fit_and_clears() {
    let bench = Bench::builder(&[]).capacities::<2, 1>().build().unwrap();
    let bus = bench.ddc();
    for _ in 0..5 {
        bus.write(0x37, &[0]).unwrap_err();
    }
    assert_eq!(bench.transcript().count(), 2);
    assert_eq!(bench.transcript_overflow(), 3);
    bench.clear_transcript();
    assert_eq!(bench.transcript().count(), 0);
    assert_eq!(bench.transcript_overflow(), 0);
}

#[test]
fn the_edid_store_is_whole_blocks_up_to_256() {
    let ok = [0u8; 256 * 128];
    assert!(Bench::new(SinkConfig::new(), &ok).is_ok());
    assert!(Bench::new(SinkConfig::new(), &[]).is_ok());
    for len in [1, 127, 129, 256 * 128 + 128] {
        let edid = vec![0u8; len];
        assert_eq!(
            Bench::new(SinkConfig::default(), &edid).unwrap_err(),
            BenchError::EdidLength(len)
        );
    }
    assert_eq!(
        BenchError::EdidLength(5).to_string(),
        "an EDID store of 5 bytes: a store is a whole number of 128-byte blocks, at most 256"
    );
    assert_eq!(
        BenchError::DisplayIdLength(7).to_string(),
        "an DisplayID store of 7 bytes: a store is a whole number of 128-byte blocks, at most 256"
    );
}

#[test]
fn scdc_over_the_bus_is_the_adapter_real_backends_use() {
    let mut scdc = Device::new(&[SCDC]);
    let mut targets: [&mut (dyn Target + Send); 1] = [&mut scdc];
    let bench = Bench::builder(&[]).targets(&mut targets).build().unwrap();
    let bus = bench.ddc();
    let mut transport = ScdcOverDdc::new(&bus).expect("the bus serves SCDC");
    transport.write(0x20, 0x03).unwrap();
    assert_eq!(transport.read(0x20), Ok(0x03));
    let mut block = [0u8; 4];
    transport.read_block(0x10, &mut block).unwrap();
    assert_eq!(block, [0x10, 0x11, 0x12, 0x13]);
}

#[test]
fn the_time_handle_passes_the_clock_check() {
    let bench = Bench::new(SinkConfig::new(), &[]).unwrap();
    let mut time = bench.time();
    check::clock(&bench.time(), &mut time).assert_ok();
    // A delay on the handle moves the bench's time.
    let before = bench.now();
    maquette::Delay::delay(&mut time, us(10));
    assert_eq!(bench.now(), Timestamp::from_micros(before.as_micros() + 10));
    assert_eq!(time.now(), bench.now());
}

#[test]
fn the_bench_and_its_handles_are_send_and_sync() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Bench<'_>>();
    send_sync::<maquette::Ddc<'_>>();
    send_sync::<maquette::Time<'_>>();
    fn copy<T: Copy>() {}
    copy::<maquette::Ddc<'_>>();
    copy::<maquette::Time<'_>>();
}

#[test]
fn threads_share_the_bus() {
    let mut device = Device::new(&[SCDC]);
    let mut targets: [&mut (dyn Target + Send); 1] = [&mut device];
    let bench = Bench::builder(&[]).targets(&mut targets).build().unwrap();
    std::thread::scope(|s| {
        for _ in 0..4 {
            let bus = bench.ddc();
            s.spawn(move || {
                for _ in 0..25 {
                    bus.write(SCDC, &[0x30, 1]).unwrap();
                }
            });
        }
    });
    assert_eq!(bus_records(&bench).len(), 100);
    assert_eq!(device.transactions, 100);
}

#[test]
fn debug_output_names_the_parts() {
    let builder = Bench::builder(&[0; 128]);
    assert!(format!("{builder:?}").starts_with("BenchBuilder { edid_len: 128"));
    let bench = builder.build().unwrap();
    let text = format!("{bench:?}");
    assert!(text.starts_with("Bench { now: Timestamp(0)"), "{text}");
    assert!(text.contains("edid_len: 128"));
    assert!(format!("{:?}", bench.ddc()).starts_with("Ddc { capabilities: "));
    assert!(format!("{:?}", bench.time()).starts_with("Time { clock: SimClock"));
    let mut device = Device::new(&[0x37]);
    device.fail = Some(NACK_ADDRESS);
    let mut targets: [&mut (dyn Target + Send); 1] = [&mut device];
    let bench = Bench::builder(&[]).targets(&mut targets).build().unwrap();
    let error = bench.ddc().write(0x37, &[0]).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Write at 0x37 failed: NoAcknowledge(Address)"
    );
}
