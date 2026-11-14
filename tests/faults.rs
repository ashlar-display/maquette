//! Faults: the schedule, what counts, the points, the effects and their records.

use core::time::Duration;

use maquette::hdmi_hal::ddc::{EDID, SCDC};
use maquette::{
    Bench, BusOp, DdcBus, DdcCapabilities, DdcErrorKind, EddcTarget, Effect, Event, Fault, Filter,
    Line, NoAcknowledgeSource, OpKind, Point, ScheduleFull, SinkConfig, Times, TmdsClockRatio,
};

const NACK_ADDRESS: DdcErrorKind = DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Address);
const NACK_DATA: DdcErrorKind = DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Data);
const BYTE: u64 = 90;

fn bench() -> Bench<'static> {
    Bench::new(SinkConfig::new(), &EDID_STORE).unwrap()
}

static EDID_STORE: [u8; 256] = {
    let mut store = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        store[i] = i as u8;
        i += 1;
    }
    store
};

fn fail(kind: DdcErrorKind) -> Fault {
    Fault::new(Effect::Fail(kind))
}

/// The bus records' results, in order.
fn results(bench: &Bench<'_>) -> Vec<Result<(), DdcErrorKind>> {
    bench
        .transcript()
        .filter_map(|record| match record.event {
            Event::Bus(bus) => Some(bus.result),
            _ => None,
        })
        .collect()
}

#[test]
fn before_the_sink_sees_it_nothing_is_applied() {
    let bench = bench();
    let id = bench.schedule(fail(NACK_ADDRESS)).unwrap();
    let error = bench.ddc().write(SCDC, &[0x20, 0x03]).unwrap_err();
    assert_eq!(error.kind, NACK_ADDRESS);
    assert_eq!(error.op, BusOp::Write);
    assert_eq!(bench.sink().tmds_config(), Some(0));
    // Only the address byte went out.
    assert_eq!(bench.now().as_micros(), BYTE);
    let events: Vec<_> = bench.transcript().map(|record| record.event).collect();
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], Event::Bus(bus) if bus.result == Err(NACK_ADDRESS)));
    assert_eq!(
        events[1],
        Event::Fault {
            id,
            effect: Effect::Fail(NACK_ADDRESS),
            point: Point::Before
        }
    );
    // It struck once and left the schedule.
    assert_eq!(bench.scheduled_faults(), 0);
    bench.ddc().write(SCDC, &[0x20, 0x03]).unwrap();
}

#[test]
fn after_the_sink_applied_it_the_write_is_latched() {
    let bench = bench();
    bench
        .schedule(fail(DdcErrorKind::Busy).at(Point::After))
        .unwrap();
    let error = bench.ddc().write(SCDC, &[0x20, 0x03]).unwrap_err();
    assert_eq!(error.kind, DdcErrorKind::Busy);
    assert_eq!(bench.sink().tmds_config(), Some(0x03));
    assert_eq!(bench.now().as_micros(), 3 * BYTE);
    assert_eq!(results(&bench), [Err(DdcErrorKind::Busy)]);
}

#[test]
fn partway_the_bytes_before_it_go_through() {
    let bench = bench();
    bench
        .schedule(fail(NACK_DATA).at(Point::Partway { byte: 2 }))
        .unwrap();
    // Offset 0x20, TMDS_Config 0x03, then 0x21 refused.
    let error = bench.ddc().write(SCDC, &[0x20, 0x03, 0x01]).unwrap_err();
    assert_eq!(error.kind, NACK_DATA);
    assert_eq!(bench.sink().tmds_config(), Some(0x03));
    // The address byte and two data bytes went out.
    assert_eq!(bench.now().as_micros(), 3 * BYTE);
}

#[test]
fn partway_through_a_read_the_whole_buffer_is_the_pattern() {
    let bench = bench();
    bench
        .schedule(fail(NACK_DATA).at(Point::Partway { byte: 3 }))
        .unwrap();
    let mut buf = [0u8; 4];
    // One byte written (the offset), then the read cut after two bytes.
    let error = bench.ddc().write_read(EDID, &[0x10], &mut buf).unwrap_err();
    assert_eq!(error.kind, NACK_DATA);
    assert_eq!(buf, [0xA5; 4]);
    // The sink's offset advanced past the two bytes it sent.
    bench.ddc().read(EDID, &mut buf[..1]).unwrap();
    assert_eq!(buf[0], 0x12);
}

