# Roadmap

What each release contains will be in the [changelog](../CHANGELOG.md); this page is about
what comes next. Today `maquette` is its design ([`architecture.md`](architecture.md)) and
`SimClock`, hdmi-hal's `Clock` and `Delay` on simulated time, with the conformance check
for `Clock` and `Delay` implementations, and the bench: its DDC bus routed to a test's own
targets, the sink's SCDC registers and its EDID and DisplayID behind the E-DDC segment
pointer, register presets, and the transcript; the line, HPD, unplug and detach as bench
events; the sink's scrambling lock, its status flags, its character error counters (with
torn reads) and its side of FRL link training, and the registers an HPD low resets; the
link's character errors and lane limits; the operation
budget; scheduled faults and scheduled actions, which change the sink during a procedure;
a hook for a quirk on top of the sink's SCDC registers;
the simulated PHY (`TmdsPhy`, `FrlPhy`, `PhyPower`) and
`Coupled<P>`, any PHY, driving the line; hdmi-hal-async's traits on the same handles; a
fuzz target checking the simulator's invariants; and the conformance checks for `DdcBus`,
`ScdcTransport`, `TmdsPhy`, `FrlPhy` and `PhyPower`.

## Planned

### Next

- **A cancellation harness:** an async procedure cancelled at every await point, to check
  its recovery at each.
- **A message-level sink** over embedded-hal 1.0's `I2c`, for checking a `DdcBus` at the
  wire.

## Later

- **An HDCP port,** with the stack's HDCP support. Until then a test adds one as its own
  target.
- **Checks for the video pipeline and packet transmission traits,** when those traits
  exist in hdmi-hal.
