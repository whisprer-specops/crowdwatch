// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! 0.1.0's `tests/test_contracts.py`, test for test: the model, the store, the analysis,
//! the exports and the URL rules.

mod common;

use common::{at, minutes, observation, observation_kw, observation_with, Fixture};
use crowdwatch::analysis::{analyse, backtest, balance_projection, baseline, project};
use crowdwatch::demo::create_demo;
use crowdwatch::error::Fail;
use crowdwatch::export::{export_skeletrace, geojson};
use crowdwatch::http::validate_url;
use crowdwatch::json::{parse_str, Dict, Value};
use crowdwatch::model::{json_text, load_json, Observation};
use crowdwatch::pytime::{parse_time_str, stamp_str, Num, Span};
use crowdwatch::report::render;
use crowdwatch::sources::{canonical_records, geohash};
use crowdwatch::store::Store;

fn is_input_error<T: std::fmt::Debug>(result: Result<T, Fail>) -> bool {
    matches!(result, Err(Fail::Input(_)))
}

fn region(report: &Value) -> &Value {
    &report.index("regions").unwrap().as_list().unwrap()[0]
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.index(key).unwrap().as_str().unwrap()
}

// ---------------------------------------------------------------------------------------
// ModelTests
// ---------------------------------------------------------------------------------------

#[test]
fn rejects_invalid_values_and_units() {
    for value in [
        Value::Float(f64::NAN),
        Value::Float(f64::INFINITY),
        Value::Int(-1),
        Value::Bool(true),
        Value::from("4"),
    ] {
        let result = observation_with(
            value.clone(),
            at(),
            "counter",
            Value::from("footfall"),
            "sensor",
            &[],
        );
        assert!(is_input_error(result), "{value:?}");
    }
    let cases: Vec<(&str, Value)> = vec![
        ("unit", Value::from("people")),
        ("period_seconds", Value::Int(0)),
        ("quality", Value::Float(1.1)),
        ("observed_at", Value::from("2026-10-01T12:00:00")),
        ("latitude", Value::Int(51)),
        ("valid_until", Value::from(at().stamp())),
        (
            "provenance_url",
            Value::from("https://example.org/?apikey=secret"),
        ),
        ("provenance_url", Value::Null),
    ];
    for (key, value) in cases {
        let result = observation_with(
            Value::Float(20.0),
            at(),
            "counter",
            Value::from("footfall"),
            "sensor",
            &[(key, value.clone())],
        );
        assert!(is_input_error(result), "{key}={value:?}");
    }
    let listed = observation_with(
        Value::Float(20.0),
        at(),
        "counter",
        Value::List(vec![]),
        "sensor",
        &[],
    );
    assert!(is_input_error(listed));
}

#[test]
fn utc_normalization() {
    assert_eq!(
        stamp_str("2026-10-01T13:00:00+01:00").unwrap(),
        at().stamp()
    );
}

#[test]
fn rejects_future_observation() {
    let earlier = at().sub(Span::seconds(1)).unwrap().stamp();
    let result = observation_with(
        Value::Float(20.0),
        at(),
        "counter",
        Value::from("footfall"),
        "sensor",
        &[("retrieved_at", Value::from(earlier))],
    );
    assert!(is_input_error(result));
}

#[test]
fn explicit_zero_is_retained() {
    let mut f = Fixture::new("zero");
    f.store.ingest(&[observation(0.0, at())]).unwrap();
    let rows = f.store.observations("area", at(), Num::Int(28)).unwrap();
    assert_eq!(rows[0].value, 0.0);
}

#[test]
fn json_rejects_duplicate_keys_and_nan() {
    for body in [r#"{"a":1,"a":2}"#, r#"{"a":NaN}"#, "[Infinity]"] {
        assert!(is_input_error(load_json(body)), "{body}");
    }
}

