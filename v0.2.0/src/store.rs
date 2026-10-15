// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! SQLite observations are versioned: replay cannot see later corrections.
//!
//! The schema, queries and scheduling rules are 0.1.0's, statement for statement, so
//! either version can open the other's database.

use std::path::Path;

use rusqlite::types::Value as SqlValue;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::error::{Fail, Result};
use crate::json::{Dict, Value};
use crate::model::{json_text, load_json, Observation};
use crate::pyfmt::truncate_chars;
use crate::pytime::{span_of, Instant, Num, Span};

const RENAME_V1: &str = "ALTER TABLE observations RENAME TO observations_v1;";
const COPY_V1: &str =
    "INSERT INTO observations SELECT * FROM observations_v1; DROP TABLE observations_v1;";

/// The schema script, with 0.1.0's exact whitespace (SQLite keeps the text of each
/// CREATE statement).
fn schema_script(rename: &str, copy: &str) -> String {
    format!(
        "
            PRAGMA journal_mode=WAL;
            PRAGMA busy_timeout=30000;
            BEGIN IMMEDIATE;
            {rename}
            CREATE TABLE IF NOT EXISTS observations (
              observation_key TEXT NOT NULL, digest TEXT NOT NULL,
              region_id TEXT NOT NULL, source_id TEXT NOT NULL,
              observed_at TEXT NOT NULL, retrieved_at TEXT NOT NULL,
              payload TEXT NOT NULL,
              PRIMARY KEY(observation_key, retrieved_at, digest)
            );
            {copy}
            CREATE INDEX IF NOT EXISTS observation_window
              ON observations(region_id, observed_at, retrieved_at);
            CREATE TABLE IF NOT EXISTS health (
              source_id TEXT PRIMARY KEY, status TEXT NOT NULL,
              last_attempt TEXT, last_success TEXT, next_due TEXT,
              failures INTEGER NOT NULL DEFAULT 0, records INTEGER NOT NULL DEFAULT 0,
              error TEXT NOT NULL DEFAULT '', warnings TEXT NOT NULL DEFAULT '[]'
            );
            CREATE TABLE IF NOT EXISTS runs (
              id INTEGER PRIMARY KEY, source_id TEXT NOT NULL, attempted_at TEXT NOT NULL,
              status TEXT NOT NULL, records INTEGER NOT NULL, detail TEXT NOT NULL
            );
            PRAGMA user_version=2;
            COMMIT;
        "
    )
}

/// An open CrowdWatch database.
pub struct Store {
    pub db: Connection,
}

/// A SQLite value as the Python value 0.1.0 read it as.
fn sql_to_value(value: SqlValue) -> Value {
    match value {
        SqlValue::Null => Value::Null,
        SqlValue::Integer(i) => Value::Int(i),
        SqlValue::Real(f) => Value::Float(f),
        SqlValue::Text(s) => Value::Str(s),
        SqlValue::Blob(b) => Value::Str(String::from_utf8_lossy(&b).into_owned()),
    }
}

impl Store {
    /// Open (and create or upgrade) the database at `path`.
    pub fn open(path: &Path) -> Result<Store> {
        // `path.parent.mkdir(parents=True, exist_ok=True)`.
        crate::model::make_dirs(&crate::model::parent_of(path))?;
        let db = Connection::open(path)?;
        db.busy_timeout(std::time::Duration::from_secs(30))?;
        let version: i64 = db.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if !(0..=2).contains(&version) {
            drop(db);
            return Err(Fail::input(
                "database schema is newer than this CrowdWatch version",
            ));
        }
        let (rename, copy) = if version == 1 {
            (RENAME_V1, COPY_V1)
        } else {
            ("", "")
        };
        db.execute_batch(&schema_script(rename, copy))?;
        Ok(Store { db })
    }

