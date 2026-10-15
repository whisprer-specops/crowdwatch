// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Transparent descriptive signals. No learned psychology, causal or safety labels.
//!
//! The arithmetic is 0.1.0's, operation for operation: Python's compensated `sum()`, its
//! `statistics.median`, its `pow` for squares, and its habit of returning the integer
//! `0` from `max(0, x)` all reach the report, so they are kept.

use std::cmp::Ordering;

use crate::config::{number, Config};
use crate::error::{Fail, Result};
use crate::json::{py_eq, Dict, Value};
use crate::model::{is_direct, Observation, StreamKey};
use crate::pyfmt::{float_sum, format_fixed, median};
use crate::pytime::{parse_time_str, span_of, Instant, Num, Span};
use crate::sources::square;
use crate::store::Store;

const MINUTE_US: i64 = 60_000_000;

// ---------------------------------------------------------------------------------------
// Python's mixed int and float comparisons
// ---------------------------------------------------------------------------------------

/// An integer against a float, exactly, as Python compares them.
fn cmp_int_float(i: i64, f: f64) -> Option<Ordering> {
    const TWO_63: f64 = 9_223_372_036_854_775_808.0;
    if f.is_nan() {
        return None;
    }
    if f >= TWO_63 {
        return Some(Ordering::Less);
    }
    if f < -TWO_63 {
        return Some(Ordering::Greater);
    }
    let whole = f.trunc();
    match i.cmp(&(whole as i64)) {
        Ordering::Equal => (whole).partial_cmp(&f),
        other => Some(other),
    }
}

/// Python's comparison of two numbers.
pub fn num_cmp(a: Num, b: Num) -> Option<Ordering> {
    match (a, b) {
        (Num::Int(x), Num::Int(y)) => Some(x.cmp(&y)),
        (Num::Float(x), Num::Float(y)) => x.partial_cmp(&y),
        (Num::Int(x), Num::Float(y)) => cmp_int_float(x, y),
        (Num::Float(x), Num::Int(y)) => cmp_int_float(y, x).map(Ordering::reverse),
    }
}

fn lt(a: Num, b: Num) -> bool {
    num_cmp(a, b) == Some(Ordering::Less)
}

fn gt(a: Num, b: Num) -> bool {
    num_cmp(a, b) == Some(Ordering::Greater)
}

fn ge(a: Num, b: Num) -> bool {
    matches!(num_cmp(a, b), Some(Ordering::Greater | Ordering::Equal))
}

fn le(a: Num, b: Num) -> bool {
    matches!(num_cmp(a, b), Some(Ordering::Less | Ordering::Equal))
}

/// Python's `max(a, b)`: `a` unless `b` is larger.
fn py_max(a: Num, b: Num) -> Num {
    if gt(b, a) {
        b
    } else {
        a
    }
}

/// Python's `min(a, b)`: `a` unless `b` is smaller.
fn py_min(a: Num, b: Num) -> Num {
    if lt(b, a) {
        b
    } else {
        a
    }
}

fn float(f: f64) -> Num {
    Num::Float(f)
}

/// `a - b` for Python numbers.
fn sub(a: Num, b: Num) -> Num {
    match (a, b) {
        (Num::Int(x), Num::Int(y)) => x
            .checked_sub(y)
            .map_or(Num::Float(x as f64 - y as f64), Num::Int),
        _ => Num::Float(a.as_f64() - b.as_f64()),
    }
}

/// `abs(x)`.
fn abs(a: Num) -> Num {
    match a {
        Num::Int(x) => x
            .checked_abs()
            .map_or(Num::Float((x as f64).abs()), Num::Int),
        Num::Float(x) => Num::Float(x.abs()),
    }
}

/// `int(x)` of a configuration number.
fn truncate(a: Num) -> i64 {
    match a {
        Num::Int(x) => x,
        Num::Float(x) => x.trunc() as i64,
    }
}

fn parse(text: &str) -> Result<Instant> {
    parse_time_str(text)
}

/// `statistics.median()` of data known to be non-empty.
fn median_of(values: &[f64]) -> Result<f64> {
    median(values).ok_or_else(|| Fail::py("StatisticsError", "no median for empty data"))
}

/// True division, with Python's ZeroDivisionError.
fn divide(a: f64, b: f64) -> Result<f64> {
    if b == 0.0 {
        Err(Fail::py("ZeroDivisionError", "float division by zero"))
    } else {
        Ok(a / b)
    }
}

