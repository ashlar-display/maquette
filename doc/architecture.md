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
assert_eq!(bench.sink().tmds_config(), Some(0));
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

    /// Sets an SCDC register, the sink's own included; recorded as by `Actor::Test`. A bench
    /// event: the sink evaluates its behaviour after it. Panics on a sink without SCDC.
    pub fn preset_scdc(&self, reg: u8, value: u8);

    /// Moves the bench's time forward, then lets every target see it (`Target::update`).
    pub fn advance(&self, duration: Duration);
    /// The bench's time.
    pub fn now(&self) -> Timestamp;

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
    pub fn bus_timing(self, timing: BusTiming) -> Self;
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
- **Settings are builder methods** (the PHY's settings, the bus's timing, the operation
  budget), and the types grow through `#[non_exhaustive]` fields and variants, so what is
  added later leaves the signature above unchanged.
- **`BenchError`** implements `Display` and `core::error::Error`.

`Ddc`'s error is maquette's `BusError`: a `DdcErrorKind` with the operation and the address
that failed, implementing `DdcError`. `Phy`'s is `PhyError`: a kind and whether the call took
effect.

An empty EDID store is accepted: a sink whose EDID does not answer, `NoAcknowledge(Address)`
at 0x50 (*chosen*, the one way to model a sink without EDID). The DisplayID store is absent
unless given, and 0x52 then does not answer either.

### State and threads

The bench's state sits in a `critical_section::Mutex<RefCell<…>>`, so the bench is `Sync` and
every handle `Send + Sync`: the hdmi-hal-async `Send` twins are implemented on the same
handles, and a test of `Send` futures can use them. maquette needs no `alloc` and no `unsafe`
for this; on a host the `std` feature enables critical-section's `std` implementation, and on
a bare target the user supplies one. The state is borrowed only inside
`critical_section::with`, a synchronous closure, so no borrow is ever held across an
`.await`.

