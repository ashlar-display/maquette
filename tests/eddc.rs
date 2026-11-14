//! The built-in E-DDC target: the EDID and DisplayID behind the segment pointer, and its
//! settings.

use maquette::hdmi_hal::ddc::{DISPLAYID, EDID, SEGMENT_POINTER};
use maquette::{
    Bench, BenchError, BlockBoundary, DdcBus, DdcCapabilities, DdcErrorKind, EddcTarget, Event,
    Ignored, MissingSegment, NoAcknowledgeSource, PastStore, SinkConfig,
};

const NACK_DATA: DdcErrorKind = DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Data);
const NACK_ADDRESS: DdcErrorKind = DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Address);

/// A store whose every byte names its place: the low byte of its index, plus its segment in
/// the high blocks, so a byte read from the wrong segment shows.
fn store<const N: usize>() -> [u8; N] {
    core::array::from_fn(|i| (i as u8) ^ ((i / 256) as u8).wrapping_mul(0x40))
}

fn bench(config: SinkConfig, edid: &[u8]) -> Bench<'_> {
    Bench::new(config, edid).unwrap()
}

#[test]
fn segment_0_reads_the_first_two_blocks() {
    let edid: [u8; 256] = store();
    let bench = bench(SinkConfig::new(), &edid);
    let mut buf = [0u8; 256];
    bench
        .ddc()
        .read_segmented(EddcTarget::Edid, 0, 0, &mut buf)
        .unwrap();
    assert_eq!(buf, edid);
}

#[test]
fn higher_segments_read_through_the_pointer() {
    let edid: [u8; 1024] = store();
    let bench = bench(SinkConfig::new(), &edid);
    for segment in 0..4u8 {
        let mut buf = [0u8; 128];
        bench
            .ddc()
            .read_segmented(EddcTarget::Edid, segment, 0x80, &mut buf)
            .unwrap();
        let start = usize::from(segment) * 256 + 0x80;
        assert_eq!(buf[..], edid[start..start + 128], "segment {segment}");
    }
}

#[test]
fn split_reads_write_the_pointer_for_every_piece() {
    let edid: [u8; 512] = store();
    let caps = DdcCapabilities::general().with_segmented_piece(Some(32));
    let bench = Bench::builder(&edid)
        .ddc_capabilities(caps)
        .build()
        .unwrap();
    let mut buf = [0u8; 256];
    bench
        .ddc()
        .read_segmented(EddcTarget::Edid, 1, 0, &mut buf)
        .unwrap();
    assert_eq!(buf[..], edid[256..]);
    assert_eq!(bench.transcript().count(), 8);
}

#[test]
fn the_word_offset_persists_for_a_current_address_read() {
    let edid: [u8; 256] = store();
    let bench = bench(SinkConfig::new(), &edid);
    let bus = bench.ddc();
    let mut buf = [0u8; 4];
    bus.write_read(EDID, &[0x10], &mut buf).unwrap();
    assert_eq!(buf[..], edid[0x10..0x14]);
    // No offset written: the read continues where the last one stopped.
    bus.read(EDID, &mut buf).unwrap();
    assert_eq!(buf[..], edid[0x14..0x18]);
    // An offset alone sets it.
    bus.write(EDID, &[0x80]).unwrap();
    bus.read(EDID, &mut buf).unwrap();
    assert_eq!(buf[..], edid[0x80..0x84]);
}

#[test]
fn the_pointer_does_not_outlive_its_transaction() {
    let edid: [u8; 512] = store();
    let bench = bench(SinkConfig::new(), &edid);
    let bus = bench.ddc();
    // The pointer written alone ends in STOP, which resets it.
    bus.write(SEGMENT_POINTER, &[1]).unwrap();
    let mut buf = [0u8; 2];
    bus.write_read(EDID, &[0x00], &mut buf).unwrap();
    assert_eq!(buf[..], edid[..2]);
    // And after a segmented read too.
    bus.read_segmented(EddcTarget::Edid, 1, 0, &mut buf)
        .unwrap();
    assert_eq!(buf[..], edid[256..258]);
    bus.write_read(EDID, &[0x00], &mut buf).unwrap();
    assert_eq!(buf[..], edid[..2]);
}

#[test]
fn the_pointer_cannot_be_read() {
    let bench = bench(SinkConfig::new(), &[0; 128]);
    let error = bench.ddc().read(SEGMENT_POINTER, &mut [0]).unwrap_err();
    assert_eq!(error.kind, NACK_ADDRESS);
}

