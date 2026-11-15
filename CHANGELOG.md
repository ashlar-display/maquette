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
  `time()`, its clock as one `Clock` and `Delay`. Calls from several threads are
  serialized: each bus call, and each delay with the event after it, is one step, so the
  transcript is in time order and faults count calls in its order. `advance` moves its time
  and lets every target see it. `BenchError` refuses an EDID store that is not whole 128-byte blocks, up
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
  source's registers (`Source_Version`, `TMDS_Config`, `Config_0`, `Config_1`,
  `Test_Read_Request`, the manufacturer's block from 0xDE) are stored as written; the update
  flags (`Update_0`, `Update_1`) clear when written back as 1; the sink's (`Sink_Version`,
  `Scrambling_Status`, `Source_Test_Configuration`, status flags, error counters, the
  manufacturer's identification at 0xD0–0xDD) follow `OwnedWrite`; any other register is
  stored. Addresses, bits
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
- **The line and the sink's events:** `Line` (`Off`, `Tmds` with rate, ratio and
  scrambling, `Frl`), set by `Bench::set_line` and seen by targets through `Env::line`;
  `Bench::hpd` and `hpd_pulse`; `unplug` and `plug` (a fresh sink with new stores and
  settings; nothing answers while unplugged, the test's own targets included); `detach` and
  `reattach` (every call that passes the checks fails `Detached`, taking no time). Each is a
  bench event after which every target sees the bench (`Target::update`), recorded as
  `Event::Line`, `Hpd`, `Plug` or `Adapter`; a delay on the bench's time handle is an event
  too. `Sink::hpd` and `Sink::is_plugged`, `Bench::line` and `Bench::is_attached` read the
  state. `SinkConfig::with_scdc_during_hpd_low` and `with_edid_during_hpd_low` say whether
  the built-in targets answer while HPD is low (`DuringHpdLow::Answers` by default, or
  `Silent`).
- **The scrambling lock:** the SCDC target sets `Scrambling_Status` when
  `Scrambling_Enable` is set, the line is scrambled TMDS at the ratio
  `TMDS_Bit_Clock_Ratio` names, that ratio fits the rate (1/40 above 340 Mcsc, 1/10 at or
  below), no refusal withholds it, and the lock's `Trigger` has passed since these
  conditions last began to hold; it clears it when a condition fails. Each change is a
  `Register` event by `Actor::Sink`, made only when the lock is gained or lost, so a preset
  holds until then; the sink's own HPD reset of the status loses the lock, and the
  conditions begin to hold again at the reset. `SinkConfig` gains
  `with_scrambling_lock` (`Trigger::After` 1 ms by default, or `Trigger::AfterReads(n)`),
  `with_strict_lock` (off by default: output must start after the write that set
  `Scrambling_Enable` or changed the ratio, and at least 1 ms after a ratio change),
  `with_scrambling_refusals` (`Refusal` with
  `RefusalKind::WithholdStatus` or `DropEnable` for the first attempts; none by default,
  each refusal an `Event::Refusal`) and `with_enable_timeout` (100 ms by default, from the
  write that set `Scrambling_Enable` or one that changed the ratio while it was set; output
  that starts at or after the deadline is late; `None` never gives up), with getters.
- **HPD resets:** around an HPD low the SCDC target resets each `RegisterGroup`
  (`TmdsConfig`, `Status` — `Scrambling_Status`, the update and status flags —
  `SourceVersion`, `Config`) to 0 once, as its `HpdReset` says: `OnHpdLow`, the default for
  every group; `After(d)`, taken at the first event at or after it or when HPD rises;
  `OnHpdRise`, losing what was written during the low too; or `Never`. Set by
  `SinkConfig::with_hpd_reset`. Each reset register is a `Register` event by `Actor::Sink`;
  resetting `TMDS_Config` loses the scrambling lock.
- **The simulated PHY**, `Bench::phy`: a `Copy` handle implementing hdmi-hal's `TmdsPhy`,
  `FrlPhy` and `PhyPower` to their contracts, the mode rule included, with `PhyError`s
  (`PhyErrorKind::PoweredOff`, `NotConfigured`, `Running`, `Unsupported`, `NotInFrl`,
  `Fault`). It drives the line whenever its output changes: running TMDS from its
  configuration and scrambler, FRL from its rate. Where the contract allows several
  behaviours, `PhySettings` sets them, each defaulting to the least forgiving one:
  `configure` fails with the output running (`ConfigureWhileRunning`), the scrambler after
  `configure` is the opposite of what the rate needs (`ScramblerAfterConfigure`), and
  `start_output` takes 1 ms; the PHY starts powered on. `BenchBuilder::phy_capabilities`
  sets what it can do (by default `DEFAULT_PHY_CAPABILITIES`: 600 000 kHz with a scrambler,
  low rates included). Every call is an `Event::Phy` with its `PhyCall` and result, a bench
  event, and counts against the budget. `Bench::schedule_phy` schedules a `PhyFault`, in
  the same schedule as the bus's faults, failing a call before or after it takes effect.
  The hdmi-hal PHY traits and types are re-exported.
- **`Coupled<P>`**, any PHY driving the bench's line: `Coupled::new(phy, &bench)`
  implements `TmdsPhy` with `P`'s error, and `PhyPower` and `FrlPhy` where `P` does. Calls go
  to `P` unchanged; what succeeded drives the line by the contract's guarantees (unscrambled
  after `configure` until `set_scrambling`); every call is an `Event::Phy` (a failure as
  `PhyErrorKind::Coupled`), a bench event, and counts against the budget. `get_ref`,
  `get_mut` and `into_inner` reach `P`.
