// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Strict canonical records. Counts, occupancy and contextual proxies stay distinct.
//!
//! An observation is one measured aggregate from one source. Its key and content digest
//! are SHA-256 hashes of Python-style JSON, so a case database written by CrowdWatch 0.1.0
//! (Python) and one written by 0.2.0 agree on every key.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use ring::digest::{digest, SHA256};

use crate::error::{Fail, Result};
use crate::json::{self, py_eq, Dict, ReadError, Value};
use crate::pytime::{parse_time, Instant};
use crate::urlparse::{urlsplit, urlunsplit};

/// Each metric's unit, in 0.1.0's order.
pub const UNITS: [(&str, &str); 14] = [
    ("footfall", "people/interval"),
    ("occupancy", "people"),
    ("inflow", "people/min"),
    ("outflow", "people/min"),
    ("speed", "m/s"),
    ("density", "people/m2"),
    ("transit_disruption", "index"),
    ("scheduled_event", "event"),
    ("temperature", "degC"),
    ("precipitation", "mm/interval"),
    ("wind", "km/h"),
    ("bikes_available", "vehicles"),
    ("docks_available", "docks"),
    ("report", "report"),
];

/// Metrics that measure movement directly; the rest are context.
pub const DIRECT: [&str; 6] = [
    "footfall",
    "occupancy",
    "inflow",
    "outflow",
    "speed",
    "density",
];

/// The unit a metric must carry.
pub fn unit_of(metric: &str) -> Option<&'static str> {
    UNITS
        .iter()
        .find(|(name, _)| *name == metric)
        .map(|(_, unit)| *unit)
}

pub fn is_direct(metric: &str) -> bool {
    DIRECT.contains(&metric)
}

/// A range bound, with the text Python printed for it in error messages.
#[derive(Clone, Copy, Debug)]
pub struct Bound(pub f64, pub &'static str);

pub const LOW: Bound = Bound(-1e12, "-1000000000000.0");
pub const HIGH: Bound = Bound(1e12, "1000000000000.0");

/// 0.1.0's `finite(value, name, low, high)`: a real number (not a bool), finite and in
/// range, returned as a float.
pub fn finite(value: &Value, name: &str, low: Bound, high: Bound) -> Result<f64> {
    let number = match value {
        Value::Int(i) => *i as f64,
        Value::Float(f) => *f,
        Value::BigInt(_) => value.py_float()?,
        _ => return Err(Fail::input(format!("{name} must be a number"))),
    };
    if !number.is_finite() || !(low.0 <= number && number <= high.0) {
        return Err(Fail::input(format!(
            "{name} must be finite and in [{}, {}]",
            low.1, high.1
        )));
    }
    Ok(number)
}

/// 0.1.0's `identifier(value, name)`: 1 to 128 of `[a-zA-Z0-9_.:-]`, starting with a
/// letter or digit.
pub fn identifier<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    match value {
        Value::Str(s) if is_identifier(s) => Ok(s),
        _ => Err(Fail::input(format!(
            "{name} must be 1–128 letters/digits or . _ : -"
        ))),
    }
}

pub fn is_identifier(text: &str) -> bool {
    let bytes = text.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-'))
}

/// 0.1.0's `clean_url(value)`: provenance keeps no query, fragment, user name or
/// password; anything that is not a usable HTTPS URL becomes "".
pub fn clean_url(value: &Value) -> Result<String> {
    match url_argument(value) {
        Ok(text) => Ok(clean_url_str(&text)),
        // urlsplit's cache refuses unhashable values with a TypeError, which clean_url
        // catches; any other non-text value fails inside urlsplit (AttributeError).
        Err(err) if err.kind() == "TypeError" => Ok(String::new()),
        Err(err) => Err(err),
    }
}

/// A value passed to Python's `urlsplit`: text as it is; a list or dict is refused by the
/// function's cache (TypeError); anything else false is read as ""; anything else true has
/// no `decode` (AttributeError).
pub fn url_argument(value: &Value) -> Result<String> {
    match value {
        Value::Str(s) => Ok(s.clone()),
        Value::List(_) | Value::Dict(_) => Err(Fail::type_error(format!(
            "unhashable type: '{}'",
            value.type_name()
        ))),
        other if !other.truthy() => Ok(String::new()),
        other => Err(json::no_attribute(other, "decode")),
    }
}

