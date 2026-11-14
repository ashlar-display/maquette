# Roadmap

What each release contains will be in the [changelog](../CHANGELOG.md); this page is about
what comes next. Today `maquette` is its design ([`architecture.md`](architecture.md)) and
`SimClock`, hdmi-hal's `Clock` and `Delay` on simulated time, with the conformance check
for `Clock` and `Delay` implementations, and the bench: its DDC bus routed to a test's own
targets, the sink's SCDC registers and its EDID and DisplayID behind the E-DDC segment
pointer, register presets, and the transcript; the line, HPD, unplug and detach as bench
events; the sink's scrambling lock and the registers a long HPD low resets; the operation
budget; scheduled faults; the simulated PHY (`TmdsPhy`, `FrlPhy`, `PhyPower`) and
`Coupled<P>`, any PHY, driving the line; hdmi-hal-async's traits on the same handles; and
a fuzz target checking the simulator's invariants.

## Planned

In the order they are built:

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