    /// Store the observations that are new versions; return how many rows were added. The
    /// whole batch is checked first and written in one transaction.
    pub fn ingest(&mut self, observations: &[Observation]) -> Result<usize> {
        let rows = observations
            .iter()
            .map(Observation::revalidated)
            .collect::<Result<Vec<_>>>()?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut inserted = 0;
        for o in &rows {
            let key = o.key()?;
            let digest = o.digest()?;
            let previous: Option<String> = tx
                .query_row(
                    "SELECT digest FROM observations
                    WHERE observation_key=? AND retrieved_at<=?
                    ORDER BY retrieved_at DESC,rowid DESC LIMIT 1",
                    params![key, o.retrieved_at],
                    |row| row.get(0),
                )
                .optional()?;
            if previous.as_deref() == Some(digest.as_str()) {
                continue;
            }
            inserted += tx.execute(
                "INSERT OR IGNORE INTO observations VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![
                    key,
                    digest,
                    o.region_id,
                    o.source_id,
                    o.observed_at,
                    o.retrieved_at,
                    json_text(&o.to_value(), false)?
                ],
            )?;
        }
        tx.commit()?;
        Ok(inserted)
    }

    /// The latest version, retrieved by `as_of`, of every observation in the region from
    /// `days` before `as_of` up to it, oldest first.
    pub fn observations(
        &self,
        region_id: &str,
        as_of: Instant,
        days: Num,
    ) -> Result<Vec<Observation>> {
        let end = as_of.stamp();
        let start = as_of.sub(span_of(days, 86_400_000_000)?)?.stamp();
        let mut statement = self.db.prepare_cached(
            "
            SELECT payload FROM (
              SELECT payload, ROW_NUMBER() OVER (
                PARTITION BY observation_key ORDER BY retrieved_at DESC, rowid DESC) AS rn
              FROM observations WHERE region_id=? AND observed_at>=?
                AND observed_at<=? AND retrieved_at<=?
            ) WHERE rn=1 ORDER BY json_extract(payload,'$.observed_at')
        ",
        )?;
        let payloads = statement
            .query_map(params![region_id, start, end, end], |row| {
                row.get::<_, SqlValue>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        payloads
            .into_iter()
            .map(|payload| {
                let text = match sql_to_value(payload) {
                    Value::Str(s) => s,
                    other => other.py_str(),
                };
                Observation::from_value(&load_json(&text)?)
            })
            .collect()
    }

    /// Mark a source as collecting, unless it is not yet due (or, even with `force`, still
    /// being collected).
    pub fn claim(&mut self, source_id: &str, now: Instant, force: bool) -> Result<bool> {
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO health(source_id,status) VALUES (?, 'pending')",
            params![source_id],
        )?;
        let (next_due, status): (Option<String>, Option<String>) = tx.query_row(
            "SELECT next_due,status,last_attempt FROM health WHERE source_id=?",
            params![source_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let now_text = now.stamp();
        let pending_later = next_due
            .as_deref()
            .is_some_and(|due| !due.is_empty() && due > now_text.as_str());
        if (status.as_deref() == Some("collecting") && pending_later) || (!force && pending_later) {
            tx.commit()?;
            return Ok(false);
        }
        tx.execute(
            "UPDATE health SET status='collecting',last_attempt=?,next_due=? WHERE source_id=?",
            params![now_text, now.add(Span::minutes(10))?.stamp(), source_id],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Record the end of a collection attempt and schedule the next one, backing off
    /// after failures.
    #[allow(clippy::too_many_arguments)]
    pub fn finish(
        &mut self,
        source_id: &str,
        now: Instant,
        interval: Num,
        records: i64,
        error: &str,
        warnings: &[String],
        retry_after: Num,
    ) -> Result<()> {
        let row: Option<(i64, Option<String>)> = self
            .db
            .query_row(
                "SELECT failures,last_success FROM health WHERE source_id=?",
                params![source_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let failed = !error.is_empty();
        let failures = match (&row, failed) {
            (Some((count, _)), true) => count + 1,
            _ => i64::from(failed),
        };
        let delay = if failed {
            let factor = (1_i64 << failures.clamp(0, 8)).min(12);
            let backoff = match interval {
                Num::Int(i) => Num::Int(i * factor),
                Num::Float(f) => Num::Float(f * factor as f64),
            };
            // max(backoff, retry_after): the first unless the second is larger.
            if retry_after.as_f64() > backoff.as_f64() {
                retry_after
            } else {
                backoff
            }
        } else {
            interval
        };
        let status = if failed {
            "error"
        } else if warnings.is_empty() {
            "ok"
        } else {
            "partial"
        };
        let last_success = match (&row, failed) {
            (Some((_, success)), true) => success.clone(),
            (None, true) => None,
            (_, false) => Some(now.stamp()),
        };
        let warnings_json = json_text(
            &Value::List(warnings.iter().map(|w| Value::from(w.as_str())).collect()),
            false,
        )?;
        let next_due = now.add(span_of(delay, 1_000_000)?)?.stamp();
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT INTO health VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
              ON CONFLICT(source_id) DO UPDATE SET status=excluded.status,
                last_attempt=excluded.last_attempt,last_success=excluded.last_success,
                next_due=excluded.next_due,failures=excluded.failures,records=excluded.records,
                error=excluded.error,warnings=excluded.warnings",
            params![
                source_id,
                status,
                now.stamp(),
                last_success,
                next_due,
                failures,
                records,
                truncate_chars(error, 500),
                warnings_json
            ],
        )?;
        let detail = if failed {
            error.to_owned()
        } else {
            warnings_json
        };
        tx.execute(
            "INSERT INTO runs(source_id,attempted_at,status,records,detail) VALUES (?,?,?,?,?)",
            params![source_id, now.stamp(), status, records, detail],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Every source's latest collection state, by source ID.
    pub fn health(&self) -> Result<Vec<Value>> {
        let mut statement = self.db.prepare("SELECT * FROM health ORDER BY source_id")?;
        let names: Vec<String> = statement
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect();
        let rows = statement
            .query_map([], |row| {
                let mut values = Vec::with_capacity(names.len());
                for index in 0..names.len() {
                    values.push(row.get::<_, SqlValue>(index)?);
                }
                Ok(values)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut out = Vec::with_capacity(rows.len());
        for values in rows {
            let mut entry = Dict::new();
            for (name, value) in names.iter().zip(values) {
                entry.insert(name.clone(), sql_to_value(value));
            }
            let warnings = match entry.get("warnings") {
                Some(Value::Str(text)) => load_json(text)?,
                Some(other) => load_json(&other.py_str())?,
                None => Value::List(Vec::new()),
            };
            entry.insert("warnings", warnings);
            out.push(Value::Dict(entry));
        }
        Ok(out)
    }

    /// Versions stored and distinct observations.
    pub fn counts(&self) -> Result<Dict> {
        let (versions, observations): (i64, i64) = self.db.query_row(
            "SELECT COUNT(*),COUNT(DISTINCT observation_key) FROM observations",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(Dict::new()
            .with("versions", versions)
            .with("observations", observations))
    }

    /// Delete observations observed, and runs attempted, before `before`.
    pub fn prune(&mut self, before: Instant) -> Result<usize> {
        let tx = self.db.transaction()?;
        let deleted = tx.execute(
            "DELETE FROM observations WHERE observed_at<?",
            params![before.stamp()],
        )?;
        tx.execute(
            "DELETE FROM runs WHERE attempted_at<?",
            params![before.stamp()],
        )?;
        tx.commit()?;
        Ok(deleted)
    }

    /// `PRAGMA quick_check`.
    pub fn quick_check(&self) -> Result<String> {
        Ok(self
            .db
            .query_row("PRAGMA quick_check", [], |row| row.get(0))?)
    }
}
