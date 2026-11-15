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
  back; the sink's registers ignored or refused; the manufacturer's block past its
  identification stored; registers outside the modelled set; bursts
  past 0xFF wrapping or refused; presets; the `Sink_Version` setting; a sink without SCDC; a
  test's target at 0x54 replacing the built-in one; the settings and the `Sink` view.
- **`src/eddc.rs`** — the E-DDC transactions the bus never makes: several pointer bytes,
  empty writes, a second address; a missing DisplayID store.
- **`tests/eddc.rs`** — the built-in EDID and DisplayID target: segment 0 and higher segments
  through the pointer, whole and split into pieces; the word offset persisting for
  current-address reads; the pointer reset between transactions and not readable; a sink
  without a pointer (E-DDC 8.1.2); missing segments refused or read past the store, and a
  segment present in one store only; reads past the store filled, wrapped or refused; the
  128-byte boundary continued or wrapped; writes after the offset ignored and recorded; an
  empty EDID; the DisplayID's own store and offset, and its length check.
- **`tests/events.rs`** — bench events: the line starting off and its changes recorded once
  and seen by targets; a delay on the time handle as an event; HPD changes, the built-in
  targets answering or silent while HPD is low, a test's target deciding with HPD, pulses,
  and HPD refused on an unplugged sink; unplugging (nothing answers, the address byte
  charged); plugging a fresh sink, over a plugged one, and with a bad store; detaching (calls
  fail `Detached` after the argument checks, take no time) and reattaching; the operation
  budget counting what the handles do and not the test's own controls, panicking past it,
  and off.
- **`tests/lock.rs`** — the scrambling lock: the status 1 ms after the later of the write
  and scrambled output, either order; the strict sink waiting for output to start again, and for 1 ms after a ratio change; the
  ratio match, the ratio fitting the rate on both sides of 340 000 kHz, scrambling at 1/10
  and unscrambled output; the lock lost when the output
  stops or `Scrambling_Enable` is cleared; a rewrite not restarting it; the count trigger;
  refusals of both kinds, the trigger counting from the accepted attempt; the trigger
  counting from the write that makes the ratio match; the count trigger restarting when a
  condition fails; the enable timeout, with and without output in time, output late
  though the time passed on the bus, a ratio change restarting it, its 100 ms default and
  a sink that never gives up; a delay on
  the time handle as the event that sets it; a preset status holding until the lock
  changes; the defaults.
- **`tests/hpd.rs`** — the HPD resets: every group on any low by default; `Config` on
  every low and the rest after 100 ms; a reset taken at the first event after it is due,
  and when HPD rises; a group that never resets; a reset after zero; a reset as HPD rises,
  losing what was written during the low; one reset per low; a `TMDS_Config` reset losing
  the lock; a status reset re-arming it while a preset still holds; each change of the
  status recorded once; the groups and their defaults.
- **`tests/faults.rs`** — faults: before (nothing applied, the address byte charged), after
  (the write latched), partway through a write and a read (the buffer the pattern) and past
  the last byte; `nth` with a count and until cleared; filters; refused, empty and detached
  calls not counting, unplugged ones counting; each piece of a split read an operation;
  the first scheduled fault striking while every fault counts; a full schedule; a fault
  that never strikes; the stretch timeout, slow operations, `Detached` and `Unsupported`
  effects; a preset as an event the lock answers.
- **`tests/phy.rs`** — the simulated PHY: a bring-up driving the line until the sink locks,
  and a procedure that never sets the scrambler failing at every rate; each scrambler
  setting after `configure`; `configure` with the output running, failing or stopping it;
  power and the configure-since-power-on rule; the capabilities (rate, scrambler, low-rate
  scrambling), 340 000 kHz counting as a low rate; the mode rule between TMDS and FRL, and the FRL calls outside FRL; a test's
  line holding until the PHY's output changes; the records; PHY faults before and after
  effect, counting and sharing the schedule; the budget; two handles one PHY; generic code
  bounded on the traits; the settings.
