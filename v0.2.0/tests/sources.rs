// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! 0.1.0's `tests/test_sources.py`, test for test: every collector against recorded
//! provider replies, with no network.

mod common;

use common::{at, minutes, observation, Fixture};
use crowdwatch::error::{Fail, Result};
use crowdwatch::http::{Env, Http};
use crowdwatch::json::{parse_str, Dict, Value};
use crowdwatch::model::json_text;
use crowdwatch::pytime::Span;
use crowdwatch::sources::{collect_sources, Batch, Collector};

/// A recorded reply: JSON, raw bytes, or a failure.
enum Reply {
    Json(Value),
    Bytes(Vec<u8>),
    Fail(Fail),
}

/// 0.1.0's `FakeClient`: hands out replies in order and remembers the URLs asked for.
struct FakeClient {
    replies: Vec<Reply>,
    urls: Vec<String>,
}

impl FakeClient {
    fn new(replies: Vec<Reply>) -> FakeClient {
        FakeClient {
            replies,
            urls: Vec::new(),
        }
    }

    fn next(&mut self, url: &str) -> Result<Reply> {
        self.urls.push(url.to_owned());
        if self.replies.is_empty() {
            return Err(Fail::py("IndexError", "pop from empty list"));
        }
        match self.replies.remove(0) {
            Reply::Fail(fail) => Err(fail),
            other => Ok(other),
        }
    }
}

impl Http for FakeClient {
    fn get(&mut self, url: &str, _headers: &[(String, String)]) -> Result<Vec<u8>> {
        match self.next(url)? {
            Reply::Bytes(bytes) => Ok(bytes),
            Reply::Json(value) => Ok(json_text(&value, false)?.into_bytes()),
            Reply::Fail(_) => unreachable!(),
        }
    }

    fn json(&mut self, url: &str, _headers: &[(String, String)]) -> Result<Value> {
        match self.next(url)? {
            Reply::Json(value) => Ok(value),
            Reply::Bytes(bytes) => crowdwatch::model::load_json_bytes(&bytes),
            Reply::Fail(_) => unreachable!(),
        }
    }
}

/// Environment variables for one test.
struct FakeEnv(Vec<(&'static str, &'static str)>);

impl Env for FakeEnv {
    fn var(&self, name: &str) -> Option<String> {
        self.0
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| (*v).to_owned())
    }
}

fn json(text: &str) -> Value {
    parse_str(text).unwrap()
}

/// `{**cfg.sources["counter"], "kind": kind, "options": options}`.
fn source(f: &Fixture, kind: &str, options: Value) -> Dict {
    let mut s = f.cfg.source("counter").unwrap().clone();
    s.insert("kind", Value::from(kind));
    s.insert("options", options);
    s
}

fn collect(
    f: &Fixture,
    replies: Vec<Reply>,
    env: &FakeEnv,
    s: &Dict,
) -> (Result<Batch>, Vec<String>) {
    let mut client = FakeClient::new(replies);
    let result = {
        let mut collector = Collector {
            config: &f.cfg,
            client: &mut client,
            env,
        };
        collector.collect(s, at())
    };
    (result, client.urls)
}

fn no_env() -> FakeEnv {
    FakeEnv(Vec::new())
}

fn weather(units_wind: &str, temperature: i64) -> Value {
    json(&format!(
        r#"{{"current":{{"time":{},"interval":900,"temperature_2m":{temperature},
            "precipitation":0,"wind_speed_10m":12}},
            "current_units":{{"temperature_2m":"°C","precipitation":"mm","wind_speed_10m":"{units_wind}"}}}}"#,
        at().timestamp() as i64
    ))
}

#[test]
fn open_meteo_values_units_and_model_label() {
    let f = Fixture::new("meteo");
    let s = source(&f, "open_meteo", Value::Dict(Dict::new()));
    let (batch, _) = collect(&f, vec![Reply::Json(weather("km/h", 18))], &no_env(), &s);
    let rows = batch.unwrap().observations;
    let metrics: Vec<&str> = rows.iter().map(|o| o.metric.as_str()).collect();
    assert_eq!(metrics, ["temperature", "precipitation", "wind"]);
    assert_eq!(rows[1].period_seconds, 900);
    assert_eq!(
        rows[0].details.get("measurement_type").unwrap().as_str(),
        Some("weather model")
    );
    let (result, _) = collect(&f, vec![Reply::Json(weather("mph", 18))], &no_env(), &s);
    assert!(matches!(result, Err(Fail::Input(_))));
}

