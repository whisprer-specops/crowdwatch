// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! JSON values that behave like the Python objects CrowdWatch 0.1.0 handled.
//!
//! Python keeps `20` and `20.0` apart, keeps keys in insertion order, prints floats with
//! `repr` and raises a named exception for each wrong-shaped access. All of that reaches
//! CrowdWatch's files: reports print `0` where Python had the integer 0, and a collector
//! records "KeyError" when a provider drops a field. This module keeps those rules.

use std::fmt::Write as _;

use crate::error::{Fail, Result};
use crate::pyfmt::{float_from_str, float_repr, int_from_str, repr_str};

/// The deepest nesting accepted, so neither reading nor writing can exhaust the stack.
pub const MAX_DEPTH: usize = 1000;

/// A JSON value, with Python's distinction between integers and floats.
#[derive(Clone, Debug)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    /// An integer outside `i64`, as canonical decimal digits.
    BigInt(String),
    Float(f64),
    Str(String),
    List(Vec<Value>),
    Dict(Dict),
}

/// An insertion-ordered mapping with string keys, like a Python `dict`.
#[derive(Clone, Debug, Default)]
pub struct Dict(Vec<(String, Value)>);

impl Dict {
    pub fn new() -> Dict {
        Dict(Vec::new())
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.0.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.0.iter().any(|(k, _)| k == key)
    }

    /// `d[key] = value`: an existing key keeps its place.
    pub fn insert(&mut self, key: impl Into<String>, value: Value) {
        let key = key.into();
        match self.get_mut(&key) {
            Some(slot) => *slot = value,
            None => self.0.push((key, value)),
        }
    }