// ---------------------------------------------------------------------------------------
// Baselines and projections
// ---------------------------------------------------------------------------------------

/// The scale of the deviations around `level`, with a floor so a constant counter cannot
/// create infinite alarms.
pub fn robust_scale(values: &[f64], level: f64) -> Result<f64> {
    let deviations: Vec<f64> = values.iter().map(|v| (v - level).abs()).collect();
    let mad = median_of(&deviations)?;
    let mut scale = 1.4826 * mad;
    for candidate in [level.abs() * 0.1, 0.1] {
        if candidate > scale {
            scale = candidate;
        }
    }
    Ok(scale)
}

/// A reference for judging the current reading.
#[derive(Clone, Debug, PartialEq)]
pub struct Baseline {
    pub median: f64,
    pub scale: f64,
    pub samples: usize,
    pub seasonal: bool,
    pub reference_start: String,
    pub reference_end: String,
}

impl Baseline {
    pub fn to_value(&self) -> Value {
        Value::Dict(
            Dict::new()
                .with("median", self.median)
                .with("scale", self.scale)
                .with("samples", self.samples)
                .with(
                    "mode",
                    if self.seasonal {
                        "UTC time-of-day / weekday-or-weekend"
                    } else {
                        "recent history fallback"
                    },
                )
                .with("reference_start", self.reference_start.as_str())
                .with("reference_end", self.reference_end.as_str()),
        )
    }
}

/// 0.1.0's `baseline(history, current, minimum)`: comparable earlier readings (same UTC
/// time of day within 30 minutes on other days, weekday or weekend alike), or recent
/// history when there are too few. The last hour is left out, so a surge is not learned
/// as normal.
pub fn baseline(
    history: &[Observation],
    current: &Observation,
    minimum: i64,
) -> Result<Option<Baseline>> {
    let t = parse(&current.observed_at)?;
    let mut candidates: Vec<&Observation> = Vec::new();
    if !history.is_empty() {
        let cutoff = t.sub(Span::hours(1))?;
        for o in history {
            if parse(&o.observed_at)? <= cutoff {
                candidates.push(o);
            }
        }
    }
    let minute = t.minute_of_day();
    let weekend = t.weekday() >= 5;
    let mut seasonal: Vec<&Observation> = Vec::new();
    for o in &candidates {
        let dt = parse(&o.observed_at)?;
        let apart = (dt.minute_of_day() - minute).abs();
        let delta = apart.min(1440 - apart);
        if dt.date() != t.date() && delta <= 30 && (dt.weekday() >= 5) == weekend {
            seasonal.push(o);
        }
    }
    let count = |n: usize| i64::try_from(n).unwrap_or(i64::MAX);
    let use_seasonal = count(seasonal.len()) >= minimum;
    let used: Vec<&Observation> = if use_seasonal {
        seasonal
    } else {
        candidates[candidates.len().saturating_sub(2000)..].to_vec()
    };
    if count(used.len()) < minimum {
        return Ok(None);
    }
    let (Some(first), Some(last)) = (used.first(), used.last()) else {
        return Ok(None);
    };
    let values: Vec<f64> = used.iter().map(|o| o.value).collect();
    let centre = median_of(&values)?;
    Ok(Some(Baseline {
        median: centre,
        scale: robust_scale(&values, centre)?,
        samples: used.len(),
        seasonal: use_seasonal,
        reference_start: first.observed_at.clone(),
        reference_end: last.observed_at.clone(),
    }))
}

/// A short conditional projection of one stream.
#[derive(Clone, Debug, PartialEq)]
pub struct Projection {
    pub trend: bool,
    pub horizon: Num,
    pub at: String,
    pub value: Num,
    pub lower: Num,
    pub upper: f64,
    pub slope: Num,
    pub fitted: usize,
}

impl Projection {
    pub fn to_value(&self) -> Value {
        Value::Dict(
            Dict::new()
                .with(
                    "method",
                    if self.trend {
                        "recent least-squares trend"
                    } else {
                        "persistence (recent holdouts did not favor trend)"
                    },
                )
                .with("horizon_minutes", self.horizon.to_value())
                .with("at", self.at.as_str())
                .with("value", self.value.to_value())
                .with("lower", self.lower.to_value())
                .with("upper", self.upper)
                .with("slope_per_minute", self.slope.to_value())
                .with("fitted_samples", self.fitted)
                .with(
                    "range_meaning",
                    "residual-based sensitivity range; coverage is not calibrated",
                ),
        )
    }
}

