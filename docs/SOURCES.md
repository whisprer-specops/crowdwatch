# Source setup and expansion

CrowdWatch has nine collector types. A collector is an implemented parser and acquisition path, not a promise that every provider is available without registration. Provider coverage, terms, timestamp accuracy and measurement quality remain distinct from successful HTTP retrieval. The live checks for this release are recorded in `VALIDATION.md`.

## Included collectors

| `kind` | Input and configuration in `options` | Output and limits |
|---|---|---|
| `canonical_json` | Exactly one of `path` or HTTPS `url`; optional `bearer_env` for a URL | Canonical JSON object, array, `{ "observations": [...] }`, or NDJSON; preserves measurement meanings |
| `canonical_csv` | Same transport options | Canonical CSV; `details` is a JSON object inside a CSV cell |
| `mapped_json` | `path` or `url`, `items_pointer`, `fields`, `metric`, optional `period_seconds`, `total_pointer`, `bearer_env` | Explicit field mapping for another aggregate JSON feed; no guessed units or automatic pagination |
| `melbourne` | Optional `sensor_ids` (positive integers), `max_pages` (1–10, default 5) | One-minute passage counts joined to active sensor locations; filters by region radius; bounded pages and stale data are reported |
| `open_meteo` | Optional HTTPS `url` and `api_key_env` | Modelled current temperature, precipitation and wind at the region centre; weather context |
| `tfl` | Required `line_ids`; optional `api_key_env` | Whole-line service status; descriptive severity, not local crowd occupancy |
| `ticketmaster` | `api_key_env` (default `TICKETMASTER_API_KEY`), `max_pages` (1–5) | Nearby scheduled events; attendance unknown; explicit four-hour assumption if end time absent |
| `gbfs` | `station_information_url` and `station_status_url` | GBFS v2/v3 shared-vehicle and dock inventory, joined by station ID; operator rebalancing may explain changes |
| `rss` | `path` or `url`; optional `bearer_env`, `area_scoped` | Dated RSS/Atom items; GeoRSS points filtered by radius, or explicitly local feeds assigned to the region; context only |

All network acquisition uses HTTPS with normal certificate validation, against the Mozilla root certificates built into the program. Redirects are rejected, rather than forwarding credentials to an unexpected destination. The response limit is 5 MiB; import/collector batches are bounded at 20,000 observations. RSS/Atom accepts at most 2,000 items and rejects DTD/entity declarations. Canonical JSON and CSV must be UTF-8; an RSS or Atom feed may declare another encoding in its XML declaration. A failed batch is not partly inserted. Collection failures are isolated by source.

The client spaces its requests by at least 250 ms, uses a 15-second request timeout and honours HTTP Retry-After through the persisted source scheduler. It does not retry in a tight loop. Respect each provider's own allocation: a multi-page collection makes several HTTP requests, and a different configured source or process has its own client. These limits are not a global quota coordinator. To send requests through a proxy, set `HTTPS_PROXY`; HTTP and SOCKS proxies both work, and `socks5h://` keeps name lookups at the proxy, as Tor needs. Hosts in `NO_PROXY` are reached directly.

## Configuring a source

Each source needs a unique `id`, a `kind`, an existing `region_id`, and nonempty `attribution`. Defaults are enabled, 300 seconds between collection attempts, 900 seconds TTL and quality 0.8. `quality` is an operator assessment in [0,1], not a statistically calibrated probability. Use the same `independence_group` for mirrors, resold data or streams with a common origin. If omitted, the source ID is used, so configure it explicitly when two feeds overlap.

`interval_seconds` may range from 60 to 86,400; `ttl_seconds` from 30 to 604,800. Increasing TTL does not make an old measurement more current. Analysis uses the shorter of record expiry and configured TTL. Keep separate deployments in separate directories, each with its own configuration and SQLite file. Configurations in one directory otherwise share the default `crowdwatch.sqlite` filename.

The shipped `examples/london/config.json`, `examples/melbourne/config.json` and `examples/new-york/config.json` are the same configurations produced by `crowdwatch init`. Copy one into a working directory or use the init command in the main README. They are real source definitions, with credential-dependent collectors disabled until configured. Coordinates select an example area, not the user's location.

## Direct aggregate measurements

The Melbourne preset uses the City of Melbourne's [past-hour pedestrian counts](https://data.melbourne.vic.gov.au/explore/dataset/pedestrian-counting-system-past-hour-counts-per-minute/) and [sensor catalogue](https://data.melbourne.vic.gov.au/explore/dataset/pedestrian-counting-system-sensor-locations/). The exact API routes are fixed in `v0.2.0/src/sources.rs` (in 0.1.0, `crowdwatch/sources.py`). It sums neither sensors nor intervals into occupancy: the supplied `total_of_directions` is a passage count for a 60-second interval. An individual can pass a sensor more than once.

The example selects sensor 3 within 0.8 km of Melbourne Central and collects up to three pages of 100 newest records. Remove `sensor_ids` or set it to `[]` to use all active sensors within the configured radius; increase `max_pages` within its bound if appropriate. A larger area needs more requests and may hit the configured cap. This release observed delayed provider readings; they were correctly excluded from current evidence. Successful collection alone does not imply a usable current baseline.

For another municipality or a permissioned venue counter, use `mapped_json` when the feed already has explicit aggregate values and timestamps. Use canonical imports when adapting or normalizing a more complex source. `examples/mapped-json/` supplies a complete local fixture and configuration showing the mapping, including spatial filtering. JSON pointers use RFC 6901 syntax: `/data/counters` selects the list, `/count` selects each value, and `~1` escapes a slash in a key. Only fields actually configured are interpreted. If coordinates are omitted, the operator is explicitly assigning the entire feed to the region. `total_pointer` reports a partial page; it does not follow a pagination cursor.

