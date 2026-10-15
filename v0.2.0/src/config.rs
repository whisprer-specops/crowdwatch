// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! The deployment configuration: regions, sources and analysis settings.
//!
//! Every check, default and message is 0.1.0's, so a configuration that worked there
//! works here and a mistake is reported in the same words. Regions and sources keep the
//! exact JSON values given, because reports print them back.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::error::{Fail, Result};
use crate::json::{py_eq, Dict, Value};
use crate::model::{finite, identifier, load_json_bytes, Bound};
use crate::pyfmt::strip;
use crate::pytime::Num;

/// The collector kinds a source may use.
pub const KINDS: [&str; 9] = [
    "canonical_json",
    "canonical_csv",
    "mapped_json",
    "melbourne",
    "tfl",
    "open_meteo",
    "ticketmaster",
    "gbfs",
    "rss",
];

/// A loaded configuration.
#[derive(Clone, Debug)]
pub struct Config {
    pub path: PathBuf,
    /// Regions by ID, in file order, exactly as written.
    pub regions: Vec<(String, Dict)>,
    /// Sources by ID, in file order, with 0.1.0's defaults filled in.
    pub sources: Vec<(String, Dict)>,
    /// Analysis settings, with defaults filled in.
    pub analysis: Dict,
    pub database: PathBuf,
}

const REGION_FIELDS: [&str; 8] = [
    "id",
    "name",
    "latitude",
    "longitude",
    "radius_km",
    "capacity_people",
    "occupancy_stream",
    "balance",
];
const SOURCE_FIELDS: [&str; 10] = [
    "id",
    "kind",
    "region_id",
    "enabled",
    "interval_seconds",
    "quality",
    "ttl_seconds",
    "options",
    "independence_group",
    "attribution",
];
const ANALYSIS_FIELDS: [&str; 5] = [
    "history_days",
    "minimum_baseline",
    "z_threshold",
    "horizon_minutes",
    "minimum_quality",
];

fn is_reference(value: &Value) -> bool {
    matches!(value, Value::Str(s) if s.contains('/'))
}

