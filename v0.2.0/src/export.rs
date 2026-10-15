// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Native SkeleTrace `FileSampleRecord` / `EngineProfile` and GeoJSON exports.
//!
//! Identifiers are UUID version 5 in 0.1.0's namespace, so a region keeps the same entity
//! ID and each metric the same metric ID across versions. SkeleTrace's
//! `profile-validate` and `poll-source` commands read the result.

use std::path::Path;

use uuid::Uuid;

use crate::config::resolve;
use crate::error::Result;
use crate::json::{Dict, Value};
use crate::model::{atomic_write, json_text, sha256_hex};

/// 0.1.0's UUID namespace for CrowdWatch identifiers.
pub const NAMESPACE: Uuid = Uuid::from_u128(0xff93bdc4_1c04_5b3e_9c67_e3fef9a03c6a);
/// The SkeleTrace commit the export shapes were checked against.
pub const UPSTREAM_COMMIT: &str = "befa8404fb6f494315eeaa3639f263498fb89e26";

/// `uuid5(NAMESPACE, text)` as text.
pub fn uid(text: &str) -> String {
    Uuid::new_v5(&NAMESPACE, text.as_bytes()).to_string()
}

/// The ID of CrowdWatch's snapshot source.
pub fn source_id() -> String {
    uid("source:crowdwatch")
}

fn at<'a>(value: &'a Value, key: &str) -> Result<&'a Value> {
    value.index(key)
}

fn list<'a>(value: &'a Value, key: &str) -> Result<&'a [Value]> {
    match at(value, key)? {
        Value::List(items) => Ok(items),
        other => Err(crate::error::Fail::type_error(format!(
            "'{}' object is not a list",
            other.type_name()
        ))),
    }
}

/// `"region:" + id`, with Python's TypeError for an ID that is not text.
fn region_entity(area: &Value) -> Result<String> {
    match at(area, "id")? {
        Value::Str(id) => Ok(uid(&format!("region:{id}"))),
        other => Err(crate::error::Fail::type_error(format!(
            "can only concatenate str (not \"{}\") to str",
            other.type_name()
        ))),
    }
}

/// The regions as GeoJSON points (longitude first, as GeoJSON orders them).
pub fn geojson(report: &Value) -> Result<Value> {
    let mut features = Vec::new();
    for r in list(report, "regions")? {
        let area = at(r, "region")?;
        features.push(Value::Dict(
            Dict::new()
                .with("type", "Feature")
                .with("id", region_entity(area)?)
                .with(
                    "geometry",
                    Dict::new().with("type", "Point").with(
                        "coordinates",
                        Value::List(vec![
                            at(area, "longitude")?.clone(),
                            at(area, "latitude")?.clone(),
                        ]),
                    ),
                )
                .with(
                    "properties",
                    Dict::new()
                        .with("name", at(area, "name")?.clone())
                        .with("region_id", at(area, "id")?.clone())
                        .with("radius_km", at(area, "radius_km")?.clone())
                        .with("level", at(r, "level")?.clone())
                        .with("anomaly_index", at(r, "anomaly_index")?.clone())
                        .with("direct_streams", at(r, "direct_streams")?.clone())
                        .with("as_of", at(report, "as_of")?.clone())
                        .with("reasons", at(r, "reasons")?.clone())
                        .with("interpretation", at(report, "limitations")?.clone()),
                ),
        ));
    }
    Ok(Value::Dict(
        Dict::new()
            .with("type", "FeatureCollection")
            .with("features", Value::List(features)),
    ))
}

fn duration(seconds: i64) -> Value {
    Value::Dict(Dict::new().with("secs", seconds).with("nanos", 0_i64))
}

fn tag(key: &str, value: &str) -> Value {
    Value::Dict(Dict::new().with("key", key).with("value", value))
}

/// The five metrics, in 0.1.0's order: name, value type, unit and description.
const DEFINITIONS: [(&str, &str, &str, &str); 5] = [
    (
        "status",
        "Code",
        "status",
        "Descriptive evidence status; baseline does not mean safe",
    ),
    (
        "anomaly_index",
        "Numeric",
        "index/100",
        "Strongest comparable movement anomaly; not a probability",
    ),
    (
        "direct_streams",
        "Numeric",
        "streams",
        "Fresh direct measurement streams with sufficient configured quality",
    ),
    (
        "stale_streams",
        "Numeric",
        "streams",
        "Stale observed streams excluded from inference",
    ),
    (
        "context_records",
        "Numeric",
        "records",
        "Fresh contextual records; these are not people",
    ),
];