#[test]
fn a_sink_without_a_pointer_answers_segment_0_only() {
    let edid: [u8; 512] = store();
    let config = SinkConfig::new().without_segment_pointer();
    assert!(!config.has_segment_pointer());
    let bench = bench(config, &edid);
    let bus = bench.ddc();
    assert_eq!(
        bus.write(SEGMENT_POINTER, &[0]).unwrap_err().kind,
        NACK_ADDRESS
    );
    let mut buf = [0u8; 4];
    bus.read_segmented(EddcTarget::Edid, 0, 0, &mut buf)
        .unwrap();
    assert_eq!(buf[..], edid[..4]);
    let error = bus
        .read_segmented(EddcTarget::Edid, 1, 0, &mut buf)
        .unwrap_err();
    assert_eq!(error.kind, NACK_ADDRESS);
    assert_eq!(buf, [0xA5; 4]);
}

#[test]
fn a_segment_the_sink_does_not_have_is_refused_at_the_pointer() {
    let edid: [u8; 256] = store();
    let bench = bench(SinkConfig::new(), &edid);
    let mut buf = [0u8; 4];
    let error = bench
        .ddc()
        .read_segmented(EddcTarget::Edid, 1, 0, &mut buf)
        .unwrap_err();
    assert_eq!(error.kind, NACK_DATA);
}

#[test]
fn a_missing_segment_can_be_acknowledged_and_read_past_the_store() {
    let edid: [u8; 256] = store();
    let config = SinkConfig::new().with_missing_segment(MissingSegment::Ack);
    assert_eq!(config.missing_segment(), MissingSegment::Ack);
    let bench = bench(config, &edid);
    let mut buf = [0u8; 4];
    bench
        .ddc()
        .read_segmented(EddcTarget::Edid, 3, 0, &mut buf)
        .unwrap();
    assert_eq!(buf, [0xFF; 4]);
}

#[test]
fn a_segment_counts_as_present_when_either_store_has_it() {
    let edid: [u8; 128] = store();
    let displayid: [u8; 512] = store();
    let bench = Bench::builder(&edid).displayid(&displayid).build().unwrap();
    let mut buf = [0u8; 2];
    // The DisplayID has segment 1, so the pointer is acknowledged; the EDID reads past.
    bench
        .ddc()
        .read_segmented(EddcTarget::Edid, 1, 0, &mut buf)
        .unwrap();
    assert_eq!(buf, [0xFF; 2]);
    bench
        .ddc()
        .read_segmented(EddcTarget::DisplayId, 1, 0, &mut buf)
        .unwrap();
    assert_eq!(buf[..], displayid[256..258]);
}

#[test]
fn reads_past_the_store_fill_with_0xff_by_default() {
    let edid: [u8; 128] = store();
    let bench = bench(SinkConfig::new(), &edid);
    let mut buf = [0u8; 4];
    bench
        .ddc()
        .read_segmented(EddcTarget::Edid, 0, 0x7E, &mut buf)
        .unwrap();
    assert_eq!(buf, [edid[0x7E], edid[0x7F], 0xFF, 0xFF]);
}

#[test]
fn reads_past_the_store_can_wrap_or_be_refused() {
    let edid: [u8; 128] = store();
    let config = SinkConfig::new().with_past_store(PastStore::Wrap);
    assert_eq!(config.past_store(), PastStore::Wrap);
    let bench = bench(config, &edid);
    let mut buf = [0u8; 4];
    bench
        .ddc()
        .read_segmented(EddcTarget::Edid, 0, 0x7E, &mut buf)
        .unwrap();
    assert_eq!(buf, [edid[0x7E], edid[0x7F], edid[0], edid[1]]);

    let bench = Bench::new(SinkConfig::new().with_past_store(PastStore::Nack), &edid).unwrap();
    let error = bench
        .ddc()
        .read_segmented(EddcTarget::Edid, 0, 0x7E, &mut buf)
        .unwrap_err();
    assert_eq!(error.kind, NACK_DATA);
    // The bytes before the store's end were read; the offset stopped at the end.
    let mut next = [0u8; 1];
    let error = bench.ddc().read(EDID, &mut next).unwrap_err();
    assert_eq!(error.kind, NACK_DATA);
}