- **The async handles** (`async` feature): `Ddc` implements hdmi-hal-async's `SendDdcBus`,
  `Time` its `SendDelay`, and `Phy` its `SendTmdsPhy`, `SendFrlPhy` and `SendPhyPower`; the
  plain async traits follow through hdmi-hal-async's blanket implementations. Each async
  method runs its sync counterpart when first polled, so the futures are `Send` and complete
  with any executor. hdmi-hal-async is re-exported as `maquette::hdmi_hal_async`.
- **Faults:** `Bench::schedule` takes a `Fault` — a `Filter` (address, `OpKind`, first byte
  written), the `nth` matching operation, `Times::Count(k)` or `UntilCleared`, an `Effect`
  (`Fail(kind)`, or `Slow(d)`) and a `Point` (`Before` the sink sees it, `After` it applied
  it, `Partway { byte }`) — and returns a `FaultId` for `clear_fault`, or `ScheduleFull`
  past `FAULTS`. An operation is one transaction; refused, empty and detached calls do not
  count. A timeout charges the bus's stretch timeout; a struck operation is charged for
  the bytes that went out; each strike is an `Event::Fault` after its bus record.
  `clear_faults` and `scheduled_faults` manage the schedule. A preset is now a bench event,
  so a preset of `TMDS_Config` can lose or start the scrambling lock.
- **The operation budget:** every bus call (refused ones included), clock read and delay
  through the bench's handles counts; past `BenchBuilder::budget` (by default
  `DEFAULT_BUDGET`, 1 000 000; `None` for none) the bench panics, naming the budget, so a
  procedure that loops forever fails its test. `Bench::operations` says how many were made.
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
- **The `DdcBus` check** (`check::DdcBusCheck`): hdmi-hal's `DdcBus` contract as a caller
  sees it, against a device the test describes (`check::DdcDevice`: its EDID, whether it has
  a segment pointer, an address where nothing answers). Argument and capability refusals,
  empty buffers, the EDID read from segment 0 (without a pointer too) and segment 1, and
  `NoAcknowledge` where nothing answers; every call safe on a real device, points without a
  description skipped. `run_threads` (with `std`) runs it from as many threads at once as
  asked for, at least one;
  `run_async` (with `async`) over hdmi-hal-async's `DdcBus`. Failures are
  `check::DdcBusFailure`s.
- **The `ScdcTransport` check** (`check::ScdcTransportCheck`): over a register image the
  test knows (`check::ScdcImage`, or `ScdcImage::of(&bench)`: the bench's registers but the
  ones that change by themselves), `read` of every known register, `read_block` of 16
  registers wherever they fit agreeing register by register, empty buffers `Ok`, reads past
  0xFF returning, and, with a `check::FailNext` hook (implemented by `Bench`), an error
  reaching the caller with its kind and the transport working after it; a failure the hook
  cannot arm is `NotArmed`, the setup's and not the transport's, and one no transfer took
  is withdrawn. `run_async` over
  hdmi-hal-async's `ScdcTransport`. Failures are `check::ScdcTransportFailure`s.
- **The PHY check** (`check::PhyCheck`): hdmi-hal's PHY contracts with a configuration the
  PHY can do and a `check::OutputProbe` (implemented by `Bench`) for whether its output
  runs. `run_tmds` (`TmdsPhy` alone, a fresh PHY), `run_powered` (with `PhyPower`, from a
  power cycle) and `run_frl` (with `FrlPhy`: the mode rule), and their async forms. Failures
  are `check::PhyFailure`s naming a `check::PhyPoint`.
- **`check::Report`**, what every check returns: the points run, every failed point counted,
  the first 16 recorded in the order found, and `assert_ok`, which panics listing them. A
  check of another crate's own trait that runs against the bench builds one with
  `Report::new` and `record`, so its users get the same report.

### Internal

- A fuzz target (`fuzz/`, `bench`) drives the bench with arbitrary actions and checks the
  simulator's invariants: the same transcript for the same input, the scrambling status
  only under the lock's conditions, the sink's registers never changed by the source, the
  segment pointer never outliving its command sequence, time never going backwards. CI
  fuzzes it for 60 s on pushes and pull requests and for an hour weekly, opening a pull
  request with the minimised corpus. release-prep refreshes maquette's entry in
  `fuzz/Cargo.lock`; Dependabot keeps the fuzz lock file current.

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
