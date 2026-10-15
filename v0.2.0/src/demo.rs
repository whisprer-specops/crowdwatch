// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Deterministic synthetic fixture: growing station, ordinary plaza, missing counter.
//!
//! The invented readings come from Python's Mersenne Twister seeded with 1701, drawn in
//! 0.1.0's order, so a demo made at a fixed `--at` is the same in both versions.

use std::path::Path;

use crate::analysis::{analyse, backtest};
use crate::config::{resolve, Config};
use crate::error::{Fail, Result};
use crate::export::export_skeletrace;
use crate::json::{Dict, Value};
use crate::model::{atomic_write, json_text, unit_of, validate, Observation};
use crate::pytime::{Instant, Num, Span};
use crate::report::render;
use crate::store::Store;

/// Python's `random.Random`: MT19937 with CPython's seeding and `randint`.
pub struct PyRandom {
    state: [u32; 624],
    index: usize,
}

impl PyRandom {
    /// `random.Random(seed)` for a small non-negative integer seed.
    pub fn new(seed: u32) -> PyRandom {
        let mut rng = PyRandom {
            state: [0; 624],
            index: 624,
        };
        rng.init_genrand(19_650_218);
        // init_by_array with the key [seed].
        let key = [seed];
        let (mut i, mut j) = (1usize, 0usize);
        for _ in 0..624.max(key.len()) {
            let previous = rng.state[i - 1];
            rng.state[i] = (rng.state[i] ^ ((previous ^ (previous >> 30)).wrapping_mul(1_664_525)))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= 624 {
                rng.state[0] = rng.state[623];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
        }
        for _ in 0..623 {
            let previous = rng.state[i - 1];
            rng.state[i] = (rng.state[i]
                ^ ((previous ^ (previous >> 30)).wrapping_mul(1_566_083_941)))
            .wrapping_sub(i as u32);
            i += 1;
            if i >= 624 {
                rng.state[0] = rng.state[623];
                i = 1;
            }
        }
        rng.state[0] = 0x8000_0000;
        rng.index = 624;
        rng
    }

    fn init_genrand(&mut self, seed: u32) {
        self.state[0] = seed;
        for i in 1..624 {
            let previous = self.state[i - 1];
            self.state[i] = 1_812_433_253_u32
                .wrapping_mul(previous ^ (previous >> 30))
                .wrapping_add(i as u32);
        }
        self.index = 624;
    }

    fn next_u32(&mut self) -> u32 {
        const MATRIX_A: u32 = 0x9908_b0df;
        const UPPER: u32 = 0x8000_0000;
        const LOWER: u32 = 0x7fff_ffff;
        if self.index >= 624 {
            for k in 0..624 {
                let y = (self.state[k] & UPPER) | (self.state[(k + 1) % 624] & LOWER);
                let mag = if y & 1 == 0 { 0 } else { MATRIX_A };
                self.state[k] = self.state[(k + 397) % 624] ^ (y >> 1) ^ mag;
            }
            self.index = 0;
        }
        let mut y = self.state[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^= y >> 18;
        y
    }

    /// `getrandbits(k)` for 1 to 32 bits.
    fn getrandbits(&mut self, k: u32) -> u32 {
        self.next_u32() >> (32 - k)
    }

    /// `randint(a, b)`: rejection sampling on `getrandbits(width.bit_length())`.
    pub fn randint(&mut self, a: i64, b: i64) -> i64 {
        let width = (b - a + 1) as u64;
        let bits = 64 - width.leading_zeros();
        loop {
            let r = u64::from(self.getrandbits(bits));
            if r < width {
                return a + r as i64;
            }
        }
    }
}

fn region(id: &str, name: &str, latitude: f64, longitude: f64, radius_km: f64) -> Dict {
    Dict::new()
        .with("id", id)
        .with("name", name)
        .with("latitude", latitude)
        .with("longitude", longitude)
        .with("radius_km", radius_km)
}

/// One synthetic reading, made and checked as 0.1.0's `add(...)` made it.
struct Reading<'a> {
    source: &'a str,
    stream: &'a str,
    region: &'a str,
    metric: &'a str,
    value: Value,
    when: Instant,
    details: Dict,
    period: i64,
    label: &'a str,
}

fn synthetic(r: Reading<'_>) -> Result<Observation> {
    let mut details = Dict::new().with("synthetic", true);
    for (k, v) in r.details.iter() {
        details.insert(k.clone(), v.clone());
    }
    let fields = Dict::new()
        .with("source_id", r.source)
        .with("stream_id", r.stream)
        .with("region_id", r.region)
        .with("metric", r.metric)
        .with("value", r.value)
        .with("unit", unit_of(r.metric).unwrap_or(""))
        .with("observed_at", r.when.stamp())
        .with("retrieved_at", r.when.stamp())
        .with("valid_until", r.when.add(Span::seconds(600))?.stamp())
        .with("period_seconds", r.period)
        .with("quality", 0.9)
        .with("provenance_url", "")
        .with("label", r.label)
        .with("latitude", Value::Null)
        .with("longitude", Value::Null)
        .with("details", details);
    validate(&fields)
}

/// 0.1.0's `create_demo(directory, at)`: a configuration, source files, database, report,
/// backtest and SkeleTrace export in a new (or empty) folder.
pub fn create_demo(directory: &Path, at: Option<Instant>) -> Result<Value> {
    let out = resolve(directory);
    if out.exists() {
        let mut entries = std::fs::read_dir(&out).map_err(|e| crate::error::io_at(&out, e))?;
        if entries.next().is_some() {
            return Err(Fail::input(
                "demo output directory must be empty; choose a new directory",
            ));
        }
    }
    crate::model::make_dirs(&out)?;
    let at = at.unwrap_or_else(Instant::now).to_minute();
    let regions = vec![
        Value::Dict(
            region(
                "station",
                "Rivergate station \u{b7} synthetic",
                51.5074,
                -0.1278,
                0.5,
            )
            .with("capacity_people", 600_i64)
            .with("occupancy_stream", "gate-system/main")
            .with(
                "balance",
                Dict::new()
                    .with("occupancy", "gate-system/main")
                    .with("inflow", "gate-system/in")
                    .with("outflow", "gate-system/out"),
            ),
        ),
        Value::Dict(region(
            "plaza",
            "Quayside plaza \u{b7} synthetic",
            51.505,
            -0.119,
            0.4,
        )),
        Value::Dict(region(
            "market",
            "Market quarter \u{b7} synthetic",
            51.511,
            -0.14,
            0.5,
        )),
    ];
    let source_info = [
        ("pedestrian-counter", "station", "optical-counter"),
        ("gate-system", "station", "gate-system"),
        ("events-context", "station", "context"),
        ("plaza-counter", "plaza", "plaza-counter"),
        ("market-counter", "market", "market-counter"),
        ("market-weather", "market", "weather"),
    ];
    let sources: Vec<Value> = source_info
        .iter()
        .map(|(id, region, group)| {
            Value::Dict(
                Dict::new()
                    .with("id", *id)
                    .with("kind", "canonical_json")
                    .with("region_id", *region)
                    .with("independence_group", *group)
                    .with("quality", 0.9)
                    .with("ttl_seconds", 600_i64)
                    .with("interval_seconds", 300_i64)
                    .with(
                        "attribution",
                        "Synthetic CrowdWatch test fixture; no real people",
                    )
                    .with("options", Dict::new().with("path", format!("{id}.jsonl"))),
            )
        })
        .collect();
    let config = Value::Dict(
        Dict::new()
            .with("schema_version", 1_i64)
            .with("database", "crowdwatch.sqlite")
            .with("regions", Value::List(regions))
            .with("sources", Value::List(sources))
            .with("analysis", Dict::new().with("horizon_minutes", 15_i64)),
    );
    atomic_write(
        &out.join("config.json"),
        &(json_text(&config, true)? + "\n"),
    )?;

    let mut observations: Vec<Observation> = Vec::new();
    let mut rng = PyRandom::new(1701);
    let reading = |source, stream, region, metric, value: Value, when, period, label| Reading {
        source,
        stream,
        region,
        metric,
        value,
        when,
        details: Dict::new(),
        period,
        label,
    };
    for day in 1..22_i64 {
        for offset in [-20_i64, -10, 0, 10, 20] {
            let when = at.sub(Span::days(day))?.add(Span::minutes(offset))?;
            let footfall = Value::Int(30 + rng.randint(-3, 3));
            observations.push(synthetic(reading(
                "pedestrian-counter",
                "concourse",
                "station",
                "footfall",
                footfall,
                when,
                300,
                "Station pedestrian counter",
            ))?);
            let occupancy = Value::Int(240 + rng.randint(-20, 20));
            observations.push(synthetic(reading(
                "gate-system",
                "main",
                "station",
                "occupancy",
                occupancy,
                when,
                0,
                "Bounded concourse occupancy",
            ))?);
            let plaza = Value::Int(20 + rng.randint(-2, 2));
            observations.push(synthetic(reading(
                "plaza-counter",
                "plaza",
                "plaza",
                "footfall",
                plaza,
                when,
                300,
                "Plaza pedestrian counter",
            ))?);
        }
    }
    for minute in (-120_i64..=0).step_by(5) {
        let when = at.add(Span::minutes(minute))?;
        let growth = (minute + 55).max(0);
        // 30 + growth*1.7 + randint(...): a float, as in Python.
        let footfall = Value::Float(30.0 + growth as f64 * 1.7 + rng.randint(-2, 2) as f64);
        observations.push(synthetic(reading(
            "pedestrian-counter",
            "concourse",
            "station",
            "footfall",
            footfall,
            when,
            300,
            "Station pedestrian counter",
        ))?);
        let occupancy = Value::Int(240 + growth * 6 + rng.randint(-3, 3));
        observations.push(synthetic(reading(
            "gate-system",
            "main",
            "station",
            "occupancy",
            occupancy,
            when,
            0,
            "Bounded concourse occupancy",
        ))?);
        let plaza = Value::Int(20 + rng.randint(-2, 2));
        observations.push(synthetic(reading(
            "plaza-counter",
            "plaza",
            "plaza",
            "footfall",
            plaza,
            when,
            300,
            "Plaza pedestrian counter",
        ))?);
    }
    observations.push(synthetic(reading(
        "gate-system",
        "in",
        "station",
        "inflow",
        Value::Int(28),
        at,
        0,
        "Concourse entrance rate",
    ))?);
    observations.push(synthetic(reading(
        "gate-system",
        "out",
        "station",
        "outflow",
        Value::Int(18),
        at,
        0,
        "Concourse exit rate",
    ))?);
    observations.push(synthetic(reading(
        "events-context",
        "transit",
        "station",
        "transit_disruption",
        Value::Float(0.7),
        at,
        0,
        "Synthetic service disruption",
    ))?);
    observations.push(synthetic(Reading {
        details: Dict::new()
            .with("starts_at", at.add(Span::minutes(45))?.stamp())
            .with("ends_at", at.add(Span::hours(3))?.stamp())
            .with("attendance", "unknown"),
        ..reading(
            "events-context",
            "show",
            "station",
            "scheduled_event",
            Value::Int(1),
            at,
            0,
            "Synthetic nearby concert",
        )
    })?);
    observations.push(synthetic(reading(
        "market-counter",
        "entrance",
        "market",
        "footfall",
        Value::Int(44),
        at.sub(Span::hours(4))?,
        300,
        "Offline market counter",
    ))?);
    observations.push(synthetic(reading(
        "market-weather",
        "weather",
        "market",
        "temperature",
        Value::Float(16.5),
        at,
        0,
        "Synthetic weather context",
    ))?);
    for (id, _, _) in source_info {
        let mut text = String::new();
        for o in observations.iter().filter(|o| o.source_id == id) {
            text.push_str(&json_text(&o.to_value(), false)?);
            text.push('\n');
        }
        atomic_write(&out.join(format!("{id}.jsonl")), &text)?;
    }
    let cfg = Config::load(&out.join("config.json"))?;
    let (report, evaluation) = {
        let mut store = Store::open(&cfg.database)?;
        store.ingest(&observations)?;
        for (id, _, _) in source_info {
            let count = observations.iter().filter(|o| o.source_id == id).count();
            let error = if id == "market-counter" {
                "Synthetic outage: counter disconnected"
            } else {
                ""
            };
            store.finish(
                id,
                at,
                Num::Int(300),
                i64::try_from(count).unwrap_or(i64::MAX),
                error,
                &[],
                Num::Int(0),
            )?;
        }
        let mut report = analyse(&cfg, &store, at)?;
        if let Value::Dict(d) = &mut report {
            d.insert("synthetic", Value::Bool(true));
        }
        let evaluation = backtest(&cfg, &store, at, Num::Int(15))?;
        (report, evaluation)
    };
    atomic_write(
        &out.join("report.json"),
        &(json_text(&report, true)? + "\n"),
    )?;
    atomic_write(
        &out.join("backtest.json"),
        &(json_text(&evaluation, true)? + "\n"),
    )?;
    render(&report, &out.join("report.html"))?;
    let manifest = export_skeletrace(&report, &out.join("skeletrace"))?;
    let mut levels = Dict::new();
    if let Some(Value::List(regions)) = report.as_dict().and_then(|d| d.get("regions")) {
        for r in regions {
            let id = r
                .index("region")?
                .index("id")?
                .as_str()
                .unwrap_or("")
                .to_owned();
            levels.insert(id, r.index("level")?.clone());
        }
    }
    Ok(Value::Dict(
        Dict::new()
            .with("directory", out.to_string_lossy().into_owned())
            .with(
                "report",
                out.join("report.html").to_string_lossy().into_owned(),
            )
            .with("records", observations.len())
            .with("levels", levels)
            .with("skeletrace_profile", manifest.index("profile")?.clone()),
    ))
}

/// 0.1.0's `starter_config(preset)`: a runnable configuration for London (the default),
/// Melbourne or New York.
pub fn starter_config(preset: &str) -> Value {
    let (region, mut sources) = match preset {
        "melbourne" => {
            let region = region(
                "melbourne-central",
                "Melbourne Central",
                -37.81101524,
                144.96429485,
                0.8,
            );
            let sources = vec![Value::Dict(
                Dict::new()
                    .with("id", "melbourne-footfall")
                    .with("kind", "melbourne")
                    .with("region_id", "melbourne-central")
                    .with("quality", 0.85)
                    .with("ttl_seconds", 600_i64)
                    .with("interval_seconds", 300_i64)
                    .with(
                        "attribution",
                        "City of Melbourne Open Data pedestrian counting system",
                    )
                    .with(
                        "options",
                        Dict::new()
                            .with("sensor_ids", Value::List(vec![Value::Int(3)]))
                            .with("max_pages", 3_i64),
                    ),
            )];
            (region, sources)
        }
        "new-york" => {
            let region = region("midtown", "Midtown New York", 40.7549, -73.9840, 0.8);
            let sources = vec![Value::Dict(
                Dict::new()
                    .with("id", "citibike")
                    .with("kind", "gbfs")
                    .with("region_id", "midtown")
                    .with("quality", 0.8)
                    .with("ttl_seconds", 600_i64)
                    .with("interval_seconds", 300_i64)
                    .with("attribution", "Citi Bike GBFS; shared-bike inventory")
                    .with(
                        "options",
                        Dict::new()
                            .with(
                                "station_information_url",
                                "https://gbfs.citibikenyc.com/gbfs/en/station_information.json",
                            )
                            .with(
                                "station_status_url",
                                "https://gbfs.citibikenyc.com/gbfs/en/station_status.json",
                            ),
                    ),
            )];
            (region, sources)
        }
        _ => {
            let region = region("westminster", "Westminster London", 51.5007, -0.1246, 1.0);
            let sources = vec![Value::Dict(
                Dict::new()
                    .with("id", "tfl")
                    .with("kind", "tfl")
                    .with("region_id", "westminster")
                    .with("quality", 0.9)
                    .with("ttl_seconds", 600_i64)
                    .with("interval_seconds", 300_i64)
                    .with("enabled", false)
                    .with(
                        "attribution",
                        "Transport for London Unified API; line-wide service status",
                    )
                    .with(
                        "options",
                        Dict::new()
                            .with(
                                "line_ids",
                                Value::List(vec![
                                    Value::from("jubilee"),
                                    Value::from("district"),
                                    Value::from("circle"),
                                ]),
                            )
                            .with("api_key_env", "TFL_APP_KEY"),
                    ),
            )];
            (region, sources)
        }
    };
    let region_id = region.get("id").cloned().unwrap_or(Value::Null);
    sources.push(Value::Dict(
        Dict::new()
            .with("id", "weather")
            .with("kind", "open_meteo")
            .with("region_id", region_id.clone())
            .with("quality", 0.8)
            .with("ttl_seconds", 1800_i64)
            .with("interval_seconds", 900_i64)
            .with(
                "attribution",
                "Open-Meteo modelled current weather; CC BY 4.0 data",
            )
            .with("options", Dict::new()),
    ));
    sources.push(Value::Dict(
        Dict::new()
            .with("id", "ticketmaster")
            .with("kind", "ticketmaster")
            .with("region_id", region_id)
            .with("quality", 0.8)
            .with("ttl_seconds", 3600_i64)
            .with("interval_seconds", 3600_i64)
            .with("enabled", false)
            .with(
                "attribution",
                "Ticketmaster Discovery API; scheduled events, attendance unknown",
            )
            .with(
                "options",
                Dict::new()
                    .with("api_key_env", "TICKETMASTER_API_KEY")
                    .with("max_pages", 3_i64),
            ),
    ));
    Value::Dict(
        Dict::new()
            .with("schema_version", 1_i64)
            .with("database", "crowdwatch.sqlite")
            .with("regions", Value::List(vec![Value::Dict(region)]))
            .with("sources", Value::List(sources))
            .with("analysis", Dict::new().with("horizon_minutes", 15_i64)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_matches_python() {
        // [random.Random(1701).randint(-3, 3) for _ in range(12)] and so on, in Python.
        let mut rng = PyRandom::new(1701);
        let draws: Vec<i64> = (0..12).map(|_| rng.randint(-3, 3)).collect();
        assert_eq!(draws, PYTHON_DRAWS_3);
        let mut rng = PyRandom::new(1701);
        let draws: Vec<i64> = (0..12).map(|_| rng.randint(-20, 20)).collect();
        assert_eq!(draws, PYTHON_DRAWS_20);
    }

    const PYTHON_DRAWS_3: [i64; 12] = [0, 0, 0, 0, 1, -3, -2, -2, -3, -2, 1, -2];
    const PYTHON_DRAWS_20: [i64; 12] = [11, 4, 9, 10, 17, -14, -5, -10, -17, -5, 12, -10];
}