    /// Insert and return `self`, for building literals.
    pub fn with(mut self, key: impl Into<String>, value: impl Into<Value>) -> Dict {
        self.insert(key, value.into());
        self
    }

    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let index = self.0.iter().position(|(k, _)| k == key)?;
        Some(self.0.remove(index).1)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.0.iter().map(|(k, v)| (k, v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.0.iter().map(|(k, _)| k)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Value {
        Value::Bool(value)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Value {
        Value::Int(value)
    }
}

impl From<usize> for Value {
    fn from(value: usize) -> Value {
        Value::Int(i64::try_from(value).unwrap_or(i64::MAX))
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Value {
        Value::Float(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Value {
        Value::Str(value.to_owned())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Value {
        Value::Str(value)
    }
}

impl From<Dict> for Value {
    fn from(value: Dict) -> Value {
        Value::Dict(value)
    }
}

impl From<Vec<Value>> for Value {
    fn from(value: Vec<Value>) -> Value {
        Value::List(value)
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(value: Option<T>) -> Value {
        value.map_or(Value::Null, Into::into)
    }
}

impl Value {
    /// Python's type name, as exception messages print it.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "NoneType",
            Value::Bool(_) => "bool",
            Value::Int(_) | Value::BigInt(_) => "int",
            Value::Float(_) => "float",
            Value::Str(_) => "str",
            Value::List(_) => "list",
            Value::Dict(_) => "dict",
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&Dict> {
        match self {
            Value::Dict(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&Vec<Value>> {
        match self {
            Value::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// `isinstance(x, (int, float))` without `bool`: a real number.
    pub fn is_number(&self) -> bool {
        matches!(self, Value::Int(_) | Value::BigInt(_) | Value::Float(_))
    }

    /// `isinstance(x, int)`, which is also true of `bool`.
    pub fn is_int_like(&self) -> bool {
        matches!(self, Value::Int(_) | Value::BigInt(_) | Value::Bool(_))
    }

    /// Python truthiness.
    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Int(i) => *i != 0,
            Value::BigInt(_) => true,
            Value::Float(f) => *f != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::List(l) => !l.is_empty(),
            Value::Dict(d) => !d.is_empty(),
        }
    }

    /// Python's `str(x)`.
    pub fn py_str(&self) -> String {
        match self {
            Value::Str(s) => s.clone(),
            other => other.py_repr(),
        }
    }

    /// Python's `repr(x)`.
    pub fn py_repr(&self) -> String {
        match self {
            Value::Null => "None".to_owned(),
            Value::Bool(true) => "True".to_owned(),
            Value::Bool(false) => "False".to_owned(),
            Value::Int(i) => i.to_string(),
            Value::BigInt(s) => s.clone(),
            Value::Float(f) => float_repr(*f),
            Value::Str(s) => repr_str(s),
            Value::List(items) => {
                let inner: Vec<String> = items.iter().map(Value::py_repr).collect();
                format!("[{}]", inner.join(", "))
            }
            Value::Dict(d) => {
                let inner: Vec<String> = d
                    .iter()
                    .map(|(k, v)| format!("{}: {}", repr_str(k), v.py_repr()))
                    .collect();
                format!("{{{}}}", inner.join(", "))
            }
        }
    }

    /// Python's `float(x)`.
    pub fn py_float(&self) -> Result<f64> {
        match self {
            Value::Float(f) => Ok(*f),
            Value::Int(i) => Ok(*i as f64),
            Value::BigInt(s) => big_to_f64(s),
            Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            Value::Str(s) => float_from_str(s),
            other => Err(Fail::type_error(format!(
                "float() argument must be a string or a real number, not '{}'",
                other.type_name()
            ))),
        }
    }

    /// Python's `int(x)`.
    pub fn py_int(&self) -> Result<Value> {
        match self {
            Value::Int(_) | Value::BigInt(_) => Ok(self.clone()),
            Value::Bool(b) => Ok(Value::Int(i64::from(*b))),
            Value::Float(f) => float_to_int(*f),
            Value::Str(s) => Ok(int_value(int_from_str(s)?)),
            other => Err(Fail::type_error(format!(
                "int() argument must be a string, a bytes-like object or a real number, not '{}'",
                other.type_name()
            ))),
        }
    }

    /// Python's `x[key]` with a string key.
    pub fn index(&self, key: &str) -> Result<&Value> {
        match self {
            Value::Dict(d) => d.get(key).ok_or_else(|| Fail::key(key)),
            Value::List(_) => Err(Fail::type_error(
                "list indices must be integers or slices, not str",
            )),
            Value::Str(_) => Err(Fail::type_error(
                "string indices must be integers, not 'str'",
            )),
            other => Err(Fail::type_error(format!(
                "'{}' object is not subscriptable",
                other.type_name()
            ))),
        }
    }

    /// Python's `x[position]` with an integer.
    pub fn index_at(&self, position: i64) -> Result<Value> {
        let out_of_range =
            |what: &str| Fail::py("IndexError", format!("{what} index out of range"));
        match self {
            Value::List(items) => {
                let index =
                    normalise_index(position, items.len()).ok_or_else(|| out_of_range("list"))?;
                Ok(items[index].clone())
            }
            Value::Str(s) => {
                let count = s.chars().count();
                let index =
                    normalise_index(position, count).ok_or_else(|| out_of_range("string"))?;
                Ok(Value::Str(
                    s.chars().nth(index).map(String::from).unwrap_or_default(),
                ))
            }
            Value::Dict(_) => Err(Fail::py("KeyError", position.to_string())),
            other => Err(Fail::type_error(format!(
                "'{}' object is not subscriptable",
                other.type_name()
            ))),
        }
    }

    /// Python's `x.get(key)`; anything but a dict has no `get`.
    pub fn get(&self, key: &str) -> Result<Option<&Value>> {
        match self {
            Value::Dict(d) => Ok(d.get(key)),
            other => Err(no_attribute(other, "get")),
        }
    }

    /// Python's `x.get(key, default)`.
    pub fn get_or<'a>(&'a self, key: &str, default: &'a Value) -> Result<&'a Value> {
        Ok(self.get(key)?.unwrap_or(default))
    }

    /// Python's `needle in self`.
    pub fn contains(&self, needle: &Value) -> Result<bool> {
        match self {
            Value::Dict(d) => match needle {
                Value::Str(key) => Ok(d.contains_key(key)),
                Value::List(_) | Value::Dict(_) => Err(Fail::type_error(format!(
                    "unhashable type: '{}'",
                    needle.type_name()
                ))),
                _ => Ok(false),
            },
            Value::List(items) => Ok(items.iter().any(|item| py_eq(item, needle))),
            Value::Str(haystack) => match needle {
                Value::Str(n) => Ok(haystack.contains(n.as_str())),
                other => Err(Fail::type_error(format!(
                    "'in <string>' requires string as left operand, not {}",
                    other.type_name()
                ))),
            },
            other => Err(Fail::type_error(format!(
                "argument of type '{}' is not iterable",
                other.type_name()
            ))),
        }
    }

    /// The items `for x in self` visits.
    pub fn py_iter(&self) -> Result<Vec<Value>> {
        match self {
            Value::List(items) => Ok(items.clone()),
            Value::Dict(d) => Ok(d.keys().map(|k| Value::Str(k.clone())).collect()),
            Value::Str(s) => Ok(s.chars().map(|c| Value::Str(c.to_string())).collect()),
            other => Err(Fail::type_error(format!(
                "'{}' object is not iterable",
                other.type_name()
            ))),
        }
    }

    /// Python's `len(x)`.
    pub fn py_len(&self) -> Result<usize> {
        match self {
            Value::List(items) => Ok(items.len()),
            Value::Dict(d) => Ok(d.len()),
            Value::Str(s) => Ok(s.chars().count()),
            other => Err(Fail::type_error(format!(
                "object of type '{}' has no len()",
                other.type_name()
            ))),
        }
    }
}

fn normalise_index(position: i64, len: usize) -> Option<usize> {
    let len = i64::try_from(len).ok()?;
    let index = if position < 0 {
        position + len
    } else {
        position
    };
    (0..len).contains(&index).then_some(index as usize)
}

/// Python's error for calling a method an object lacks.
pub fn no_attribute(value: &Value, name: &str) -> Fail {
    Fail::py(
        "AttributeError",
        format!("'{}' object has no attribute '{name}'", value.type_name()),
    )
}

/// An integer value from canonical decimal digits.
pub fn int_value(digits: String) -> Value {
    match digits.parse::<i64>() {
        Ok(i) => Value::Int(i),
        Err(_) => Value::BigInt(digits),
    }
}

/// Python's `float(int)` for an integer outside `i64`: correctly rounded, or OverflowError.
fn big_to_f64(digits: &str) -> Result<f64> {
    let value: f64 = digits
        .parse()
        .map_err(|_| Fail::value_error("invalid integer"))?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(Fail::overflow("int too large to convert to float"))
    }
}

/// Python's `int(float)`: truncation toward zero.
fn float_to_int(value: f64) -> Result<Value> {
    if value.is_nan() {
        return Err(Fail::value_error("cannot convert float NaN to integer"));
    }
    if value.is_infinite() {
        return Err(Fail::overflow("cannot convert float infinity to integer"));
    }
    let truncated = value.trunc();
    if truncated.abs() < 9.0e18 {
        return Ok(Value::Int(truncated as i64));
    }
    Ok(int_value(format!("{truncated:.0}")))
}

/// Python's `a == b` for JSON values: numbers compare exactly across int, float and bool,
/// and dicts compare without regard to key order.
pub fn py_eq(a: &Value, b: &Value) -> bool {
    use Value::*;
    match (a, b) {
        (Null, Null) => true,
        (Str(x), Str(y)) => x == y,
        (List(x), List(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_eq(p, q)),
        (Dict(x), Dict(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|other| py_eq(v, other)))
        }
        _ => match (exact_number(a), exact_number(b)) {
            (Some(p), Some(q)) => p == q,
            _ => false,
        },
    }
}

/// A number in a form that compares exactly: an integer's digits (integral floats
/// included), or a non-integral float's bits.
#[derive(PartialEq)]
enum Exact {
    Integer(String),
    Fraction(u64),
}

fn exact_number(value: &Value) -> Option<Exact> {
    match value {
        Value::Bool(b) => Some(Exact::Integer(u8::from(*b).to_string())),
        Value::Int(i) => Some(Exact::Integer(i.to_string())),
        Value::BigInt(s) => Some(Exact::Integer(s.clone())),
        Value::Float(f) if f.is_finite() && f.fract() == 0.0 => {
            let text = format!("{f:.0}");
            Some(Exact::Integer(if text == "-0" {
                "0".to_owned()
            } else {
                text
            }))
        }
        // NaN never equals anything, itself included.
        Value::Float(f) if f.is_nan() => None,
        Value::Float(f) => Some(Exact::Fraction(f.to_bits())),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------------------

/// Why a document could not be read: an ordinary syntax error, or one of the two
/// conditions CrowdWatch 0.1.0 reported in its own words.
#[derive(Clone, Debug, PartialEq)]
pub enum ReadError {
    Syntax,
    Constant,
    DuplicateKey(String),
    /// An integer of more than 4,300 digits (Python's ValueError).
    TooManyDigits(String),
}

/// Python's `json.loads(text)` for a `str`, with CrowdWatch's hooks: `NaN`, `Infinity` and
/// duplicate keys are refused where Python's parser meets them.
pub fn parse_str(text: &str) -> std::result::Result<Value, ReadError> {
    if text.starts_with('\u{feff}') {
        return Err(ReadError::Syntax);
    }
    let mut parser = Parser {
        chars: text.chars().collect(),
        pos: 0,
    };
    parser.skip_ws();
    let value = parser.value(0)?;
    parser.skip_ws();
    if parser.pos != parser.chars.len() {
        return Err(ReadError::Syntax);
    }
    Ok(value)
}

/// Python's `json.loads(bytes)`: UTF-8, UTF-16 or UTF-32, told apart as Python's
/// `json.detect_encoding` does, with a UTF-8 byte-order mark allowed. Lone surrogates
/// (which Python kept, and later failed to write) become U+FFFD.
pub fn parse_bytes(bytes: &[u8]) -> std::result::Result<Value, ReadError> {
    let text = decode_json_bytes(bytes).ok_or(ReadError::Syntax)?;
    parse_str(&text)
}

fn decode_json_bytes(b: &[u8]) -> Option<String> {
    const BOM8: &[u8] = b"\xef\xbb\xbf";
    if b.starts_with(&[0x00, 0x00, 0xfe, 0xff]) {
        return decode_utf32(&b[4..], true);
    }
    if b.starts_with(&[0xff, 0xfe, 0x00, 0x00]) {
        return decode_utf32(&b[4..], false);
    }
    if b.starts_with(&[0xfe, 0xff]) {
        return decode_utf16(&b[2..], true);
    }
    if b.starts_with(&[0xff, 0xfe]) {
        return decode_utf16(&b[2..], false);
    }
    if let Some(rest) = b.strip_prefix(BOM8) {
        return decode_utf8_surrogatepass(rest);
    }
    if b.len() >= 4 {
        if b[0] == 0 {
            return if b[1] != 0 {
                decode_utf16(b, true)
            } else {
                decode_utf32(b, true)
            };
        }
        if b[1] == 0 {
            return if b[2] != 0 || b[3] != 0 {
                decode_utf16(b, false)
            } else {
                decode_utf32(b, false)
            };
        }
    } else if b.len() == 2 {
        if b[0] == 0 {
            return decode_utf16(b, true);
        }
        if b[1] == 0 {
            return decode_utf16(b, false);
        }
    }
    decode_utf8_surrogatepass(b)
}

/// UTF-8 with Python's `surrogatepass`: encoded surrogates are accepted (as U+FFFD here).
fn decode_utf8_surrogatepass(bytes: &[u8]) -> Option<String> {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Some(text.to_owned());
    }
    let mut out = String::with_capacity(bytes.len());
    let mut rest = bytes;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(text) => {
                out.push_str(text);
                break;
            }
            Err(err) => {
                let good = err.valid_up_to();
                out.push_str(std::str::from_utf8(&rest[..good]).ok()?);
                rest = &rest[good..];
                // ED A0..BF xx: a surrogate, which surrogatepass lets through.
                if rest.len() >= 3
                    && rest[0] == 0xED
                    && (0xA0..=0xBF).contains(&rest[1])
                    && (0x80..=0xBF).contains(&rest[2])
                {
                    out.push('\u{fffd}');
                    rest = &rest[3..];
                } else {
                    return None;
                }
            }
        }
    }
    Some(out)
}

fn decode_utf16(bytes: &[u8], big_endian: bool) -> Option<String> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let units = bytes.chunks_exact(2).map(|pair| {
        if big_endian {
            u16::from_be_bytes([pair[0], pair[1]])
        } else {
            u16::from_le_bytes([pair[0], pair[1]])
        }
    });
    Some(
        char::decode_utf16(units)
            .map(|c| c.unwrap_or('\u{fffd}'))
            .collect(),
    )
}

fn decode_utf32(bytes: &[u8], big_endian: bool) -> Option<String> {
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = String::with_capacity(bytes.len() / 4);
    for quad in bytes.chunks_exact(4) {
        let cp = if big_endian {
            u32::from_be_bytes([quad[0], quad[1], quad[2], quad[3]])
        } else {
            u32::from_le_bytes([quad[0], quad[1], quad[2], quad[3]])
        };
        if (0xD800..=0xDFFF).contains(&cp) {
            out.push('\u{fffd}');
        } else {
            out.push(char::from_u32(cp)?);
        }
    }
    Some(out)
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.pos += 1;
        }
    }

    fn starts_with(&self, word: &str) -> bool {
        let mut rest = self.chars[self.pos.min(self.chars.len())..].iter();
        word.chars().all(|c| rest.next() == Some(&c))
    }

    fn value(&mut self, depth: usize) -> std::result::Result<Value, ReadError> {
        if depth >= MAX_DEPTH {
            return Err(ReadError::Syntax);
        }
        match self.peek() {
            Some('"') => {
                self.pos += 1;
                Ok(Value::Str(self.string()?))
            }
            Some('{') => {
                self.pos += 1;
                self.object(depth)
            }
            Some('[') => {
                self.pos += 1;
                self.array(depth)
            }
            Some('n') if self.starts_with("null") => {
                self.pos += 4;
                Ok(Value::Null)
            }
            Some('t') if self.starts_with("true") => {
                self.pos += 4;
                Ok(Value::Bool(true))
            }
            Some('f') if self.starts_with("false") => {
                self.pos += 5;
                Ok(Value::Bool(false))
            }
            Some('N') if self.starts_with("NaN") => Err(ReadError::Constant),
            Some('I') if self.starts_with("Infinity") => Err(ReadError::Constant),
            Some('-') if self.starts_with("-Infinity") => Err(ReadError::Constant),
            Some('-' | '0'..='9') => self.number(),
            _ => Err(ReadError::Syntax),
        }
    }

    fn object(&mut self, depth: usize) -> std::result::Result<Value, ReadError> {
        let mut pairs: Vec<(String, Value)> = Vec::new();
        self.skip_ws();
        if self.peek() == Some('}') {
            self.pos += 1;
            return Ok(Value::Dict(Dict(pairs)));
        }
        loop {
            if self.peek() != Some('"') {
                return Err(ReadError::Syntax);
            }
            self.pos += 1;
            let key = self.string()?;
            self.skip_ws();
            if self.peek() != Some(':') {
                return Err(ReadError::Syntax);
            }
            self.pos += 1;
            self.skip_ws();
            let value = self.value(depth + 1)?;
            pairs.push((key, value));
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.pos += 1;
                    self.skip_ws();
                }
                Some('}') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err(ReadError::Syntax),
            }
        }
        // CrowdWatch's object_pairs_hook runs when the object is complete.
        let mut dict = Dict::new();
        for (key, value) in pairs {
            if dict.contains_key(&key) {
                return Err(ReadError::DuplicateKey(key));
            }
            dict.0.push((key, value));
        }
        Ok(Value::Dict(dict))
    }

    fn array(&mut self, depth: usize) -> std::result::Result<Value, ReadError> {
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(']') {
            self.pos += 1;
            return Ok(Value::List(items));
        }
        loop {
            items.push(self.value(depth + 1)?);
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.pos += 1;
                    self.skip_ws();
                }
                Some(']') => {
                    self.pos += 1;
                    return Ok(Value::List(items));
                }
                _ => return Err(ReadError::Syntax),
            }
        }
    }

    fn hex4(&mut self) -> std::result::Result<u32, ReadError> {
        let mut value = 0;
        for _ in 0..4 {
            let digit = self
                .peek()
                .and_then(|c| c.to_digit(16))
                .ok_or(ReadError::Syntax)?;
            value = value * 16 + digit;
            self.pos += 1;
        }
        Ok(value)
    }

    fn string(&mut self) -> std::result::Result<String, ReadError> {
        let mut out = String::new();
        loop {
            let c = self.peek().ok_or(ReadError::Syntax)?;
            self.pos += 1;
            match c {
                '"' => return Ok(out),
                '\\' => {
                    let escape = self.peek().ok_or(ReadError::Syntax)?;
                    self.pos += 1;
                    match escape {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => {
                            let first = self.hex4()?;
                            if (0xD800..=0xDBFF).contains(&first) && self.starts_with("\\u") {
                                let save = self.pos;
                                self.pos += 2;
                                let second = self.hex4()?;
                                if (0xDC00..=0xDFFF).contains(&second) {
                                    let cp = 0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00);
                                    out.push(char::from_u32(cp).ok_or(ReadError::Syntax)?);
                                    continue;
                                }
                                self.pos = save;
                            }
                            out.push(char::from_u32(first).unwrap_or('\u{fffd}'));
                        }
                        _ => return Err(ReadError::Syntax),
                    }
                }
                c if (c as u32) < 0x20 => return Err(ReadError::Syntax),
                c => out.push(c),
            }
        }
    }

    fn number(&mut self) -> std::result::Result<Value, ReadError> {
        let start = self.pos;
        if self.peek() == Some('-') {
            self.pos += 1;
        }
        match self.peek() {
            Some('0') => self.pos += 1,
            Some('1'..='9') => {
                while matches!(self.peek(), Some('0'..='9')) {
                    self.pos += 1;
                }
            }
            _ => return Err(ReadError::Syntax),
        }
        let mut is_float = false;
        if self.peek() == Some('.') && matches!(self.chars.get(self.pos + 1), Some('0'..='9')) {
            is_float = true;
            self.pos += 1;
            while matches!(self.peek(), Some('0'..='9')) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            let save = self.pos;
            self.pos += 1;
            if matches!(self.peek(), Some('+' | '-')) {
                self.pos += 1;
            }
            if matches!(self.peek(), Some('0'..='9')) {
                is_float = true;
                while matches!(self.peek(), Some('0'..='9')) {
                    self.pos += 1;
                }
            } else {
                self.pos = save;
            }
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        if is_float {
            // Python's float() of the literal: correctly rounded, overflow to infinity.
            return text
                .parse::<f64>()
                .map(Value::Float)
                .map_err(|_| ReadError::Syntax);
        }
        let negative = text.starts_with('-');
        let digits = text.trim_start_matches('-');
        if digits.len() > 4300 {
            return Err(ReadError::TooManyDigits(digits.len().to_string()));
        }
        let canonical = if digits == "0" {
            "0".to_owned()
        } else if negative {
            format!("-{digits}")
        } else {
            digits.to_owned()
        };
        Ok(int_value(canonical))
    }
}

// ---------------------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------------------

/// Python's `json.dumps(value, ensure_ascii=False, allow_nan=False, sort_keys=True, ...)`,
/// compact with `(",", ":")` separators or indented by two spaces. NaN and infinities are
/// refused, as `allow_nan=False` refuses them.
pub fn dumps(value: &Value, pretty: bool) -> Result<String> {
    let mut out = String::new();
    write_value(&mut out, value, pretty, 0)?;
    Ok(out)
}

fn write_value(out: &mut String, value: &Value, pretty: bool, depth: usize) -> Result<()> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Value::BigInt(s) => out.push_str(s),
        Value::Float(f) => {
            if !f.is_finite() {
                return Err(Fail::value_error(format!(
                    "Out of range float values are not JSON compliant: {}",
                    float_repr(*f)
                )));
            }
            out.push_str(&float_repr(*f));
        }
        Value::Str(s) => write_string(out, s),
        Value::List(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return Ok(());
            }
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                newline(out, pretty, depth + 1);
                write_value(out, item, pretty, depth + 1)?;
            }
            newline(out, pretty, depth);
            out.push(']');
        }
        Value::Dict(d) => {
            if d.is_empty() {
                out.push_str("{}");
                return Ok(());
            }
            let mut entries: Vec<(&String, &Value)> = d.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            out.push('{');
            for (index, (key, item)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                newline(out, pretty, depth + 1);
                write_string(out, key);
                out.push_str(if pretty { ": " } else { ":" });
                write_value(out, item, pretty, depth + 1)?;
            }
            newline(out, pretty, depth);
            out.push('}');
        }
    }
    Ok(())
}