#[test]
fn tfl_status_uses_explicit_lines() {
    let f = Fixture::new("tfl");
    let data = json(
        r#"[{"id":"jubilee","name":"Jubilee","lineStatuses":[{"statusSeverityDescription":"Severe Delays"}]}]"#,
    );
    let s = source(&f, "tfl", json(r#"{"line_ids":["jubilee"]}"#));
    let (batch, _) = collect(&f, vec![Reply::Json(data)], &no_env(), &s);
    let rows = batch.unwrap().observations;
    assert_eq!(rows[0].value, 0.7);
    assert!(rows[0]
        .details
        .get("scope")
        .unwrap()
        .as_str()
        .unwrap()
        .contains("whole line"));
}

#[test]
fn unknown_tfl_status_not_invented() {
    let f = Fixture::new("tfl-unknown");
    let data =
        json(r#"[{"id":"jubilee","lineStatuses":[{"statusSeverityDescription":"New status"}]}]"#);
    let s = source(&f, "tfl", json(r#"{"line_ids":["jubilee"]}"#));
    let (batch, _) = collect(&f, vec![Reply::Json(data)], &no_env(), &s);
    let batch = batch.unwrap();
    assert!(batch.observations.is_empty());
    assert!(!batch.warnings.is_empty());
}

#[test]
fn gbfs_v2_and_v3() {
    let f = Fixture::new("gbfs");
    let epoch = (at().timestamp() as i64).to_string();
    let iso = format!("\"{}\"", at().stamp());
    for (timestamp, count_field) in [
        (epoch.as_str(), "num_bikes_available"),
        (iso.as_str(), "num_vehicles_available"),
    ] {
        let info = json(
            r#"{"data":{"stations":[{"station_id":"a","lat":51.5,"lon":-0.12,"name":"Bike station"},
                {"station_id":"b","lat":40,"lon":-72,"name":"Far away"}]}}"#,
        );
        let state = json(&format!(
            r#"{{"data":{{"stations":[{{"station_id":"a","is_installed":true,"is_renting":true,
                "is_returning":true,"last_reported":{timestamp},"{count_field}":6,"num_docks_available":4}}]}}}}"#
        ));
        let options = json(
            r#"{"station_information_url":"https://example.org/info","station_status_url":"https://example.org/status"}"#,
        );
        let s = source(&f, "gbfs", options);
        let (batch, _) = collect(
            &f,
            vec![Reply::Json(info), Reply::Json(state)],
            &no_env(),
            &s,
        );
        let rows = batch.unwrap().observations;
        let values: Vec<f64> = rows.iter().map(|o| o.value).collect();
        assert_eq!(values, [6.0, 4.0]);
        assert!(rows[0]
            .details
            .get("interpretation")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("rebalancing"));
    }
}

#[test]
fn ticketmaster_geohash_time_and_auth_redaction() {
    let f = Fixture::new("ticketmaster");
    let event = json(&format!(
        r#"{{"id":"abc","name":"Concert","url":"https://example.org/event?tracking=1",
            "dates":{{"start":{{"dateTime":"{}"}}}},
            "_embedded":{{"venues":[{{"location":{{"latitude":"51.5","longitude":"-0.12"}}}}]}}}}"#,
        at().add(Span::hours(1)).unwrap().stamp()
    ));
    let page = Value::Dict(
        Dict::new()
            .with(
                "_embedded",
                Dict::new().with("events", Value::List(vec![event])),
            )
            .with("page", Dict::new().with("totalPages", 1_i64)),
    );
    let s = source(&f, "ticketmaster", Value::Dict(Dict::new()));
    let env = FakeEnv(vec![("TICKETMASTER_API_KEY", "secret-test")]);
    let (batch, urls) = collect(&f, vec![Reply::Json(page)], &env, &s);
    let rows = batch.unwrap().observations;
    assert_eq!(rows[0].metric, "scheduled_event");
    assert_eq!(
        rows[0].details.get("attendance").unwrap().as_str(),
        Some("unknown")
    );
    assert!(urls[0].contains("geoPoint="));
    assert!(!json_text(&rows[0].to_value(), false)
        .unwrap()
        .contains("secret-test"));
    assert_eq!(rows[0].provenance_url, "https://example.org/event");
}

#[test]
fn missing_api_key_is_explicit() {
    let f = Fixture::new("no-key");
    let s = source(&f, "ticketmaster", Value::Dict(Dict::new()));
    let (result, _) = collect(&f, Vec::new(), &no_env(), &s);
    assert!(matches!(result, Err(Fail::Fetch { .. })));
}

