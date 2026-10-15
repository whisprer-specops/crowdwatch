// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! The small part of Python's ElementTree that the RSS/Atom collector used.
//!
//! A strict, non-validating XML reader: namespaces are expanded to `{uri}name` as
//! ElementTree does, character and predefined entity references are decoded, and any
//! document expat would refuse is refused (CrowdWatch rejects DTDs before parsing, so
//! no other entities exist). An element keeps its text up to the first child, as
//! ElementTree's `element.text` does.

use std::collections::HashMap;

use crate::error::{Fail, Result};
use crate::xml_encodings::{Codec, ALIASES, MODULES, TABLES};

const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

/// An element: its expanded tag, attributes, leading text and children (as indexes into
/// the document, so no tree depth can exhaust the stack).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Element {
    pub tag: String,
    pub attrib: Vec<(String, String)>,
    pub text: Option<String>,
    pub children: Vec<usize>,
}

/// A parsed document; element 0 is the root.
#[derive(Clone, Debug, Default)]
pub struct Document {
    pub elements: Vec<Element>,
}

impl Element {
    /// `element.get(name)`.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.attrib
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

impl Document {
    pub fn root(&self) -> &Element {
        &self.elements[0]
    }

    pub fn element(&self, index: usize) -> &Element {
        &self.elements[index]
    }

    /// `element.find(tag)`: the first child with this tag.
    pub fn find(&self, index: usize, tag: &str) -> Option<&Element> {
        self.elements[index]
            .children
            .iter()
            .map(|&child| &self.elements[child])
            .find(|child| child.tag == tag)
    }

    /// `element.findall(".//tag")`: every descendant with this tag, in document order.
    pub fn findall_descendants(&self, index: usize, tag: &str) -> Vec<usize> {
        let mut found = Vec::new();
        let mut stack: Vec<usize> = self.elements[index]
            .children
            .iter()
            .rev()
            .copied()
            .collect();
        while let Some(current) = stack.pop() {
            if self.elements[current].tag == tag {
                found.push(current);
            }
            stack.extend(self.elements[current].children.iter().rev().copied());
        }
        found
    }
}

fn parse_error(message: &str) -> Fail {
    Fail::py("ParseError", message.to_owned())
}

/// Python's `xml.etree.ElementTree.fromstring(data)` for bytes.
pub fn fromstring(data: &[u8]) -> Result<Document> {
    let text = decode(data)?;
    let mut parser = Parser {
        chars: normalise_newlines(&text),
        pos: 0,
        bindings: HashMap::from([("xml".to_owned(), vec![XML_NS.to_owned()])]),
        scopes: Vec::new(),
    };
    parser.document()
}

/// Bytes to text, choosing the encoding as expat does: a byte-order mark, or a zero byte
/// among the first two, means UTF-16; otherwise the text is UTF-8 unless the XML
/// declaration names another encoding. Python's codecs supply the encodings expat does
/// not know itself, with the same refusals (and exception names) as 0.1.0.
fn decode(data: &[u8]) -> Result<String> {
    let (two_byte, big_endian, body) = match data {
        [0xEF, 0xBB, 0xBF, rest @ ..] => (false, false, rest),
        [0xFE, 0xFF, rest @ ..] => (true, true, rest),
        [0xFF, 0xFE, rest @ ..] => (true, false, rest),
        [0, _, ..] => (true, true, data),
        [_, 0, ..] => (true, false, data),
        _ => (false, false, data),
    };
    if two_byte {
        let text = utf16(body, big_endian)?;
        let chars: Vec<char> = text.chars().collect();
        if let Some(name) = declared_encoding(&chars)? {
            match native(&name) {
                Some(Native::Utf16) => {}
                Some(Native::Utf16Le) if !big_endian => {}
                Some(Native::Utf16Be) if big_endian => {}
                Some(_) => return Err(incorrect_encoding()),
                None => match python_codec(&name) {
                    // A single-byte table cannot read the rest of a UTF-16 document.
                    Codec::Table(_) | Codec::Rejected => return Err(incorrect_encoding()),
                    other => return Err(codec_failure(other, &name)),
                },
            }
        }
        return Ok(text);
    }
    // The declaration is ASCII, so its bytes can be read one character each.
    let head_len = if body.starts_with(b"<?xml") {
        body.windows(2)
            .position(|pair| pair == b"?>")
            .map_or(body.len(), |at| at + 2)
    } else {
        0
    };
    let head: Vec<char> = body[..head_len].iter().map(|&b| char::from(b)).collect();
    let Some(name) = declared_encoding(&head)? else {
        return utf8(body);
    };
    match native(&name) {
        Some(Native::Utf8) => utf8(body),
        Some(Native::Latin1) => Ok(body.iter().map(|&b| char::from(b)).collect()),
        Some(Native::Ascii) => {
            if body.is_ascii() {
                utf8(body)
            } else {
                Err(invalid_token())
            }
        }
        Some(_) => Err(incorrect_encoding()),
        None => match python_codec(&name) {
            Codec::Table(index) => {
                let table = &TABLES[index];
                let high: Vec<char> = table.high.chars().collect();
                body.iter()
                    .map(|&b| {
                        let c = if b < 0x80 {
                            if table.low_invalid.contains(&b) {
                                return Err(invalid_token());
                            }
                            char::from(b)
                        } else {
                            high[usize::from(b - 0x80)]
                        };
                        if c == '\u{fffd}' {
                            Err(invalid_token())
                        } else {
                            Ok(c)
                        }
                    })
                    .collect()
            }
            other => Err(codec_failure(other, &name)),
        },
    }
}

fn invalid_token() -> Fail {
    parse_error("not well-formed (invalid token)")
}

fn incorrect_encoding() -> Fail {
    parse_error("encoding specified in XML declaration is incorrect")
}

/// The error for a codec Python could not turn into a single-byte table.
fn codec_failure(codec: Codec, name: &str) -> Fail {
    match codec {
        Codec::MultiByte => Fail::value_error("multi-byte encodings are not supported"),
        Codec::Lookup => Fail::py("LookupError", format!("unknown encoding: {name}")),
        Codec::Undefined => Fail::py("UnicodeError", "undefined encoding"),
        Codec::Table(_) | Codec::Rejected => parse_error("unknown encoding"),
    }
}

/// The encodings expat reads without help.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Native {
    Utf8,
    Latin1,
    Ascii,
    Utf16,
    Utf16Le,
    Utf16Be,
}

