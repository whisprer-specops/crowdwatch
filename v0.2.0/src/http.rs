// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Bounded HTTPS GET transport. Keys are never written to logs or provenance.
//!
//! As in 0.1.0: HTTPS on port 443 only, no redirects, no compressed replies, at most
//! 5 MiB, 15 seconds for each network step, a quarter of a second between requests, and
//! feed hosts that must resolve to public addresses. Proxies come from `HTTPS_PROXY` and
//! `NO_PROXY`; SOCKS proxies work too, and an unusable proxy setting stops the request
//! rather than letting it go out directly.

use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant as Clock};

use crate::error::{Fail, Result};
use crate::json::{Dict, Value};
use crate::model::load_json_bytes;
use crate::pyfmt::{float_from_str, isalnum};
use crate::pytime::{parsedate_to_datetime, Instant, Num};
use crate::urlparse::{parse_qsl, urlencode, urlsplit, urlunsplit};

/// The largest body accepted: 5 MiB.
pub const MAX_BODY: usize = 5 * 1024 * 1024;
/// Each network step (connect, send, each read) may take this long.
pub const TIMEOUT: Duration = Duration::from_secs(15);
/// The gap kept between requests.
pub const SPACING: Duration = Duration::from_millis(250);
pub const USER_AGENT: &str = concat!("Skeletrace-CrowdWatch/", env!("CARGO_PKG_VERSION"));

/// What collectors need from the network. Tests stand in recorded replies.
pub trait Http {
    /// The body of an HTTPS GET.
    fn get(&mut self, url: &str, headers: &[(String, String)]) -> Result<Vec<u8>>;

    /// A JSON reply, read as 0.1.0 read it.
    fn json(&mut self, url: &str, headers: &[(String, String)]) -> Result<Value> {
        load_json_bytes(&self.get(url, headers)?)
    }
}

/// Environment variables, so tests can supply credentials without touching the process.
pub trait Env {
    fn var(&self, name: &str) -> Option<String>;
}

/// The real process environment.
pub struct ProcessEnv;

impl Env for ProcessEnv {
    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

/// 0.1.0's `env_secret(name, required)`: the value of a named environment variable.
pub fn env_secret(env: &dyn Env, name: &Value, required: bool) -> Result<Option<String>> {
    let text = match name {
        Value::Null => None,
        Value::Str(s) => {
            if !isalnum(&s.replace('_', "")) {
                return Err(Fail::input(
                    "credential reference must be an environment variable name",
                ));
            }
            Some(s.as_str())
        }
        _ => {
            return Err(Fail::input(
                "credential reference must be an environment variable name",
            ))
        }
    };
    let value = text.and_then(|n| env.var(n));
    if required && value.as_deref().is_none_or(str::is_empty) {
        return Err(Fail::fetch(format!(
            "set environment variable {} before enabling this source",
            text.unwrap_or("None")
        )));
    }
    Ok(value)
}

/// 0.1.0's `with_query(url, params)`: the URL's own query, then the parameters (as text,
/// skipping `None`), re-encoded; the fragment is dropped.
pub fn with_query(url: &str, params: &[(&str, Value)]) -> Result<String> {
    let parts = urlsplit(url).map_err(Fail::value_error)?;
    let mut query = Dict::new();
    for (name, value) in parse_qsl(&parts.query) {
        query.insert(name, Value::Str(value));
    }
    for (name, value) in params {
        if !value.is_null() {
            query.insert(*name, Value::Str(value.py_str()));
        }
    }
    let pairs: Vec<(String, String)> = query
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_owned()))
        .collect();
    Ok(urlunsplit(
        &parts.scheme,
        &parts.netloc,
        &parts.path,
        &urlencode(&pairs),
        "",
    ))
}

// ---------------------------------------------------------------------------------------
// Address checks: Python's ipaddress `is_global` (3.13 and 3.14 tables)
// ---------------------------------------------------------------------------------------

fn in_v4(address: u32, network: [u8; 4], prefix: u32) -> bool {
    let base = u32::from_be_bytes(network);
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    address & mask == base & mask
}

fn in_v6(address: u128, network: &str, prefix: u32) -> bool {
    let base = u128::from(network.parse::<Ipv6Addr>().unwrap_or(Ipv6Addr::UNSPECIFIED));
    let mask = if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    };
    address & mask == base & mask
}

