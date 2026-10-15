// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! The `crowdwatch` command line.
//!
//! Commands, options, help and error messages follow 0.1.0's Python `argparse` parser:
//! long options may be shortened to any unique prefix, `--option=value` works, a usage
//! error exits with code 2 and a failed command with code 1. `collect` exits with 2 when a
//! source failed. One command is new: `verify-skeletrace`, which replaces the old
//! `tools/verify_skeletrace.py` script.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::analysis::{analyse, backtest};
use crate::config::{pure_path, resolve, Config};
use crate::demo::{create_demo, starter_config};
use crate::error::{Fail, Result};
use crate::export::export_skeletrace;
use crate::http::{Env, Http, HttpClient, ProcessEnv};
use crate::json::{int_value, py_eq, Dict, Value};
use crate::model::{atomic_write, finite, json_text, Bound};
use crate::pyfmt::{float_from_str, int_from_str, repr_str, splitlines};
use crate::pytime::{parse_time_str, span_of, Instant, Num};
use crate::report::render;
use crate::sources::{canonical_records, collect_sources};
use crate::store::Store;

/// The GNU-style notice under the version number.
pub const NOTICE: &str = "\
Copyright (C) 2026 whispr.dev
License GPL-3.0-only: GNU General Public License, version 3 <https://www.gnu.org/licenses/gpl-3.0.html>
This is free software: you are free to change and redistribute it.
There is NO WARRANTY, to the extent permitted by law.";

const MAIN_USAGE: &str = "\
usage: crowdwatch [-h] [--version]
                  {demo,init,collect,import,analyse,report,export-skeletrace,backtest,doctor,watch,prune,verify-skeletrace} ...";

const MAIN_HELP: &str = "\
usage: crowdwatch [-h] [--version]
                  {demo,init,collect,import,analyse,report,export-skeletrace,backtest,doctor,watch,prune,verify-skeletrace} ...

Aggregate movement observatory for Skeletrace
Copyright (C) 2026 whispr.dev. Free software under GPL-3.0-only, with NO WARRANTY.

positional arguments:
  {demo,init,collect,import,analyse,report,export-skeletrace,backtest,doctor,watch,prune,verify-skeletrace}
    demo                create an offline synthetic demonstration in a new
                        directory
    init                write a runnable public-source configuration
    verify-skeletrace   check an export with the SkeleTrace program installed
                        here

options:
  -h, --help            show this help message and exit
  --version             show program's version number and exit";

const COMMANDS: [&str; 12] = [
    "demo",
    "init",
    "collect",
    "import",
    "analyse",
    "report",
    "export-skeletrace",
    "backtest",
    "doctor",
    "watch",
    "prune",
    "verify-skeletrace",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Text,
    Path,
    Float,
    Int,
    Flag,
}

struct Opt {
    flag: &'static str,
    kind: Kind,
    required: bool,
    choices: &'static [&'static str],
}

const fn opt(flag: &'static str, kind: Kind) -> Opt {
    Opt {
        flag,
        kind,
        required: false,
        choices: &[],
    }
}

const fn required(flag: &'static str, kind: Kind) -> Opt {
    Opt {
        flag,
        kind,
        required: true,
        choices: &[],
    }
}

const CONFIG: Opt = required("--config", Kind::Path);
const AT: Opt = opt("--at", Kind::Text);

struct Spec {
    name: &'static str,
    options: &'static [Opt],
    positionals: &'static [&'static str],
    /// Where each positional sits among the options, for the "required" message.
    positional_after: usize,
    usage: &'static str,
    help: &'static str,
}