fn newline(out: &mut String, pretty: bool, depth: usize) {
    if pretty {
        out.push('\n');
        for _ in 0..depth {
            out.push_str("  ");
        }
    }
}

/// A JSON string as Python writes it with `ensure_ascii=False`.
fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Value {
        parse_str(text).unwrap()
    }

    #[test]
    fn reads_like_python() {
        assert!(matches!(read("20"), Value::Int(20)));
        assert!(matches!(read("20.0"), Value::Float(f) if f == 20.0));
        assert!(matches!(read("-0"), Value::Int(0)));
        assert!(matches!(read("1e400"), Value::Float(f) if f.is_infinite()));
        assert!(
            matches!(read("12345678901234567890"), Value::BigInt(ref s) if s == "12345678901234567890")
        );
        assert_eq!(read(r#""😀""#).as_str(), Some("😀"));
        assert_eq!(read(r#""\ud800x""#).as_str(), Some("\u{fffd}x"));
        for bad in [
            "",
            "01",
            "[1,]",
            "{\"a\" 1}",
            "\"\u{1}\"",
            "tru",
            "1.",
            ".5",
            "\u{feff}1",
            "[] x",
        ] {
            assert_eq!(parse_str(bad).unwrap_err(), ReadError::Syntax, "{bad:?}");
        }
        assert_eq!(parse_str("[1, NaN]").unwrap_err(), ReadError::Constant);
        assert_eq!(parse_str("-Infinity").unwrap_err(), ReadError::Constant);
        assert_eq!(
            parse_str(r#"{"a":1,"a":2}"#).unwrap_err(),
            ReadError::DuplicateKey("a".into())
        );
        assert_eq!(
            parse_str(&"[".repeat(MAX_DEPTH + 1)).unwrap_err(),
            ReadError::Syntax
        );
    }

    #[test]
    fn reads_python_byte_encodings() {
        let utf16: Vec<u8> = "[1]".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert!(matches!(parse_bytes(&utf16).unwrap(), Value::List(_)));
        assert!(matches!(
            parse_bytes(b"\xef\xbb\xbf{}").unwrap(),
            Value::Dict(_)
        ));
        assert!(parse_bytes(b"\xff").is_err());
    }

    #[test]
    fn writes_like_python() {
        let value = read(r#"{"b":[1,2.5,{"z":null}],"a":"é\n\u0001","c":{},"d":[]}"#);
        assert_eq!(
            dumps(&value, false).unwrap(),
            r#"{"a":"é\n\u0001","b":[1,2.5,{"z":null}],"c":{},"d":[]}"#
        );
        assert_eq!(
            dumps(&value, true).unwrap(),
            "{\n  \"a\": \"é\\n\\u0001\",\n  \"b\": [\n    1,\n    2.5,\n    {\n      \"z\": null\n    }\n  ],\n  \"c\": {},\n  \"d\": []\n}"
        );
        assert!(dumps(&Value::Float(f64::NAN), false).is_err());
    }

    #[test]
    fn behaves_like_python_objects() {
        let row = read(r#"{"a":[1,"x"],"s":"text","n":null}"#);
        assert_eq!(row.index("missing").unwrap_err().to_string(), "'missing'");
        assert_eq!(
            row.index("a").unwrap().index("x").unwrap_err().kind(),
            "TypeError"
        );
        assert_eq!(
            row.index("n").unwrap().get("x").unwrap_err().kind(),
            "AttributeError"
        );
        assert!(row
            .index("s")
            .unwrap()
            .contains(&Value::from("ex"))
            .unwrap());
        assert!(row
            .index("a")
            .unwrap()
            .contains(&Value::Float(1.0))
            .unwrap());
        assert_eq!(
            read("[1, 'x']".replace('\'', "\"").as_str()).py_str(),
            "[1, 'x']"
        );
        assert_eq!(Value::from("3.5").py_float().unwrap(), 3.5);
        assert!(matches!(
            Value::Float(-2.7).py_int().unwrap(),
            Value::Int(-2)
        ));
        assert_eq!(Value::Null.py_float().unwrap_err().kind(), "TypeError");
    }
}