/// `IPv4Address.is_global`.
pub fn v4_is_global(address: Ipv4Addr) -> bool {
    let a = u32::from(address);
    const PRIVATE: [([u8; 4], u32); 14] = [
        ([0, 0, 0, 0], 8),
        ([10, 0, 0, 0], 8),
        ([127, 0, 0, 0], 8),
        ([169, 254, 0, 0], 16),
        ([172, 16, 0, 0], 12),
        ([192, 0, 0, 0], 24),
        ([192, 0, 0, 170], 31),
        ([192, 0, 2, 0], 24),
        ([192, 168, 0, 0], 16),
        ([198, 18, 0, 0], 15),
        ([198, 51, 100, 0], 24),
        ([203, 0, 113, 0], 24),
        ([240, 0, 0, 0], 4),
        ([255, 255, 255, 255], 32),
    ];
    const EXCEPTIONS: [([u8; 4], u32); 2] = [([192, 0, 0, 9], 32), ([192, 0, 0, 10], 32)];
    let private = PRIVATE.iter().any(|(net, p)| in_v4(a, *net, *p))
        && !EXCEPTIONS.iter().any(|(net, p)| in_v4(a, *net, *p));
    !in_v4(a, [100, 64, 0, 0], 10) && !private
}

/// `IPv6Address.is_global`, with IPv4-mapped addresses judged as IPv4.
pub fn v6_is_global(address: Ipv6Addr) -> bool {
    let a = u128::from(address);
    if a >> 32 == 0xFFFF {
        return v4_is_global(Ipv4Addr::from(a as u32));
    }
    const PRIVATE: [(&str, u32); 11] = [
        ("::1", 128),
        ("::", 128),
        ("::ffff:0:0", 96),
        ("64:ff9b:1::", 48),
        ("100::", 64),
        ("2001::", 23),
        ("2001:db8::", 32),
        ("2002::", 16),
        ("3fff::", 20),
        ("fc00::", 7),
        ("fe80::", 10),
    ];
    const EXCEPTIONS: [(&str, u32); 6] = [
        ("2001:1::1", 128),
        ("2001:1::2", 128),
        ("2001:3::", 32),
        ("2001:4:112::", 48),
        ("2001:20::", 28),
        ("2001:30::", 28),
    ];
    let private = PRIVATE.iter().any(|(net, p)| in_v6(a, net, *p))
        && !EXCEPTIONS.iter().any(|(net, p)| in_v6(a, net, *p));
    !private
}

fn ip_is_global(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => v4_is_global(v4),
        IpAddr::V6(v6) => v6_is_global(v6),
    }
}

/// Python's `ipaddress.ip_address(text)`, including an IPv6 zone (`fe80::1%eth0`).
pub fn parse_ip(text: &str) -> Option<IpAddr> {
    if let Ok(v4) = text.parse::<Ipv4Addr>() {
        return Some(IpAddr::V4(v4));
    }
    let address = match text.split_once('%') {
        Some((address, zone)) if !zone.is_empty() && !zone.contains('%') => address,
        Some(_) => return None,
        None => text,
    };
    address.parse::<Ipv6Addr>().ok().map(IpAddr::V6)
}

/// A host name as DNS sees it: IDNA for international names.
fn ascii_host(host: &str) -> String {
    if host.is_ascii() {
        host.to_owned()
    } else {
        idna::domain_to_ascii(host).unwrap_or_else(|_| host.to_owned())
    }
}

/// 0.1.0's `validate_url(url, resolve)`: an HTTPS feed on port 443 whose host is (or
/// resolves only to) public addresses. Returns the host name.
pub fn validate_url(url: &str, resolve: bool) -> Result<String> {
    let invalid = || Fail::input("invalid feed URL");
    let parts = urlsplit(url).map_err(|_| invalid())?;
    let host = parts.hostname().unwrap_or_default();
    if parts.scheme != "https"
        || host.is_empty()
        || parts.username().is_some_and(|u| !u.is_empty())
        || parts.password().is_some_and(|p| !p.is_empty())
        || !parts.fragment.is_empty()
    {
        return Err(Fail::input(
            "feed URL must be HTTPS, without credentials or fragments",
        ));
    }
    match parts.port() {
        Ok(None | Some(443)) => {}
        Ok(Some(_)) => return Err(Fail::input("feed URLs must use port 443")),
        Err(_) => return Err(invalid()),
    }
    if host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
    {
        return Err(Fail::input("private network feed URLs are not supported"));
    }
    let addresses: Vec<IpAddr> = match parse_ip(&host) {
        Some(address) => vec![address],
        None => {
            if !resolve {
                return Ok(host);
            }
            let found = (ascii_host(&host).as_str(), 443)
                .to_socket_addrs()
                .map_err(|_| Fail::fetch(format!("DNS resolution failed for {host}")))?;
            found.map(|address| address.ip()).collect()
        }
    };
    if addresses.is_empty() || addresses.iter().any(|a| !ip_is_global(*a)) {
        return Err(Fail::input("feed resolves to a non-public address"));
    }
    Ok(host)
}