const SPECS: [Spec; 12] = [
    Spec {
        name: "demo",
        options: &[opt("--out", Kind::Text), AT],
        positionals: &[],
        positional_after: 0,
        usage: "usage: crowdwatch demo [-h] [--out OUT] [--at AT]",
        help: "\
usage: crowdwatch demo [-h] [--out OUT] [--at AT]

options:
  -h, --help  show this help message and exit
  --out OUT
  --at AT     optional fixed UTC timestamp for reproducibility",
    },
    Spec {
        name: "init",
        options: &[
            opt("--out", Kind::Text),
            Opt {
                flag: "--preset",
                kind: Kind::Text,
                required: false,
                choices: &["london", "melbourne", "new-york"],
            },
        ],
        positionals: &[],
        positional_after: 0,
        usage: "usage: crowdwatch init [-h] [--out OUT] [--preset {london,melbourne,new-york}]",
        help: "\
usage: crowdwatch init [-h] [--out OUT] [--preset {london,melbourne,new-york}]

options:
  -h, --help            show this help message and exit
  --out OUT
  --preset {london,melbourne,new-york}",
    },
    Spec {
        name: "collect",
        options: &[
            CONFIG,
            opt("--source", Kind::Text),
            opt("--force", Kind::Flag),
        ],
        positionals: &[],
        positional_after: 0,
        usage: "usage: crowdwatch collect [-h] --config CONFIG [--source SOURCE] [--force]",
        help: "\
usage: crowdwatch collect [-h] --config CONFIG [--source SOURCE] [--force]

options:
  -h, --help       show this help message and exit
  --config CONFIG
  --source SOURCE  collect one configured source
  --force          ignore next-due time, preserving in-flight claims",
    },
    Spec {
        name: "import",
        options: &[
            CONFIG,
            Opt {
                flag: "--format",
                kind: Kind::Text,
                required: false,
                choices: &["canonical_json", "canonical_csv"],
            },
            opt("--historical-availability", Kind::Flag),
        ],
        positionals: &["file"],
        positional_after: 1,
        usage: "\
usage: crowdwatch import [-h] --config CONFIG
                         [--format {canonical_json,canonical_csv}]
                         [--historical-availability]
                         file",
        help: "\
usage: crowdwatch import [-h] --config CONFIG
                         [--format {canonical_json,canonical_csv}]
                         [--historical-availability]
                         file

positional arguments:
  file

options:
  -h, --help            show this help message and exit
  --config CONFIG
  --format {canonical_json,canonical_csv}
  --historical-availability
                        trust the supplied retrieved_at times for replay;
                        ordinary import records availability now",
    },
    Spec {
        name: "analyse",
        options: &[CONFIG, AT, opt("--out", Kind::Path)],
        positionals: &[],
        positional_after: 0,
        usage: "usage: crowdwatch analyse [-h] --config CONFIG [--at AT] [--out OUT]",
        help: "\
usage: crowdwatch analyse [-h] --config CONFIG [--at AT] [--out OUT]

options:
  -h, --help       show this help message and exit
  --config CONFIG
  --at AT          as-of timestamp; only observations available by then are
                   used
  --out OUT",
    },
    Spec {
        name: "report",
        options: &[CONFIG, AT, required("--out", Kind::Path)],
        positionals: &[],
        positional_after: 0,
        usage: "usage: crowdwatch report [-h] --config CONFIG [--at AT] --out OUT",
        help: "\
usage: crowdwatch report [-h] --config CONFIG [--at AT] --out OUT

options:
  -h, --help       show this help message and exit
  --config CONFIG
  --at AT          as-of timestamp; only observations available by then are
                   used
  --out OUT",
    },
    Spec {
        name: "export-skeletrace",
        options: &[CONFIG, AT, required("--out", Kind::Path)],
        positionals: &[],
        positional_after: 0,
        usage: "usage: crowdwatch export-skeletrace [-h] --config CONFIG [--at AT] --out OUT",
        help: "\
usage: crowdwatch export-skeletrace [-h] --config CONFIG [--at AT] --out OUT

options:
  -h, --help       show this help message and exit
  --config CONFIG
  --at AT          as-of timestamp; only observations available by then are
                   used
  --out OUT",
    },
    Spec {
        name: "backtest",
        options: &[
            CONFIG,
            AT,
            opt("--out", Kind::Path),
            opt("--horizon", Kind::Float),
        ],
        positionals: &[],
        positional_after: 0,
        usage: "\
usage: crowdwatch backtest [-h] --config CONFIG [--at AT] [--out OUT]
                           [--horizon HORIZON]",
        help: "\
usage: crowdwatch backtest [-h] --config CONFIG [--at AT] [--out OUT]
                           [--horizon HORIZON]

options:
  -h, --help         show this help message and exit
  --config CONFIG
  --at AT            as-of timestamp; only observations available by then are
                     used
  --out OUT
  --horizon HORIZON",
    },
    Spec {
        name: "doctor",
        options: &[CONFIG],
        positionals: &[],
        positional_after: 0,
        usage: "usage: crowdwatch doctor [-h] --config CONFIG",
        help: "\
usage: crowdwatch doctor [-h] --config CONFIG

options:
  -h, --help       show this help message and exit
  --config CONFIG",
    },
    Spec {
        name: "watch",
        options: &[
            CONFIG,
            opt("--source", Kind::Text),
            opt("--force", Kind::Flag),
            required("--out", Kind::Path),
            opt("--sleep", Kind::Float),
            opt("--cycles", Kind::Int),
            opt("--skeletrace", Kind::Flag),
        ],
        positionals: &[],
        positional_after: 0,
        usage: "\
usage: crowdwatch watch [-h] --config CONFIG [--source SOURCE] [--force]
                        --out OUT [--sleep SLEEP] [--cycles CYCLES]
                        [--skeletrace]",
        help: "\
usage: crowdwatch watch [-h] --config CONFIG [--source SOURCE] [--force]
                        --out OUT [--sleep SLEEP] [--cycles CYCLES]
                        [--skeletrace]

options:
  -h, --help       show this help message and exit
  --config CONFIG
  --source SOURCE  collect one configured source
  --force          ignore next-due time, preserving in-flight claims
  --out OUT
  --sleep SLEEP    scheduler check period; source polling intervals still
                   apply
  --cycles CYCLES  0 keeps running until Ctrl+C
  --skeletrace     also create native snapshot exports each cycle",
    },
    Spec {
        name: "prune",
        options: &[CONFIG, opt("--keep-days", Kind::Int)],
        positionals: &[],
        positional_after: 0,
        usage: "usage: crowdwatch prune [-h] --config CONFIG [--keep-days KEEP_DAYS]",
        help: "\
usage: crowdwatch prune [-h] --config CONFIG [--keep-days KEEP_DAYS]

options:
  -h, --help            show this help message and exit
  --config CONFIG
  --keep-days KEEP_DAYS",
    },
    Spec {
        name: "verify-skeletrace",
        options: &[opt("--host", Kind::Text)],
        positionals: &["profile"],
        positional_after: 0,
        usage: "usage: crowdwatch verify-skeletrace [-h] [--host HOST] profile",
        help: "\
usage: crowdwatch verify-skeletrace [-h] [--host HOST] profile

positional arguments:
  profile      the skeletrace-profile.json an export wrote

options:
  -h, --help   show this help message and exit
  --host HOST  path to skeletrace.exe or command on PATH",
    },
];

// ---------------------------------------------------------------------------------------
// Parsing, as argparse parses
// ---------------------------------------------------------------------------------------

/// How a parse ended early.
enum Stop {
    /// Print this text to standard output and exit 0 (help or version).
    Show(String),
    /// A usage error: the usage text and the message, exit 2.
    Usage {
        usage: &'static str,
        prog: String,
        message: String,
    },
}

fn usage_error(spec: Option<&Spec>, message: String) -> Stop {
    match spec {
        Some(spec) => Stop::Usage {
            usage: spec.usage,
            prog: format!("crowdwatch {}", spec.name),
            message,
        },
        None => Stop::Usage {
            usage: MAIN_USAGE,
            prog: "crowdwatch".to_owned(),
            message,
        },
    }
}

/// A value given on the command line, converted as argparse's `type=` converted it.
#[derive(Clone, Debug)]
enum Arg {
    Text(String),
    Path(PathBuf),
    Float(f64),
    Int(Value),
    Flag,
}

/// The parsed arguments of a subcommand.
struct Args {
    command: &'static str,
    values: Vec<(&'static str, Arg)>,
    positionals: Vec<String>,
}

impl Args {
    fn text(&self, flag: &str) -> Option<&str> {
        self.values
            .iter()
            .rev()
            .find(|(f, _)| *f == flag)
            .and_then(|(_, v)| match v {
                Arg::Text(t) => Some(t.as_str()),
                _ => None,
            })
    }

    fn path(&self, flag: &str) -> Option<&Path> {
        self.values
            .iter()
            .rev()
            .find(|(f, _)| *f == flag)
            .and_then(|(_, v)| match v {
                Arg::Path(p) => Some(p.as_path()),
                _ => None,
            })
    }

    fn flag(&self, flag: &str) -> bool {
        self.values
            .iter()
            .any(|(f, v)| *f == flag && matches!(v, Arg::Flag))
    }

