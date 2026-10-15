// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Self-contained offline report; all source text is escaped, no remote scripts.
//!
//! The page is 0.1.0's, byte for byte, apart from the version in its header: the same
//! markup, the same stylesheet, the same chart geometry and the same number formatting.

use std::path::Path;

use crate::error::{Fail, Result};
use crate::json::Value;
use crate::model::atomic_write;
use crate::pyfmt::{format_fixed, format_grouped, html_escape};
use crate::pytime::parse_time;

/// The stylesheet 0.1.0 shipped, embedded so the report needs nothing else.
const STYLE: &str = include_str!("../assets/style.css");

/// The version shown in the page header ("0.2").
const SHORT_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION_MAJOR"),
    ".",
    env!("CARGO_PKG_VERSION_MINOR")
);

/// The stylesheet with universal newlines, as Python's `read_text` returned it (a Windows
/// checkout may have stored it with CRLF).
fn style() -> String {
    STYLE.replace("\r\n", "\n").replace('\r', "\n")
}

/// `escape(str(value), quote=True)`.
pub fn h(value: &Value) -> String {
    html_escape(&value.py_str())
}

fn h_str(text: &str) -> String {
    html_escape(text)
}

/// 0.1.0's `number(value)`: an em dash for nothing, otherwise grouped with two decimals
/// and trailing zeros removed.
pub fn number(value: &Value) -> Result<String> {
    let f = match value {
        Value::Null => return Ok("\u{2014}".to_owned()),
        Value::Float(f) => *f,
        Value::Int(i) => *i as f64,
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::BigInt(_) => value.py_float()?,
        other => {
            return Err(Fail::value_error(format!(
                "Unknown format code 'f' for object of type '{}'",
                other.type_name()
            )))
        }
    };
    let text = format_grouped(f, 2);
    Ok(text.trim_end_matches('0').trim_end_matches('.').to_owned())
}

fn number_f(value: f64) -> String {
    // Infallible for a float.
    number(&Value::Float(value)).unwrap_or_default()
}

/// A field of a report object.
fn at<'a>(value: &'a Value, key: &str) -> Result<&'a Value> {
    value.index(key)
}

fn text_of<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    match at(value, key)? {
        Value::Str(s) => Ok(s),
        other => Err(Fail::type_error(format!(
            "expected text, not '{}'",
            other.type_name()
        ))),
    }
}

/// `o["label"] or o["stream_id"]`.
fn name_of(o: &Value) -> Result<String> {
    let label = at(o, "label")?;
    Ok(if label.truthy() {
        h(label)
    } else {
        h(at(o, "stream_id")?)
    })
}

/// `display_unit(observation)`: an interval count shows its interval in minutes.
pub fn display_unit(observation: &Value) -> Result<String> {
    let unit = text_of(observation, "unit")?;
    let period = at(observation, "period_seconds")?;
    if period.truthy() && unit.ends_with("/interval") {
        let minutes = period.py_float()? / 60.0;
        let head = unit.split('/').next().unwrap_or("");
        return Ok(format!("{head}/{} min", number_f(minutes)));
    }
    Ok(unit.to_owned())
}

/// Python's `max(a, b)` on floats where `b` may be the integer 1: the first unless the
/// second is larger. Integer or float, the chart prints the same digits.
fn larger(a: f64, b: f64) -> f64 {
    if b > a {
        b
    } else {
        a
    }
}

fn timestamp(value: &Value) -> Result<f64> {
    Ok(parse_time(value)?.timestamp())
}

/// `x[11:16]` of a timestamp.
fn clock(text: &str) -> String {
    text.chars().skip(11).take(5).collect()
}