/// Resolves only to public addresses, so a host cannot switch to a private address
/// between the check and the connection.
struct PublicResolver;

impl ureq::Resolver for PublicResolver {
    fn resolve(&self, netloc: &str) -> std::io::Result<Vec<SocketAddr>> {
        let found: Vec<SocketAddr> = netloc.to_socket_addrs()?.collect();
        if found.is_empty() || found.iter().any(|a| !ip_is_global(a.ip())) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "feed resolves to a non-public address",
            ));
        }
        Ok(found)
    }
}

// ---------------------------------------------------------------------------------------
// Proxies, as Python's urllib reads them from the environment
// ---------------------------------------------------------------------------------------

/// Python's `getproxies_environment()[scheme]`: a lower-case name wins over an upper-case
/// one, and an empty value means unset.
pub fn proxy_setting(lookup: &dyn Fn(&str) -> Option<String>, scheme: &str) -> Option<String> {
    let lower = format!("{scheme}_proxy");
    match lookup(&lower) {
        Some(value) if !value.is_empty() => Some(value),
        Some(_) => None,
        None => lookup(&lower.to_uppercase()).filter(|value| !value.is_empty()),
    }
}

/// Python's `proxy_bypass_environment`: `*`, or a host equal to or ending in an entry.
pub fn bypass_proxy(host: &str, no_proxy: Option<&str>) -> bool {
    let Some(list) = no_proxy else {
        return false;
    };
    if list == "*" {
        return true;
    }
    let host = host.to_lowercase();
    list.split(',')
        .map(|name| name.trim().trim_start_matches('.').to_lowercase())
        .filter(|name| !name.is_empty())
        .any(|name| host == name || host.ends_with(&format!(".{name}")))
}

/// A proxy address in the form ureq reads, or `None` for one it cannot use. `https`
/// becomes `http` (urllib also reached such a proxy over plain TCP), and `socks5h`
/// becomes `socks5` (ureq already hands host names to a SOCKS5 proxy).
fn proxy_address(address: &str) -> Option<String> {
    let Some((scheme, rest)) = address.split_once("://") else {
        return Some(address.to_owned());
    };
    let scheme = scheme.to_ascii_lowercase();
    match scheme.as_str() {
        "http" | "https" => Some(format!("http://{rest}")),
        "socks5h" => Some(format!("socks5://{rest}")),
        "socks" | "socks4" | "socks4a" | "socks5" => Some(format!("{scheme}://{rest}")),
        _ => None,
    }
}

/// How a request that `NO_PROXY` does not exempt leaves the machine.
enum Route {
    Direct,
    Proxy(ureq::Agent),
    /// A proxy is set but unusable: fail rather than go around it.
    Refused,
}

pub const UNUSABLE_PROXY: &str = "HTTPS_PROXY holds an unsupported proxy address";

/// The real network.
pub struct HttpClient {
    direct: ureq::Agent,
    route: Route,
    no_proxy: Option<String>,
    proxy_set: bool,
    last_request: Option<Clock>,
}

impl Default for HttpClient {
    fn default() -> Self {
        HttpClient::from_env()
    }
}

impl HttpClient {
    /// A client using the process's `HTTPS_PROXY` and `NO_PROXY`.
    pub fn from_env() -> HttpClient {
        let lookup = |name: &str| std::env::var(name).ok();
        HttpClient::with_proxy(
            proxy_setting(&lookup, "https"),
            proxy_setting(&lookup, "no"),
        )
    }

    pub fn with_proxy(proxy: Option<String>, no_proxy: Option<String>) -> HttpClient {
        let builder = || {
            ureq::AgentBuilder::new()
                .timeout_connect(TIMEOUT)
                .timeout_read(TIMEOUT)
                .timeout_write(TIMEOUT)
                .redirects(0)
                .user_agent(USER_AGENT)
        };
        let proxy_set = proxy.is_some();
        let route = match proxy {
            None => Route::Direct,
            Some(address) => {
                match proxy_address(&address).and_then(|usable| ureq::Proxy::new(usable).ok()) {
                    Some(proxy) => Route::Proxy(builder().proxy(proxy).build()),
                    None => Route::Refused,
                }
            }
        };
        HttpClient {
            direct: builder().resolver(PublicResolver).build(),
            route,
            no_proxy,
            proxy_set,
            last_request: None,
        }
    }

