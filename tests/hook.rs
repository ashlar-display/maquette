//! The SCDC hook: a test's quirk on top of the built-in SCDC sink.

use core::time::Duration;

use maquette::hdmi_hal::ddc::SCDC;
use maquette::{
    Actor, Bench, DdcBus, DdcErrorKind, Env, Event, HookWrite, Ignored, Line, NoAcknowledgeSource,
    ScdcHook, ScdcRegisters, SinkConfig, TmdsClockRatio,
};

/// A hook that does `on_write` to writes of `reg`, and lets every other byte through.
struct Quirk {
    reg: u8,
    on_write: fn(u8, &ScdcRegisters<'_>) -> HookWrite,
}

impl ScdcHook for Quirk {
    fn write(
        &mut self,
        reg: u8,
        value: u8,
        regs: &ScdcRegisters<'_>,
        _: &mut Env<'_>,
    ) -> HookWrite {
        if reg == self.reg {
            (self.on_write)(value, regs)
        } else {
            HookWrite::Store(value)
        }
    }
}

/// The register's ignored writes, by reason.
fn ignored(bench: &Bench<'_>) -> Vec<(u8, u8, Ignored)> {
    bench
        .transcript()
        .filter_map(|record| match record.event {
            Event::IgnoredWrite {
                reg, value, why, ..
            } => Some((reg, value, why)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_dropped_byte_is_ignored_and_the_burst_goes_on() {
    let mut hook = Quirk {
        reg: 0x30,
        on_write: |_, _| HookWrite::Drop,
    };
    let bench = Bench::builder(&[]).scdc_hook(&mut hook).build().unwrap();
    bench.ddc().write(SCDC, &[0x30, 0x02, 0x35]).unwrap();
    assert_eq!(bench.sink().config_0(), Some(0));
    assert_eq!(bench.sink().config_1(), Some(0x35));
    assert_eq!(ignored(&bench), [(0x30, 0x02, Ignored::Hook)]);
}

#[test]
fn a_refused_byte_ends_the_burst() {
    let mut hook = Quirk {
        reg: 0x31,
        on_write: |_, _| HookWrite::Nack,
    };
    let bench = Bench::builder(&[]).scdc_hook(&mut hook).build().unwrap();
    let error = bench.ddc().write(SCDC, &[0x30, 0x01, 0x02]).unwrap_err();
    assert_eq!(
        error.kind,
        DdcErrorKind::NoAcknowledge(NoAcknowledgeSource::Data)
    );
    assert_eq!(bench.sink().config_0(), Some(0x01));
    assert_eq!(bench.sink().config_1(), Some(0));
    assert_eq!(ignored(&bench), [(0x31, 0x02, Ignored::Hook)]);
}

#[test]
fn a_changed_value_is_stored_under_the_registers_role() {
    // Config_0 keeps only its low bit; a byte stored to the sink's own status is ignored as
    // without a hook.
    let mut hook = Quirk {
        reg: 0x30,
        on_write: |value, _| HookWrite::Store(value & 1),
    };
    let bench = Bench::builder(&[]).scdc_hook(&mut hook).build().unwrap();
    bench.ddc().write(SCDC, &[0x30, 0x03]).unwrap();
    assert_eq!(bench.sink().config_0(), Some(0x01));
    let mut store = Quirk {
        reg: 0x21,
        on_write: |value, _| HookWrite::Store(value),
    };
    let bench = Bench::builder(&[]).scdc_hook(&mut store).build().unwrap();
    bench.ddc().write(SCDC, &[0x21, 0x01]).unwrap();
    assert_eq!(bench.sink().scrambling_status(), Some(0));
    assert_eq!(ignored(&bench), [(0x21, 0x01, Ignored::SinkOwned)]);
}

const TMDS_594: Line = Line::Tmds {
    character_rate_khz: 594_000,
    ratio: TmdsClockRatio::Div40,
    scrambled: true,
};

#[test]
fn a_sink_that_loses_its_lock_when_rewritten() {
    // A rewrite of TMDS_Config with the value it holds makes the sink re-sync.
    let mut hook = Quirk {
        reg: 0x20,
        on_write: |value, regs| {
            if regs.get(0x20) == value {
                HookWrite::StoreRelocking(value)
            } else {
                HookWrite::Store(value)
            }
        },
    };
    let bench = Bench::builder(&[]).scdc_hook(&mut hook).build().unwrap();
    let bus = bench.ddc();
    bus.write(SCDC, &[0x20, 0x03]).unwrap();
    bench.set_line(TMDS_594);
    bench.advance(Duration::from_millis(1));
    assert_eq!(bench.sink().scrambling_status(), Some(1));
    // The quick restore's rewrite: the status clears, then comes back after the trigger.
    bus.write(SCDC, &[0x20, 0x03]).unwrap();
    assert_eq!(bench.sink().scrambling_status(), Some(0));
    bench.advance(Duration::from_millis(1));
    assert_eq!(bench.sink().scrambling_status(), Some(1));
    let status: Vec<_> = bench
        .transcript()
        .filter_map(|record| match record.event {
            Event::Register {
                reg: 0x21,
                from,
                to,
                by: Actor::Sink,
                ..
            } => Some((from, to)),
            _ => None,
        })
        .collect();
    assert_eq!(status, [(0, 1), (1, 0), (0, 1)]);
}

/// A hook that reports `Scrambling_Status` as 0 on the wire, and notes each read.
struct HidesStatus;

impl ScdcHook for HidesStatus {
    fn read(&mut self, reg: u8, value: u8, _: &ScdcRegisters<'_>, env: &mut Env<'_>) -> u8 {
        env.note(SCDC, u32::from(reg), u32::from(value));
        if reg == 0x21 { 0 } else { value }
    }
}

#[test]
fn a_read_hook_changes_only_the_wire() {
    let mut hook = HidesStatus;
    let bench = Bench::builder(&[]).scdc_hook(&mut hook).build().unwrap();
    let bus = bench.ddc();
    bus.write(SCDC, &[0x20, 0x03]).unwrap();
    bench.set_line(TMDS_594);
    bench.advance(Duration::from_millis(1));
    let mut status = [0xFF];
    bus.write_read(SCDC, &[0x21], &mut status).unwrap();
    assert_eq!(status, [0]);
    // The sink is locked; the register and the bench's view say so.
    assert_eq!(bench.sink().scrambling_status(), Some(1));
    let notes: Vec<_> = bench
        .transcript()
        .filter_map(|record| match record.event {
            Event::Note { code, value, .. } => Some((code, value)),
            _ => None,
        })
        .collect();
    assert_eq!(notes, [(0x21, 1)]);
}

#[test]
fn presets_do_not_pass_through_the_hook() {
    let mut hook = Quirk {
        reg: 0x30,
        on_write: |_, _| HookWrite::Drop,
    };
    let bench = Bench::builder(&[]).scdc_hook(&mut hook).build().unwrap();
    bench.preset_scdc(0x30, 0x02);
    assert_eq!(bench.sink().config_0(), Some(0x02));
}

#[test]
fn the_hook_is_kept_across_a_plug() {
    let mut hook = Quirk {
        reg: 0x30,
        on_write: |_, _| HookWrite::Drop,
    };
    let bench = Bench::builder(&[]).scdc_hook(&mut hook).build().unwrap();
    assert!(format!("{bench:?}").starts_with("Bench"));
    bench.plug(&[], None, SinkConfig::new()).unwrap();
    bench.ddc().write(SCDC, &[0x30, 0x02]).unwrap();
    assert_eq!(bench.sink().config_0(), Some(0));
}

#[test]
fn the_default_hook_changes_nothing() {
    struct Plain;
    impl ScdcHook for Plain {}
    let mut hook = Plain;
    let builder = Bench::builder(&[]).scdc_hook(&mut hook);
    assert!(format!("{builder:?}").contains("scdc_hook: true"));
    let bench = builder.build().unwrap();
    bench.ddc().write(SCDC, &[0x30, 0x02]).unwrap();
    let mut read = [0];
    bench.ddc().write_read(SCDC, &[0x30], &mut read).unwrap();
    assert_eq!(read, [0x02]);
}
