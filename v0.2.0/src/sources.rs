// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Collectors for public aggregate data. No endpoint is allowed to invent headcounts.
//!
//! Each collector reads a provider's reply in the order 0.1.0 did, so a reply of the wrong
//! shape fails at the same field with the same Python exception name, and a good reply
//! gives the same observations, stream IDs and warnings.

use std::cmp::Ordering;
use std::collections::HashSet;

use crate::config::{number, resolve, Config};
use crate::csvread::DictReader;
use crate::error::{Fail, Result};
use crate::http::{env_secret, with_query, Env, Http, MAX_BODY};
use crate::json::{int_value, no_attribute, py_eq, Dict, Value};
use crate::model::{
    clean_url, finite, load_json, sha256_hex, unit_of, validate, Bound, Observation,
};
use crate::pyfmt::{float_from_str, int_from_str, is_space, splitlines, strip, truncate_chars};
use crate::pytime::{
    from_timestamp, parse_time, parse_time_str, parsedate_to_datetime, span_of, Instant, Span,
};
use crate::store::Store;
use crate::xml;

/// The most observations one import or collection may hold.
pub const MAX_RECORDS: usize = 20_000;

/// What one collection produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Batch {
    pub observations: Vec<Observation>,
    pub warnings: Vec<String>,
}

impl Batch {
    /// `sorted(set(warnings))`.
    fn sort_warnings(&mut self) {
        self.warnings.sort();
        self.warnings.dedup();
    }
}

// ---------------------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------------------

/// `math.radians(x)`.
fn radians(degrees: f64) -> f64 {
    degrees * (std::f64::consts::PI / 180.0)
}

/// Python's `x ** 2` for a float: the platform's `pow`, which is what CPython calls. The
/// exponent is hidden from the optimiser, which would otherwise turn this into `x * x`,
/// and that differs from `pow` in the last bit about once in a thousand.
pub fn square(x: f64) -> f64 {
    x.abs().powf(std::hint::black_box(2.0))
}

/// Great-circle distance in kilometres (haversine, mean Earth radius).
pub fn distance_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (a, b, c, d) = (radians(lat1), radians(lon1), radians(lat2), radians(lon2));
    let h = square(((c - a) / 2.0).sin()) + a.cos() * c.cos() * square(((d - b) / 2.0).sin());
    // min(1, sqrt(max(0, h)))
    let root = if h > 0.0 { h.sqrt() } else { 0.0 };
    let clipped = if root < 1.0 { root } else { 1.0 };
    6371.0088 * 2.0 * clipped.asin()
}

/// A region's number (latitude, longitude or radius), which the configuration checked.
fn region_number(region: &Dict, name: &str) -> Result<f64> {
    region.get(name).ok_or_else(|| Fail::key(name))?.py_float()
}

/// 0.1.0's `in_region(region, lat, lon)`: a valid position within the region's radius.
pub fn in_region(region: &Dict, lat: f64, lon: f64) -> Result<bool> {
    finite(
        &Value::Float(lat),
        "latitude",
        Bound(-90.0, "-90"),
        Bound(90.0, "90"),
    )?;
    finite(
        &Value::Float(lon),
        "longitude",
        Bound(-180.0, "-180"),
        Bound(180.0, "180"),
    )?;
    let distance = distance_km(
        region_number(region, "latitude")?,
        region_number(region, "longitude")?,
        lat,
        lon,
    );
    Ok(distance <= region_number(region, "radius_km")?)
}

/// 0.1.0's `stream_id(value)`: a short stable hash of the provider's identifier.
pub fn stream_id(value: &Value) -> String {
    format!("s-{}", &sha256_hex(value.py_str().as_bytes())[..24])
}

/// Geohash of a point, as Ticketmaster's `geoPoint` takes it.
pub fn geohash(lat: f64, lon: f64, precision: usize) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789bcdefghjkmnpqrstuvwxyz";
    let mut intervals = [[-180.0_f64, 180.0], [-90.0, 90.0]];
    let values = [lon, lat];
    let mut out = String::with_capacity(precision);
    let mut digit = 0usize;
    for bit in 0..precision * 5 {
        let axis = bit % 2;
        let [low, high] = intervals[axis];
        let middle = (low + high) / 2.0;
        let upper = values[axis] >= middle;
        digit = (digit << 1) | usize::from(upper);
        intervals[axis] = if upper { [middle, high] } else { [low, middle] };
        if bit % 5 == 4 {
            out.push(char::from(ALPHABET[digit]));
            digit = 0;
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// JSON helpers with Python's rules
// ---------------------------------------------------------------------------------------

/// 0.1.0's `pointer(data, path)`: an RFC 6901 JSON pointer.
pub fn pointer<'a>(data: &'a Value, path: &Value) -> Result<&'a Value> {
    if matches!(path, Value::Str(p) if p.is_empty()) {
        return Ok(data);
    }
    let path = match path {
        Value::Str(p) if p.starts_with('/') => p,
        _ => {
            return Err(Fail::input(
                "JSON mapping uses RFC 6901 pointers starting with /",
            ))
        }
    };
    let missing = || Fail::input("required mapped JSON field is missing");
    let mut current = data;
    for raw in path.split('/').skip(1) {
        let part = raw.replace("~1", "/").replace("~0", "~");
        current = match current {
            Value::List(items) => {
                let digits = int_from_str(&part).map_err(|_| missing())?;
                let position: i64 = digits.parse().map_err(|_| missing())?;
                let len = i64::try_from(items.len()).map_err(|_| missing())?;
                let index = if position < 0 {
                    position + len
                } else {
                    position
                };
                if !(0..len).contains(&index) {
                    return Err(missing());
                }
                &items[index as usize]
            }
            other => other.index(&part).map_err(|_| missing())?,
        };
    }
    Ok(current)
}

/// A URL argument as `urlsplit` takes it (see `model::url_argument`).
fn url_text(value: &Value) -> Result<String> {
    crate::model::url_argument(value)
}

/// Python's comparison of a number with an integer; `None` where it is undefined (NaN).
/// Anything but a number is Python's TypeError.
fn compare_int(value: &Value, rhs: i64, operator: &str) -> Result<Option<Ordering>> {
    match value {
        Value::Int(i) => Ok(Some(i.cmp(&rhs))),
        Value::Bool(b) => Ok(Some(i64::from(*b).cmp(&rhs))),
        Value::BigInt(digits) => Ok(Some(if digits.starts_with('-') {
            Ordering::Less
        } else {
            Ordering::Greater
        })),
        Value::Float(f) => Ok(f.partial_cmp(&(rhs as f64))),
        other => Err(Fail::type_error(format!(
            "'{operator}' not supported between instances of 'int' and '{}'",
            other.type_name()
        ))),
    }
}