#[test]
fn canonical_jsonl_and_csv() {
    let o = observation(20.0, at());
    let raw = json_text(&o.to_value(), false).unwrap();
    let body = format!("{raw}\n{raw}\n");
    assert_eq!(
        canonical_records(body.as_bytes(), "canonical_json").unwrap(),
        vec![o.clone(), o.clone()]
    );
    let header = "source_id,stream_id,region_id,metric,value,unit,observed_at,retrieved_at,valid_until,period_seconds\n";
    let line = format!(
        "counter,sensor,area,footfall,20,people/interval,{},{},{},300\n",
        at().stamp(),
        at().stamp(),
        at().add(minutes(10)).unwrap().stamp()
    );
    let rows = canonical_records(format!("{header}{line}").as_bytes(), "canonical_csv").unwrap();
    assert_eq!(rows[0], o);
}

#[test]
fn equivalent_csv_json_numbers_do_not_create_revisions() {
    let mut f = Fixture::new("revisions");
    let o = observation_with(
        Value::Int(20),
        at(),
        "counter",
        Value::from("footfall"),
        "sensor",
        &[
            ("quality", Value::Int(1)),
            ("latitude", Value::Int(0)),
            ("longitude", Value::Int(0)),
        ],
    )
    .unwrap();
    let mut raw = o.to_dict();
    for (key, value) in [
        ("value", 20),
        ("quality", 1),
        ("latitude", 0),
        ("longitude", 0),
    ] {
        raw.insert(key, Value::Int(value));
    }
    let a = canonical_records(
        json_text(&Value::Dict(raw.clone()), false)
            .unwrap()
            .as_bytes(),
        "canonical_json",
    )
    .unwrap();
    // csv.DictWriter over the same fields, with JSON details and a negative zero.
    let names: Vec<&str> = raw.keys().map(String::as_str).collect();
    let mut csv = names.join(",") + "\r\n";
    let cells: Vec<String> = raw
        .iter()
        .map(|(key, value)| match key.as_str() {
            "details" => format!(
                "\"{}\"",
                json_text(value, false).unwrap().replace('"', "\"\"")
            ),
            "longitude" => "-0.0".to_owned(),
            _ => match value {
                Value::Str(s) => s.clone(),
                other => other.py_str(),
            },
        })
        .collect();
    csv.push_str(&cells.join(","));
    csv.push_str("\r\n");
    let b = canonical_records(csv.as_bytes(), "canonical_csv").unwrap();
    assert_eq!(f.store.ingest(&a).unwrap(), 1);
    assert_eq!(f.store.ingest(&b).unwrap(), 0);
    assert_eq!(a[0].digest().unwrap(), b[0].digest().unwrap());
}

#[test]
fn config_rejects_bad_shapes() {
    let mut f = Fixture::new("shapes");
    if let Value::Dict(top) = &mut f.config_raw {
        top.insert("regions", Value::Int(42));
    }
    assert!(is_input_error(f.config()));
}

#[test]
fn capacity_requires_exact_occupancy_stream() {
    let mut f = Fixture::new("capacity");
    if let Value::Dict(top) = &mut f.config_raw {
        if let Some(Value::List(regions)) = top.get_mut("regions") {
            if let Value::Dict(r) = &mut regions[0] {
                r.insert("capacity_people", Value::Int(500));
            }
        }
    }
    assert!(is_input_error(f.config()));
}

// ---------------------------------------------------------------------------------------
// StoreTests
// ---------------------------------------------------------------------------------------

fn retrieved(o: &Observation, offset: Span) -> Observation {
    let mut copy = o.clone();
    copy.retrieved_at = at().add(offset).unwrap().stamp();
    copy
}

fn valued(o: &Observation, value: f64, offset: Span) -> Observation {
    let mut copy = retrieved(o, offset);
    copy.value = value;
    copy
}

#[test]
fn ingestion_idempotent() {
    let mut f = Fixture::new("idempotent");
    let o = observation(20.0, at());
    assert_eq!(f.store.ingest(std::slice::from_ref(&o)).unwrap(), 1);
    assert_eq!(f.store.ingest(&[retrieved(&o, minutes(1))]).unwrap(), 0);
    let counts = f.store.counts().unwrap();
    assert!(matches!(counts.get("versions"), Some(Value::Int(1))));
    assert!(matches!(counts.get("observations"), Some(Value::Int(1))));
}

