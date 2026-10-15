// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Errors, named after the Python exceptions CrowdWatch 0.1.0 raised.
//!
//! The names matter: a collector that meets a malformed reply records
//! "Source I/O or schema error (KeyError); no observations produced", exactly as 0.1.0 did.

use std::fmt;
use std::io;
use std::path::Path;

/// Something CrowdWatch could not do.
#[derive(Clone, Debug, PartialEq)]
pub enum Fail {
    /// Invalid input, worded for the user (0.1.0's `InputError`).
    Input(String),
    /// A source could not be fetched (0.1.0's `FetchError`). `retry_after` is the number of
    /// seconds the provider asked us to wait, or 0.
    Fetch { message: String, retry_after: f64 },
    /// Any other failure, with the name of the Python exception 0.1.0 raised for it.
    Py { kind: &'static str, message: String },
}

pub type Result<T> = std::result::Result<T, Fail>;

impl Fail {
    pub fn input(message: impl Into<String>) -> Fail {
        Fail::Input(message.into())
    }

    pub fn fetch(message: impl Into<String>) -> Fail {
        Fail::Fetch {
            message: message.into(),
            retry_after: 0.0,
        }
    }

    pub fn py(kind: &'static str, message: impl Into<String>) -> Fail {
        Fail::Py {
            kind,
            message: message.into(),
        }
    }

    pub fn key(key: &str) -> Fail {
        Fail::py("KeyError", crate::pyfmt::repr_str(key))
    }

    pub fn type_error(message: impl Into<String>) -> Fail {
        Fail::py("TypeError", message)
    }

    pub fn value_error(message: impl Into<String>) -> Fail {
        Fail::py("ValueError", message)
    }

    pub fn overflow(message: impl Into<String>) -> Fail {
        Fail::py("OverflowError", message)
    }

    /// The Python exception class name.
    pub fn kind(&self) -> &'static str {
        match self {
            Fail::Input(_) => "InputError",
            Fail::Fetch { .. } => "FetchError",
            Fail::Py { kind, .. } => kind,
        }
    }

    /// Whether 0.1.0's `except ValueError` would have caught it (`InputError` is a
    /// `ValueError` there).
    pub fn is_value_error(&self) -> bool {
        matches!(self, Fail::Input(_))
            || matches!(
                self,
                Fail::Py {
                    kind: "ValueError" | "UnicodeDecodeError" | "JSONDecodeError",
                    ..
                }
            )
    }
}

impl fmt::Display for Fail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fail::Input(message) | Fail::Fetch { message, .. } | Fail::Py { message, .. } => {
                f.write_str(message)
            }
        }
    }
}

impl std::error::Error for Fail {}

impl From<io::Error> for Fail {
    /// An error from a call Python made straight to the operating system (starting a
    /// program, writing to the console), which names no file.
    fn from(err: io::Error) -> Fail {
        os_error(&err, Route::System, &[])
    }
}

/// Which way the failing call reached the operating system. On Linux and macOS both read
/// alike; on Windows they decide how Python worded the error.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Route {
    /// Straight to Windows: `os.mkdir`, `os.replace`, `os.scandir`, starting a program.
    /// Python gives Windows' own text, `[WinError 5] Access is denied`.
    System,
    /// Through the C runtime: `open()`, `os.open`, reading, writing and `os.fsync`. The C
    /// runtime turns the Windows code into a C `errno` and words it from its own list, so
    /// a file another program has locked gives `[Errno 13] Permission denied`.
    CRuntime,
}

/// A file error that names the file, from a call Python made straight to the operating
/// system (`os.mkdir`, `os.scandir`): `[Errno 2] No such file or directory: 'data'`.
pub fn io_at(path: &Path, err: io::Error) -> Fail {
    os_error(&err, Route::System, &[path])
}

/// An error opening a file, as Python's `open()` and `os.open` reported it, naming the file:
/// `[Errno 2] No such file or directory: 'counts.csv'`, on Windows too.
pub fn open_at(path: &Path, err: io::Error) -> Fail {
    os_error(&err, Route::CRuntime, &[path])
}