#[test]
fn reads_continue_across_the_block_boundary_by_default() {
    let edid: [u8; 256] = store();
    let bench = bench(SinkConfig::new(), &edid);
    let mut buf = [0u8; 4];
    bench.ddc().write_read(EDID, &[0x7E], &mut buf).unwrap();
    assert_eq!(buf[..], edid[0x7E..0x82]);
    // Past 0xFF, the offset wraps within the segment.
    bench.ddc().write_read(EDID, &[0xFE], &mut buf).unwrap();
    assert_eq!(buf, [edid[0xFE], edid[0xFF], edid[0], edid[1]]);
}

#[test]
fn reads_can_wrap_at_the_block_boundary() {
    let edid: [u8; 256] = store();
    let config = SinkConfig::new().with_block_boundary(BlockBoundary::Wrap);
    assert_eq!(config.block_boundary(), BlockBoundary::Wrap);
    let bench = bench(config, &edid);
    let mut buf = [0u8; 4];
    bench.ddc().write_read(EDID, &[0x7E], &mut buf).unwrap();
    assert_eq!(buf, [edid[0x7E], edid[0x7F], edid[0], edid[1]]);
    bench.ddc().write_read(EDID, &[0xFE], &mut buf).unwrap();
    assert_eq!(buf, [edid[0xFE], edid[0xFF], edid[0x80], edid[0x81]]);
}

#[test]
fn writes_after_the_offset_are_ignored_and_recorded() {
    let edid: [u8; 128] = store();
    let bench = bench(SinkConfig::new(), &edid);
    bench.ddc().write(EDID, &[0x20, 0xAA, 0xBB]).unwrap();
    let ignored: Vec<_> = bench
        .transcript()
        .filter_map(|record| match record.event {
            Event::IgnoredWrite {
                addr,
                reg,
                value,
                why,
            } => Some((addr, reg, value, why)),
            _ => None,
        })
        .collect();
    assert_eq!(
        ignored,
        [
            (EDID, 0x20, 0xAA, Ignored::ReadOnly),
            (EDID, 0x20, 0xBB, Ignored::ReadOnly),
        ]
    );
    // The offset is the one written; the ignored bytes did not move it.
    let mut buf = [0u8; 1];
    bench.ddc().read(EDID, &mut buf).unwrap();
    assert_eq!(buf, [edid[0x20]]);
}

#[test]
fn an_empty_edid_does_not_answer() {
    let bench = bench(SinkConfig::new(), &[]);
    let error = bench
        .ddc()
        .read_segmented(EddcTarget::Edid, 0, 0, &mut [0; 8])
        .unwrap_err();
    assert_eq!(error.kind, NACK_ADDRESS);
    assert_eq!(
        bench.ddc().read(DISPLAYID, &mut [0]).unwrap_err().kind,
        NACK_ADDRESS,
        "no DisplayID store, nothing at 0x52"
    );
}

#[test]
fn the_displayid_has_its_own_store_and_offset() {
    let edid: [u8; 256] = store();
    let displayid = [0x5Au8; 128];
    let bench = Bench::builder(&edid).displayid(&displayid).build().unwrap();
    let bus = bench.ddc();
    let mut buf = [0u8; 2];
    bus.write_read(EDID, &[0x40], &mut buf).unwrap();
    bus.read_segmented(EddcTarget::DisplayId, 0, 0x10, &mut buf)
        .unwrap();
    assert_eq!(buf, [0x5A; 2]);
    // The EDID's offset was not moved by the DisplayID read.
    bus.read(EDID, &mut buf).unwrap();
    assert_eq!(buf[..], edid[0x42..0x44]);
}

#[test]
fn the_displayid_store_is_whole_blocks_up_to_256() {
    let edid = [0u8; 128];
    for len in [1, 129, 256 * 128 + 128] {
        let displayid = vec![0u8; len];
        let error = Bench::builder(&edid)
            .displayid(&displayid)
            .build()
            .unwrap_err();
        assert_eq!(error, BenchError::DisplayIdLength(len));
    }
    // The EDID is checked first.
    let error = Bench::builder(&[0; 5])
        .displayid(&[0; 3])
        .build()
        .unwrap_err();
    assert_eq!(error, BenchError::EdidLength(5));
    let bench = Bench::builder(&edid).displayid(&[]).build().unwrap();
    assert!(format!("{bench:?}").contains("displayid_len: Some(0)"));
}
