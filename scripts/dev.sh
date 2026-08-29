#!/usr/bin/env bash
# Stand up a fresh jawohl checkout: hooks, then the gate CI runs. Safe to
# re-run; every step is idempotent.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

# A clone runs no hooks until it is pointed at them: core.hooksPath is per-clone
# configuration, so nothing a checkout carries can set it for you.
git config core.hooksPath .githooks
if [ ! -d .githooks ]; then
    echo "note: .githooks is fleet-managed and not synced here yet; git will"
    echo "      start using it the moment ordnung writes it."
fi

if ! command -v cargo >/dev/null; then
    echo "error: cargo is not on PATH; install Rust from https://rustup.rs" >&2
    exit 1
fi

echo "== build"
cargo build --all-targets

echo "== fmt"
cargo fmt --all -- --check

echo "== clippy"
cargo clippy --all-targets -- -D warnings

echo "== test"
cargo test --all-targets

# Every Rust snippet in the README is a doctest, so this is what stops the front
# page from drifting away from the crate. It is a separate cargo invocation
# because --all-targets does not run doctests.
echo "== doctests"
cargo test --doc

echo
echo "ready. the examples double as documentation:"
echo "  cargo run --example complete    # finish truncated documents"
echo "  cargo run --example streaming   # chunk by chunk"
echo "  cargo run --example validate    # early cancellation"
echo "  cargo run --example sse         # JSON inside a data: stream"