/// An error reading, writing or flushing an open file. Python names no file for these.
pub fn crt(err: io::Error) -> Fail {
    os_error(&err, Route::CRuntime, &[])
}

/// An error from `os.replace(source, target)`, which names both:
/// `[Errno 13] Permission denied: '/data/.crowdwatch-1a2b' -> '/data/report.json'`.
pub fn io_between(source: &Path, target: &Path, err: io::Error) -> Fail {
    os_error(&err, Route::System, &[source, target])
}

/// Whether Python would have raised `FileNotFoundError` for this error.
pub fn is_not_found(err: &io::Error) -> bool {
    os_kind(err, Route::System, cfg!(windows)) == "FileNotFoundError"
}

/// The error Python's `open()` raises for a folder on Linux and macOS, where the system
/// itself would open it: `[Errno 21] Is a directory`.
pub fn is_a_directory() -> io::Error {
    if cfg!(unix) {
        io::Error::from_raw_os_error(21)
    } else {
        io::Error::from(io::ErrorKind::IsADirectory)
    }
}

fn os_error(err: &io::Error, route: Route, names: &[&Path]) -> Fail {
    let windows = cfg!(windows);
    let mut message = os_message(err, route, windows);
    let names: Vec<String> = names
        .iter()
        .map(|p| crate::pyfmt::repr_str(&p.to_string_lossy()))
        .collect();
    if !names.is_empty() {
        message.push_str(": ");
        message.push_str(&names.join(" -> "));
    }
    Fail::py(os_kind(err, route, windows), message)
}

/// The Python exception class for an operating-system error. Python picks it from the C
/// `errno`; on Windows it first turns the Windows code into one.
fn os_kind(err: &io::Error, route: Route, windows: bool) -> &'static str {
    if windows {
        if let Some(code) = err.raw_os_error() {
            return errno_kind(match route {
                Route::System => winerror_to_errno(code),
                Route::CRuntime => crt_errno(code),
            });
        }
    }
    match err.kind() {
        io::ErrorKind::NotFound => "FileNotFoundError",
        io::ErrorKind::PermissionDenied => "PermissionError",
        io::ErrorKind::AlreadyExists => "FileExistsError",
        io::ErrorKind::IsADirectory => "IsADirectoryError",
        io::ErrorKind::NotADirectory => "NotADirectoryError",
        io::ErrorKind::TimedOut => "TimeoutError",
        io::ErrorKind::ConnectionRefused => "ConnectionRefusedError",
        io::ErrorKind::ConnectionReset => "ConnectionResetError",
        io::ErrorKind::ConnectionAborted => "ConnectionAbortedError",
        io::ErrorKind::BrokenPipe => "BrokenPipeError",
        io::ErrorKind::WouldBlock => "BlockingIOError",
        io::ErrorKind::Interrupted => "InterruptedError",
        _ => "OSError",
    }
}

/// Python's text for an operating-system error: `[Errno 2] No such file or directory`, or
/// on Windows `[WinError 2] The system cannot find the file specified` for a direct call.
fn os_message(err: &io::Error, route: Route, windows: bool) -> String {
    let text = err.to_string();
    let Some(code) = err.raw_os_error() else {
        return text;
    };
    if windows && route == Route::CRuntime {
        let errno = crt_errno(code);
        return format!("[Errno {errno}] {}", crt_text(errno));
    }
    let suffix = format!(" (os error {code})");
    let reason = text.strip_suffix(&suffix).unwrap_or(&text);
    if windows {
        let reason = reason.trim_end_matches(|c: char| c == '.' || c <= ' ');
        format!("[WinError {code}] {reason}")
    } else {
        format!("[Errno {code}] {reason}")
    }
}