    fn get(&self, flag: &str) -> Option<&Arg> {
        self.values
            .iter()
            .rev()
            .find(|(f, _)| *f == flag)
            .map(|(_, v)| v)
    }
}

/// What `_parse_optional` made of an argument.
enum Parsed<'a> {
    Positional,
    /// A known option (`None` is `-h`/`--help`), with any `=value` part.
    Known(Option<&'a Opt>, Option<String>),
    Unknown,
}

/// `-5`, `-.5`, `-1e3`: argparse reads these as values, not options.
fn looks_negative(arg: &str) -> bool {
    let rest = arg.strip_prefix('-').unwrap_or("");
    let rest = rest.strip_prefix('.').unwrap_or(rest);
    rest.starts_with(|c: char| c.is_ascii_digit())
}

/// argparse's `_parse_optional` for one argument of a subcommand (or, with no spec, of
/// the main parser, whose options are `-h`, `--help` and `--version`).
fn parse_optional<'a>(spec: Option<&'a Spec>, arg: &str) -> std::result::Result<Parsed<'a>, Stop> {
    if !arg.starts_with('-') || arg == "-" {
        return Ok(Parsed::Positional);
    }
    let options: &'a [Opt] = spec.map_or(&[], |s| s.options);
    let main_version = spec.is_none();
    let exact = |name: &str| -> Option<Parsed<'a>> {
        if name == "-h" || name == "--help" {
            return Some(Parsed::Known(None, None));
        }
        if main_version && name == "--version" {
            return Some(Parsed::Known(Some(&VERSION_OPT), None));
        }
        options
            .iter()
            .find(|o| o.flag == name)
            .map(|o| Parsed::Known(Some(o), None))
    };
    let with_value = |parsed: Parsed<'a>, value: Option<String>| match parsed {
        Parsed::Known(o, _) => Parsed::Known(o, value),
        other => other,
    };
    if let Some(found) = exact(arg) {
        return Ok(found);
    }
    if let Some((name, value)) = arg.split_once('=') {
        if let Some(found) = exact(name) {
            return Ok(with_value(found, Some(value.to_owned())));
        }
    }
    // Unique prefixes of long options.
    if arg.starts_with("--") {
        let (prefix, value) = match arg.split_once('=') {
            Some((p, v)) => (p, Some(v.to_owned())),
            None => (arg, None),
        };
        let mut names: Vec<&str> = vec!["--help"];
        if main_version {
            names.push("--version");
        }
        names.extend(options.iter().map(|o| o.flag));
        // argparse lists option strings in the order they were added: -h, --help first.
        let matches: Vec<&str> = names
            .into_iter()
            .filter(|n| n.starts_with(prefix))
            .collect();
        if matches.len() > 1 {
            return Err(usage_error(
                spec,
                format!(
                    "ambiguous option: {prefix} could match {}",
                    matches.join(", ")
                ),
            ));
        }
        if let Some(name) = matches.first() {
            if let Some(found) = exact(name) {
                return Ok(with_value(found, value));
            }
        }
    } else if let Some(rest) = arg.strip_prefix("-h") {
        // `-hX`: the short option with an attached value.
        if !rest.is_empty() && !arg.starts_with("--") {
            return Ok(Parsed::Known(None, Some(rest.to_owned())));
        }
    }
    if looks_negative(arg) || arg.contains(' ') {
        return Ok(Parsed::Positional);
    }
    Ok(Parsed::Unknown)
}

const VERSION_OPT: Opt = opt("--version", Kind::Flag);

fn convert(spec: &Spec, option: &Opt, raw: String) -> std::result::Result<Arg, Stop> {
    let value = match option.kind {
        Kind::Text => Arg::Text(raw.clone()),
        // argparse's `type=Path`.
        Kind::Path => Arg::Path(pure_path(&raw)),
        Kind::Float => Arg::Float(float_from_str(&raw).map_err(|_| {
            usage_error(
                Some(spec),
                format!(
                    "argument {}: invalid float value: {}",
                    option.flag,
                    repr_str(&raw)
                ),
            )
        })?),
        Kind::Int => Arg::Int(int_value(int_from_str(&raw).map_err(|_| {
            usage_error(
                Some(spec),
                format!(
                    "argument {}: invalid int value: {}",
                    option.flag,
                    repr_str(&raw)
                ),
            )
        })?)),
        Kind::Flag => Arg::Flag,
    };
    if !option.choices.is_empty() && !option.choices.contains(&raw.as_str()) {
        return Err(usage_error(
            Some(spec),
            format!(
                "argument {}: invalid choice: {} (choose from {})",
                option.flag,
                repr_str(&raw),
                option.choices.join(", ")
            ),
        ));
    }
    Ok(value)
}

/// Parse a subcommand's arguments; return them and any it did not recognise.
fn parse_command(
    spec: &'static Spec,
    args: &[String],
) -> std::result::Result<(Args, Vec<String>), Stop> {
    // argparse classifies every argument first, so an ambiguous option anywhere fails
    // before anything else is looked at.
    let mut kinds = Vec::with_capacity(args.len());
    let mut after_dashes = false;
    for arg in args {
        if after_dashes {
            kinds.push(Parsed::Positional);
        } else if arg == "--" {
            after_dashes = true;
            kinds.push(Parsed::Positional);
        } else {
            kinds.push(parse_optional(Some(spec), arg)?);
        }
    }
    let mut parsed = Args {
        command: spec.name,
        values: Vec::new(),
        positionals: Vec::new(),
    };
    let mut extras = Vec::new();
    let mut seen_dashes = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match &kinds[i] {
            Parsed::Positional => {
                if arg == "--" && !seen_dashes {
                    seen_dashes = true;
                } else if parsed.positionals.len() < spec.positionals.len() {
                    parsed.positionals.push(arg.clone());
                } else {
                    extras.push(arg.clone());
                }
                i += 1;
            }
            Parsed::Unknown => {
                extras.push(arg.clone());
                i += 1;
            }
            Parsed::Known(None, explicit) => {
                if let Some(value) = explicit {
                    return Err(usage_error(
                        Some(spec),
                        format!(
                            "argument -h/--help: ignored explicit argument {}",
                            repr_str(value)
                        ),
                    ));
                }
                return Err(Stop::Show(spec.help.to_owned()));
            }
            Parsed::Known(Some(option), explicit) => {
                if option.kind == Kind::Flag {
                    if let Some(value) = explicit {
                        return Err(usage_error(
                            Some(spec),
                            format!(
                                "argument {}: ignored explicit argument {}",
                                option.flag,
                                repr_str(value)
                            ),
                        ));
                    }
                    parsed.values.push((option.flag, Arg::Flag));
                    i += 1;
                    continue;
                }
                let raw = match explicit {
                    Some(value) => value.clone(),
                    None => {
                        let next_is_value = matches!(kinds.get(i + 1), Some(Parsed::Positional))
                            && args.get(i + 1).is_some_and(|a| a != "--");
                        if !next_is_value {
                            return Err(usage_error(
                                Some(spec),
                                format!("argument {}: expected one argument", option.flag),
                            ));
                        }
                        i += 1;
                        args[i].clone()
                    }
                };
                let value = convert(spec, option, raw)?;
                parsed.values.push((option.flag, value));
                i += 1;
            }
        }
    }
    let mut missing: Vec<&str> = Vec::new();
    for (index, option) in spec.options.iter().enumerate() {
        if index == spec.positional_after {
            for (p, name) in spec.positionals.iter().enumerate() {
                if parsed.positionals.len() <= p {
                    missing.push(name);
                }
            }
        }
        if option.required && parsed.get(option.flag).is_none() {
            missing.push(option.flag);
        }
    }
    if spec.positional_after >= spec.options.len() {
        for (p, name) in spec.positionals.iter().enumerate() {
            if parsed.positionals.len() <= p {
                missing.push(name);
            }
        }
    }
    if !missing.is_empty() {
        return Err(usage_error(
            Some(spec),
            format!(
                "the following arguments are required: {}",
                missing.join(", ")
            ),
        ));
    }
    Ok((parsed, extras))
}

