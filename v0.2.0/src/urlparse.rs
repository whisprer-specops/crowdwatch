// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! URL splitting, joining and percent-encoding with the rules of Python's `urllib.parse`.
//!
//! CrowdWatch 0.1.0 cleaned provenance URLs, checked feed URLs and added query parameters
//! with `urlsplit`, `urlunsplit`, `parse_qsl` and `urlencode`. These functions keep those
//! rules exactly, including Python's guards against bracket tricks and hosts that change
//! meaning under Unicode compatibility normalisation (NFKC).

use std::net::{Ipv4Addr, Ipv6Addr};

use icu_normalizer::ComposingNormalizerBorrowed;

/// The five parts `urlsplit` returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitUrl {
    pub scheme: String,
    pub netloc: String,
    pub path: String,
    pub query: String,
    pub fragment: String,
}

/// Python's `urllib.parse.urlsplit(url)`. An `Err` carries Python's `ValueError` message.
pub fn urlsplit(url: &str) -> Result<SplitUrl, String> {
    // Leading C0 controls and spaces are stripped; tabs and line breaks are removed anywhere.
    let url: String = url
        .trim_start_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();
    let mut scheme = String::new();
    let mut rest = url.as_str();
    if let Some(colon) = url.find(':') {
        let candidate = &url[..colon];
        let starts_with_letter = candidate
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic());
        if starts_with_letter
            && candidate
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        {
            scheme = candidate.to_ascii_lowercase();
            rest = &url[colon + 1..];
        }
    }
    let mut netloc = String::new();
    if let Some(after) = rest.strip_prefix("//") {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        netloc = after[..end].to_owned();
        rest = &after[end..];
        let opens = netloc.contains('[');
        let closes = netloc.contains(']');
        if opens != closes {
            return Err("Invalid IPv6 URL".to_owned());
        }
        if opens {
            check_bracketed_netloc(&netloc)?;
        }
    }
    let (rest, fragment) = rest.split_once('#').unwrap_or((rest, ""));
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    check_netloc(&netloc)?;
    Ok(SplitUrl {
        scheme,
        netloc,
        path: path.to_owned(),
        query: query.to_owned(),
        fragment: fragment.to_owned(),
    })
}

impl SplitUrl {
    /// Python's `SplitResult.username`.
    pub fn username(&self) -> Option<&str> {
        self.netloc
            .rsplit_once('@')
            .map(|(info, _)| info.split_once(':').map_or(info, |(user, _)| user))
    }

    /// Python's `SplitResult.password`.
    pub fn password(&self) -> Option<&str> {
        self.netloc
            .rsplit_once('@')
            .and_then(|(info, _)| info.split_once(':').map(|(_, password)| password))
    }

    /// Python's `SplitResult.port`: `Ok(None)` when there is none, `Err` with Python's
    /// ValueError message when it is not a number from 0 to 65535.
    pub fn port(&self) -> Result<Option<u32>, String> {
        let hostinfo = self
            .netloc
            .rsplit_once('@')
            .map_or(self.netloc.as_str(), |(_, host)| host);
        let port = match hostinfo.split_once('[') {
            Some((_, bracketed)) => bracketed.split_once(']').map_or("", |(_, after)| {
                after.split_once(':').map_or("", |(_, p)| p)
            }),
            None => hostinfo.split_once(':').map_or("", |(_, p)| p),
        };
        if port.is_empty() {
            return Ok(None);
        }
        if !(port.is_ascii() && port.chars().all(|c| c.is_ascii_digit())) {
            return Err(format!(
                "Port could not be cast to integer value as {}",
                crate::pyfmt::repr_str(port)
            ));
        }
        match port.trim_start_matches('0').parse::<u32>() {
            Ok(value) if value <= 65535 => Ok(Some(value)),
            _ if port.bytes().all(|b| b == b'0') => Ok(Some(0)),
            _ => Err("Port out of range 0-65535".to_owned()),
        }
    }

    /// Python's `SplitResult.hostname`: lower-cased, without port or brackets.
    pub fn hostname(&self) -> Option<String> {
        let hostinfo = self
            .netloc
            .rsplit_once('@')
            .map_or(self.netloc.as_str(), |(_, host)| host);
        let host = match hostinfo.split_once('[') {
            Some((_, bracketed)) => bracketed.split_once(']').map_or(bracketed, |(h, _)| h),
            None => hostinfo.split_once(':').map_or(hostinfo, |(h, _)| h),
        };
        if host.is_empty() {
            return None;
        }
        // A scoped IPv6 address keeps the case of its zone.
        Some(match host.split_once('%') {
            Some((name, zone)) => format!("{}%{zone}", name.to_lowercase()),
            None => host.to_lowercase(),
        })
    }
}

