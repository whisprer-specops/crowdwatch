# Runnable examples

Run these commands from the repository folder, with `crowdwatch` installed as the main README describes. Each example directory has its own database path, resolved relative to its configuration file.


## Public providers

`london`, `melbourne` and `new-york` contain the corresponding starter configurations. For example:

```powershell
crowdwatch collect --config .\examples\melbourne\config.json
crowdwatch report --config .\examples\melbourne\config.json --out .\examples\melbourne\report.html
```

See `docs/SOURCES.md` for attribution, keys, intervals, TTL and actual coverage. The Melbourne counter may be delayed. Context-only examples correctly produce insufficient movement evidence. Use `init` into a new deployment directory for ongoing operation.

## Canonical JSON and CSV

The files in `canonical` contain the same three invented passage counts from 1 September 2026. Replay them at their supplied historical timestamps:

```powershell
crowdwatch import --config .\examples\canonical\config.json .\examples\canonical\observations.jsonl --historical-availability
crowdwatch report --config .\examples\canonical\config.json --at 2026-09-01T12:10:00Z --out .\examples\canonical\report.html
```

The historical flag is appropriate for this known fixture. For other files, use it only when the supplied retrieval timestamps reflect actual availability. Ordinary imports record availability now.

CSV alternative:

```powershell
crowdwatch import --config .\examples\canonical\config.json .\examples\canonical\observations.csv --format canonical_csv --historical-availability
```

Importing the second representation after the first should insert zero new versions: its records are the same. Three samples are deliberately insufficient for the configured baseline; the report states this. The complete synthetic demo supplies longer history and contrasting evidence states.

## Explicit JSON mapping

`mapped-json/counters.json` represents another sensor's JSON shape, with an explicit mapping in its configuration:

```powershell
crowdwatch collect --config .\examples\mapped-json\config.json
crowdwatch analyse --config .\examples\mapped-json\config.json
```

The fixture timestamps are historical. Collection records retrieval now; these old measurements are therefore stale for a current report. This demonstrates successful parsing without disguising an old file as live data. Replace the file with fresh measured aggregate records for actual operation. Mapping does not copy arbitrary fields into observations; its title identifies this fixture as synthetic.
