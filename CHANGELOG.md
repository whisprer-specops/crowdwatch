# Changelog

## 0.2.0 — 2026-10-14

- Rewrote CrowdWatch in Rust, in `v0.2.0/`: one self-contained program, `crowdwatch`, built with `cargo build --locked --release` and installed with `cargo install --locked --path .`. Commands, options, configuration files, import formats and the database are those of 0.1.0, and either version opens the other's database. Given the same data and `--at` time, reports and SkeleTrace exports match 0.1.0's byte for byte, apart from the version number.
- Added `verify-skeletrace`, which replaces `tools/verify_skeletrace.py`. It runs SkeleTrace's `profile-validate` and `poll-source` on an export and requires every sample line to be read. SkeleTrace 1.0.0 accepted the export.
- Kept collection going when a reply is malformed in ways 0.1.0 did not catch. A malformed CSV file, an RSS feed naming an unknown text encoding, or a JSON reply with a list where an object belongs is now that source's error, and the other sources still run. 0.1.0 stopped the whole run with a Python traceback. Any remaining unexpected failure prints one line and exits with code 1.
- Added SOCKS proxies to the `HTTPS_PROXY` and `NO_PROXY` support, so collection can go through Tor with `socks5h://127.0.0.1:9050`. A proxy address the client cannot use makes each request fail; it never goes direct. Windows' own proxy settings are no longer read.
- Checked HTTPS certificates against the Mozilla root list built into the program, and converted international domain names with UTS #46, the rules browsers use, instead of IDNA 2003.
- Read times, option errors and character classes as Python 3.14 does, the version 0.1.0's README used.
- Added `--version`'s copyright and licence notice, a copyright line in the help, and the `platform` entry in `doctor`'s report.
- Ported all 51 Python tests and added more, 99 in all, with `scripts/check.ps1` and `scripts/check.sh` to run them with the formatting check, strict lints, a release build and the demo. Every code file opens with whispr.dev's copyright line and the GPL-3.0-only SPDX tag, and a test enforces it.
- Replaced the template CONTRIBUTING, SECURITY, CODE_OF_CONDUCT and CHANGELOG files with real ones, and stated the GPL-3.0-only licence in NOTICE.md.
- [MIGRATION.md](MIGRATION.md) lists every deliberate difference from 0.1.0.
- Ready-built downloads for Windows, macOS (one program for Apple silicon and Intel) and Linux (x86_64 and ARM64), from the public repository [whisprer-specops/crowdwatch](https://github.com/whisprer-specops/crowdwatch). Each system has an installer that shows the licence and asks you to accept it, then asks for a folder and changes nothing else, beside a plain archive. GitHub Actions builds and checks every release on all three systems (`.github/workflows/release.yml`, with `packaging/`).

## 0.1.0 — 2026-10-01

- First release, in Python, with no third-party runtime dependencies: an aggregate movement observatory built from the useful data-fusion idea in the supplied `osint-behavioural-sink` archive.
- Nine collector types: canonical JSON and CSV, mapped JSON, City of Melbourne pedestrian counts, Open-Meteo, TfL line status, Ticketmaster events, GBFS bike and dock inventory, and RSS or Atom.
- A versioned SQLite store in which replay cannot see later corrections, with per-source scheduling, retry backoff and pruning.
- Transparent anomaly signals against a time-of-day baseline, independence groups, conditional short-term projections, area flow balance and a backtest against persistence.
- An offline HTML report, JSON reports, and a SkeleTrace `EngineProfile` and NDJSON sample export, with GeoJSON and a module manifest.
- A deterministic synthetic demonstration, starter configurations for London, Melbourne and New York, JSON Schemas, and 51 contract tests.
