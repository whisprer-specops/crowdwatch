# Release validation

Each section records what was actually exercised for one release. It separates executable tests, comparisons, real provider observations, synthetic evaluation and what remains unverified. Test success is evidence about the implemented contracts, not proof of an error-free program or of predictive value for crowd harm.

## 0.2.0 — 14 October 2026

0.2.0 is 0.1.0 rewritten in Rust. The question for this release is whether it does what 0.1.0 did, so most of the evidence compares the two.

### Automated contracts

| Environment | Command, from `v0.2.0/` | Result |
|---|---|---|
| Rust 1.97.0, Linux x86_64 | `bash scripts/check.sh` | Formatting clean; 99 tests passed; strict lints (`clippy -D warnings`) clean; release build and demo made |

The 99 tests are 40 unit tests, the 39 contract tests and 12 collector tests ported one for one from 0.1.0's 51 Python tests, 6 command-line tests and 2 copyright-notice tests. None needs the network. The unit tests cover the Windows-only rules by running them on Linux with those rules switched on: Python's path spelling (`pathlib`), program lookup (`shutil.which`) and the C runtime's wording for file errors.

### Comparison with 0.1.0

The same inputs went through 0.1.0, on CPython 3.13.16, and through 0.2.0, and the outputs were compared byte for byte. Python 3.14's rules were applied where 3.13 differs. These rules cover time parsing (ported from CPython 3.14's `_datetimemodule.c`), option-error wording, the Unicode 16.0 character tables and the codec names 3.14 added.

| Comparison | Scale | Result |
|---|---|---|
| Generated cases: JSON reading, time parsing, URL cleaning, observation checks, canonical imports, geohashes, distances and number formatting, plus made-up provider replies for all nine collector kinds | About 41,000 cases, about 8,200 of them replies | Identical, apart from time strings on which Python 3.13 and 3.14 disagree |
| Random deployments, each taken through 13 commands: imports, analyses, reports, backtests, an export, `doctor` and `prune` | 36 deployments | Identical printed output, files and database rows |
| Help, usage errors, failures and exit codes | 94 command lines | Identical, apart from the differences listed in `MIGRATION.md` and outputs that contain the current time |
| The demonstration at `--at 2026-10-01T09:00:00Z` | 15 files, 396 observations | Identical files and database rows, apart from the version number |

Proxies were tested against local stand-ins. An HTTP proxy received `CONNECT api.open-meteo.com:443`, and a SOCKS5 proxy, set as `socks5h://`, received the host name rather than an address. An `ftp://` proxy address made requests fail rather than go direct. An empty `https_proxy` counted as unset, as in Python.

### SkeleTrace host check

SkeleTrace 1.0.0, built from `whisprer-specops/osint-skeletrace-standalone` at commit `29fbef24071ea64b2002091a5f403a31c6bcef75`, accepted the demonstration's export. `crowdwatch verify-skeletrace`, given that build with `--host`, printed "Host accepted the profile and read all 15 sample records." and exited with 0.

This closes the host execution test that 0.1.0 left outstanding. The check ran the host's `profile-validate` and `poll-source` commands, and `Tick.samples_seen` was 15, with no failed sources. The same check passed with the program found on `PATH`, and through 0.1.0's own `tools/verify_skeletrace.py`. A bridge that feeds snapshots into a long-running SkeleTrace engine is still a separate, host-side task (`SKELETRACE.md`).

### Speed

Median of five runs on one sandbox core, including program start:

| Command | 0.1.0 (Python 3.13) | 0.2.0 |
|---|---:|---:|
| `demo` at a fixed `--at` | 2.09 s | 0.38 s |
| `backtest` on the demonstration | 1.93 s | 0.33 s |
| `analyse` on the demonstration | 0.12 s | 0.01 s |

The Linux release program is about 5 MB.

### Not verified for 0.2.0

* **Windows itself.** The build environment was Linux. Windows-specific behaviour is written in portable Rust and unit-tested with Windows rules, but the first Windows build and `check.ps1` run happen on the user's machine.
* **Live providers.** The build environment could not reach the providers' servers, so 0.2.0's collectors were checked against 0.1.0 with made-up replies rather than live ones. 0.1.0's live observations below still describe the providers' contracts.
* **macOS**, and credentialed TfL and Ticketmaster access, as for 0.1.0.