pub fn clean_url_str(text: &str) -> String {
    let Ok(parts) = urlsplit(text) else {
        return String::new();
    };
    let Some(host) = parts.hostname() else {
        return String::new();
    };
    if parts.scheme != "https"
        || parts.username().is_some_and(|u| !u.is_empty())
        || parts.password().is_some_and(|p| !p.is_empty())
    {
        return String::new();
    }
    let netloc = match parts.port() {
        Ok(Some(port)) if port != 0 => format!("{host}:{port}"),
        Ok(_) => host,
        Err(_) => return String::new(),
    };
    urlunsplit(&parts.scheme, &netloc, &parts.path, "", "")
}

/// 0.1.0's `json_text(obj)`: sorted keys, UTF-8, compact `(",", ":")` or indented by two.
pub fn json_text(value: &Value, pretty: bool) -> Result<String> {
    json::dumps(value, pretty)
}

/// 0.1.0's `load_json(text)` for text.
pub fn load_json(text: &str) -> Result<Value> {
    json::parse_str(text).map_err(read_error)
}

/// 0.1.0's `load_json(data)` for bytes, which Python decodes as UTF-8, 16 or 32.
pub fn load_json_bytes(bytes: &[u8]) -> Result<Value> {
    json::parse_bytes(bytes).map_err(read_error)
}

fn read_error(err: ReadError) -> Fail {
    match err {
        ReadError::Syntax => Fail::input("invalid JSON input"),
        ReadError::Constant => Fail::input("NaN and Infinity are not valid data"),
        ReadError::DuplicateKey(key) => Fail::input(format!("duplicate JSON key: {key}")),
        ReadError::TooManyDigits(count) => Fail::value_error(format!(
            "Exceeds the limit (4300 digits) for integer string conversion: value has {count} digits; use sys.set_int_max_str_digits() to increase the limit"
        )),
    }
}

