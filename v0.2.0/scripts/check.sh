#!/usr/bin/env bash
# Copyright (C) 2026 whispr.dev
# SPDX-License-Identifier: GPL-3.0-only
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
cargo fmt --all --check
cargo test --locked --all-targets
cargo test --locked --doc
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
# The offline demo at a fixed time, into a fresh folder that git ignores.
rm -rf target/demo
cargo run --locked --release -- demo --out target/demo --at 2026-10-01T09:00:00Z
# Check the export with SkeleTrace itself when its program is on PATH.
if command -v skeletrace >/dev/null 2>&1; then
    cargo run --locked --release -- verify-skeletrace target/demo/skeletrace/skeletrace-profile.json
else
    printf '%s\n' 'SkeleTrace is not on PATH; skipped verify-skeletrace.'
fi
printf '%s\n' 'CrowdWatch validation passed.'
