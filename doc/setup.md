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

The crate is `no_std` without `alloc` in every build. CI checks it on bare targets, an
embedded one (firmware without a heap) and two UEFI ones (firmware drivers with one):

```sh
rustup target add thumbv7em-none-eabi x86_64-unknown-uefi aarch64-unknown-uefi
for target in thumbv7em-none-eabi x86_64-unknown-uefi aarch64-unknown-uefi; do
  cargo build --target "$target"
  cargo build --features async --target "$target"
done
```

An application that runs the bench on a bare target supplies a
[critical-section](https://github.com/rust-embedded/critical-section) implementation, as
other embedded code does; on a host the `std` feature supplies one.

## Checking the MSRV

The MSRV is `rust-version` in `Cargo.toml`. CI checks it with
[`cargo-msrv`](https://github.com/foresterre/cargo-msrv), which runs the checks on that
toolchain:

```sh
cargo install cargo-msrv --locked --version 0.19.6
cargo msrv verify -- cargo test --locked
cargo msrv verify -- cargo test --locked --features std
cargo msrv verify -- cargo test --locked --features async
for target in thumbv7em-none-eabi x86_64-unknown-uefi aarch64-unknown-uefi; do
  cargo msrv verify --target "$target" -- cargo build --locked --target "$target"
  cargo msrv verify --target "$target" \
    -- cargo build --locked --features async --target "$target"
done
```

## Fuzzing

Fuzzing requires nightly and [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz):

```sh
cargo install cargo-fuzz --locked --version 0.13.2
cargo +nightly fuzz run bench -- -max_total_time=60
```

The fuzz crate is its own workspace with its own `Cargo.lock`. See
[`doc/testing.md`](testing.md) for what the target checks and how CI runs it.

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
version count as uncovered lines. A newer Rust or cargo-llvm-cov can also count lines
differently, so measure with the versions CI's coverage job reports (its log names both);
if the same code then measures lower, lower the baseline to that measurement, since the
ratchet only raises it.