#[test]
fn corrections_do_not_leak_backwards() {
    let mut f = Fixture::new("corrections");
    let old = observation(10.0, at());
    let corrected = valued(&old, 80.0, minutes(5));
    f.store.ingest(&[old, corrected]).unwrap();
    assert_eq!(
        f.store.observations("area", at(), Num::Int(28)).unwrap()[0].value,
        10.0
    );
    assert_eq!(
        f.store
            .observations("area", at().add(minutes(6)).unwrap(), Num::Int(28))
            .unwrap()[0]
            .value,
        80.0
    );
    assert!(matches!(
        f.store.counts().unwrap().get("versions"),
        Some(Value::Int(2))
    ));
}

#[test]
fn correction_can_revert_to_an_earlier_value() {
    let mut f = Fixture::new("revert");
    let first = observation(10.0, at());
    let corrected = valued(&first, 80.0, minutes(5));
    let reverted = retrieved(&first, minutes(10));
    assert_eq!(
        f.store
            .ingest(&[first, corrected, reverted.clone()])
            .unwrap(),
        3
    );
    for (minute, value) in [(0, 10.0), (6, 80.0), (11, 10.0)] {
        let rows = f
            .store
            .observations("area", at().add(minutes(minute)).unwrap(), Num::Int(28))
            .unwrap();
        assert_eq!(rows[0].value, value);
    }
    assert_eq!(
        f.store
            .ingest(&[retrieved(&reverted, minutes(12))])
            .unwrap(),
        0
    );
}

#[test]
fn old_database_migrates_without_losing_history() {
    let f = Fixture::new("migrate");
    let path = f.root.join("legacy.sqlite");
    let o = observation(20.0, at());
    {
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE observations (
                observation_key TEXT NOT NULL, digest TEXT NOT NULL,
                region_id TEXT NOT NULL, source_id TEXT NOT NULL,
                observed_at TEXT NOT NULL, retrieved_at TEXT NOT NULL,
                payload TEXT NOT NULL, PRIMARY KEY(observation_key,digest));
                CREATE INDEX observation_window ON observations(region_id,observed_at,retrieved_at);
                PRAGMA user_version=1;",
        )
        .unwrap();
        db.execute(
            "INSERT INTO observations VALUES (?,?,?,?,?,?,?)",
            rusqlite::params![
                o.key().unwrap(),
                o.digest().unwrap(),
                o.region_id,
                o.source_id,
                o.observed_at,
                o.retrieved_at,
                json_text(&o.to_value(), false).unwrap()
            ],
        )
        .unwrap();
    }
    let mut migrated = Store::open(&path).unwrap();
    assert_eq!(
        migrated.observations("area", at(), Num::Int(28)).unwrap(),
        vec![o.clone()]
    );
    let version: i64 = migrated
        .db
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 2);
    assert_eq!(migrated.ingest(&[o]).unwrap(), 0);
}

#[test]
fn failed_batch_is_atomic() {
    let mut f = Fixture::new("atomic");
    let mut broken = observation(20.0, at());
    broken.value = f64::NAN;
    assert!(is_input_error(
        f.store.ingest(&[observation(20.0, at()), broken])
    ));
    assert!(matches!(
        f.store.counts().unwrap().get("versions"),
        Some(Value::Int(0))
    ));
}

#[test]
fn same_metric_with_different_interval_is_distinct() {
    let mut f = Fixture::new("interval");
    let a = observation(20.0, at());
    let mut b = a.clone();
    b.period_seconds = 3600;
    f.store.ingest(&[a, b]).unwrap();
    assert!(matches!(
        f.store.counts().unwrap().get("observations"),
        Some(Value::Int(2))
    ));
}

#[test]
fn repeated_and_concurrent_claims() {
    let mut f = Fixture::new("claims");
    assert!(f.store.claim("counter", at(), false).unwrap());
    {
        let mut other = Store::open(&f.cfg.database).unwrap();
        assert!(!other.claim("counter", at(), true).unwrap());
    }
    f.store
        .finish("counter", at(), Num::Int(300), 3, "", &[], Num::Int(0))
        .unwrap();
    assert!(!f.store.claim("counter", at(), false).unwrap());
    assert!(f
        .store
        .claim("counter", at().add(minutes(6)).unwrap(), false)
        .unwrap());
}