fn check_bracketed_netloc(netloc: &str) -> Result<(), String> {
    let host_and_port = netloc.rsplit_once('@').map_or(netloc, |(_, host)| host);
    let hostname = match host_and_port.split_once('[') {
        Some((before, bracketed)) => {
            if !before.is_empty() {
                return Err("Invalid IPv6 URL".to_owned());
            }
            let (hostname, port) = bracketed.split_once(']').unwrap_or((bracketed, ""));
            if !port.is_empty() && !port.starts_with(':') {
                return Err("Invalid IPv6 URL".to_owned());
            }
            hostname
        }
        None => host_and_port
            .split_once(':')
            .map_or(host_and_port, |(h, _)| h),
    };
    check_bracketed_host(hostname)
}

fn check_bracketed_host(hostname: &str) -> Result<(), String> {
    if hostname.starts_with(['v', 'V']) {
        // IPvFuture: "v" HEXDIG+ "." something
        let body = &hostname[1..];
        let hex_len = body.bytes().take_while(u8::is_ascii_hexdigit).count();
        let valid = hex_len > 0
            && body[hex_len..]
                .strip_prefix('.')
                .is_some_and(|tail| !tail.is_empty());
        return if valid {
            Ok(())
        } else {
            Err("IPvFuture address is invalid".to_owned())
        };
    }
    if hostname.parse::<Ipv4Addr>().is_ok() {
        return Err("An IPv4 address cannot be in brackets".to_owned());
    }
    let address = match hostname.split_once('%') {
        Some((address, zone)) if !zone.is_empty() && !zone.contains('%') => address,
        Some(_) => "",
        None => hostname,
    };
    if address.parse::<Ipv6Addr>().is_ok() {
        Ok(())
    } else {
        Err(format!(
            "'{hostname}' does not appear to be an IPv4 or IPv6 address"
        ))
    }
}

/// Python's guard against host names such as "a\u{2100}b" that gain `/`, `?`, `#`, `@` or
/// `:` under NFKC, the normalisation IDNA applies.
fn check_netloc(netloc: &str) -> Result<(), String> {
    if netloc.is_ascii() {
        return Ok(());
    }
    let stripped: String = netloc
        .chars()
        .filter(|c| !matches!(c, '@' | ':' | '#' | '?'))
        .collect();
    let normalized = ComposingNormalizerBorrowed::new_nfkc().normalize(&stripped);
    if normalized == stripped {
        return Ok(());
    }
    if normalized.contains(['/', '?', '#', '@', ':']) {
        return Err(format!(
            "netloc '{netloc}' contains invalid characters under NFKC normalization"
        ));
    }
    Ok(())
}

