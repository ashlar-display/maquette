# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **`SimClock`:** hdmi-hal's `Clock` and `Delay` on simulated time, starting at zero and
  moving only by a delay or the test's `advance`; it never reads the host's time, never
  moves backwards and saturates instead of overflowing. `with_tick` gives it a coarse
  resolution (`now` rounded down, delays rounded up to whole ticks) and `with_overshoot`
  makes every delay wait longer than asked; either way `now` after `delay(d)` is at least
  `d` later. `Sync`, `const`-constructible, and implemented for `&SimClock` too, so one
  clock is passed as both clock and delay.
- Re-exports of hdmi-hal's `Clock`, `Delay` and `Timestamp`.
- **The clock check** (`check::clock`): the conformance check for implementations of
  hdmi-hal's `Clock` and `Delay`, passed as two values. Every read of the clock must be no
  earlier than the one before it, and every delay must advance the clock by at least its
  duration, so a frozen clock fails. `check::ClockCheck` sets the delays (by default 0, 1 ns,
  1 µs, 999 µs, 1 ms, 1.001 ms and 10 ms) and the reads in a row (1000); with `async`,
  `run_async` checks an async delay, its `Send` twin included. Failures are
  `check::ClockFailure` values.
- **`Bench`**, the simulation a procedure runs against, built with `Bench::new` or
  `Bench::builder` (the sink's settings, the bus's capabilities and timing, the clock, the
  test's own targets, and the capacities, which default to 256 records and 16 faults). Its
  handles are `Copy`, `Send` and `Sync` and name no capacity: `ddc()`, the sink's DDC bus, and
  `time()`, its clock as one `Clock` and `Delay`. `advance` moves its time and lets every
  target see it. `BenchError` refuses an EDID store that is not whole 128-byte blocks, up
  to 256; an empty one is accepted.
- **`Ddc`, the bench's `DdcBus`:** each call is one transaction, routed to the first target
  that answers the address of its first operation, or `NoAcknowledge(Address)`. It refuses
  what `DdcBus`'s argument rules and its capabilities rule out, before any target and
  without taking time; splits `read_segmented` into pieces where the capabilities say so,
  writing the segment pointer from segment 1; charges the clock by `BusTiming` (90 µs a byte
  by default, standard mode); and fills a failed read with `0xA5`. Its errors are
  `BusError`s, with the kind, the call and the address.
- **`Target`**, a device on the sink's bus, for a test's own: `answers`, `transaction` over
  `Operation`s, and `update`; `Env` gives it the time, HPD and `note`.
- **The transcript**, `Bench::transcript`: one `Record` per bus call (`BusRecord`, refused
  calls included, a split read per piece) and per target note, in order, with its time;
  what does not fit is counted. `Event` also has the variants the built-in targets will
  record (`Register`, `IgnoredWrite`, with `Actor` and `Ignored`).
- **The sink's SCDC registers** at 0x54, the built-in target routed after a test's own: a
  write sets the offset with its first byte, a read reads from it, the offset persists. The
  source's registers (`Source_Version`, `TMDS_Config`, `Config_0`, `Config_1`, …) are stored
  as written; the update flags (`Update_0`, `Update_1`) clear when written back as 1; the
  sink's (`Sink_Version`, `Scrambling_Status`, status flags, error counters, the
  manufacturer's block) follow `OwnedWrite`; any other register is stored. Addresses, bits
  and roles are cited in the code from Linux, amdgpu, Xilinx and Amlogic. Every byte written
  is a `Register` or `IgnoredWrite` event in the transcript.
- **The sink's EDID at 0x50 and DisplayID at 0x52, behind the E-DDC segment pointer at
  0x30** (VESA E-DDC 1.3), the built-in target routed after a test's own. The pointer is 0
  at the start of every transaction and acknowledged for segments the sink has; each
  structure's word offset persists, for current-address reads; writes after the offset are
  ignored and recorded; a sink without a pointer answers segment 0 only; an empty store does
  not answer. `BenchBuilder::displayid` gives the DisplayID store, checked like the EDID's
  (`BenchError::DisplayIdLength`). `SinkConfig` gains `without_segment_pointer`,
  `with_past_store` (`PastStore::Fill`, reading 0xFF, by default; `Wrap`, `Nack`),
  `with_missing_segment` (`MissingSegment::Nack` by default; `Ack`) and
  `with_block_boundary` (`BlockBoundary::Continue` by default; `Wrap`), with getters.
- **`Bench::preset_scdc`**, which sets any SCDC register, recorded as by `Actor::Test`, and
  **`Bench::sink`**, a `Sink` view of the registers (`scdc`, `tmds_config`,
  `scrambling_status`, `sink_version`, `source_version`, `update_0`, `config_0`, `config_1`)
  that takes no bus operation.
- **`SinkConfig`**, the sink's settings, with `const` builders and getters: `without_scdc`
  (nothing answers 0x54), `with_sink_version` (1 by default), `with_owned_scdc_writes`
  (`OwnedWrite::Ignore` by default, or `Nack`) and `with_scdc_past_end`
  (`ScdcPastEnd::Wrap` by default, or `Nack`).
- Re-exports of `hdmi_hal` itself and its DDC types (`DdcBus`, `DdcCapabilities`,
  `DdcError`, `DdcErrorKind`, `EddcTarget`, `NoAcknowledgeSource`).
- **`check::Report`**, what every check returns: the points run, every failed point counted,
  the first 16 recorded in the order found, and `assert_ok`, which panics listing them.

### Internal

- CI: format, clippy and tests without features, with `std` and with `async`; docs with
  every feature and `-D warnings`, as docs.rs builds them; `no_std` builds for the bare
  `thumbv7em-none-eabi` target with and without `async`; a read-only token except where
  the coverage job opens its PR. On `main`, CI fails when `Cargo.toml` has a `[patch]`
  section.
- MSRV check: `cargo msrv verify` runs the tests per feature set and the bare `no_std` builds
  on the `rust-version` in `Cargo.toml` (1.85).
- The README's examples are compiled and run as doctests.
- Coverage ratchet: CI measures line coverage against `.coverage-baseline` and opens a PR
  when it rises.
- Weekly RustSec audit, a weekly docs build as docs.rs does it (nightly, `--cfg docsrs`),
  and Dependabot for the workflow pins and `Cargo.lock`.
- Release workflows (release-prep, release-tag, publish) as in the other crates, with a
  first-release path: release-prep releases the version in `Cargo.toml` when the crate is
  not on crates.io yet, and publish uses a first-publish token where trusted publishing is
  not yet possible. publish's verify and MSRV jobs mirror CI and fail on a `[patch]`.
