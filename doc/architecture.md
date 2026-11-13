# Architecture

> **Design.** This document describes `maquette` as designed, before it is built; it becomes
> the description of the released crate when the design ships. Points marked *To settle* are
> decided when the part they belong to is built. Claims about HDMI behaviour cite at least
> two independent public implementations of HDMI sources, or say that they do not and are
> settings (see "Sources").

## Role

`maquette` is test support: a simulated HDMI sink, at the level of its registers and its DDC
bus, with a simulated clock, so that code above the hardware traits runs against a sink that
behaves like one — with its delays, its refusals and its faults — under deterministic time.
It also carries conformance checks that an implementor of
[hdmi-hal](https://github.com/ashlar-display/hdmi-hal)'s traits runs against their
implementation.

Its users are procedures written over hdmi-hal's traits, in their tests: TMDS bring-up,
tear-down and checks ([spiritlevel](https://github.com/ashlar-display/spiritlevel)), the EDID
read, SCDC access through [culvert](https://github.com/ashlar-display/culvert), and a link
orchestrator's whole-session scenarios; and implementors of hdmi-hal's traits — a driver, a
firmware image, a userspace backend — checking their implementation.

It is not a sink implementation: it models what a source can observe of a sink, not what a
sink must do internally.

---

## Scope

`maquette` covers:

- **`SimClock`:** hdmi-hal's `Clock` and `Delay` on simulated time, standalone or held by a
  bench.
- **A register-level sink:** targets at DDC addresses — the SCDC register file, an EDID store
  behind the E-DDC segment pointer, a DisplayID store — and the sink's behaviour over time.
  A test adds its own targets.
- **`DdcBus` over the sink,** with capabilities a test chooses.
- **The line:** what the source transmits, as the sink sees it. maquette's simulated PHY
  (`TmdsPhy`, `FrlPhy`, `PhyPower`) drives it; `Coupled<P>` lets any PHY drive it; a test can
  set it directly.
- **Test controls:** register presets, HPD, unplug and replug with a new EDID, detach,
  `advance`, an operation budget, and one transcript of everything.
- **Fault injection:** NACKs with their source, timeouts, busy, detach, slow operations, at a
  point before, after or partway through an operation.
- **Conformance checks** for implementors of hdmi-hal's traits: `Clock` and `Delay`,
  `DdcBus`, `ScdcTransport`, `TmdsPhy`, `FrlPhy` and `PhyPower`.
- **The async traits** (hdmi-hal-async) on the same handles, including their `Send` twins.

Planned before the first release: a harness that cancels async procedures at every await
point; the sink's FRL training responses; a message-level sink over embedded-hal 1.0's `I2c`,
for checking a `DdcBus` at the wire; the CED counters, with torn reads.

Later: an HDCP port, with the stack's HDCP support (until then a test adds one as a target);
checks for the video pipeline and packet transmission traits, when those traits exist.

It does not cover:

- **Checks for traits defined by maquette's users,** such as spiritlevel's `TmdsScdc` or
  culvert's `ScdcClient`. Those crates use maquette in their tests, so a check here would
  make maquette depend on them, and a test build would hold two copies of each. Such checks
  live with the trait and run against maquette's sink. For the same reason hdmi-hal and
  hdmi-hal-async cannot use maquette in their own tests and keep their own doubles.
- **A sink's internals:** video decoding, the sink's own EDID policy, CEC.
- **Real hardware:** what a backend must be checked for on a board is that backend's
  documentation, not code here.

---

## Dependencies and features

```toml
[features]
std = ["critical-section/std"]
async = ["dep:hdmi-hal-async"]

[dependencies]
hdmi-hal = { default-features = false }
hdmi-hal-async = { default-features = false, optional = true }
critical-section = "1.2"
```

Dependencies point only down the stack.

- **hdmi-hal** (always): the traits maquette implements and checks — `Clock`, `Delay`,
  `Timestamp`; `DdcBus`, `DdcCapabilities`, `DdcError`; `ScdcTransport`, `ScdcOverDdc`;
  `TmdsPhy`, `FrlPhy`, `PhyPower`. maquette's API is made of these types and re-exports them,
  so a test needs no other dependency and names the same versions; each breaking hdmi-hal
  release is therefore a breaking maquette release.
- **hdmi-hal-async** (`async`): the async traits and their `Send` twins, on the same handles.
- **critical-section:** the lock around the bench's state. maquette's `std` feature enables
  its `std` implementation; on a bare target the user supplies one, as embedded code does.
- **embedded-hal 1.0** (planned, behind a feature): the message-level sink.

Two dependencies are left out on purpose:

- **Not culvert.** The SCDC register addresses and bits are written from public sources and
  cited in the code, so a wrong address in culvert does not reappear here and pass. Tests
  that go through culvert's client meet an independently written register map.
- **Not piaf.** The EDID store holds bytes; a test builds them however it likes.

---

## Model

### One bench, several handles

The sink's state, the clock and the line are one simulation: the sink sets
`Scrambling_Status` only once scrambled TMDS at the right ratio has run long enough after the
write. A `Bench` holds all three; the trait implementations are `Copy` handles borrowed from
it:

```rust
let bench = Bench::new(SinkConfig::new(), &edid_bytes)?;
let bus = bench.ddc();     // impl DdcBus
let phy = bench.phy();     // impl TmdsPhy + FrlPhy + PhyPower; two phy() handles are one PHY
let time = bench.time();   // impl Clock + Delay
let scdc = culvert::Scdc::new(ScdcOverDdc::new(&bus).ok().unwrap());
let mut link = TmdsLink::new(scdc, phy, time, time);   // spiritlevel, `culvert` feature
// … run the procedure …
bench.hpd_pulse(Duration::from_millis(500));   // the test's controls
assert_eq!(bench.sink().tmds_config(), 0);
```

The time handle implements both `Clock` and `Delay`, so one value is passed as both, as
spiritlevel's `TmdsLink::new(scdc, phy, clock, delay)` expects. SCDC is hdmi-hal's
`ScdcOverDdc` over the bench's bus, the path real backends take; maquette has no
`ScdcTransport` of its own. Procedure tests go through culvert's client, as above.

There is one value per trait, each borrowing the bench, so a test reads as the wiring of a
real source. Implementing every trait on `&Bench` would also work, but a test would pass the
same value as bus, PHY and clock.

### `Bench`'s signature

```rust
pub struct Bench<'a, const LOG: usize = 256, const FAULTS: usize = 16> { /* private */ }

impl<'a> Bench<'a> {
    /// A bench with the default capacities: `config`'s sink with `edid`, a bus with
    /// `DdcCapabilities::general()`, and a PHY with the default capabilities.
    pub fn new(config: SinkConfig, edid: &'a [u8]) -> Result<Self, BenchError>;

    /// A builder for everything else, starting from the defaults `new` uses.
    pub fn builder(edid: &'a [u8]) -> BenchBuilder<'a>;
}

impl<'a, const LOG: usize, const FAULTS: usize> Bench<'a, LOG, FAULTS> {
    pub fn ddc(&self) -> Ddc<'_>;      // DdcBus
    pub fn phy(&self) -> Phy<'_>;      // TmdsPhy, FrlPhy, PhyPower
    pub fn time(&self) -> Time<'_>;    // Clock, Delay
    pub fn sink(&self) -> Sink<'_>;    // reads the sink's registers and state

    // The test's controls (HPD, plug, detach, advance, presets, faults, the transcript)
    // take `&self` too: a test calls them while the handles are lent to the procedure.
}

#[must_use]
pub struct BenchBuilder<'a, const LOG: usize = 256, const FAULTS: usize = 16> { /* private */ }

impl<'a, const LOG: usize, const FAULTS: usize> BenchBuilder<'a, LOG, FAULTS> {
    pub fn sink(self, config: SinkConfig) -> Self;
    pub fn displayid(self, store: &'a [u8]) -> Self;
    pub fn ddc_capabilities(self, capabilities: DdcCapabilities) -> Self;
    pub fn phy_capabilities(self, capabilities: TmdsPhyCapabilities) -> Self;
    pub fn clock(self, clock: SimClock) -> Self;
    pub fn targets(self, targets: &'a mut [&'a mut (dyn Target + Send)]) -> Self;
    pub fn capacities<const L: usize, const F: usize>(self) -> BenchBuilder<'a, L, F>;
    pub fn build(self) -> Result<Bench<'a, LOG, FAULTS>, BenchError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BenchError {
    /// The EDID store is not a whole number of 128-byte blocks, or is longer than 256.
    EdidLength(usize),
    /// The DisplayID store is not a whole number of 128-byte blocks, or is longer than 256.
    DisplayIdLength(usize),
}
```

- **Capacities are const generics with defaults.** `LOG` is the transcript's capacity in
  records, `FAULTS` the fault schedule's. The records are stored inline, so the bench needs no
  `alloc`. `new` and `builder` are defined for the defaults, so a host test never names one;
  a bare target with a small stack picks smaller ones with
  `Bench::builder(&edid).capacities::<32, 4>()`. A full record counts its overflow and never
  panics.
- **The stores and the targets are borrowed** for `'a`: the EDID and DisplayID bytes, and the
  test's own targets. Nothing is copied into the bench, and the bench needs no `alloc`.
- **The handles name no capacity.** `Ddc<'_>`, `Phy<'_>`, `Time<'_>` and `Sink<'_>` reach the
  bench through a private interface, so a function that takes a handle works with a bench of
  any size. Every handle is `Copy`, `Send` and `Sync`.
- **What later parts add, they add as builder methods** (the PHY's settings, the bus's
  timing, the operation budget) and as `#[non_exhaustive]` fields and variants, without
  changing the signature above.
- **`BenchError`** implements `Display` and `core::error::Error`.

`Ddc`'s error is maquette's `BusError`: a `DdcErrorKind` with the operation and the address
that failed, implementing `DdcError`. `Phy`'s is `PhyError`: a kind and whether the call took
effect.

*To settle:* whether an empty EDID store is accepted, as a sink whose EDID does not answer.

### State and threads

The bench's state sits in a `critical_section::Mutex<RefCell<…>>`, so the bench is `Sync` and
every handle `Send + Sync`: the hdmi-hal-async `Send` twins are implemented on the same
handles, and a test of `Send` futures can use them. maquette needs no `alloc` and no `unsafe`
for this; on a host the `std` feature enables critical-section's `std` implementation, and on
a bare target the user supplies one. The state is borrowed only inside
`critical_section::with`, a synchronous closure, so no borrow is ever held across an
`.await`.

### Time

`SimClock`'s `now` starts at zero and moves only when something advances it:

- a `delay(d)`: by `d`, or by `d` plus a configured overshoot, so deadline arithmetic is
  tested against a delay that sleeps long, as a real one may;
- a bus transfer: a charge per transfer and per byte, by default the byte time at 100 kHz,
  I²C standard mode's maximum clock (DDC/CI 1.1), so the fastest a DDC bus runs; a slow sink
  is a setting. A timeout fault charges the bus's `stretch_timeout_us` where it has one;
- the test (`advance(d)`).

`now` never moves backwards, never reads the host, and saturates rather than overflowing. A
tick setting gives it a coarse resolution, as some platform clocks have: `now` reads in
whole ticks, rounded down, and a delay is rounded up to whole ticks, so `now` read after
`delay(d)` is always at least `d` later than before it.

```rust
pub struct SimClock { /* private */ }   // Send + Sync

impl SimClock {
    pub const fn new() -> Self;                                // at zero, 1 µs tick
    pub const fn with_tick(self, tick: Duration) -> Self;
    pub const fn with_overshoot(self, overshoot: Duration) -> Self;
    pub const fn tick(&self) -> Duration;
    pub const fn overshoot(&self) -> Duration;
    pub fn advance(&self, duration: Duration);
}

impl Clock for SimClock {}   impl Delay for SimClock {}
impl Clock for &SimClock {}  impl Delay for &SimClock {}
```

Standing alone, the clock keeps its time behind a `critical_section::Mutex`, so it is `Sync`
and can be a `static`. `Clock` and `Delay` are also implemented for `&SimClock`, so one clock
is passed as both where a procedure takes them as separate values (`&clock`,
`&mut &clock`), as hdmi-hal's time module describes for test doubles. Durations are rounded
up to whole microseconds, the resolution of hdmi-hal's `Timestamp`.

**When things are evaluated.** The sink evaluates its behaviour at every bench event: a bus
transfer, a PHY call, a line change, an HPD change, `advance`. Nothing runs in the
background, so a run is a function of the calls and the settings. A transfer is evaluated at
its start and charged after it; an operation the bus's capabilities refuse touches nothing
and is not charged; a faulted one is charged. A `read_segmented` that the bus splits into
pieces (`segmented_piece`) is several transfers, each charged, so something can change
between pieces, which an EDID read has to catch.

**Torn reads.** By default the sink holds its registers still during a transfer (*chosen*:
sinks of both kinds exist, and no source settles which is common). A sink that does not is a
setting, as is a bus that `splits_reads`: each byte is then evaluated at its own moment, the
transfer's start plus the per-byte charge so far. Only registers that change by themselves
can tear — the CED counters and FRL's LTP requests — so the setting arrives with them.

**The operation budget.** A bench counts bus operations, PHY calls and clock reads against a
budget, configurable or off. Past it the bench panics, naming the budget: a procedure that
loops without letting time move fails its test instead of hanging CI. A budget that failed
operations instead could let the procedure end quietly and the test pass.

*To settle:* the budget's default size.

### Settings

One `SinkConfig`, `#[non_exhaustive]`, with a `const fn new()` (an SCDC sink with a segment
pointer, `Sink_Version` 1, and the defaults in "Sink behaviour") and `const` `with_*`
builders for each setting (`without_scdc()`, `without_segment_pointer()`,
`with_scrambling_delay(d)`, `with_tmds_config_reset(rule)`, …), as the stack's other
capability and option types are built (`DdcCapabilities`, `TmdsPhyCapabilities`). There are
no named sink models: a name would suggest a real product whose behaviour no source backs.
The bus's and the PHY's capabilities are hdmi-hal's own types, passed to the bench, not
copied into `SinkConfig`.

### Targets

The sink is a set of targets at DDC addresses, which the bench routes operations to. A
public `Target` trait — `write`, `write_read` and `read` against the bench's time, with
access to the transcript for the target's own events — is implemented by the built-in
targets and by a test's own:

- **SCDC (0x54):** a 256-byte register file. Which registers the source writes, which the
  sink owns and which are write-1-to-clear are cited in the code from public sources. The
  registers modelled first are those TMDS bring-up uses: `Sink_Version`, `Source_Version`,
  `Update_0`, `TMDS_Config`, `Scrambling_Status`, `Config_1`.
- **EDID (0x50) behind the segment pointer (0x30),** and optionally **DisplayID (0x52):**
  the caller's bytes, a whole number of 128-byte blocks, up to 256. The pointer resets when a
  NACK or STOP is received (E-DDC 1.3 §8.1.1, Table 9-1) — so a 0x30 write in one call does
  not carry into the next, because the first call ended in STOP — and at the end of each
  command sequence (§2.2.5). The word offset persists between calls: a read with no offset
  written reads at the current address (§6.1). A sink without a segment pointer does not
  acknowledge its address byte (§8.1.2), `NoAcknowledge(Address)`, and answers segment 0
  reads (§9.5.1).

A test adds targets: DDC/CI at 0x37, an HDCP port, a vendor's register quirk. Register
presets set any register, the sink-owned ones too.

*To settle with the targets:* the `Target` trait's exact methods, and how a test's target
and a built-in one at the same address are routed.

### The line

What the source transmits, as the sink sees it: running or not, TMDS or FRL, the character
rate and clock ratio, scrambled or not, each change with its time. It is public, and three
things can drive it:

- maquette's simulated PHY (the default);
- `Coupled<P>`, which wraps any `TmdsPhy` (a third party's, a forwarding stub, a firmware
  PHY under hardware-in-the-loop), forwards each call and drives the line from what
  succeeded;
- the test, directly.

### HPD, unplug and detach

Three events a test needs to tell apart:

- **HPD** (`hpd(level)`, `hpd_pulse(d)`): the sink stays on the bus. What answers during HPD
  low is a setting per target: the EDID answers by default (*chosen*, after E-DDC's rule that
  the EDID is readable with the display powered off, §9.2, §9.5.3). What a long HPD low
  resets is a setting over every register group (`TMDS_Config`, the status and update flags,
  `Source_Version`, `Config_1`, later the CED counters).
- **Unplug and replug** (`unplug()`, `plug(..)`): while unplugged nothing answers,
  `NoAcknowledge(Address)` on every address. `plug` may bring a new EDID and new settings: a
  different sink coming back.
- **Detach** (`detach()`, `reattach()`): the adapter is gone; every operation fails
  `Detached`.

*To settle with the sink's behaviour:* whether SCDC answers during HPD low by default.

### The transcript

One ordered, typed record of every bus operation, every PHY call and line change, every
fault, and every decision the sink made on its own (reset `TMDS_Config`, ignored a write,
refused, cleared a bit, set the status), each with its time. Ordering tests — the sink
written before scrambled output starts, the FRL clear before `TMDS_Config`, the wait after a
ratio write — assert on it, and a failing test shows why the sink did what it did.
Behaviour can be triggered by a count of operations as well as by time.

---

## Sink behaviour

Each behaviour is settled by the sources (fixed) or a setting with a default; a default no
source settles is marked *chosen*.

| Behaviour | Sources | In maquette |
|---|---|---|
| `TMDS_Config` holds the source's `Scrambling_Enable` and `TMDS_Bit_Clock_Ratio` | settled [1] | fixed |
| `Scrambling_Status` set when scrambled TMDS runs at the ratio `TMDS_Bit_Clock_Ratio` names, with `Scrambling_Enable` set, for the delay after the later of the write and scrambled output starting | settled that sources read it [2]; the delay and the ratio match have no source | delay a setting, *chosen* default 1 ms; ratio match *chosen*; scrambling at 1/10 sets it too |
| A sink that does not lock when scrambled TMDS arrived before the write | sources that redo the whole mode set say some sinks would accept the SCDC writes alone; others restore with the output running [3] | a setting, off by default (the lenient sink) |
| "Some displays refuse to set the scrambling bit right away" | one source, which reads two ways [4] | a setting in both readings: the status withheld, or `Scrambling_Enable` dropped from the write; for the first *n* attempts, an attempt being a write that sets `Scrambling_Enable`; default none (*chosen*) |
| The sink clears `Scrambling_Enable` without scrambled TMDS within 100 ms of the write | one source [5] | a setting, default off |
| `TMDS_Config` and other registers reset after a long HPD low | settled that sources restore it [6]; when the sink resets it has no public source | a setting per register group: after HPD low for at least *d*, *chosen* default 100 ms; or never; or on every HPD low |
| `Source_Version` written by the source, `Sink_Version` reported | [7] | `Sink_Version` a setting (default 1); `Source_Version` stored |
| Update flags set by the sink and kept until the source clears them; `FLT_update` cleared by writing it back as 1 | the clear settled [8] | set by a preset at first; the rule that sets them a setting |
| A sink without SCDC | no source for its answer | NACKs 0x54 (*chosen*) |

*To settle with the targets, each with its sources or a chosen default:* a write to a
register the sink owns; a read past the EDID store, and a segment the sink does not have
(E-DDC 1.3 §6.4–§6.5 for segments that exist); an SCDC read past 0xFF; reading across the
128-byte boundary at 0x50 (E-DDC 1.3 §2.2.6: "some implementations may support" it).

Planned before the first release: FRL training responses (`FLT_ready`, LTP requests,
`FLT_update`, `FRL_start`), the CED counters (saturation at 0x7FFF, both clear-on-read
behaviours, held or moving during a burst, reset on hot plug) and `Status_Flags`.

---

## `DdcBus` over the sink

The bench's `ddc()` handle implements `DdcBus` with capabilities a test sets: by default
`DdcCapabilities::general()`, or narrower ones to test a procedure against a limited adapter,
such as an engine that reads only the EDID or only segment 0. It refuses what its
capabilities rule out with `Unsupported` before touching the sink, as `DdcBus`'s contract
asks; a test that wants a backend to misbehave uses a fault.

---

## The simulated PHY

`phy()` implements `TmdsPhy`, `FrlPhy` and `PhyPower` to hdmi-hal's contracts, including the
mode rule (entering one mode leaves the other):

- its power state at creation is a setting, **on** by default (*chosen*: `PhyPower`'s
  documentation says procedures never change the power, so a procedure test expects a
  powered PHY); creation in the on state counts as a power-on, so `start_output` after a
  `configure` works either way the contract is read;
- `start_output` needs a `configure` since the last power-on; `stop_output` works in every
  state; `set_scrambling(true)` fails on a PHY without a scrambler (`TmdsPhyCapabilities`);
- where the contract allows several behaviours — `configure` with the output running (fail,
  or stop it), the scrambler after `configure` (kept, reset, or set from the rate), the
  start-up latency — each is a setting, defaulting to the least forgiving one: a procedure
  must be correct under any behaviour the contract allows.

Its calls go into the transcript and drive the line. The coupling between the line and the
sink catches a procedure that never writes the sink or never starts scrambled output; it does
not judge order, which a test checks from the transcript.

---

## Faults

A test schedules faults: *on the n-th operation matching a filter* (an address, an operation
kind, a register; operations the capabilities refuse do not count), *for k operations* or
*until cleared*. A fault strikes at a point:

- **before** the sink sees the operation;
- **after** the sink applied it (a write latched, the error still returned);
- **partway:** a NACK on data byte *k*, the bytes before it applied.

A fault fails with a `DdcErrorKind`; a NACK carries its `NoAcknowledgeSource`; a timeout
charges the clock. An `Unsupported` fault breaks the `DdcBus` contract on purpose, to test
code against a misbehaving backend, and its documentation says so. A failed read fills the
buffer with a pattern, so a caller that uses it after an error fails (the contract leaves the
contents unspecified). PHY faults use `PhyError`, with a kind and whether the call took
effect. Faults are applied after the capability check and recorded in the transcript.

---

## Conformance checks

Functions an implementor calls in a test, with their implementation and what is on the other
side of it. Each returns a report of the points that failed, as typed values, rather than
panicking at the first. Reports have a fixed capacity and count their overflow; one
assertion helper formats every failed point. A panic inside the implementation under test is
not reported: without `std` it ends the check.

- **`Clock` and `Delay`:** `now` never decreases; `delay(d)` lets at least `d` pass on the
  implementation's own clock, and `now` advances by at least `d` across it, so a frozen
  clock fails.
- **`DdcBus`:** `Unsupported` for the operations the check drives that the capabilities rule
  out; empty buffers; a segment 0 read from a display without a segment pointer; with `std`,
  concurrent calls from several threads without a panic.
- **`ScdcTransport`:** over a known register image, `read_block` agrees with `read` register
  by register; an empty buffer is `Ok`; 16 bytes are accepted wherever they fit; a read past
  0xFF returns without a panic; the transport works after an error, where the check can cause
  one. Whether a block read is one transaction on the wire cannot be checked from outside.
- **`TmdsPhy`, `FrlPhy` and `PhyPower`:** `stop_output` in every state; `start_output` only
  after `configure`; `set_scrambling` with the output on and off; `configure` leaving the
  output stopped; `power_on` leaving it stopped; after `power_off`, `configure` again before
  `start_output`; the mode rule on a PHY implementing both mode traits. What the traits
  cannot show — whether the output is running — comes from a probe the implementor passes.

**What each can see.** Run against the bench, everything above. Against Linux's `i2c-stub`
or a board, only what the trait returns: an allowed operation to an address where nothing
answers is `NoAcknowledge`, and an error the check cannot cause is not checked. On the
message-level sink, which implements embedded-hal 1.0's `I2c` (the lower bus a firmware
`DdcBus` most likely sits on), what reaches the wire, per call; "before bus activity" is
visible only there. embedded-hal's transactions have one address each, so the multi-address
transfers of an E-DDC read in one transaction are out of its reach, as they are out of such
an implementation's.

**Stability.** New check points come only in a breaking release, so an implementor's CI does
not turn red on `cargo update`.

maquette runs each check against its own implementations, and the checks' negative cases
against deliberately broken doubles.

*To settle before the checks are written:* their signatures — how clock and delay are passed
(as two values, the general platform case), how the known device and its image are
described, the PHY probe's type, the report type, and the async forms.

---

## `no_std`, `alloc`, and features

`#![no_std]` without `alloc` in every build, like the procedures it tests, and
`#![forbid(unsafe_code)]`.

- **`std`:** critical-section's `std` implementation, and the concurrent-call `DdcBus`
  check (threads). Not a default feature, as across the stack: a host test enables it.
- **`async`:** hdmi-hal-async's traits and their `Send` twins on the same handles; the
  cancellation harness when it comes.
- **`embedded-hal`** (planned): the message-level sink.

CI builds each `no_std` feature set for a bare target (`thumbv7em-none-eabi`): a host build
without `std` does not prove `no_std`, because `std` is still available to dependencies.

---

## Testing

See [`testing.md`](testing.md). maquette's own tests check its implementations against the
conformance checks, the checks' negative cases against broken doubles, and the sink's rules
against the sources above. A fuzz target drives the bench with arbitrary sequences of bus and
PHY calls, line changes, faults, controls and clock advances, and checks the invariants a
simulator must keep: the same input gives the same transcript; nothing panics and no
arithmetic overflows; the status is set only under the coupling's conditions; registers the
sink owns change only by the sink's rules; the segment pointer never outlives its command
sequence.

---

## Design Principles

- **Deterministic time, evaluated at events.** Behaviour is evaluated at every bench event
  against `SimClock`, with no background thread and no host time, so a run is a function of
  its inputs.
- **Unsourced behaviour is a setting.** A sink behaviour no public source settles is never a
  fixed rule; it is a setting with a default marked *chosen*.
- **Cited registers.** Register addresses and bits come from public sources, cited in the
  code, not from the crates under test.
- **The real path.** SCDC runs through `ScdcOverDdc` over the bench's bus, as on real
  backends.
- **Diagnostics are first-class.** One typed transcript of everything the bench and the sink
  did, and checks that report every failed point instead of panicking at the first.
- **No `alloc`.** Capacities are const generics with defaults; every record counts its
  overflow.
- **Interfaces, not implementations.** maquette implements and checks hdmi-hal's traits
  only, so nothing that uses it in tests is a dependency of it.

---

## What comes next

The parts of this design in the order they are built, and what follows them, are on the
[roadmap](roadmap.md).

---

## Sources

Driver source read at these commits; line numbers refer to them.

- Linux, github.com/torvalds/linux `e9954f3fbddd` (2026-10-31)
- Xilinx embeddedsw, github.com/Xilinx/embeddedsw `97f2baf7f69e` (2026-06-24)
- Amlogic, github.com/CoreELEC/common_drivers `8ca9b6c3b715` (2026-10-31)

|     | Source | Location |
|-----|--------|----------|
| [1] | `TMDS_Config` written by the source | Linux's SCDC helper `drivers/gpu/drm/display/drm_scdc_helper.c:221–245`, `277–307` (its callers include i915 and vc4); amdgpu `drivers/gpu/drm/amd/display/dc/link/protocols/link_ddc.c:602–613`; nouveau `drivers/gpu/drm/nouveau/dispnv50/disp.c:800–812`; Xilinx `XilinxProcessorIPLib/drivers/v_hdmitx1/src/xv_hdmitx1.c:643–680`; Amlogic `drivers/media/vout/hdmitx21/hw/hdmi_tx_hw.c:2889–2904` |
| [2] | `Scrambling_Status` read after enabling scrambling | i915 `drivers/gpu/drm/i915/display/intel_hdmi.c:2749–2754`; DRM helper and vc4 (one lineage) `drivers/gpu/drm/display/drm_hdmi_helper.c:484–512`, `drivers/gpu/drm/vc4/vc4_hdmi.c:863–878`; Tegra `drivers/gpu/drm/tegra/sor.c:2183–2193` |
| [3] | Restoring with the output running | i915, vc4 and the DRM helper (one lineage), comment that some sinks would accept the SCDC writes alone: `drivers/gpu/drm/i915/display/intel_ddi.c:4871–4880`, `drivers/gpu/drm/vc4/vc4_hdmi.c:344–354`, `drivers/gpu/drm/display/drm_hdmi_helper.c:712–726`; Xilinx `v_hdmitxss1/examples/xhdmi_example/PassThrough/xhdmi_exdes_sm_tx.c:1394–1403`, `v_hdmitxss1/src/xv_hdmitxss1.c:2213–2217`; Amlogic `drivers/media/vout/hdmitx21/hdmi_tx_main.c:5053–5062`, `hw/hdmi_tx_hw.c:2972–2985` |
| [4] | Displays that refuse to set the scrambling bit right away | DRM helper `drivers/gpu/drm/display/drm_hdmi_helper.c:521–523` |
| [5] | The sink disabling scrambling without a scrambled clock within 100 ms | i915 `drivers/gpu/drm/i915/display/intel_hdmi.c:2767–2770` |
| [6] | Restoring `TMDS_Config` when the sink may have lost it | i915, vc4 and the DRM helper (one lineage) `drivers/gpu/drm/i915/display/intel_ddi.c:4851–4880`, `drivers/gpu/drm/vc4/vc4_hdmi.c:276–354`, `drivers/gpu/drm/display/drm_hdmi_helper.c:640–641` (the sink's SCDC status "is reset and must be explicitly restored"); Amlogic `drivers/media/vout/hdmitx21/hdmi_tx_main.c:5053–5062`, `4786–4796` |
| [7] | `Source_Version` and `Sink_Version` | amdgpu `drivers/gpu/drm/amd/display/dc/link/protocols/link_ddc.c:592–600` (when `Sink_Version` is 1); Linux's SCDC helper `drivers/gpu/drm/display/drm_scdc_helper.c:322–350`; dw-hdmi `drivers/gpu/drm/bridge/synopsys/dw-hdmi.c:2112–2115`; Xilinx `XilinxProcessorIPLib/drivers/v_hdmitx1/src/xv_hdmitx1.c:789–806` |
| [8] | `FLT_update` cleared by writing it back as 1 | amdgpu `drivers/gpu/drm/amd/display/dc/link/protocols/link_hdmi_frl.c:178–200`; Amlogic `drivers/media/vout/hdmitx21/hdmi_tx_main.c:446–463` |

Standards (VESA, public):

- **E-DDC 1.3:** the segment pointer (§2.2.5) and its reset on NACK or STOP (§8.1.1,
  Table 9-1); a display without one (§8.1.2, §9.5.1); the current-address read (§6.1);
  segments (§6.4–§6.5); reading across 128 bytes (§2.2.6); the EDID readable with the
  display powered off (§9.2, §9.5.3).
- **DDC/CI 1.1:** I²C standard mode, at most 100 kHz.