#[test]
fn retry_after_is_respected() {
    let mut f = Fixture::new("retry");
    f.store.claim("counter", at(), false).unwrap();
    f.store
        .finish(
            "counter",
            at(),
            Num::Int(300),
            0,
            "HTTP 429",
            &[],
            Num::Int(3600),
        )
        .unwrap();
    assert!(!f
        .store
        .claim("counter", at().add(minutes(59)).unwrap(), false)
        .unwrap());
}

#[test]
fn source_failure_preserves_existing_data() {
    let mut f = Fixture::new("failure");
    f.store.ingest(&[observation(45.0, at())]).unwrap();
    f.store
        .finish(
            "counter",
            at(),
            Num::Int(300),
            0,
            "outage",
            &[],
            Num::Int(0),
        )
        .unwrap();
    assert_eq!(
        f.store.observations("area", at(), Num::Int(28)).unwrap()[0].value,
        45.0
    );
    let health = f.store.health().unwrap();
    assert_eq!(text(&health[0], "status"), "error");
}

// ---------------------------------------------------------------------------------------
// AnalysisTests
// ---------------------------------------------------------------------------------------

#[test]
fn context_only_remains_unknown() {
    let mut f = Fixture::new("context");
    let o = observation_with(
        Value::Int(1),
        at(),
        "counter",
        Value::from("transit_disruption"),
        "sensor",
        &[],
    )
    .unwrap();
    f.store.ingest(&[o]).unwrap();
    let report = analyse(&f.cfg, &f.store, at()).unwrap();
    let r = region(&report);
    assert_eq!(text(r, "level"), "insufficient_data");
    assert!(r.index("anomaly_index").unwrap().is_null());
}

#[test]
fn missing_readings_are_not_zero() {
    let f = Fixture::new("missing");
    let report = analyse(&f.cfg, &f.store, at()).unwrap();
    let r = region(&report);
    assert_eq!(text(r, "level"), "insufficient_data");
    assert!(r.index("signals").unwrap().as_list().unwrap().is_empty());
}

#[test]
fn stale_data_is_excluded() {
    let mut f = Fixture::new("stale");
    let mut rows = f.history("counter", 20.0, &[]);
    rows.push(observation(200.0, at().sub(Span::hours(2)).unwrap()));
    f.store.ingest(&rows).unwrap();
    let report = analyse(&f.cfg, &f.store, at()).unwrap();
    let r = region(&report);
    assert_eq!(text(r, "level"), "insufficient_data");
    assert_eq!(
        r.index("stale_streams").unwrap().as_list().unwrap().len(),
        1
    );
}

#[test]
fn ttl_config_caps_feed_claims() {
    let mut f = Fixture::new("ttl");
    let long = at().add(Span::days(1)).unwrap().stamp();
    let o = observation_kw(
        200.0,
        at().sub(Span::hours(2)).unwrap(),
        &[("valid_until", Value::from(long))],
    );
    f.store.ingest(&[o]).unwrap();
    let report = analyse(&f.cfg, &f.store, at()).unwrap();
    assert!(matches!(
        region(&report).index("direct_streams").unwrap(),
        Value::Int(0)
    ));
}

#[test]
fn recent_anomaly_is_excluded_from_baseline() {
    let f = Fixture::new("baseline");
    let mut history = f.history("counter", 20.0, &[]);
    history.push(observation(999.0, at().sub(minutes(5)).unwrap()));
    let reference = baseline(&history, &observation(200.0, at()), 8)
        .unwrap()
        .unwrap();
    assert!(reference.median < 25.0);
}

#[test]
fn single_spike_watch_then_sustained_elevated() {
    let mut f = Fixture::new("spike");
    let mut rows = f.history("counter", 20.0, &[]);
    rows.push(observation(200.0, at()));
    f.store.ingest(&rows).unwrap();
    let report = analyse(&f.cfg, &f.store, at()).unwrap();
    assert_eq!(text(region(&report), "level"), "watch");
    f.store
        .ingest(&[observation(200.0, at().sub(minutes(5)).unwrap())])
        .unwrap();
    let report = analyse(&f.cfg, &f.store, at()).unwrap();
    assert_eq!(text(region(&report), "level"), "elevated");
}