/// `a <op> b` for two Python integers (`int()` results).
fn int_order(a: &Value, b: &Value) -> Ordering {
    fn parts(v: &Value) -> (bool, String) {
        match v {
            Value::Int(i) => (*i < 0, i.unsigned_abs().to_string()),
            Value::BigInt(s) => match s.strip_prefix('-') {
                Some(rest) => (true, rest.to_owned()),
                None => (false, s.clone()),
            },
            Value::Bool(b) => (false, u8::from(*b).to_string()),
            _ => (false, "0".to_owned()),
        }
    }
    let (neg_a, mag_a) = parts(a);
    let (neg_b, mag_b) = parts(b);
    let magnitude = mag_a
        .len()
        .cmp(&mag_b.len())
        .then_with(|| mag_a.cmp(&mag_b));
    match (neg_a, neg_b) {
        (false, false) => magnitude,
        (true, true) => magnitude.reverse(),
        (true, false) => {
            if mag_a == "0" && mag_b == "0" {
                Ordering::Equal
            } else {
                Ordering::Less
            }
        }
        (false, true) => {
            if mag_a == "0" && mag_b == "0" {
                Ordering::Equal
            } else {
                Ordering::Greater
            }
        }
    }
}

/// `value in {"cancelled", "postponed", "rescheduled"}`: hashable values only.
fn in_string_set(value: &Value, set: &[&str]) -> Result<bool> {
    match value {
        Value::List(_) | Value::Dict(_) => Err(Fail::type_error(format!(
            "unhashable type: '{}'",
            value.type_name()
        ))),
        Value::Str(s) => Ok(set.contains(&s.as_str())),
        _ => Ok(false),
    }
}

/// Python's `str.lower()` on a value that must be text.
fn lower(value: &Value) -> Result<String> {
    match value {
        Value::Str(s) => Ok(s.to_lowercase()),
        other => Err(no_attribute(other, "lower")),
    }
}

// ---------------------------------------------------------------------------------------
// Imports
// ---------------------------------------------------------------------------------------

/// 0.1.0's `canonical_records(body, kind)`: observations from canonical JSON, JSON lines
/// or CSV, at most 5 MiB and 20,000 records.
pub fn canonical_records(body: &[u8], kind: &str) -> Result<Vec<Observation>> {
    if body.len() > MAX_BODY {
        return Err(Fail::input(
            "import exceeds 5 MiB; split it into smaller files",
        ));
    }
    let raw = body.strip_prefix(b"\xef\xbb\xbf").unwrap_or(body);
    let text = std::str::from_utf8(raw).map_err(|_| Fail::input("imports must be UTF-8"))?;
    let items = if kind == "canonical_csv" {
        csv_items(text)?
    } else {
        match load_json(text) {
            Ok(Value::List(items)) => Value::List(items),
            Ok(Value::Dict(mut d)) if d.contains_key("observations") => {
                d.remove("observations").unwrap_or(Value::Null)
            }
            Ok(other) => Value::List(vec![other]),
            // Not one JSON document: JSON lines, one record per non-blank line.
            Err(Fail::Input(_)) => {
                let mut items = Vec::new();
                for line in splitlines(text) {
                    if !strip(line).is_empty() {
                        items.push(load_json(line)?);
                    }
                }
                Value::List(items)
            }
            Err(other) => return Err(other),
        }
    };
    let items = match items {
        Value::List(items) if items.len() <= MAX_RECORDS => items,
        _ => {
            return Err(Fail::input(
                "import must contain at most 20000 observations",
            ))
        }
    };
    items.iter().map(Observation::from_value).collect()
}

/// The CSV rows of an import, numbers and details converted as 0.1.0 converted them.
fn csv_items(text: &str) -> Result<Value> {
    let mut reader = DictReader::new(text)?;
    let unique = reader.fieldnames().is_some_and(|names| {
        let mut seen = HashSet::new();
        names.iter().all(|name| seen.insert(name))
    });
    if !unique {
        return Err(Fail::input("CSV requires unique column names"));
    }
    let mut items = Vec::new();
    while let Some(row) = reader.next_row() {
        let row = row?;
        if row.too_long {
            return Err(Fail::input("CSV row has more cells than its header"));
        }
        let mut item = Dict::new();
        for (name, cell) in row.cells {
            if let Some(text) = cell.filter(|t| !t.is_empty()) {
                item.insert(name, Value::Str(text));
            }
        }
        convert_csv_item(&mut item).map_err(|_| Fail::input("CSV has an invalid numeric field"))?;
        items.push(Value::Dict(item));
    }
    Ok(Value::List(items))
}

/// Numbers from text, and details from JSON. Every failure here is Python's ValueError.
fn convert_csv_item(item: &mut Dict) -> Result<()> {
    for key in ["value", "quality", "latitude", "longitude"] {
        if let Some(Value::Str(text)) = item.get(key) {
            let number = float_from_str(text)?;
            item.insert(key, Value::Float(number));
        }
    }
    if let Some(Value::Str(text)) = item.get("period_seconds") {
        let digits = int_from_str(text)?;
        item.insert("period_seconds", int_value(digits));
    }
    if let Some(Value::Str(text)) = item.get("details") {
        let details = load_json(text)?;
        item.insert("details", details);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Collectors
// ---------------------------------------------------------------------------------------

/// The optional fields of an observation a collector makes.
struct Extra {
    period_seconds: Value,
    latitude: Value,
    longitude: Value,
    provenance_url: Value,
    label: Value,
    details: Value,
}

impl Default for Extra {
    fn default() -> Extra {
        Extra {
            period_seconds: Value::Int(0),
            latitude: Value::Null,
            longitude: Value::Null,
            provenance_url: Value::from(""),
            label: Value::from(""),
            details: Value::Dict(Dict::new()),
        }
    }
}

/// A source's option.
fn option<'a>(source: &'a Dict, name: &str) -> Option<&'a Value> {
    source
        .get("options")
        .and_then(Value::as_dict)
        .and_then(|o| o.get(name))
}

fn options(source: &Dict) -> Value {
    source
        .get("options")
        .cloned()
        .unwrap_or_else(|| Value::Dict(Dict::new()))
}