impl Config {
    /// 0.1.0's `Config.load(path)`.
    pub fn load(path: &Path) -> Result<Config> {
        let path = resolve(path);
        let bytes = std::fs::read(&path)
            .map_err(|_| Fail::input(format!("cannot read configuration: {}", path.display())))?;
        let raw = load_json_bytes(&bytes)?;
        let top = match &raw {
            Value::Dict(d)
                if d.get("schema_version")
                    .is_some_and(|v| py_eq(v, &Value::Int(1))) =>
            {
                d
            }
            _ => return Err(Fail::input("configuration requires schema_version: 1")),
        };
        if top.keys().any(|k| {
            ![
                "schema_version",
                "database",
                "regions",
                "sources",
                "analysis",
            ]
            .contains(&k.as_str())
        }) {
            return Err(Fail::input("unknown top-level configuration field"));
        }
        let empty_list = Value::List(Vec::new());
        let empty_dict = Value::Dict(Dict::new());
        let (Some(Value::List(raw_regions)), Value::List(raw_sources)) = (
            top.get("regions"),
            top.get("sources").unwrap_or(&empty_list),
        ) else {
            return Err(Fail::input("regions and sources must be arrays"));
        };
        let Value::Dict(raw_analysis) = top.get("analysis").unwrap_or(&empty_dict) else {
            return Err(Fail::input("analysis must be an object"));
        };
        if raw_regions.len() > 128 || raw_sources.len() > 128 {
            return Err(Fail::input(
                "this module supports at most 128 regions and 128 configured sources",
            ));
        }
        let none = Value::Null;
        let mut regions: Vec<(String, Dict)> = Vec::new();
        for raw_region in raw_regions {
            let Value::Dict(r) = raw_region else {
                return Err(Fail::input("region must be an object"));
            };
            if r.keys().any(|k| !REGION_FIELDS.contains(&k.as_str())) {
                return Err(Fail::input("unknown region configuration field"));
            }
            let key = identifier(r.get("id").unwrap_or(&none), "region.id")?.to_owned();
            if regions.iter().any(|(id, _)| *id == key) {
                return Err(Fail::input("duplicate region id"));
            }
            match r.get("name") {
                Some(Value::Str(name)) if !strip(name).is_empty() => {}
                _ => return Err(Fail::input("region needs a name")),
            }
            let get = |name: &str| r.get(name).unwrap_or(&none);
            finite(
                get("latitude"),
                "region latitude",
                Bound(-90.0, "-90"),
                Bound(90.0, "90"),
            )?;
            finite(
                get("longitude"),
                "region longitude",
                Bound(-180.0, "-180"),
                Bound(180.0, "180"),
            )?;
            finite(
                get("radius_km"),
                "radius_km",
                Bound(0.01, "0.01"),
                Bound(100.0, "100"),
            )?;
            if let Some(capacity) = r.get("capacity_people") {
                finite(
                    capacity,
                    "capacity_people",
                    Bound(1.0, "1"),
                    Bound(1e8, "100000000.0"),
                )?;
                if !is_reference(get("occupancy_stream")) {
                    return Err(Fail::input(
                        "capacity_people requires an exact source_id/stream_id occupancy_stream",
                    ));
                }
            }
            if let Some(balance) = r.get("balance") {
                let Value::Dict(b) = balance else {
                    return Err(Fail::input(
                        "balance requires occupancy, inflow and outflow stream references",
                    ));
                };
                let mut keys: Vec<&str> = b.keys().map(String::as_str).collect();
                keys.sort_unstable();
                if keys != ["inflow", "occupancy", "outflow"] {
                    return Err(Fail::input(
                        "balance requires occupancy, inflow and outflow stream references",
                    ));
                }
                if b.iter().any(|(_, v)| !is_reference(v)) {
                    return Err(Fail::input(
                        "balance references must be source_id/stream_id",
                    ));
                }
            }
            regions.push((key, r.clone()));
        }
        if regions.is_empty() {
            return Err(Fail::input("configure at least one region"));
        }
        let mut sources: Vec<(String, Dict)> = Vec::new();
        for raw_source in raw_sources {
            let Value::Dict(given) = raw_source else {
                return Err(Fail::input("source must be an object"));
            };
            if given.keys().any(|k| !SOURCE_FIELDS.contains(&k.as_str())) {
                return Err(Fail::input("unknown source configuration field"));
            }
            let mut s = Dict::new()
                .with("enabled", true)
                .with("interval_seconds", 300_i64)
                .with("quality", 0.8)
                .with("ttl_seconds", 900_i64)
                .with("options", Dict::new());
            for (k, v) in given.iter() {
                s.insert(k.clone(), v.clone());
            }
            let key = identifier(s.get("id").unwrap_or(&none), "source.id")?.to_owned();
            let kind_ok =
                matches!(s.get("kind"), Some(Value::Str(k)) if KINDS.contains(&k.as_str()));
            if sources.iter().any(|(id, _)| *id == key) || !kind_ok {
                return Err(Fail::input(
                    "duplicate source id or unsupported source kind",
                ));
            }
            let region_ok = matches!(s.get("region_id"), Some(Value::Str(id)) if regions.iter().any(|(r, _)| r == id));
            if !region_ok {
                return Err(Fail::input("source references an unknown region"));
            }
            if !matches!(s.get("enabled"), Some(Value::Bool(_)))
                || !matches!(s.get("options"), Some(Value::Dict(_)))
            {
                return Err(Fail::input(
                    "source enabled must be boolean and options must be an object",
                ));
            }
            let group = s
                .get("independence_group")
                .cloned()
                .unwrap_or_else(|| Value::from(key.as_str()));
            identifier(&group, "independence_group")?;
            s.insert("independence_group", group);
            let get = |name: &str| s.get(name).unwrap_or(&none);
            finite(
                get("quality"),
                "source quality",
                Bound(0.0, "0"),
                Bound(1.0, "1"),
            )?;
            finite(
                get("interval_seconds"),
                "source interval",
                Bound(60.0, "60"),
                Bound(86_400.0, "86400"),
            )?;
            finite(
                get("ttl_seconds"),
                "source TTL",
                Bound(30.0, "30"),
                Bound(604_800.0, "604800"),
            )?;
            match s.get("attribution") {
                Some(Value::Str(text)) if !strip(text).is_empty() => {}
                _ => return Err(Fail::input("every source needs attribution")),
            }
            sources.push((key, s));
        }
        let mut analysis = Dict::new()
            .with("history_days", 28_i64)
            .with("minimum_baseline", 8_i64)
            .with("z_threshold", 3.0)
            .with("horizon_minutes", 15_i64)
            .with("minimum_quality", 0.5);
        for (k, v) in raw_analysis.iter() {
            analysis.insert(k.clone(), v.clone());
        }
        if analysis
            .keys()
            .any(|k| !ANALYSIS_FIELDS.contains(&k.as_str()))
        {
            return Err(Fail::input("unknown analysis configuration field"));
        }
        let get = |name: &str| analysis.get(name).unwrap_or(&none);
        finite(
            get("history_days"),
            "history_days",
            Bound(2.0, "2"),
            Bound(90.0, "90"),
        )?;
        finite(
            get("minimum_baseline"),
            "minimum_baseline",
            Bound(4.0, "4"),
            Bound(500.0, "500"),
        )?;
        finite(
            get("z_threshold"),
            "z_threshold",
            Bound(2.0, "2"),
            Bound(10.0, "10"),
        )?;
        finite(
            get("horizon_minutes"),
            "horizon_minutes",
            Bound(1.0, "1"),
            Bound(30.0, "30"),
        )?;
        finite(
            get("minimum_quality"),
            "minimum_quality",
            Bound(0.0, "0"),
            Bound(1.0, "1"),
        )?;
        let database = match top.get("database") {
            None => "crowdwatch.sqlite".to_owned(),
            Some(Value::Str(name)) if !name.is_empty() => name.clone(),
            Some(_) => return Err(Fail::input("database must be a nonempty path")),
        };
        let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let database = resolve(&folder.join(database));
        Ok(Config {
            path,
            regions,
            sources,
            analysis,
            database,
        })
    }