#[test]
fn zero_variance_baseline_is_finite() {
    let history: Vec<Observation> = (0..8)
        .map(|i| {
            observation(
                0.0,
                at().sub(Span::days(1)).unwrap().sub(minutes(i)).unwrap(),
            )
        })
        .collect();
    let reference = baseline(&history, &observation(1.0, at()), 8)
        .unwrap()
        .unwrap();
    assert!(((1.0 - reference.median) / reference.scale).is_finite());
}

#[test]
fn correlated_copies_cannot_corroborate() {
    let mut f = Fixture::new("copies");
    let mut copy = f.first_source().clone();
    copy.insert("id", Value::from("mirror"));
    copy.insert("independence_group", Value::from("origin"));
    f.first_source()
        .insert("independence_group", Value::from("origin"));
    if let Value::Dict(top) = &mut f.config_raw {
        if let Some(Value::List(sources)) = top.get_mut("sources") {
            sources.push(Value::Dict(copy));
        }
    }
    let cfg = f.config().unwrap();
    for source in ["counter", "mirror"] {
        let mut rows = f.history(source, 20.0, &[]);
        for when in [at(), at().sub(minutes(5)).unwrap()] {
            rows.push(
                observation_with(
                    Value::Float(200.0),
                    when,
                    source,
                    Value::from("footfall"),
                    "sensor",
                    &[],
                )
                .unwrap(),
            );
        }
        f.store.ingest(&rows).unwrap();
    }
    let report = analyse(&cfg, &f.store, at()).unwrap();
    let r = region(&report);
    assert_eq!(text(r, "level"), "elevated");
    assert_eq!(
        r.index("corroborating_groups")
            .unwrap()
            .as_list()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn disabled_source_excluded() {
    let mut f = Fixture::new("disabled");
    let mut rows = f.history("counter", 20.0, &[]);
    rows.push(observation(200.0, at()));
    f.store.ingest(&rows).unwrap();
    f.first_source().insert("enabled", Value::Bool(false));
    let cfg = f.config().unwrap();
    let report = analyse(&cfg, &f.store, at()).unwrap();
    assert!(matches!(
        region(&report).index("direct_streams").unwrap(),
        Value::Int(0)
    ));
}

#[test]
fn low_quality_does_not_drive_alert() {
    let mut f = Fixture::new("quality");
    let mut rows = f.history("counter", 20.0, &[]);
    rows.push(observation_kw(
        200.0,
        at(),
        &[("quality", Value::Float(0.1))],
    ));
    f.store.ingest(&rows).unwrap();
    let report = analyse(&f.cfg, &f.store, at()).unwrap();
    assert_eq!(text(region(&report), "level"), "insufficient_data");
}

#[test]
fn linear_projection_and_horizon_bound() {
    let rows: Vec<Observation> = (0..9)
        .map(|i| {
            observation(
                50.0 + 5.0 * i as f64,
                at().sub(minutes(40 - i * 5)).unwrap(),
            )
        })
        .collect();
    let p = project(&rows, Num::Int(15)).unwrap().unwrap();
    assert!((p.value.as_f64() - 105.0).abs() < 1e-7);
    assert!(p.horizon.as_f64() <= 17.5);
}

#[test]
fn forecast_suppressed_across_gap() {
    let mut rows: Vec<Observation> = (0..6)
        .map(|i| observation(50.0, at().sub(minutes(100 - i * 5)).unwrap()))
        .collect();
    rows.push(observation(80.0, at()));
    assert!(project(&rows, Num::Int(15)).unwrap().is_none());
}

#[test]
fn conservation_projection() {
    let region = parse_str(
        r#"{"balance":{"occupancy":"counter/occ","inflow":"counter/in","outflow":"counter/out"},
            "capacity_people":100,"occupancy_stream":"counter/occ"}"#,
    )
    .unwrap();
    let region = region.as_dict().unwrap();
    let rows = [
        observation_with(
            Value::Int(80),
            at(),
            "counter",
            Value::from("occupancy"),
            "occ",
            &[],
        )
        .unwrap(),
        observation_with(
            Value::Int(5),
            at(),
            "counter",
            Value::from("inflow"),
            "in",
            &[],
        )
        .unwrap(),
        observation_with(
            Value::Int(3),
            at(),
            "counter",
            Value::from("outflow"),
            "out",
            &[],
        )
        .unwrap(),
    ];
    let currents: Vec<&Observation> = rows.iter().collect();
    let result = balance_projection(region, &currents, at(), Num::Int(15))
        .unwrap()
        .unwrap();
    assert!(matches!(result.index("projected_occupancy").unwrap(), Value::Float(v) if *v == 110.0));
    assert!(
        matches!(result.index("minutes_to_configured_capacity").unwrap(), Value::Float(v) if *v == 10.0)
    );
    let fewer: Vec<&Observation> = rows[..2].iter().collect();
    let missing = balance_projection(region, &fewer, at(), Num::Int(15))
        .unwrap()
        .unwrap();
    assert_eq!(text(&missing, "status"), "unavailable");
}

#[test]
fn backtest_cannot_use_late_backfill() {
    let mut f = Fixture::new("backfill");
    let rows: Vec<Observation> = (0..19)
        .map(|i| {
            observation_kw(
                20.0 + i as f64,
                at().sub(minutes(90 - i * 5)).unwrap(),
                &[("retrieved_at", Value::from(at().stamp()))],
            )
        })
        .collect();
    f.store.ingest(&rows).unwrap();
    let result = backtest(&f.cfg, &f.store, at(), Num::Int(15)).unwrap();
    assert!(result
        .index("series")
        .unwrap()
        .as_list()
        .unwrap()
        .is_empty());
}

// ---------------------------------------------------------------------------------------
// ExportTests
// ---------------------------------------------------------------------------------------

fn spiked(name: &str) -> (Fixture, Value) {
    let mut f = Fixture::new(name);
    let mut rows = f.history("counter", 20.0, &[]);
    rows.push(observation(200.0, at()));
    f.store.ingest(&rows).unwrap();
    let report = analyse(&f.cfg, &f.store, at()).unwrap();
    (f, report)
}

#[test]
fn native_serialization_matches_checked_rust_shapes() {
    let (f, report) = spiked("native");
    let out = f.root.join("native");
    let manifest = export_skeletrace(&report, &out).unwrap();
    let profile_text = std::fs::read(text(&manifest, "profile")).unwrap();
    let profile = crowdwatch::model::load_json_bytes(&profile_text).unwrap();
    let mut keys: Vec<&str> = profile
        .as_dict()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "boundaries",
            "config",
            "edges",
            "metrics",
            "name",
            "nodes",
            "sources"
        ]
    );
    let source = &profile.index("sources").unwrap().as_list().unwrap()[0];
    assert_eq!(text(source, "adapter_profile"), "NdjsonFile");
    assert_eq!(
        text(source.index("definition").unwrap(), "adapter"),
        "FileImport"
    );
    let samples = std::fs::read_to_string(text(&manifest, "samples")).unwrap();
    let records: Vec<Value> = samples.lines().map(|l| load_json(l).unwrap()).collect();
    assert_eq!(records.len(), 5);
    let metric_ids: Vec<&str> = profile
        .index("metrics")
        .unwrap()
        .as_list()
        .unwrap()
        .iter()
        .map(|m| text(m, "id"))
        .collect();
    let entity_ids: Vec<&str> = profile
        .index("nodes")
        .unwrap()
        .as_list()
        .unwrap()
        .iter()
        .map(|n| text(n, "id"))
        .collect();
    for row in &records {
        let mut keys: Vec<&str> = row.as_dict().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "entity_id",
                "metric_id",
                "quality",
                "raw_payload",
                "ts_ingested",
                "ts_observed",
                "value"
            ]
        );
        assert!(metric_ids.contains(&text(row, "metric_id")));
        assert!(entity_ids.contains(&text(row, "entity_id")));
        assert!(uuid::Uuid::parse_str(text(row, "metric_id")).is_ok());
        assert!(
            parse_time_str(text(row, "ts_observed")).unwrap()
                <= parse_time_str(text(row, "ts_ingested")).unwrap()
        );
        let value = row.index("value").unwrap();
        let shaped = match value {
            Value::Str(s) => s == "Missing",
            Value::Dict(d) => d.len() == 1 && (d.contains_key("Code") || d.contains_key("Numeric")),
            _ => false,
        };
        assert!(shaped, "{value:?}");
    }
    let node = &profile.index("nodes").unwrap().as_list().unwrap()[0];
    assert_eq!(
        json_text(node.index("position").unwrap(), false).unwrap(),
        r#"{"alt":null,"lat":51.5,"lon":-0.12}"#
    );
    let again = export_skeletrace(&report, &out).unwrap();
    assert_eq!(text(&manifest, "samples"), text(&again, "samples"));
}

