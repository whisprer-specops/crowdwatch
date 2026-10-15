// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Python's text and number rules, reproduced exactly.
//!
//! CrowdWatch 0.1.0 was written in Python, and its stored keys, digests, reports and
//! exports depend on how Python prints a float, reads a number from text, splits lines or
//! sums a list. Each function here names the Python behaviour it copies, so 0.2.0 writes
//! the same bytes and reads the same files.

use crate::error::{Fail, Result};
use crate::unicode_tables::{ALNUM, DIGIT, DIGIT_ZEROS, SPACE, UNPRINTABLE};

fn in_ranges(table: &[(u32, u32)], c: char) -> bool {
    let cp = u32::from(c);
    table
        .binary_search_by(|&(low, high)| {
            if high < cp {
                std::cmp::Ordering::Less
            } else if low > cp {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// Python's `str.isspace()` for one character: bidirectional class WS, B or S, or general
/// category Zs (which takes in the ASCII information separators U+001C to U+001F).
pub fn is_space(c: char) -> bool {
    if c.is_ascii() {
        return matches!(c, ' ' | '\t'..='\r' | '\u{1c}'..='\u{1f}');
    }
    in_ranges(SPACE, c)
}

/// Python's `str.strip()`.
pub fn strip(text: &str) -> &str {
    text.trim_matches(is_space)
}

/// C's `isspace` in the "C" locale, as CPython's number parsers use it.
fn is_c_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
}

/// Python's `str.splitlines()`: lines without their endings, breaking at `\n`, `\r`,
/// `\r\n`, `\v`, `\f`, U+001C to U+001E, U+0085, U+2028 and U+2029.
pub fn splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        let is_break = matches!(
            c,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if !is_break {
            continue;
        }
        lines.push(&text[start..index]);
        let mut end = index + c.len_utf8();
        if c == '\r' {
            if let Some(&(next, '\n')) = chars.peek() {
                chars.next();
                end = next + 1;
            }
        }
        start = end;
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// Python's `str.isalnum()`: not empty, and every character a letter or a number.
pub fn isalnum(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| in_ranges(ALNUM, c))
}

/// Python's `str.isdigit()` for one character.
pub fn is_digit(c: char) -> bool {
    in_ranges(DIGIT, c)
}

/// Python's `str.isprintable()` for one character.
pub fn is_printable(c: char) -> bool {
    !in_ranges(UNPRINTABLE, c)
}

/// The value of a Unicode decimal digit, as `int()` and `float()` read it.
pub fn decimal_value(c: char) -> Option<u32> {
    if c.is_ascii_digit() {
        return Some(u32::from(c) - u32::from('0'));
    }
    let cp = u32::from(c);
    let index = DIGIT_ZEROS.partition_point(|&zero| zero <= cp);
    let zero = DIGIT_ZEROS.get(index.checked_sub(1)?)?;
    let value = cp - zero;
    (value < 10).then_some(value)
}

/// Python's `repr()` of a string.
pub fn repr_str(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c == '\x7f' => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c if c.is_ascii() || is_printable(c) => out.push(c),
            c if (c as u32) < 0x100 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) < 0x10000 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push(quote);
    out
}

/// Python's `repr(float)`, which `str()` and `json.dumps` also print.
///
/// The shortest digits that read back as the same number, in fixed notation unless the
/// decimal point sits more than 16 places right or 4 places left of the first digit.
pub fn float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0"
        } else {
            "0.0"
        }
        .to_owned();
    }
    let Some((digits, exponent)) = shortest_digits(value.abs()) else {
        return format!("{value:e}");
    };
    // The value is 0.DIGITS x 10^point.
    let point = exponent + 1;
    let mut out = String::with_capacity(digits.len() + 8);
    if value < 0.0 {
        out.push('-');
    }
    if point <= -4 || point > 16 {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let shown = point - 1;
        out.push('e');
        out.push(if shown < 0 { '-' } else { '+' });
        out.push_str(&format!("{:02}", shown.unsigned_abs()));
    } else if point <= 0 {
        out.push_str("0.");
        for _ in 0..point.unsigned_abs() {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        let whole = point.unsigned_abs() as usize;
        if digits.len() <= whole {
            out.push_str(&digits);
            for _ in digits.len()..whole {
                out.push('0');
            }
            out.push_str(".0");
        } else {
            out.push_str(&digits[..whole]);
            out.push('.');
            out.push_str(&digits[whole..]);
        }
    }
    out
}