    pub fn region(&self, id: &str) -> Option<&Dict> {
        self.regions
            .iter()
            .find(|(key, _)| key == id)
            .map(|(_, r)| r)
    }

    pub fn source(&self, id: &str) -> Option<&Dict> {
        self.sources
            .iter()
            .find(|(key, _)| key == id)
            .map(|(_, s)| s)
    }

    /// An analysis setting as a Python number.
    pub fn setting(&self, name: &str) -> Num {
        Num::from_value(self.analysis.get(name).unwrap_or(&Value::Null))
    }

    /// Whether the source exists and is enabled.
    pub fn enabled(&self, id: &str) -> bool {
        self.source(id)
            .is_some_and(|s| matches!(s.get("enabled"), Some(Value::Bool(true))))
    }
}

/// A source setting as a Python number (the configuration checked it is one).
pub fn number(source: &Dict, name: &str) -> Num {
    Num::from_value(source.get(name).unwrap_or(&Value::Null))
}

/// A source's string field (its ID, kind or region).
pub fn text<'a>(source: &'a Dict, name: &str) -> &'a str {
    source.get(name).and_then(Value::as_str).unwrap_or("")
}

/// Python's `Path.resolve()`: absolute, links followed where the path exists, `..`
/// applied to the rest. Windows' `\\?\` prefix is removed, as Python removes it.
pub fn resolve(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|dir| dir.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let parts: Vec<Component<'_>> = absolute.components().collect();
    for split in (0..=parts.len()).rev() {
        let head: PathBuf = parts[..split].iter().collect();
        if head.as_os_str().is_empty() {
            continue;
        }
        let Ok(real) = std::fs::canonicalize(&head) else {
            continue;
        };
        let mut out = simplify(real);
        for part in &parts[split..] {
            match part {
                Component::ParentDir => {
                    out.pop();
                }
                Component::CurDir => {}
                other => out.push(other.as_os_str()),
            }
        }
        return out;
    }
    absolute
}

/// Python's `Path(text)`, as it prints and as it reaches the system. pathlib drops `.`
/// parts and repeated or trailing separators, and on Windows writes every separator as
/// `\`. So `./counts.csv` is `counts.csv`, `out/` is `out` and an empty argument is `.`.
pub fn pure_path(text: &str) -> PathBuf {
    PathBuf::from(pure_path_text(text, cfg!(windows)))
}

