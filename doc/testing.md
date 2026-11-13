# Testing Strategy

This page says how the tests are designed, and which file tests what; it grows as the
parts are built.

## Test structure

- **`src/clock.rs`** — `SimClock`: the start at zero, `advance`, delays with and without
  overshoot, the tick's rounding (`now` down, delays up), that a delay from any point within
  a tick waits at least its duration, saturation at the largest `Timestamp`, monotonic
  reads over mixed delays and advances, the reference implementations, `Send` and `Sync`,
  sharing between threads, and use in a `static`.
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
