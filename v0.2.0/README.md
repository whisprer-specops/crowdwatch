# CrowdWatch 0.2.0

This folder is the Rust source of CrowdWatch 0.2.0, an aggregate movement observatory with a SkeleTrace import adapter. Copyright (C) 2026 whispr.dev, free software under GPL-3.0-only.

The full guide is the repository's [README](../README.md). [MIGRATION.md](../MIGRATION.md) covers the move from the Python 0.1.0 and the few deliberate differences.

## Build, check and install

On Windows 11, in PowerShell, from this folder:

```powershell
cargo build --locked --release
powershell -ExecutionPolicy Bypass -File .\scripts\check.ps1
cargo install --locked --path .
```

The first command builds `target\release\crowdwatch.exe`. The second runs every check: formatting, the tests, the strict lints, a release build and the offline demo. When `skeletrace` is on your `PATH`, it also checks the demo's export with SkeleTrace itself. The third command puts `crowdwatch` on your `PATH`. On Linux or macOS, use `bash scripts/check.sh` for the checks.

The toolchain is pinned to Rust 1.97.0 in `rust-toolchain.toml`, matching SkeleTrace. The code keeps to Rust 1.87, the minimum declared in `Cargo.toml`. Every dependency, at its exact version, is one SkeleTrace already uses. SQLite is compiled from the copy bundled with `rusqlite`, so a C compiler is needed: Visual Studio Build Tools on Windows. The crate forbids `unsafe` code.

## Layout

| Path | What it holds |
| --- | --- |
| `src/main.rs`, `src/cli.rs` | The command line: parsing, help, every command and `verify-skeletrace` |
| `src/config.rs` | The deployment configuration and its checks, and Python's path rules |
| `src/model.rs` | Observations, their checks, keys and digests, and file writing |
| `src/store.rs` | The versioned SQLite store, source scheduling and pruning |
| `src/sources.rs` | The nine collectors, and canonical and mapped imports |
| `src/http.rs` | The bounded HTTPS client and its proxy rules |
| `src/analysis.rs` | Baselines, anomaly signals, projections and the backtest |
| `src/report.rs`, `assets/style.css` | The offline HTML report |
| `src/export.rs` | The SkeleTrace profile, sample, GeoJSON and module exports |
| `src/demo.rs` | The synthetic demonstration and the starter configurations |
| `src/error.rs` | Errors, named and worded as Python reported them |
| `src/json.rs`, `src/pyfmt.rs`, `src/pytime.rs`, `src/urlparse.rs`, `src/csvread.rs`, `src/xml.rs` | Python's JSON, number, text, time, URL, CSV and XML rules, kept so outputs match 0.1.0 byte for byte |
| `src/unicode_tables.rs`, `src/xml_encodings.rs` | Generated tables; do not edit them by hand |
| `tests/` | Offline contract tests with synthetic data |
| `scripts/check.ps1`, `scripts/check.sh` | Every check, in order |
| `scripts/gen_unicode.py`, `scripts/gen_xml_encodings.py` | Regenerate the two table files |

## The generated tables

`src/unicode_tables.rs` holds Python 3.14's Unicode 16.0 character classes. `src/xml_encodings.rs` holds how Python's XML parser treats each text encoding a feed can name. Both are generated from Python itself, so the rules match Python exactly. From this folder:

```powershell
py -3.14 scripts\gen_unicode.py src\unicode_tables.rs
py -3.14 -W ignore scripts\gen_xml_encodings.py src\xml_encodings.rs
```

`gen_unicode.py` needs Unicode 16.0 data, which Python 3.14 has; under an older Python, install the `unicodedata2` package at version 16.0.0 first. `gen_xml_encodings.py` needs Python 3.14's codec list, and is best run on Linux or macOS, because on Windows Python also offers Windows' own code pages. Run the checks afterwards.
