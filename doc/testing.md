# Testing Strategy

This page says how the tests are designed, and which file tests what; it grows as the
parts are built.

## Test structure

- **`src/clock.rs`** — `SimClock`: the start at zero, `advance`, delays with and without
  overshoot, the tick's rounding (`now` down, delays up), that a delay from any point within
  a tick waits at least its duration, saturation at the largest `Timestamp`, monotonic
  reads over mixed delays and advances, the reference implementations, `Send` and `Sync`,
  sharing between threads, and use in a `static`.
- **`src/check/report.rs`** — `Report`: points and failures counted, failures recorded in
  order up to the capacity and counted past it (a capacity of 0 included), the default
  capacity, the printed form, `Display` on a failing writer, and `assert_ok`'s panic.
- **`src/check/clock.rs`** — the clock check: `SimClock` passes with the defaults and with
  coarse ticks and overshoots; broken doubles fail with the points expected — a frozen
  clock, a delay that waits half, a clock that counts down on every read, a delay that moves
  the clock back; the settings; the async form, through `Delay` and through the `Send` twin
  (with `async`).
- **`src/transcript.rs`** — the transcript's storage: order, overflow past the capacity
  (a capacity of 0 included), clearing, and completing a bus record's result.
- **`tests/bus.rs`** — the bench's DDC bus over targets the test defines: routing to the
  first target that answers, a transaction no target takes, a target taking all of its
  transaction; `read_segmented` with and without the segment pointer and split into pieces;
  every refusal by argument and by capability, in hdmi-hal's order, reaching no target and
  taking no time; empty buffers; timing per byte and per transaction; a target's error with
  what it applied kept and the read filled with the pattern; notes and `Env`; `advance`
  updating every target; the transcript's overflow; the EDID store's lengths; SCDC through
  hdmi-hal's `ScdcOverDdc`; the time handle passing the clock check; `Send`, `Sync` and
  threads.
- **`src/scdc.rs`** — the SCDC register roles, and the transactions the bus never makes: an
  empty write, a second address.
- **`tests/scdc.rs`** — the built-in SCDC target: the default registers; source writes
  stored and recorded, bursts included; register events after their bus record, at its time;
  the offset persisting; SCDC through `ScdcOverDdc`; update flags cleared by writing them
  back; the sink's registers ignored or refused; registers outside the modelled set; bursts
  past 0xFF wrapping or refused; presets; the `Sink_Version` setting; a sink without SCDC; a
  test's target at 0x54 replacing the built-in one; the settings and the `Sink` view.
- **`README.md` and the rustdoc examples** — run as doctests.

## How the tests are designed

- **Deterministic.** Every test runs on simulated time. Nothing reads the host's clock or
  sleeps, so a test gives the same result on every run and every machine.
- **maquette's implementations pass its own checks.** `SimClock`, the bus and the simulated
  PHY are each run through the conformance check for their trait.
- **The checks' negative cases run against broken doubles:** a clock that goes backwards, a
  delay that returns early, a bus that ignores its capabilities, a PHY that starts without
  being configured. Each must be reported by the check, as the point that failed.
- **Sink behaviour against its sources.** Each row of the sink-behaviour table in
  [`architecture.md`](architecture.md) has a test of its fixed behaviour or of each setting,
  default included.
- **Ordering through the transcript.** Tests assert on the transcript's records and their
  times, not on internal state.
- **Records at capacity.** Each record (the transcript, the fault schedule, the reports) is
  tested full, counting its overflow without a panic, at the default and small capacities.

Tests of other crates' procedures against the bench (TMDS bring-up through culvert's client,
the EDID read) live with those crates, which use maquette as a dev-dependency.

## Fuzzing

A fuzz target drives the bench with arbitrary sequences of bus and PHY calls, line changes,
faults, controls and clock advances, and checks the simulator's invariants: the same input
gives the same transcript; nothing panics and no arithmetic overflows; `Scrambling_Status`
is set only under the coupling's conditions; registers the sink owns change only by the
sink's rules; the segment pointer never outlives its command sequence.

## Coverage

CI measures line coverage without features, with `std` and with `async`, merged into one
report, against `.coverage-baseline` (see [`setup.md`](setup.md)).