/// CPython's `winerror_to_errno` (`PC/errmap.h`): the `errno` Python files a Windows error
/// under, which picks its class.
fn winerror_to_errno(code: i32) -> i32 {
    let code = if (code as u32) & 0xFFFF_0000 == 0x8007_0000 {
        code & 0xFFFF
    } else {
        code
    };
    if (10000..12000).contains(&code) {
        // Winsock codes are errno values; six of them stand for the C ones.
        return match code {
            10004 | 10009 | 10013 | 10014 | 10022 | 10024 => code - 10000,
            _ => code,
        };
    }
    match code {
        2 | 3 | 15 | 18 | 53 | 67 | 161 | 206 => 2, // ENOENT
        10 => 7,                                    // E2BIG
        11 | 188..=202 => 8,                        // ENOEXEC
        6 | 114 | 130 => 9,                         // EBADF
        128 | 129 => 10,                            // ECHILD
        89 | 164 | 215 => 11,                       // EAGAIN
        7..=9 | 1816 => 12,                         // ENOMEM
        5 | 16 | 19..=36 | 65 | 82 | 83 | 108 | 132 | 158 | 167 => 13, // EACCES
        80 | 183 => 17,                             // EEXIST
        17 => 18,                                   // EXDEV
        267 => 20,                                  // ENOTDIR
        4 => 24,                                    // EMFILE
        112 => 28,                                  // ENOSPC
        109 | 232 => 32,                            // EPIPE
        145 => 41,                                  // ENOTEMPTY
        1113 => 42,                                 // EILSEQ
        258 => 10060,                               // ETIMEDOUT (WAIT_TIMEOUT)
        _ => 22,                                    // EINVAL
    }
}

/// The Windows C runtime's `_dosmaperr`: the `errno` a failed `open`, `read`, `write` or
/// `_commit` leaves behind.
fn crt_errno(code: i32) -> i32 {
    match code {
        2 | 3 | 15 | 18 | 53 | 67 | 161 | 206 => 2, // ENOENT
        10 => 7,                                    // E2BIG
        11 => 8,                                    // ENOEXEC
        6 | 114 | 130 => 9,                         // EBADF
        128 | 129 => 10,                            // ECHILD
        89 | 164 | 215 => 11,                       // EAGAIN
        7..=9 | 1816 => 12,                         // ENOMEM
        5 | 16 | 33 | 65 | 82 | 83 | 108 | 132 | 158 | 167 => 13, // EACCES
        80 | 183 => 17,                             // EEXIST
        17 => 18,                                   // EXDEV
        4 => 24,                                    // EMFILE
        112 => 28,                                  // ENOSPC
        109 => 32,                                  // EPIPE
        145 => 41,                                  // ENOTEMPTY
        1113 => 42,                                 // EILSEQ
        // Outside the table: the write-protect to sharing-buffer range means "access
        // denied", the bad-executable range "exec format error", and the rest EINVAL.
        19..=36 => 13,
        188..=202 => 8,
        _ => 22,
    }
}

/// The Windows C runtime's wording for an `errno` (its `_sys_errlist`).
fn crt_text(errno: i32) -> &'static str {
    const TEXT: [&str; 43] = [
        "No error",
        "Operation not permitted",
        "No such file or directory",
        "No such process",
        "Interrupted function call",
        "Input/output error",
        "No such device or address",
        "Arg list too long",
        "Exec format error",
        "Bad file descriptor",
        "No child processes",
        "Resource temporarily unavailable",
        "Not enough space",
        "Permission denied",
        "Bad address",
        "Unknown error",
        "Resource device",
        "File exists",
        "Improper link",
        "No such device",
        "Not a directory",
        "Is a directory",
        "Invalid argument",
        "Too many open files in system",
        "Too many open files",
        "Inappropriate I/O control operation",
        "Unknown error",
        "File too large",
        "No space left on device",
        "Invalid seek",
        "Read-only file system",
        "Too many links",
        "Broken pipe",
        "Domain error",
        "Result too large",
        "Unknown error",
        "Resource deadlock avoided",
        "Unknown error",
        "Filename too long",
        "No locks available",
        "Function not implemented",
        "Directory not empty",
        "Illegal byte sequence",
    ];
    usize::try_from(errno)
        .ok()
        .and_then(|i| TEXT.get(i))
        .copied()
        .unwrap_or("Unknown error")
}