/// Parse the whole command line.
fn parse(args: &[String]) -> std::result::Result<Args, Stop> {
    let mut extras: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match parse_optional(None, arg)? {
            Parsed::Positional => {
                let Some(spec) = SPECS.iter().find(|s| s.name == arg) else {
                    return Err(usage_error(
                        None,
                        format!(
                            "argument command: invalid choice: {} (choose from {})",
                            repr_str(arg),
                            COMMANDS.join(", ")
                        ),
                    ));
                };
                let (parsed, more) = parse_command(spec, &args[i + 1..])?;
                extras.extend(more);
                if !extras.is_empty() {
                    return Err(usage_error(
                        None,
                        format!("unrecognized arguments: {}", extras.join(" ")),
                    ));
                }
                return Ok(parsed);
            }
            Parsed::Known(None, explicit) => {
                if let Some(value) = explicit {
                    return Err(usage_error(
                        None,
                        format!(
                            "argument -h/--help: ignored explicit argument {}",
                            repr_str(&value)
                        ),
                    ));
                }
                return Err(Stop::Show(MAIN_HELP.to_owned()));
            }
            Parsed::Known(Some(_), explicit) => {
                if let Some(value) = explicit {
                    return Err(usage_error(
                        None,
                        format!(
                            "argument --version: ignored explicit argument {}",
                            repr_str(&value)
                        ),
                    ));
                }
                return Err(Stop::Show(format!(
                    "{}\n{NOTICE}",
                    env!("CARGO_PKG_VERSION")
                )));
            }
            Parsed::Unknown => extras.push(arg.clone()),
        }
        i += 1;
    }
    Err(usage_error(
        None,
        "the following arguments are required: command".to_owned(),
    ))
}

// ---------------------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------------------

/// What a command run depends on from outside, so tests can stand in for it.
pub struct Runtime {
    pub env: Box<dyn Env>,
    /// Makes the HTTP client for a collection.
    pub http: Box<dyn Fn() -> Box<dyn Http>>,
    /// The current time.
    pub now: Box<dyn Fn() -> Instant>,
    /// Waits between watch cycles.
    pub sleep: Box<dyn Fn(f64)>,
}

impl Runtime {
    /// The real environment, network, clock and sleep.
    pub fn standard() -> Runtime {
        Runtime {
            env: Box::new(ProcessEnv),
            http: Box::new(|| Box::new(HttpClient::from_env()) as Box<dyn Http>),
            now: Box::new(Instant::now),
            sleep: Box::new(|seconds| {
                if seconds.is_finite() && seconds > 0.0 {
                    std::thread::sleep(Duration::from_secs_f64(seconds));
                }
            }),
        }
    }
}

struct Output<'a> {
    out: &'a mut dyn Write,
    err: &'a mut dyn Write,
}

impl Output<'_> {
    fn print(&mut self, text: &str) -> Result<()> {
        writeln!(self.out, "{text}").map_err(Fail::from)?;
        self.out.flush().map_err(Fail::from)
    }

    /// 0.1.0's `emit(obj, path)`: pretty JSON to a file (printing its full path) or to
    /// standard output.
    fn emit(&mut self, value: &Value, path: Option<&Path>) -> Result<()> {
        let body = json_text(value, true)? + "\n";
        match path {
            Some(path) => {
                atomic_write(path, &body)?;
                self.print(&resolve(path).to_string_lossy())
            }
            None => {
                self.out.write_all(body.as_bytes()).map_err(Fail::from)?;
                self.out.flush().map_err(Fail::from)
            }
        }
    }
}

/// Run the command line (arguments without the program name); return the exit code.
pub fn run(args: &[String], rt: &Runtime, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let mut output = Output { out, err };
    let parsed = match parse(args) {
        Ok(parsed) => parsed,
        Err(Stop::Show(text)) => {
            return if output.print(&text).is_ok() { 0 } else { 1 };
        }
        Err(Stop::Usage {
            usage,
            prog,
            message,
        }) => {
            let _ = writeln!(output.err, "{usage}\n{prog}: error: {message}");
            let _ = output.err.flush();
            return 2;
        }
    };
    match execute(&parsed, rt, &mut output) {
        Ok(code) => code,
        Err(Stop2::Usage(message)) => {
            let spec = SPECS.iter().find(|s| s.name == parsed.command);
            let usage = spec.map_or(MAIN_USAGE, |s| s.usage);
            let _ = writeln!(
                output.err,
                "{usage}\ncrowdwatch {}: error: {message}",
                parsed.command
            );
            2
        }
        Err(Stop2::Fail(fail)) => {
            let _ = writeln!(output.err, "crowdwatch: {fail}");
            let _ = output.err.flush();
            1
        }
        Err(Stop2::Code(code)) => code,
    }
}

/// How a command ended other than with an exit code.
enum Stop2 {
    /// A usage error found after parsing (verify-skeletrace's own checks).
    Usage(String),
    Fail(Fail),
    /// A code to exit with, the message already written.
    Code(i32),
}

impl From<Fail> for Stop2 {
    fn from(fail: Fail) -> Stop2 {
        Stop2::Fail(fail)
    }
}

/// `parse_time(args.at) if args.at else default`.
fn at_or(args: &Args, default: impl FnOnce() -> Instant) -> Result<Instant> {
    match args.text("--at") {
        Some(text) if !text.is_empty() => parse_time_str(text),
        _ => Ok(default()),
    }
}