/// The shortest digits that read back as `value` (positive and finite), and the decimal
/// exponent of the first digit, chosen as Python's repr chooses them.
fn shortest_digits(value: f64) -> Option<(String, i32)> {
    let (digits, exponent) = split_scientific(&format!("{value:e}"))?;
    // When the exact value lies exactly halfway between two shortest candidates, Python
    // keeps the even last digit and Rust's `{:e}` rounds up. Such ties need about 16
    // digits, where neighbouring doubles are coarser than the last decimal place.
    let last = digits.as_bytes().last().copied()?;
    if digits.len() < 15 || last % 2 == 0 {
        return Some((digits, exponent));
    }
    let mut lower = digits.clone();
    lower.pop();
    lower.push(char::from(last - 1));
    let midpoint = format!("{lower}5");
    // 800 places is enough to write any double exactly.
    let (exact, exact_exponent) = split_scientific(&format!("{value:.800e}"))?;
    let is_tie = exact_exponent == exponent
        && exact.starts_with(&midpoint)
        && exact[midpoint.len()..].bytes().all(|b| b == b'0');
    if is_tie {
        let trimmed = lower.trim_end_matches('0');
        let lower = if trimmed.is_empty() { "0" } else { trimmed };
        return Some((lower.to_owned(), exponent));
    }
    Some((digits, exponent))
}

/// `"1.2345e-7"` to `("12345", -7)`.
fn split_scientific(text: &str) -> Option<(String, i32)> {
    let (mantissa, exponent) = text.split_once('e')?;
    let exponent = exponent.parse::<i32>().ok()?;
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    Some((digits, exponent))
}

/// Python's `f"{value:.{decimals}f}"`: the exact binary value rounded half to even.
pub fn format_fixed(value: f64, decimals: usize) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    format!("{value:.decimals$}")
}

/// Python's `f"{value:,.{decimals}f}"`: as `format_fixed`, with commas between thousands.
pub fn format_grouped(value: f64, decimals: usize) -> String {
    let plain = format_fixed(value, decimals);
    if !value.is_finite() {
        return plain;
    }
    let (sign, rest) = match plain.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", plain.as_str()),
    };
    let (whole, fraction) = match rest.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (rest, None),
    };
    let mut grouped = String::with_capacity(whole.len() + whole.len() / 3);
    for (index, digit) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    match fraction {
        Some(fraction) => format!("{sign}{grouped}.{fraction}"),
        None => format!("{sign}{grouped}"),
    }
}

/// Python's built-in `sum()` over floats (CPython 3.12 and later): compensated
/// (Neumaier) summation, so totals match 0.1.0 to the last bit.
pub fn float_sum<I: IntoIterator<Item = f64>>(values: I) -> f64 {
    let mut iter = values.into_iter();
    let Some(first) = iter.next() else {
        return 0.0;
    };
    // sum() starts from the integer 0, and 0 + x is a float equal to x (-0.0 becomes 0.0).
    let mut total = 0.0 + first;
    let mut compensation = 0.0_f64;
    for x in iter {
        let t = total + x;
        if total.abs() >= x.abs() {
            compensation += (total - t) + x;
        } else {
            compensation += (x - t) + total;
        }
        total = t;
    }
    if compensation != 0.0 && compensation.is_finite() {
        total += compensation;
    }
    total
}

/// Python's `statistics.median()` of floats; `None` for no data.
pub fn median(values: &[f64]) -> Option<f64> {
    let mut sorted = values.to_vec();
    // A stable sort by `<`, as Python's sorted() is.
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len();
    if n == 0 {
        return None;
    }
    if n % 2 == 1 {
        Some(sorted[n / 2])
    } else {
        Some((sorted[n / 2 - 1] + sorted[n / 2]) / 2.0)
    }
}