/// `str(Path(text))` with POSIX or Windows rules (pathlib's `_parse_path` and
/// `_format_parsed_parts`).
fn pure_path_text(text: &str, windows: bool) -> String {
    let (sep, text) = if windows {
        ('\\', text.replace('/', "\\"))
    } else {
        ('/', text.to_owned())
    };
    let (drive, mut root, rest) = if windows {
        nt_splitroot(&text)
    } else {
        posix_splitroot(&text)
    };
    if windows && root.is_empty() && drive.starts_with('\\') && !drive.ends_with('\\') {
        let pieces: Vec<&str> = drive.split('\\').collect();
        // `\\server\share` and `\\?\UNC\server\share` gain a root.
        if (pieces.len() == 4 && !"?.".contains(pieces[2])) || pieces.len() == 6 {
            root = "\\";
        }
    }
    let parts: Vec<&str> = rest
        .split(sep)
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    let joined = parts.join(if windows { "\\" } else { "/" });
    if !drive.is_empty() || !root.is_empty() {
        return format!("{drive}{root}{joined}");
    }
    if windows
        && parts
            .first()
            .is_some_and(|first| !nt_splitroot(first).0.is_empty())
    {
        // A first part that reads as a drive (`c:x`) keeps a `.` in front.
        return format!(".\\{joined}");
    }
    if joined.is_empty() {
        ".".to_owned()
    } else {
        joined
    }
}

/// `posixpath.splitroot`: drive (always empty), root and the rest. Exactly two leading
/// slashes are kept, as POSIX allows them a meaning of their own.
fn posix_splitroot(p: &str) -> (&str, &str, &str) {
    if !p.starts_with('/') {
        ("", "", p)
    } else if !p[1..].starts_with('/') || p[2..].starts_with('/') {
        ("", "/", &p[1..])
    } else {
        ("", &p[..2], &p[2..])
    }
}