#[test]
fn native_missing_is_not_numeric_zero() {
    let f = Fixture::new("missing-native");
    let report = analyse(&f.cfg, &f.store, at()).unwrap();
    let manifest = export_skeletrace(&report, &f.root.join("native")).unwrap();
    let samples = std::fs::read_to_string(text(&manifest, "samples")).unwrap();
    let rows: Vec<Value> = samples.lines().map(|l| load_json(l).unwrap()).collect();
    assert_eq!(text(&rows[1], "value"), "Missing");
}

#[test]
fn geojson_coordinate_order() {
    let f = Fixture::new("geojson");
    let report = analyse(&f.cfg, &f.store, at()).unwrap();
    let features = geojson(&report).unwrap();
    let first = &features.index("features").unwrap().as_list().unwrap()[0];
    let coordinates = first
        .index("geometry")
        .unwrap()
        .index("coordinates")
        .unwrap();
    assert_eq!(json_text(coordinates, false).unwrap(), "[-0.12,51.5]");
}

#[test]
fn html_injection_escaped() {
    let mut f = Fixture::new("html");
    let o = observation_kw(
        20.0,
        at(),
        &[("label", Value::from("<img src=x onerror=\"alert(1)\">"))],
    );
    f.store.ingest(&[o]).unwrap();
    let report = analyse(&f.cfg, &f.store, at()).unwrap();
    let out = f.root.join("report.html");
    render(&report, &out).unwrap();
    let page = std::fs::read_to_string(&out).unwrap();
    assert!(!page.contains("<img src=x"));
    assert!(page.contains("&lt;img"));
    assert!(page.contains("Content-Security-Policy"));
    assert!(!page.contains("<script"));
}

