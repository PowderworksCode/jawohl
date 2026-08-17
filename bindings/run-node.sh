#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
cargo run -q -p jawohl-surface --bin generate
cargo build -q -p jawohl-node
OUT="$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
mkdir -p .nodeimport && cp "$OUT/debug/libjawohl_node.so" .nodeimport/jawohl.node
node test.mjs