fn native(name: &str) -> Option<Native> {
    Some(match name.to_ascii_uppercase().as_str() {
        "UTF-8" => Native::Utf8,
        "ISO-8859-1" => Native::Latin1,
        "US-ASCII" => Native::Ascii,
        "UTF-16" => Native::Utf16,
        "UTF-16LE" => Native::Utf16Le,
        "UTF-16BE" => Native::Utf16Be,
        _ => return None,
    })
}

/// Python's codec lookup for a name expat passed on: lower case, runs of punctuation as one
/// underscore, then the alias table, then the module of that name.
fn python_codec(name: &str) -> Codec {
    let mut normal = String::with_capacity(name.len());
    let mut punct = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '.' {
            if punct && !normal.is_empty() {
                normal.push('_');
            }
            punct = false;
            normal.push(c.to_ascii_lowercase());
        } else {
            punct = true;
        }
    }
    let alias = |key: &str| {
        ALIASES
            .binary_search_by(|(k, _)| (*k).cmp(key))
            .ok()
            .map(|i| ALIASES[i].1)
    };
    let aliased = alias(&normal).or_else(|| alias(&normal.replace('.', "_")));
    for module in aliased.into_iter().chain([normal.as_str()]) {
        if module.is_empty() || module.contains('.') {
            continue;
        }
        if let Ok(i) = MODULES.binary_search_by(|(m, _)| (*m).cmp(module)) {
            return MODULES[i].1;
        }
    }
    Codec::Lookup
}

fn utf8(data: &[u8]) -> Result<String> {
    String::from_utf8(data.to_vec()).map_err(|_| invalid_token())
}

fn utf16(data: &[u8], big_endian: bool) -> Result<String> {
    if !data.len().is_multiple_of(2) {
        return Err(parse_error("partial character"));
    }
    let units = data.chunks_exact(2).map(|pair| {
        if big_endian {
            u16::from_be_bytes([pair[0], pair[1]])
        } else {
            u16::from_le_bytes([pair[0], pair[1]])
        }
    });
    char::decode_utf16(units)
        .collect::<std::result::Result<String, _>>()
        .map_err(|_| invalid_token())
}