    fn agent_for(&self, host: &str) -> Result<&ureq::Agent> {
        if bypass_proxy(host, self.no_proxy.as_deref()) {
            return Ok(&self.direct);
        }
        match &self.route {
            Route::Direct => Ok(&self.direct),
            Route::Proxy(agent) => Ok(agent),
            Route::Refused => Err(Fail::fetch(UNUSABLE_PROXY)),
        }
    }
}

/// Python's name for a transport failure before the reply arrived: urllib wraps them
/// all in URLError.
fn transport_kind(err: &ureq::Transport) -> &'static str {
    match err.kind() {
        ureq::ErrorKind::BadStatus => "BadStatusLine",
        ureq::ErrorKind::BadHeader => "LineTooLong",
        _ => "URLError",
    }
}

/// Python's name for a failure while reading the body.
fn read_kind(err: &std::io::Error) -> &'static str {
    match err.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => "TimeoutError",
        std::io::ErrorKind::ConnectionReset => "ConnectionResetError",
        std::io::ErrorKind::ConnectionAborted => "ConnectionAbortedError",
        std::io::ErrorKind::UnexpectedEof => "IncompleteRead",
        _ => "OSError",
    }
}

/// 0.1.0's Retry-After rule: seconds, or an HTTP date from now, within a week.
pub fn retry_after(header: Option<&str>, now: Instant) -> Num {
    let raw = header.unwrap_or("0");
    let mut retry = Num::Float(0.0);
    match float_from_str(raw) {
        // max(0, x): 0 unless x is larger.
        Ok(x) => retry = if x > 0.0 { Num::Float(x) } else { Num::Int(0) },
        Err(_) => {
            // A naive date cannot be subtracted from an aware one (TypeError): no wait.
            if let Some(seconds) = parsedate_to_datetime(raw)
                .ok()
                .and_then(|when| when.seconds_since(now))
            {
                retry = if seconds > 0.0 {
                    Num::Float(seconds)
                } else {
                    Num::Int(0)
                };
            }
        }
    }
    let value = retry.as_f64();
    if !(0.0..=604_800.0).contains(&value) {
        retry = Num::Int(604_800);
    }
    retry
}

/// Python's http.client refuses header values that would break the request.
fn check_header(value: &str) -> Result<()> {
    if value
        .bytes()
        .all(|b| b == b'\t' || (0x20..0x7f).contains(&b))
    {
        Ok(())
    } else {
        Err(Fail::value_error("Invalid header value"))
    }
}

