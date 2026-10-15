// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! CSV reading with the rules 0.1.0 used: Python's `csv.DictReader` over an
//! `io.StringIO`, in the default "excel" dialect.
//!
//! A `StringIO` hands the reader lines ending at `\n` only, so a carriage return is part of
//! a line. The state machine is a port of CPython's `Modules/_csv.c`: quoted fields, doubled
//! quotes, line breaks inside quotes and stray quotes come out as they did before. Records
//! are read one at a time, so an error in a late row is met only after the earlier rows,
//! as in 0.1.0.

use crate::error::{Fail, Result};

/// Python's default `csv.field_size_limit()`.
pub const FIELD_LIMIT: usize = 131_072;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    StartRecord,
    StartField,
    InField,
    InQuotedField,
    QuoteInQuotedField,
    EatCrnl,
}

fn csv_error(message: &str) -> Fail {
    Fail::py("csv.Error", message.to_owned())
}

/// `csv.reader` over a `StringIO`: one record at a time.
pub struct Reader<'a> {
    lines: std::vec::IntoIter<&'a str>,
    state: State,
    fields: Vec<String>,
    field: String,
    field_len: usize,
    done: bool,
}

/// Lines as `StringIO(text)` yields them: each ends after `\n` and keeps it.
fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

impl<'a> Reader<'a> {
    pub fn new(text: &'a str) -> Reader<'a> {
        Reader {
            lines: lines(text).into_iter(),
            state: State::StartRecord,
            fields: Vec::new(),
            field: String::new(),
            field_len: 0,
            done: false,
        }
    }

    fn save_field(&mut self) {
        self.fields.push(std::mem::take(&mut self.field));
        self.field_len = 0;
    }

    fn add_char(&mut self, c: char) -> Result<()> {
        if self.field_len >= FIELD_LIMIT {
            return Err(csv_error(&format!(
                "field larger than field limit ({FIELD_LIMIT})"
            )));
        }
        self.field.push(c);
        self.field_len += 1;
        Ok(())
    }

    /// One character of a line, or `None` for the end of the line.
    fn process(&mut self, c: Option<char>) -> Result<()> {
        let line_break = matches!(c, Some('\n' | '\r'));
        match self.state {
            State::StartRecord => {
                match c {
                    None => return Ok(()),
                    Some(_) if line_break => {
                        self.state = State::EatCrnl;
                        return Ok(());
                    }
                    Some(_) => {}
                }
                self.state = State::StartField;
                self.start_field(c)
            }
            State::StartField => self.start_field(c),
            State::InField => match c {
                None => {
                    self.save_field();
                    self.state = State::StartRecord;
                    Ok(())
                }
                Some(_) if line_break => {
                    self.save_field();
                    self.state = State::EatCrnl;
                    Ok(())
                }
                Some(',') => {
                    self.save_field();
                    self.state = State::StartField;
                    Ok(())
                }
                Some(other) => self.add_char(other),
            },
            State::InQuotedField => match c {
                None => Ok(()),
                Some('"') => {
                    self.state = State::QuoteInQuotedField;
                    Ok(())
                }
                Some(other) => self.add_char(other),
            },
            State::QuoteInQuotedField => match c {
                Some('"') => {
                    self.state = State::InQuotedField;
                    self.add_char('"')
                }
                Some(',') => {
                    self.save_field();
                    self.state = State::StartField;
                    Ok(())
                }
                None => {
                    self.save_field();
                    self.state = State::StartRecord;
                    Ok(())
                }
                Some(_) if line_break => {
                    self.save_field();
                    self.state = State::EatCrnl;
                    Ok(())
                }
                Some(other) => {
                    // Not strict: a character after a closing quote joins the field.
                    self.state = State::InField;
                    self.add_char(other)
                }
            },
            State::EatCrnl => match c {
                Some(_) if line_break => Ok(()),
                None => {
                    self.state = State::StartRecord;
                    Ok(())
                }
                Some(_) => Err(csv_error(
                    "new-line character seen in unquoted field - do you need to open the file with newline=''?",
                )),
            },
        }
    }

    fn start_field(&mut self, c: Option<char>) -> Result<()> {
        match c {
            None => {
                self.save_field();
                self.state = State::StartRecord;
                Ok(())
            }
            Some('\n' | '\r') => {
                self.save_field();
                self.state = State::EatCrnl;
                Ok(())
            }
            Some('"') => {
                self.state = State::InQuotedField;
                Ok(())
            }
            Some(',') => {
                self.save_field();
                Ok(())
            }
            Some(other) => {
                self.state = State::InField;
                self.add_char(other)
            }
        }
    }

    /// The next record (blank lines are empty records), or `None` at the end.
    pub fn next_record(&mut self) -> Option<Result<Vec<String>>> {
        if self.done {
            return None;
        }
        self.state = State::StartRecord;
        self.fields.clear();
        self.field.clear();
        self.field_len = 0;
        loop {
            let Some(line) = self.lines.next() else {
                self.done = true;
                if self.field_len != 0 || self.state == State::InQuotedField {
                    self.save_field();
                    return Some(Ok(std::mem::take(&mut self.fields)));
                }
                return None;
            };
            for c in line.chars() {
                if let Err(err) = self.process(Some(c)) {
                    self.done = true;
                    return Some(Err(err));
                }
            }
            if let Err(err) = self.process(None) {
                self.done = true;
                return Some(Err(err));
            }
            if self.state == State::StartRecord {
                return Some(Ok(std::mem::take(&mut self.fields)));
            }
        }
    }
}

/// One `csv.DictReader` row over a header with unique names: each column's text, or
/// `None` where the row was too short (`restval`), and whether the row had more cells than
/// the header (which `restkey` would have collected under `None`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DictRow {
    pub cells: Vec<(String, Option<String>)>,
    pub too_long: bool,
}