/// Lower-case hex SHA-256.
pub fn sha256_hex(data: &[u8]) -> String {
    digest(&SHA256, data)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The fields of an observation in 0.1.0's dataclass order.
pub const FIELDS: [&str; 16] = [
    "source_id",
    "stream_id",
    "region_id",
    "metric",
    "value",
    "unit",
    "observed_at",
    "retrieved_at",
    "valid_until",
    "period_seconds",
    "quality",
    "provenance_url",
    "label",
    "latitude",
    "longitude",
    "details",
];

const REQUIRED: [&str; 9] = [
    "source_id",
    "stream_id",
    "region_id",
    "metric",
    "value",
    "unit",
    "observed_at",
    "retrieved_at",
    "valid_until",
];

/// A validated observation: identifiers checked, numbers finite floats, times in UTC.
#[derive(Clone, Debug)]
pub struct Observation {
    pub source_id: String,
    pub stream_id: String,
    pub region_id: String,
    pub metric: String,
    pub value: f64,
    pub unit: String,
    pub observed_at: String,
    pub retrieved_at: String,
    pub valid_until: String,
    pub period_seconds: i64,
    pub quality: f64,
    pub provenance_url: String,
    pub label: String,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub details: Dict,
}

impl PartialEq for Observation {
    fn eq(&self, other: &Self) -> bool {
        self.source_id == other.source_id
            && self.stream_id == other.stream_id
            && self.region_id == other.region_id
            && self.metric == other.metric
            && self.value == other.value
            && self.unit == other.unit
            && self.observed_at == other.observed_at
            && self.retrieved_at == other.retrieved_at
            && self.valid_until == other.valid_until
            && self.period_seconds == other.period_seconds
            && self.quality == other.quality
            && self.provenance_url == other.provenance_url
            && self.label == other.label
            && self.latitude == other.latitude
            && self.longitude == other.longitude
            && py_eq(
                &Value::Dict(self.details.clone()),
                &Value::Dict(other.details.clone()),
            )
    }
}

/// The key of a stream: source, stream, region, metric and count interval.
pub type StreamKey = (String, String, String, String, i64);

impl Observation {
    /// 0.1.0's `Observation.from_dict(raw)`: exactly the known fields, the required ones
    /// present, then every check in `validated()`.
    pub fn from_value(raw: &Value) -> Result<Observation> {
        let Value::Dict(fields) = raw else {
            return Err(Fail::input("each observation must be an object"));
        };
        let mut unknown: Vec<&String> = fields
            .keys()
            .filter(|k| !FIELDS.contains(&k.as_str()))
            .collect();
        if !unknown.is_empty() {
            unknown.sort();
            unknown.dedup();
            let names: Vec<&str> = unknown.iter().map(|k| k.as_str()).collect();
            return Err(Fail::input(format!(
                "unknown observation fields: {}",
                names.join(", ")
            )));
        }
        if REQUIRED.iter().any(|name| !fields.contains_key(name)) {
            return Err(Fail::input("observation is missing required fields"));
        }
        validate(fields)
    }

    /// `asdict(self)`.
    pub fn to_value(&self) -> Value {
        Value::Dict(self.to_dict())
    }

    pub fn to_dict(&self) -> Dict {
        Dict::new()
            .with("source_id", self.source_id.as_str())
            .with("stream_id", self.stream_id.as_str())
            .with("region_id", self.region_id.as_str())
            .with("metric", self.metric.as_str())
            .with("value", self.value)
            .with("unit", self.unit.as_str())
            .with("observed_at", self.observed_at.as_str())
            .with("retrieved_at", self.retrieved_at.as_str())
            .with("valid_until", self.valid_until.as_str())
            .with("period_seconds", self.period_seconds)
            .with("quality", self.quality)
            .with("provenance_url", self.provenance_url.as_str())
            .with("label", self.label.as_str())
            .with("latitude", self.latitude)
            .with("longitude", self.longitude)
            .with("details", Value::Dict(self.details.clone()))
    }

    /// `validated()` again, after fields were replaced.
    pub fn revalidated(&self) -> Result<Observation> {
        validate(&self.to_dict())
    }

    pub fn stream_key(&self) -> StreamKey {
        (
            self.source_id.clone(),
            self.stream_id.clone(),
            self.region_id.clone(),
            self.metric.clone(),
            self.period_seconds,
        )
    }

    /// SHA-256 of the stream key and observation time: one measurement, across versions.
    pub fn key(&self) -> Result<String> {
        let body = Value::List(vec![
            Value::from(self.source_id.as_str()),
            Value::from(self.stream_id.as_str()),
            Value::from(self.region_id.as_str()),
            Value::from(self.metric.as_str()),
            Value::Int(self.period_seconds),
            Value::from(self.observed_at.as_str()),
        ]);
        Ok(sha256_hex(json_text(&body, false)?.as_bytes()))
    }

    /// SHA-256 of everything but the retrieval time: one version of a measurement.
    pub fn digest(&self) -> Result<String> {
        let mut body = self.to_dict();
        body.remove("retrieved_at");
        Ok(sha256_hex(json_text(&Value::Dict(body), false)?.as_bytes()))
    }

    pub fn observed(&self) -> Instant {
        parse_time(&Value::from(self.observed_at.as_str())).unwrap_or_else(|_| Instant::now())
    }

    pub fn valid_until_at(&self) -> Instant {
        parse_time(&Value::from(self.valid_until.as_str())).unwrap_or_else(|_| Instant::now())
    }
}

/// Python's default for an optional field.
fn field<'a>(fields: &'a Dict, name: &str, default: &'a Value) -> &'a Value {
    fields.get(name).unwrap_or(default)
}

/// 0.1.0's `Observation.validated()`, check for check and message for message.
pub fn validate(fields: &Dict) -> Result<Observation> {
    let zero = Value::Int(0);
    let one = Value::Float(1.0);
    let empty = Value::from("");
    let none = Value::Null;
    let no_details = Value::Dict(Dict::new());
    let get = |name: &str| fields.get(name).unwrap_or(&none);

    let source_id = identifier(get("source_id"), "source_id")?.to_owned();
    let stream_id = identifier(get("stream_id"), "stream_id")?.to_owned();
    let region_id = identifier(get("region_id"), "region_id")?.to_owned();
    let metric = match get("metric") {
        Value::Str(m) if unit_of(m).is_some() => m.clone(),
        _ => return Err(Fail::input("metric/unit mismatch")),
    };
    let unit = match get("unit") {
        Value::Str(u) if Some(u.as_str()) == unit_of(&metric) => u.clone(),
        _ => return Err(Fail::input("metric/unit mismatch")),
    };
    let low = if metric == "temperature" {
        Bound(-100.0, "-100")
    } else {
        Bound(0.0, "0")
    };
    let raw_value = get("value");
    let value = finite(raw_value, "value", low, HIGH)?;
    if metric == "transit_disruption" {
        finite(
            raw_value,
            "transit_disruption",
            Bound(0.0, "0"),
            Bound(1.0, "1"),
        )?;
    }
    let quality = finite(
        field(fields, "quality", &one),
        "quality",
        Bound(0.0, "0"),
        Bound(1.0, "1"),
    )?;
    let period_seconds = match field(fields, "period_seconds", &zero) {
        Value::Int(i) => *i,
        Value::BigInt(_) => i64::MAX,
        _ => return Err(Fail::input("period_seconds must be an integer")),
    };
    if !(0..=86_400).contains(&period_seconds) {
        return Err(Fail::input("period_seconds must be in [0, 86400]"));
    }
    if (metric == "footfall" || metric == "precipitation") && period_seconds == 0 {
        return Err(Fail::input(
            "interval measurements require positive period_seconds",
        ));
    }
    let observed = parse_time(get("observed_at"))?;
    let retrieved = parse_time(get("retrieved_at"))?;
    let until = parse_time(get("valid_until"))?;
    if observed > retrieved {
        return Err(Fail::input(
            "observed_at cannot be later than retrieved_at; schedules belong in details",
        ));
    }
    if until <= observed {
        return Err(Fail::input("valid_until must be later than observed_at"));
    }
    let raw_lat = field(fields, "latitude", &none);
    let raw_lon = field(fields, "longitude", &none);
    if raw_lat.is_null() != raw_lon.is_null() {
        return Err(Fail::input(
            "latitude and longitude must be supplied together",
        ));
    }
    let (latitude, longitude) = if raw_lat.is_null() {
        (None, None)
    } else {
        let lat = finite(raw_lat, "latitude", Bound(-90.0, "-90"), Bound(90.0, "90"))?;
        let lon = finite(
            raw_lon,
            "longitude",
            Bound(-180.0, "-180"),
            Bound(180.0, "180"),
        )?;
        (Some(lat + 0.0), Some(lon + 0.0))
    };
    let label = match field(fields, "label", &empty) {
        Value::Str(s) if s.chars().count() <= 500 => s.clone(),
        _ => return Err(Fail::input("label must be at most 500 characters")),
    };
    let details = match field(fields, "details", &no_details) {
        Value::Dict(d) => {
            if json_text(&Value::Dict(d.clone()), false)?.chars().count() > 8192 {
                return Err(Fail::input(
                    "details must be a JSON object no larger than 8192 characters",
                ));
            }
            d.clone()
        }
        _ => {
            return Err(Fail::input(
                "details must be a JSON object no larger than 8192 characters",
            ))
        }
    };
    let provenance_url = match field(fields, "provenance_url", &empty) {
        Value::Str(s) => s.clone(),
        _ => return Err(Fail::input("provenance_url must be a string")),
    };
    if !provenance_url.is_empty() && clean_url_str(&provenance_url) != provenance_url {
        return Err(Fail::input(
            "provenance_url must be HTTPS without query, credentials or fragment",
        ));
    }
    if metric == "scheduled_event" {
        let start = parse_time(details.get("starts_at").unwrap_or(&none))?;
        let end = parse_time(details.get("ends_at").unwrap_or(&none))?;
        if start >= end {
            return Err(Fail::input("scheduled event end must follow start"));
        }
    }
    Ok(Observation {
        source_id,
        stream_id,
        region_id,
        metric,
        value: value + 0.0,
        unit,
        observed_at: observed.stamp(),
        retrieved_at: retrieved.stamp(),
        valid_until: until.stamp(),
        period_seconds,
        quality: quality + 0.0,
        provenance_url,
        label,
        latitude,
        longitude,
        details,
    })
}

/// pathlib's `path.parent`: the folder above, `.` above a bare name, and a root or a
/// drive is its own parent.
pub fn parent_of(path: &Path) -> PathBuf {
    match path.parent() {
        None => path.to_path_buf(),
        Some(parent) if parent.as_os_str().is_empty() => PathBuf::from("."),
        Some(parent) => parent.to_path_buf(),
    }
}

/// 0.1.0's `path.mkdir(parents=True, exist_ok=True)`. Like Python, an error names the
/// folder that could not be made, which may be one of the missing parents.
pub fn make_dirs(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(err) if crate::error::is_not_found(&err) => {
            let parent = parent_of(path);
            if parent == path {
                return Err(crate::error::io_at(path, err));
            }
            make_dirs(&parent)?;
            // `self.mkdir(parents=False, exist_ok=True)`: a parent missing again is an error.
            match fs::create_dir(path) {
                Ok(()) => Ok(()),
                Err(err) if !crate::error::is_not_found(&err) && path.is_dir() => Ok(()),
                Err(err) => Err(crate::error::io_at(path, err)),
            }
        }
        Err(_) if path.is_dir() => Ok(()),
        Err(err) => Err(crate::error::io_at(path, err)),
    }
}