fn execute(args: &Args, rt: &Runtime, output: &mut Output<'_>) -> std::result::Result<i32, Stop2> {
    match args.command {
        "demo" => {
            let out = args.text("--out").unwrap_or("crowdwatch-demo");
            let at = match args.text("--at") {
                Some(text) if !text.is_empty() => Some(parse_time_str(text)?),
                _ => None,
            };
            let result = create_demo(&pure_path(out), at)?;
            output.emit(&result, None)?;
            return Ok(0);
        }
        "init" => {
            let out = pure_path(args.text("--out").unwrap_or("crowdwatch-config.json"));
            if out.exists() {
                return Err(Fail::input(
                    "configuration already exists; choose a new output filename",
                )
                .into());
            }
            let preset = args.text("--preset").unwrap_or("london");
            output.emit(&starter_config(preset), Some(&out))?;
            return Ok(0);
        }
        "verify-skeletrace" => return verify_skeletrace(args, output),
        _ => {}
    }
    let config_path = args.path("--config").unwrap_or(Path::new("."));
    let mut config = Config::load(config_path)?;
    let mut store = Store::open(&config.database)?;
    let now = at_or(args, || (rt.now)())?;
    match args.command {
        "collect" => {
            let mut client = (rt.http)();
            let result = collect_sources(
                &config,
                &mut store,
                now,
                args.flag("--force"),
                args.text("--source"),
                client.as_mut(),
                rt.env.as_ref(),
            )?;
            let failed = result.iter().any(|r| {
                r.as_dict()
                    .and_then(|d| d.get("status"))
                    .is_some_and(|s| py_eq(s, &Value::from("error")))
            });
            output.emit(
                &Value::Dict(Dict::new().with("sources", Value::List(result))),
                None,
            )?;
            Ok(if failed { 2 } else { 0 })
        }
        "import" => {
            let body = read_import(&pure_path(&args.positionals[0]))?;
            let format = args.text("--format").unwrap_or("canonical_json");
            let mut rows = canonical_records(&body, format)?;
            let historical = args.flag("--historical-availability");
            if !historical {
                rows = rows
                    .into_iter()
                    .map(|mut o| {
                        o.retrieved_at = now.stamp();
                        o.revalidated()
                    })
                    .collect::<Result<Vec<_>>>()?;
            }
            for o in &rows {
                let matches = config.source(&o.source_id).is_some_and(|s| {
                    s.get("region_id").and_then(Value::as_str) == Some(o.region_id.as_str())
                });
                if !matches {
                    return Err(Fail::input(
                        "import record references an unconfigured source or mismatched region",
                    )
                    .into());
                }
                if parse_time_str(&o.retrieved_at)? > now {
                    return Err(Fail::input("import contains data retrieved in the future").into());
                }
            }
            let inserted = store.ingest(&rows)?;
            output.emit(
                &Value::Dict(
                    Dict::new()
                        .with("received", rows.len())
                        .with("inserted_versions", inserted)
                        .with(
                            "availability",
                            if historical {
                                "supplied historical timestamps"
                            } else {
                                "now"
                            },
                        ),
                ),
                None,
            )?;
            Ok(0)
        }
        "doctor" => {
            let enabled: Vec<Value> = config
                .sources
                .iter()
                .filter(|(id, _)| config.enabled(id))
                .map(|(id, _)| Value::from(id.as_str()))
                .collect();
            let report = Dict::new()
                .with("version", env!("CARGO_PKG_VERSION"))
                .with(
                    "platform",
                    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
                )
                .with("database", config.database.to_string_lossy().into_owned())
                .with("counts", store.counts()?)
                .with("integrity", store.quick_check()?)
                .with("sources", Value::List(store.health()?))
                .with("enabled_sources", Value::List(enabled));
            output.emit(&Value::Dict(report), None)?;
            Ok(0)
        }
        "prune" => {
            let keep = match args.get("--keep-days") {
                Some(Arg::Int(v)) => v.clone(),
                _ => Value::Int(35),
            };
            // keep_days < max(2, history_days + 1)
            let history = config.setting("history_days");
            let floor = match history {
                Num::Int(d) => Num::Int(d.saturating_add(1)),
                Num::Float(d) => Num::Float(d + 1.0),
            };
            let floor = if floor.as_f64() > 2.0 {
                floor
            } else {
                Num::Int(2)
            };
            let too_short = match &keep {
                Value::Int(k) => (*k as f64) < floor.as_f64(),
                Value::BigInt(digits) => digits.starts_with('-'),
                _ => false,
            };
            if too_short {
                return Err(Fail::input(
                    "keep-days must exceed the baseline history window by at least one day",
                )
                .into());
            }
            let days = match keep {
                Value::Int(k) => k,
                _ => return Err(Fail::overflow("Python int too large to convert to C int").into()),
            };
            let before = now.sub(span_of(Num::Int(days), 86_400_000_000)?)?;
            let deleted = store.prune(before)?;
            output.emit(
                &Value::Dict(Dict::new().with("deleted_versions", deleted)),
                None,
            )?;
            Ok(0)
        }
        "backtest" => {
            let horizon = match args.get("--horizon") {
                Some(Arg::Float(h)) => Num::Float(*h),
                _ => Num::Int(15),
            };
            finite(
                &horizon.to_value(),
                "horizon",
                Bound(1.0, "1"),
                Bound(30.0, "30"),
            )?;
            let result = backtest(&config, &store, now, horizon)?;
            output.emit(&result, args.path("--out"))?;
            Ok(0)
        }
        "watch" => {
            let sleep = match args.get("--sleep") {
                Some(Arg::Float(s)) => Num::Float(*s),
                _ => Num::Int(60),
            };
            finite(
                &sleep.to_value(),
                "scheduler sleep",
                Bound(5.0, "5"),
                Bound(3600.0, "3600"),
            )?;
            let cycles = match args.get("--cycles") {
                Some(Arg::Int(Value::Int(n))) => *n,
                Some(Arg::Int(Value::BigInt(d))) if d.starts_with('-') => -1,
                Some(Arg::Int(_)) => i64::MAX,
                _ => 0,
            };
            if cycles < 0 {
                return Err(Fail::input("cycles must be nonnegative").into());
            }
            let out = args.path("--out").unwrap_or(Path::new("."));
            let mut cycle = 0_i64;
            while cycles == 0 || cycle < cycles {
                // Reload settings each cycle so enabling or disabling a source takes effect.
                let fresh = Config::load(config_path)?;
                if fresh.database != config.database {
                    return Err(Fail::input(
                        "database path changed; restart watch to use the new database",
                    )
                    .into());
                }
                config = fresh;
                cycle += 1;
                let mut client = (rt.http)();
                let result = collect_sources(
                    &config,
                    &mut store,
                    (rt.now)(),
                    args.flag("--force") && cycle == 1,
                    args.text("--source"),
                    client.as_mut(),
                    rt.env.as_ref(),
                )?;
                let report = analyse(&config, &store, (rt.now)())?;
                atomic_write(
                    &out.join("report.json"),
                    &(json_text(&report, true)? + "\n"),
                )?;
                render(&report, &out.join("report.html"))?;
                if args.flag("--skeletrace") {
                    export_skeletrace(&report, &out.join("skeletrace"))?;
                }
                output.emit(
                    &Value::Dict(
                        Dict::new()
                            .with("cycle", cycle)
                            .with("sources", Value::List(result))
                            .with(
                                "report",
                                resolve(&out.join("report.html"))
                                    .to_string_lossy()
                                    .into_owned(),
                            ),
                    ),
                    None,
                )?;
                if cycles == 0 || cycle < cycles {
                    (rt.sleep)(sleep.as_f64());
                }
            }
            Ok(0)
        }
        command => {
            let report = analyse(&config, &store, now)?;
            match command {
                "analyse" => output.emit(&report, args.path("--out"))?,
                "report" => {
                    let out = args.path("--out").unwrap_or(Path::new("."));
                    render(&report, out)?;
                    output.print(&resolve(out).to_string_lossy())?;
                }
                _ => {
                    let out = args.path("--out").unwrap_or(Path::new("."));
                    let manifest = export_skeletrace(&report, out)?;
                    output.emit(&manifest, None)?;
                }
            }
            Ok(0)
        }
    }
}