The operational limits and the evidence still needed, listed for 0.1.0 below, apply unchanged.

## 0.1.0 — 1 October 2026

Validated 1 October 2026. This record distinguishes executable tests, real provider observations, synthetic evaluation and unverified integration. Test success is evidence about the implemented contracts, not proof of an error-free program or of predictive value for crowd harm.

### Automated contracts

| Environment | Command from the project root | Result |
|---|---|---|
| CPython 3.12.14, Linux x86_64 | `python -m unittest discover -v` | 51 tests passed, 1.281 seconds |
| CPython 3.14.7, Linux x86_64 | `python3.14 -m unittest discover -v` | 51 tests passed, 1.212 seconds |

Full logs are in `docs/evidence/python-312-tests.txt` and `python-314-tests.txt`. These are fixture/contract tests and do not require network access. Use discovery from the project root as shown; if specifying `-s tests`, also specify `-t .` so relative test imports resolve.

Covered behavior includes finite values and units, explicit timestamps, missing versus zero, invalid JSON, interval identity, equivalent CSV/JSON numeric representations, duplicate acquisition, atomic batches, revision replay, corrections reverting to an earlier value, version-1 database migration, overlapping collection claims, retry scheduling, source failure isolation, stale/low-quality exclusion, independence groups, baseline warmup, sustained anomalies, limited projections, gap refusal, conditional area balance, availability-aware backtesting, all nine collector types, GBFS v2/v3, HTML escaping, GeoJSON coordinate order and inspected native serialization shapes.

The final example replay inserted three JSON records, then zero versions from the equivalent CSV. The mapped JSON example inserted three records while retaining their historical observation times. Both JSON Schemas were checked with `jsonschema` 4.25.1; all five example configurations, the demo configuration, three canonical example observations and 396 demo observations validated. That validator is a build-time check only; the runtime has no third-party dependencies.

### Package and report checks

The wheel was built with setuptools 84.0.0 and wheel 0.48.0 using:

```bash
python -m pip wheel --no-deps --no-build-isolation --wheel-dir dist .
```

The supplied wheel was then installed with `--no-index --no-deps` in a fresh CPython 3.14 virtual environment. From outside the source directory, isolated Python (`-I`) successfully ran `-m crowdwatch --version` and created the complete offline demo. The installed console entry point ran `doctor`; SQLite integrity was `ok`. This verifies that `demo.py`, report assets and entry points are actually in the wheel, rather than accidentally being imported from the source checkout.

The demo generated 396 observations and the expected region states: station `corroborated`, plaza `baseline`, market `insufficient_data`. It produced HTML/JSON reports, the backtest result, configuration/fixtures, a SQLite store and native snapshot files. The portable pre-rendered report is also shipped in `preview/report.html`; regenerate the demo to obtain paths valid on your own machine.

The HTML report was opened in headless Chromium 134 (Playwright browser build 1161) at 1360×1024 and 390×844 viewports. Both showed all three area sections, three time-series charts and the synthetic-data notice; there were no page errors or document-level horizontal overflow. Full-page screenshots were visually inspected. Tables can scroll inside their own containers on narrow screens. The report contains no external scripts, fonts, images or network dependency; expandable provenance uses native HTML `details` elements.

Windows commands are documented and the implementation uses portable standard-library paths and atomic writes, but **Windows itself was not executed** in this Linux environment. Python 3.14 compatibility testing is not a substitute for an installed Windows/Skeletrace smoke test. Python 3.11 and 3.13 were not separately exercised.

### Actual public-source smoke checks

These checks were made against live HTTPS endpoints, not mocked fixtures. They are observations at the listed times, not guarantees about subsequent provider availability. The recorded collector health is in `docs/evidence/live-source-health.json`.