#[test]
fn demo_is_complete_and_labelled() {
    let f = Fixture::new("demo");
    let result = create_demo(&f.root.join("demo"), Some(at())).unwrap();
    let levels = result.index("levels").unwrap();
    assert_eq!(
        json_text(levels, false).unwrap(),
        r#"{"market":"insufficient_data","plaza":"baseline","station":"corroborated"}"#
    );
    let page = std::fs::read_to_string(text(&result, "report")).unwrap();
    assert!(page.contains("SYNTHETIC DEMONSTRATION"));
    assert!(is_input_error(create_demo(
        &f.root.join("demo"),
        Some(at())
    )));
}

// ---------------------------------------------------------------------------------------
// HttpTests
// ---------------------------------------------------------------------------------------

#[test]
fn rejects_private_or_credential_urls() {
    for url in [
        "http://example.org",
        "https://localhost/x",
        "https://127.0.0.1/x",
        "https://[::1]/",
        "https://user:secret@example.org/",
        "https://example.org:8443/",
    ] {
        assert!(is_input_error(validate_url(url, false)), "{url}");
    }
    assert_eq!(
        validate_url("https://example.org/feed", false).unwrap(),
        "example.org"
    );
}

#[test]
fn geohash_reference_vector() {
    assert_eq!(geohash(42.6, -5.6, 5), "ezs42");
}

/// The keys of an observation dict, for readable failures.
#[allow(dead_code)]
fn keys(d: &Dict) -> Vec<String> {
    d.keys().cloned().collect()
}