fn is_declaration_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

/// If the text opens with an XML declaration, check it as expat does and return the
/// encoding it names.
fn declared_encoding(chars: &[char]) -> Result<Option<String>> {
    let opens = chars.len() > 5
        && chars[..5].iter().copied().eq("<?xml".chars())
        && is_declaration_space(chars[5]);
    if !opens {
        return Ok(None);
    }
    let end = (5..chars.len().saturating_sub(1))
        .find(|&i| chars[i] == '?' && chars[i + 1] == '>')
        .ok_or_else(|| parse_error("unclosed token"))?;
    parse_declaration(&chars[5..end])
}

fn bad_declaration() -> Fail {
    parse_error("XML declaration not well-formed")
}

/// expat's `doParseXmlDecl` for the text between `<?xml` and `?>`: a version first, then an
/// optional encoding that starts with a letter, then an optional standalone of yes or no.
fn parse_declaration(inner: &[char]) -> Result<Option<String>> {
    let mut pos = 0;
    let (name, _) = pseudo_attribute(inner, &mut pos)?.ok_or_else(bad_declaration)?;
    if name != "version" {
        return Err(bad_declaration());
    }
    let Some(mut next) = pseudo_attribute(inner, &mut pos)? else {
        return Ok(None);
    };
    let mut encoding = None;
    if next.0 == "encoding" {
        if !next.1.starts_with(|c: char| c.is_ascii_alphabetic()) {
            return Err(bad_declaration());
        }
        encoding = Some(next.1.clone());
        match pseudo_attribute(inner, &mut pos)? {
            Some(after) => next = after,
            None => return Ok(encoding),
        }
    }
    if next.0 != "standalone" || !(next.1 == "yes" || next.1 == "no") {
        return Err(bad_declaration());
    }
    while inner.get(pos).copied().is_some_and(is_declaration_space) {
        pos += 1;
    }
    if pos != inner.len() {
        return Err(bad_declaration());
    }
    Ok(encoding)
}

/// expat's `parsePseudoAttribute`: white space, a name, `=`, and a quoted value of letters,
/// digits, `.`, `-` or `_`; `None` at the end of the declaration.
fn pseudo_attribute(inner: &[char], pos: &mut usize) -> Result<Option<(String, String)>> {
    if *pos == inner.len() {
        return Ok(None);
    }
    if !is_declaration_space(inner[*pos]) {
        return Err(bad_declaration());
    }
    while inner.get(*pos).copied().is_some_and(is_declaration_space) {
        *pos += 1;
    }
    if *pos == inner.len() {
        return Ok(None);
    }
    let name_start = *pos;
    let name_end;
    loop {
        let Some(&c) = inner.get(*pos) else {
            return Err(bad_declaration());
        };
        if c == '=' {
            name_end = *pos;
            break;
        }
        if is_declaration_space(c) {
            name_end = *pos;
            while inner.get(*pos).copied().is_some_and(is_declaration_space) {
                *pos += 1;
            }
            if inner.get(*pos) != Some(&'=') {
                return Err(bad_declaration());
            }
            break;
        }
        *pos += 1;
    }
    if name_end == name_start {
        return Err(bad_declaration());
    }
    *pos += 1;
    while inner.get(*pos).copied().is_some_and(is_declaration_space) {
        *pos += 1;
    }
    let open = match inner.get(*pos) {
        Some(&quote @ ('"' | '\'')) => quote,
        _ => return Err(bad_declaration()),
    };
    *pos += 1;
    let value_start = *pos;
    loop {
        let Some(&c) = inner.get(*pos) else {
            return Err(bad_declaration());
        };
        if c == open {
            break;
        }
        if !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')) {
            return Err(bad_declaration());
        }
        *pos += 1;
    }
    let value: String = inner[value_start..*pos].iter().collect();
    *pos += 1;
    Ok(Some((inner[name_start..name_end].iter().collect(), value)))
}