/// CPython's `_PyUnicode_TransformDecimalAndSpaceToASCII`, ahead of `int()` and `float()`:
/// non-ASCII whitespace becomes a space, Unicode digits become ASCII digits, and any other
/// non-ASCII character ends the text with `?`, which no number parser accepts.
fn decimal_and_space_to_ascii(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if (c as u32) < 127 {
            out.push(c);
        } else if is_space(c) {
            out.push(' ');
        } else if let Some(digit) = decimal_value(c) {
            out.push(char::from(b'0' + digit as u8));
        } else {
            out.push('?');
            break;
        }
    }
    out
}

/// CPython's `_Py_string_to_number_with_underscores`: underscores only between digits.
fn remove_underscores(text: &str) -> Option<String> {
    if !text.contains('_') {
        return Some(text.to_owned());
    }
    let mut out = String::with_capacity(text.len());
    let mut previous = '\0';
    for c in text.chars() {
        if c == '\0' {
            return None;
        }
        if c == '_' {
            if !previous.is_ascii_digit() {
                return None;
            }
        } else {
            if previous == '_' && !c.is_ascii_digit() {
                return None;
            }
            out.push(c);
        }
        previous = c;
    }
    if previous == '_' {
        return None;
    }
    Some(out)
}

/// Python's `float(text)`.
pub fn float_from_str(text: &str) -> Result<f64> {
    let invalid = || {
        Fail::value_error(format!(
            "could not convert string to float: {}",
            repr_str(text)
        ))
    };
    let ascii = decimal_and_space_to_ascii(text);
    let cleaned = remove_underscores(&ascii).ok_or_else(invalid)?;
    let bytes = cleaned.as_bytes();
    let mut start = 0;
    let mut end = bytes.len();
    while start < end && is_c_space(bytes[start]) {
        start += 1;
    }
    if start == end {
        return Err(invalid());
    }
    while end - start > 1 && is_c_space(bytes[end - 1]) {
        end -= 1;
    }
    let body = &cleaned[start..end];
    if !is_python_float_literal(body) {
        return Err(invalid());
    }
    body.parse::<f64>().map_err(|_| invalid())
}

/// The forms CPython's `PyOS_string_to_double` reads: decimal floats, `inf`, `infinity` and
/// `nan` in any case, each with an optional sign.
fn is_python_float_literal(text: &str) -> bool {
    let body = text.strip_prefix(['+', '-']).unwrap_or(text);
    let lower = body.to_ascii_lowercase();
    if matches!(lower.as_str(), "inf" | "infinity" | "nan") {
        return true;
    }
    let bytes = body.as_bytes();
    let mut pos = 0;
    let whole = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    pos += whole;
    let mut fraction = 0;
    if bytes.get(pos) == Some(&b'.') {
        pos += 1;
        fraction = bytes[pos..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        pos += fraction;
    }
    if whole == 0 && fraction == 0 {
        return false;
    }
    if matches!(bytes.get(pos), Some(b'e' | b'E')) {
        pos += 1;
        if matches!(bytes.get(pos), Some(b'+' | b'-')) {
            pos += 1;
        }
        let exponent = bytes[pos..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if exponent == 0 {
            return false;
        }
        pos += exponent;
    }
    pos == bytes.len()
}

/// Python's `int(text)` in base 10, as canonical decimal digits (`"-12"`, `"0"`).
pub fn int_from_str(text: &str) -> Result<String> {
    let invalid = || {
        Fail::value_error(format!(
            "invalid literal for int() with base 10: {}",
            repr_str(text)
        ))
    };
    let ascii = decimal_and_space_to_ascii(text);
    let bytes = ascii.as_bytes();
    let mut start = 0;
    let mut end = bytes.len();
    while start < end && is_c_space(bytes[start]) {
        start += 1;
    }
    while end > start && is_c_space(bytes[end - 1]) {
        end -= 1;
    }
    let body = &ascii[start..end];
    let (negative, digits) = match body.as_bytes().first() {
        Some(b'-') => (true, &body[1..]),
        Some(b'+') => (false, &body[1..]),
        _ => (false, body),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.bytes().all(|b| b.is_ascii_digit() || b == b'_')
    {
        return Err(invalid());
    }
    let plain: String = digits.chars().filter(|&c| c != '_').collect();
    if plain.len() > 4300 {
        return Err(Fail::value_error(format!(
            "Exceeds the limit (4300 digits) for integer string conversion: value has {} digits; use sys.set_int_max_str_digits() to increase the limit",
            plain.len()
        )));
    }
    let trimmed = plain.trim_start_matches('0');
    if trimmed.is_empty() {
        return Ok("0".to_owned());
    }
    Ok(if negative {
        format!("-{trimmed}")
    } else {
        trimmed.to_owned()
    })
}

/// Python's `html.escape(text, quote=True)`.
pub fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            c => out.push(c),
        }
    }
    out
}