Calls from several threads are serialized: a bus call (the fault count, the routing, the
records and the clock's charge) and a delay with the event after it each run in one
critical section, which nests, so no other handle's call falls between their steps. The
transcript is in time order and the *n*-th matching call a fault counts is the *n*-th in
the transcript, whichever thread made it. A PHY call already runs under one lock; a call
through `Coupled` runs the test's own PHY outside it, since that PHY may call the bench.

### Time

`SimClock`'s `now` starts at zero and moves only when something advances it:

- a `delay(d)`: by `d`, or by `d` plus a configured overshoot, so deadline arithmetic is
  tested against a delay that sleeps long, as a real one may;
- a bus transfer: a charge per transfer and per byte, by default the byte time at 100 kHz,
  I²C standard mode's maximum clock, which sources use [14]; a faster or slower bus is a
  setting. A timeout fault charges the bus's `stretch_timeout_us` where it has one;
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
transfer, a PHY call, a line change, an HPD, plug or adapter change, a delay on the bench's
time handle (after it), `advance`. Nothing runs in the background, so a run is a function of
the calls and the settings. A decision that falls due between two events (the status set
1 ms after the write, a reset after 100 ms of HPD low) is taken at the first event at or
after it, and recorded at that event's time: a read always sees it, since a read is an
event, and the transcript stays in time order. A transfer is evaluated at
its start and charged after it; an operation the bus's capabilities refuse touches nothing
and is not charged; a faulted one is charged. A `read_segmented` that the bus splits into
pieces (`segmented_piece`) is several transfers, each charged, so something can change
between pieces, which an EDID read has to catch.

**Torn reads.** By default the sink holds its registers still during a transfer (*chosen*:
sinks of both kinds exist, and no source settles which is common). A sink that does not is a
setting, as is a bus that `splits_reads`: each byte is then evaluated at its own moment, the
transfer's start plus the per-byte charge so far. Only registers that change by themselves
can tear — the CED counters and FRL's LTP requests — so the setting arrives with them.

**The operation budget.** A bench counts every operation through its handles — bus calls,
refused ones included, PHY calls, clock reads and delays — against a budget,
`BenchBuilder::budget(Option<u64>)`: by default `DEFAULT_BUDGET`, 1 000 000 operations
(*chosen*: far more than a bring-up, a training run or a long session of polling takes,
and spent within seconds by a loop that never ends), `None` for none. Past it the bench
panics, naming the budget and how many operations it allowed: a procedure that loops
forever fails its test instead of hanging CI. A budget that failed operations instead could
let the procedure end quietly and the test pass. A `SimClock` standing alone counts
nothing.

### Settings

One `SinkConfig`, `#[non_exhaustive]`, with a `const fn new()` (an SCDC sink with a segment
pointer, `Sink_Version` 1, and the defaults in "Sink behaviour") and `const` `with_*`
builders for each setting (`without_scdc()`, `without_segment_pointer()`,
`with_scrambling_lock(trigger)`, `with_strict_lock(..)`, `with_scrambling_refusals(..)`,
`with_enable_timeout(..)`, `with_hpd_reset(group, rule)`, `with_scdc_during_hpd_low(..)`,
`with_edid_during_hpd_low(..)`,
…), with a getter for each, as the stack's other
capability and option types are built (`DdcCapabilities`, `TmdsPhyCapabilities`). There are
no named sink models: a name would suggest a real product whose behaviour no source backs.
The bus's and the PHY's capabilities are hdmi-hal's own types, passed to the bench, not
copied into `SinkConfig`.

### Targets

The sink is a set of targets at DDC addresses, which the bench routes transactions to. A
target sees what a device on the bus sees: transactions, each a sequence of writes and reads
to addresses, ending in STOP. The built-in targets implement the same public trait a test's
own do:

```rust
pub trait Target {
    /// Whether this target answers at `addr` now (HPD may matter: `env.hpd()`).
    fn answers(&self, addr: u8, env: &Env<'_>) -> bool;

    /// One transaction: `ops` in order, ending in STOP. The target applies what it accepts
    /// and fills every read; an error says how the transaction failed, and what the target
    /// applied before it stays applied.
    fn transaction(&mut self, ops: &mut [Operation<'_>], env: &mut Env<'_>)
        -> Result<(), DdcErrorKind>;

    /// A bench event that is not a transaction: time moved, HPD or the line changed.
    fn update(&mut self, env: &mut Env<'_>) {}
}

#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Operation<'b> {
    Write { addr: u8, bytes: &'b [u8] },
    Read { addr: u8, buf: &'b mut [u8] },
}

/// The bench as a target sees it, for the length of one call.
pub struct Env<'e> { /* private */ }

impl Env<'_> {
    pub fn now(&self) -> Timestamp;
    pub fn hpd(&self) -> bool;
    /// Records one of the target's own events in the transcript.
    pub fn note(&mut self, addr: u8, code: u32, value: u32);
}
```

The bench turns each `DdcBus` call into one transaction: `write` and `read` into one
operation, `write_read` into a write and a read, and a piece of `read_segmented` into a
write of the segment to 0x30 (for segment 1 and higher), a write of the offset and a read.
The line joins `Env` with the PHY.

**Routing.** While the sink is unplugged or the adapter detached, the bench answers for it
(see "HPD, unplug and detach"). Otherwise a transaction goes to the first target that
answers the address of its first operation, the test's targets first, in the order given,
then the built-in ones; when none answers, it fails `NoAcknowledge(Address)` and no target
sees it. The target that takes a transaction takes all of it: a later operation to an
address it does not answer is its to refuse, as a device that does not acknowledge. So a
test's target at 0x54 replaces the built-in SCDC, and one that replaces the EDID for E-DDC
reads answers 0x30 as well as 0x50.

The built-in targets:

- **SCDC (0x54):** a 256-byte register file [9]. A write sets the offset with its first
  byte, then writes from there; a read starts at the offset, which advances with every byte,
  and persists between transactions (*chosen*: every source surveyed writes the offset
  before reading). What a byte written does depends on the register:
  - **the source's** — `Source_Version` (0x02), `TMDS_Config` (0x20), `Config_0` (0x30),
    `Config_1` (0x31), `Test_Read_Request` (0xC0) and the manufacturer's block past its
    identification (0xDE–0xFF, which Amlogic marks `RW`) [12]: stored as written;
  - **update flags** — `Update_0` (0x10) and `Update_1` (0x11): a 1 written clears that
    flag [8];
  - **the sink's** — `Sink_Version` (0x01), `Scrambling_Status` (0x21),
    `Source_Test_Configuration` (0x35, the sink's test requests, which sources only read)
    [12], the status flags (0x40–0x42), the error counters (0x50–0x5A), the manufacturer's
    identification (0xD0–0xDD): by the setting in "Sink behaviour";
  - **any other register:** stored as written (*chosen*, until it is modelled).

  At creation every register is 0 but `Sink_Version`, which is the setting (1 by default).
  Every byte written is recorded as a `Register` event, with its value before and after, or
  as an `IgnoredWrite`. A test reads the registers through `bench.sink()` (`scdc(reg)`,
  `tmds_config()`, `scrambling_status()`, …, each `None` for a sink without SCDC), which
  takes no bus operation and no time.
- **EDID (0x50) behind the segment pointer (0x30),** and optionally **DisplayID (0x52)**,
  one target: the caller's bytes, a whole number of 128-byte blocks, up to 256. A segment
  is 256 bytes (E-DDC 1.3 §2.2.5).
  - The pointer is 0 at the start of every transaction: it resets on a NACK or STOP
    (§8.1.1, Table 9-1) and at the end of each command sequence (§2.2.5), so a write to
    0x30 matters only to the reads in the same transaction. A read at 0x30 is refused,
    `NoAcknowledge(Address)` (*chosen*: §2.2.5 calls the pointer effectively write-only
    because it resets after each command sequence, and does not say what a read gets); of
    several bytes written to it, the first counts (*chosen*).
  - Each structure's word offset persists between transactions and advances with every
    byte read: a read with no offset written reads at the current address (§6.1, §6.3).
    Past 0xFF it wraps within the segment.
  - For segment 1 and higher the sink acknowledges the pointer value (§6.4–§6.5). A segment
    is present when either store has bytes in it, the pointer being shared; a store that
    lacks a present segment reads past its end.
  - A sink without a segment pointer does not answer 0x30, `NoAcknowledge(Address)`, and
    answers segment 0 reads (§8.1.2, Table 9-1).
  - A write to 0x50 or 0x52 sets the word offset with its first byte; the bytes after it
    are acknowledged and ignored without moving the offset, and recorded as `IgnoredWrite`:
    E-DDC does not require a display to support writes there (§6).
  - An empty store does not answer its address (*chosen*: a sink without that structure);
    without a DisplayID store, nothing answers 0x52.
  - The settings in "Sink behaviour" govern reads past the store, segments the sink does
    not have and the 128-byte boundary: `with_past_store`, `with_missing_segment`,
    `with_block_boundary`, and `without_segment_pointer`.

A test adds targets: DDC/CI at 0x37, an HDCP port, a vendor's register quirk. Register
presets set any register, the sink-owned ones too.

### The line

What the source transmits, as the sink sees it: running or not, TMDS or FRL, the character
rate and clock ratio, scrambled or not, each change with its time. It is public, and three
things can drive it:

- maquette's simulated PHY (the default);
- `Coupled<P>`, which wraps any `TmdsPhy` (a third party's, a forwarding stub, a firmware
  PHY under hardware-in-the-loop), forwards each call and drives the line from what
  succeeded;
- the test, directly, with `Bench::set_line`.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Line {
    /// Nothing transmitted.
    Off,
    /// TMDS at `character_rate_khz`, the clock at `ratio`, scrambled or not.
    Tmds { character_rate_khz: u32, ratio: TmdsClockRatio, scrambled: bool },
    /// FRL at `rate`; what it carries comes with the sink's FRL responses, so the variant
    /// is `#[non_exhaustive]`: built with `Line::frl(rate)`, matched with `..`.
    Frl { rate: HdmiForumFrl },
}

impl<'a, const LOG: usize, const FAULTS: usize> Bench<'a, LOG, FAULTS> {
    /// Sets what the source transmits: a bench event, recorded as `Event::Line`.
    pub fn set_line(&self, line: Line);
    pub fn line(&self) -> Line;
}
```

`TmdsClockRatio` and `HdmiForumFrl` are hdmi-hal's. A target sees the line through
`Env::line()`. Setting the line it already has is not a change: nothing is recorded and the
time it started stays.

### HPD, unplug and detach

Three events a test needs to tell apart:

```rust
impl<'a, const LOG: usize, const FAULTS: usize> Bench<'a, LOG, FAULTS> {
    pub fn set_hpd(&self, high: bool);
    /// HPD low, `low_for` on the bench's clock (`advance`), HPD high.
    pub fn hpd_pulse(&self, low_for: Duration);
    pub fn unplug(&self);
    pub fn plug(&self, edid: &'a [u8], displayid: Option<&'a [u8]>, config: SinkConfig)
        -> Result<(), BenchError>;
    pub fn detach(&self);
    pub fn reattach(&self);
    pub fn is_attached(&self) -> bool;
    /// How many operations the handles made: what the budget counts.
    pub fn operations(&self) -> u64;
}
```

- **HPD:** the sink stays on the bus; `bench.sink().hpd()` and `is_plugged()` read it.
  Raising HPD on an unplugged sink panics: the test plugs it in instead. What answers during
  HPD low is a setting per built-in
  target (`DuringHpdLow::{Answers, Silent}`): the EDID answers by default (settled: E-DDC
  1.3 §4.2.2 makes it available whenever +5 V is present, and DVI 1.0 §2.2.9.2 has HPD high
  only while it can be read, which does not forbid answering with HPD low; `Silent` is a
  sink whose EDID is unavailable), and so does SCDC (*chosen*: no source says a sink's SCDC
  stops answering). A test's own target decides with `Env::hpd()`. What an HPD low resets
  is a setting per register group ("Sink behaviour").
- **Unplug and replug:** while unplugged HPD is low and nothing answers, the test's own
  targets included: `NoAcknowledge(Address)` on every address, charging the address byte.
  `plug` brings a sink back — new stores and new settings, its registers fresh, HPD high:
  a different sink, or the same one after a power cycle. The test's own targets stay.
- **Detach:** the adapter is gone. Every `DdcBus` call that passes the argument and
  capability checks fails `Detached`, reaches no target and takes no time, until
  `reattach`.

Each is a bench event, recorded: `Event::Hpd { high }`, `Event::Plug { plugged }`,
`Event::Adapter { attached }`. A change to the state it already has records nothing.

### The transcript

One ordered, typed record of every bus operation, every PHY call and line change, every
fault, and every decision the sink made on its own (reset `TMDS_Config`, ignored a write,
refused, cleared a bit, set the status), each with its time. Ordering tests — the sink
written before scrambled output starts, the FRL clear before `TMDS_Config`, the wait after a
ratio write — assert on it, and a failing test shows why the sink did what it did.
Behaviour can be triggered by a count of operations as well as by time.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Record {
    /// When it happened: for a bus operation, when it started.
    pub at: Timestamp,
    pub event: Event,
}

/// Each variant with fields is `#[non_exhaustive]` too: the bench makes records, a test
/// matches them with `..`, and a variant may gain a field without a breaking change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Event {
    /// A `DdcBus` call: what was asked and how it ended, refused calls included.
    Bus(BusRecord),
    /// A register of a built-in target was written or set, by the source, the sink or a
    /// preset; `from` and `to` may be equal.
    Register { addr: u8, reg: u8, from: u8, to: u8, by: Actor },
    /// A built-in target ignored a write, and why.
    IgnoredWrite { addr: u8, reg: u8, value: u8, why: Ignored },
    /// A test's target's own event (`Env::note`).
    Note { addr: u8, code: u32, value: u32 },
    /// What the source transmits changed (`Bench::set_line`, or the PHY's output).
    Line(Line),
    /// HPD changed.
    Hpd { high: bool },
    /// The sink was unplugged or plugged.
    Plug { plugged: bool },
    /// The adapter was detached or reattached.
    Adapter { attached: bool },
    /// The sink refused to set the scrambling bit right away, at this attempt.
    Refusal { attempt: u32, kind: RefusalKind },
    /// A scheduled fault struck the bus operation recorded before it.
    Fault { id: FaultId, effect: Effect, point: Point },
    /// A call of the simulated or a coupled PHY, and how it ended.
    Phy { call: PhyCall, result: Result<(), PhyErrorKind> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct BusRecord {
    pub op: BusOp,        // Write, WriteRead, Read, ReadSegmented { target, segment }
    pub addr: u8,
    /// The first byte written (an offset or register), if any.
    pub first: Option<u8>,
    pub written: u16,
    pub read: u16,
    pub result: Result<(), DdcErrorKind>,
}

impl<'a, const LOG: usize, const FAULTS: usize> Bench<'a, LOG, FAULTS> {
    /// The records, oldest first; each is copied out under the lock.
    pub fn transcript(&self) -> impl Iterator<Item = Record> + '_;
    /// How many records did not fit, past `LOG`.
    pub fn transcript_overflow(&self) -> usize;
    pub fn clear_transcript(&self);
}
```

Records are `Copy` and hold no slices, so the transcript needs no `alloc`; a bus record keeps
the first byte written, which for SCDC and the EDID is the register or offset, and the
register events carry the values. When the transcript is full, later records are counted,
not stored: the oldest are the ones a test of a bring-up asks about.

---

## Sink behaviour

Each behaviour is settled by the sources (fixed) or a setting with a default; a default no
source settles is marked *chosen*.

| Behaviour | Sources | In maquette |
|---|---|---|
| `TMDS_Config` holds the source's `Scrambling_Enable` and `TMDS_Bit_Clock_Ratio` | settled [1] | fixed |
| `Scrambling_Status` set when scrambled TMDS runs at the ratio `TMDS_Bit_Clock_Ratio` names, with `Scrambling_Enable` set, once the lock trigger has passed since the later of the write and scrambled output starting | settled that sources read it [2]; the delay and the ratio match have no source | the trigger a setting, `Trigger::After(d)`, *chosen* default 1 ms, or `Trigger::AfterReads(n)`, the *n*-th read of `Scrambling_Status`; ratio match *chosen*; scrambling at 1/10 sets it too |
| The ratio fits the rate: 1/40 above 340 Mcsc, 1/10 at or below | settled above 340 Mcsc [10]; no source uses 1/40 at or below | fixed: the sink does not lock on a line whose ratio does not fit its rate; at or below 340 Mcsc, 1/40 does not lock (*chosen*) |
| A sink that does not lock when scrambled TMDS arrived before the write | sources that redo the whole mode set say some sinks would accept the SCDC writes alone; others restore with the output running [3] | a setting, off by default (the lenient sink) |
| "Some displays refuse to set the scrambling bit right away" | one source, which reads two ways [4] | a setting in both readings: the status withheld, or `Scrambling_Enable` dropped from the write; for the first *n* attempts, an attempt being a write that sets `Scrambling_Enable`; default none (*chosen*) |
| The sink clears `Scrambling_Enable` without scrambled TMDS within 100 ms of the write | one source [5]; settled that a source resumes within 100 ms of a ratio change [11] | a setting, default 100 ms (the least forgiving), counted from the write that set `Scrambling_Enable` or one that changed `TMDS_Bit_Clock_Ratio` while it was set; `None` never gives up |
| A ratio change: the source suspends the TMDS clock and data, writes `TMDS_Bit_Clock_Ratio`, and resumes 1 to 100 ms after | settled [11] | the strict sink (below) does not take output resumed sooner than 1 ms after the write; the 100 ms is the timeout's |
| `TMDS_Config` and other registers reset around an HPD low | settled that sources restore it [6]; the public sinks reset `TMDS_Config` as HPD falls [13]; for the other registers one sink each way [13] | a setting per register group (`RegisterGroup::{TmdsConfig, Status, SourceVersion, Config}`): `HpdReset::OnHpdLow`, the default for every group (the least forgiving; *chosen* after Amlogic for all but `TMDS_Config`); `HpdReset::After(d)`; `HpdReset::OnHpdRise`, as Xilinx clears `FRL_Rate`, losing what was written during the low too; or `HpdReset::Never`. The `Status` group is `Scrambling_Status`, the update flags and the status flags (0x40–0x42). Each group resets once per HPD low, before the lock is evaluated; only registers that change are recorded |
| `Source_Version` written by the source, `Sink_Version` reported | [7] | `Sink_Version` a setting (default 1); `Source_Version` stored |
| Update flags set by the sink and kept until the source clears them; `FLT_update` cleared by writing it back as 1 | the clear settled [8] | set by a preset; the rule that sets them a setting, with the status flags and FRL responses they report |
| A sink without SCDC | one source: a failed `Source_Version` write means no SCDC [7] | NACKs 0x54 (*chosen*) |
| A write to an SCDC register the sink owns | no source | a setting: acknowledged and ignored, recorded (*chosen*), or `NoAcknowledge(Data)` at that byte |
| An SCDC read or write past 0xFF | left to the sink: hdmi-hal's `ScdcTransport` leaves it open, and no source covers it | a setting: the offset wraps to 0x00 (*chosen*, an 8-bit offset register), or `NoAcknowledge(Data)` past 0xFF |
| A read past the end of the EDID or DisplayID store, in a segment the sink has | E-DDC covers only the data a display has | a setting: reads 0xFF (*chosen*), wraps to the start of the store, or `NoAcknowledge(Data)` |
| A segment the sink does not have | E-DDC requires an acknowledge of the pointer for segments that exist (§6.4–§6.5), not what follows for others | a setting: `NoAcknowledge(Data)` at the pointer value (*chosen*, as hdmi-hal's `DdcBus` contract reads it), or acknowledged and read as past the store |
| Reading across the 128-byte boundary at 0x50 or 0x52 | "some implementations may support" it (E-DDC 1.3 §2.2.6) | a setting: continues into the next block (*chosen*), or wraps to the start of the block |

**The scrambling lock, in full.** The lock's conditions: `Scrambling_Enable` is set, the
line is scrambled TMDS at the ratio `TMDS_Bit_Clock_Ratio` names, that ratio fits the rate
(1/40 above 340 Mcsc, 1/10 at or below), and no refusal withholds the status. The SCDC
target keeps the time they last began to hold: the write that set `Scrambling_Enable` (from
0 to 1; a rewrite that keeps it set, as a quick restore makes, does not restart it), a write
that made the ratio match, an attempt accepted after a withheld one, or the line becoming
what it is, whichever came last.

- **Locking:** `Scrambling_Status` is set once the lock trigger has passed since that time.
- **The sink's own reset:** an HPD reset of the `Status` group loses the lock along with
  the status, and the conditions begin to hold again at the reset: the lock is gained again
  once the trigger has passed since. Only a test's preset of the status holds against the
  lock.
- **The strict sink** (`with_strict_lock(true)`; off by default): the output must start
  after the write that set `Scrambling_Enable` or changed `TMDS_Bit_Clock_Ratio`, and at
  least 1 ms after the last change of the ratio. Output already running at the write, or
  resumed sooner, is not taken until it stops and starts again. With the PHY's default
  start latency of 1 ms, output started at once after the write meets the 1 ms; a test of
  the source's own wait sets the latency to 0.
- **Losing it:** the status is cleared at the first event at which a condition fails: the
  bit or the ratio changes, or the line stops, changes rate, or stops scrambling.
- **Refusals** (`with_scrambling_refusals(Refusal { kind, attempts })`; none by default):
  an attempt is a write that sets `Scrambling_Enable`. For the first `attempts` attempts the
  sink either withholds the status (`RefusalKind::WithholdStatus`: no lock until a later
  attempt) or stores the write without `Scrambling_Enable` (`RefusalKind::DropEnable`); each
  refusal is an `Event::Refusal`.
- **Giving up** (`with_enable_timeout(Some(d))`; 100 ms by default, `None` never gives up):
  without scrambled TMDS at the matching ratio within `d` of the write that set
  `Scrambling_Enable`, or of one that changed `TMDS_Bit_Clock_Ratio` while it was set, the
  sink clears
  `Scrambling_Enable` itself, at the first event at or after the deadline. Output that
  starts at or after the deadline is late, whether or not an SCDC event fell between: the
  line and the registers change only at events, so the sink knows what held since the last
  one.

Every change the sink makes on its own is a `Register` event by `Actor::Sink`. The sink
changes `Scrambling_Status` only when its lock is gained or lost, so a test's preset of the
status holds until then; `Scrambling_Enable` is tracked the same way, from the register,
whoever set it. The lock is evaluated at the start of each transaction, after each byte
written, before each read of the status (which `Trigger::AfterReads` counts) and at every
other bench event.

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

- **Before the sink:** the argument rules of `DdcBus` (an address outside
  `is_usable_address`, an empty `write`, a segment above `MAX_SEGMENT` or a read past 256
  bytes of a segment: `Unsupported`; an empty buffer: `Ok`), then the capabilities. A
  refused call touches no target, takes no time and is recorded with its result.
- **`read_segmented`:** split into pieces of `segmented_piece` bytes where the capabilities
  set one, each piece its own transaction with the pointer and offset written again, so
  something can change between pieces. Segment 0 writes no pointer.
- **Errors:** `BusError`, which implements `DdcError`, `Display` and `core::error::Error`:

  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  #[non_exhaustive]
  pub struct BusError {
      pub kind: DdcErrorKind,
      pub op: BusOp,
      pub addr: u8,
  }
  ```

- **A failed read** fills its buffer with `0xA5`, so a caller that uses it after an error
  reads a pattern, not plausible data (`DdcBus` leaves the contents unspecified).
- **Timing:** each transaction charges the clock after it is evaluated, by `BusTiming`:
  every byte of it, address bytes included, as if it all went out, except a transaction
  nothing answered, which charges its first address byte.

  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  #[non_exhaustive]
  pub struct BusTiming {
      /// Each byte on the wire, address bytes included.
      pub per_byte: Duration,
      /// Each transaction, on top of its bytes.
      pub per_transaction: Duration,
  }

  impl BusTiming {
      /// 90 µs a byte (nine clocks at 100 kHz), nothing per transaction.
      pub const STANDARD_MODE: Self;
      pub const fn with_per_byte(self, d: Duration) -> Self;
      pub const fn with_per_transaction(self, d: Duration) -> Self;
  }
  ```

  A byte is eight data bits and the acknowledge, nine clocks; at 100 kHz, I²C standard
  mode's maximum clock, that is 90 µs. DDC/CI 1.1 assumes standard mode without precluding
  fast mode (400 kHz); sources use 100 kHz [14]. A faster or slower sink or adapter is a
  setting.

---

## The simulated PHY

`phy()` implements `TmdsPhy`, `FrlPhy` and `PhyPower` to hdmi-hal's contracts, including the
mode rule (entering one mode leaves the other). Like the other handles it is `Copy`; two
`phy()` handles are one PHY.

```rust
impl<'a, const LOG: usize, const FAULTS: usize> Bench<'a, LOG, FAULTS> {
    pub fn phy(&self) -> Phy<'_>;   // TmdsPhy + FrlPhy + PhyPower, Error = PhyError
}

impl<'a, const LOG: usize, const FAULTS: usize> BenchBuilder<'a, LOG, FAULTS> {
    /// What the PHY can do; by default `DEFAULT_PHY_CAPABILITIES`: 600 000 kHz with a
    /// scrambler that also scrambles at or below 340 Mcsc.
    pub fn phy_capabilities(self, capabilities: TmdsPhyCapabilities) -> Self;
    /// How the PHY behaves where the contract allows several behaviours.
    pub fn phy(self, settings: PhySettings) -> Self;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct PhySettings {
    /// Powered on at creation; true by default.
    pub powered: bool,
    pub configure_while_running: ConfigureWhileRunning,   // Fail (default) | StopFirst
    pub scrambler_after_configure: ScramblerAfterConfigure,
        // Opposite (default) | Keep | Off | FromRate
    /// How long `start_output` takes before it returns, on the bench's clock; 1 ms by
    /// default.
    pub start_latency: Duration,
}
// PhySettings::DEFAULT, with const `with_*` builders.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct PhyError {
    pub kind: PhyErrorKind,
    pub call: PhyCall,
    /// Whether the call took effect before it failed (a fault that strikes after).
    pub took_effect: bool,
}

#[non_exhaustive]
pub enum PhyErrorKind {
    PoweredOff,     // configure, start_output, set_frl_rate and the FRL calls while off
    NotConfigured,  // start_output without a configure since the last power-on
    Running,        // configure with the output running, under `ConfigureWhileRunning::Fail`
    Unsupported,    // beyond the capabilities: the rate, the scrambler, low-rate scrambling
    NotInFrl,       // send_ltp, set_frl_output, adjust_equalization outside FRL
    Fault,          // a scheduled PHY fault
    Coupled,        // a coupled PHY's own error (see `Coupled<P>`)
}
```

- **Power:** on at creation by default (*chosen*: `PhyPower`'s documentation says procedures
  never change the power, so a procedure test expects a powered PHY); creation in the on
  state counts as a power-on, so `start_output` after a `configure` works either way the
  contract is read. `power_on` on a PHY that is on changes nothing; `power_off` stops the
  output and forgets the mode and its configuration.
- **TMDS:** `configure` fails while powered off, and for a rate above the capabilities'
  maximum; it enters TMDS, leaving FRL, and leaves the output stopped. `start_output` needs a
  `configure` since the last power-on; starting a running output changes nothing.
  `stop_output` works in every state and leaves FRL. `set_scrambling(true)` fails on a PHY
  without a scrambler, and at or below 340 000 kHz on one that cannot scramble there (a
  capability, so an error rather than a silent no-op); `set_scrambling(false)` always
  succeeds.
- **FRL:** `set_frl_rate` with a rate enters FRL, leaving TMDS (the TMDS configuration is
  forgotten), and returns once the PHY transmits at that rate; `NotSupported` leaves FRL and
  enters nothing. `send_ltp`, `set_frl_output` and `adjust_equalization` need FRL; their
  values are recorded, not modelled, until the sink's FRL responses arrive.
- **Where the contract allows several behaviours, each is a setting defaulting to the least
  forgiving one:** a procedure must be correct under any behaviour the contract allows.
  `configure` with the output running fails (`ConfigureWhileRunning::Fail`) rather than
  stopping it. After `configure` the scrambler is set to the opposite of what the rate
  needs (`ScramblerAfterConfigure::Opposite`: on at or below 340 000 kHz, off above), so a
  procedure that never calls `set_scrambling` fails at every rate. Every setting stays
  within the capabilities: the scrambler is never on without one, nor on at or below
  340 000 kHz where the PHY cannot scramble there. `start_output` takes 1 ms
  before it returns, so a deadline counted from before it is visibly late.
- **The line:** the PHY drives it whenever its own output changes: running TMDS output is
  `Line::Tmds { character_rate_khz, ratio, scrambled }` from the configuration and the
  scrambler; FRL is `Line::frl(rate)`; anything else is `Line::Off`. A test's own
  `set_line` changes it too, and holds until the PHY's output next changes: the last change
  wins.
- **Records:** every call is an `Event::Phy { call, result }`, with `PhyCall` the call and
  its arguments, recorded when it returns, followed by any line change it made. Each call
  counts against the budget; `capabilities()`, a report, does not.
- **Faults:** `Bench::schedule_phy(PhyFault)` schedules a PHY fault in the same schedule as
  the bus's, sharing its `FAULTS` capacity: on the `nth` call matching an optional
  `PhyCallKind`, for `times` calls, failing with `PhyErrorKind::Fault` either before the call
  (`took_effect: false`, nothing changes) or after it (`PhyFault::after_effect()`;
  `took_effect: true`, the call applied and the line driven). The struck call's
  `Event::Phy` carries the fault's error.

The coupling between the line and the sink catches a procedure that never writes the sink or
never starts scrambled output; it does not judge order, which a test checks from the
transcript.

### `Coupled<P>`

`Coupled::new(phy, &bench)` wraps any `TmdsPhy` — a third party's, a forwarding stub, a
firmware PHY under hardware-in-the-loop — and implements `TmdsPhy` with `P`'s error, and
`PhyPower` and `FrlPhy` where `P` does. Each call goes to `P` unchanged and returns what `P`
returned; what succeeded drives the bench's line when the coupled output changes, and every
call is recorded (a failure as `PhyErrorKind::Coupled`, `P`'s own error going back to the
caller), is a bench event and counts against the budget, as the simulated PHY's are.
Scheduled PHY faults are the simulated PHY's: `P` has its own. `get_ref`, `get_mut` (whose
calls bypass the coupling) and `into_inner` reach `P`.

The coupled line follows the contract's guarantees only: `configure` enters TMDS with the
output stopped, `start_output` starts it, `stop_output` stops it and leaves FRL, a FRL rate
enters FRL, `power_off` leaves every mode, and a failed call changes nothing. The line is
unscrambled after `configure` until a `set_scrambling` says otherwise (*chosen*: what `P`
does to its scrambler there is its own, and the bench cannot see it).

---

## Faults

A test schedules faults, each striking bus operations that match a filter, at a point of the
operation, with an effect:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Fault {
    pub filter: Filter,
    /// The first matching operation it strikes, counted from 1 from when it is scheduled.
    pub nth: u32,
    pub times: Times,
    pub effect: Effect,
    pub point: Point,
}

impl Fault {
    /// Strikes the first operation, once, before the sink sees it.
    pub const fn new(effect: Effect) -> Self;
    pub const fn on(self, filter: Filter) -> Self;
    pub const fn nth(self, n: u32) -> Self;
    pub const fn times(self, times: Times) -> Self;
    pub const fn at(self, point: Point) -> Self;
}

/// Which operations a fault counts: every field that is set must match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Filter {
    /// The operation's address; for `read_segmented`, the target's.
    pub addr: Option<u8>,
    pub op: Option<OpKind>,          // Write, WriteRead, Read, ReadSegmented
    /// The first byte written: the register or offset.
    pub first: Option<u8>,
}
// Filter::ANY, with const `with_addr`, `with_op`, `with_first` builders.

pub enum Times { Count(u32), UntilCleared }
pub enum Effect { Fail(DdcErrorKind), Slow(Duration) }
pub enum Point { Before, After, Partway { byte: u16 } }

impl<'a, const LOG: usize, const FAULTS: usize> Bench<'a, LOG, FAULTS> {
    pub fn schedule(&self, fault: Fault) -> Result<FaultId, ScheduleFull>;
    pub fn clear_fault(&self, id: FaultId) -> bool;
    pub fn clear_faults(&self);
    pub fn scheduled_faults(&self) -> usize;
}
```

- **What an operation is:** one transaction — a `DdcBus` call, or one piece of a split
  `read_segmented`. Calls refused by the argument or capability checks, empty buffers and
  calls while detached do not count: nothing went out. Calls while unplugged count: they
  reach the wire, and nothing answers.
- **Counting:** each fault counts the matching operations from when it is scheduled, and
  strikes the `nth` and, by `times`, the `k - 1` after it or every one until cleared. Faults
  are checked in the order scheduled; the first that strikes applies, and every fault
  counts the operation. A fault leaves the schedule once its last operation has passed,
  struck or not (an earlier fault may have taken it); `Times::Count(0)` never strikes.
  `FAULTS` is the schedule's capacity; a full schedule refuses with `ScheduleFull`, so the
  test knows its fault is not there.
- **Points:**
  - **before** the sink sees the operation: no target sees it, and only its address byte
    goes out;
  - **after** the sink applied it: the transaction runs as usual (a write latched), and the
    fault's error is returned in place of its result;
  - **partway**, at data byte *k* of the transaction, counted across its operations: the
    target sees the transaction cut there (a write's first *k* bytes; a read shortened; the
    operations after it dropped), then the fault's error is returned. A transaction of *k*
    data bytes or fewer is struck after.
- **Effects:**
  - `Fail(kind)`: the call fails with `kind`. A NACK carries its `NoAcknowledgeSource`; a
    timeout also charges the bus's `stretch_timeout_us` where its capabilities give one; a
    `Detached` fault fails that operation only, the adapter staying attached. An
    `Unsupported` fault breaks the `DdcBus` contract on purpose (`Unsupported` means
    refused before bus activity), to test code against a misbehaving backend.
  - `Slow(d)`: the operation succeeds and takes `d` longer.
- **Time:** a faulted operation is charged for the bytes that went out: its address byte
  before, all of it after, up to byte *k* partway.
- **Records:** the bus record carries the call's result; an `Event::Fault { id, effect,
  point }` follows it. A failed read fills the buffer with the pattern, so a caller that
  uses it after an error fails.

PHY faults are `PhyFault`s, in the same schedule (see "The simulated PHY").

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
  cannot show — what the PHY transmits: whether its output runs, at which rate and ratio,
  scrambled or not, or FRL at which rate — comes from a probe the implementor passes.

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

### The checks' shape

Every check lives in `maquette::check`, takes the implementation under test as the trait
values a procedure would get, and returns a `Report` of its own failure type:

```rust
/// What a check found: the points that failed, in the order found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report<F, const N: usize = 16> { /* private */ }

impl<F, const N: usize> Report<F, N> {
    /// An empty report, and a point recorded in it: for a check of a crate's own trait
    /// that runs against the bench, so its users get the report maquette's checks give.
    pub fn new() -> Self;
    pub fn record(&mut self, failure: Option<F>);
    /// Whether every point passed.
    pub fn is_ok(&self) -> bool;
    /// How many points the check ran.
    pub fn points(&self) -> usize;
    /// How many points failed, recorded or not.
    pub fn failed(&self) -> usize;
    /// The failed points recorded: the first `N`, in the order found.
    pub fn failures(&self) -> impl Iterator<Item = &F>;
    /// How many failed points were not recorded, past the first `N`.
    pub fn overflow(&self) -> usize;
    /// Panics, listing every recorded failure and the overflow, unless every point passed.
    #[track_caller]
    pub fn assert_ok(&self) where F: fmt::Display;
}

impl<F: fmt::Display, const N: usize> fmt::Display for Report<F, N> { /* one line per failure */ }
```

- **A report, not a panic:** a check runs every point, so an implementor sees every failure
  at once. `assert_ok` is the one assertion a test needs.
- **Fixed capacity, overflow counted:** no `alloc`; `failed()` stays exact past `N`.
- **Failure types are the check's own** `#[non_exhaustive]` enums, `Copy` where their fields
  are, implementing `Display`, so a check can gain a point in a breaking release without
  changing the report type.
- **A check's settings** are a `#[non_exhaustive]` struct with a `const DEFAULT` and `const`
  `with_*` builders, and `run` (and `run_async` with the `async` feature) on it; a function
  named after the check runs it with the defaults.
- **The async form** takes hdmi-hal-async's trait, which their `Send` twins implement through
  their blanket implementations, so one async check covers both. It does not promise a `Send`
  future: a test drives it to completion where it is.

### The clock check

Clock and delay are passed as two values, as a platform supplies them from two peripherals;
one value implementing both (a `SimClock`) is passed as `&clock` and `&mut &clock`.

```rust
/// The clock check with its default settings.
pub fn clock<C: Clock + ?Sized, D: Delay + ?Sized>(clock: &C, delay: &mut D)
    -> Report<ClockFailure>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ClockCheck<'a> {
    /// The delays run, each between two reads of the clock.
    pub delays: &'a [Duration],
    /// The reads of the clock in a row, with no delay between them.
    pub reads: u32,
}

impl ClockCheck<'static> {
    /// Delays of 0, 1 ns, 1 µs, 999 µs, 1 ms, 1.001 ms and 10 ms (about 13 ms in all on a
    /// real timer), and 1000 reads in a row.
    pub const DEFAULT: Self;
}

impl<'a> ClockCheck<'a> {
    pub const fn with_delays<'b>(self, delays: &'b [Duration]) -> ClockCheck<'b>;
    pub const fn with_reads(self, reads: u32) -> Self;

    pub fn run<C: Clock + ?Sized, D: Delay + ?Sized>(&self, clock: &C, delay: &mut D)
        -> Report<ClockFailure>;

    #[cfg(feature = "async")]
    pub async fn run_async<C, D>(&self, clock: &C, delay: &mut D) -> Report<ClockFailure>
    where
        C: Clock + ?Sized,
        D: hdmi_hal_async::time::Delay + ?Sized;

    /// Each delay measured on `reference` too, a clock the test trusts.
    pub fn run_with_reference<C, D, R>(&self, clock: &C, delay: &mut D, reference: &R,
        tolerance: Duration) -> Report<ClockFailure>
    where C: Clock + ?Sized, D: Delay + ?Sized, R: Clock;
    // run_with_reference_async with the `async` feature.
}

/// With `std`: `std::time::Instant` from its creation, a reference on a host.
pub struct HostClock { /* … */ }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClockFailure {
    /// A read of the clock was earlier than the read before it.
    WentBackwards { earlier: Timestamp, later: Timestamp },
    /// Across `delay(requested)`, the clock advanced only `elapsed`.
    DelayTooShort { requested: Duration, elapsed: Duration },
    /// The reference advanced short of `requested` by more than the tolerance.
    DelayTooShortOnReference { requested: Duration, elapsed: Duration },
    /// The clock and the reference advanced further apart than the tolerance.
    Drift { requested: Duration, clock: Duration, reference: Duration },
}
```

The points: each read compared with the read before it, over the reads in a row and around
every delay (`WentBackwards`); each delay, measured on the clock from the read before it to
the read after it (`DelayTooShort`). A clock that never advances fails every delay but the
zero one. The delays run in the order given, so a coarse clock or a timer with a long
minimum delay is checked at the durations its procedures use; `with_delays` sets them, for
a platform whose real delays should be few or short.

The clock alone cannot show its rate: a wrong timer frequency scales the clock and every
delay measured on it alike, so a clock and delay 1000 times fast pass. `run_with_reference`
reads a clock the test trusts around each delay too (on a host, `HostClock`; on a board,
another timer): the delay must take at least the time asked on it (`DelayTooShortOnReference`),
and the clock must advance as far as the reference (`Drift`), each within a tolerance for
the reads between them and the reference's resolution.

### The checks for hdmi-hal's traits

Each takes the implementation under test, a description of what is on its other side, and
the hooks a check needs for what the traits cannot show. The descriptions and hooks are
plain values and small traits, so the checks run against the bench, against Linux's
`i2c-stub`, against a board, or against a test's own double; the bench provides every hook.

```rust
/// What is behind a DDC bus, as far as the check needs to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct DdcDevice<'d> {
    /// The EDID the device serves, if the check should read and compare it.
    pub edid: Option<&'d [u8]>,
    /// Whether the device has an E-DDC segment pointer (`false`: segment 0 is read without
    /// one, E-DDC 1.3 §8.1.2).
    pub segment_pointer: bool,
    /// An address the capabilities reach where nothing answers, for the `NoAcknowledge`
    /// point.
    pub silent: Option<u8>,
}
// DdcDevice::new(), with const `with_edid`, `without_segment_pointer`, `with_silent`.

pub struct DdcBusCheck<'d> { /* the device; settings with `with_*` */ }
impl<'d> DdcBusCheck<'d> {
    pub const fn new(device: DdcDevice<'d>) -> Self;
    pub fn run<B: DdcBus + ?Sized>(&self, bus: &B) -> Report<DdcBusFailure>;
    #[cfg(feature = "std")]   // concurrent calls from `threads` (> 0) threads, without a panic
    pub fn run_threads<B: DdcBus + Sync + ?Sized>(&self, bus: &B, threads: usize)
        -> Report<DdcBusFailure>;
    #[cfg(feature = "async")]
    pub async fn run_async<B: hdmi_hal_async::ddc::DdcBus + ?Sized>(&self, bus: &B)
        -> Report<DdcBusFailure>;
}

/// The SCDC registers whose values the check knows and that hold still while it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScdcImage { /* 256 × Option<u8> */ }
// ScdcImage::new(), `with(reg, value)`; `ScdcImage::of(&bench)`: the bench's registers but
// the ones that change by themselves (status and update flags, the error counters).

/// Makes the next transfer fail, so a check can see what follows an error.
pub trait FailNext {
    fn fail_next(&self, kind: DdcErrorKind) -> bool; // whether it could
    fn withdraw(&self) {}                             // a failure no transfer took
}
// Implemented for `Bench` (a scheduled fault on the next operation; `false` when the schedule
// is full). A failure the hook cannot arm is the setup's: `ScdcTransportFailure::NotArmed`.

pub struct ScdcTransportCheck<'i> { /* the image; an optional `&dyn FailNext` */ }
impl<'i> ScdcTransportCheck<'i> {
    pub const fn new(image: &'i ScdcImage) -> Self;
    pub const fn with_faults(self, faults: &'i dyn FailNext) -> Self;
    pub const fn with_write(self, reg: u8, value: u8) -> Self; // a register safe to write
    pub fn run<T: ScdcTransport + ?Sized>(&self, transport: &mut T)
        -> Report<ScdcTransportFailure>;
    // run_async with the `async` feature, over hdmi-hal-async's `ScdcTransport`.
}

/// What the PHY transmits: what the traits cannot show. Handed the PHY, so a PHY that knows
/// its own output is probed by a type that reads it.
pub trait OutputProbe<P: ?Sized> {
    /// `Some(Line::Off)` when nothing; `None` when it cannot tell, and the points that need
    /// it are then skipped.
    fn line(&self, phy: &P) -> Option<Line>;
}
// Implemented for `Bench` (its line, whichever PHY drives it) and `NoProbe` (`None`).

pub struct PhyCheck<'p, R: ?Sized> { /* a configuration the PHY can do, an optional FRL rate, the probe */ }
impl<'p, R: ?Sized> PhyCheck<'p, R> {
    pub const fn new(config: TmdsPhyConfig, probe: &'p R) -> Self;
    pub const fn with_frl_rate(self, rate: HdmiForumFrl) -> Self;
    /// `TmdsPhy` alone: the points that need no power or FRL.
    pub fn run_tmds<P>(&self, phy: &mut P) -> Report<PhyFailure>
    where P: TmdsPhy + ?Sized, R: OutputProbe<P>;
    /// With `PhyPower`: the power points too.
    pub fn run_powered<P>(&self, phy: &mut P) -> Report<PhyFailure>
    where P: TmdsPhy + PhyPower + ?Sized, R: OutputProbe<P>;
    /// With `FrlPhy`: the mode rule too (needs `with_frl_rate`).
    pub fn run_frl<P>(&self, phy: &mut P) -> Report<PhyFailure>
    where P: TmdsPhy + PhyPower + FrlPhy + ?Sized, R: OutputProbe<P>;
    // The async forms with the `async` feature.
}
```

- **The device and the image are the caller's word.** The checks do not discover them: a
  test on the bench builds the bench from the same values (or takes `ScdcImage::of`), and
  a test against `i2c-stub` or a board states what it set up there. A point whose
  description is missing (no EDID given, no silent address) is skipped, not failed.
- **The `DdcBus` check drives only what is safe on a real device:** the refusals it checks
  are reads, or writes to the silent address or past `write_max` there, so a backend that
  wrongly lets one through writes nothing that matters; the only other write is an offset
  byte to 0x50, which a display takes as its word offset (E-DDC 1.3 §6.1). The data points
  read the EDID at offsets 0, 0x11, 0x20, 0x40 and 128, so a bus that drops an offset or a
  write reads the wrong bytes. The read at the current address follows the write of its
  offset, so `run_threads` leaves it out.
- **The PHY's errors are `Debug` only**, so its failures say which point failed and how
  (`Failed` where the contract says it succeeds, `Succeeded` where it says it fails,
  `Running { expected, found }` and `Line { found }` from the probe), not why. Once the
  output runs, the probe's line must be TMDS at the configuration's rate and ratio,
  scrambled as `set_scrambling` last said where it has been called while running, or FRL
  at the rate entered. The configuration is the caller's: one the PHY can do.
- **Failure types** follow the clock check's: `#[non_exhaustive]` enums naming the point
  (`DdcBusFailure`, `ScdcTransportFailure`, `PhyFailure`), `Copy`, with `Display`.

---

## `no_std`, `alloc`, and features

`#![no_std]` without `alloc` in every build, like the procedures it tests, and
`#![forbid(unsafe_code)]`.

- **`std`:** critical-section's `std` implementation, and the concurrent-call `DdcBus`
  check (threads). Not a default feature, as across the stack: a host test enables it.
- **`async`:** hdmi-hal-async's traits on the same handles: `Ddc` implements `SendDdcBus`,
  `Time` `SendDelay`, and `Phy` `SendTmdsPhy`, `SendFrlPhy` and `SendPhyPower`, and the
  plain async traits come through hdmi-hal-async's blanket implementations. The bench is
  synchronous: each async method runs its sync counterpart when its future is first
  polled, and the future is then ready, so an async procedure runs with any executor and
  its futures are `Send`. hdmi-hal-async is re-exported as `maquette::hdmi_hal_async`.
  `Coupled<P>` stays synchronous: an async PHY coupled to the line would need a coupling of
  its own. The cancellation harness comes with this feature before the first release.
- **`embedded-hal`** (planned): the message-level sink.

CI builds each `no_std` feature set for a bare target (`thumbv7em-none-eabi`): a host build
without `std` does not prove `no_std`, because `std` is still available to dependencies.

---

## Testing

See [`testing.md`](testing.md). maquette's own tests check its implementations against the
conformance checks, the checks' negative cases against broken doubles, and the sink's rules
against the sources above.

A fuzz target drives the bench with arbitrary sequences of bus calls, PHY calls, line
changes, HPD, plug and adapter changes, presets, faults and clock advances, on a sink whose
settings and bus capabilities the input also chooses. It decodes the input bytes itself, as
the stack's other fuzz targets do. After every action it checks
the invariants a simulator must keep:

- the same input gives the same transcript (the input is run twice);
- nothing panics and no arithmetic overflows (the target builds with overflow checks; the
  budget is off and the input bounds the actions);
- `Scrambling_Status` is set only while `Scrambling_Enable` is set and the line is
  scrambled TMDS at the matching ratio, and that ratio fits the rate (the input presets any
  register but the status; its lines include ratios that do not fit);
- once the lock's conditions have held for a time trigger, the status is set: for sinks
  neither strict nor refusing, after an advance or a delay, with HPD high (the input picks
  each register group's HPD reset);
- no `Register` event changes a register the sink owns by `Actor::Source`;
- the segment pointer never outlives its command sequence: every EDID read at 0x50 that
  succeeds reads segment 0;
- the transcript's times never go backwards.

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
| [7] | `Source_Version` and `Sink_Version` | amdgpu `drivers/gpu/drm/amd/display/dc/link/protocols/link_ddc.c:592–600` (when `Sink_Version` is 1); Linux's SCDC helper `drivers/gpu/drm/display/drm_scdc_helper.c:322–350`; dw-hdmi `drivers/gpu/drm/bridge/synopsys/dw-hdmi.c:2112–2115`; Xilinx `XilinxProcessorIPLib/drivers/v_hdmitx1/src/xv_hdmitx1.c:789–807` (a failed write: a sink without SCDC) |
| [8] | Update flags cleared by writing them back as 1 | amdgpu `drivers/gpu/drm/amd/display/dc/link/protocols/link_hdmi_frl.c:178–200` (`FLT_update`); Amlogic `drivers/media/vout/hdmitx21/hdmi_tx_main.c:446–463` (`FLT_update`), `hdmitx21/hdmi_tx_scdc.c:106–107` (`Status_update`, `CED_update`, `RSED_update`) |
| [9] | The SCDC register map: addresses, bits, and which registers the source writes | Linux `include/drm/display/drm_scdc.h:27–53`; amdgpu `drivers/gpu/drm/amd/display/dc/dc_hdmi_types.h:58–65`, `dc/link/protocols/link_ddc.c:602–609`; Xilinx `XilinxProcessorIPLib/drivers/v_hdmitx1/src/xv_hdmitx1.c:41–49`, `645–669`, `719–743`, `790`, `870`; Amlogic `drivers/media/vout/hdmitx21/hw/hdmi_tx_ddc.h:16–56` (the source's registers marked `RW`) |
| [10] | 1/40 and scrambling above 340 Mcsc, 1/10 at or below | Linux's SCDC helper `drivers/gpu/drm/display/drm_scdc_helper.c:262–267` (comment); i915 `drivers/gpu/drm/i915/display/intel_hdmi.c:2437–2440` (`port_clock > 340000`); amdgpu `drivers/gpu/drm/amd/display/dc/link/protocols/link_ddc.c:575`, `604–605` (`pix_clk > 340000` writes `TMDS_Config` 3); Xilinx `XilinxProcessorIPLib/drivers/v_hdmitx1/src/xv_hdmitx1.c:740–743` (the ratio bit above 3.4 Gbps) |
| [11] | A ratio change: suspend, write `TMDS_Bit_Clock_Ratio`, resume 1 to 100 ms after | Linux's SCDC helper `drivers/gpu/drm/display/drm_scdc_helper.c:300–305` (waits 1–2 ms after the write); dw-hdmi `drivers/gpu/drm/bridge/synopsys/dw-hdmi.c:1407–1419`; Amlogic `drivers/media/vout/hdmitx21/hdmi_tx_main.c:510–517` |
| [12] | The sink's test and manufacturer registers | `Source_Test_Configuration` (0x35) read by the source: amdgpu `drivers/gpu/drm/amd/display/dc/link/protocols/link_hdmi_frl.c:58`, `98`, `220`, `310`; Xilinx `XilinxProcessorIPLib/drivers/v_hdmitx1/src/xv_hdmitx1_frl.c:360`, `538` (`xv_hdmitx1_hw.h:44`); Amlogic `drivers/media/vout/hdmitx21/hdmi_tx_scdc.c:198–200`, and not marked `RW` in `hw/hdmi_tx_ddc.h:56`. Amlogic marks 0xC0 and 0xDE–0xFF `RW` (`hw/hdmi_tx_ddc.h:85`, `93`), not 0xD0–0xDD |
| [13] | What the sink resets around HPD | Xilinx's receiver `XilinxProcessorIPLib/drivers/v_hdmirx1/src/xv_hdmirx1.c:740–765` (lowering HPD resets FRL, raising it clears `FRL_Rate`), `xv_hdmirx1_frl.c:76`, `1480–1502` (the reset clears `TMDS_Config` bits 1:0, in FRL builds), `xv_hdmirx1_intr.c:1168–1182` (the cable's 5 V lost clears all of SCDC); Amlogic's receiver `drivers/media/vin/tvin/hdmirx/hdmi_rx_hw.c:2045–2069`, `2854–2870` (SCDC cleared on T7 and later chips, or the HPD state handed to the IP's SCDC block), called with HPD low from `hdmi_rx_wrapper.c:3663`, `4794–4797` |
| [14] | DDC at 100 kHz | i915 `drivers/gpu/drm/i915/display/intel_gmbus.c:960–961` ("a conservative clock rate"); Xilinx `XilinxProcessorIPLib/drivers/v_hdmitx1/src/xv_hdmitx1.c:1580` (the DDC clock divider) |

Standards (VESA, public):

- **E-DDC 1.3:** the segment pointer (§2.2.5) and its reset on NACK or STOP (§8.1.1,
  Table 9-1); a display without one (§8.1.2, §9.5.1); the current-address read (§6.1);
  segments (§6.4–§6.5); reading across 128 bytes (§2.2.6); the EDID available whenever +5 V
  is present (§4.2.2), and readable with the display powered off (§9.2, §9.5.3).
- **DDC/CI 1.1:** I²C standard mode, 100 kHz, assumed in its examples; fast mode (400 kHz)
  and high-speed mode not precluded.

Standards (DDWG, public):

- **DVI 1.0:** HPD high only while the EDID can be read (§2.2.9.2).