/// `(b - a).total_seconds() / 60`.
fn minutes_between(a: Instant, b: Instant) -> f64 {
    b.since(a).total_seconds() / 60.0
}

/// 0.1.0's `project(history, horizon_minutes)`: a least-squares trend over the last two
/// hours (at most eight readings), used only when recent one-step holdouts favour it over
/// persistence, and never beyond half the fitted span or 30 minutes.
pub fn project(history: &[Observation], horizon_minutes: Num) -> Result<Option<Projection>> {
    if history.len() < 6 {
        return Ok(None);
    }
    let mut sorted: Vec<&Observation> = history.iter().collect();
    sorted.sort_by(|a, b| a.observed_at.cmp(&b.observed_at));
    let tail = &sorted[sorted.len().saturating_sub(8)..];
    let last = parse(&tail[tail.len() - 1].observed_at)?;
    let since = last.sub(Span::hours(2))?;
    let mut rows: Vec<&Observation> = Vec::with_capacity(tail.len());
    for o in tail {
        if parse(&o.observed_at)? >= since {
            rows.push(o);
        }
    }
    if rows.len() < 6 {
        return Ok(None);
    }
    let times = rows
        .iter()
        .map(|o| parse(&o.observed_at))
        .collect::<Result<Vec<Instant>>>()?;
    let gaps: Vec<f64> = times
        .windows(2)
        .map(|w| minutes_between(w[0], w[1]))
        .collect();
    let cadence = median_of(&gaps)?;
    let widest = gaps
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, |m, g| if g > m { g } else { m });
    let allowed = if 15.0 > 3.0 * cadence {
        15.0
    } else {
        3.0 * cadence
    };
    if cadence <= 0.0 || widest > allowed {
        return Ok(None);
    }
    let x: Vec<f64> = times.iter().map(|t| minutes_between(last, *t)).collect();
    let span = -x[0];
    if span < 10.0 {
        return Ok(None);
    }
    // Extrapolation cannot exceed half the fitted time span or 30 minutes.
    let horizon = py_min(py_min(horizon_minutes, float(span / 2.0)), Num::Int(30));
    let h = horizon.as_f64();
    let y: Vec<f64> = rows.iter().map(|o| o.value).collect();
    let n = x.len() as f64;
    let mean_x = float_sum(x.iter().copied()) / n;
    let mean_y = float_sum(y.iter().copied()) / n;
    let denominator = float_sum(x.iter().map(|v| square(v - mean_x)));
    if denominator <= 0.0 {
        return Ok(None);
    }
    let slope = float_sum(x.iter().zip(&y).map(|(a, b)| (a - mean_x) * (b - mean_y))) / denominator;
    let intercept = mean_y - slope * mean_x;
    // Choose between local trend and persistence using recent one-step holdouts, never
    // using a reading after the forecast origin.
    let mut trend_error = Vec::new();
    let mut persistence_error = Vec::new();
    for cut in 4..x.len() {
        let (xx, yy) = (&x[..cut], &y[..cut]);
        let mx = float_sum(xx.iter().copied()) / cut as f64;
        let my = float_sum(yy.iter().copied()) / cut as f64;
        let den = float_sum(xx.iter().map(|v| square(v - mx)));
        let ss = divide(
            float_sum(xx.iter().zip(yy).map(|(a, b)| (a - mx) * (b - my))),
            den,
        )?;
        let guess = my + ss * (x[cut] - mx);
        trend_error.push((guess - y[cut]).abs());
        persistence_error.push((y[cut - 1] - y[cut]).abs());
    }
    let use_trend = !trend_error.is_empty()
        && float_sum(trend_error.iter().copied())
            < float_sum(persistence_error.iter().copied()) * 0.95;
    let last_value = y[y.len() - 1];
    let predicted = if use_trend {
        py_max(Num::Int(0), float(intercept + slope * h))
    } else {
        float(last_value)
    };
    let residuals: Vec<f64> = if use_trend {
        x.iter()
            .zip(&y)
            .map(|(a, b)| b - (intercept + slope * a))
            .collect()
    } else {
        y.windows(2).map(|w| w[1] - w[0]).collect()
    };
    let centre = median_of(&residuals)?;
    let spread: Vec<f64> = residuals.iter().map(|v| (v - centre).abs()).collect();
    let mut noise = 1.4826 * median_of(&spread)?;
    for candidate in [last_value.abs() * 0.05, 0.1] {
        if candidate > noise {
            noise = candidate;
        }
    }
    let margin = 2.0 * noise * (1.0 + h / span).sqrt();
    let at = last.add(span_of(horizon, MINUTE_US)?)?.stamp();
    let lower = py_max(Num::Int(0), float(predicted.as_f64() - margin));
    Ok(Some(Projection {
        trend: use_trend,
        horizon,
        at,
        value: predicted,
        lower,
        upper: predicted.as_f64() + margin,
        slope: if use_trend { float(slope) } else { Num::Int(0) },
        fitted: rows.len(),
    }))
}