/// 0.1.0's `with path.open("rb") as f: data = f.read(limit)`, or all of it when `limit`
/// is `None`. Opening names the file in any error; reading does not, as in Python.
pub fn read_file(path: &Path, limit: Option<u64>) -> Result<Vec<u8>> {
    let file = fs::File::open(path).map_err(|e| crate::error::open_at(path, e))?;
    // Python's open() refuses a folder, which Linux and macOS would otherwise open.
    if file.metadata().is_ok_and(|m| m.is_dir()) {
        return Err(crate::error::open_at(path, crate::error::is_a_directory()));
    }
    let mut data = Vec::new();
    match limit {
        Some(limit) => file.take(limit).read_to_end(&mut data),
        None => (&file).read_to_end(&mut data),
    }
    .map_err(crate::error::crt)?;
    Ok(data)
}

/// 0.1.0's `atomic_write(path, text)`: a private temporary file in the same folder,
/// flushed to disk, then moved over the target.
pub fn atomic_write(path: &Path, text: &str) -> Result<()> {
    let folder = parent_of(path);
    make_dirs(&folder)?;
    // `tempfile.mkstemp(dir=folder)` makes the folder absolute first.
    let base = std::path::absolute(&folder).unwrap_or_else(|_| folder.clone());
    let (temp, mut file) = loop {
        let candidate = base.join(format!(".crowdwatch-{}", uuid::Uuid::new_v4().simple()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(file) => break (candidate, file),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(crate::error::open_at(&candidate, err)),
        }
    };
    let written = file
        .write_all(text.as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);
    let result = match written {
        Err(err) => Err(crate::error::crt(err)),
        Ok(()) => fs::rename(&temp, path).map_err(|err| crate::error::io_between(&temp, path, err)),
    };
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse_str;

    fn raw(extra: &str) -> Value {
        parse_str(&format!(
            r#"{{"source_id":"counter","stream_id":"sensor","region_id":"area","metric":"footfall",
            "value":20,"unit":"people/interval","observed_at":"2026-10-01T12:00:00Z",
            "retrieved_at":"2026-10-01T12:00:00Z","valid_until":"2026-10-01T12:10:00Z",
            "period_seconds":300{extra}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn validates_like_0_1_0() {
        let o = Observation::from_value(&raw("")).unwrap();
        assert_eq!(o.value, 20.0);
        assert_eq!(o.observed_at, "2026-10-01T12:00:00.000000Z");
        // Values computed by CrowdWatch 0.1.0 (Python) for this record.
        assert_eq!(
            o.key().unwrap(),
            "2a3706c2a79b5a6278e1cac6fad9f736b4c979bf24b2b9856ea7964528d313ee"
        );
        assert_eq!(
            o.digest().unwrap(),
            "524b50d9fdadc301a50ab408a9277f11de612e0265e6a801239bf12fa32e1d37"
        );
        for (extra, message) in [
            (r#","quality":1.1"#, "quality must be finite and in [0, 1]"),
            (
                r#","latitude":51"#,
                "latitude and longitude must be supplied together",
            ),
            (
                r#","provenance_url":"https://example.org/?k=s""#,
                "provenance_url must be HTTPS without query, credentials or fragment",
            ),
            (r#","colour":"red""#, "unknown observation fields: colour"),
            (r#","label":5"#, "label must be at most 500 characters"),
        ] {
            assert_eq!(
                Observation::from_value(&raw(extra))
                    .unwrap_err()
                    .to_string(),
                message
            );
        }
        let bad_value = raw("").to_owned();
        let mut fields = bad_value.as_dict().unwrap().clone();
        fields.insert("value", Value::Bool(true));
        assert_eq!(
            validate(&fields).unwrap_err().to_string(),
            "value must be a number"
        );
        fields.insert("value", Value::Float(-1.0));
        assert_eq!(
            validate(&fields).unwrap_err().to_string(),
            "value must be finite and in [0, 1000000000000.0]"
        );
    }

    #[test]
    fn cleans_urls_like_0_1_0() {
        for (input, output) in [
            (
                "https://Example.org:443/a?q=1#f",
                "https://example.org:443/a",
            ),
            ("https://example.org/feed", "https://example.org/feed"),
            ("http://example.org/", ""),
            ("https://user@example.org/", ""),
            ("https://example.org:99999/", ""),
            ("https://example.org:0/x", "https://example.org/x"),
            ("", ""),
        ] {
            assert_eq!(clean_url_str(input), output, "{input}");
        }
        assert_eq!(clean_url(&Value::Null).unwrap(), "");
        assert_eq!(
            clean_url(&Value::Int(5)).unwrap_err().kind(),
            "AttributeError"
        );
    }
}
