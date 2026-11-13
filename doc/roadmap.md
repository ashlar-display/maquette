# Roadmap

What each release contains will be in the [changelog](../CHANGELOG.md); this page is about
what comes next. Today `maquette` is its design ([`architecture.md`](architecture.md)) and
`SimClock`, hdmi-hal's `Clock` and `Delay` on simulated time.

## Planned

In the order they are built:

### The clock check

The conformance check for `Clock` and `Delay` implementations, run against `SimClock` and
against broken clocks.

### The register-level sink and `DdcBus`

The `Target` trait; the SCDC register file; the EDID behind the E-DDC segment pointer;
DisplayID; register presets; the transcript; and `DdcBus` over the sink, with capabilities a
test chooses.

### The sink's behaviour

`TMDS_Config`, `Scrambling_Status` and their settings; HPD, unplug and detach;
`Source_Version` and `Sink_Version`; the update flags; behaviour triggered by a count of
operations; the operation budget.

### Faults and fuzzing

Faults that strike before, after or partway through an operation, and a fuzz target that
checks the simulator's invariants.

### The line and the simulated PHY

The line between source and sink; a simulated PHY implementing `TmdsPhy`, `FrlPhy` and
`PhyPower` with the mode rule; `Coupled<P>`, so any PHY can drive the line.

### The async handles

hdmi-hal-async's traits and their `Send` twins on the same handles.

### The conformance checks

Checks for implementations of `DdcBus`, `ScdcTransport`, `TmdsPhy`, `FrlPhy` and
`PhyPower`, each returning a report of every point that failed.

### Before the first release

A harness that cancels async procedures at every await point; the sink's FRL training
responses; a message-level sink over embedded-hal 1.0's `I2c`, for checking a `DdcBus` at
the wire; the CED counters, with torn reads.

## Later

- **An HDCP port,** with the stack's HDCP support. Until then a test adds one as its own
  target.
- **Checks for the video pipeline and packet transmission traits,** when those traits
  exist in hdmi-hal.