/// Python's `str[:limit]`: at most `limit` characters.
pub fn truncate_chars(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((index, _)) => text[..index].to_owned(),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_print_like_python() {
        for (value, text) in [
            (0.1, "0.1"),
            (1e16, "1e+16"),
            (123456789012345680.0, "1.2345678901234568e+17"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (-0.0, "-0.0"),
            (2.5, "2.5"),
            (100.0, "100.0"),
            (948_841_720_842_342.0 + 0.25, "948841720842342.2"),
        ] {
            assert_eq!(float_repr(value), text, "{value:e}");
        }
    }

    #[test]
    fn fixed_and_grouped_formats() {
        assert_eq!(format_fixed(0.125, 2), "0.12");
        assert_eq!(format_fixed(0.375, 2), "0.38");
        assert_eq!(format_fixed(2.5, 0), "2");
        assert_eq!(format_fixed(-0.001, 2), "-0.00");
        assert_eq!(format_grouped(1234567.891, 2), "1,234,567.89");
        assert_eq!(format_grouped(-1000.0, 2), "-1,000.00");
        assert_eq!(format_grouped(999.999, 2), "1,000.00");
        assert_eq!(format_grouped(f64::NAN, 2), "nan");
    }

    #[test]
    fn numbers_read_like_python() {
        assert_eq!(float_from_str(" 1_000.5\n").unwrap(), 1000.5);
        assert_eq!(float_from_str("\u{0661}\u{0662}").unwrap(), 12.0);
        assert_eq!(float_from_str("-Infinity").unwrap(), f64::NEG_INFINITY);
        assert_eq!(float_from_str("1e400").unwrap(), f64::INFINITY);
        for bad in [
            "", " ", "1__0", "_1", "1_", "1e", "e5", ".", "0x10", "1.5.2", "\x1c5",
        ] {
            assert!(float_from_str(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            float_from_str("abc").unwrap_err().to_string(),
            "could not convert string to float: 'abc'"
        );
        assert_eq!(int_from_str(" -007 ").unwrap(), "-7");
        assert_eq!(int_from_str("1_000").unwrap(), "1000");
        assert!(int_from_str("1.0").is_err());
        assert!(int_from_str("").is_err());
    }

    #[test]
    fn lines_and_repr() {
        assert_eq!(
            splitlines("a\r\nb\rc\n\nd\u{2028}e"),
            ["a", "b", "c", "", "d", "e"]
        );
        assert_eq!(splitlines("a\n"), ["a"]);
        assert!(splitlines("").is_empty());
        assert_eq!(repr_str("it's"), "\"it's\"");
        assert_eq!(
            repr_str("a\u{0}\u{7f}\u{200b}é\n"),
            "'a\\x00\\x7f\\u200bé\\n'"
        );
        assert_eq!(repr_str("'\""), "'\\'\"'");
        assert!(isalnum("abc123é"));
        assert!(!isalnum("a-b"));
        assert!(!isalnum(""));
    }

    #[test]
    fn sums_and_medians_match_python() {
        assert_eq!(float_sum([0.1, 0.2, 0.3]), 0.6);
        assert_eq!(float_sum([1e100, 1.0, -1e100]), 1.0);
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), Some(2.5));
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn html_is_escaped_like_python() {
        assert_eq!(
            html_escape("<a href=\"x\">'&'</a>"),
            "&lt;a href=&quot;x&quot;&gt;&#x27;&amp;&#x27;&lt;/a&gt;"
        );
    }
}