/// The trend chart for one signal: the last two hours on a true elapsed-time axis.
pub fn chart(signal: &Value) -> Result<String> {
    let history = match at(signal, "history")? {
        Value::List(items) => items,
        other => {
            return Err(Fail::type_error(format!(
                "history is '{}'",
                other.type_name()
            )))
        }
    };
    if history.len() < 2 {
        return Ok(
            "<p class=\"muted\">More observations are needed to draw a trend.</p>".to_owned(),
        );
    }
    let end = timestamp(at(&history[history.len() - 1], "at")?)?;
    let mut rows: Vec<&Value> = Vec::new();
    for x in history {
        if timestamp(at(x, "at")?)? >= end - 7200.0 {
            rows.push(x);
        }
    }
    if rows.len() < 2 {
        rows = history[history.len() - 2..].iter().collect();
    }
    let start = timestamp(at(rows[0], "at")?)?;
    let forecast = at(signal, "projection")?;
    let forecast = if forecast.truthy() {
        Some(forecast)
    } else {
        None
    };
    let xmax = match forecast {
        Some(f) => timestamp(at(f, "at")?)?,
        None => end,
    };
    let mut values: Vec<f64> = Vec::with_capacity(rows.len() + 1);
    for x in &rows {
        values.push(at(x, "value")?.py_float()?);
    }
    if let Some(f) = forecast {
        values.push(at(f, "upper")?.py_float()?);
    }
    let highest = values
        .iter()
        .copied()
        .reduce(|m, v| if v > m { v } else { m })
        .unwrap_or(0.0);
    let mut ymax = larger(highest * 1.15, 1.0);
    let baseline = at(signal, "baseline")?;
    let baseline = if baseline.truthy() {
        Some(at(baseline, "median")?.py_float()?)
    } else {
        None
    };
    if let Some(b) = baseline {
        ymax = larger(ymax, b * 1.15);
    }
    let width = larger(xmax - start, 1.0);
    let point = |at_time: f64, value: f64| {
        format!(
            "{},{}",
            format_fixed(50.0 + (at_time - start) / width * 620.0, 2),
            format_fixed(175.0 - value / ymax * 140.0, 2)
        )
    };
    let mut points = Vec::with_capacity(rows.len());
    for x in &rows {
        points.push(point(timestamp(at(x, "at")?)?, at(x, "value")?.py_float()?));
    }
    let mut svg = String::from("<svg viewBox=\"0 0 720 220\" role=\"img\" aria-label=\"Observed values over time, with an optional trend projection\">");
    for v in [0.0, ymax / 2.0, ymax] {
        let y = 175.0 - v / ymax * 140.0;
        svg.push_str(&format!(
            "<path d=\"M50 {} H670\" class=\"grid\"/><text x=\"43\" y=\"{}\" text-anchor=\"end\">{}</text>",
            format_fixed(y, 2),
            format_fixed(y + 4.0, 2),
            h_str(&number_f(v))
        ));
    }
    if let Some(b) = baseline {
        let y = 175.0 - b / ymax * 140.0;
        svg.push_str(&format!(
            "<path d=\"M50 {} H670\" class=\"baseline\"/>",
            format_fixed(y, 2)
        ));
    }
    svg.push_str(&format!(
        "<polyline points=\"{}\" class=\"actual\"/>",
        points.join(" ")
    ));
    if let Some(f) = forecast {
        let last_point = point(end, at(rows[rows.len() - 1], "value")?.py_float()?);
        let future = point(xmax, at(f, "value")?.py_float()?);
        svg.push_str(&format!(
            "<polyline points=\"{last_point} {future}\" class=\"forecast\"/>"
        ));
        let lower = point(xmax, at(f, "lower")?.py_float()?);
        let upper = point(xmax, at(f, "upper")?.py_float()?);
        let (xl, yl) = lower.split_once(',').unwrap_or(("", ""));
        let (_, yu) = upper.split_once(',').unwrap_or(("", ""));
        svg.push_str(&format!("<path d=\"M{xl} {yl} V{yu}\" class=\"range\"/>"));
    }
    let first_at = text_of(rows[0], "at")?;
    let last_at = match forecast {
        Some(f) => text_of(f, "at")?,
        None => text_of(rows[rows.len() - 1], "at")?,
    };
    svg.push_str(&format!(
        "<text x=\"50\" y=\"202\">{} UTC</text><text x=\"670\" y=\"202\" text-anchor=\"end\">{} UTC</text></svg>",
        h_str(&clock(first_at)),
        h_str(&clock(last_at))
    ));
    Ok(svg)
}

