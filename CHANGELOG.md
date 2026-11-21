# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

The first release: a simulated HDMI sink for tests, and conformance checks for
implementations of hdmi-hal's traits.

**Time**

- **`SimClock`:** hdmi-hal's `Clock` and `Delay` on simulated time, starting at zero and
  moving only by a delay or the test's `advance`; it never reads the host's time, never
  moves backwards and saturates instead of overflowing. `with_tick` gives it a coarse
  resolution and `with_overshoot` makes every delay wait longer than asked. `Sync`,
  `const`-constructible, and implemented for `&SimClock` too, so one clock is passed as both
  clock and delay.

**The bench**

- **`Bench`**, the simulation a procedure runs against, built with `Bench::new` or
  `Bench::builder` (the sink's settings, the bus's capabilities and timing, the clock, the
  PHY's settings and capabilities, the test's own targets, an SCDC hook, the transcript's
  mode, and the capacities: 256 records and 16 scheduled faults by default). `BenchError`
  refuses an EDID or DisplayID store that is not whole 128-byte blocks, up to 256 blocks.
- **Handles** that are `Copy`, `Send` and `Sync` and name no capacity: `ddc()`, the sink's
  DDC bus (`DdcBus`, and its own `DdcHandle`, so it passes to hdmi-hal's `ScdcOverDdc` by
  value); `time()`, its clock as one `Clock` and `Delay`; `phy()`, the source's
  simulated PHY; `sink()`, a read-only view of the sink's registers, HPD and plug state.
  Calls from several threads are serialized: each bus call, and each delay with the event
  after it, is one step.
- **The test's controls:** `advance`; `set_line`; `set_character_errors`; `set_lane_limits`;
  `set_hpd` and `hpd_pulse`; `unplug` and `plug` (a fresh sink with new stores and
  settings); `detach` and `reattach`; `preset_scdc`. Each is a bench event: every target sees
  the bench after it.
- **The operation budget:** every bus call, clock read, delay and PHY call through the
  handles counts; past `BenchBuilder::budget` (`DEFAULT_BUDGET`, 1 000 000, by default;
  `None` for none) the bench panics, naming the budget, so a procedure that loops forever
  fails its test.

**The bus**

- **`Ddc`, the bench's `DdcBus`:** each call is one transaction, routed to the first
  target that answers the address of its first operation, or `NoAcknowledge(Address)`. It
  refuses what `DdcBus`'s argument rules and its capabilities rule out, before any target
  and without taking time; splits `read_segmented` into pieces where the capabilities say
  so; charges the clock by `BusTiming` (90 µs a byte by default, 100 kHz); and fills a
  failed read with `0xA5`. Its errors are `BusError`s, printed as a sentence.

**The sink**

- **SCDC at 0x54:** a 256-byte register file. A write sets the offset with its first byte;
  the offset persists. The source's registers are stored as written; the update flags clear
  when written back as 1; the sink's registers follow `OwnedWrite`; roles, addresses and
  bits are cited in the code from Linux, amdgpu, Xilinx and Amlogic.
- **The EDID at 0x50 and DisplayID at 0x52, behind the E-DDC segment pointer at 0x30**
  (VESA E-DDC 1.3): the pointer is 0 at the start of every transaction; each structure's
  word offset persists, for current-address reads; a read goes on with data past a store,
  0xFF or wrapped, since the host acknowledges what it reads.
- **The line** (`Line`: `Off`, `Tmds` with rate, ratio and scrambling, `Frl` with rate,
  each lane's training pattern, the output and the equalization, built with `Line::frl` or
  `Line::frl_with` and `with_equalization`), which the PHY drives when its output changes and a test sets with
  `set_line`; the last change wins. An unplugged sink receives no line.