/// `args.file.open("rb").read(5 MiB + 1)`.
fn read_import(file: &Path) -> Result<Vec<u8>> {
    crate::model::read_file(file, Some(5 * 1024 * 1024 + 1))
}

// ---------------------------------------------------------------------------------------
// verify-skeletrace
// ---------------------------------------------------------------------------------------

/// `shutil.which(name)`, which 0.1.0's verification helper called (Python 3.12 to
/// 3.14 behave alike). A name with a folder in it is tried where it points; any other
/// name in each `PATH` folder in turn. On Windows the current folder is searched first
/// (unless `NoDefaultCurrentDirectoryInExePath` is set) and the `PATHEXT` endings are
/// tried, the bare name only when it already ends with one of them.
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH");
    let pathext = std::env::var("PATHEXT").ok();
    let search = Which {
        windows: cfg!(windows),
        path: path.as_deref(),
        pathext: pathext.as_deref(),
        current_dir: std::env::var_os("NoDefaultCurrentDirectoryInExePath")
            .is_none_or(|value| value.is_empty()),
    };
    search
        .candidates(name)
        .into_iter()
        .map(PathBuf::from)
        .find(|candidate| runnable(candidate))
}

/// Python's `_access_check`: the path exists, may be run, and is not a folder.
fn runnable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if meta.is_dir() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// What `shutil.which` reads from the system, gathered so its Windows rules can be
/// tested anywhere.
struct Which<'a> {
    windows: bool,
    path: Option<&'a std::ffi::OsStr>,
    pathext: Option<&'a str>,
    current_dir: bool,
}

impl Which<'_> {
    /// Every path `shutil.which(name)` tries, in its order.
    fn candidates(&self, name: &str) -> Vec<std::ffi::OsString> {
        use std::ffi::OsString;
        let (dirname, cmd) = if self.windows {
            nt_split(name)
        } else {
            let (head, tail) = posix_split(name);
            (head.to_owned(), tail)
        };
        let dirs: Vec<OsString> = if dirname.is_empty() {
            let path: OsString = match self.path {
                Some(path) => path.to_owned(),
                // os.defpath on Windows; elsewhere the C library's default (CS_PATH).
                None if self.windows => ".;C:\\bin".into(),
                None if cfg!(target_os = "macos") => "/usr/bin:/bin:/usr/sbin:/sbin".into(),
                None => "/bin:/usr/bin".into(),
            };
            if path.is_empty() {
                return Vec::new();
            }
            let mut dirs = split_os(&path, if self.windows { ';' } else { ':' });
            if self.windows && self.current_dir {
                dirs.insert(0, ".".into());
            }
            dirs
        } else {
            vec![OsString::from(dirname)]
        };
        let files: Vec<String> = if self.windows {
            let source = match self.pathext {
                Some(text) if !text.is_empty() => text,
                _ => ".COM;.EXE;.BAT;.CMD;.VBS;.JS;.WS;.MSC",
            };
            let endings: Vec<&str> = source
                .split(';')
                .filter(|ending| !ending.is_empty())
                .map(|ending| ending.trim_end_matches('.'))
                .collect();
            let upper = cmd.to_uppercase();
            let mut files: Vec<String> = endings.iter().map(|e| format!("{cmd}{e}")).collect();
            if endings.iter().any(|e| upper.ends_with(&e.to_uppercase())) {
                files.insert(0, cmd.to_owned());
            }
            files
        } else {
            vec![cmd.to_owned()]
        };
        let mut seen: Vec<String> = Vec::new();
        let mut out = Vec::new();
        for dir in dirs {
            let text = dir.to_string_lossy();
            let key = if self.windows {
                text.replace('/', "\\").to_lowercase()
            } else {
                text.into_owned()
            };
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            for file in &files {
                out.push(join_os(&dir, file, self.windows));
            }
        }
        out
    }
}

/// `posixpath.split`: the folder part (trailing slashes removed unless it is all slashes)
/// and the last part.
fn posix_split(p: &str) -> (&str, &str) {
    let i = p.rfind('/').map_or(0, |i| i + 1);
    let (head, tail) = p.split_at(i);
    if head.chars().all(|c| c == '/') {
        (head, tail)
    } else {
        (head.trim_end_matches('/'), tail)
    }
}

/// `ntpath.split`: the folder part, keeping a drive and root whole, and the last part.
fn nt_split(p: &str) -> (String, &str) {
    let is_sep = |c: char| c == '\\' || c == '/';
    let drive_len = nt_drive_and_root(p);
    let rest = &p[drive_len..];
    let i = rest.rfind(is_sep).map_or(0, |i| i + 1);
    let (head, tail) = rest.split_at(i);
    (
        format!("{}{}", &p[..drive_len], head.trim_end_matches(is_sep)),
        tail,
    )
}

/// The length of the drive and root at the start of a Windows path (`ntpath.splitroot`).
fn nt_drive_and_root(p: &str) -> usize {
    let normal = p.replace('/', "\\");
    let (drive, root, _) = crate::config::nt_splitroot(&normal);
    drive.len() + root.len()
}

/// `os.path.join(folder, name)` for a plain file name.
fn join_os(dir: &std::ffi::OsStr, file: &str, windows: bool) -> std::ffi::OsString {
    let text = dir.to_string_lossy();
    let mut joined = dir.to_owned();
    let needs_sep = if windows {
        !(text.is_empty() || text.ends_with(['\\', '/', ':']))
    } else {
        !(text.is_empty() || text.ends_with('/'))
    };
    if needs_sep {
        joined.push(if windows { "\\" } else { "/" });
    }
    joined.push(file);
    joined
}

/// Split a `PATH` value at `sep`, keeping empty entries as Python's `str.split` does.
fn split_os(value: &std::ffi::OsStr, sep: char) -> Vec<std::ffi::OsString> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let sep = sep as u8;
        value
            .as_bytes()
            .split(|&byte| byte == sep)
            .map(|part| std::ffi::OsStr::from_bytes(part).to_owned())
            .collect()
    }
    #[cfg(not(unix))]
    {
        value
            .to_string_lossy()
            .split(sep)
            .map(std::ffi::OsString::from)
            .collect()
    }
}