impl Http for HttpClient {
    fn get(&mut self, url: &str, headers: &[(String, String)]) -> Result<Vec<u8>> {
        // A configured HTTPS proxy owns origin DNS; local DNS may intentionally be
        // unavailable. Literal and private host checks still apply in both modes.
        let host = validate_url(url, !self.proxy_set)?;
        if let Some(last) = self.last_request {
            let elapsed = last.elapsed();
            if elapsed < SPACING {
                std::thread::sleep(SPACING - elapsed);
            }
        }
        self.last_request = Some(Clock::now());
        let mut merged: Vec<(String, String)> = vec![
            ("User-Agent".to_owned(), USER_AGENT.to_owned()),
            (
                "Accept".to_owned(),
                "application/json,application/xml,text/csv,*/*".to_owned(),
            ),
            ("Accept-Encoding".to_owned(), "identity".to_owned()),
        ];
        for (name, value) in headers {
            check_header(value)?;
            match merged
                .iter_mut()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
            {
                Some(slot) => slot.1 = value.clone(),
                None => merged.push((name.clone(), value.clone())),
            }
        }
        let agent = self.agent_for(&host)?;
        let mut request = agent.get(url);
        for (name, value) in &merged {
            request = request.set(name, value);
        }
        let response = match request.call() {
            Ok(response) => response,
            Err(ureq::Error::Status(_, response)) => response,
            Err(ureq::Error::Transport(err)) => {
                return Err(Fail::fetch(format!(
                    "connection failed for {host} ({})",
                    transport_kind(&err)
                )))
            }
        };
        let status = response.status();
        if !(200..300).contains(&status) {
            let retry = retry_after(response.header("Retry-After"), Instant::now());
            return Err(Fail::Fetch {
                message: format!("HTTP {status} from {host}; no observations produced"),
                retry_after: retry.as_f64(),
            });
        }
        let encoding = response.header("Content-Encoding").unwrap_or("identity");
        if encoding.to_lowercase() != "identity" {
            return Err(Fail::fetch(format!(
                "unsupported compressed response from {host}"
            )));
        }
        let mut body = Vec::new();
        response
            .into_reader()
            .take(MAX_BODY as u64 + 1)
            .read_to_end(&mut body)
            .map_err(|err| {
                Fail::fetch(format!(
                    "connection failed for {host} ({})",
                    read_kind(&err)
                ))
            })?;
        if body.len() > MAX_BODY {
            return Err(Fail::fetch(format!(
                "response from {host} exceeds {MAX_BODY} bytes"
            )));
        }
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_private_or_credential_urls() {
        for url in [
            "http://example.org",
            "https://localhost/x",
            "https://127.0.0.1/x",
            "https://[::1]/",
            "https://user:secret@example.org/",
            "https://example.org:8443/",
            "https://x.internal/",
            "https://example.org/#frag",
            "https://10.1.2.3/",
            "https://[::ffff:192.168.0.1]/",
            "https://example.org:99999/",
        ] {
            assert!(
                matches!(validate_url(url, false), Err(Fail::Input(_))),
                "{url}"
            );
        }
        assert_eq!(
            validate_url("https://example.org/feed", false).unwrap(),
            "example.org"
        );
        assert_eq!(validate_url("https://8.8.8.8/", true).unwrap(), "8.8.8.8");
    }

    #[test]
    fn global_addresses_follow_python() {
        for (text, global) in [
            ("8.8.8.8", true),
            ("100.64.0.1", false),
            ("192.0.0.9", true),
            ("192.0.0.8", false),
            ("224.0.0.1", true),
            ("2606:4700::1", true),
            ("2001:db8::1", false),
            ("2001:4:112::1", true),
            ("3fff::1", false),
            ("fe80::1%eth0", false),
            ("::ffff:8.8.8.8", true),
        ] {
            assert_eq!(parse_ip(text).map(ip_is_global), Some(global), "{text}");
        }
        assert!(parse_ip("01.2.3.4").is_none());
        assert!(parse_ip("fe80::1%").is_none());
    }

    #[test]
    fn queries_secrets_and_retries() {
        assert_eq!(
            with_query(
                "https://x.org/a?b=1&c=&d=2#f",
                &[
                    ("d", Value::Int(3)),
                    ("e", Value::Null),
                    ("f", Value::Float(51.5))
                ]
            )
            .unwrap(),
            "https://x.org/a?b=1&d=3&f=51.5"
        );
        struct Fake;
        impl Env for Fake {
            fn var(&self, name: &str) -> Option<String> {
                (name == "KEY").then(|| "secret".to_owned())
            }
        }
        assert_eq!(
            env_secret(&Fake, &Value::from("KEY"), true)
                .unwrap()
                .as_deref(),
            Some("secret")
        );
        assert_eq!(
            env_secret(&Fake, &Value::from("MISSING"), true)
                .unwrap_err()
                .to_string(),
            "set environment variable MISSING before enabling this source"
        );
        assert_eq!(
            env_secret(&Fake, &Value::Null, true)
                .unwrap_err()
                .to_string(),
            "set environment variable None before enabling this source"
        );
        assert!(matches!(
            env_secret(&Fake, &Value::from("A-B"), false),
            Err(Fail::Input(_))
        ));
        let now = Instant::from_parts(2026, 10, 1, 12, 0, 0, 0).unwrap();
        assert_eq!(retry_after(Some("120"), now), Num::Float(120.0));
        assert_eq!(retry_after(Some("-5"), now), Num::Int(0));
        assert_eq!(retry_after(Some("nan"), now), Num::Int(0));
        assert_eq!(retry_after(Some("inf"), now), Num::Int(604_800));
        assert_eq!(
            retry_after(Some("Thu, 01 Oct 2026 12:01:00 GMT"), now),
            Num::Float(60.0)
        );
        assert_eq!(retry_after(Some("garbage"), now), Num::Float(0.0));
        assert_eq!(retry_after(None, now), Num::Int(0));
        // Python subtracts aware times without moving them to UTC first.
        assert_eq!(
            retry_after(Some("Fri, 31 Dec 9999 23:59:00 -0100"), now),
            Num::Int(604_800)
        );
    }

    #[test]
    fn an_unusable_proxy_refuses_instead_of_going_direct() {
        let mut client = HttpClient::with_proxy(Some("ftp://proxy.example".into()), None);
        assert_eq!(
            client.get("https://93.184.216.34/", &[]).unwrap_err(),
            Fail::fetch(UNUSABLE_PROXY)
        );
        assert_eq!(
            proxy_address("SOCKS5H://127.0.0.1:9050").as_deref(),
            Some("socks5://127.0.0.1:9050")
        );
        assert_eq!(
            proxy_address("https://p:3128").as_deref(),
            Some("http://p:3128")
        );
    }
}
