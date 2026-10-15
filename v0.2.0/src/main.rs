// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! `crowdwatch` command-line entry point.

#![forbid(unsafe_code)]

use std::io::{self, Write};
use std::process::ExitCode;

/// Console text as Python printed it: on Windows each line ends with CRLF.
struct Console<W: Write> {
    inner: W,
}

impl<W: Write> Write for Console<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if cfg!(windows) {
            let mut converted = Vec::with_capacity(buf.len() + buf.len() / 16);
            for &byte in buf {
                if byte == b'\n' {
                    converted.push(b'\r');
                }
                converted.push(byte);
            }
            self.inner.write_all(&converted)?;
        } else {
            self.inner.write_all(buf)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn main() -> ExitCode {
    let mut args = Vec::new();
    for arg in std::env::args_os().skip(1) {
        match arg.into_string() {
            Ok(arg) => args.push(arg),
            Err(raw) => {
                let mut err = Console {
                    inner: io::stderr().lock(),
                };
                let _ = writeln!(
                    err,
                    "crowdwatch: error: argument is not valid Unicode: {}",
                    raw.to_string_lossy()
                );
                return ExitCode::from(2);
            }
        }
    }
    let runtime = crowdwatch::cli::Runtime::standard();
    let mut out = Console {
        inner: io::stdout().lock(),
    };
    let mut err = Console {
        inner: io::stderr().lock(),
    };
    let code = crowdwatch::cli::run(&args, &runtime, &mut out, &mut err);
    let _ = out.flush();
    let _ = err.flush();
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