/// What a finished host command printed.
struct Finished {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Run a program with a 60-second limit, collecting what it prints.
fn run_with_timeout(program: &Path, args: &[&str]) -> std::result::Result<Finished, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Fail::from(e).to_string())?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out_reader = std::thread::spawn(move || {
        let mut text = Vec::new();
        if let Some(pipe) = stdout.as_mut() {
            let _ = pipe.read_to_end(&mut text);
        }
        text
    });
    let err_reader = std::thread::spawn(move || {
        let mut text = Vec::new();
        if let Some(pipe) = stderr.as_mut() {
            let _ = pipe.read_to_end(&mut text);
        }
        text
    });
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= Duration::from_secs(60) => {
                let _ = child.kill();
                let _ = child.wait();
                let shown: Vec<String> = std::iter::once(program.to_string_lossy().into_owned())
                    .chain(args.iter().map(|a| (*a).to_owned()))
                    .map(|a| repr_str(&a))
                    .collect();
                return Err(format!(
                    "Command '[{}]' timed out after 60 seconds",
                    shown.join(", ")
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(err) => return Err(Fail::from(err).to_string()),
        }
    };
    let stdout = String::from_utf8_lossy(&out_reader.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err_reader.join().unwrap_or_default()).into_owned();
    Ok(Finished {
        code: status.code().unwrap_or(1),
        stdout,
        stderr,
    })
}