/// Python's class for an `errno` on Windows (the C runtime's numbers; sockets use the
/// Winsock codes).
fn errno_kind(errno: i32) -> &'static str {
    match errno {
        1 | 13 => "PermissionError",
        2 => "FileNotFoundError",
        3 => "ProcessLookupError",
        4 => "InterruptedError",
        10 => "ChildProcessError",
        11 | 10035..=10037 => "BlockingIOError",
        17 => "FileExistsError",
        20 => "NotADirectoryError",
        21 => "IsADirectoryError",
        32 | 10058 => "BrokenPipeError",
        10053 => "ConnectionAbortedError",
        10054 => "ConnectionResetError",
        10060 => "TimeoutError",
        10061 => "ConnectionRefusedError",
        _ => "OSError",
    }
}

impl From<rusqlite::Error> for Fail {
    fn from(err: rusqlite::Error) -> Fail {
        let message = match &err {
            rusqlite::Error::SqliteFailure(_, Some(message)) => message.clone(),
            other => other.to_string(),
        };
        Fail::py("sqlite3.Error", message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_errors_read_like_python() {
        let err = std::fs::File::open("no-such-file.jsonl").unwrap_err();
        let fail = open_at(Path::new("no-such-file.jsonl"), err);
        assert_eq!(fail.kind(), "FileNotFoundError");
        assert_eq!(
            fail.to_string(),
            "[Errno 2] No such file or directory: 'no-such-file.jsonl'"
        );
        let err = std::fs::File::open("no-such-file.jsonl").unwrap_err();
        assert!(is_not_found(&err));
        #[cfg(unix)]
        {
            let moved = io_between(
                Path::new("/data/.crowdwatch-1"),
                Path::new("report.json"),
                io::Error::from_raw_os_error(13),
            );
            assert_eq!(moved.kind(), "PermissionError");
            assert_eq!(
                moved.to_string(),
                "[Errno 13] Permission denied: '/data/.crowdwatch-1' -> 'report.json'"
            );
            let read = crt(io::Error::from_raw_os_error(5));
            assert_eq!(read.to_string(), "[Errno 5] Input/output error");
            let folder = open_at(Path::new("dir"), is_a_directory());
            assert_eq!(folder.kind(), "IsADirectoryError");
            assert_eq!(folder.to_string(), "[Errno 21] Is a directory: 'dir'");
        }
    }

    #[test]
    fn windows_errors_read_like_python_on_windows() {
        // A file locked by another program (ERROR_SHARING_VIOLATION) and a missing one,
        // as Python's open() reports them on Windows.
        let locked = io::Error::from_raw_os_error(32);
        assert_eq!(os_kind(&locked, Route::CRuntime, true), "PermissionError");
        assert_eq!(
            os_message(&locked, Route::CRuntime, true),
            "[Errno 13] Permission denied"
        );
        let missing = io::Error::from_raw_os_error(3);
        assert_eq!(
            os_kind(&missing, Route::CRuntime, true),
            "FileNotFoundError"
        );
        assert_eq!(
            os_message(&missing, Route::CRuntime, true),
            "[Errno 2] No such file or directory"
        );
        assert_eq!(
            os_message(&io::Error::from_raw_os_error(123), Route::CRuntime, true),
            "[Errno 22] Invalid argument"
        );
        assert_eq!(
            os_message(&io::Error::from_raw_os_error(112), Route::CRuntime, true),
            "[Errno 28] No space left on device"
        );
        // A direct call keeps Windows' own code; the class comes from PC/errmap.h.
        assert_eq!(os_kind(&locked, Route::System, true), "PermissionError");
        assert_eq!(
            os_kind(&io::Error::from_raw_os_error(267), Route::System, true),
            "NotADirectoryError"
        );
        assert_eq!(
            os_kind(&io::Error::from_raw_os_error(267), Route::CRuntime, true),
            "OSError"
        );
        assert_eq!(
            os_kind(&io::Error::from_raw_os_error(183), Route::System, true),
            "FileExistsError"
        );
        assert_eq!(winerror_to_errno(0x8007_0005_u32 as i32), 13);
        assert_eq!(winerror_to_errno(10061), 10061);
        assert_eq!(winerror_to_errno(10013), 13);
        assert_eq!(crt_errno(190), 8);
        assert_eq!(crt_text(13), "Permission denied");
        assert_eq!(crt_text(99), "Unknown error");
    }
}