/// 0.1.0's `balance_projection(...)`: occupancy plus net flow, for an area whose entrances
/// and exits are all counted. The operator must establish that the assumptions hold.
pub fn balance_projection(
    region: &Dict,
    currents: &[&Observation],
    as_of: Instant,
    horizon: Num,
) -> Result<Option<Value>> {
    let Some(Value::Dict(selection)) = region.get("balance") else {
        return Ok(None);
    };
    if selection.is_empty() {
        return Ok(None);
    }
    let unavailable = |reason: &str| {
        Ok(Some(Value::Dict(
            Dict::new()
                .with("status", "unavailable")
                .with("reason", reason),
        )))
    };
    let mut found: Vec<(&str, &Observation)> = Vec::new();
    for (metric, reference) in selection.iter() {
        let matches: Vec<&&Observation> = currents
            .iter()
            .filter(|o| {
                o.metric == *metric
                    && py_eq(
                        &Value::Str(format!("{}/{}", o.source_id, o.stream_id)),
                        reference,
                    )
            })
            .collect();
        if matches.len() != 1 {
            return unavailable("Missing, stale or ambiguous balance input");
        }
        found.push((metric.as_str(), matches[0]));
    }
    let get = |metric: &str| {
        found
            .iter()
            .find(|(m, _)| *m == metric)
            .map(|(_, o)| *o)
            .ok_or_else(|| Fail::key(metric))
    };
    let times = found
        .iter()
        .map(|(_, o)| parse(&o.observed_at))
        .collect::<Result<Vec<Instant>>>()?;
    let (Some(newest), Some(oldest)) = (times.iter().max(), times.iter().min()) else {
        return Ok(None);
    };
    if newest.since(*oldest).total_seconds() > 120.0 {
        return unavailable("Balance inputs are more than two minutes apart");
    }
    let occupancy_reading = get("occupancy")?;
    let occupancy = occupancy_reading.value;
    let origin = parse(&occupancy_reading.observed_at)?;
    let ahead = span_of(horizon, MINUTE_US)?;
    if origin.add(ahead)? <= as_of {
        return unavailable("Occupancy reading is too old for this projection horizon");
    }
    let net = get("inflow")?.value - get("outflow")?.value;
    let estimate = py_max(Num::Int(0), float(occupancy + net * horizon.as_f64()));
    let matches_stream = py_eq(
        region.get("occupancy_stream").unwrap_or(&Value::Null),
        selection.get("occupancy").unwrap_or(&Value::Null),
    );
    let cap = if matches_stream {
        region
            .get("capacity_people")
            .filter(|v| !v.is_null())
            .map(Num::from_value)
    } else {
        None
    };
    let crossing = match cap {
        Some(cap) if net > 0.0 && lt(float(occupancy), cap) => {
            Some(float((cap.as_f64() - occupancy) / net))
        }
        Some(cap) if ge(float(occupancy), cap) => Some(Num::Int(0)),
        _ => None,
    };
    let reachable = crossing.filter(|c| le(*c, horizon));
    let input_ids = found
        .iter()
        .map(|(_, o)| o.key().map(Value::Str))
        .collect::<Result<Vec<Value>>>()?;
    Ok(Some(Value::Dict(
        Dict::new()
            .with("status", "available")
            .with("occupancy", occupancy)
            .with("net_people_per_minute", net)
            .with("horizon_minutes", horizon.to_value())
            .with("projected_occupancy", estimate.to_value())
            .with(
                "minutes_to_configured_capacity",
                reachable.map_or(Value::Null, Num::to_value),
            )
            .with(
                "assumptions",
                "same bounded area, complete entrances/exits, constant rates; operator must establish these",
            )
            .with("input_ids", Value::List(input_ids))
            .with("origin_at", origin.stamp())
            .with("at", origin.add(ahead)?.stamp()),
    )))
}

// ---------------------------------------------------------------------------------------
// Regions
// ---------------------------------------------------------------------------------------

/// Whether a configured source exists and is enabled.
fn usable_source(config: &Config, id: &str) -> bool {
    config.enabled(id)
}

