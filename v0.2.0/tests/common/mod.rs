// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Shared test fixtures: 0.1.0's `Fixture` class and `observation()` helper.

#![allow(dead_code)]

use std::path::PathBuf;

use crowdwatch::config::Config;
use crowdwatch::error::Result;
use crowdwatch::json::{parse_str, Dict, Value};
use crowdwatch::model::{atomic_write, json_text, unit_of, Observation};
use crowdwatch::pytime::{parse_time_str, Instant, Span};
use crowdwatch::store::Store;

/// 2026-10-01T12:00:00Z, the tests' "now".
pub fn at() -> Instant {
    parse_time_str("2026-10-01T12:00:00Z").unwrap()
}

pub fn minutes(m: i64) -> Span {
    Span::minutes(m)
}

/// A fresh empty folder for one test.
pub fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "crowdwatch-test-{name}-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 0.1.0's test `observation(value, at, source, metric, stream, **kw)`.
pub fn observation_with(
    value: Value,
    when: Instant,
    source: &str,
    metric: Value,
    stream: &str,
    extra: &[(&str, Value)],
) -> Result<Observation> {
    let unit = match &metric {
        Value::Str(m) => unit_of(m).unwrap_or(""),
        _ => "",
    };
    let footfall = matches!(&metric, Value::Str(m) if m == "footfall");
    let mut raw = Dict::new()
        .with("source_id", source)
        .with("stream_id", stream)
        .with("region_id", "area")
        .with("metric", metric.clone())
        .with("value", value)
        .with("unit", unit)
        .with("observed_at", when.stamp())
        .with("retrieved_at", when.stamp())
        .with("valid_until", when.add(minutes(10)).unwrap().stamp())
        .with("period_seconds", if footfall { 300_i64 } else { 0 });
    for (key, v) in extra {
        raw.insert(*key, v.clone());
    }
    Observation::from_value(&Value::Dict(raw))
}

/// `observation(value, at)` with the defaults.
pub fn observation(value: f64, when: Instant) -> Observation {
    observation_with(
        Value::Float(value),
        when,
        "counter",
        Value::from("footfall"),
        "sensor",
        &[],
    )
    .unwrap()
}

/// `observation(value, at, **kw)`.
pub fn observation_kw(value: f64, when: Instant, extra: &[(&str, Value)]) -> Observation {
    observation_with(
        Value::Float(value),
        when,
        "counter",
        Value::from("footfall"),
        "sensor",
        extra,
    )
    .unwrap()
}

/// 0.1.0's `Fixture`: a temporary folder, a one-region configuration and its database.
pub struct Fixture {
    pub root: PathBuf,
    pub config_raw: Value,
    pub cfg: Config,
    pub store: Store,
}

impl Fixture {
    pub fn new(name: &str) -> Fixture {
        let root = temp_dir(name);
        let config_raw = parse_str(
            r#"{"schema_version": 1, "regions": [{"id": "area", "name": "Test area",
            "latitude": 51.5, "longitude": -0.12, "radius_km": 2}], "sources": [{
            "id": "counter", "kind": "canonical_json", "region_id": "area", "quality": 0.9,
            "ttl_seconds": 600, "attribution": "Synthetic fixture", "options": {"path": "data.jsonl"}}]}"#,
        )
        .unwrap();
        let cfg = write_config(&root, &config_raw).unwrap();
        let store = Store::open(&cfg.database).unwrap();
        Fixture {
            root,
            config_raw,
            cfg,
            store,
        }
    }

    /// Write `config_raw` and load it.
    pub fn config(&self) -> Result<Config> {
        write_config(&self.root, &self.config_raw)
    }

    /// The first configured source, for editing `config_raw`.
    pub fn first_source(&mut self) -> &mut Dict {
        let Value::Dict(top) = &mut self.config_raw else {
            unreachable!()
        };
        let Some(Value::List(sources)) = top.get_mut("sources") else {
            unreachable!()
        };
        let Value::Dict(source) = &mut sources[0] else {
            unreachable!()
        };
        source
    }

    /// `history(source, value, **kw)`: nine readings a day earlier, five minutes apart.
    pub fn history(&self, source: &str, value: f64, extra: &[(&str, Value)]) -> Vec<Observation> {
        (0..9)
            .map(|i| {
                let when = at()
                    .sub(Span::days(1))
                    .unwrap()
                    .sub(minutes(20 - i * 5))
                    .unwrap();
                observation_with(
                    Value::Float(value + (i % 3) as f64),
                    when,
                    source,
                    Value::from("footfall"),
                    "sensor",
                    extra,
                )
                .unwrap()
            })
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub fn write_config(root: &std::path::Path, raw: &Value) -> Result<Config> {
    let path = root.join("config.json");
    atomic_write(&path, &json_text(raw, false)?)?;
    Config::load(&path)
}