- **Character errors** (`ErrorRates`, errors a second on each lane, and Reed-Solomon
  corrections a second), set with
  `set_character_errors`, and **lane limits** for FRL link training (`LinkLanes`, each
  lane's highest rate and least TxFFE level), set with `set_lane_limits`.
- **The scrambling lock:** `Scrambling_Status` is set once `Scrambling_Enable` is set, the
  line is scrambled TMDS at the ratio `TMDS_Bit_Clock_Ratio` names and that ratio fits the
  rate (1/40 above 340 Mcsc), no refusal withholds it, and the lock's `Trigger` has passed
  since these conditions last began to hold. The sink changes the status only when its lock
  is gained or lost, so a preset holds; its own reset of the status re-arms the lock.
- **The status flags:** `Status_Flags_0`'s `Clock_Detected` follows a TMDS line, and the
  locks of lanes 0–2 are set once the sink can decode the line (the ratio fits the rate and
  matches `TMDS_Config`, the line is scrambled exactly when `Scrambling_Enable` is set, and
  always above 340 Mcsc) and the lock's `Trigger` has passed. Each change sets
  `Status_Update`, unless `StatusUpdate::Never`.
- **HPD resets:** each `RegisterGroup` resets as its `HpdReset` says: as HPD falls (the
  default, after the public sink drivers), after a time, as HPD rises, or never.
- **`SinkConfig`**, the sink's settings with `const` builders and getters: SCDC or none, the
  `Sink_Version`, owned writes, bursts past 0xFF, reads past a store, missing segments, the
  128-byte boundary, the segment pointer, what answers while HPD is low, the lock's trigger,
  the strict sink (output started after the write, and 1 ms after a ratio change),
  refusals, the enable timeout (100 ms by default, from the write that set
  `Scrambling_Enable` or changed the ratio), the HPD resets, `Status_Update`, `CED_Update`,
  clear on read, the registers during a read, the maximum FRL rate, the LTP requests, the
  LTS:3 timeout and what the sink checks before it passes. Each default is sourced or marked *chosen*, the least forgiving where
  sources differ.
- **FRL link training, the sink's side:** a sink with a maximum FRL rate sets `FLT_ready`;
  a write of `FRL_Rate` clears `FRL_start` and posts its LTP requests with `FLT_update`
  (0xF on every lane for a rate it cannot take); once the source has cleared `FLT_update`
  and every active lane has carried its pattern, it posts the pass; once the lanes lock on
  the source's FRL output, it sets `FRL_start`. `FRL_Rate` 0 returns to legacy. Lanes that
  do not pass within the LTS:3 timeout (180 ms by default, in `FFE_Levels` + 1 attempts)
  are asked for another TxFFE level (0xE), then every lane for a lower rate (0xF). That is
  `FrlPass::Patterns`, the default; `FrlPass::Unchecked` is a sink that waits up to 100 ms
  for each clear and passes, and sets `FRL_start`, whatever the lanes carry.
- **The character error counters (0x50–0x5A):** lanes 0–2, and lane 3 in 4-lane FRL,
  count the link's errors while locked, and the Reed-Solomon corrections count while the
  FRL lanes are; each saturates at 0x7FFF and is valid while counting, and the checksum
  covers lane 3 at a 4-lane rate. They reset with `RegisterGroup::ErrorCounters`.
  `CED_Update` (and `RSED_Update`, for the corrections) is set by a check at each whole
  second when a count rose by 4 or more since the last check or just reached 0x7FFF
  (`CedUpdate::EverySecond`), or never. A read may clear a count (`CedRead::ClearOnRead`),
  and the counters move during a read by default (`DuringRead::Moving`, each byte at its own
  moment on the bus's time), so a burst can read torn values; `DuringRead::Held` holds them.

**The PHY**

- **The simulated PHY**, `Bench::phy`: hdmi-hal's `TmdsPhy`, `FrlPhy` and `PhyPower` to their
  contracts, the mode rule included, with `PhyError`s. `PhySettings` sets what the contract
  leaves open, each defaulting to the least forgiving: `configure` with the output running
  fails, the scrambler after `configure` is the opposite of what the rate needs (within the
  PHY's capabilities), and `start_output` takes 1 ms. `BenchBuilder::phy_capabilities` sets
  what it can do.
- **`Coupled<P>`**, any PHY driving the bench's line: calls go to `P` unchanged, and what
  succeeded drives the line by the contract's guarantees; `get_ref`, `get_mut` and
  `into_inner` reach `P`.

**Faults and scheduled actions**

- **Faults:** `Bench::schedule` takes a `Fault`: a `Filter`, the `nth` matching operation,
  `Times` (`Count(k)`, `UntilCleared`, or `For(d)` on the bench's clock), an `Effect`
  (`Fail(kind)`, `Slow(d)`, or `Corrupt { byte, xor }`, a read byte garbled under an `Ok`)
  and a `Point` (`Before`, `After`, `Partway { byte }`; a NACK partway through a read falls
  on its address byte). `Bench::schedule_phy` schedules a `PhyFault` in the same schedule.
  Each strike is an `Event::Fault`.
- **Scheduled actions:** `Bench::schedule_action` takes an `Action` (a preset, HPD, unplug,
  plug, the line, the character errors, the lane limits, detach, reattach) and a `When`
  (before or after the `nth` matching bus operation, after the `nth` PHY call, or at a
  time), so the sink changes during a procedure. An action comes once, in the faults'
  schedule, recorded as an `Event::Action` before the events its change makes.

**Extending the sink**

- **`Target`**, a device on the sink's bus, for a test's own (DDC/CI, an HDCP port):
  `answers`, `transaction` over `Operation`s, and `update`; `Env` gives it the time, HPD,
  the line and `note`. It runs with the bench locked: using the bench's handles from it
  panics, naming the cause.
- **`ScdcHook`**, a test's quirk on top of the built-in SCDC sink
  (`BenchBuilder::scdc_hook`): it decides each byte the source writes (`HookWrite::Store`,
  `Drop`, `Nack`, `StoreRelocking`) and what each read returns, while the sink keeps its
  lock, resets and records.

**The transcript**

- **`Bench::transcript`:** one ordered `Record` per bus call, register change, ignored
  write, refusal, fault, PHY call, line, HPD, plug, adapter change, action and target note,
  with its time. `Record`, `BusRecord` and `Event` are read, not built: each `Event` variant
  with fields is `#[non_exhaustive]`. What does not fit is counted
  (`Bench::transcript_overflow`; `Bench::transcript_len` counts what is kept); by default the oldest records are kept, and
  `BenchBuilder::keep_newest_records` keeps the newest, for a session-length test.

**Async**

- **The `async` feature:** the handles implement hdmi-hal-async's `Send` twins (`SendDdcBus`,
  `SendDelay`, `SendTmdsPhy`, `SendFrlPhy`, `SendPhyPower`), and so the plain async traits;
  each async method runs its sync counterpart when first polled, so the futures are `Send`
  and complete with any executor. `SimClock` implements `SendDelay` too.

**Conformance checks** (`check`)

- **The clock check** (`check::clock`, `check::ClockCheck`): every read no earlier than the
  one before, every delay advancing the clock by at least its duration; with
  `run_with_reference`, each delay measured on a clock the test trusts too
  (`check::HostClock` with `std`), so a clock at the wrong rate fails.
- **The `DdcBus` check** (`check::DdcBusCheck`, against a `check::DdcDevice`): argument and
  capability refusals, empty buffers, the EDID read at offsets, through `write_read` and
  through a `write` then a current-address `read`, and `NoAcknowledge` where nothing
  answers; every call safe on a real device. `run_threads` (with `std`) runs it from several
  threads at once.
- **The `ScdcTransport` check** (`check::ScdcTransportCheck`, over a `check::ScdcImage`):
  `read` and `read_block` against the image; with `with_write`, a register written and read
  back; with a `check::FailNext` hook, an error reaching the caller and the transport
  working after it.
- **The PHY check** (`check::PhyCheck`, with a `check::OutputProbe`, or `check::NoProbe`):
  hdmi-hal's PHY contracts, the line checked for the configuration's rate and ratio and the
  scrambling asked; `run_tmds`, `run_powered` and `run_frl`.
- **`check::Report`**, what every check returns: every failed point counted, the first 16
  recorded, and `assert_ok`. Another crate's check of its own trait against the bench builds
  one with `Report::new` and `record`. Each check has an async form with the `async`
  feature.

**API**

- Every hdmi-hal type maquette's API names is re-exported, as are `hdmi_hal` and, with
  `async`, `hdmi_hal_async`. Value builders are `#[must_use]`; the plain value types have a
  `Default` and the kinds a test may collect are `Hash`.

### Internal

- A fuzz target (`fuzz/`, `bench`) drives the bench with arbitrary actions and checks the
  simulator's invariants: the same transcript for the same input, the scrambling status
  only under the lock's conditions and set once they have held for a time trigger, the
  sink's registers never changed by the source, the segment pointer never outliving its
  command sequence, time never going backwards, and a transcript (4096 records) that never
  overflows. CI
  fuzzes it for 60 s on pushes and pull requests and for an hour weekly, opening a pull
  request with the minimised corpus. release-prep refreshes maquette's entry in
  `fuzz/Cargo.lock`; Dependabot keeps the fuzz lock file current.

- CI: format, clippy and tests without features, with `std` and with `async`; docs with
  every feature and `-D warnings`, as docs.rs builds them; `no_std` builds for the bare
  `thumbv7em-none-eabi`, `x86_64-unknown-uefi` and `aarch64-unknown-uefi` targets with and
  without `async`; a read-only token except where the coverage job opens its PR. On `main`,
  CI fails when `Cargo.toml` or `fuzz/Cargo.toml` has a `[patch]` section. The fuzz job
  checks `fuzz/Cargo.lock` is current before it runs.
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