/// 0.1.0's `analyse_region(...)`: the evidence for one area at `as_of`.
pub fn analyse_region(
    config: &Config,
    store: &Store,
    region: &Dict,
    as_of: Instant,
) -> Result<Value> {
    let region_id = region.get("id").and_then(Value::as_str).unwrap_or("");
    let observations: Vec<Observation> = store
        .observations(region_id, as_of, config.setting("history_days"))?
        .into_iter()
        // Removed or disabled sources never continue silently contributing to alerts.
        .filter(|o| usable_source(config, &o.source_id))
        .collect();
    let mut streams: Vec<(StreamKey, Vec<&Observation>)> = Vec::new();
    for o in &observations {
        let key = o.stream_key();
        match streams.iter_mut().find(|(k, _)| *k == key) {
            Some((_, series)) => series.push(o),
            None => streams.push((key, vec![o])),
        }
    }
    streams.sort_by(|a, b| a.0.cmp(&b.0));
    let minimum_quality = config.setting("minimum_quality");
    let z_threshold = config.setting("z_threshold");
    let horizon = config.setting("horizon_minutes");
    let minimum_baseline = truncate(config.setting("minimum_baseline"));
    let mut signals: Vec<Value> = Vec::new();
    let mut context: Vec<Value> = Vec::new();
    let mut stale: Vec<Value> = Vec::new();
    let mut currents: Vec<&Observation> = Vec::new();
    let mut adequate = false;
    let mut independent: Vec<String> = Vec::new();
    let (mut watch, mut elevated) = (false, false);
    let mut scores: Vec<Num> = Vec::new();
    // What the reasons need from each signal.
    struct Seen {
        name: String,
        metric: String,
        z: Option<f64>,
        sustained: bool,
        capacity_ratio: Option<f64>,
    }
    let mut seen: Vec<Seen> = Vec::new();
    for (_, series) in &streams {
        let current = series[series.len() - 1];
        let source = config
            .source(&current.source_id)
            .ok_or_else(|| Fail::key(&current.source_id))?;
        let ttl = span_of(number(source, "ttl_seconds"), 1_000_000)?;
        let until = parse(&current.valid_until)?;
        let held = parse(&current.observed_at)?.add(ttl)?;
        let expiry = if held < until { held } else { until };
        if expiry < as_of {
            stale.push(Value::Dict(
                Dict::new()
                    .with("source_id", current.source_id.as_str())
                    .with("stream_id", current.stream_id.as_str())
                    .with("metric", current.metric.as_str())
                    .with("last_observed", current.observed_at.as_str()),
            ));
            continue;
        }
        let quality = py_min(float(current.quality), number(source, "quality"));
        if lt(quality, minimum_quality) {
            continue;
        }
        currents.push(current);
        if !is_direct(&current.metric) {
            if current.metric == "scheduled_event" {
                let none = Value::Null;
                let starts =
                    crate::pytime::parse_time(current.details.get("starts_at").unwrap_or(&none))?;
                let ends =
                    crate::pytime::parse_time(current.details.get("ends_at").unwrap_or(&none))?;
                if starts > as_of.add(Span::hours(3))? || ends <= as_of {
                    continue;
                }
            }
            context.push(current.to_value());
            continue;
        }
        let usable: Vec<Observation> = series
            .iter()
            .filter(|x| {
                ge(float(x.quality), minimum_quality)
                    && (x.latitude, x.longitude) == (current.latitude, current.longitude)
            })
            .map(|x| (*x).clone())
            .collect();
        let earlier = &usable[..usable.len().saturating_sub(1)];
        let reference = baseline(earlier, current, minimum_baseline)?;
        let direction = if current.metric == "speed" { -1.0 } else { 1.0 };
        let z = reference
            .as_ref()
            .map(|r| direction * (current.value - r.median) / r.scale);
        if let Some(z) = z {
            adequate = true;
            if current.metric != "outflow" {
                scores.push(py_max(
                    Num::Int(0),
                    py_min(Num::Int(100), float(z / 6.0 * 100.0)),
                ));
            }
        }
        let recent = &usable[usable.len().saturating_sub(2)..];
        let mut sustained = false;
        if let Some(r) = &reference {
            if recent.len() == 2 {
                let mut all = true;
                for x in recent {
                    let fresh = parse(&x.valid_until)? >= as_of.sub(Span::minutes(5))?;
                    if !(fresh
                        && ge(
                            float(direction * (x.value - r.median) / r.scale),
                            z_threshold,
                        ))
                    {
                        all = false;
                        break;
                    }
                }
                sustained = all
                    && parse(&recent[1].observed_at)?
                        .since(parse(&recent[0].observed_at)?)
                        .total_seconds()
                        <= 900.0;
            }
        }
        let mut anomaly = z.is_some_and(|z| ge(float(z), z_threshold));
        // Falling exit flow is not pressure by itself. Both directions remain visible.
        if current.metric == "outflow" {
            anomaly = false;
            sustained = false;
        }
        watch |= anomaly;
        elevated |= sustained;
        if sustained {
            let group = source
                .get("independence_group")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            if !independent.contains(&group) {
                independent.push(group);
            }
        }
        let mut projection = project(&usable, horizon)?;
        if let Some(p) = &projection {
            if parse(&p.at)? <= as_of {
                projection = None;
            }
        }
        let mut capacity_ratio = None;
        let stream_ref = Value::Str(format!("{}/{}", current.source_id, current.stream_id));
        let configured = current.metric == "occupancy"
            && py_eq(
                region.get("occupancy_stream").unwrap_or(&Value::Null),
                &stream_ref,
            )
            && region.contains_key("capacity_people");
        if configured {
            let capacity = Num::from_value(region.get("capacity_people").unwrap_or(&Value::Null));
            let ratio = current.value / capacity.as_f64();
            capacity_ratio = Some(ratio);
            adequate = true;
            watch |= ratio >= 0.85;
            elevated |= ratio >= 1.0;
            if let Some(p) = &projection {
                if ge(float(p.upper), capacity) {
                    watch = true;
                }
            }
        }
        let history: Vec<Value> = usable[usable.len().saturating_sub(80)..]
            .iter()
            .map(|x| {
                Value::Dict(
                    Dict::new()
                        .with("at", x.observed_at.as_str())
                        .with("value", x.value),
                )
            })
            .collect();
        seen.push(Seen {
            name: if current.label.is_empty() {
                current.stream_id.clone()
            } else {
                current.label.clone()
            },
            metric: current.metric.clone(),
            z,
            sustained,
            capacity_ratio,
        });
        signals.push(Value::Dict(
            Dict::new()
                .with("observation", current.to_value())
                .with("observation_id", current.key()?)
                .with(
                    "baseline",
                    reference.as_ref().map_or(Value::Null, Baseline::to_value),
                )
                .with("z", z)
                .with("sustained", sustained)
                .with("capacity_ratio", capacity_ratio)
                .with(
                    "projection",
                    projection
                        .as_ref()
                        .map_or(Value::Null, Projection::to_value),
                )
                .with("history", Value::List(history)),
        ));
    }
    let mut level = if independent.len() >= 2 {
        "corroborated"
    } else if elevated {
        "elevated"
    } else if watch {
        "watch"
    } else if adequate {
        "baseline"
    } else {
        "insufficient_data"
    };
    let balance = balance_projection(region, &currents, as_of, horizon)?;
    let crossing_known = balance.as_ref().is_some_and(|b| {
        b.as_dict()
            .and_then(|d| d.get("minutes_to_configured_capacity"))
            .is_some_and(|v| !v.is_null())
    });
    if crossing_known && (level == "baseline" || level == "insufficient_data") {
        level = "watch";
    }
    let mut reasons: Vec<String> = Vec::new();
    if signals.is_empty() {
        reasons.push(
            "No fresh direct movement measurements; context cannot establish crowd conditions."
                .to_owned(),
        );
    } else if !adequate {
        reasons.push("Fresh measurements exist but there is not enough comparable baseline history or a configured occupancy threshold.".to_owned());
    }
    for s in &seen {
        if s.sustained {
            reasons.push(format!(
                "{}: sustained unusual {} ({} robust scale units).",
                s.name,
                s.metric,
                format_fixed(s.z.unwrap_or(0.0), 1)
            ));
        } else if s.z.is_some_and(|z| ge(float(z), z_threshold)) && s.metric != "outflow" {
            reasons.push(format!(
                "{}: one recent elevated reading; persistence is not established.",
                s.name
            ));
        }
        if let Some(ratio) = s.capacity_ratio {
            if ratio >= 0.85 {
                reasons.push(format!(
                    "Configured occupancy reference is {}% used.",
                    format_fixed(ratio * 100.0, 0)
                ));
            }
        }
    }
    let context_metric = |x: &Value, metric: &str| {
        x.as_dict()
            .and_then(|d| d.get("metric"))
            .and_then(Value::as_str)
            == Some(metric)
    };
    let disrupted = context.iter().any(|x| {
        context_metric(x, "transit_disruption")
            && x.as_dict()
                .and_then(|d| d.get("value"))
                .is_some_and(|v| v.py_float().is_ok_and(|f| f > 0.0))
    });
    if disrupted {
        reasons.push(
            "Transport disruption is present as context; local headcount impact is unknown."
                .to_owned(),
        );
    }
    if context.iter().any(|x| context_metric(x, "scheduled_event")) {
        reasons.push(
            "Nearby scheduled events may affect demand; their attendance is unknown.".to_owned(),
        );
    }
    let anomaly_index = scores.iter().copied().reduce(py_max);
    independent.sort();
    Ok(Value::Dict(
        Dict::new()
            .with("region", Value::Dict(region.clone()))
            .with("level", level)
            .with(
                "anomaly_index",
                anomaly_index.map_or(Value::Null, Num::to_value),
            )
            .with(
                "index_meaning",
                "strongest upward count/occupancy or downward speed anomaly; not a probability",
            )
            .with(
                "reasons",
                Value::List(reasons.into_iter().map(Value::Str).collect()),
            )
            .with("direct_streams", signals.len())
            .with("signals", Value::List(signals))
            .with("context", Value::List(context))
            .with("stale_streams", Value::List(stale))
            .with(
                "corroborating_groups",
                Value::List(independent.into_iter().map(Value::Str).collect()),
            )
            .with("balance_projection", balance.unwrap_or(Value::Null)),
    ))
}