#[test]
fn partway_past_the_last_byte_strikes_after() {
    let bench = bench();
    bench
        .schedule(fail(NACK_DATA).at(Point::Partway { byte: 2 }))
        .unwrap();
    let error = bench.ddc().write(SCDC, &[0x20, 0x03]).unwrap_err();
    assert_eq!(error.kind, NACK_DATA);
    assert_eq!(bench.sink().tmds_config(), Some(0x03));
}

#[test]
fn nth_and_count_choose_the_operations_struck() {
    let bench = bench();
    bench
        .schedule(fail(DdcErrorKind::Busy).nth(2).times(Times::Count(2)))
        .unwrap();
    let bus = bench.ddc();
    for _ in 0..4 {
        let _ = bus.write(SCDC, &[0x02, 0x01]);
    }
    assert_eq!(
        results(&bench),
        [
            Ok(()),
            Err(DdcErrorKind::Busy),
            Err(DdcErrorKind::Busy),
            Ok(())
        ]
    );
    assert_eq!(bench.scheduled_faults(), 0);
}

#[test]
fn a_fault_until_cleared_strikes_until_it_is() {
    let bench = bench();
    let id = bench
        .schedule(fail(DdcErrorKind::Timeout).times(Times::UntilCleared))
        .unwrap();
    for _ in 0..3 {
        bench.ddc().write(SCDC, &[0x02, 0x01]).unwrap_err();
    }
    assert!(bench.clear_fault(id));
    assert!(!bench.clear_fault(id));
    bench.ddc().write(SCDC, &[0x02, 0x01]).unwrap();
}

#[test]
fn filters_count_only_what_they_match() {
    let bench = bench();
    let filter = Filter::ANY
        .with_addr(SCDC)
        .with_op(OpKind::WriteRead)
        .with_first(0x21);
    assert_eq!(filter.addr, Some(SCDC));
    bench
        .schedule(fail(DdcErrorKind::Busy).on(filter).nth(2))
        .unwrap();
    let bus = bench.ddc();
    let mut status = [0];
    bus.write_read(SCDC, &[0x21], &mut status).unwrap(); // 1st match
    bus.write_read(SCDC, &[0x20], &mut status).unwrap(); // another register
    bus.write(SCDC, &[0x21, 0]).unwrap(); // a write
    bus.write_read(EDID, &[0x21], &mut status).unwrap(); // another address
    assert_eq!(
        bus.write_read(SCDC, &[0x21], &mut status).unwrap_err().kind,
        DdcErrorKind::Busy
    );
}

#[test]
fn refused_empty_and_detached_calls_do_not_count_but_unplugged_ones_do() {
    let bench = bench();
    bench
        .schedule(fail(DdcErrorKind::Busy).nth(2).times(Times::UntilCleared))
        .unwrap();
    let bus = bench.ddc();
    let _ = bus.write(SCDC, &[]); // refused
    bus.read(SCDC, &mut []).unwrap(); // empty
    bench.detach();
    assert_eq!(
        bus.write(SCDC, &[0x02, 1]).unwrap_err().kind,
        DdcErrorKind::Detached
    );
    bench.reattach();
    bench.unplug();
    assert_eq!(bus.write(SCDC, &[0x02, 1]).unwrap_err().kind, NACK_ADDRESS); // 1st
    assert_eq!(
        bus.write(SCDC, &[0x02, 1]).unwrap_err().kind,
        DdcErrorKind::Busy
    ); // 2nd
}

#[test]
fn each_piece_of_a_split_read_is_an_operation() {
    let caps = DdcCapabilities::general().with_segmented_piece(Some(32));
    let bench = Bench::builder(&EDID_STORE)
        .ddc_capabilities(caps)
        .build()
        .unwrap();
    bench
        .schedule(
            fail(NACK_DATA)
                .on(Filter::ANY.with_op(OpKind::ReadSegmented))
                .nth(2),
        )
        .unwrap();
    let mut buf = [0u8; 128];
    let error = bench
        .ddc()
        .read_segmented(EddcTarget::Edid, 0, 0, &mut buf)
        .unwrap_err();
    assert_eq!(error.kind, NACK_DATA);
    assert_eq!(buf, [0xA5; 128]);
    // The first piece read, the second failed, the rest were not read.
    assert_eq!(results(&bench), [Ok(()), Err(NACK_DATA)]);
}

