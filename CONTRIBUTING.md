# Contributing to maquette

Thanks for your interest in contributing. This document covers the basics.

## Getting started

Relevant docs for contributors:

- [`doc/setup.md`](doc/setup.md) — build, test, MSRV and coverage commands
- [`doc/architecture.md`](doc/architecture.md) — role, scope, the bench and its handles,
  time, the sink's behaviour and its sources, faults, the conformance checks and design
  principles
- [`doc/testing.md`](doc/testing.md) — how the tests are designed
- [`doc/roadmap.md`](doc/roadmap.md) — what is planned

## Issues and pull requests

**Open an issue first** if you're unsure whether something is a bug or if you want to
discuss a change before implementing it. For small, self-contained fixes a PR on its own
is fine.

- Bug reports: give the bench's settings, the calls the test made, the transcript, and what
  you expected, ideally with the sink or the HDMI source implementation whose behaviour you
  expected the bench to match.
- A conformance check that fails your implementation wrongly: give the check, the failed
  points it reported, and why your implementation meets hdmi-hal's contract there.
- Feature requests: a brief description of what you need and why is enough to start a
  conversation.
- PRs: keep them focused. One logical change per PR makes review faster and keeps history
  readable.

## Coding standards

- Run `cargo fmt` and `cargo clippy --all-targets -- -D warnings` before pushing.
- Public items need rustdoc comments (`cargo rustdoc --all-features -- -D warnings` must
  pass).
- Follow the existing patterns in the codebase — see
  [`doc/architecture.md`](doc/architecture.md) for the design principles behind them.
- `#![forbid(unsafe_code)]` is enforced; no unsafe code.
- Keep `no_std` compatibility without `alloc`: records have a fixed capacity and count their
  overflow, never panic.
- Keep the simulation deterministic: nothing reads the host's clock or runs in the
  background, and the sink evaluates its behaviour only at bench events.
- A claim about HDMI behaviour (a register, a rule, a default) cites at least two
  independent public implementations (repository, commit, file and line) in the
  architecture document's Sources, or a public standard by section. Where implementations
  differ, or fewer than two say, the behaviour is a setting, with the majority as default
  or a default marked *chosen*.
- **Conformance checks change only in a breaking release.** A new check point can turn an
  implementor's CI red, so it waits for the next breaking release (a minor release while
  the crate is 0.x), never a patch, and the changelog lists it under the breaking changes.

## Commit and PR expectations

- Write commit messages in the imperative mood ("Add support for …", not "Added …").
- Keep commits logically atomic. A PR that touches three unrelated things should be three
  commits (or three PRs).
- Tests are expected for every behaviour: each sink behaviour with its default and each
  setting, each check point with an implementation that passes it and a broken double that
  fails it.
- CI must be green before a PR can merge: fmt, clippy, docs, the tests without features,
  with `std` and with `async`, the bare-target `no_std` builds, the same tests
  and builds on the MSRV (Rust 1.85), and coverage must not drop more than 0.1 percentage
  points below the baseline (stored in `.coverage-baseline`).

## Publishing and downstream dependencies

Crates of the [Ashlar stack](https://ashlar-display.org/crates.html) and implementations of
hdmi-hal's traits use maquette in their tests. maquette's API is made of hdmi-hal's types,
so a breaking hdmi-hal release is a breaking maquette release, and its dependents move their
tests when maquette moves. A change dependents need must be merged, tagged, and published to
crates.io before they can adopt it. The workflow is:

1. Merge and tag a release in this repo (`vX.Y.Z`).
2. The publish CI workflow uploads the new version to crates.io.
3. Update the version constraint in the dependent crate's `Cargo.toml`.

## Review process

PRs are reviewed on a best-effort basis. Expect feedback within a few days; if you haven't
heard back in a week feel free to ping the thread. Reviews aim to be constructive — if
something needs to change, the reviewer will explain why. Approval from the maintainer is
required to merge.

## Code of Conduct

This project follows the [Contributor Covenant 3.0](https://github.com/ashlar-display/.github/blob/HEAD/CODE_OF_CONDUCT.md),
the code of conduct of every [Ashlar](https://ashlar-display.org) repository. Please read it
before participating.