/// Runs the configured sources' collectors.
pub struct Collector<'a> {
    pub config: &'a Config,
    pub client: &'a mut dyn Http,
    pub env: &'a dyn Env,
}

const ATOM: &str = "{http://www.w3.org/2005/Atom}";

impl Collector<'_> {
    fn region(&self, source: &Dict) -> Result<&Dict> {
        let id = source
            .get("region_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        self.config.region(id).ok_or_else(|| Fail::key(id))
    }

    /// 0.1.0's `make(...)`: a validated observation from this source.
    #[allow(clippy::too_many_arguments)]
    fn make(
        &self,
        source: &Dict,
        now: Instant,
        metric: &Value,
        value: Value,
        stream: Value,
        observed: Option<Instant>,
        extra: Extra,
    ) -> Result<Observation> {
        let observed = observed.unwrap_or(now);
        let unit = match metric {
            Value::Str(m) => unit_of(m).ok_or_else(|| Fail::key(m))?,
            Value::List(_) | Value::Dict(_) => {
                return Err(Fail::type_error(format!(
                    "unhashable type: '{}'",
                    metric.type_name()
                )))
            }
            other => return Err(Fail::py("KeyError", other.py_repr())),
        };
        let ttl = span_of(number(source, "ttl_seconds"), 1_000_000)?;
        let until = observed.add(ttl)?;
        let field = |name: &str| source.get(name).cloned().unwrap_or(Value::Null);
        let fields = Dict::new()
            .with("source_id", field("id"))
            .with("stream_id", stream)
            .with("region_id", field("region_id"))
            .with("metric", metric.clone())
            .with("value", value)
            .with("unit", unit)
            .with("observed_at", observed.stamp())
            .with("retrieved_at", now.stamp())
            .with("valid_until", until.stamp())
            .with("period_seconds", extra.period_seconds)
            .with("quality", field("quality"))
            .with("provenance_url", extra.provenance_url)
            .with("label", extra.label)
            .with("latitude", extra.latitude)
            .with("longitude", extra.longitude)
            .with("details", extra.details);
        validate(&fields)
    }

    /// Collect one source. More than 20,000 observations is refused.
    pub fn collect(&mut self, source: &Dict, now: Instant) -> Result<Batch> {
        let kind = source.get("kind").and_then(Value::as_str).unwrap_or("");
        let batch = match kind {
            "canonical_json" | "canonical_csv" => self.canonical(source, now)?,
            "mapped_json" => self.mapped_json(source, now)?,
            "open_meteo" => self.open_meteo(source, now)?,
            "tfl" => self.tfl(source, now)?,
            "gbfs" => self.gbfs(source, now)?,
            "ticketmaster" => self.ticketmaster(source, now)?,
            "rss" => self.rss(source, now)?,
            "melbourne" => self.melbourne(source, now)?,
            other => {
                return Err(no_attribute(
                    &Value::from("Collector"),
                    &format!("_{other}"),
                ))
            }
        };
        if batch.observations.len() > MAX_RECORDS {
            return Err(Fail::input("collector exceeded 20000 observations"));
        }
        Ok(batch)
    }

    /// A file or feed body: exactly one of `options.path` and `options.url`.
    fn body(&mut self, source: &Dict) -> Result<Vec<u8>> {
        let path = option(source, "path");
        let url = option(source, "url");
        if path.is_some() == url.is_some() {
            return Err(Fail::input(
                "file/feed source requires exactly one of options.path or options.url",
            ));
        }
        if let Some(path) = path {
            let Value::Str(relative) = path else {
                return Err(Fail::type_error(format!(
                    "unsupported operand type(s) for /: 'PosixPath' and '{}'",
                    path.type_name()
                )));
            };
            let folder = self
                .config
                .path
                .parent()
                .unwrap_or(std::path::Path::new(""));
            let file = resolve(&folder.join(relative));
            return read_limited(&file);
        }
        let mut headers = Vec::new();
        if let Some(name) = option(source, "bearer_env") {
            let secret = env_secret(self.env, name, true)?.unwrap_or_default();
            headers.push(("Authorization".to_owned(), format!("Bearer {secret}")));
        }
        let url = url_text(url.unwrap_or(&Value::Null))?;
        self.client.get(&url, &headers)
    }

    fn json(&mut self, url: &Value) -> Result<Value> {
        let url = url_text(url)?;
        self.client.json(&url, &[])
    }

    fn canonical(&mut self, source: &Dict, now: Instant) -> Result<Batch> {
        let kind = source.get("kind").and_then(Value::as_str).unwrap_or("");
        let rows = canonical_records(&self.body(source)?, kind)?;
        let id = source.get("id").and_then(Value::as_str).unwrap_or("");
        let region = source
            .get("region_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        if rows
            .iter()
            .any(|o| o.source_id != id || o.region_id != region)
        {
            return Err(Fail::input(
                "canonical feed source_id/region_id must match its configured source",
            ));
        }
        // Collection records availability now; import/replay is a separate explicit command.
        let ceiling = number(source, "quality").as_f64();
        let mut observations = Vec::with_capacity(rows.len());
        for mut o in rows {
            o.retrieved_at = now.stamp();
            // min(o.quality, ceiling): the first unless the second is smaller.
            if ceiling < o.quality {
                o.quality = ceiling;
            }
            observations.push(o.revalidated()?);
        }
        Ok(Batch {
            observations,
            warnings: Vec::new(),
        })
    }

    fn mapped_json(&mut self, source: &Dict, now: Instant) -> Result<Batch> {
        let opts = options(source);
        let raw = load_json_bytes_strict(&self.body(source)?)?;
        let empty = Value::from("");
        let items_pointer = opts.get_or("items_pointer", &empty)?;
        let rows = match pointer(&raw, items_pointer)? {
            Value::List(rows) => rows,
            _ => return Err(Fail::input("items_pointer must select an array")),
        };
        if rows.len() > MAX_RECORDS {
            return Err(Fail::input("mapped feed has too many records"));
        }
        let fields = opts.index("fields")?;
        let metric = opts.index("metric")?;
        let region = self.region(source)?;
        let mut out = Vec::new();
        let zero = Value::Int(0);
        for row in rows {
            let (mut lat, mut lon) = (Value::Null, Value::Null);
            let located = fields.contains(&Value::from("latitude"))?
                && fields.contains(&Value::from("longitude"))?;
            if located {
                let latitude = pointer(row, fields.index("latitude")?)?.py_float()?;
                let longitude = pointer(row, fields.index("longitude")?)?.py_float()?;
                if !in_region(region, latitude, longitude)? {
                    continue;
                }
                lat = Value::Float(latitude);
                lon = Value::Float(longitude);
            }
            let value = pointer(row, fields.index("value")?)?.py_float()?;
            let stream = stream_id(pointer(row, fields.index("stream_id")?)?);
            let observed = parse_time(pointer(row, fields.index("observed_at")?)?)?;
            let period_seconds = opts.get_or("period_seconds", &zero)?.clone();
            let provenance_url = clean_url(opts.get_or("url", &empty)?)?;
            let label = if fields.contains(&Value::from("label"))? {
                truncate_chars(&pointer(row, fields.index("label")?)?.py_str(), 500)
            } else {
                "Mapped aggregate".to_owned()
            };
            let scope = if lat.is_null() {
                "configured area"
            } else {
                "point"
            };
            out.push(
                self.make(
                    source,
                    now,
                    metric,
                    Value::Float(value),
                    Value::Str(stream),
                    Some(observed),
                    Extra {
                        period_seconds,
                        latitude: lat,
                        longitude: lon,
                        provenance_url: Value::Str(provenance_url),
                        label: Value::Str(label),
                        details: Value::Dict(
                            Dict::new()
                                .with("mapping", "explicit")
                                .with("spatial_scope", scope),
                        ),
                    },
                )?,
            );
        }
        let mut warnings = Vec::new();
        if let Some(total_pointer) = opts.get("total_pointer")? {
            let total = pointer(&raw, total_pointer)?.py_int()?;
            let count = i64::try_from(rows.len()).unwrap_or(i64::MAX);
            if compare_int(&total, count, ">")? == Some(Ordering::Greater) {
                warnings.push(
                    "Feed is paginated: this configured endpoint supplied only a partial page."
                        .to_owned(),
                );
            }
        }
        Ok(Batch {
            observations: out,
            warnings,
        })
    }

    fn open_meteo(&mut self, source: &Dict, now: Instant) -> Result<Batch> {
        let region = self.region(source)?.clone();
        let opts = options(source);
        let default_base = Value::from("https://api.open-meteo.com/v1/forecast");
        let base = opts.get_or("url", &default_base)?.clone();
        let latitude = region.get("latitude").cloned().unwrap_or(Value::Null);
        let longitude = region.get("longitude").cloned().unwrap_or(Value::Null);
        let mut params: Vec<(&str, Value)> = vec![
            ("latitude", latitude.clone()),
            ("longitude", longitude.clone()),
            (
                "current",
                Value::from("temperature_2m,precipitation,wind_speed_10m"),
            ),
            ("timezone", Value::from("UTC")),
            ("timeformat", Value::from("unixtime")),
            ("wind_speed_unit", Value::from("kmh")),
        ];
        if opts.get("api_key_env")?.is_some_and(Value::truthy) {
            let name = opts.index("api_key_env")?;
            let key = env_secret(self.env, name, true)?;
            params.push(("apikey", Value::from(key)));
        }
        let url = with_query(&url_text(&base)?, &params)?;
        let payload = self.client.json(&url, &[])?;
        let current = payload.index("current")?;
        let units = payload.index("current_units")?;
        let observed = from_timestamp(current.index("time")?)?;
        let expected = [
            ("temperature_2m", "temperature", "\u{b0}C"),
            ("precipitation", "precipitation", "mm"),
            ("wind_speed_10m", "wind", "km/h"),
        ];
        let mut batch = Batch::default();
        for (key, metric, unit) in expected {
            let given = units.get(key)?;
            if !given.is_some_and(|u| py_eq(u, &Value::from(unit))) {
                return Err(Fail::input("Open-Meteo response used unexpected units"));
            }
            if current.get(key)?.is_none_or(Value::is_null) {
                batch.warnings.push(format!("Weather field missing: {key}"));
                continue;
            }
            let period_seconds = if metric == "precipitation" {
                current.index("interval")?.py_int()?
            } else {
                Value::Int(0)
            };
            let value = current.index(key)?.clone();
            batch.observations.push(self.make(
                source,
                now,
                &Value::from(metric),
                value,
                Value::from("weather"),
                Some(observed),
                Extra {
                    period_seconds,
                    latitude: latitude.clone(),
                    longitude: longitude.clone(),
                    provenance_url: Value::Str(clean_url(&base)?),
                    label: Value::from("Open-Meteo modelled current weather"),
                    details: Value::Dict(Dict::new().with("measurement_type", "weather model")),
                },
            )?);
        }
        Ok(batch)
    }

    fn tfl(&mut self, source: &Dict, now: Instant) -> Result<Batch> {
        let opts = options(source);
        let none = Value::List(Vec::new());
        let lines = opts.get_or("line_ids", &none)?.clone();
        let refused = || Fail::input("TfL requires explicit line_ids relevant to the area");
        if !lines.truthy() {
            return Err(refused());
        }
        let names = lines.py_iter()?;
        let mut texts = Vec::with_capacity(names.len());
        for name in &names {
            match name {
                Value::Str(text) if crate::pyfmt::isalnum(&text.replace('-', "")) => {
                    texts.push(text.clone())
                }
                _ => return Err(refused()),
            }
        }
        let base = format!("https://api.tfl.gov.uk/Line/{}/Status", texts.join(","));
        let app_key = env_secret(self.env, opts.get_or("api_key_env", &Value::Null)?, false)?;
        let payload = self.client.json(
            &with_query(&base, &[("app_key", Value::from(app_key))])?,
            &[],
        )?;
        const LEVELS: [(&str, f64); 12] = [
            ("good service", 0.0),
            ("minor delays", 0.25),
            ("reduced service", 0.4),
            ("severe delays", 0.7),
            ("part closure", 0.7),
            ("part suspended", 0.7),
            ("planned closure", 0.8),
            ("suspended", 1.0),
            ("closed", 1.0),
            ("service closed", 1.0),
            ("bus service", 0.6),
            ("special service", 0.5),
        ];
        let level = |text: &str| {
            LEVELS
                .iter()
                .find(|(name, _)| *name == text)
                .map(|(_, v)| *v)
        };
        let mut batch = Batch::default();
        let no_statuses = Value::List(Vec::new());
        for line in payload.py_iter()? {
            let id = line.index("id")?.clone();
            if !lines.contains(&id)? {
                continue;
            }
            let mut descriptions = Vec::new();
            for status in line.get_or("lineStatuses", &no_statuses)?.py_iter()? {
                descriptions.push(lower(status.index("statusSeverityDescription")?)?);
            }
            if descriptions.is_empty() || descriptions.iter().any(|d| level(d).is_none()) {
                batch
                    .warnings
                    .push(format!("Unmapped TfL status on {}; omitted.", id.py_str()));
                continue;
            }
            let severity = descriptions
                .iter()
                .filter_map(|d| level(d))
                .fold(f64::NEG_INFINITY, f64::max);
            let label = truncate_chars(&line.get_or("name", &id)?.py_str(), 500);
            batch.observations.push(self.make(
                source,
                now,
                &Value::from("transit_disruption"),
                Value::Float(severity),
                id,
                None,
                Extra {
                    provenance_url: Value::from(base.as_str()),
                    label: Value::Str(label),
                    details: Value::Dict(
                        Dict::new()
                            .with(
                                "statuses",
                                Value::List(descriptions.into_iter().map(Value::Str).collect()),
                            )
                            .with("scope", "whole line; not local crowd measurement")
                            .with(
                                "scale",
                                "CrowdWatch descriptive severity mapping, not a TfL numeric scale",
                            ),
                    ),
                    ..Extra::default()
                },
            )?);
        }
        if batch.observations.is_empty() {
            batch
                .warnings
                .push("No usable statuses for configured lines.".to_owned());
        }
        Ok(batch)
    }

    fn gbfs(&mut self, source: &Dict, now: Instant) -> Result<Batch> {
        let opts = options(source);
        let info = self.json(opts.index("station_information_url")?)?;
        let status = self.json(opts.index("station_status_url")?)?;
        let region = self.region(source)?.clone();
        let mut stations: Vec<(String, Value)> = Vec::new();
        for x in info.index("data")?.index("stations")?.py_iter()? {
            let lat = x.index("lat")?.py_float()?;
            let lon = x.index("lon")?.py_float()?;
            if in_region(&region, lat, lon)? {
                let key = x.index("station_id")?.py_str();
                match stations.iter_mut().find(|(k, _)| *k == key) {
                    Some(slot) => slot.1 = x,
                    None => stations.push((key, x)),
                }
            }
        }
        let mut batch = Batch::default();
        for row in status.index("data")?.index("stations")?.py_iter()? {
            let station_key = row.index("station_id")?.py_str();
            let Some(loc) = stations
                .iter()
                .find(|(k, _)| *k == station_key)
                .map(|(_, v)| v.clone())
            else {
                continue;
            };
            if !loc.truthy() || !row.get("is_installed")?.is_some_and(Value::truthy) {
                continue;
            }
            let t = row.index("last_reported")?;
            let observed = match t {
                Value::Int(_) | Value::BigInt(_) | Value::Float(_) | Value::Bool(_) => {
                    from_timestamp(t)?
                }
                other => parse_time(other)?,
            };
            let count_field = if row.contains(&Value::from("num_vehicles_available"))? {
                "num_vehicles_available"
            } else {
                "num_bikes_available"
            };
            for (metric, field) in [
                ("bikes_available", count_field),
                ("docks_available", "num_docks_available"),
            ] {
                if !row.contains(&Value::from(field))? {
                    batch
                        .warnings
                        .push("GBFS station omitted a vehicle/dock availability field.".to_owned());
                    continue;
                }
                let station_name = Value::Str(row.index("station_id")?.py_str());
                let mut name = loc.get_or("name", &station_name)?.clone();
                if let Value::List(names) = &name {
                    name = match names.first() {
                        Some(first) => first.get_or("text", &Value::from("Station"))?.clone(),
                        None => Value::from("Station"),
                    };
                }
                let value = row.index(field)?.clone();
                let stream = stream_id(row.index("station_id")?);
                let latitude = loc.index("lat")?.py_float()?;
                let longitude = loc.index("lon")?.py_float()?;
                let label = truncate_chars(&name.py_str(), 500);
                let provenance_url = clean_url(opts.index("station_status_url")?)?;
                let renting = row.get("is_renting")?.is_some_and(Value::truthy);
                let returning = row.get("is_returning")?.is_some_and(Value::truthy);
                batch.observations.push(
                    self.make(
                        source,
                        now,
                        &Value::from(metric),
                        value,
                        Value::Str(stream),
                        Some(observed),
                        Extra {
                            latitude: Value::Float(latitude),
                            longitude: Value::Float(longitude),
                            label: Value::Str(label),
                            provenance_url: Value::Str(provenance_url),
                            details: Value::Dict(
                                Dict::new()
                                    .with("is_renting", renting)
                                    .with("is_returning", returning)
                                    .with(
                                        "interpretation",
                                        "inventory; changes may include operator rebalancing",
                                    ),
                            ),
                            ..Extra::default()
                        },
                    )?,
                );
            }
        }
        Ok(batch)
    }

    fn ticketmaster(&mut self, source: &Dict, now: Instant) -> Result<Batch> {
        let opts = options(source);
        let region = self.region(source)?.clone();
        let default_env = Value::from("TICKETMASTER_API_KEY");
        let key = env_secret(self.env, opts.get_or("api_key_env", &default_env)?, true)?;
        let base = "https://app.ticketmaster.com/discovery/v2/events.json";
        let default_pages = Value::Int(5);
        let max_pages = opts.get_or("max_pages", &default_pages)?.py_int()?;
        let max_pages = match max_pages {
            Value::Int(n) if (1..=5).contains(&n) => n,
            _ => return Err(Fail::input("Ticketmaster max_pages must be 1–5")),
        };
        let latitude = region.get("latitude").cloned().unwrap_or(Value::Null);
        let longitude = region.get("longitude").cloned().unwrap_or(Value::Null);
        let geo = geohash(latitude.py_float()?, longitude.py_float()?, 8);
        let start = now.sub(Span::hours(6))?.strftime_seconds();
        let end = now.add(Span::hours(24))?.strftime_seconds();
        let mut batch = Batch::default();
        let empty_dict = Value::Dict(Dict::new());
        let empty_list = Value::List(Vec::new());
        let mut pages = Value::Int(1);
        for page in 0..max_pages {
            let params: Vec<(&str, Value)> = vec![
                ("apikey", Value::from(key.clone())),
                ("geoPoint", Value::from(geo.as_str())),
                (
                    "radius",
                    region.get("radius_km").cloned().unwrap_or(Value::Null),
                ),
                ("unit", Value::from("km")),
                ("size", Value::Int(200)),
                ("page", Value::Int(page)),
                ("startDateTime", Value::from(start.as_str())),
                ("endDateTime", Value::from(end.as_str())),
            ];
            let data = self.client.json(&with_query(base, &params)?, &[])?;
            let events = data
                .get_or("_embedded", &empty_dict)?
                .get_or("events", &empty_list)?
                .py_iter()?;
            for event in events {
                let dates = event.get_or("dates", &empty_dict)?;
                let code = dates.get_or("status", &empty_dict)?.get("code")?;
                if in_string_set(
                    code.unwrap_or(&Value::Null),
                    &["cancelled", "postponed", "rescheduled"],
                )? {
                    continue;
                }
                let date = dates
                    .get_or("start", &empty_dict)?
                    .get("dateTime")?
                    .cloned()
                    .unwrap_or(Value::Null);
                let venues = event
                    .get_or("_embedded", &empty_dict)?
                    .get_or("venues", &empty_list)?
                    .clone();
                let unusable = !date.truthy()
                    || !venues.truthy()
                    || !venues.index_at(0)?.contains(&Value::from("location"))?;
                if unusable {
                    batch.warnings.push(
                        "An event lacked a definite time or venue location; omitted.".to_owned(),
                    );
                    continue;
                }
                let first = venues.index_at(0)?;
                let loc = first.index("location")?;
                let lat = loc.index("latitude")?.py_float()?;
                let lon = loc.index("longitude")?.py_float()?;
                if !in_region(&region, lat, lon)? {
                    continue;
                }
                let starts = parse_time(&date)?;
                let end_value = dates
                    .get_or("end", &empty_dict)?
                    .get("dateTime")?
                    .cloned()
                    .unwrap_or(Value::Null);
                let ends = if end_value.truthy() {
                    parse_time(&end_value)?
                } else {
                    starts.add(Span::hours(4))?
                };
                if ends <= now {
                    continue;
                }
                let stream = stream_id(event.index("id")?);
                let label = truncate_chars(&event.index("name")?.py_str(), 500);
                let base_value = Value::from(base);
                let provenance_url = clean_url(event.get_or("url", &base_value)?)?;
                batch.observations.push(
                    self.make(
                        source,
                        now,
                        &Value::from("scheduled_event"),
                        Value::Int(1),
                        Value::Str(stream),
                        None,
                        Extra {
                            latitude: Value::Float(lat),
                            longitude: Value::Float(lon),
                            label: Value::Str(label),
                            provenance_url: Value::Str(provenance_url),
                            details: Value::Dict(
                                Dict::new()
                                    .with("starts_at", starts.stamp())
                                    .with("ends_at", ends.stamp())
                                    .with("end_is_assumed", !end_value.truthy())
                                    .with("attendance", "unknown"),
                            ),
                            ..Extra::default()
                        },
                    )?,
                );
            }
            pages = data
                .get_or("page", &empty_dict)?
                .get_or("totalPages", &Value::Int(1))?
                .clone();
            // page + 1 >= pages
            if matches!(
                compare_int(&pages, page + 1, ">=")?,
                Some(Ordering::Less | Ordering::Equal)
            ) {
                break;
            }
        }
        // pages > max_pages
        if compare_int(&pages, max_pages, ">")? == Some(Ordering::Greater) {
            batch
                .warnings
                .push("Ticketmaster result cap reached; coverage is partial.".to_owned());
        }
        Ok(batch)
    }

    fn rss(&mut self, source: &Dict, now: Instant) -> Result<Batch> {
        let body = self.body(source)?;
        let upper = body.to_ascii_uppercase();
        let has = |needle: &[u8]| upper.windows(needle.len()).any(|w| w == needle);
        if has(b"<!DOCTYPE") || has(b"<!ENTITY") {
            return Err(Fail::input(
                "DTD/entity declarations are not allowed in feeds",
            ));
        }
        let document = xml::fromstring(&body)?;
        let opts = options(source);
        let region = self.region(source)?.clone();
        let mut batch = Batch::default();
        let mut nodes = document.findall_descendants(0, "item");
        nodes.extend(document.findall_descendants(0, &format!("{ATOM}entry")));
        if nodes.len() > 2000 {
            return Err(Fail::input("RSS/Atom feed exceeds 2000 items"));
        }
        for item in nodes {
            let val = |tag: &str| -> String {
                document
                    .find(item, tag)
                    .map(|x| strip(x.text.as_deref().unwrap_or("")).to_owned())
                    .unwrap_or_default()
            };
            let or = |first: String, second: &dyn Fn() -> String| {
                if first.is_empty() {
                    second()
                } else {
                    first
                }
            };
            let title = or(val("title"), &|| val(&format!("{ATOM}title")));
            let mut link = val("link");
            if let Some(atom_link) = document.find(item, &format!("{ATOM}link")) {
                if let Some(href) = atom_link.get("href") {
                    link = href.to_owned();
                }
            }
            let date = or(val("pubDate"), &|| val(&format!("{ATOM}updated")));
            if date.is_empty() {
                batch.warnings.push("Undated feed item omitted.".to_owned());
                continue;
            }
            let observed = match parse_time_str(&date) {
                Ok(t) => t,
                Err(Fail::Input(_)) => match parsedate_to_datetime(&date) {
                    // A naive time is refused by parse_time (InputError, a ValueError).
                    Ok(when) => match when.to_utc()? {
                        Some(t) => t,
                        None => {
                            batch
                                .warnings
                                .push("Feed item with an invalid date omitted.".to_owned());
                            continue;
                        }
                    },
                    Err(err) if err.is_value_error() || err.kind() == "TypeError" => {
                        batch
                            .warnings
                            .push("Feed item with an invalid date omitted.".to_owned());
                        continue;
                    }
                    Err(err) => return Err(err),
                },
                Err(err) => return Err(err),
            };
            let (mut lat, mut lon) = (Value::Null, Value::Null);
            let point = val("{http://www.georss.org/georss}point");
            if !point.is_empty() {
                let parts: Vec<&str> = point.split(is_space).filter(|p| !p.is_empty()).collect();
                let mut numbers = Vec::with_capacity(parts.len().min(3));
                // `lat, lon = map(float, parts)` converts up to three before it complains.
                for part in parts.iter().take(3) {
                    numbers.push(float_from_str(part)?);
                }
                if parts.len() != 2 {
                    return Err(Fail::value_error(if parts.len() < 2 {
                        format!(
                            "not enough values to unpack (expected 2, got {})",
                            parts.len()
                        )
                    } else {
                        "too many values to unpack (expected 2)".to_owned()
                    }));
                }
                if !in_region(&region, numbers[0], numbers[1])? {
                    continue;
                }
                lat = Value::Float(numbers[0]);
                lon = Value::Float(numbers[1]);
            } else if !opts.get("area_scoped")?.is_some_and(Value::truthy) {
                batch.warnings.push(
                    "Unlocated feed item omitted; set area_scoped only for a local feed."
                        .to_owned(),
                );
                continue;
            }
            if observed > now {
                batch
                    .warnings
                    .push("Future-dated feed item omitted.".to_owned());
                continue;
            }
            let mut guid = or(val("guid"), &|| val(&format!("{ATOM}id")));
            if guid.is_empty() {
                guid = if link.is_empty() {
                    title.clone()
                } else {
                    link.clone()
                };
            }
            batch.observations.push(self.make(
                source,
                now,
                &Value::from("report"),
                Value::Int(1),
                Value::Str(stream_id(&Value::Str(guid))),
                Some(observed),
                Extra {
                    latitude: lat,
                    longitude: lon,
                    label: Value::Str(truncate_chars(&title, 500)),
                    provenance_url: Value::Str(clean_url(&Value::Str(link))?),
                    details: Value::Dict(Dict::new().with(
                        "interpretation",
                        "reported context only; not verified attendance or a headcount",
                    )),
                    ..Extra::default()
                },
            )?);
        }
        batch.sort_warnings();
        Ok(batch)
    }

    /// Join the City of Melbourne's minute counts to its sensor-location catalogue.
    fn melbourne(&mut self, source: &Dict, now: Instant) -> Result<Batch> {
        const ROOT: &str = "https://data.melbourne.vic.gov.au/api/explore/v2.1/catalog/datasets/";
        let location_url = format!("{ROOT}pedestrian-counting-system-sensor-locations/records");
        let counts_url =
            format!("{ROOT}pedestrian-counting-system-past-hour-counts-per-minute/records");
        let opts = options(source);
        let region = self.region(source)?.clone();
        let empty_list = Value::List(Vec::new());
        let selected = opts.get_or("sensor_ids", &empty_list)?.clone();
        let valid_selection = match &selected {
            Value::List(ids) => ids.iter().all(|x| match x {
                Value::Int(i) => *i >= 1,
                Value::BigInt(digits) => !digits.starts_with('-'),
                _ => false,
            }),
            _ => false,
        };
        if !valid_selection {
            return Err(Fail::input(
                "Melbourne sensor_ids must be an array of positive integers",
            ));
        }
        let selected = selected.as_list().cloned().unwrap_or_default();
        let max_pages = match opts.get("max_pages")? {
            None => 5,
            Some(Value::Int(n)) if (1..=10).contains(n) => *n,
            Some(_) => return Err(Fail::input("Melbourne max_pages must be 1–10")),
        };
        // Sensors by location ID (a Python int), in first-seen order.
        let mut sensors: Vec<(Value, Value)> = Vec::new();
        let mut finished = false;
        for page in 0..10_i64 {
            let url = with_query(
                &location_url,
                &[
                    ("limit", Value::Int(100)),
                    ("offset", Value::Int(page * 100)),
                    ("order_by", Value::from("location_id")),
                ],
            )?;
            let data = self.client.json(&url, &[])?;
            for loc in data.index("results")?.py_iter()? {
                let lid = loc.index("location_id")?.py_int()?;
                let active = py_eq(loc.index("status")?, &Value::from("A"));
                let chosen = selected.is_empty() || selected.iter().any(|s| py_eq(s, &lid));
                if active && chosen {
                    let lat = loc.index("latitude")?.py_float()?;
                    let lon = loc.index("longitude")?.py_float()?;
                    if in_region(&region, lat, lon)? {
                        match sensors.iter_mut().find(|(k, _)| py_eq(k, &lid)) {
                            Some(slot) => slot.1 = loc,
                            None => sensors.push((lid, loc)),
                        }
                    }
                }
            }
            // (page + 1) * 100 >= total_count
            if matches!(
                compare_int(data.index("total_count")?, (page + 1) * 100, ">=")?,
                Some(Ordering::Less | Ordering::Equal)
            ) {
                finished = true;
                break;
            }
        }
        if !finished {
            return Err(Fail::input(
                "Melbourne location catalogue exceeded its page bound",
            ));
        }
        let mut batch = Batch::default();
        if sensors.is_empty() {
            batch
                .warnings
                .push("No active Melbourne pedestrian sensors in the configured area.".to_owned());
            return Ok(batch);
        }
        let mut ids: Vec<&Value> = sensors.iter().map(|(k, _)| k).collect();
        ids.sort_by(|a, b| int_order(a, b));
        let names: Vec<String> = ids.iter().map(|id| id.py_str()).collect();
        let selection = format!("location_id in ({})", names.join(","));
        let mut finished = false;
        for page in 0..max_pages {
            let url = with_query(
                &counts_url,
                &[
                    ("limit", Value::Int(100)),
                    ("offset", Value::Int(page * 100)),
                    ("order_by", Value::from("sensing_datetime DESC,location_id")),
                    ("where", Value::from(selection.as_str())),
                ],
            )?;
            let data = self.client.json(&url, &[])?;
            for row in data.index("results")?.py_iter()? {
                let location = row.index("location_id")?;
                let lid = location.py_int()?;
                let Some(sensor) = sensors
                    .iter()
                    .find(|(k, _)| py_eq(k, &lid))
                    .map(|(_, v)| v.clone())
                else {
                    return Err(Fail::input(
                        "Melbourne returned a sensor outside the requested selection",
                    ));
                };
                let observed = parse_time(row.index("sensing_datetime")?)?;
                if observed > now {
                    batch
                        .warnings
                        .push("Future-dated Melbourne count omitted.".to_owned());
                    continue;
                }
                if row.get("total_of_directions")?.is_none_or(Value::is_null) {
                    batch
                        .warnings
                        .push("Missing Melbourne count omitted; not treated as zero.".to_owned());
                    continue;
                }
                let value = row.index("total_of_directions")?.clone();
                let stream = format!("sensor-{}", location.py_str());
                let latitude = sensor.index("latitude")?.py_float()?;
                let longitude = sensor.index("longitude")?.py_float()?;
                let label = truncate_chars(&sensor.index("sensor_description")?.py_str(), 500);
                let direction = |name: &str| -> Result<Value> {
                    Ok(row.get(name)?.cloned().unwrap_or(Value::Null))
                };
                let details = Dict::new()
                    .with("direction_1", direction("direction_1")?)
                    .with("direction_2", direction("direction_2")?)
                    .with(
                        "scope",
                        "sensor passage count; repeat people may be counted",
                    );
                batch.observations.push(self.make(
                    source,
                    now,
                    &Value::from("footfall"),
                    value,
                    Value::Str(stream),
                    Some(observed),
                    Extra {
                        period_seconds: Value::Int(60),
                        latitude: Value::Float(latitude),
                        longitude: Value::Float(longitude),
                        label: Value::Str(label),
                        provenance_url: Value::from(counts_url.as_str()),
                        details: Value::Dict(details),
                    },
                )?);
            }
            if matches!(
                compare_int(data.index("total_count")?, (page + 1) * 100, ">=")?,
                Some(Ordering::Less | Ordering::Equal)
            ) {
                finished = true;
                break;
            }
        }
        if !finished {
            batch.warnings.push(
                "Melbourne history page cap reached; only the newest bounded slice was collected."
                    .to_owned(),
            );
        }
        let newest = batch
            .observations
            .iter()
            .map(Observation::valid_until_at)
            .max();
        if newest.is_some_and(|until| until < now) {
            batch.warnings.push(
                "All returned pedestrian readings are stale; current crowd conditions remain unknown."
                    .to_owned(),
            );
        }
        batch.sort_warnings();
        Ok(batch)
    }
}

/// `load_json(bytes)` for a feed body.
fn load_json_bytes_strict(body: &[u8]) -> Result<Value> {
    crate::model::load_json_bytes(body)
}

/// Read at most 5 MiB of a local source file.
fn read_limited(path: &std::path::Path) -> Result<Vec<u8>> {
    let data = crate::model::read_file(path, Some(MAX_BODY as u64 + 1))?;
    if data.len() > MAX_BODY {
        return Err(Fail::input("source file exceeds 5 MiB"));
    }
    Ok(data)
}

/// Collect every enabled source that is due (or only `only`), storing what each returns.
/// A source that fails is recorded as an error and the others still run; only a database
/// failure stops the run.
pub fn collect_sources(
    config: &Config,
    store: &mut Store,
    now: Instant,
    force: bool,
    only: Option<&str>,
    client: &mut dyn Http,
    env: &dyn Env,
) -> Result<Vec<Value>> {
    let only = only.filter(|id| !id.is_empty());
    if let Some(id) = only {
        if config.source(id).is_none() {
            return Err(Fail::input("unknown source id"));
        }
    }
    let mut collector = Collector {
        config,
        client,
        env,
    };
    let mut results = Vec::new();
    for (id, source) in &config.sources {
        if !config.enabled(id) || only.is_some_and(|o| o != id) {
            continue;
        }
        let interval = number(source, "interval_seconds");
        if !store.claim(id, now, force)? {
            results.push(Value::Dict(
                Dict::new()
                    .with("source_id", id.as_str())
                    .with("status", "not_due"),
            ));
            continue;
        }
        let attempt = collector.collect(source, now).and_then(|batch| {
            let count = store.ingest(&batch.observations)?;
            store.finish(
                id,
                now,
                interval,
                i64::try_from(count).unwrap_or(i64::MAX),
                "",
                &batch.warnings,
                crate::pytime::Num::Int(0),
            )?;
            Ok((batch, count))
        });
        let (error, retry_after) = match attempt {
            Ok((batch, count)) => {
                let status = if batch.warnings.is_empty() {
                    "ok"
                } else {
                    "partial"
                };
                results.push(Value::Dict(
                    Dict::new()
                        .with("source_id", id.as_str())
                        .with("status", status)
                        .with("received", batch.observations.len())
                        .with("inserted", count)
                        .with(
                            "warnings",
                            Value::List(batch.warnings.into_iter().map(Value::Str).collect()),
                        ),
                ));
                continue;
            }
            Err(Fail::Input(message)) => (message, 0.0),
            Err(Fail::Fetch {
                message,
                retry_after,
            }) => (message, retry_after),
            Err(
                err @ Fail::Py {
                    kind: "sqlite3.Error",
                    ..
                },
            ) => return Err(err),
            Err(Fail::Py { kind, .. }) => (
                format!("Source I/O or schema error ({kind}); no observations produced"),
                0.0,
            ),
        };
        store.finish(
            id,
            now,
            interval,
            0,
            &error,
            &[],
            crate::pytime::Num::Float(retry_after),
        )?;
        results.push(Value::Dict(
            Dict::new()
                .with("source_id", id.as_str())
                .with("status", "error")
                .with("error", error),
        ));
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_matches_python() {
        // Reference values from CrowdWatch 0.1.0 on Python (glibc's pow, sin, cos, asin).
        assert_eq!(geohash(42.6, -5.6, 5), "ezs42");
        assert_eq!(geohash(51.5, -0.12, 8), "gcpuvr29");
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        {
            assert_eq!(square(4820.524451282704), 23237455.985414416);
            assert_eq!(square(-0.0007723356233719994), 5.965023151294149e-07);
        }
        assert_eq!(
            stream_id(&Value::Int(3)),
            format!("s-{}", &sha256_hex(b"3")[..24])
        );
        assert_eq!(
            stream_id(&Value::Null),
            format!("s-{}", &sha256_hex(b"None")[..24])
        );
    }

    #[test]
    fn pointers_follow_rfc_6901_like_0_1_0() {
        let data = crate::json::parse_str(r#"{"a":[{"b~c":1},{"d/e":2}],"":3}"#).unwrap();
        let p = |path: &str| pointer(&data, &Value::from(path)).cloned();
        assert!(matches!(p("/a/0/b~0c"), Ok(Value::Int(1))));
        assert!(matches!(p("/a/-1/d~1e"), Ok(Value::Int(2))));
        assert!(matches!(p("/a/ 1 /d~1e"), Ok(Value::Int(2))));
        assert!(matches!(p("/"), Ok(Value::Int(3))));
        assert_eq!(
            p("/a/2").unwrap_err().to_string(),
            "required mapped JSON field is missing"
        );
        assert_eq!(
            p("/a/x").unwrap_err().to_string(),
            "required mapped JSON field is missing"
        );
        assert_eq!(
            p("a").unwrap_err().to_string(),
            "JSON mapping uses RFC 6901 pointers starting with /"
        );
        assert!(matches!(
            pointer(&data, &Value::from("")),
            Ok(Value::Dict(_))
        ));
        assert!(pointer(&data, &Value::Int(1)).is_err());
    }
}