| Provider and time (UTC) | Observed result | What it establishes |
|---|---|---|
| City of Melbourne, 2026-10-01 08:26:33 | 300 pedestrian observations from the bounded three-page query; location join succeeded; page cap and stale-data warnings recorded | The real API contract worked. The newest returned pedestrian data was delayed, so current direct evidence was excluded. |
| Open-Meteo, 2026-10-01 08:26:33 | Three observations collected successfully with explicit returned units | The real current-weather endpoint and parsing worked; these are modelled weather context. |
| Citi Bike GBFS, 2026-10-01 08:29:12 | 76 observations from 38 stations inside the region | Real station-information/status joining and inventory parsing worked. This does not establish pedestrian occupancy. |
| TfL, keyless request during source inspection | HTTP 403 | Authentication/access is needed for that request in this environment. No credentialed live success is claimed. |
| Ticketmaster | No user API key supplied; fixture coverage only | Request construction, geohash, parsing and credential redaction were tested, not live account access. |
| Canonical JSON/CSV, mapped JSON and RSS/Atom | Local synthetic/fixture tests | Parsers and common transport are exercised; no particular third-party custom feed is certified. |

At the Melbourne report cutoff, the result was correctly `insufficient_data`: zero fresh direct streams, one stale pedestrian stream and three weather observations. It did not turn source success, weather or old counts into a current crowd assertion. Real forecast accuracy could not be evaluated from this short, delayed acquisition window.

### Synthetic projection evaluation

The fixed demonstration at `2026-10-01T09:00:00Z` evaluates a 15-minute horizon with six nonoverlapping origins per series. These results are included to expose the evaluation machinery and its failures, **not** to estimate real-world performance. The synthetic trajectory helped develop the demonstration and is not an independent external test set.

| Synthetic series | MAE | Persistence MAE | Skill versus persistence |
|---|---:|---:|---:|
| Station footfall, people/5 min | 10.060 | 17.083 | 0.411 |
| Station occupancy, people | 31.619 | 55.833 | 0.434 |
| Plaza footfall, people/5 min | 1.887 | 1.667 | -0.132 |

Positive skill means lower MAE than holding the last value; negative skill is worse. The quiet plaza illustrates that the trend-selection heuristic does not always beat persistence. Abrupt onset also creates forecast lag. The module reports those results without suppressing poorly performing streams. Displayed sensitivity ranges are uncalibrated and are not confidence levels or harm probabilities.

### SkeleTrace boundary

The native serialization tests are based on the actual Rust source at the commit and path in `SKELETRACE.md`: `EngineProfile`, `NdjsonFile`, `FileSampleRecord`, enums, durations, UUIDs and source definitions. The exported immutable snapshots and reload rule account for the host adapter's line cursor.

No matching Skeletrace executable or native plugin-loader contract was available here. **A host runtime integration test remains outstanding.** Use `tools/verify_skeletrace.py` against the intended installed executable. A sidecar bridge to a long-lived engine and UI remains the host-side integration task; this release does not claim to have installed that bridge into the suite.

### Operational limits and next evidence needed

* A newly collected stream needs history and sufficient fresh direct measurements. Context-only collection correctly remains insufficient for movement inference.
* The baseline uses UTC time-of-day and weekday/weekend grouping, with an explicitly labelled fallback. Local daylight-saving time, holidays, special events, calibration changes and seasonal shifts are not fully modelled.
* Source quality and independence groups are assigned by the operator. Multiple providers can still share an undisclosed origin or correlated sampling bias.
* Provider requests and response sizes are bounded, but analysis loads the configured region's retained observation window into memory. Backtesting repeatedly queries historical windows. This release is suitable for a small area pilot, not a benchmarked city-wide streaming platform. Use retention, source intervals and sensible area scopes; profile the intended workload before expanding it.
* `watch` is a foreground process and reloads configuration each cycle. It does not install a service, auto-update the package or restart itself after a machine reboot. Native snapshot retention is separate from database pruning; remove superseded snapshot directories only after the host no longer uses them.
* An API's continued availability, licence, quota and schema can change. Source failure and partial coverage are visible states. The deployment must use provider terms appropriate to its purpose.
* Predicting a defined adverse outcome requires independent outcome labels, held-out venues/dates, calibration and false-alarm/lead-time evaluation. None of those empirical results is supplied by the original archive or fabricated by this release.

The observed evidence supports confidence in the delivered collection, replay, descriptive analysis, report and export mechanisms within their tested scope. It does not support a numerical reliability guarantee for unseen providers, native host versions or real-world hazard forecasts.