/// Python's `urllib.parse.quote(text, safe)`: UTF-8 bytes, with letters, digits, `_.-~`
/// and the `safe` characters left as they are and everything else written as `%XX`.
pub fn quote(text: &str, safe: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'_' | b'.' | b'-' | b'~')
            || (byte.is_ascii() && safe.as_bytes().contains(&byte))
        {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Python's `urllib.parse.quote_plus(text)`: as `quote` with nothing extra safe, but
/// spaces become `+`.
pub fn quote_plus(text: &str) -> String {
    if text.contains(' ') {
        quote(text, " ").replace(' ', "+")
    } else {
        quote(text, "")
    }
}

/// Python's `urllib.parse.urlencode(params)` for string keys and values, in order.
pub fn urlencode(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{}={}", quote_plus(key), quote_plus(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// The schemes Python's `urlunsplit` writes with `//` even without a network location.
const USES_NETLOC: [&str; 27] = [
    "",
    "ftp",
    "http",
    "gopher",
    "nntp",
    "telnet",
    "imap",
    "wais",
    "file",
    "mms",
    "https",
    "shttp",
    "snews",
    "prospero",
    "rtsp",
    "rtsps",
    "rtspu",
    "rsync",
    "svn",
    "svn+ssh",
    "sftp",
    "nfs",
    "git",
    "git+ssh",
    "ws",
    "wss",
    "itms-services",
];

/// Python's `urllib.parse.urlunsplit((scheme, netloc, path, query, fragment))`.
pub fn urlunsplit(scheme: &str, netloc: &str, path: &str, query: &str, fragment: &str) -> String {
    let mut url = path.to_owned();
    if !netloc.is_empty() {
        if !url.is_empty() && !url.starts_with('/') {
            url.insert(0, '/');
        }
        url = format!("//{netloc}{url}");
    } else if url.starts_with("//")
        || (!scheme.is_empty()
            && USES_NETLOC.contains(&scheme)
            && (url.is_empty() || url.starts_with('/')))
    {
        url = format!("//{url}");
    }
    if !scheme.is_empty() {
        url = format!("{scheme}:{url}");
    }
    if !query.is_empty() {
        url = format!("{url}?{query}");
    }
    if !fragment.is_empty() {
        url = format!("{url}#{fragment}");
    }
    url
}

/// Python's `urllib.parse.unquote(text)`: `%XX` escapes decoded as UTF-8, one run of
/// ASCII at a time, with invalid sequences replaced by U+FFFD.
pub fn unquote(text: &str) -> String {
    if !text.contains('%') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        let ascii_len = rest.bytes().take_while(u8::is_ascii).count();
        if ascii_len == 0 {
            let c = rest.chars().next().unwrap_or_default();
            out.push(c);
            rest = &rest[c.len_utf8()..];
            continue;
        }
        let (run, tail) = rest.split_at(ascii_len);
        out.push_str(&String::from_utf8_lossy(&unquote_to_bytes(run)));
        rest = tail;
    }
    out
}

fn unquote_to_bytes(run: &str) -> Vec<u8> {
    let bytes = run.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && bytes[index + 1].is_ascii_hexdigit()
            && bytes[index + 2].is_ascii_hexdigit()
        {
            let pair = &run[index + 1..index + 3];
            out.push(u8::from_str_radix(pair, 16).unwrap_or(b'?'));
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    out
}

/// Python's `urllib.parse.parse_qsl(query)`: `&`-separated pairs, `+` as space, pairs
/// without a value dropped.
pub fn parse_qsl(query: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    if query.is_empty() {
        return pairs;
    }
    for item in query.split('&') {
        if item.is_empty() {
            continue;
        }
        let (name, value) = item.split_once('=').unwrap_or((item, ""));
        if value.is_empty() {
            continue;
        }
        pairs.push((
            unquote(&name.replace('+', " ")),
            unquote(&value.replace('+', " ")),
        ));
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_and_joins_like_python() {
        let parts = urlsplit("HTTPS://User:pw@Example.org:8443/a/b?x=1#frag").unwrap();
        assert_eq!(parts.scheme, "https");
        assert_eq!(parts.hostname().as_deref(), Some("example.org"));
        assert_eq!(parts.port(), Ok(Some(8443)));
        assert_eq!(parts.username(), Some("User"));
        assert!(urlsplit("https://x:99999/").unwrap().port().is_err());
        assert!(urlsplit("https://x:１/").unwrap().port().is_err());
        assert_eq!(urlsplit("https://x:/").unwrap().port(), Ok(None));
        assert_eq!(
            urlsplit("https://[::1]:443/").unwrap().port(),
            Ok(Some(443))
        );
        assert_eq!(
            urlunsplit("https", "example.org", "a", "", ""),
            "https://example.org/a"
        );
        assert_eq!(urlunsplit("https", "", "", "q=1", ""), "https://?q=1");
        assert_eq!(urlunsplit("mailto", "", "x", "", "f"), "mailto:x#f");
        assert!(urlsplit("http://[::1/").is_err());
        assert!(urlsplit("http://a\u{2100}b/").is_err());
    }

    #[test]
    fn query_strings_like_python() {
        assert_eq!(
            parse_qsl("a=1&b=&c&d=x+y&e=%7E%zz&f=%C3%A9&g=%C3"),
            [
                ("a".to_owned(), "1".to_owned()),
                ("d".to_owned(), "x y".to_owned()),
                ("e".to_owned(), "~%zz".to_owned()),
                ("f".to_owned(), "é".to_owned()),
                ("g".to_owned(), "\u{fffd}".to_owned()),
            ]
        );
        assert_eq!(unquote("%C3é%A9"), "\u{fffd}é\u{fffd}");
        assert_eq!(
            urlencode(&[
                ("q".to_owned(), "a b&c".to_owned()),
                ("n".to_owned(), "1".to_owned())
            ]),
            "q=a+b%26c&n=1"
        );
        assert_eq!(quote_plus("~_.-"), "~_.-");
    }
}