`examples/canonical/observations.jsonl` and `observations.csv` encode the same historical synthetic records, with a complete configuration and replay commands in `examples/README.md`. They are examples of the contract, not a live sensor feed. For current collection, replace the file contents with newly measured observations from the configured source. `schemas/observation.schema.json` and `schemas/config.schema.json` describe the structural contracts; runtime validation also checks temporal ordering, source/region references, finite numbers and metric/unit agreement.

Canonical units are listed in `MODEL.md`. In particular, never label hourly passages as `occupancy`. Set `period_seconds` for footfall and precipitation. A configured capacity applies only to an exact `source_id/stream_id` occupancy reference, with a documented spatial boundary. An optional flow balance must use complete, synchronized occupancy, inflow and outflow measurements for that same boundary.

## Context providers

**Open-Meteo.** The [official API documentation](https://open-meteo.com/en/docs) describes modelled current weather and units. Default collection requests Unix timestamps, Celsius, millimetres and km/h. Its public service has non-commercial terms and data attribution requirements; choose the correct endpoint/arrangement for the deployment. An alternate endpoint can be set through `options.url`, with an environment variable named by `api_key_env`. The preset records attribution. Weather does not raise a movement alert on its own.

**TfL.** Use the [official Unified API portal](https://api.tfl.gov.uk/) and [API access portal](https://api-portal.tfl.gov.uk/) for the relevant access arrangements. The London preset lists Jubilee, District and Circle line IDs explicitly and is disabled initially. Set the issued key in the environment variable named in the configuration, then set `enabled` to `true`. A whole-line disruption may be far from the configured area. Unknown status descriptions are omitted with a warning; the numeric severity mapping belongs to CrowdWatch and is documented in the stored details. A keyless request returned HTTP 403 in this environment; credentialed live operation was not verified.

**Ticketmaster.** Obtain access through the [official Discovery API documentation](https://developer.ticketmaster.com/products-and-docs/apis/discovery-api/v2/). Set the key in `TICKETMASTER_API_KEY` and enable the configured source. The implementation uses the region's geohash, radius in km, a six-hour lookback and 24-hour lookahead, with up to 200 results per page and at most five pages. Cancelled, postponed and rescheduled records are excluded. Missing definite times/coordinates and exceeded page limits produce warnings. Listings are not attendance estimates. No user key was available for a live test.

**GBFS.** The [GBFS specification resources](https://gbfs.org/get-started/) describe discovery and station feeds. Configure the actual station-information and station-status URLs advertised by the chosen operator. The New York preset uses Citi Bike's public endpoints and was live-tested. It supports numeric v2 and ISO v3 station timestamps, station names in either representation and the corresponding inventory field names. It does not import vehicle trajectories, trips or rider identities. Check the chosen operator's licence; a standard protocol does not grant a universal redistribution licence.

**RSS/Atom.** Use a local authority, venue or transport bulletin feed whose coverage you understand. Dated GeoRSS items can be filtered by radius. Set `area_scoped: true` only when it is reasonable to assign every included item to that region. The collector does not geocode prose, perform sentiment scoring, assess the truth of a report or turn mentions into people counts. Titles are escaped in HTML. The fixture demonstrates a feed without relying on a third-party publisher.

## Where more sources fit

The extensible path is to add measurements with known semantics, not simply count API brands. These routes are deliberately separated from the implemented provider list:

| Candidate data | Route into this module | What must be established first |
|---|---|---|
| Municipal footfall, turnstile, venue occupancy or aggregate queue counters | Canonical JSON/CSV or mapped JSON | Units, interval, timestamp, coverage, boundary and calibration |
| Open-data portals using CKAN, Socrata or Opendatasoft | Mapped JSON for a bounded page, or a new provider adapter | Query and pagination contract, provider quota and redistribution terms |
| Public transport GTFS-Realtime alerts/arrivals | A future protobuf adapter, or upstream normalization to canonical context | Actual feed access, station/region association, delay semantics; no occupancy inferred from arrivals |
| Road counts or traffic speeds | A future explicit vehicle metric, kept separate from pedestrian metrics | Vehicle unit and spatial scope; do not relabel vehicle counts as people |
| Aggregate licensed mobility products | Permissioned normalization to the canonical contract | Aggregation, permitted use, sampling biases and appropriate spatial/time resolution |
| Street topology, exits and configured reference capacity | Region metadata or a future host-side spatial layer | Verified geometry and operational meaning; map features alone do not measure current occupancy |

No connector for these future routes is claimed to exist merely because it appears in this table. New collector handlers belong in `v0.2.0/src/sources.rs`. Each returns a `Batch` of observations and warnings, and needs fixture tests in `v0.2.0/tests/sources.rs` for field meaning, timestamps, missing values, spatial filtering and partial coverage. Keep credentials in environment variables and strip queries from provenance URLs. Never add personal identifiers to labels or arbitrary details to compensate for missing aggregate measurements.

## Operating a real pilot

Start with one area with a reliable direct counter. Record the counter's boundary, interval and expected delays; collect enough history to assess the baseline. Watch source-health warnings before interpreting an anomaly. Compare each projection with the persistence baseline through `backtest`. Define an independently observable target, such as an operator-confirmed queue spillback, before claiming predictive utility. Adding dozens of contextual sources cannot substitute for those measurements and outcomes.
