# Measurement and inference contract

## Quantities

| Metric | Unit | Meaning |
|---|---|---|
| `footfall` | `people/interval` | Passages during an explicit interval; not unique people or occupancy |
| `occupancy` | `people` | Aggregate people present in a stated bounded area |
| `density` | `people/m2` | Supplied measured density; never derived from a street passage counter |
| `inflow`, `outflow` | `people/min` | Directional rates across a defined boundary |
| `speed` | `m/s` | Aggregate measured pedestrian speed |
| `transit_disruption` | `index` | Descriptive disruption severity in [0,1]; not local crowd load |
| `scheduled_event` | `event` | An event listing; attendance unknown |
| `temperature` | `degC` | Context; Open-Meteo uses modelled weather |
| `precipitation` | `mm/interval` | Precipitation during the specified interval |
| `wind` | `km/h` | Weather context |
| `bikes_available`, `docks_available` | `vehicles`, `docks` | Station inventory; operator rebalancing can change it |
| `report` | `report` | A dated RSS/Atom report; no inferred attendance, danger or truth score |

No demographic, psychological or identity features are collected. Regions describe monitoring areas; the module does not track individual phone locations or build personal movement dossiers.

## Time, revisions and identity

All timestamps require an explicit offset and are normalized to UTC. `observed_at` is the measurement time, `retrieved_at` is when it was available, and `valid_until` is its maximum validity. Analysis additionally caps validity using the configured source TTL. A future planned event stores its scheduled time in `details.starts_at`; its observation time is the fetch time. Null, absent and stale readings are not numeric zero.

An observation key hashes `(source_id, stream_id, region_id, metric, period_seconds, observed_at)`. A content digest excludes retrieval time, with measured numbers normalized so equivalent CSV/JSON representations match. A repeated record identical to the latest known version at its availability time is ignored. Changed records become immutable versions, including corrections that revert to an earlier value; an as-of query picks the latest version retrieved before its cutoff. The SQLite schema is version 2 and migrates the initial version 1 structure without deleting history. Ordinary network acquisition and file import record availability now. The explicitly named historical-import option trusts supplied availability times for replay.

Baselines never mix sources, streams, metrics or count intervals. Historical coordinates must equal the latest stream coordinates, so a relocated sensor does not silently retain its former baseline. Configured area-scoped feeds with no coordinates rely on the operator's assignment to that region.

## Baseline and unusualness

For each fresh, sufficiently good direct stream:

1. Exclude the most recent hour from its baseline reference.
2. Prefer observations from previous dates within ±30 minutes of the current UTC time of day and the same weekday/weekend class.
3. If fewer than the configured minimum exist, use at most 2,000 recent eligible historical observations. Mark this as a fallback; it is not seasonal normalization.
4. Let `m = median(reference)` and `s = max(1.4826*MAD(reference), 0.1*abs(m), 0.1)`.
5. Compute `z = (current - m)/s`, reversing the sign for speed so a slowdown is positive.

The scale floor is a stability heuristic, not an estimated sampling distribution. The UTC seasonal baseline does not correct for local daylight-saving changes, holidays, special events, season changes or changes in counter population. Configurations should be segmented or recalibrated where those matter.

A positive anomaly requires z ≥ 3 by default. Sustained means the last two distinct samples exceed that threshold, are no more than 15 minutes apart, and have suitably recent validity. Repeated downloads of one sample cannot satisfy persistence. A high exit-flow reading is visible but does not independently raise the descriptive alert.

The anomaly index is `100*clamp(max(positive z)/6, 0, 1)`, excluding outflow. It is the strongest comparable direct-stream deviation, not a fused probability or measured physical crowd pressure. Sources are not added together: multiple counters may observe the same people.

## Evidence status

| Status | Exact use |
|---|---|
| `insufficient_data` | No usable baseline or configured occupancy comparison. Context-only areas remain here. |
| `baseline` | Available comparable measurements do not trigger the configured rules. It does not mean “safe”. |
| `watch` | One unusual reading; or at least 85% of an operator-configured occupancy reference; or a projection range/flow balance reaches that reference. |
| `elevated` | A sustained unusual direct stream, or observed occupancy at/above its configured reference. |
| `corroborated` | Sustained direct anomalies from at least two configured independent origin groups. This is evidence corroboration, not proof of causation. |

`independence_group` is operator-supplied. Two mirrors of one sensor, two directional fields from the same hardware, or two suppliers reselling the same telemetry must share a group. The software cannot discover hidden common origins. For known mirrors, grouping prevents false corroboration but does not magically make their estimates independent.

Context is joined by the explicitly configured region and valid time. Reports state when disruptions or nearby events are present, but context does not manufacture a count or elevate a context-only area. News stories and scheduled events are kept as individual records rather than summed into “event intensity”.

## Short-term projection

The forecaster uses at most eight recent observations, within two hours, with at least six observations and ten minutes of span. It refuses series with excessive gaps. Horizon is the minimum of the requested horizon, half that fitted span, and 30 minutes. The target is relative to the last observation, not the report generation time.

Ordinary least squares estimates a local slope and intercept. Recent one-step holdouts compare it with persistence; trend is selected only when its absolute error sum improves on persistence by more than 5%. Otherwise the projection holds the latest value. It uses no future observation at its forecast origin.

The displayed range is a residual-based sensitivity range: two robust residual scale units, with a 5%-of-level/0.1 floor and a horizon/span widening term. Values are floored at zero. **It is not a 95% confidence interval, a calibrated prediction interval, or a probability of a harmful outcome.** Abrupt regime changes can outrun either forecast; the rolling-origin backtest reports MAE, bias and skill against persistence without hiding worse performance.

Backtesting only uses records available at each forecast origin. Later values are targets, never inputs. Origins are at least one requested horizon apart and require an exact latest observation at the origin. Late historical backfill cannot masquerade as information known in real time. An empty evaluation is reported explicitly. The synthetic demonstration checks machinery, not real-world forecast skill.

## Optional area balance

For a specifically configured occupancy, entrance and exit stream referring to the same bounded area:

`N(t+h) = max(0, N(t) + h*(inflow - outflow))`.

Units are people, minutes and people/minute. All three measurements must be fresh, unambiguous and within two minutes of one another. The projection is relative to the occupancy observation. A configured reference may be crossed after `(capacity - N)/(inflow - outflow)` minutes when net inflow is positive. The model does not clip occupancy at capacity, since doing so would hide an exceedance.

This is conservation bookkeeping under constant-rate assumptions. Complete entrance/exit coverage, a consistent boundary, calibrated counts and the meaning of the configured reference are operator responsibilities. There is no universal safe density or capacity hardcoded in the module. The synthetic demo's capacity is invented.

## Empirical limits and research basis

Sparse public observations cannot measure local force, person-to-person contacts, velocity variance or all egress constraints. The physical crowd-pressure quantities discussed in Helbing, Johansson and Al-Abideen's [empirical crowd-disaster study](https://arxiv.org/abs/physics/0701203) require substantially richer measurements. CrowdWatch does not claim to implement or validate that study's indicators from weather or event listings.

The original Hartman–Grobman analogy is kept in the archive review as a research idea, not part of the runtime. Establishing adverse-state dynamics would require identified equations, state estimates, meaningful equilibria and independent outcome validation. The present module supplies data contracts, evidence, replay and baseline tools needed before attempting that research.
