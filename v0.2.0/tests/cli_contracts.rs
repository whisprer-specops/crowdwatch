// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! The `crowdwatch` program as a user runs it: exit codes, help, the version notice, and
//! the demo, analysis, import and SkeleTrace commands end to end.

mod common;

use std::path::Path;
use std::process::{Command, Output};

use common::temp_dir;
use crowdwatch::json::Value;
use crowdwatch::model::load_json;

fn crowdwatch(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_crowdwatch"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run crowdwatch")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .replace("\r\n", "\n")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone())
        .unwrap()
        .replace("\r\n", "\n")
}

#[test]
fn version_keeps_the_number_first_then_the_notice() {
    let dir = temp_dir("version");
    let out = crowdwatch(&dir, &["--version"]);
    assert!(out.status.success());
    let text = stdout(&out);
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some(env!("CARGO_PKG_VERSION")));
    assert_eq!(lines.next(), Some("Copyright (C) 2026 whispr.dev"));
    assert!(text.contains("License GPL-3.0-only"));
    assert!(text.contains("NO WARRANTY"));
}

#[test]
fn help_and_usage_errors() {
    let dir = temp_dir("help");
    let help = crowdwatch(&dir, &["--help"]);
    assert!(help.status.success());
    let text = stdout(&help);
    assert!(text.starts_with("usage: crowdwatch [-h] [--version]"));
    assert!(text.contains(
        "Copyright (C) 2026 whispr.dev. Free software under GPL-3.0-only, with NO WARRANTY."
    ));
    assert!(text.contains("verify-skeletrace"));
    let none = crowdwatch(&dir, &[]);
    assert_eq!(none.status.code(), Some(2));
    assert!(stderr(&none)
        .ends_with("crowdwatch: error: the following arguments are required: command\n"));
    let missing = crowdwatch(&dir, &["report", "--config", "c.json"]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(stderr(&missing)
        .contains("crowdwatch report: error: the following arguments are required: --out"));
    let unreadable = crowdwatch(&dir, &["analyse", "--config", "nope.json"]);
    assert_eq!(unreadable.status.code(), Some(1));
    assert!(stderr(&unreadable).starts_with("crowdwatch: cannot read configuration: "));
}

#[test]
fn demo_analysis_and_export_end_to_end() {
    let dir = temp_dir("demo-cli");
    let out = crowdwatch(
        &dir,
        &["demo", "--out", "demo", "--at", "2026-10-01T09:00:00Z"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let result = load_json(&stdout(&out)).unwrap();
    assert_eq!(
        crowdwatch::model::json_text(result.index("levels").unwrap(), false).unwrap(),
        r#"{"market":"insufficient_data","plaza":"baseline","station":"corroborated"}"#
    );
    assert!(matches!(result.index("records").unwrap(), Value::Int(396)));
    let report = std::fs::read_to_string(dir.join("demo/report.json")).unwrap();
    assert!(report.contains(&format!("\"version\": \"{}\"", env!("CARGO_PKG_VERSION"))));
    let page = std::fs::read_to_string(dir.join("demo/report.html")).unwrap();
    assert!(page.contains("SKELETRACE / CROWDWATCH 0.2"));
    let analysed = crowdwatch(
        &dir,
        &[
            "analyse",
            "--config",
            "demo/config.json",
            "--at",
            "2026-10-01T09:00:00Z",
        ],
    );
    assert!(analysed.status.success());
    // The stored report and a fresh analysis differ only where the demo marks itself
    // synthetic (both are synthetic here) and in nothing else.
    assert_eq!(stdout(&analysed), report);
    let exported = crowdwatch(
        &dir,
        &[
            "export-skeletrace",
            "--config",
            "demo/config.json",
            "--at",
            "2026-10-01T09:00:00Z",
            "--out",
            "exp",
        ],
    );
    assert!(exported.status.success());
    let manifest = load_json(&stdout(&exported)).unwrap();
    assert!(Path::new(manifest.index("profile").unwrap().as_str().unwrap()).is_file());
    let again = crowdwatch(&dir, &["demo", "--out", "demo"]);
    assert_eq!(again.status.code(), Some(1));
    assert!(stderr(&again).contains("demo output directory must be empty"));
}

#[test]
fn collect_exits_two_when_a_source_fails() {
    let dir = temp_dir("collect-cli");
    std::fs::write(
        dir.join("config.json"),
        r#"{"schema_version":1,"regions":[{"id":"area","name":"Area","latitude":51.5,"longitude":-0.12,"radius_km":2}],
            "sources":[{"id":"feed","kind":"canonical_json","region_id":"area","attribution":"x","options":{"path":"missing.jsonl"}}]}"#,
    )
    .unwrap();
    let out = crowdwatch(&dir, &["collect", "--config", "config.json"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stdout(&out)
        .contains("Source I/O or schema error (FileNotFoundError); no observations produced"));
    let again = crowdwatch(&dir, &["collect", "--config", "config.json"]);
    assert_eq!(again.status.code(), Some(0));
    assert!(stdout(&again).contains("\"not_due\""));
}

#[test]
fn init_import_and_doctor() {
    let dir = temp_dir("init-cli");
    for preset in ["london", "melbourne", "new-york"] {
        let name = format!("{preset}.json");
        let out = crowdwatch(&dir, &["init", "--out", &name, "--preset", preset]);
        assert!(out.status.success());
        assert!(crowdwatch::config::Config::load(&dir.join(&name)).is_ok());
    }
    let exists = crowdwatch(&dir, &["init", "--out", "london.json"]);
    assert_eq!(exists.status.code(), Some(1));
    std::fs::write(
        dir.join("config.json"),
        r#"{"schema_version":1,"regions":[{"id":"area","name":"Area","latitude":51.5,"longitude":-0.12,"radius_km":2}],
            "sources":[{"id":"counter","kind":"canonical_csv","region_id":"area","attribution":"x","options":{"path":"x.csv"}}]}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("counts.csv"),
        "source_id,stream_id,region_id,metric,value,unit,observed_at,retrieved_at,valid_until,period_seconds\r\n\
         counter,gate,area,footfall,12,people/interval,2026-10-01T11:00:00Z,2026-10-01T11:00:00Z,2026-10-01T11:10:00Z,300\r\n",
    )
    .unwrap();
    let imported = crowdwatch(
        &dir,
        &[
            "import",
            "--config",
            "config.json",
            "counts.csv",
            "--format",
            "canonical_csv",
            "--historical-availability",
        ],
    );
    assert!(imported.status.success(), "{}", stderr(&imported));
    assert_eq!(
        stdout(&imported),
        "{\n  \"availability\": \"supplied historical timestamps\",\n  \"inserted_versions\": 1,\n  \"received\": 1\n}\n"
    );
    let doctor = crowdwatch(&dir, &["doctor", "--config", "config.json"]);
    let report = load_json(&stdout(&doctor)).unwrap();
    assert_eq!(report.index("integrity").unwrap().as_str(), Some("ok"));
    assert!(report
        .index("platform")
        .unwrap()
        .as_str()
        .unwrap()
        .contains('-'));
}

#[test]
fn verify_skeletrace_needs_the_host_program() {
    let dir = temp_dir("verify-cli");
    let out = crowdwatch(
        &dir,
        &[
            "verify-skeletrace",
            "profile.json",
            "--host",
            "no-such-skeletrace-program",
        ],
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains(
        "crowdwatch verify-skeletrace: error: Skeletrace executable not found; pass --host with its full path"
    ));
}