/// The report's `version`, which 0.1.0 wrote as its own version number.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 0.1.0's `analyse(config, store, as_of)`: every region, source health and the
/// interpretation boundary.
pub fn analyse(config: &Config, store: &Store, as_of: Instant) -> Result<Value> {
    let mut regions = Vec::with_capacity(config.regions.len());
    for (_, region) in &config.regions {
        regions.push(analyse_region(config, store, region, as_of)?);
    }
    let synthetic = regions.iter().any(|r| {
        let mut observations: Vec<&Value> = Vec::new();
        if let Some(Value::List(signals)) = r.as_dict().and_then(|d| d.get("signals")) {
            observations.extend(
                signals
                    .iter()
                    .filter_map(|s| s.as_dict().and_then(|d| d.get("observation"))),
            );
        }
        if let Some(Value::List(context)) = r.as_dict().and_then(|d| d.get("context")) {
            observations.extend(context.iter());
        }
        observations.iter().any(|o| {
            o.as_dict()
                .and_then(|d| d.get("details"))
                .and_then(Value::as_dict)
                .and_then(|d| d.get("synthetic"))
                .is_some_and(Value::truthy)
        })
    });
    let sources: Vec<Value> = config
        .sources
        .iter()
        .map(|(_, s)| {
            let field = |name: &str| s.get(name).cloned().unwrap_or(Value::Null);
            Value::Dict(
                Dict::new()
                    .with("id", field("id"))
                    .with("kind", field("kind"))
                    .with("attribution", field("attribution"))
                    .with("enabled", field("enabled"))
                    .with("independence_group", field("independence_group")),
            )
        })
        .collect();
    Ok(Value::Dict(
        Dict::new()
            .with("schema_version", 1_i64)
            .with("module", "skeletrace-crowdwatch")
            .with("version", VERSION)
            .with("synthetic", synthetic)
            .with("as_of", as_of.stamp())
            .with("regions", Value::List(regions))
            .with("source_health", Value::List(store.health()?))
            .with("sources", Value::List(sources))
            .with(
                "limitations",
                "Descriptive movement signals and conditional projections. No calibrated prediction of crowd injury, psychological state or social collapse.",
            )
            .with(
                "health_time_basis",
                "Source health reflects the latest collection; observations are filtered as-of.",
            ),
    ))
}