fn label(level: &str) -> Result<&'static str> {
    Ok(match level {
        "corroborated" => "Corroborated anomaly",
        "elevated" => "Elevated signal",
        "watch" => "Watch",
        "baseline" => "Within observed baseline",
        "insufficient_data" => "Insufficient data",
        other => return Err(Fail::key(other)),
    })
}

fn list<'a>(value: &'a Value, key: &str) -> Result<&'a [Value]> {
    match at(value, key)? {
        Value::List(items) => Ok(items),
        other => Err(Fail::type_error(format!(
            "'{}' object is not a list",
            other.type_name()
        ))),
    }
}

/// The report page as text.
pub fn render_text(report: &Value) -> Result<String> {
    let mut body: Vec<String> = vec![
        format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src data:; base-uri 'none'\"><title>CrowdWatch \u{b7} Movement observatory</title><style>{}</style></head><body>",
            style()
        ),
        format!("<header><div class=\"eyebrow\">SKELETRACE / CROWDWATCH {SHORT_VERSION}</div><h1>Movement, in context.</h1>"),
        "<p class=\"lead\">Local observations. Visible evidence. Honest uncertainty.</p>".to_owned(),
        format!(
            "<div class=\"timestamp\">As of {} \u{b7} UTC \u{b7} offline report</div></header><main>",
            h(at(report, "as_of")?)
        ),
    ];
    if report.get("synthetic")?.is_some_and(Value::truthy) {
        body.push("<div class=\"notice\">SYNTHETIC DEMONSTRATION \u{b7} This report contains invented test observations. Do not interpret it as live conditions.</div>".to_owned());
    }
    let regions = list(report, "regions")?;
    let mut nav = String::from("<nav aria-label=\"Areas\">");
    for r in regions {
        let region = at(r, "region")?;
        nav.push_str(&format!(
            "<a href=\"#{}\">{}</a>",
            h(at(region, "id")?),
            h(at(region, "name")?)
        ));
    }
    nav.push_str("</nav>");
    body.push(nav);
    for r in regions {
        let region = at(r, "region")?;
        let level = text_of(r, "level")?;
        body.push(format!(
            "<section id=\"{}\"><div class=\"section-head\"><div><div class=\"eyebrow\">AREA OBSERVATORY</div><h2>{}</h2><p class=\"muted\">{}, {} \u{b7} {} km radius</p></div><span class=\"badge {}\">{}</span></div>",
            h(at(region, "id")?),
            h(at(region, "name")?),
            h(at(region, "latitude")?),
            h(at(region, "longitude")?),
            h(at(region, "radius_km")?),
            h_str(level),
            h_str(label(level)?)
        ));
        body.push(format!(
            "<div class=\"stats\"><article><strong>{}</strong><span>Anomaly index / 100</span><small>Descriptive, never a probability</small></article><article><strong>{}</strong><span>Fresh direct streams</span><small>Counts and measured movement</small></article><article><strong>{}</strong><span>Context observations</span><small>Events, mobility and conditions</small></article><article><strong>{}</strong><span>Stale streams excluded</span><small>Missing does not mean zero</small></article></div>",
            number(at(r, "anomaly_index")?)?,
            at(r, "direct_streams")?.py_str(),
            list(r, "context")?.len(),
            list(r, "stale_streams")?.len()
        ));
        let reasons = list(r, "reasons")?;
        let mut block = String::from("<div class=\"reasons\"><h3>What the evidence says</h3><ul>");
        for reason in reasons {
            block.push_str(&format!("<li>{}</li>", h(reason)));
        }
        if reasons.is_empty() {
            block.push_str("<li>No unusual signal relative to the available reference. This does not establish safety.</li>");
        }
        block.push_str("</ul></div>");
        body.push(block);
        for sig in list(r, "signals")? {
            let o = at(sig, "observation")?;
            body.push(format!(
                "<article class=\"signal\"><div class=\"section-head\"><div><h3>{}</h3><span class=\"muted\">{} / {} \u{b7} {}</span></div><div class=\"reading\">{} <small>{}</small></div></div>",
                name_of(o)?,
                h(at(o, "source_id")?),
                h(at(o, "stream_id")?),
                h(at(o, "metric")?),
                number(at(o, "value")?)?,
                h_str(&display_unit(o)?)
            ));
            body.push(chart(sig)?);
            body.push("<div class=\"legend\"><span class=\"cyan\">\u{2501} observed</span><span class=\"amber\">\u{2504} conditional projection</span><span>\u{2504} baseline median</span></div>".to_owned());
            let baseline = at(sig, "baseline")?;
            if baseline.truthy() {
                body.push(format!(
                    "<p>Baseline {} \u{b7} robust z {} \u{b7} {} reference samples \u{b7} {}</p>",
                    number(at(baseline, "median")?)?,
                    number(at(sig, "z")?)?,
                    at(baseline, "samples")?.py_str(),
                    h(at(baseline, "mode")?)
                ));
            } else {
                body.push("<p class=\"muted\">Baseline is still warming up; no anomaly claim for this stream.</p>".to_owned());
            }
            let projection = at(sig, "projection")?;
            if projection.truthy() {
                body.push(format!(
                    "<p>At +{} minutes from the latest reading: <b>{}</b> {}; sensitivity range {}\u{2013}{}. Range coverage is uncalibrated. Method: {}.</p>",
                    number(at(projection, "horizon_minutes")?)?,
                    number(at(projection, "value")?)?,
                    h_str(&display_unit(o)?),
                    number(at(projection, "lower")?)?,
                    number(at(projection, "upper")?)?,
                    h(at(projection, "method")?)
                ));
            }
            body.push(format!(
                "<details><summary>Observation provenance</summary><p>Observed {}<br>Retrieved {}<br>Valid until {}<br>Interval {} s \u{b7} assigned quality {}</p>",
                h(at(o, "observed_at")?),
                h(at(o, "retrieved_at")?),
                h(at(o, "valid_until")?),
                at(o, "period_seconds")?.py_str(),
                number(at(o, "quality")?)?
            ));
            let provenance = at(o, "provenance_url")?;
            if provenance.truthy() {
                body.push(format!(
                    "<a href=\"{}\" rel=\"noreferrer\">Open original source</a>",
                    h(provenance)
                ));
            }
            body.push("</details></article>".to_owned());
        }
        let bal = at(r, "balance_projection")?;
        if bal.truthy() {
            if text_of(bal, "status")? == "available" {
                body.push(format!(
                    "<div class=\"reasons\"><h3>Conditional area balance</h3><p>Measured {} people + net {} people/min \u{2192} {} people at +{} minutes from the occupancy reading.</p><p class=\"muted\">{}</p></div>",
                    number(at(bal, "occupancy")?)?,
                    number(at(bal, "net_people_per_minute")?)?,
                    number(at(bal, "projected_occupancy")?)?,
                    number(at(bal, "horizon_minutes")?)?,
                    h(at(bal, "assumptions")?)
                ));
            } else {
                body.push(format!(
                    "<p>Area balance unavailable: {}.</p>",
                    h(at(bal, "reason")?)
                ));
            }
        }
        let context = list(r, "context")?;
        if !context.is_empty() {
            body.push("<details class=\"context\" open><summary>Context \u{b7} never converted into headcounts</summary><div class=\"table-wrap\"><table><thead><tr><th>Source / observation</th><th>Type</th><th>Value</th><th>Observed UTC</th></tr></thead><tbody>".to_owned());
            for o in context {
                let observed: String = text_of(o, "observed_at")?
                    .chars()
                    .skip(11)
                    .take(8)
                    .collect();
                body.push(format!(
                    "<tr><td>{}<small>{}</small></td><td>{}</td><td>{} {}</td><td>{}</td></tr>",
                    name_of(o)?,
                    h(at(o, "source_id")?),
                    h(at(o, "metric")?),
                    number(at(o, "value")?)?,
                    h(at(o, "unit")?),
                    h_str(&observed)
                ));
            }
            body.push("</tbody></table></div></details>".to_owned());
        }
        let stale = list(r, "stale_streams")?;
        if !stale.is_empty() {
            let mut block = String::from("<details><summary>Excluded stale streams</summary><ul>");
            for s in stale {
                block.push_str(&format!(
                    "<li>{} / {} ({}): {}</li>",
                    h(at(s, "source_id")?),
                    h(at(s, "stream_id")?),
                    h(at(s, "metric")?),
                    h(at(s, "last_observed")?)
                ));
            }
            block.push_str("</ul></details>");
            body.push(block);
        }
        body.push("</section>".to_owned());
    }
    body.push("<section><h2>Collection health</h2><p class=\"muted\">Latest collector state, which can be later than an historical report cutoff.</p><div class=\"table-wrap\"><table><thead><tr><th>Source</th><th>Status</th><th>Last success</th><th>Detail</th></tr></thead><tbody>".to_owned());
    let health = list(report, "source_health")?;
    let empty = Value::Dict(crate::json::Dict::new());
    for source in list(report, "sources")? {
        let id = at(source, "id")?;
        // {s["source_id"]: s for s in health}: the last row for an ID wins.
        let s = health
            .iter()
            .rev()
            .find(|row| {
                row.get("source_id")
                    .ok()
                    .flatten()
                    .is_some_and(|v| crate::json::py_eq(v, id))
            })
            .unwrap_or(&empty);
        let state = if at(source, "enabled")?.truthy() {
            s.get("status")?
                .cloned()
                .unwrap_or_else(|| Value::from("not collected"))
        } else {
            Value::from("disabled")
        };
        let error = s.get("error")?.cloned().unwrap_or_else(|| Value::from(""));
        let detail = if error.truthy() {
            error.py_str()
        } else {
            let warnings = s
                .get("warnings")?
                .cloned()
                .unwrap_or_else(|| Value::List(Vec::new()));
            let mut parts = Vec::new();
            for w in warnings.py_iter()? {
                match w {
                    Value::Str(text) => parts.push(text),
                    other => {
                        return Err(Fail::type_error(format!(
                            "sequence item: expected str instance, {} found",
                            other.type_name()
                        )))
                    }
                }
            }
            parts.join("; ")
        };
        let success = s.get("last_success")?.cloned().unwrap_or(Value::Null);
        let success = if success.truthy() {
            h(&success)
        } else {
            "\u{2014}".to_owned()
        };
        body.push(format!(
            "<tr><td>{}<small>{}</small></td><td>{}</td><td>{}</td><td>{}</td></tr>",
            h(id),
            h(at(source, "attribution")?),
            h(&state),
            success,
            h_str(&detail)
        ));
    }
    body.push(format!(
        "</tbody></table></div></section></main><footer><b>Interpretation boundary</b><p>{}</p><p>Area-level public and supplied aggregate data. No individual tracking. Built for inspection, comparison and validation.</p></footer></body></html>",
        h(at(report, "limitations")?)
    ));
    Ok(body.join("\n"))
}

/// 0.1.0's `render(report, path)`: write the report page.
pub fn render(report: &Value, path: &Path) -> Result<()> {
    atomic_write(path, &render_text(report)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_print_like_0_1_0() {
        // Values from 0.1.0's number() on Python.
        for (value, text) in [
            (Value::Null, "\u{2014}"),
            (Value::Int(0), "0"),
            (Value::Int(1234), "1,234"),
            (Value::Float(1234.5), "1,234.5"),
            (Value::Float(0.004), "0"),
            (Value::Float(-0.004), "-0"),
            (Value::Float(100.0), "100"),
            (Value::Float(2.675), "2.67"),
            (Value::Float(1e21), "1,000,000,000,000,000,000,000"),
        ] {
            assert_eq!(number(&value).unwrap(), text, "{value:?}");
        }
        assert_eq!(STYLE.lines().count(), 1);
    }
}