/// `csv.DictReader`: the header first, then rows with blank lines skipped.
pub struct DictReader<'a> {
    reader: Reader<'a>,
    fieldnames: Option<Vec<String>>,
}

impl<'a> DictReader<'a> {
    /// Reads the header, as touching `reader.fieldnames` did. `None` for an empty file.
    pub fn new(text: &'a str) -> Result<DictReader<'a>> {
        let mut reader = Reader::new(text);
        let fieldnames = reader.next_record().transpose()?;
        Ok(DictReader { reader, fieldnames })
    }

    pub fn fieldnames(&self) -> Option<&[String]> {
        self.fieldnames.as_deref()
    }

    /// The next row, or `None` at the end.
    pub fn next_row(&mut self) -> Option<Result<DictRow>> {
        let header = self.fieldnames.clone().unwrap_or_default();
        loop {
            let record = match self.reader.next_record()? {
                Ok(record) => record,
                Err(err) => return Some(Err(err)),
            };
            if record.is_empty() {
                continue;
            }
            let mut cells: Vec<(String, Option<String>)> = Vec::with_capacity(header.len());
            for (index, name) in header.iter().enumerate() {
                cells.push((name.clone(), record.get(index).cloned()));
            }
            return Some(Ok(DictRow {
                too_long: record.len() > header.len(),
                cells,
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records(text: &str) -> Result<Vec<Vec<String>>> {
        let mut reader = Reader::new(text);
        let mut out = Vec::new();
        while let Some(record) = reader.next_record() {
            out.push(record?);
        }
        Ok(out)
    }

    #[test]
    fn reads_like_python_csv_over_stringio() {
        // Expected values from Python: list(csv.reader(io.StringIO(text))).
        let text = "a,\"b \"\"q\"\"\",c\r\n\r\n\"multi\nline\",x\"y\"z,\"q\"w\nlast";
        assert_eq!(
            records(text).unwrap(),
            vec![
                vec!["a".to_owned(), "b \"q\"".to_owned(), "c".to_owned()],
                vec![],
                vec![
                    "multi\nline".to_owned(),
                    "x\"y\"z".to_owned(),
                    "qw".to_owned()
                ],
                vec!["last".to_owned()],
            ]
        );
        assert_eq!(
            records("x,\"open").unwrap(),
            vec![vec!["x".to_owned(), "open".to_owned()]]
        );
        // A lone carriage return inside a line is an error, because StringIO does not
        // end lines there.
        assert_eq!(records("a\rb\n").unwrap_err().kind(), "csv.Error");
        assert_eq!(
            records("a,b\r").unwrap(),
            vec![vec!["a".to_owned(), "b".to_owned()]]
        );
        assert_eq!(
            records(",").unwrap(),
            vec![vec![String::new(), String::new()]]
        );
        assert!(records(&"x".repeat(FIELD_LIMIT + 1)).is_err());
        assert!(records(&"x".repeat(FIELD_LIMIT)).is_ok());
    }

    #[test]
    fn dict_rows_follow_dictreader() {
        let mut reader = DictReader::new("source,name\n\nA,B,C\nE\n").unwrap();
        assert_eq!(reader.fieldnames().unwrap(), ["source", "name"]);
        let first = reader.next_row().unwrap().unwrap();
        assert!(first.too_long);
        assert_eq!(first.cells[1], ("name".to_owned(), Some("B".to_owned())));
        let second = reader.next_row().unwrap().unwrap();
        assert_eq!(second.cells[1], ("name".to_owned(), None));
        assert!(reader.next_row().is_none());
        assert!(DictReader::new("").unwrap().fieldnames().is_none());
        // A late error is met only when its row is read.
        let mut late = DictReader::new("a\n1\n2\rx\n").unwrap();
        assert!(late.next_row().unwrap().is_ok());
        assert!(late.next_row().unwrap().is_err());
    }
}
