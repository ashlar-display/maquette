# Development Setup

**Requirements:** Rust 1.85+ (stable). Install via [rustup](https://rustup.rs/).

## Clone and build

```sh
git clone https://github.com/ashlar-display/maquette.git
cd maquette
cargo build
```

## Running checks

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --features std -- -D warnings
cargo clippy --all-targets --features async -- -D warnings
cargo rustdoc --all-features -- -D warnings
```

## Running tests

```sh
cargo test                          # no features
cargo test --features std           # with std
cargo test --features async         # with the async handles
```

The README's examples run as doctests with the others.

## `no_std` builds

The crate is `no_std` without `alloc` in every build. CI checks it on a bare target:

```sh
rustup target add thumbv7em-none-eabi
cargo build --target thumbv7em-none-eabi
cargo build --features async --target thumbv7em-none-eabi
```

An application that runs the bench on a bare target supplies a
[critical-section](https://github.com/rust-embedded/critical-section) implementation, as
other embedded code does; on a host the `std` feature supplies one.

## Checking the MSRV

The MSRV is `rust-version` in `Cargo.toml`. CI checks it with
[`cargo-msrv`](https://github.com/foresterre/cargo-msrv), which runs the checks on that
toolchain:

```sh
cargo install cargo-msrv --locked
cargo msrv verify -- cargo test --locked
cargo msrv verify -- cargo test --locked --features std
cargo msrv verify -- cargo test --locked --features async
cargo msrv verify --target thumbv7em-none-eabi \
  -- cargo build --locked --target thumbv7em-none-eabi
cargo msrv verify --target thumbv7em-none-eabi \
  -- cargo build --locked --features async --target thumbv7em-none-eabi
```

## Measuring coverage

Coverage requires [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov):

```sh
cargo install cargo-llvm-cov
cargo llvm-cov clean --workspace
cargo llvm-cov --no-report --locked
cargo llvm-cov --no-report --locked --features std
cargo llvm-cov --no-report --locked --features async
cargo llvm-cov report
```

The second and third runs cover what `std` and the async handles add; the report merges
all three, as CI does.

The current baseline is stored in `.coverage-baseline`. CI fails if coverage drops more
than 0.1 percentage points below it. On pushes to `main` or `develop`, an improvement
automatically opens a `ci/coverage-ratchet` PR to commit the new baseline. After a Rust
update, measure from a clean `target/` first: instrumented binaries from another Rust
version count as uncovered lines.