- **`tests/coupled.rs`** — `Coupled<P>` over a double: a bring-up locking the sink; the line
  unscrambled after `configure` until `set_scrambling`, whatever the double's scrambler;
  errors returned unchanged and a failed call changing nothing, recorded as `Coupled`; the
  budget; FRL and power passing through by the contract; the last change to the line
  winning, with the simulated PHY and the test's own line; scheduled PHY faults left to the
  simulated PHY; reaching and taking back the PHY; a TMDS-only PHY.
- **`tests/asynch.rs`** (with `async`) — an async bring-up bounded on the `Send` twins alone,
  its future proved `Send` and run on another thread, locking the sink; the plain async
  traits through the blanket implementations; hdmi-hal-async's `SendScdcOverDdc` over the
  bench's bus; errors and records as the sync handles make them; nothing happening until a
  future is polled.
- **`tests/ddc_check.rs`** — the `DdcBus` check: the bench's bus passing with a described
  device, as a narrow SMBus-style bus, without a segment pointer, without a description,
  from several threads and async; and buses broken one way each caught — doing more than
  they claim, accepting an empty write, writing the pointer for segment 0, corrupting what
  they read, answering where nothing does, panicking; unreachable addresses and no
  segmented reads refused; failures from several threads counted; every failure printed.
- **`tests/scdc_check.rs`** — the `ScdcTransport` check: hdmi-hal's `ScdcOverDdc` over the
  bench passing, block and bytewise, behind a 16-byte bus, without faults and async; and
  transports broken one way each caught with exact counts — block reads from the wrong
  register, no 16-byte reads, hidden errors, stuck after an error, failing an empty buffer —
  and a sink without SCDC failing every read; the failures printed.
- **`tests/phy_check.rs`** — the PHY check: the bench's PHY passing every run under both
  `configure`-while-running settings and both rates, a PHY coupled to the bench passing,
  capabilities refusing or skipping `set_scrambling(true)`, a probe that cannot tell
  skipping its points, a scrambler for high rates only at 340 000 kHz and just above, the
  async forms; and PHYs broken one way each caught at their point —
  starting unconfigured, `configure` keeping the output running, `stop_output` failing
  while off, `configure` not leaving FRL, `power_on` remembering the configuration,
  `set_scrambling(true)` refused at high rates.
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

`fuzz/fuzz_targets/bench.rs` drives a bench with arbitrary sequences of bus calls, PHY calls,
line changes, HPD, plug and adapter changes, presets, faults and clock advances, on a sink whose
settings and bus capabilities the input chooses; it decodes the input bytes itself. After
every action it checks the invariants a simulator must keep:

- the same input gives the same transcript (each input runs twice);
- nothing panics and no arithmetic overflows (the budget is off; an input runs at most 256
  actions);
- `Scrambling_Status` is set only while `Scrambling_Enable` is set and the line is
  scrambled TMDS at the matching ratio, and that ratio fits the rate (no action presets
  the status; the input's lines include ratios that do not fit);
- once the lock's conditions have held for a time trigger, the status is set (for sinks
  neither strict nor refusing, after an advance or a delay, with HPD high; the input picks
  each register group's HPD reset);
- no `Register` event changes a register the sink owns by `Actor::Source`;
- the segment pointer never outlives its command sequence: the EDID's segment 0 holds
  0x00–0x7F and segment 1 0x80–0xBF, so every EDID read at 0x50 that succeeds must hold no
  byte of segment 1;
- the transcript's times never go backwards.

CI runs the target for 60 seconds on every push and pull request, and for an hour weekly
and on demand; the weekly run minimises the corpus and opens a `ci/fuzz-corpus` pull
request with it. A crash is uploaded as an artifact.

## Coverage

CI measures line coverage without features, with `std` and with `async`, merged into one
report, against `.coverage-baseline` (see [`setup.md`](setup.md)).
