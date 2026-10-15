# CrowdWatch 0.2.0

**An aggregate movement observatory, with a working SkeleTrace import adapter.**

CrowdWatch collects public or supplied aggregate observations and keeps their provenance and timing. It finds unusual changes in comparable movement streams and makes conditional short-term projections. An offline HTML report explains the evidence. It rescues the useful data-fusion idea from the supplied `osint-behavioural-sink` archive.

It does not claim to predict a psychological "behavioural sink", crowd injury or social collapse. Its output is an inspectable signal about the measurements available. **The main constraint is direct measurement coverage:** weather, bike inventory, news and event schedules cannot tell you how many pedestrians are in an area.

0.2.0 is CrowdWatch rewritten in Rust; its source is in the `v0.2.0/` folder. It is one program, `crowdwatch`, with the same commands, files, database and reports as the earlier Python 0.1.0. `MIGRATION.md` lists the few deliberate differences.

Copyright (C) 2026 whispr.dev. CrowdWatch is free software under the GNU General Public License, version 3 only (`LICENSE`), with no warranty.

## Download

Ready-built programs for Windows, macOS and Linux are on the [Releases page](https://github.com/whisprer-specops/crowdwatch/releases/latest). These links always give the newest release:

| System | Installer | Plain archive |
| --- | --- | --- |
| Windows 10 or 11, 64-bit | [`crowdwatch-windows-x86_64-setup.exe`](https://github.com/whisprer-specops/crowdwatch/releases/latest/download/crowdwatch-windows-x86_64-setup.exe) | [`crowdwatch-windows-x86_64.zip`](https://github.com/whisprer-specops/crowdwatch/releases/latest/download/crowdwatch-windows-x86_64.zip) |
| macOS 11 or newer, Apple silicon and Intel | [`crowdwatch-macos-universal.run`](https://github.com/whisprer-specops/crowdwatch/releases/latest/download/crowdwatch-macos-universal.run) | [`crowdwatch-macos-universal.tar.gz`](https://github.com/whisprer-specops/crowdwatch/releases/latest/download/crowdwatch-macos-universal.tar.gz) |
| Linux, x86_64 | [`crowdwatch-linux-x86_64.run`](https://github.com/whisprer-specops/crowdwatch/releases/latest/download/crowdwatch-linux-x86_64.run) | [`crowdwatch-linux-x86_64.tar.gz`](https://github.com/whisprer-specops/crowdwatch/releases/latest/download/crowdwatch-linux-x86_64.tar.gz) |
| Linux, ARM64 | [`crowdwatch-linux-aarch64.run`](https://github.com/whisprer-specops/crowdwatch/releases/latest/download/crowdwatch-linux-aarch64.run) | [`crowdwatch-linux-aarch64.tar.gz`](https://github.com/whisprer-specops/crowdwatch/releases/latest/download/crowdwatch-linux-aarch64.tar.gz) |

Each installer shows the licence and asks you to accept it, then asks which folder to install into. It changes nothing else: no registry entries, no PATH changes and no uninstaller. To remove CrowdWatch, delete its folder. The plain archives hold the same files. Every release lists each file's SHA-256 in [`SHA256SUMS.txt`](https://github.com/whisprer-specops/crowdwatch/releases/latest/download/SHA256SUMS.txt).

- **Windows:** run the `-setup.exe`. It isn't code-signed yet, so Windows may say "Windows protected your PC". That's expected: click **More info**, then **Run anyway**.
- **macOS and Linux:** open a terminal and run the installer with `sh`, for example `sh ~/Downloads/crowdwatch-macos-universal.run`. On macOS, running it with `sh` avoids the warning about programs that aren't notarised yet. Page through the licence with Space, then type `y` to accept.

Then, in the folder you chose, `.\crowdwatch.exe --help` on Windows, or `./crowdwatch --help` on macOS and Linux, lists every option. To build it yourself instead, read on.

## Build and install on Windows

You need Rust, installed with rustup, and Visual Studio Build Tools with the C++ workload. These are the same tools that build SkeleTrace. The first build downloads Rust 1.97.0, which `v0.2.0/rust-toolchain.toml` pins, and the pinned libraries.

In PowerShell, from the repository folder:

```powershell
cd .\v0.2.0
cargo build --locked --release
cargo install --locked --path .
crowdwatch --version
```

The build makes `target\release\crowdwatch.exe`. The install puts a copy in `%USERPROFILE%\.cargo\bin`, which rustup adds to your `PATH`, so `crowdwatch` works from any folder. The last command prints `0.2.0` and the copyright notice.

To run every check (formatting, the 99 tests, the strict lints, a release build and the offline demo):

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\check.ps1
```

On Linux or macOS, use the same `cargo` commands, and `bash scripts/check.sh` for the checks.

## First run

```powershell
crowdwatch demo --out .\my-demo
Start-Process .\my-demo\report.html
```

This makes a new demonstration folder and opens its report in your browser. It needs no network, keys or downloads. The demonstration is explicitly synthetic. It has a growing station crowd, an ordinary plaza, and an area whose pedestrian counter has failed. Add `--at 2026-10-01T09:00:00Z` to get exactly the same demonstration every time.

## Use real public data

The Melbourne preset includes real public pedestrian counters. Counts can be delayed, and a new database needs history before it can set a baseline:

```powershell
crowdwatch init --preset melbourne --out .\melbourne\config.json
crowdwatch collect --config .\melbourne\config.json
crowdwatch report --config .\melbourne\config.json --out .\melbourne\report.html
Start-Process .\melbourne\report.html
```

The other presets are `london` and `new-york`. London starts with weather. Its TfL and Ticketmaster sources are configured but switched off until you add their keys. New York adds Citi Bike inventory, which is transport context, not a pedestrian count. For your own area, change the region's coordinates and radius in the configuration, then add local aggregate feeds. `docs/SOURCES.md` covers all nine collector types, with complete examples.

For TfL, set `$env:TFL_APP_KEY` to your key and enable the `tfl` source in the configuration. For Ticketmaster, set `$env:TICKETMASTER_API_KEY` and enable its source. Keys belong in environment variables, never in the configuration or reports. Open-Meteo's public endpoint has non-commercial terms; a paid or self-hosted endpoint can be configured instead.

To send requests through a proxy, set `$env:HTTPS_PROXY`, for example to `http://127.0.0.1:8080` or, for Tor, `socks5h://127.0.0.1:9050`. Hosts listed in `$env:NO_PROXY` are reached directly. If the proxy address cannot be used, requests fail rather than go direct.

## Keep collecting

```powershell
crowdwatch watch --config .\melbourne\config.json --out .\melbourne\monitor
```

Keep this window open. The scheduler checks every minute, and each source has its own interval and stored retry backoff. A failed source does not stop the others. Refresh `monitor\report.html` to see the latest report. The configuration is reloaded every cycle. Ctrl+C stops the program at once, and nothing half-written is kept. This command does not install a Windows service or scheduled task.

`--cycles 1` runs one monitoring cycle. `--source SOURCE_ID` collects one source only. `--force` ignores a source's next-due time for a manual retry, but never interrupts a collection already under way. `--skeletrace` also writes a SkeleTrace snapshot each cycle.

## SkeleTrace integration

Make the export on the machine where SkeleTrace will read it:

```powershell
crowdwatch export-skeletrace --config .\my-demo\config.json --out .\native-export
crowdwatch verify-skeletrace .\native-export\skeletrace-profile.json --host C:\path\to\skeletrace.exe
```

The export holds a native `EngineProfile`, an unchanging NDJSON file of `FileSampleRecord` lines, GeoJSON and a descriptive module manifest. `verify-skeletrace` runs SkeleTrace's own `profile-validate` and `poll-source` commands. It succeeds only if SkeleTrace reads every sample line. Leave out `--host` when `skeletrace.exe` is on your `PATH`, or on Windows in the current folder. SkeleTrace 1.0.0 accepted this release's export.

**This is a working sidecar and import module, not a plugin loaded into SkeleTrace.** SkeleTrace takes data through its built-in, profile-driven source adapters; it has no loader for outside plugins. Its file adapter remembers how many lines it has read, so load each new snapshot profile afresh. `docs/SKELETRACE.md` gives the exact host commands, JSON types and integration boundary.

## Inspect, replay and maintain

```powershell
crowdwatch doctor --config .\my-demo\config.json
crowdwatch analyse --config .\my-demo\config.json --out .\my-demo\analysis.json
crowdwatch backtest --config .\my-demo\config.json --horizon 15 --out .\my-demo\evaluation.json
crowdwatch prune --config .\my-demo\config.json --keep-days 35
```

`analyse`, `report`, `backtest` and `export-skeletrace` accept `--at` with an ISO 8601 time, such as `2026-10-01T08:00:00Z`. Only observations, and revisions, available by that time enter the analysis. Source health is labelled as the latest collection state, not historical health.

`import --config CONFIG FILE` reads canonical JSON or NDJSON; add `--format canonical_csv` for CSV. An ordinary import records the data as available now. Use `--historical-availability` only when you can vouch for the supplied retrieval times; it deliberately allows historical replay. See `examples/` and `schemas/`.

Exit codes: `0` success; `1` a configuration or local failure; `2` a usage error, or a `collect` run in which a source failed. Partial provider coverage is recorded as a warning, never as complete coverage.

## What is where

| Location | Purpose |
|---|---|
| `v0.2.0/` | CrowdWatch 0.2.0, the Rust source; its `README.md` maps the code |
| `MIGRATION.md` | Moving from 0.1.0 to 0.2.0, and what is deliberately different |
| `CHANGELOG.md` | What changed in each version |
| `examples/` | Runnable preset configurations and canonical and mapped import examples |
| `schemas/` | JSON Schema descriptions of the configuration and canonical observations |
| `docs/MODEL.md` | Exactly what the model computes: inference rules, projections and limits |
| `docs/SOURCES.md` | Collector setup, provenance, restrictions and routes for more sources |
| `docs/SKELETRACE.md` | The SkeleTrace integration contract and its host check |
| `docs/VALIDATION.md` | What was tested, how, and what remains unverified |
| `packaging/`, `.github/workflows/release.yml` | How releases are built, packed and published for Windows, macOS and Linux |

`NOTICE.md` covers ownership, third-party data and the libraries 0.2.0 is built with.