/// Check an export with the SkeleTrace program itself: `profile-validate`, then
/// `poll-source`, which must read every sample line.
fn verify_skeletrace(args: &Args, output: &mut Output<'_>) -> std::result::Result<i32, Stop2> {
    let host = args.text("--host").unwrap_or("skeletrace");
    let Some(executable) = which(host) else {
        return Err(Stop2::Usage(
            "Skeletrace executable not found; pass --host with its full path".to_owned(),
        ));
    };
    let profile_path = resolve(&pure_path(&args.positionals[0]));
    let failed = |output: &mut Output<'_>, message: String| {
        let _ = writeln!(output.err, "Host verification failed: {message}");
        Stop2::Code(1)
    };
    // `profile_path.read_text(encoding="utf-8")`.
    let text = match crate::model::read_file(&profile_path, None) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => {
                return Err(failed(
                    output,
                    "'utf-8' codec can't decode the profile".to_owned(),
                ))
            }
        },
        Err(err) => return Err(failed(output, err.to_string())),
    };
    let profile = match crate::json::parse_str(&text) {
        Ok(value) => value,
        Err(_) => return Err(failed(output, "the profile is not valid JSON".to_owned())),
    };
    let definition = profile
        .index("sources")
        .and_then(|s| s.index_at(0))
        .and_then(|first| first.index("definition").cloned());
    let definition = match definition {
        Ok(d) => d,
        Err(err) => return Err(failed(output, err.to_string())),
    };
    let (endpoint, source_id) = match (definition.index("endpoint"), definition.index("id")) {
        (Ok(Value::Str(e)), Ok(Value::Str(id))) => (e.clone(), id.clone()),
        (Err(err), _) | (_, Err(err)) => return Err(failed(output, err.to_string())),
        _ => {
            return Err(failed(
                output,
                "the profile's source is malformed".to_owned(),
            ))
        }
    };
    let sample_file = pure_path(&endpoint);
    if !sample_file.is_file() {
        return Err(Stop2::Usage(
            "sample path is missing; regenerate the export on this machine".to_owned(),
        ));
    }
    let samples = match crate::model::read_file(&sample_file, None) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => {
                return Err(failed(
                    output,
                    "'utf-8' codec can't decode the samples".to_owned(),
                ))
            }
        },
        Err(err) => return Err(failed(output, err.to_string())),
    };
    let universal = samples.replace("\r\n", "\n").replace('\r', "\n");
    let expected = splitlines(&universal).len();
    let profile_text = profile_path.to_string_lossy().into_owned();
    let mut last = None;
    for command in [
        vec!["profile-validate", profile_text.as_str()],
        vec!["poll-source", profile_text.as_str(), source_id.as_str()],
    ] {
        let finished = match run_with_timeout(&executable, &command) {
            Ok(finished) => finished,
            Err(message) => return Err(failed(output, message)),
        };
        if finished.code != 0 {
            let shown = if finished.stderr.is_empty() {
                &finished.stdout
            } else {
                &finished.stderr
            };
            let _ = writeln!(output.err, "{shown}");
            return Err(Stop2::Code(finished.code));
        }
        last = Some(finished);
    }
    let stdout = last.map(|f| f.stdout).unwrap_or_default();
    let response = match crate::json::parse_str(&stdout) {
        Ok(value) => value,
        Err(_) => {
            return Err(failed(
                output,
                "the host printed something other than JSON".to_owned(),
            ))
        }
    };
    let tick = response
        .get("Tick")
        .ok()
        .flatten()
        .cloned()
        .unwrap_or_else(|| Value::Dict(Dict::new()));
    let seen = tick.get("samples_seen").ok().flatten().cloned();
    let failed_sources = tick
        .get("failed_sources")
        .ok()
        .flatten()
        .cloned()
        .unwrap_or(Value::Int(0));
    let count_matches = seen.is_some_and(|s| py_eq(&s, &Value::from(expected)));
    if !count_matches || failed_sources.truthy() {
        let _ = writeln!(output.err, "Unexpected host ingestion result: {stdout}");
        return Err(Stop2::Code(1));
    }
    output.print(&format!(
        "Host accepted the profile and read all {expected} sample records."
    ))?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    fn usage_message(text: &str) -> String {
        match parse(&words(text)) {
            Err(Stop::Usage { prog, message, .. }) => format!("{prog}: error: {message}"),
            Err(Stop::Show(_)) => "shown".to_owned(),
            Ok(_) => "parsed".to_owned(),
        }
    }

    #[test]
    fn parses_like_argparse() {
        // Messages from Python 3.13's argparse for 0.1.0's parser (3.14 prints choices
        // without quotes).
        assert_eq!(
            usage_message(""),
            "crowdwatch: error: the following arguments are required: command"
        );
        assert_eq!(
            usage_message("collect"),
            "crowdwatch collect: error: the following arguments are required: --config"
        );
        assert_eq!(
            usage_message("report"),
            "crowdwatch report: error: the following arguments are required: --config, --out"
        );
        assert_eq!(
            usage_message("import --config c"),
            "crowdwatch import: error: the following arguments are required: file"
        );
        assert_eq!(
            usage_message("import"),
            "crowdwatch import: error: the following arguments are required: --config, file"
        );
        assert_eq!(
            usage_message("backtest --config c --horizon abc"),
            "crowdwatch backtest: error: argument --horizon: invalid float value: 'abc'"
        );
        assert_eq!(
            usage_message("watch --config c --cycles x"),
            "crowdwatch watch: error: argument --cycles: invalid int value: 'x'"
        );
        assert_eq!(
            usage_message("collect --config"),
            "crowdwatch collect: error: argument --config: expected one argument"
        );
        assert_eq!(
            usage_message("analyse --config c --bogus"),
            "crowdwatch: error: unrecognized arguments: --bogus"
        );
        assert_eq!(
            usage_message("analyse --config c extra"),
            "crowdwatch: error: unrecognized arguments: extra"
        );
        assert_eq!(usage_message("watch --config c --s 5"), "crowdwatch watch: error: ambiguous option: --s could match --source, --sleep, --skeletrace");
        assert_eq!(usage_message("init --preset paris"), "crowdwatch init: error: argument --preset: invalid choice: 'paris' (choose from london, melbourne, new-york)");
        assert_eq!(
            usage_message("collect --config c --force=yes"),
            "crowdwatch collect: error: argument --force: ignored explicit argument 'yes'"
        );
        assert_eq!(
            usage_message("nosuch").split(" (choose").next().unwrap(),
            "crowdwatch: error: argument command: invalid choice: 'nosuch'"
        );
        assert_eq!(usage_message("--version"), "shown");
        assert_eq!(usage_message("collect --config c -h"), "shown");
        let parsed = parse(&words("collect --conf c --sour x")).ok().unwrap();
        assert_eq!(parsed.text("--source"), Some("x"));
        let parsed = parse(&words("prune --config c --keep-days -5"))
            .ok()
            .unwrap();
        assert!(matches!(
            parsed.get("--keep-days"),
            Some(Arg::Int(Value::Int(-5)))
        ));
        let parsed = parse(&words("backtest --config=c --horizon=1_5"))
            .ok()
            .unwrap();
        assert!(matches!(parsed.get("--horizon"), Some(Arg::Float(h)) if *h == 15.0));
    }

    fn candidates(
        name: &str,
        windows: bool,
        path: Option<&str>,
        pathext: Option<&str>,
        current_dir: bool,
    ) -> Vec<String> {
        let search = Which {
            windows,
            path: path.map(std::ffi::OsStr::new),
            pathext,
            current_dir,
        };
        search
            .candidates(name)
            .into_iter()
            .map(|c| c.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn which_tries_what_python_tries() {
        // Each list is what Python 3.14's shutil.which checked, run with Windows or POSIX
        // rules.
        let tools = r"C:\Tools;D:\bin;c:\tools";
        assert_eq!(
            candidates("skeletrace", true, Some(tools), Some(".COM;.EXE"), true),
            [
                r".\skeletrace.COM",
                r".\skeletrace.EXE",
                r"C:\Tools\skeletrace.COM",
                r"C:\Tools\skeletrace.EXE",
                r"D:\bin\skeletrace.COM",
                r"D:\bin\skeletrace.EXE"
            ]
        );
        assert_eq!(
            candidates(
                "skeletrace.exe",
                true,
                Some(r"C:\Tools"),
                Some(".COM;.EXE"),
                true
            ),
            [
                r".\skeletrace.exe",
                r".\skeletrace.exe.COM",
                r".\skeletrace.exe.EXE",
                r"C:\Tools\skeletrace.exe",
                r"C:\Tools\skeletrace.exe.COM",
                r"C:\Tools\skeletrace.exe.EXE"
            ]
        );
        assert_eq!(
            candidates(
                "skeletrace.foo",
                true,
                Some(r"C:\Tools"),
                Some(".COM;.EXE"),
                true
            ),
            [
                r".\skeletrace.foo.COM",
                r".\skeletrace.foo.EXE",
                r"C:\Tools\skeletrace.foo.COM",
                r"C:\Tools\skeletrace.foo.EXE"
            ]
        );
        assert_eq!(
            candidates(
                r"C:\SkeleTrace\skeletrace",
                true,
                Some(r"C:\Tools"),
                Some(".COM;.EXE"),
                true
            ),
            [
                r"C:\SkeleTrace\skeletrace.COM",
                r"C:\SkeleTrace\skeletrace.EXE"
            ]
        );
        assert_eq!(
            candidates(
                r"..\target/release\skeletrace.EXE",
                true,
                Some(r"C:\Tools"),
                Some(".COM;.EXE"),
                true
            ),
            [
                r"..\target/release\skeletrace.EXE",
                r"..\target/release\skeletrace.EXE.COM",
                r"..\target/release\skeletrace.EXE.EXE"
            ]
        );
        assert_eq!(
            candidates(
                "skeletrace",
                true,
                Some(r"C:\Tools;;D:"),
                Some(".EXE;."),
                false
            ),
            [
                r"C:\Tools\skeletrace",
                r"C:\Tools\skeletrace.EXE",
                r"C:\Tools\skeletrace",
                "skeletrace",
                "skeletrace.EXE",
                "skeletrace",
                "D:skeletrace",
                "D:skeletrace.EXE",
                "D:skeletrace"
            ]
        );
        assert!(candidates("skeletrace", true, Some(""), Some(".EXE"), true).is_empty());
        assert_eq!(
            candidates("skeletrace", true, None, None, true),
            [
                r".\skeletrace.COM",
                r".\skeletrace.EXE",
                r".\skeletrace.BAT",
                r".\skeletrace.CMD",
                r".\skeletrace.VBS",
                r".\skeletrace.JS",
                r".\skeletrace.WS",
                r".\skeletrace.MSC",
                r"C:\bin\skeletrace.COM",
                r"C:\bin\skeletrace.EXE",
                r"C:\bin\skeletrace.BAT",
                r"C:\bin\skeletrace.CMD",
                r"C:\bin\skeletrace.VBS",
                r"C:\bin\skeletrace.JS",
                r"C:\bin\skeletrace.WS",
                r"C:\bin\skeletrace.MSC"
            ]
        );
        assert_eq!(
            candidates("C:skeletrace", true, Some(r"C:\Tools"), Some(".EXE"), true),
            ["C:skeletrace.EXE"]
        );
        assert_eq!(
            candidates("skeletrace", false, Some("/usr/bin::/opt/st/"), None, true),
            ["/usr/bin/skeletrace", "skeletrace", "/opt/st/skeletrace"]
        );
        assert_eq!(
            candidates("./skeletrace", false, Some("/usr/bin"), None, true),
            ["./skeletrace"]
        );
        assert_eq!(
            candidates("/opt//st//skeletrace", false, Some("/usr/bin"), None, true),
            ["/opt//st/skeletrace"]
        );
        assert!(candidates("skeletrace", false, Some(""), None, true).is_empty());
        if !cfg!(target_os = "macos") {
            assert_eq!(
                candidates("skeletrace", false, None, None, true),
                ["/bin/skeletrace", "/usr/bin/skeletrace"]
            );
        }
        assert_eq!(
            candidates("skeletrace", false, Some("/a:/a:/b"), None, true),
            ["/a/skeletrace", "/b/skeletrace"]
        );
    }
}