#[test]
fn melbourne_join_filters_area_and_paginates() {
    let f = Fixture::new("melbourne");
    let loc = json(
        r#"{"total_count":1,"results":[{"location_id":3,"status":"A","latitude":51.5,
            "longitude":-0.12,"sensor_description":"Counter"}]}"#,
    );
    let readings = json(&format!(
        r#"{{"total_count":1,"results":[{{"location_id":3,"sensing_datetime":"{}",
            "total_of_directions":19,"direction_1":7,"direction_2":12}}]}}"#,
        at().stamp()
    ));
    let s = source(&f, "melbourne", json(r#"{"sensor_ids":[3]}"#));
    let (batch, urls) = collect(
        &f,
        vec![Reply::Json(loc), Reply::Json(readings)],
        &no_env(),
        &s,
    );
    let rows = batch.unwrap().observations;
    assert_eq!(rows[0].value, 19.0);
    assert_eq!(rows[0].period_seconds, 60);
    assert_eq!(rows[0].stream_id, "sensor-3");
    assert!(urls[1].contains("where=location_id"));
}

const RSS: &[u8] = b"<rss><channel><item><guid>1</guid><title>Road closure</title><link>https://example.org/news</link><pubDate>Thu, 01 Oct 2026 11:55:00 GMT</pubDate></item></channel></rss>";

#[test]
fn rss_keeps_reporting_separate() {
    let f = Fixture::new("rss");
    let mut s = source(
        &f,
        "rss",
        json(r#"{"url":"https://example.org/rss","area_scoped":true}"#),
    );
    let (batch, _) = collect(&f, vec![Reply::Bytes(RSS.to_vec())], &no_env(), &s);
    assert_eq!(batch.unwrap().observations[0].metric, "report");
    s.insert(
        "options",
        json(r#"{"url":"https://example.org/rss","area_scoped":false}"#),
    );
    let (batch, _) = collect(&f, vec![Reply::Bytes(RSS.to_vec())], &no_env(), &s);
    let batch = batch.unwrap();
    assert!(batch.observations.is_empty());
    assert!(!batch.warnings.is_empty());
}

#[test]
fn rss_rejects_entities() {
    let f = Fixture::new("rss-entities");
    let body = b"<!DOCTYPE rss [<!ENTITY x \"secret\">]><rss/>".to_vec();
    let s = source(&f, "rss", json(r#"{"url":"https://example.org/rss"}"#));
    let (result, _) = collect(&f, vec![Reply::Bytes(body)], &no_env(), &s);
    assert!(matches!(result, Err(Fail::Input(_))));
}

#[test]
fn mapped_json_uses_explicit_timestamp() {
    let f = Fixture::new("mapped");
    let payload = json(&format!(
        r#"{{"results":[{{"sensor":"abc","n":34,"time":"{}"}}],"total":2}}"#,
        at().stamp()
    ));
    let options = json(
        r#"{"url":"https://example.org/counts","items_pointer":"/results","total_pointer":"/total",
            "metric":"footfall","period_seconds":60,
            "fields":{"stream_id":"/sensor","value":"/n","observed_at":"/time"}}"#,
    );
    let s = source(&f, "mapped_json", options);
    let (batch, _) = collect(&f, vec![Reply::Json(payload)], &no_env(), &s);
    let batch = batch.unwrap();
    assert_eq!(batch.observations[0].value, 34.0);
    assert!(!batch.warnings.is_empty());
}

#[test]
fn canonical_network_source_cannot_claim_early_availability() {
    let f = Fixture::new("canonical-network");
    let old = observation(35.0, at().sub(Span::hours(2)).unwrap());
    let s = source(
        &f,
        "canonical_json",
        json(r#"{"url":"https://example.org/aggregate"}"#),
    );
    let (batch, _) = collect(
        &f,
        vec![Reply::Json(Value::List(vec![old.to_value()]))],
        &no_env(),
        &s,
    );
    assert_eq!(batch.unwrap().observations[0].retrieved_at, at().stamp());
}

#[test]
fn failed_source_does_not_stop_next_source() {
    let mut f = Fixture::new("failed-source");
    let template = f.first_source().clone();
    let broken = template
        .clone()
        .with("kind", "open_meteo")
        .with("id", "broken");
    let good = template.with("kind", "open_meteo").with("id", "good");
    if let Value::Dict(top) = &mut f.config_raw {
        top.insert(
            "sources",
            Value::List(vec![Value::Dict(broken), Value::Dict(good)]),
        );
    }
    let cfg = f.config().unwrap();
    let mut client = FakeClient::new(vec![
        Reply::Fail(Fail::fetch("HTTP 503 from source")),
        Reply::Json(weather("km/h", 17)),
    ]);
    let result = collect_sources(
        &cfg,
        &mut f.store,
        at(),
        false,
        None,
        &mut client,
        &no_env(),
    )
    .unwrap();
    let statuses: Vec<&str> = result
        .iter()
        .map(|r| r.index("status").unwrap().as_str().unwrap())
        .collect();
    assert_eq!(statuses, ["error", "ok"]);
    assert!(matches!(
        f.store.counts().unwrap().get("observations"),
        Some(Value::Int(3))
    ));
    // A collection inside the next-due window is skipped, and --force does not
    // override a source still being collected.
    let again = collect_sources(
        &cfg,
        &mut f.store,
        at().add(minutes(1)).unwrap(),
        false,
        None,
        &mut FakeClient::new(Vec::new()),
        &no_env(),
    )
    .unwrap();
    assert!(again
        .iter()
        .all(|r| r.index("status").unwrap().as_str() == Some("not_due")));
}