/// `ntpath.splitroot` for a path already written with `\`: drive (`C:`,
/// `\\server\share`, `\\?\UNC\server\share`), root and the rest.
pub(crate) fn nt_splitroot(p: &str) -> (&str, &str, &str) {
    if let Some(after) = p.strip_prefix('\\') {
        if !after.starts_with('\\') {
            return ("", &p[..1], &p[1..]);
        }
        let unc = p.len() >= 8 && p.as_bytes()[..8].eq_ignore_ascii_case(br"\\?\UNC\");
        let start = if unc { 8 } else { 2 };
        let Some(index) = p[start..].find('\\').map(|i| start + i) else {
            return (p, "", "");
        };
        let Some(index2) = p[index + 1..].find('\\').map(|i| index + 1 + i) else {
            return (p, "", "");
        };
        return (&p[..index2], &p[index2..=index2], &p[index2 + 1..]);
    }
    let mut chars = p.char_indices();
    match (chars.next(), chars.next()) {
        (Some(_), Some((colon, ':'))) => {
            let (drive, rest) = p.split_at(colon + 1);
            match rest.strip_prefix('\\') {
                Some(tail) => (drive, &rest[..1], tail),
                None => (drive, "", rest),
            }
        }
        _ => ("", "", p),
    }
}

/// `\\?\C:\x` to `C:\x` and `\\?\UNC\server\share` to `\\server\share`.
fn simplify(path: PathBuf) -> PathBuf {
    let text = path.as_os_str().to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(OsString::from(format!(r"\\{rest}")));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        if rest.as_bytes().get(1) == Some(&b':') {
            return PathBuf::from(rest);
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_config(dir: &Path, text: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("config.json");
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn loads_and_fills_defaults() {
        let dir = std::env::temp_dir().join(format!("cw-config-{}", uuid::Uuid::new_v4().simple()));
        let path = write_config(
            &dir,
            r#"{"schema_version":1,"regions":[{"id":"area","name":"Test","latitude":51.5,"longitude":-0.12,"radius_km":2}],
               "sources":[{"id":"counter","kind":"canonical_json","region_id":"area","attribution":"x","options":{"path":"d.jsonl"}}]}"#,
        );
        let cfg = Config::load(&path).unwrap();
        let s = cfg.source("counter").unwrap();
        let keys: Vec<&str> = s.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "enabled",
                "interval_seconds",
                "quality",
                "ttl_seconds",
                "options",
                "id",
                "kind",
                "region_id",
                "attribution",
                "independence_group"
            ]
        );
        assert_eq!(cfg.database, resolve(&dir.join("crowdwatch.sqlite")));
        assert_eq!(cfg.setting("horizon_minutes"), Num::Int(15));
        for (text, message) in [
            (
                r#"{"schema_version":2,"regions":[]}"#,
                "configuration requires schema_version: 1",
            ),
            (
                r#"{"schema_version":1,"regions":42}"#,
                "regions and sources must be arrays",
            ),
            (
                r#"{"schema_version":1,"regions":[]}"#,
                "configure at least one region",
            ),
            (
                r#"{"schema_version":1,"regions":[{"id":"a","name":" ","latitude":0,"longitude":0,"radius_km":1}]}"#,
                "region needs a name",
            ),
            (
                r#"{"schema_version":1,"regions":[{"id":"a","name":"A","latitude":0,"longitude":0,"radius_km":0}]}"#,
                "radius_km must be finite and in [0.01, 100]",
            ),
            (
                r#"{"schema_version":1,"regions":[{"id":"a","name":"A","latitude":0,"longitude":0,"radius_km":1,"capacity_people":500}]}"#,
                "capacity_people requires an exact source_id/stream_id occupancy_stream",
            ),
            (
                r#"{"schema_version":1,"regions":[{"id":"a","name":"A","latitude":0,"longitude":0,"radius_km":1}],"analysis":{"z_threshold":1}}"#,
                "z_threshold must be finite and in [2, 10]",
            ),
        ] {
            let path = write_config(&dir, text);
            assert_eq!(
                Config::load(&path).unwrap_err().to_string(),
                message,
                "{text}"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn resolves_like_python() {
        let here = std::env::current_dir().unwrap();
        let resolved = resolve(Path::new("no-such-dir/../x.json"));
        assert_eq!(
            resolved,
            simplify(std::fs::canonicalize(&here).unwrap()).join("x.json")
        );
    }

    #[test]
    fn pure_paths_print_like_pathlib() {
        // (argument, str(PurePosixPath(argument)), str(PureWindowsPath(argument))) from
        // Python's pathlib.
        let cases = [
            ("", ".", "."),
            (".", ".", "."),
            ("./", ".", "."),
            ("./counts.csv", "counts.csv", "counts.csv"),
            ("a//b", "a/b", r"a\b"),
            ("a/./b", "a/b", r"a\b"),
            ("a/", "a", "a"),
            ("a/b/", "a/b", r"a\b"),
            ("/", "/", r"\"),
            ("//", "//", r"\\"),
            ("///", "/", r"\\\"),
            ("//a", "//a", r"\\a"),
            ("///a", "/a", r"\\\a"),
            ("/a/../b", "/a/../b", r"\a\..\b"),
            ("../x", "../x", r"..\x"),
            (r".\counts.csv", r".\counts.csv", "counts.csv"),
            (r"sub\x.json", r"sub\x.json", r"sub\x.json"),
            ("C:", "C:", "C:"),
            (r"C:\", r"C:\", r"C:\"),
            ("C:x", "C:x", "C:x"),
            (r"C:\x\", r"C:\x\", r"C:\x"),
            ("c:/x/y", "c:/x/y", r"c:\x\y"),
            (r"\\server\share", r"\\server\share", r"\\server\share\"),
            (
                r"\\server\share\x",
                r"\\server\share\x",
                r"\\server\share\x",
            ),
            ("//server/share", "//server/share", r"\\server\share\"),
            (r"\\?\C:\x", r"\\?\C:\x", r"\\?\C:\x"),
            (
                r"\\?\UNC\srv\share\f",
                r"\\?\UNC\srv\share\f",
                r"\\?\UNC\srv\share\f",
            ),
            (r"\\.\pipe\x", r"\\.\pipe\x", r"\\.\pipe\x"),
            (r"\x", r"\x", r"\x"),
            ("./c:x", "c:x", r".\c:x"),
            ("a/c:x", "a/c:x", r"a\c:x"),
            (r"\\server", r"\\server", r"\\server"),
            ("é:x", "é:x", "é:x"),
            (" ", " ", " "),
            (r"a\.\b", r"a\.\b", r"a\b"),
        ];
        for (raw, posix, windows) in cases {
            assert_eq!(pure_path_text(raw, false), posix, "POSIX {raw:?}");
            assert_eq!(pure_path_text(raw, true), windows, "Windows {raw:?}");
        }
    }
}