#[test]
fn the_first_scheduled_fault_strikes_and_every_fault_counts() {
    let bench = bench();
    let first = bench.schedule(fail(DdcErrorKind::Busy)).unwrap();
    let second = bench.schedule(fail(DdcErrorKind::Timeout)).unwrap();
    assert_ne!(first, second);
    assert_eq!(
        bench.ddc().write(SCDC, &[0x02, 1]).unwrap_err().kind,
        DdcErrorKind::Busy
    );
    // The second counted that operation as its first, its only one: it has passed.
    assert_eq!(bench.scheduled_faults(), 0);
    bench.ddc().write(SCDC, &[0x02, 1]).unwrap();
}

#[test]
fn a_full_schedule_refuses() {
    let bench = Bench::builder(&[]).capacities::<16, 2>().build().unwrap();
    let fault = fail(DdcErrorKind::Busy).times(Times::UntilCleared);
    bench.schedule(fault).unwrap();
    bench.schedule(fault).unwrap();
    assert_eq!(bench.schedule(fault), Err(ScheduleFull));
    assert_eq!(
        ScheduleFull.to_string(),
        "the fault schedule is full; clear a fault or give the bench more"
    );
    bench.clear_faults();
    assert_eq!(bench.scheduled_faults(), 0);
    bench.schedule(fault).unwrap();
}

#[test]
fn a_fault_that_never_strikes_leaves_once_counted() {
    let bench = bench();
    bench
        .schedule(fail(DdcErrorKind::Busy).times(Times::Count(0)))
        .unwrap();
    bench.ddc().write(SCDC, &[0x02, 1]).unwrap();
    assert_eq!(bench.scheduled_faults(), 0);
    // nth 0 is taken as 1.
    bench.schedule(fail(DdcErrorKind::Busy).nth(0)).unwrap();
    assert!(bench.ddc().write(SCDC, &[0x02, 1]).is_err());
}

#[test]
fn a_timeout_charges_the_stretch_timeout() {
    let caps = DdcCapabilities::general().with_stretch_timeout_us(Some(2_000));
    let bench = Bench::builder(&[]).ddc_capabilities(caps).build().unwrap();
    bench.schedule(fail(DdcErrorKind::Timeout)).unwrap();
    bench.ddc().write(SCDC, &[0x02, 1]).unwrap_err();
    assert_eq!(bench.now().as_micros(), BYTE + 2_000);
    // Without a stretch timeout in the capabilities, nothing extra.
    let bench = self::bench();
    bench.schedule(fail(DdcErrorKind::Timeout)).unwrap();
    bench.ddc().write(SCDC, &[0x02, 1]).unwrap_err();
    assert_eq!(bench.now().as_micros(), BYTE);
}

#[test]
fn a_slow_operation_succeeds_late() {
    let bench = bench();
    bench
        .schedule(Fault::new(Effect::Slow(Duration::from_millis(5))))
        .unwrap();
    bench.ddc().write(SCDC, &[0x20, 0x03]).unwrap();
    assert_eq!(bench.sink().tmds_config(), Some(0x03));
    assert_eq!(bench.now().as_micros(), 3 * BYTE + 5_000);
    assert!(bench.transcript().any(|record| matches!(
        record.event,
        Event::Fault {
            effect: Effect::Slow(_),
            ..
        }
    )));
}

#[test]
fn detached_and_unsupported_faults_fail_one_operation() {
    let bench = bench();
    bench.schedule(fail(DdcErrorKind::Detached)).unwrap();
    // Every fault counts every operation: the second strikes the second.
    bench
        .schedule(fail(DdcErrorKind::Unsupported).nth(2))
        .unwrap();
    let bus = bench.ddc();
    assert_eq!(
        bus.write(SCDC, &[0x02, 1]).unwrap_err().kind,
        DdcErrorKind::Detached
    );
    assert!(bench.is_attached());
    assert_eq!(
        bus.write(SCDC, &[0x02, 1]).unwrap_err().kind,
        DdcErrorKind::Unsupported
    );
    bus.write(SCDC, &[0x02, 1]).unwrap();
}

#[test]
fn a_preset_is_an_event_the_lock_answers() {
    let bench = bench();
    bench.ddc().write(SCDC, &[0x20, 0x03]).unwrap();
    bench.set_line(Line::Tmds {
        character_rate_khz: 594_000,
        ratio: TmdsClockRatio::Div40,
        scrambled: true,
    });
    bench.advance(Duration::from_millis(1));
    assert_eq!(bench.sink().scrambling_status(), Some(1));
    bench.preset_scdc(0x20, 0x00);
    assert_eq!(bench.sink().scrambling_status(), Some(0));
}