/// XML end-of-line handling: `\r\n` and lone `\r` become `\n`.
fn normalise_newlines(text: &str) -> Vec<char> {
    let mut out = Vec::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

fn is_xml_char(c: char) -> bool {
    matches!(c as u32, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)
}

fn is_name_start(c: char) -> bool {
    matches!(c, ':' | 'A'..='Z' | '_' | 'a'..='z')
        || matches!(c as u32,
            0xC0..=0xD6 | 0xD8..=0xF6 | 0xF8..=0x2FF | 0x370..=0x37D | 0x37F..=0x1FFF
            | 0x200C..=0x200D | 0x2070..=0x218F | 0x2C00..=0x2FEF | 0x3001..=0xD7FF
            | 0xF900..=0xFDCF | 0xFDF0..=0xFFFD | 0x10000..=0xEFFFF)
}

fn is_name_char(c: char) -> bool {
    is_name_start(c)
        || matches!(c, '-' | '.' | '0'..='9')
        || matches!(c as u32, 0xB7 | 0x300..=0x36F | 0x203F..=0x2040)
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    /// Each prefix's bindings, innermost last. The empty prefix is the default namespace.
    bindings: HashMap<String, Vec<String>>,
    /// The prefixes each open element declared, so they can be unbound when it closes.
    scopes: Vec<Vec<String>>,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn starts_with(&self, text: &str) -> bool {
        let mut rest = self.chars[self.pos.min(self.chars.len())..].iter();
        text.chars().all(|c| rest.next() == Some(&c))
    }

    fn skip_space(&mut self) -> bool {
        let start = self.pos;
        while matches!(self.peek(), Some(' ' | '\t' | '\n')) {
            self.pos += 1;
        }
        self.pos > start
    }

    fn name(&mut self) -> Result<String> {
        let start = self.pos;
        match self.peek() {
            Some(c) if is_name_start(c) => self.pos += 1,
            _ => return Err(parse_error("not well-formed (invalid token)")),
        }
        while self.peek().is_some_and(is_name_char) {
            self.pos += 1;
        }
        Ok(self.chars[start..self.pos].iter().collect())
    }

    fn document(&mut self) -> Result<Document> {
        if self.starts_with("<?xml") && self.chars.get(5).copied().is_some_and(is_declaration_space)
        {
            self.declaration()?;
        }
        self.misc()?;
        if self.peek() != Some('<') {
            return Err(parse_error(if self.peek().is_none() {
                "no element found"
            } else {
                "syntax error"
            }));
        }
        let document = self.elements()?;
        self.misc()?;
        if self.peek().is_some() {
            return Err(parse_error("junk after document element"));
        }
        Ok(document)
    }

    /// Skip the XML declaration (already checked by `decode`).
    fn declaration(&mut self) -> Result<()> {
        let end = (self.pos..self.chars.len().saturating_sub(1))
            .find(|&i| self.chars[i] == '?' && self.chars[i + 1] == '>')
            .ok_or_else(|| parse_error("unclosed token"))?;
        parse_declaration(&self.chars[self.pos + 5..end])?;
        self.pos = end + 2;
        Ok(())
    }

    /// Comments, processing instructions and white space outside the root.
    fn misc(&mut self) -> Result<()> {
        loop {
            self.skip_space();
            if self.starts_with("<!--") {
                self.comment()?;
            } else if self.starts_with("<?") {
                self.processing_instruction()?;
            } else {
                return Ok(());
            }
        }
    }

    fn comment(&mut self) -> Result<()> {
        self.pos += 4;
        loop {
            match self.peek() {
                None => return Err(parse_error("unclosed token")),
                Some('-') if self.starts_with("--") => {
                    if self.starts_with("-->") {
                        self.pos += 3;
                        return Ok(());
                    }
                    return Err(parse_error("not well-formed (invalid token)"));
                }
                Some(c) if !is_xml_char(c) => {
                    return Err(parse_error("not well-formed (invalid token)"))
                }
                Some(_) => self.pos += 1,
            }
        }
    }

    fn processing_instruction(&mut self) -> Result<()> {
        self.pos += 2;
        let target = self.name()?;
        if target.eq_ignore_ascii_case("xml") {
            return Err(parse_error(
                "XML or text declaration not at start of entity",
            ));
        }
        if target.contains(':') {
            return Err(parse_error("not well-formed (invalid token)"));
        }
        if !self.starts_with("?>") && !self.skip_space() {
            return Err(parse_error("not well-formed (invalid token)"));
        }
        loop {
            match self.peek() {
                None => return Err(parse_error("unclosed token")),
                Some('?') if self.starts_with("?>") => {
                    self.pos += 2;
                    return Ok(());
                }
                Some(c) if !is_xml_char(c) => {
                    return Err(parse_error("not well-formed (invalid token)"))
                }
                Some(_) => self.pos += 1,
            }
        }
    }

    fn resolve_prefix(&self, prefix: &str) -> Option<&str> {
        self.bindings
            .get(prefix)
            .and_then(|uris| uris.last())
            .map(String::as_str)
    }

    /// Bind the prefixes an element declared, for its own scope.
    fn push_scope(&mut self, scope: Vec<(String, String)>) {
        let mut prefixes = Vec::with_capacity(scope.len());
        for (prefix, uri) in scope {
            self.bindings.entry(prefix.clone()).or_default().push(uri);
            prefixes.push(prefix);
        }
        self.scopes.push(prefixes);
    }

    /// Unbind what the closing element declared.
    fn pop_scope(&mut self) {
        for prefix in self.scopes.pop().unwrap_or_default() {
            if let Some(uris) = self.bindings.get_mut(&prefix) {
                uris.pop();
            }
        }
    }

    /// `prefix:local` to ElementTree's `{uri}local`.
    fn expand(&self, name: &str, is_element: bool) -> Result<String> {
        let mut parts = name.splitn(3, ':');
        let first = parts.next().unwrap_or_default();
        match (parts.next(), parts.next()) {
            (None, _) => {
                if is_element {
                    match self.resolve_prefix("") {
                        Some(uri) if !uri.is_empty() => Ok(format!("{{{uri}}}{first}")),
                        _ => Ok(first.to_owned()),
                    }
                } else {
                    Ok(first.to_owned())
                }
            }
            (Some(local), None) if !first.is_empty() && !local.is_empty() => {
                let uri = self
                    .resolve_prefix(first)
                    .ok_or_else(|| parse_error("unbound prefix"))?;
                Ok(format!("{{{uri}}}{local}"))
            }
            _ => Err(parse_error("not well-formed (invalid token)")),
        }
    }

    /// A start tag (the `<` already seen): the element, its raw name, and whether it
    /// closed itself. Its namespace scope is pushed.
    fn start_tag(&mut self) -> Result<(Element, String, bool)> {
        self.pos += 1;
        let raw_name = self.name()?;
        let mut raw_attributes: Vec<(String, String)> = Vec::new();
        let empty;
        loop {
            let spaced = self.skip_space();
            match self.peek() {
                Some('>') => {
                    self.pos += 1;
                    empty = false;
                    break;
                }
                Some('/') if self.starts_with("/>") => {
                    self.pos += 2;
                    empty = true;
                    break;
                }
                Some(_) if spaced => {
                    let name = self.name()?;
                    self.skip_space();
                    if self.peek() != Some('=') {
                        return Err(parse_error("not well-formed (invalid token)"));
                    }
                    self.pos += 1;
                    self.skip_space();
                    let value = self.attribute_value()?;
                    if raw_attributes.iter().any(|(n, _)| *n == name) {
                        return Err(parse_error("duplicate attribute"));
                    }
                    raw_attributes.push((name, value));
                }
                None => return Err(parse_error("unclosed token")),
                Some(_) => return Err(parse_error("not well-formed (invalid token)")),
            }
        }
        let mut scope = Vec::new();
        let mut attributes = Vec::new();
        for (name, value) in raw_attributes {
            if name == "xmlns" {
                scope.push((String::new(), value));
            } else if let Some(prefix) = name.strip_prefix("xmlns:") {
                if value.is_empty() || prefix == "xmlns" || (prefix == "xml" && value != XML_NS) {
                    return Err(parse_error("reserved prefix (xml) must not be undeclared or bound to another namespace name"));
                }
                scope.push((prefix.to_owned(), value));
            } else {
                attributes.push((name, value));
            }
        }
        self.push_scope(scope);
        let tag = self.expand(&raw_name, true)?;
        let mut expanded: Vec<(String, String)> = Vec::with_capacity(attributes.len());
        for (name, value) in attributes {
            let key = self.expand(&name, false)?;
            if expanded.iter().any(|(k, _)| *k == key) {
                return Err(parse_error("duplicate attribute"));
            }
            expanded.push((key, value));
        }
        Ok((
            Element {
                tag,
                attrib: expanded,
                text: None,
                children: Vec::new(),
            },
            raw_name,
            empty,
        ))
    }

    /// The root element and everything in it, without recursion.
    fn elements(&mut self) -> Result<Document> {
        struct Open {
            index: usize,
            raw_name: String,
            text: String,
            saw_text: bool,
        }
        let mut document = Document::default();
        let mut open: Vec<Open> = Vec::new();
        let (root, raw_name, empty) = self.start_tag()?;
        document.elements.push(root);
        if empty {
            self.pop_scope();
            return Ok(document);
        }
        open.push(Open {
            index: 0,
            raw_name,
            text: String::new(),
            saw_text: false,
        });
        while let Some(current) = open.last_mut() {
            let no_children = document.elements[current.index].children.is_empty();
            match self.peek() {
                None => return Err(parse_error("no element found")),
                Some('<') if self.starts_with("</") => {
                    self.pos += 2;
                    let name = self.name()?;
                    self.skip_space();
                    if self.peek() != Some('>') {
                        return Err(parse_error("not well-formed (invalid token)"));
                    }
                    self.pos += 1;
                    if name != current.raw_name {
                        return Err(parse_error("mismatched tag"));
                    }
                    if no_children && current.saw_text {
                        document.elements[current.index].text =
                            Some(std::mem::take(&mut current.text));
                    }
                    open.pop();
                    self.pop_scope();
                }
                Some('<') if self.starts_with("<!--") => self.comment()?,
                Some('<') if self.starts_with("<![CDATA[") => {
                    self.pos += 9;
                    loop {
                        match self.peek() {
                            None => return Err(parse_error("unclosed CDATA section")),
                            Some(']') if self.starts_with("]]>") => {
                                self.pos += 3;
                                break;
                            }
                            Some(c) if !is_xml_char(c) => {
                                return Err(parse_error("not well-formed (invalid token)"))
                            }
                            Some(c) => {
                                if no_children {
                                    current.text.push(c);
                                    current.saw_text = true;
                                }
                                self.pos += 1;
                            }
                        }
                    }
                }
                Some('<') if self.starts_with("<?") => self.processing_instruction()?,
                Some('<') if self.starts_with("<!") => return Err(parse_error("syntax error")),
                Some('<') => {
                    if no_children && current.saw_text {
                        document.elements[current.index].text =
                            Some(std::mem::take(&mut current.text));
                    }
                    let parent = current.index;
                    let (child, raw_name, empty) = self.start_tag()?;
                    let index = document.elements.len();
                    document.elements.push(child);
                    document.elements[parent].children.push(index);
                    if empty {
                        self.pop_scope();
                    } else {
                        open.push(Open {
                            index,
                            raw_name,
                            text: String::new(),
                            saw_text: false,
                        });
                    }
                }
                Some('&') => {
                    let c = self.reference()?;
                    if no_children {
                        current.text.push_str(&c);
                        current.saw_text = true;
                    }
                }
                Some(']') if self.starts_with("]]>") => {
                    return Err(parse_error("not well-formed (invalid token)"))
                }
                Some(c) if !is_xml_char(c) => {
                    return Err(parse_error("not well-formed (invalid token)"))
                }
                Some(c) => {
                    if no_children {
                        current.text.push(c);
                        current.saw_text = true;
                    }
                    self.pos += 1;
                }
            }
        }
        Ok(document)
    }

    /// `&name;`, `&#N;` or `&#xH;`.
    fn reference(&mut self) -> Result<String> {
        self.pos += 1;
        if self.peek() == Some('#') {
            self.pos += 1;
            let hex = self.peek() == Some('x');
            if hex {
                self.pos += 1;
            }
            let start = self.pos;
            while self.peek().is_some_and(|c| {
                if hex {
                    c.is_ascii_hexdigit()
                } else {
                    c.is_ascii_digit()
                }
            }) {
                self.pos += 1;
            }
            let digits: String = self.chars[start..self.pos].iter().collect();
            if digits.is_empty() || self.peek() != Some(';') {
                return Err(parse_error("not well-formed (invalid token)"));
            }
            self.pos += 1;
            let value = u32::from_str_radix(&digits, if hex { 16 } else { 10 })
                .ok()
                .and_then(char::from_u32)
                .filter(|c| is_xml_char(*c))
                .ok_or_else(|| parse_error("reference to invalid character number"))?;
            return Ok(value.to_string());
        }
        let name = self.name()?;
        if self.peek() != Some(';') {
            return Err(parse_error("not well-formed (invalid token)"));
        }
        self.pos += 1;
        Ok(match name.as_str() {
            "lt" => "<",
            "gt" => ">",
            "amp" => "&",
            "quot" => "\"",
            "apos" => "'",
            _ => return Err(parse_error("undefined entity")),
        }
        .to_owned())
    }

    fn attribute_value(&mut self) -> Result<String> {
        let quote = match self.peek() {
            Some(q @ ('"' | '\'')) => q,
            _ => return Err(parse_error("not well-formed (invalid token)")),
        };
        self.pos += 1;
        let mut value = String::new();
        loop {
            match self.peek() {
                None => return Err(parse_error("unclosed token")),
                Some(c) if c == quote => {
                    self.pos += 1;
                    return Ok(value);
                }
                Some('<') => return Err(parse_error("not well-formed (invalid token)")),
                Some('&') => value.push_str(&self.reference()?),
                Some('\t' | '\n') => {
                    value.push(' ');
                    self.pos += 1;
                }
                Some(c) if !is_xml_char(c) => {
                    return Err(parse_error("not well-formed (invalid token)"))
                }
                Some(c) => {
                    value.push(c);
                    self.pos += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_rss_and_atom_like_elementtree() {
        let rss = fromstring(
            br#"<?xml version="1.0" encoding="utf-8"?>
<rss xmlns:georss="http://www.georss.org/georss"><channel><!-- c --><item>
<title>Road &amp; rail <![CDATA[<closure>]]></title><georss:point>51.5 -0.12</georss:point>
<link>https://example.org/news</link></item></channel></rss>"#,
        )
        .unwrap();
        let items = rss.findall_descendants(0, "item");
        assert_eq!(items.len(), 1);
        assert_eq!(
            rss.find(items[0], "title").unwrap().text.as_deref(),
            Some("Road & rail <closure>")
        );
        assert!(rss
            .find(items[0], "{http://www.georss.org/georss}point")
            .is_some());
        let atom = fromstring(
            br#"<feed xmlns="http://www.w3.org/2005/Atom"><entry><title>A</title><link href="https://x.org/a&#10;b"/></entry></feed>"#,
        )
        .unwrap();
        let entries = atom.findall_descendants(0, "{http://www.w3.org/2005/Atom}entry");
        assert_eq!(entries.len(), 1);
        assert_eq!(
            atom.find(entries[0], "{http://www.w3.org/2005/Atom}link")
                .unwrap()
                .get("href"),
            Some("https://x.org/a\nb")
        );
        let text_then_child = fromstring(b"<a>x<b/>y</a>").unwrap();
        assert_eq!(text_then_child.root().text.as_deref(), Some("x"));
        assert_eq!(fromstring(b"<a></a>").unwrap().root().text, None);
        let deep = format!("{}{}", "<a>".repeat(200_000), "</a>".repeat(200_000));
        assert_eq!(fromstring(deep.as_bytes()).unwrap().elements.len(), 200_000);
    }

    #[test]
    fn encodings_follow_expat_and_python() {
        // Outcomes from Python's xml.etree.ElementTree.fromstring, with Python 3.14's
        // codec names (3.14 added windows-874, ISO-8859-8-I and csEUCKR, among others).
        let decl = |name: &str, body: &[u8]| {
            let mut doc = format!("<?xml version=\"1.0\" encoding=\"{name}\"?><a>").into_bytes();
            doc.extend_from_slice(body);
            doc.extend_from_slice(b"</a>");
            fromstring(&doc).map(|d| d.root().text.clone())
        };
        let kind = |r: Result<Option<String>>| r.unwrap_err().kind();
        assert_eq!(kind(decl("foo", b"")), "LookupError");
        assert_eq!(decl("utf8", b"x").unwrap().as_deref(), Some("x"));
        assert_eq!(kind(decl("utf8", "\u{e9}".as_bytes())), "ParseError");
        assert_eq!(kind(decl("shift_jis", b"")), "ValueError");
        assert_eq!(kind(decl("windows-1252", b"\x81")), "ParseError");
        assert_eq!(
            decl("windows-1252", b"\x80").unwrap().as_deref(),
            Some("\u{20ac}")
        );
        assert_eq!(kind(decl("cp864", b"%")), "ParseError");
        assert_eq!(kind(decl("hex", b"")), "LookupError");
        assert_eq!(kind(decl("undefined", b"")), "UnicodeError");
        assert_eq!(kind(decl("utf-32", b"")), "ValueError");
        assert_eq!(decl("koi8-r", b"\xc1").unwrap().as_deref(), Some("\u{430}"));
        assert_eq!(decl("KOI8_R", b"\xc1").unwrap().as_deref(), Some("\u{430}"));
        assert_eq!(kind(decl("US-ASCII", b"\xe9")), "ParseError");
        assert_eq!(decl("latin1", b"\xe9").unwrap().as_deref(), Some("\u{e9}"));
        assert_eq!(
            decl("iso-8859-1", b"\x85").unwrap().as_deref(),
            Some("\u{85}")
        );
        assert_eq!(kind(decl("cp037", b"")), "ParseError");
        assert_eq!(
            decl("windows-874", b"\xa1").unwrap().as_deref(),
            Some("\u{e01}")
        );
        assert_eq!(
            decl("ISO-8859-8-I", b"\xe0").unwrap().as_deref(),
            Some("\u{5d0}")
        );
        assert_eq!(kind(decl("csEUCKR", b"")), "ValueError");
        assert_eq!(kind(decl("mbcs", b"")), "LookupError");
        assert_eq!(kind(decl("", b"")), "ParseError");
        assert_eq!(kind(decl("1252", b"")), "ParseError");
        let bom_latin1 = b"\xef\xbb\xbf<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a>\xe9</a>";
        assert_eq!(
            fromstring(bom_latin1).unwrap().root().text.as_deref(),
            Some("\u{e9}")
        );
        let bom_utf16 = b"\xef\xbb\xbf<?xml version=\"1.0\" encoding=\"UTF-16\"?><a/>";
        assert_eq!(fromstring(bom_utf16).unwrap_err().kind(), "ParseError");
        let utf16 = |text: &str, big: bool, bom: bool| {
            let mut out = Vec::new();
            if bom {
                out.extend_from_slice(if big { b"\xfe\xff" } else { b"\xff\xfe" });
            }
            for unit in text.encode_utf16() {
                out.extend_from_slice(&if big {
                    unit.to_be_bytes()
                } else {
                    unit.to_le_bytes()
                });
            }
            out
        };
        assert!(fromstring(&utf16("<a>x</a>", false, false)).is_ok());
        assert!(fromstring(&utf16("<a>x</a>", true, false)).is_ok());
        assert!(fromstring(&utf16(
            "<?xml version=\"1.0\" encoding=\"UTF-16LE\"?><a>x</a>",
            false,
            false
        ))
        .is_ok());
        assert_eq!(
            fromstring(&utf16(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><a>x</a>",
                false,
                true
            ))
            .unwrap_err()
            .kind(),
            "ParseError"
        );
        assert_eq!(
            fromstring(&utf16(
                "<?xml version=\"1.0\" encoding=\"UTF-16BE\"?><a>x</a>",
                false,
                true
            ))
            .unwrap_err()
            .kind(),
            "ParseError"
        );
        assert!(fromstring(b"<?xml version='1.0' standalone='yes' ?><a/>").is_ok());
        assert!(fromstring(b"<?xml version='1.0' standalone='maybe'?><a/>").is_err());
        assert!(fromstring(b"<?xml encoding='UTF-8'?><a/>").is_err());
    }

    #[test]
    fn refuses_what_expat_refuses() {
        for bad in [
            &b""[..],
            b"<a>",
            b"<a></b>",
            b"<a/><b/>",
            b"<a>&nbsp;</a>",
            b"<a x='1' x='2'/>",
            b"<p:a/>",
            b"<a>\x01</a>",
            b"<a><!-- -- --></a>",
            b"<a b=\"<\"/>",
            b"<a>]]></a>",
            b"text",
            b"<a>\xff</a>",
            b" <?xml version='1.0'?><a/>",
        ] {
            let err = fromstring(bad).unwrap_err();
            assert_eq!(
                err.kind(),
                "ParseError",
                "{:?}",
                String::from_utf8_lossy(bad)
            );
        }
    }
}