/// 0.1.0's `export_skeletrace(report, directory)`: an immutable sample snapshot named by
/// its hash, the engine profile that reads it, the GeoJSON areas and a manifest.
///
/// Load the profile afresh for each snapshot; the host's file adapter keeps a line
/// cursor, so reusing a profile for a new file would skip lines.
pub fn export_skeletrace(report: &Value, directory: &Path) -> Result<Value> {
    let out = resolve(directory);
    let source = source_id();
    let metrics: Vec<Value> = DEFINITIONS
        .iter()
        .enumerate()
        .map(|(i, (name, kind, unit, description))| {
            Value::Dict(
                Dict::new()
                    .with("id", uid(&format!("metric:{name}")))
                    .with("name", format!("CrowdWatch {name}"))
                    .with("unit", *unit)
                    .with("value_type", *kind)
                    .with("cadence", "Manual")
                    .with("interpolation", "None")
                    .with(
                        "retention",
                        Dict::new()
                            .with("hot_duration", duration(3600))
                            .with("warm_duration", duration(2_592_000))
                            .with("store_on_change_only", false)
                            .with("change_threshold", Value::Null)
                            .with("relative_change_threshold", Value::Null)
                            .with("max_silent_gap", duration(600)),
                    )
                    .with(
                        "source_ids",
                        Value::List(vec![Value::from(source.as_str())]),
                    )
                    .with("show_in_popup", true)
                    .with("popup_priority", i)
                    .with("description", *description),
            )
        })
        .collect();
    let as_of = at(report, "as_of")?;
    let limitations = at(report, "limitations")?;
    let mut nodes = Vec::new();
    let mut samples = Vec::new();
    for r in list(report, "regions")? {
        let area = at(r, "region")?;
        let entity = region_entity(area)?;
        let level = at(r, "level")?;
        let status = match level.as_str() {
            Some("insufficient_data") => "Stale",
            Some("elevated" | "corroborated") => "Alerting",
            _ => "Active",
        };
        nodes.push(Value::Dict(
            Dict::new()
                .with("id", entity.as_str())
                .with("kind", "Anchor")
                .with("label", at(area, "name")?.clone())
                .with(
                    "position",
                    Dict::new()
                        .with("lat", at(area, "latitude")?.clone())
                        .with("lon", at(area, "longitude")?.clone())
                        .with("alt", Value::Null),
                )
                .with("position_confidence", 1.0)
                .with("status", status)
                .with("priority", 128_i64)
                .with(
                    "tags",
                    Value::List(vec![
                        tag("module", "crowdwatch"),
                        tag(
                            "spatial_scope",
                            "configured area centre; not a person's location",
                        ),
                    ]),
                )
                .with("first_seen", as_of.clone())
                .with("last_seen", as_of.clone()),
        ));
        let values = [
            level.clone(),
            at(r, "anomaly_index")?.clone(),
            at(r, "direct_streams")?.clone(),
            Value::from(list(r, "stale_streams")?.len()),
            Value::from(list(r, "context")?.len()),
        ];
        for ((name, kind, _, _), value) in DEFINITIONS.iter().zip(values) {
            let missing = value.is_null();
            samples.push(Value::Dict(
                Dict::new()
                    .with("entity_id", entity.as_str())
                    .with("metric_id", uid(&format!("metric:{name}")))
                    .with("ts_observed", as_of.clone())
                    .with("ts_ingested", as_of.clone())
                    .with(
                        "value",
                        if missing {
                            Value::from("Missing")
                        } else {
                            Value::Dict(Dict::new().with(*kind, value.clone()))
                        },
                    )
                    .with(
                        "quality",
                        if *name == "anomaly_index" && missing {
                            0.0
                        } else {
                            1.0
                        },
                    )
                    .with(
                        "raw_payload",
                        Dict::new()
                            .with("module", "crowdwatch")
                            .with("field", *name)
                            .with("value", value)
                            .with("evidence", r.clone())
                            .with("limitations", limitations.clone()),
                    ),
            ));
        }
    }
    let mut payload = String::new();
    for sample in &samples {
        payload.push_str(&json_text(sample, false)?);
        payload.push('\n');
    }
    let snapshot = sha256_hex(payload.as_bytes());
    let sample_path = out.join(format!("samples-{}.jsonl", &snapshot[..16]));
    atomic_write(&sample_path, &payload)?;
    let sample_text = sample_path.to_string_lossy().into_owned();
    let profile = Value::Dict(
        Dict::new()
            .with("name", "CrowdWatch area observatory")
            .with(
                "config",
                Dict::new()
                    .with(
                        "cache_budget",
                        Dict::new()
                            .with("max_active_entities", (nodes.len() * 2).max(1024))
                            .with("max_total_ring_samples", (samples.len() * 2).max(8192))
                            .with("max_highres_entities", 64_i64)
                            .with("max_approx_hot_bytes", 16_777_216_i64)
                            .with("max_per_entity_hot_bytes", 262_144_i64),
                    )
                    .with("default_ring_capacity", 32_i64)
                    .with("default_entity_ttl", duration(3600))
                    .with("journal_dir", Value::Null)
                    .with("warm_store_path", Value::Null),
            )
            .with("metrics", Value::List(metrics))
            .with("nodes", Value::List(nodes))
            .with("edges", Value::List(Vec::new()))
            .with("boundaries", Value::List(Vec::new()))
            .with(
                "sources",
                Value::List(vec![Value::Dict(
                    Dict::new()
                        .with(
                            "definition",
                            Dict::new()
                                .with("id", source.as_str())
                                .with("name", "CrowdWatch aggregate snapshot")
                                .with("kind", "File")
                                .with("adapter", "FileImport")
                                .with("schedule", "Manual")
                                .with("endpoint", sample_text.as_str())
                                .with("auth_ref", Value::Null)
                                .with("health", "Pending")
                                .with("last_polled", Value::Null)
                                .with("last_error", Value::Null)
                                .with("backoff", duration(5))
                                .with("max_backoff", duration(300))
                                .with("tags", Value::List(vec![tag("module", "crowdwatch")])),
                        )
                        .with("adapter_profile", "NdjsonFile"),
                )]),
            ),
    );
    let profile_path = out.join("skeletrace-profile.json");
    atomic_write(&profile_path, &(json_text(&profile, true)? + "\n"))?;
    atomic_write(
        &out.join("areas.geojson"),
        &(json_text(&geojson(report)?, true)? + "\n"),
    )?;
    let manifest = Value::Dict(
        Dict::new()
            .with("module", "skeletrace-crowdwatch")
            .with("version", env!("CARGO_PKG_VERSION"))
            .with("source_id", source.as_str())
            .with("integration", "EngineProfile + NdjsonFile / FileSampleRecord")
            .with("profile", profile_path.to_string_lossy().into_owned())
            .with("samples", sample_text.as_str())
            .with("snapshot_sha256", snapshot.as_str())
            .with(
                "host_reference",
                Dict::new()
                    .with("repository", "whisprer-specops/osint-skeletrace")
                    .with("commit", UPSTREAM_COMMIT)
                    .with("source_directory", "previous-versions/st12fix"),
            )
            .with(
                "reload_rule",
                "Instantiate/load the profile afresh for each snapshot; do not reuse a prior line cursor.",
            )
            .with(
                "validation",
                "Serialization contract checked; host runtime compatibility must be checked against the installed host build.",
            ),
    );
    atomic_write(
        &out.join("module.json"),
        &(json_text(&manifest, true)? + "\n"),
    )?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_match_0_1_0() {
        // uuid.uuid5(NAMESPACE, ...) in Python.
        assert_eq!(
            NAMESPACE.to_string(),
            "ff93bdc4-1c04-5b3e-9c67-e3fef9a03c6a"
        );
        assert_eq!(source_id(), "577a6a34-0694-5d4e-a76d-5b71cf8f825a");
        assert_eq!(
            uid("region:station"),
            "116a9098-ecc0-508b-b637-0e91f5faa375"
        );
        assert_eq!(uid("metric:status"), "20847ae7-6b2d-5e2a-983d-9f3dd2b5f083");
        assert_eq!(
            uid("region:caf\u{e9}"),
            "9faf3bb6-e2eb-58f4-83da-dfd4fcd1e76e"
        );
    }
}
