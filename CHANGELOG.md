# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **`SimClock`:** hdmi-hal's `Clock` and `Delay` on simulated time, starting at zero and
  moving only by a delay or the test's `advance`; it never reads the host's time, never
  moves backwards and saturates instead of overflowing. `with_tick` gives it a coarse
  resolution (`now` rounded down, delays rounded up to whole ticks) and `with_overshoot`
  makes every delay wait longer than asked; either way `now` after `delay(d)` is at least
  `d` later. `Sync`, `const`-constructible, and implemented for `&SimClock` too, so one
  clock is passed as both clock and delay.
- Re-exports of hdmi-hal's `Clock`, `Delay` and `Timestamp`.
- **The clock check** (`check::clock`): the conformance check for implementations of
  hdmi-hal's `Clock` and `Delay`, passed as two values. Every read of the clock must be no
  earlier than the one before it, and every delay must advance the clock by at least its
  duration, so a frozen clock fails. `check::ClockCheck` sets the delays (by default 0, 1 ns,
  1 µs, 999 µs, 1 ms, 1.001 ms and 10 ms) and the reads in a row (1000); with `async`,
  `run_async` checks an async delay, its `Send` twin included. Failures are
  `check::ClockFailure` values.
- **`check::Report`**, what every check returns: the points run, every failed point counted,
  the first 16 recorded in the order found, and `assert_ok`, which panics listing them.

### Internal

- CI: format, clippy and tests without features, with `std` and with `async`; docs with
  every feature and `-D warnings`, as docs.rs builds them; `no_std` builds for the bare
  `thumbv7em-none-eabi` target with and without `async`; a read-only token except where
  the coverage job opens its PR. On `main`, CI fails when `Cargo.toml` has a `[patch]`
  section.
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
