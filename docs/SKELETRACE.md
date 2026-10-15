# SkeleTrace integration contract

## Host versions

The adapter targets SkeleTrace's Rust serialization and file-ingestion contract. CrowdWatch 0.1.0 read it from `whisprer-specops/osint-skeletrace`, commit `befa8404fb6f494315eeaa3639f263498fb89e26`, under `previous-versions/st12fix`. That Cargo package identifies itself as SkeleTrace 0.11.0; the directory name is not a reliable API version.

CrowdWatch 0.2.0 writes the same files, byte for byte apart from the version number in `module.json`. On 14 October 2026 SkeleTrace 1.0.0, built from `whisprer-specops/osint-skeletrace-standalone` at commit `29fbef24071ea64b2002091a5f403a31c6bcef75`, accepted them. `profile-validate` passed, and `poll-source` read all 15 sample records of the demonstration export, with no failed sources.

Relevant source files at the 0.11.0 revision:

* [profile.rs](https://github.com/whisprer-specops/osint-skeletrace/blob/befa8404fb6f494315eeaa3639f263498fb89e26/previous-versions/st12fix/src/profile.rs): `EngineProfile`, `SourceProfile`, `AdapterProfile` and instantiation.
* [adapter.rs](https://github.com/whisprer-specops/osint-skeletrace/blob/befa8404fb6f494315eeaa3639f263498fb89e26/previous-versions/st12fix/src/adapter.rs): `FileSampleRecord` and the NDJSON file adapter.
* [metric.rs](https://github.com/whisprer-specops/osint-skeletrace/blob/befa8404fb6f494315eeaa3639f263498fb89e26/previous-versions/st12fix/src/metric.rs), [types.rs](https://github.com/whisprer-specops/osint-skeletrace/blob/befa8404fb6f494315eeaa3639f263498fb89e26/previous-versions/st12fix/src/types.rs), [entity.rs](https://github.com/whisprer-specops/osint-skeletrace/blob/befa8404fb6f494315eeaa3639f263498fb89e26/previous-versions/st12fix/src/entity.rs) and [ingest.rs](https://github.com/whisprer-specops/osint-skeletrace/blob/befa8404fb6f494315eeaa3639f263498fb89e26/previous-versions/st12fix/src/ingest.rs): metric, value, identifier, position, node and source types.
* [operator.rs](https://github.com/whisprer-specops/osint-skeletrace/blob/befa8404fb6f494315eeaa3639f263498fb89e26/previous-versions/st12fix/src/operator.rs) and [src/bin/skeletrace.rs](https://github.com/whisprer-specops/osint-skeletrace/blob/befa8404fb6f494315eeaa3639f263498fb89e26/previous-versions/st12fix/src/bin/skeletrace.rs): the actual profile validation and poll commands.

No dynamic plugin discovery or registration API was found there, and SkeleTrace 1.0.0 has none either: its source adapters are built in. CrowdWatch is therefore an independently installable collection and analysis module with a concrete native import boundary. `module.json` describes that boundary; it is not presented as a manifest for a loader that does not exist. No files in the host repository were changed.

## Export on the destination machine

In PowerShell, with `crowdwatch` installed as the README describes:

```powershell
crowdwatch demo --out .\my-demo
crowdwatch export-skeletrace --config .\my-demo\config.json --out .\native-export
```

For a real deployment, substitute its configuration. The export contains:

| File | Meaning |
|---|---|
| `skeletrace-profile.json` | Full `EngineProfile`: region anchors, five metric definitions and one manual `NdjsonFile` source |
| `samples-<digest>.jsonl` | Immutable snapshot of serialized `FileSampleRecord` objects; five samples per region |
| `areas.geojson` | Standard GeoJSON point features at configured area centres, with radius and evidence properties |
| `module.json` | CrowdWatch metadata, source UUID, host-reference commit, hashes, paths and reload rule |

The profile contains an **absolute local sample-file path**. Regenerate after moving the output or transferring it to another computer. Exporting on Windows produces the appropriate Windows path and JSON escaping automatically. Regenerating the offline demo is the easiest way to create a valid local demonstration profile.

The region point is the configured area centre, not a sensor's exact location and not a person's location. Its radius is descriptive area metadata. UUIDs are deterministic across exports using a fixed namespace and the region/metric names. Rename a region ID only if creating a new logical entity is intended.

## Values and serialization

| Metric | SkeleTrace value | Interpretation |
|---|---|---|
| `status` | `{"Code":"watch"}` and the other named statuses | Descriptive evidence state |
| `anomaly_index` | `{"Numeric":number}` or `"Missing"` | Strongest comparable positive deviation scaled to 0–100; no probability interpretation |
| `direct_streams` | `{"Numeric":number}` | Fresh direct movement streams above the configured quality threshold |
| `stale_streams` | `{"Numeric":number}` | Expired observed streams excluded from inference |
| `context_records` | `{"Numeric":number}` | Fresh contextual items, not headcounts |

Identifiers serialize as UUID strings. Timestamps use UTC RFC 3339 strings. The duration shape is `{"secs":300,"nanos":0}`. The profile uses `adapter_profile: "NdjsonFile"`, source kind `"File"` and adapter `"FileImport"`. A numeric missing value is the enum string `"Missing"`, never a zero measurement.

Each sample includes `entity_id`, `metric_id`, `ts_observed`, `ts_ingested`, `value`, `quality` and `raw_payload`. Evidence, original stream timestamps, source attribution, reasons and model limitations are retained in `raw_payload`. Summary quality 1.0 indicates successful serialization of the computed summary, not perfect real-world knowledge. An unknown anomaly index has quality 0.0 and `Missing`. Node state is `Stale` for insufficient evidence, `Alerting` for elevated/corroborated evidence, and `Active` otherwise; read the explicit status metric for the finer distinction.

## Run the host check

```powershell
crowdwatch verify-skeletrace .\native-export\skeletrace-profile.json --host C:\path\to\skeletrace.exe
```

Leave out `--host` when `skeletrace.exe` is on your `PATH`, or on Windows in the current folder; it is found the way Python's `shutil.which` finds a program. The check confirms the sample file exists, runs the host's `profile-validate` and `poll-source` commands, and requires the returned `Tick.samples_seen` to equal the number of NDJSON records, with no failed sources. On success it prints "Host accepted the profile and read all 15 sample records." (for the demonstration) and exits with 0. Otherwise it prints the host's response and exits with a nonzero code: 2 if the host program or the sample file cannot be found, the host's own code if a host command fails, and 1 for any other failure. 0.1.0 ran the same check as `py .\tools\verify_skeletrace.py`.

Equivalent direct PowerShell commands:

```powershell
skeletrace profile-validate .\native-export\skeletrace-profile.json
$module = Get-Content .\native-export\module.json -Raw | ConvertFrom-Json
skeletrace poll-source .\native-export\skeletrace-profile.json $module.source_id
```

These subcommands exist in SkeleTrace 0.11.0 and 1.0.0. A later build may have another interface; run the check after upgrading SkeleTrace.

## Snapshot lifecycle and a native plugin's next step

The reference `NdjsonFileAdapter` tracks a line cursor and expects append-oriented input. CrowdWatch writes content-addressed immutable snapshots so replacing a report cannot silently leave a reused adapter at end-of-file. For each new snapshot, load its profile into a fresh adapter/engine instance. `watch --skeletrace` produces these snapshots each collection cycle; it does not inject them into a running host or discover the host's UI.

The reference CLI creates a new engine for each command. Running `poll-source` and then a separate `query-latest` command does not preserve the first command's in-memory samples. In a long-lived host, perform profile instantiation, source polling and view/query operations on the **same** engine instance. To retain native history across snapshots, the host needs an explicit ingestion bridge that submits records into the persistent engine, or an append-only feed with rotation/checkpoint handling. That bridge is the concrete next development step once the intended host version and its plugin lifecycle are fixed.

For host integration, keep the current boundary small: launch and monitor the CrowdWatch sidecar; load its versioned report or sample stream; place configured area anchors on the existing map; show status and a link or panel for evidence; preserve `insufficient_data` and source-failure states. Keep the inference in CrowdWatch rather than writing a second copy inside SkeleTrace, at least until its measurements and labels have been validated in a real pilot.
