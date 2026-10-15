# Contributing

## Copyright first

whispr.dev holds the copyright in CrowdWatch. Before starting work on a change you would like merged, open an issue so the terms for your contribution can be agreed first. Bug reports need no agreement: see the end of this file.

Every code file opens with these two lines, as comments in its own language, after any `#!` line. `v0.2.0/tests/notice_contracts.rs` fails a file without them.

```text
Copyright (C) 2026 whispr.dev
SPDX-License-Identifier: GPL-3.0-only
```

## Building and checking

Work in `v0.2.0/`. Use Rust 1.97.0, as pinned in `rust-toolchain.toml` (rustup installs it on the first build), and a C build toolchain for the bundled SQLite and the cryptography code. The code keeps to Rust 1.87, the declared minimum in `Cargo.toml`.

Build and validate from `v0.2.0/`:

```text
cargo fmt --all --check
cargo test --locked --all-targets
cargo test --locked --doc
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
cargo run --locked --release -- demo --out target/demo --at 2026-10-01T09:00:00Z
```

On Windows, `powershell -ExecutionPolicy Bypass -File .\scripts\check.ps1` runs all of these; `scripts/check.sh` does the same elsewhere. With SkeleTrace's program on your `PATH`, both scripts also run `verify-skeletrace` on the demo's export.

## Rules for changes

- Tests must never need network access or describe real people. Add synthetic fixtures for any collector change; `v0.2.0/tests/sources.rs` has a recording fake HTTP client.
- Keep the configuration format, the canonical observation contract, the database schema and the report and export formats, unless a documented breaking change is required. Both versions must still open each other's databases.
- A collector must keep provenance, bound its requests and never turn context into a headcount. Keep credentials in environment variables.
- Explain any dependency change and update `Cargo.lock` intentionally.
- Update `docs/SOURCES.md`, the README and `CHANGELOG.md` with any change to collectors or behaviour, and `MIGRATION.md` with any new difference from 0.1.0.

Bug reports should include the version (`crowdwatch --version`), the operating system, the exact command, the expected result and redacted error output. Never include credentials or private data in public reports. Keep discussion constructive and follow `CODE_OF_CONDUCT.md`.