/// 0.1.0's `backtest(...)`: rolling-origin short-term forecast error against observed
/// later counts, using only the versions available at each origin. Corrected later values
/// are evaluation targets, never inputs. This evaluates telemetry forecasts, not hazard
/// warnings.
pub fn backtest(config: &Config, store: &Store, as_of: Instant, horizon: Num) -> Result<Value> {
    let history_days = config.setting("history_days");
    let minimum_quality = config.setting("minimum_quality");
    let ahead = span_of(horizon, MINUTE_US)?;
    let mut results: Vec<Value> = Vec::new();
    for (_, region) in &config.regions {
        let region_id = region.get("id").and_then(Value::as_str).unwrap_or("");
        let all_rows = store.observations(region_id, as_of, history_days)?;
        let mut groups: Vec<(StreamKey, Vec<Observation>)> = Vec::new();
        for o in all_rows {
            if is_direct(&o.metric) && usable_source(config, &o.source_id) {
                let key = o.stream_key();
                match groups.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, rows)) => rows.push(o),
                    None => groups.push((key, vec![o])),
                }
            }
        }
        for (key, rows) in &groups {
            let mut errors: Vec<f64> = Vec::new();
            let mut naive: Vec<f64> = Vec::new();
            let mut signed: Vec<f64> = Vec::new();
            let mut last_origin: Option<Instant> = None;
            for target in rows {
                let origin = parse(&target.observed_at)?.sub(ahead)?;
                if lt(float(target.quality), minimum_quality) {
                    continue;
                }
                if let Some(previous) = last_origin {
                    let gap = float(origin.since(previous).total_seconds());
                    let spacing = match horizon {
                        Num::Int(m) => m
                            .checked_mul(60)
                            .map_or(Num::Float(m as f64 * 60.0), Num::Int),
                        Num::Float(m) => Num::Float(m * 60.0),
                    };
                    if lt(gap, spacing) {
                        continue;
                    }
                }
                let known: Vec<Observation> = store
                    .observations(region_id, origin, history_days)?
                    .into_iter()
                    .filter(|o| o.stream_key() == *key && ge(float(o.quality), minimum_quality))
                    .collect();
                let Some(latest) = known.last() else {
                    continue;
                };
                if parse(&latest.observed_at)?
                    .since(origin)
                    .total_seconds()
                    .abs()
                    > 1.0
                {
                    continue;
                }
                let Some(forecast) = project(&known, horizon)? else {
                    continue;
                };
                if abs(sub(forecast.horizon, horizon)).as_f64() > 1e-6 {
                    continue;
                }
                last_origin = Some(origin);
                let error = forecast.value.as_f64() - target.value;
                errors.push(error.abs());
                signed.push(error);
                naive.push((latest.value - target.value).abs());
            }
            if !errors.is_empty() {
                let count = errors.len() as f64;
                let mae = float_sum(errors.iter().copied()) / count;
                let persistence = float_sum(naive.iter().copied()) / naive.len() as f64;
                let bias = float_sum(signed.iter().copied()) / signed.len() as f64;
                let skill = if persistence != 0.0 {
                    Value::Float(1.0 - mae / persistence)
                } else {
                    Value::Null
                };
                results.push(Value::Dict(
                    Dict::new()
                        .with("source_id", key.0.as_str())
                        .with("stream_id", key.1.as_str())
                        .with("region_id", key.2.as_str())
                        .with("metric", key.3.as_str())
                        .with("period_seconds", key.4)
                        .with("evaluated_origins", errors.len())
                        .with("mae", mae)
                        .with("bias", bias)
                        .with("persistence_mae", persistence)
                        .with("skill_vs_persistence", skill),
                ));
            }
        }
    }
    let empty = results.is_empty();
    Ok(Value::Dict(
        Dict::new()
            .with("horizon_minutes", horizon.to_value())
            .with("series", Value::List(results))
            .with(
                "meaning",
                "Telemetry prediction only. Positive skill beats persistence on these evaluated origins; no hazard-validation claim.",
            )
            .with(
                "empty_reason",
                if empty {
                    Value::from("No eligible historical as-of origins with a matching horizon target. Imported backfill is unavailable before retrieval.")
                } else {
                    Value::Null
                },
            ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_numbers_compare_like_python() {
        assert_eq!(py_min(Num::Int(15), float(30.0)), Num::Int(15));
        assert_eq!(py_min(Num::Int(30), float(30.0)), Num::Int(30));
        assert_eq!(py_min(float(30.0), Num::Int(30)), float(30.0));
        assert_eq!(py_max(Num::Int(0), float(-0.0)), Num::Int(0));
        assert_eq!(py_max(Num::Int(0), float(0.5)), float(0.5));
        assert_eq!(
            num_cmp(Num::Int(i64::MAX), float(9.223_372_036_854_776e18)),
            Some(Ordering::Less)
        );
        assert_eq!(num_cmp(Num::Int(3), float(f64::NAN)), None);
        assert!(lt(float(2.5), Num::Int(3)));
        assert!(gt(float(3.5), Num::Int(3)));
    }
}
