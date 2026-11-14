# maquette

[![CI](https://github.com/ashlar-display/maquette/actions/workflows/ci.yml/badge.svg)](https://github.com/ashlar-display/maquette/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/maquette.svg)](https://crates.io/crates/maquette)
[![docs.rs](https://docs.rs/maquette/badge.svg)](https://docs.rs/maquette)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](LICENSE)
[![Rust 1.85+](https://img.shields.io/badge/rustc-1.85+-orange.svg)](https://blog.rust-lang.org/2025/02/20/Rust-1.85.0.html)
[![SLSA Level 2](https://slsa.dev/images/gh-badge-level2.svg)](https://slsa.dev)

> **Part of [Ashlar](https://ashlar-display.org)** — a display connection stack in Rust,
> HDMI 2.1 first: from EDID parsing through mode negotiation to FRL link training.
> See [all crates](https://ashlar-display.org/crates.html).

A simulated HDMI sink for tests.

`maquette` simulates what an HDMI source can observe of a sink: its SCDC registers, its EDID
and DisplayID behind the DDC bus, the source's PHY and the line between them, and a clock,
all under deterministic simulated time. Code written over
[hdmi-hal](https://github.com/ashlar-display/hdmi-hal)'s traits runs against it as against a
sink — with the sink's delays, its refusals and its faults — and a test asserts on one
ordered transcript of everything that happened. Where real sinks differ and no public source
settles how, the behaviour is a setting with a documented default. It also carries
conformance checks that an implementation of hdmi-hal's traits runs in its own tests.

## Status

In development. Built so far: `SimClock`, hdmi-hal's clock and delay on simulated time; the
conformance check for clock and delay implementations; and the bench, with its DDC bus routed
to a test's own targets, the sink's SCDC registers and its EDID and DisplayID behind the
E-DDC segment pointer, register presets and the transcript; the line, HPD, unplug and detach;
the sink's scrambling lock and its HPD resets; an operation budget; and scheduled faults,
with a fuzz target checking the simulator's invariants. The simulated PHY comes next. The
design is in [`doc/architecture.md`](doc/architecture.md), and the order in which the rest is
built is on the [roadmap](doc/roadmap.md).

## Usage

A procedure that waits and reads the time runs on `SimClock` in no time at all, and a test
checks exactly how long it took. One clock serves as both clock and delay:

```rust
use core::time::Duration;
use maquette::{Clock, Delay, SimClock, Timestamp};

/// Polls `ready` every millisecond until it holds or `timeout` has passed.
fn poll<C: Clock, D: Delay>(
    clock: &C,
    delay: &mut D,
    timeout: Duration,
    mut ready: impl FnMut() -> bool,
) -> bool {
    let start = clock.now();
    loop {
        if ready() {
            return true;
        }
        if clock.now().saturating_duration_since(start) >= timeout {
            return false;
        }
        delay.delay(Duration::from_millis(1));
    }
}

// A delay that sleeps 100 µs long, as a real one may.
let clock = SimClock::new().with_overshoot(Duration::from_micros(100));
let mut polls = 0;
let found = poll(&clock, &mut &clock, Duration::from_millis(5), || {
    polls += 1;
    polls == 4
});
assert!(found);
// Three delays of 1 ms, each 100 µs long.
assert_eq!(clock.now(), Timestamp::from_micros(3_300));
```

`SimClock` is `Sync` and can be a `static`. With the `std` feature a host supplies its
critical section; on a bare target the application supplies one.

An implementation of hdmi-hal's clock and delay runs the conformance check in its own tests.
It reports every point that failed, not just the first:

```rust
use maquette::{SimClock, check};

// Your platform's clock and delay; one `SimClock` serves as both here.
let clock = SimClock::new();
let report = check::clock(&clock, &mut &clock);
report.assert_ok(); // panics listing every failed point
```

`check::ClockCheck` chooses the delays and reads the check runs, for a platform whose real
delays should be few or short; with the `async` feature, `run_async` checks an async delay.

## Features

- **`std`** — the critical-section implementation for hosts, and the checks that need
  threads. Without it, the application supplies its own critical section, as embedded code
  does. A host test enables it:

  ```toml
  [dev-dependencies]
  maquette = { version = "…", features = ["std"] }
  ```
- **`async`** — hdmi-hal-async's traits, and their `Send` twins, on the same handles.

## `no_std` builds

`maquette` is `#![no_std]` without `alloc` in every build, and `#![forbid(unsafe_code)]`:
its capacities are const generics with defaults, so a bench also runs on a bare target.

## Stack position

`maquette` implements and checks [hdmi-hal](https://github.com/ashlar-display/hdmi-hal)'s
traits, and [hdmi-hal-async](https://github.com/ashlar-display/hdmi-hal-async)'s with the
`async` feature. The crates above them use it in their tests.

## Out of scope

Checks for traits defined by the crates that test against it, a sink's internals (video
decoding, its EDID policy, CEC), and real hardware. See
[`doc/architecture.md`](doc/architecture.md).

## Documentation

- [`doc/architecture.md`](doc/architecture.md) — role, scope, the bench and its handles,
  time, the sink's behaviour and its sources, faults, the conformance checks
- [`doc/roadmap.md`](doc/roadmap.md) — what is planned, in the order it is built
- [`doc/setup.md`](doc/setup.md) — build, test, MSRV and coverage commands
- [`doc/testing.md`](doc/testing.md) — how the tests are designed
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — issues, pull requests and coding standards

## Verifying releases

Releases will be built on GitHub Actions and attested with
[SLSA Build Level 2](https://slsa.dev) provenance. To verify a release `.crate` against its
signed provenance, install the [GitHub CLI](https://cli.github.com/) and run:

```sh
gh attestation verify maquette-X.Y.Z.crate --repo ashlar-display/maquette
```

The attested `.crate` will be attached to each
[GitHub release](https://github.com/ashlar-display/maquette/releases).

The first release will be uploaded by the publish workflow with a crates.io API token scoped
to publishing this new crate, because trusted publishing can only be set up for a crate
already on crates.io; the token is revoked afterwards. Later releases will be uploaded
through crates.io
[trusted publishing](https://rust-lang.github.io/rfcs/3691-trusted-publishing-cratesio.html):
the publish workflow obtains a short-lived token via OpenID Connect instead of using a
stored API token. Each version's publishing details, including the workflow run and commit,
are available from the crates.io API:

```sh
curl -A "your-tool (contact)" https://crates.io/api/v1/crates/maquette/X.Y.Z
# see .version.trustpub_data
```

## License

[MPL-2.0](LICENSE)
