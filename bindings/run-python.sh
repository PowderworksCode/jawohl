#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
cargo run -q -p jawohl-surface --bin generate
cargo build -q -p jawohl-py
OUT="$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
mkdir -p .pyimport && cp "$OUT/debug/libjawohl.so" .pyimport/jawohl.so
PYTHONPATH=.pyimport python3 test.py
