// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Every code file opens with whispr.dev's copyright line and the GPL-3.0-only SPDX tag,
//! after any `#!` line, and the manifest says the same.

use std::fs;
use std::path::{Path, PathBuf};

const COPYRIGHT: &str = "Copyright (C) 2026 whispr.dev";
const SPDX: &str = "SPDX-License-Identifier: GPL-3.0-only";

fn code_files(folder: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries {
        let path = entry.expect("read a folder entry").path();
        if path.is_dir() {
            code_files(&path, found);
        } else if matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("rs" | "ps1" | "sh" | "py")
        ) {
            found.push(path);
        }
    }
}

#[test]
fn every_code_file_carries_the_notice() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for folder in ["src", "tests", "scripts"] {
        code_files(&root.join(folder), &mut files);
    }
    assert!(
        files.len() >= 30,
        "expected every code file, found {}",
        files.len()
    );
    for path in &files {
        let text =
            fs::read_to_string(path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let mut lines = text.lines().peekable();
        if lines.peek().is_some_and(|line| line.starts_with("#!/")) {
            lines.next();
        }
        let marker = if path.extension().is_some_and(|ext| ext == "rs") {
            "//"
        } else {
            "#"
        };
        assert_eq!(
            lines.next(),
            Some(format!("{marker} {COPYRIGHT}").as_str()),
            "{} must open with the copyright line",
            path.display()
        );
        assert_eq!(
            lines.next(),
            Some(format!("{marker} {SPDX}").as_str()),
            "{} must carry the SPDX line second",
            path.display()
        );
    }
}

#[test]
fn the_manifest_names_the_holder_and_licence() {
    let manifest = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .expect("read Cargo.toml");
    assert!(manifest.contains("authors = [\"whispr.dev\"]"));
    assert!(manifest.contains("license = \"GPL-3.0-only\""));
}
