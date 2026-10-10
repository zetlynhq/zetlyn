//! The store. Browsing, faceting, filtering and full text are one query over one file.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use rusqlite::types::Value as S;
use rusqlite::{params_from_iter, Connection};
use serde_json::Value as J;

use crate::sourcedecl::{SourceDecl, PropertyType};
use crate::expr::{Lit, Op, Pred};
use crate::claim::{Attachment, Claim, Id, Origin, Value, Version};

pub struct Store {
    pub db: Connection,
}

const SCHEMA: &str = "
create table if not exists record(
  rowid       integer primary key,
  record_id   text not null unique,
  kind        text not null,
  title       text not null,
  url         text,
  text        text not null,
  known       text not null,
  valid_from  text,
  valid_to    text,
  ids         text not null,
  fields      text not null,
  origin      text not null,
  attachments text not null,
  hash        text not null,
  -- When this deployment first held it. Not `known`, which is when the publisher said it: a CVE
  -- from 2019 that reached KEV yesterday is new here and old there, and what is sold is the
  -- second kind of new.
  first_seen  text not null default '',
  first_run   integer not null,
  changed_run integer not null,
  last_run    integer not null
);
create index if not exists record_known on record(known);
create index if not exists record_changed on record(changed_run);

create table if not exists ident(
  record_id text not null, scheme text not null, value text not null);
create index if not exists ident_value on ident(scheme, value);
create index if not exists ident_folded on ident(lower(value));
create index if not exists ident_record on ident(record_id);

create table if not exists field(
  record_id  text not null,
  name       text not null,
  type       text not null,
  vocabulary text,
  s text, n real, b integer, d text);
create index if not exists field_name on field(name);
create index if not exists field_record on field(record_id);
create index if not exists field_s on field(name, s);
create index if not exists field_n on field(name, n);
create index if not exists field_d on field(name, d);

create table if not exists removed(
  record_id text primary key, run integer not null, at text not null, title text not null);

create table if not exists meta(key text primary key, value text not null);

-- Earlier versions, where the dataset declares `retention.history`. What lets a field's
-- previous value be shown beside the new one, and what `as_of` reads.
create table if not exists revision(
  record_id text not null,
  run       integer not null,
  at        text not null,
  hash      text not null,
  title     text not null,
  known     text not null,
  fields    text not null,
  primary key(record_id, run));
create index if not exists revision_at on revision(at);

create table if not exists run(
  id integer primary key,
  started text not null, finished text, complete integer not null default 0,
  added integer default 0, changed integer default 0, removed integer default 0,
  unchanged integer default 0, no_text integer default 0, no_known integer default 0,
  unparsed integer default 0, duplicates integer default 0, note text, error text,
  -- What the run saw, for the next run to hold its shape against.
  fields text, refused text);

-- What a source handed over, once per distinct excerpt: compressed, and named by its own hash, so
-- a version that did not change what the source said costs nothing more.
create table if not exists excerpt(digest text primary key, body blob not null);

create virtual table if not exists fts using fts5(record_id unindexed, title, text);
";

/// What a source could not answer, which is shown rather than swallowed.
#[derive(Debug, Default, Clone)]
pub struct Unanswered(pub Vec<String>);

pub struct Filter {
    pub sql: String,
    pub params: Vec<S>,
    pub unanswered: Unanswered,
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub record_id: String,
    pub rank: usize,
    pub title: String,
    pub url: Option<String>,
    pub kind: String,
    pub known: String,
    pub ids: Vec<Id>,
    pub fields: BTreeMap<String, Value>,
    pub why_text: Vec<String>,
    pub why_field: Vec<String>,
    pub why_id: Option<Id>,
    pub snippet: String,
}

#[derive(Debug, Clone)]
pub struct FieldSummary {
    pub name: String,
    pub kind: String,
    pub vocabulary: Option<String>,
    pub records: u64,
    pub values: Vec<(String, u64)>,
    pub min: Option<String>,
    pub max: Option<String>,
}

/// A source is a directory somebody moves, so a store written by an earlier build has to
/// open under a later one. Each of these is a column that arrived after the first release;
/// SQLite refuses a duplicate and that refusal is the whole of the check.
fn migrate(db: &Connection) {
    // Existing full-text rows carry arbitrary rowids. Rebuilt once, aligned to the claim
    // they belong to, so a replacement is a lookup ever after.
    let aligned: Option<String> = db
        .query_row("select value from meta where key = 'fts_rowid'", [], |r| {
            r.get(0)
        })
        .ok();
    if aligned.is_none() {
        let _ = db.execute_batch(
            "drop table if exists fts;
             create virtual table fts using fts5(record_id unindexed, title, text);
             insert into fts(rowid, record_id, title, text)
               select rowid, record_id, title, text from record;
             insert or replace into meta(key, value) values('fts_rowid', '1');",
        );
    }
    // A changed claim used to be given a new rowid, and its old full-text row stayed behind
    // matching searches under a claim that no longer said that. `cve/kev` carried 3,453 index
    // rows for 1,726 claims, and a search for one word answered eleven where five was the truth.
    let swept: Option<String> = db
        .query_row(
            "select value from meta where key = 'fts_orphans'",
            [],
            |r| r.get(0),
        )
        .ok();
    if swept.is_none() {
        let _ = db.execute_batch(
            "delete from fts where rowid not in (select rowid from record);
             insert or replace into meta(key, value) values('fts_orphans', '1');",
        );
    }
    for statement in [
        "alter table run add column fields text",
        "alter table run add column refused text",
        "alter table run add column records integer",
        "alter table run add column duplicates integer default 0",
        "alter table record add column first_seen text not null default ''",
        "alter table record add column excerpt text",
        "alter table revision add column excerpt text",
        // Claims held before the column existed take the stamp of the update that first saw them.
        "update record set first_seen = coalesce((select started from run where id = first_run), '')
         where first_seen = ''",
    ] {
        let _ = db.execute_batch(statement);
    }
}

impl Store {
    pub fn open(dir: &Path) -> Result<Store, String> {
        let db = Connection::open(dir.join("claims.db")).map_err(|e| e.to_string())?;
        db.execute_batch("pragma journal_mode=wal; pragma synchronous=normal;")
            .map_err(|e| e.to_string())?;
        db.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        migrate(&db);
        Ok(Store { db })
    }

    /// The high-water mark of the last complete run. A partial run does not advance it, so
    /// the claims it failed to reach are fetched again rather than skipped forever.
    pub fn meta(&self, key: &str) -> Option<String> {
        self.db
            .query_row(
                "select value from meta where key = ?1",
                rusqlite::params![key],
                |r| r.get(0),
            )
            .ok()
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), String> {
        self.db
            .execute(
                "insert or replace into meta(key, value) values(?1, ?2)",
                rusqlite::params![key, value],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub fn last_run(&self) -> i64 {
        self.db
            .query_row("select coalesce(max(id), 0) from run", [], |r| r.get(0))
            .unwrap_or(0)
    }

    /// The last run that finished. A run's row is written when it starts and its claims only when
    /// it commits, so a tracker that took a started run as its mark looked at a source in the
    /// middle of an update, saw none of its claims, and never looked again (2026-10-06).
    pub fn last_finished_run(&self) -> i64 {
        self.db
            .query_row("select coalesce(max(id), 0) from run where finished is not null", [], |r| r.get(0))
            .unwrap_or(0)
    }

    pub fn count(&self) -> u64 {
        self.db
            .query_row("select count(*) from record", [], |r| r.get(0))
            .unwrap_or(0)
    }

    // -- writing ------------------------------------------------------------------------------

    /// A run whose shape was refused rolled its transaction back, and the row that said it
    /// had started went with it. This writes it again so the refusal has somewhere to live.
    pub fn begin_run_at(&self, id: i64, at: &str) -> Result<(), String> {
        self.db
            .execute(
                "insert or replace into run(id, started) values(?1, ?2)",
                rusqlite::params![id, at],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub fn begin_run(&self) -> Result<i64, String> {
        let id = self.last_run() + 1;
        self.db
            .execute(
                "insert into run(id, started) values(?1, ?2)",
                rusqlite::params![id, crate::iso_stamp(crate::now())],
            )
            .map_err(|e| e.to_string())?;
        Ok(id)
    }

    /// Added, changed or unchanged. A claim whose hash matches the one held is not rewritten.
    pub fn put(
        &self,
        rec: &Claim,
        run: i64,
        at: &str,
        history: bool,
    ) -> Result<&'static str, String> {
        let held: Option<String> = self
            .db
            .query_row(
                "select hash from record where record_id = ?1",
                rusqlite::params![rec.record_id],
                |r| r.get(0),
            )
            .ok();
        let excerpt = match &rec.excerpt {
            Some(e) => Some(self.keep_excerpt(e)?),
            None => None,
        };
        if held.as_deref() == Some(rec.hash.as_str()) {
            // A claim held from before receipts were kept takes the one this update saw, once.
            // The claim did not change, so what the source handed over now is what it said then.
            self.db
                .execute(
                    "update record set last_run = ?2, excerpt = coalesce(excerpt, ?3)
                     where record_id = ?1",
                    rusqlite::params![rec.record_id, run, excerpt],
                )
                .map_err(|e| e.to_string())?;
            if history {
                self.keep_current_version(&rec.record_id, excerpt.as_deref())?;
            }
            return Ok("unchanged");
        }
        let verdict = if held.is_some() { "changed" } else { "added" };
        // The row keeps the rowid it already had. `insert or replace` deletes the old row and
        // assigns a new one otherwise, and the full-text row is keyed on this rowid: a changed
        // claim would leave its old index thing behind, matching a search forever under a
        // claim that no longer says that. One generation of that doubled a store's index.
        let kept_rowid: Option<i64> = held.as_ref().and_then(|_| {
            self.db
                .query_row(
                    "select rowid from record where record_id = ?1",
                    rusqlite::params![rec.record_id],
                    |r| r.get(0),
                )
                .ok()
        });
        // Every version, where the source asked for history. Nothing is written for a
        // claim whose hash matched, so an unchanged source costs nothing.
        if history {
            self.db
                .execute(
                    "insert or replace into revision(record_id, run, at, hash, title, known, fields,
                       excerpt)
                     values(?1,?2,?3,?4,?5,?6,?7,?8)",
                    rusqlite::params![
                        rec.record_id,
                        run,
                        at,
                        rec.hash,
                        rec.title,
                        rec.known,
                        rec.fields_json().to_string(),
                        excerpt,
                    ],
                )
                .map_err(|e| e.to_string())?;
        }
        let first_seen: String = if held.is_some() {
            self.db
                .query_row(
                    "select first_seen from record where record_id = ?1",
                    rusqlite::params![rec.record_id],
                    |r| r.get(0),
                )
                .unwrap_or_else(|_| at.to_string())
        } else {
            at.to_string()
        };
        let first_run: i64 = if held.is_some() {
            self.db
                .query_row(
                    "select first_run from record where record_id = ?1",
                    rusqlite::params![rec.record_id],
                    |r| r.get(0),
                )
                .unwrap_or(run)
        } else {
            run
        };
        self.db
            .execute(
                "insert or replace into record(rowid, record_id, kind, title, url, text, known,
                   valid_from, valid_to, ids, fields, origin, attachments, hash, first_seen,
                   first_run, changed_run, last_run, excerpt)
                 values(?17,?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?16,?14,?15,?15,?18)",
                rusqlite::params![
                    rec.record_id,
                    rec.kind,
                    rec.title,
                    rec.url,
                    rec.text,
                    rec.known,
                    rec.valid.as_ref().and_then(|v| v.0.clone()),
                    rec.valid.as_ref().and_then(|v| v.1.clone()),
                    rec.ids_json().to_string(),
                    rec.fields_json().to_string(),
                    rec.from.to_json().to_string(),
                    J::Array(rec.attachments.iter().map(Attachment::to_json).collect()).to_string(),
                    rec.hash,
                    first_run,
                    run,
                    first_seen,
                    kept_rowid,
                    excerpt,
                ],
            )
            .map_err(|e| e.to_string())?;

        self.db
            .execute(
                "delete from ident where record_id = ?1",
                rusqlite::params![rec.record_id],
            )
            .map_err(|e| e.to_string())?;
        for id in &rec.ids {
            self.db
                .execute(
                    "insert into ident(record_id, scheme, value) values(?1,?2,?3)",
                    rusqlite::params![rec.record_id, id.scheme, id.value],
                )
                .map_err(|e| e.to_string())?;
        }

        self.db
            .execute(
                "delete from field where record_id = ?1",
                rusqlite::params![rec.record_id],
            )
            .map_err(|e| e.to_string())?;
        for (name, value) in &rec.fields {
            // A list is one row per element, so a comparison is satisfied when one element is.
            for v in value.flatten() {
                let (s, n, b, d) = match v {
                    Value::Number(x) => (None, Some(*x), None, None),
                    Value::Bool(x) => (None, None, Some(*x as i64), None),
                    Value::Date(x) => (None, None, None, Some(x.clone())),
                    Value::Interval { from, .. } => (None, None, None, from.clone()),
                    other => (Some(other.display()), None, None, None),
                };
                let vocab = match v {
                    Value::Code { vocabulary, .. } => vocabulary.clone(),
                    _ => None,
                };
                self.db
                    .execute(
                        "insert into field(record_id, name, type, vocabulary, s, n, b, d)
                         values(?1,?2,?3,?4,?5,?6,?7,?8)",
                        rusqlite::params![rec.record_id, name, v.type_name(), vocab, s, n, b, d],
                    )
                    .map_err(|e| e.to_string())?;
            }
        }

        // The full-text row carries the claim's own rowid, so replacing it is a lookup rather
        // than a scan. `record_id` cannot be indexed inside an FTS table, and deleting by it is
        // linear: the day an upstream change rewrites 46,000 claims, a scan per claim turns a
        // twenty-five second run into two hours.
        let rowid: i64 = self
            .db
            .query_row(
                "select rowid from record where record_id = ?1",
                rusqlite::params![rec.record_id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if held.is_some() {
            self.db
                .execute("delete from fts where rowid = ?1", rusqlite::params![rowid])
                .map_err(|e| e.to_string())?;
        }
        self.db
            .execute(
                "insert into fts(rowid, record_id, title, text) values(?1,?2,?3,?4)",
                rusqlite::params![rowid, rec.record_id, rec.title, rec.text],
            )
            .map_err(|e| e.to_string())?;
        Ok(verdict)
    }

    /// A partial run never removes a claim: a source that answered half its pages looks exactly
    /// like a source that deleted half its claims.
    pub fn sweep(&self, run: i64) -> Result<u64, String> {
        let at = crate::iso_stamp(crate::now());
        let n = self
            .db
            .execute(
                "insert or replace into removed(record_id, run, at, title)
                 select record_id, ?1, ?2, title from record where last_run < ?1",
                rusqlite::params![run, at],
            )
            .map_err(|e| e.to_string())?;
        for table in ["ident", "field", "fts"] {
            self.db
                .execute(
                    &format!(
                        "delete from {table} where record_id in
                         (select record_id from record where last_run < ?1)"
                    ),
                    rusqlite::params![run],
                )
                .map_err(|e| e.to_string())?;
        }
        self.db
            .execute(
                "delete from record where last_run < ?1",
                rusqlite::params![run],
            )
            .map_err(|e| e.to_string())?;
        Ok(n as u64)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn finish_run(
        &self,
        run: i64,
        complete: bool,
        added: u64,
        changed: u64,
        removed: u64,
        unchanged: u64,
        fields: &std::collections::BTreeSet<String>,
        notes: &crate::build::Notes,
        error: Option<&str>,
    ) -> Result<(), String> {
        self.db
            .execute(
                "update run set finished=?2, complete=?3, added=?4, changed=?5, removed=?6,
                        unchanged=?7, no_text=?8, no_known=?9, unparsed=?10, note=?11,
                        error=?12, fields=?13, records=?14, duplicates=?15
                 where id=?1",
                rusqlite::params![
                    run,
                    crate::iso_stamp(crate::now()),
                    complete as i64,
                    added as i64,
                    changed as i64,
                    removed as i64,
                    unchanged as i64,
                    notes.no_text as i64,
                    notes.no_known as i64,
                    notes.unparsed as i64,
                    notes.examples.join(" | "),
                    error,
                    fields.iter().cloned().collect::<Vec<_>>().join(","),
                    self.count() as i64,
                    notes.duplicates as i64,
                ],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Stamp a run with a time that is not now.
    ///
    /// A subscriber's run is a fetch, and stamping it with the moment of the fetch would make a
    /// year-old version look as fresh as the minute it arrived. What a reader is told about
    /// freshness has to be the age of the claims, so a subscribed run carries the time the
    /// publisher's run finished.
    pub fn set_finished(&self, run: i64, stamp: &str) -> Result<(), String> {
        self.db
            .execute(
                "update run set finished=?2 where id=?1",
                rusqlite::params![run, stamp],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    // -- reading ------------------------------------------------------------------------------

    /// A predicate becomes SQL. What it names and this source cannot answer is returned rather
    /// than dropped, because a filter applied to some claims and not others is a wrong count.
    pub fn filter(&self, pred: &Pred, types: &BTreeMap<String, PropertyType>) -> Filter {
        let mut params = Vec::new();
        let mut unanswered = Vec::new();
        let sql = self.filter_part(pred, types, &mut params, &mut unanswered);
        Filter {
            sql,
            params,
            unanswered: Unanswered(unanswered),
        }
    }

    fn filter_part(
        &self,
        pred: &Pred,
        types: &BTreeMap<String, PropertyType>,
        params: &mut Vec<S>,
        unanswered: &mut Vec<String>,
    ) -> String {
        match pred {
            Pred::And(a, b) => format!(
                "({} and {})",
                self.filter_part(a, types, params, unanswered),
                self.filter_part(b, types, params, unanswered)
            ),
            Pred::Or(a, b) => format!(
                "({} or {})",
                self.filter_part(a, types, params, unanswered),
                self.filter_part(b, types, params, unanswered)
            ),
            Pred::Cmp { left, op, right } => {
                let lit = |params: &mut Vec<S>, v: S| {
                    params.push(v);
                    format!("?{}", params.len())
                };
                match left.as_str() {
                    "known" | "title" | "kind" | "url" => {
                        let p = lit(params, S::Text(right.display()));
                        return format!("r.{left} {} {p}", op.sql());
                    }
                    "id" => {
                        let p = lit(params, S::Text(right.display().to_lowercase()));
                        // An identifier is stored as the source wrote it and compared with the
                        // case folded, so both sides fold. Comparing a folded parameter against
                        // the stored spelling is a filter that matches nothing.
                        return format!(
                            "r.record_id in (select i.record_id from ident i \
                             where lower(i.value) = {p})"
                        );
                    }
                    _ => {}
                }
                let Some(kind) = types.get(left) else {
                    unanswered.push(format!("{left}: no such property here"));
                    return "1=0".into();
                };
                if !kind.ordered() && !matches!(op, Op::Eq | Op::Ne) {
                    unanswered.push(format!(
                        "{left} {} {}: a {} has no order until a tracker declares a scale",
                        op.sql(),
                        right.display(),
                        kind.name()
                    ));
                    return "1=0".into();
                }
                let name = lit(params, S::Text(left.clone()));
                let cmp = match kind {
                    PropertyType::Number => {
                        let n = match right {
                            Lit::Num(n) => *n,
                            other => other.display().parse().unwrap_or(f64::NAN),
                        };
                        let p = lit(params, S::Real(n));
                        format!("f.n {} {p}", op.sql())
                    }
                    PropertyType::Date | PropertyType::Interval => {
                        let p = lit(params, S::Text(right.display()));
                        format!("f.d {} {p}", op.sql())
                    }
                    PropertyType::Bool => {
                        let b = matches!(right, Lit::Bool(true))
                            || right.display().eq_ignore_ascii_case("true");
                        let p = lit(params, S::Integer(b as i64));
                        format!("f.b {} {p}", op.sql())
                    }
                    _ => {
                        let p = lit(params, S::Text(right.display().to_lowercase()));
                        format!("lower(f.s) {} {p}", op.sql())
                    }
                };
                // `in`, not a correlated `exists`. An `exists` makes SQLite walk the claim table
                // and probe the field index once per row, and a count walks all of it; this way
                // the index on (name, value) picks the few matching claims first and the claim
                // table is reached by its own key.
                format!(
                    "r.record_id in (select f.record_id from field f \
                     where f.name = {name} and {cmp})"
                )
            }
        }
    }

    /// A person types `router` and means RouterOS, so every term is a prefix.
    fn fts_query(terms: &str) -> String {
        terms
            .split_whitespace()
            .map(|t| {
                let cleaned = t.replace('"', "");
                format!("\"{cleaned}\"*")
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// One query over one file: the text index, the typed fields, the paywall's bound and
    /// the order, all of it resolved before a row is read.
    #[allow(clippy::too_many_arguments)]
    pub fn search(
        &self,
        terms: &str,
        filter: Option<&Filter>,
        sort: Option<&str>,
        types: &BTreeMap<String, PropertyType>,
        seen_before: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<(u64, Vec<Hit>), String> {
        let mut params: Vec<S> = Vec::new();
        let has_text = !terms.trim().is_empty();
        if has_text {
            params.push(S::Text(Self::fts_query(terms)));
        }
        // The filter was numbered from ?1, and the text takes that slot when there is text.
        let shift = has_text as usize;
        let (mut where_sql, filter_params) = match filter {
            Some(f) => (renumber(&f.sql, shift), f.params.clone()),
            None => ("1=1".to_string(), Vec::new()),
        };
        params.extend(filter_params);
        if let Some(edge) = seen_before {
            // Free where this workspace has held it a month, or where the publisher said
            // it a month ago. A workspace that started yesterday learned everything
            // yesterday, and the second half is what keeps its free page from being empty
            // while still holding back what is new in the world.
            params.push(S::Text(edge.to_string()));
            let a = params.len();
            params.push(S::Text(edge.chars().take(10).collect::<String>()));
            let b = params.len();
            where_sql = format!("({where_sql}) and (r.first_seen <= ?{a} or r.known <= ?{b})");
        }

        let (from, order) = if has_text {
            (
                "from (select record_id, bm25(fts) as score from fts where fts match ?1) m \
                 join record r on r.record_id = m.record_id"
                    .to_string(),
                "m.score".to_string(),
            )
        } else {
            let (join, by) = order_by(sort, types, &mut params);
            (format!("from record r {join}"), by)
        };

        let total: u64 = {
            let sql = format!("select count(*) {from} where {where_sql}");
            let p = params_from_iter(params.iter());
            self.db
                .query_row(&sql, p, |r| r.get(0))
                .map_err(|e| format!("{e}\n{sql}"))?
        };

        let sql = format!(
            "select r.record_id, r.title, r.url, r.kind, r.known, r.ids, r.fields, substr(r.text, 1, 2000)
             {from} where {where_sql} order by {order} limit {limit} offset {offset}"
        );
        let mut stmt = self.db.prepare(&sql).map_err(|e| format!("{e}\n{sql}"))?;
        let want: Vec<String> = terms.split_whitespace().map(|t| t.to_lowercase()).collect();
        let rows = stmt
            .query_map(params_from_iter(params.iter()), |r| {
                let ids: String = r.get(5)?;
                let fields: String = r.get(6)?;
                Ok(Hit {
                    record_id: r.get(0)?,
                    rank: 0,
                    title: r.get(1)?,
                    url: r.get(2)?,
                    kind: r.get(3)?,
                    known: r.get(4)?,
                    ids: parse_ids(&ids),
                    fields: parse_fields(&fields),
                    why_text: Vec::new(),
                    why_field: Vec::new(),
                    why_id: None,
                    snippet: r.get(7)?,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut out = Vec::new();
        for (i, row) in rows.enumerate() {
            let mut hit = row.map_err(|e| e.to_string())?;
            hit.rank = offset + i + 1;
            let hay = format!("{} {}", hit.title, hit.snippet).to_lowercase();
            hit.why_text = want.iter().filter(|t| hay.contains(*t)).cloned().collect();
            hit.why_id = hit.ids.iter().find(|id| want.contains(&id.value)).cloned();
            out.push(hit);
        }
        Ok((total, out))
    }

    /// Whether one claim satisfies a filter, by the same SQL a search runs, so that a watch and
    /// a search cannot disagree about what a claim says.
    pub fn satisfies(&self, record_id: &str, filter: &Filter) -> bool {
        let mut params = filter.params.clone();
        params.push(S::Text(record_id.to_string()));
        let sql = format!(
            "select count(*) from record r where r.record_id = ?{} and ({})",
            params.len(),
            filter.sql
        );
        self.db
            .query_row(&sql, params_from_iter(params.iter()), |r| r.get::<_, i64>(0))
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    pub fn facet(
        &self,
        field: &str,
        filter: Option<&Filter>,
        limit: usize,
    ) -> Result<Vec<(String, u64)>, String> {
        let (where_sql, mut params) = match filter {
            Some(f) => (renumber(&f.sql, 1), f.params.clone()),
            None => ("1=1".to_string(), Vec::new()),
        };
        params.insert(0, S::Text(field.to_string()));
        let sql = format!(
            "select coalesce(g.s, g.d, cast(g.n as text), case g.b when 1 then 'true' else 'false' end)
                  as v, count(distinct g.record_id) as c
             from field g join record r on r.record_id = g.record_id
             where g.name = ?1 and {where_sql}
             group by v order by c desc, v limit {limit}"
        );
        let mut stmt = self.db.prepare(&sql).map_err(|e| format!("{e}\n{sql}"))?;
        let rows = stmt
            .query_map(params_from_iter(params.iter()), |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    r.get::<_, i64>(1)? as u64,
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// Per field: how many claims carry it, every value with its count for a code or a bool, the
    /// range for a number or a date. A faceted browse before a query has been asked.
    pub fn fields(&self, decl: &SourceDecl) -> Vec<FieldSummary> {
        let mut out = Vec::new();
        for (name, spec) in &decl.records.fields {
            let records: u64 = self
                .db
                .query_row(
                    "select count(distinct record_id) from field where name = ?1",
                    rusqlite::params![name],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap_or(0) as u64;
            let mut values = Vec::new();
            let (mut min, mut max) = (None, None);
            match spec.kind {
                PropertyType::Code | PropertyType::Bool | PropertyType::Text => {
                    values = self.facet(name, None, 24).unwrap_or_default();
                }
                PropertyType::Number => {
                    if let Ok((lo, hi)) = self.db.query_row(
                        "select min(n), max(n) from field where name = ?1",
                        rusqlite::params![name],
                        |r| Ok((r.get::<_, Option<f64>>(0)?, r.get::<_, Option<f64>>(1)?)),
                    ) {
                        min = lo.map(|v| Value::Number(v).display());
                        max = hi.map(|v| Value::Number(v).display());
                    }
                }
                PropertyType::Date | PropertyType::Interval => {
                    if let Ok((lo, hi)) = self.db.query_row(
                        "select min(d), max(d) from field where name = ?1",
                        rusqlite::params![name],
                        |r| {
                            Ok((
                                r.get::<_, Option<String>>(0)?,
                                r.get::<_, Option<String>>(1)?,
                            ))
                        },
                    ) {
                        min = lo;
                        max = hi;
                    }
                }
            }
            out.push(FieldSummary {
                name: name.clone(),
                kind: spec.kind.name().to_string(),
                vocabulary: spec.vocabulary.clone(),
                records,
                values,
                min,
                max,
            });
        }
        out
    }

    pub fn schemes(&self) -> Vec<(String, u64)> {
        let Ok(mut stmt) = self.db.prepare(
            "select scheme, count(distinct record_id) from ident group by scheme order by 2 desc",
        ) else {
            return Vec::new();
        };
        stmt.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64)))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    /// An excerpt, kept once under its own hash. JSON serialises objects in key order here, so
    /// the same excerpt is the same bytes whichever update saw it.
    pub fn keep_excerpt(&self, e: &J) -> Result<String, String> {
        let bytes = serde_json::to_vec(e).map_err(|e| e.to_string())?;
        let digest = crate::place::sha256(&bytes)[..32].to_string();
        let body = miniz_oxide::deflate::compress_to_vec(&bytes, 6);
        self.db
            .execute(
                "insert or ignore into excerpt(digest, body) values(?1, ?2)",
                rusqlite::params![digest, body],
            )
            .map_err(|e| e.to_string())?;
        Ok(digest)
    }

    /// The receipt a claim is at, as its publisher sent it. A subscriber holds the publisher's
    /// receipt, whatever it held before: the publisher is the one who saw the source.
    pub fn set_excerpt(&self, record_id: &str, e: &J) -> Result<(), String> {
        let digest = self.keep_excerpt(e)?;
        self.db
            .execute(
                "update record set excerpt = ?2 where record_id = ?1",
                rusqlite::params![record_id, digest],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn excerpt(&self, digest: &str) -> Option<J> {
        let body: Vec<u8> = self
            .db
            .query_row(
                "select body from excerpt where digest = ?1",
                rusqlite::params![digest],
                |r| r.get(0),
            )
            .ok()?;
        let bytes = miniz_oxide::inflate::decompress_to_vec(&body).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// The version a claim is at, written down where it is not. History turned on part-way
    /// through a source's life has nothing for the claims that have not changed since, and the
    /// version they are at began with the update that last changed them.
    fn keep_current_version(&self, record_id: &str, excerpt: Option<&str>) -> Result<(), String> {
        let held: Option<i64> = self
            .db
            .query_row(
                "select changed_run from record r where record_id = ?1 and not exists
                   (select 1 from revision v where v.record_id = r.record_id and v.hash = r.hash)",
                rusqlite::params![record_id],
                |r| r.get(0),
            )
            .ok();
        match held {
            Some(changed) => {
                self.db
                    .execute(
                        "insert or replace into revision(record_id, run, at, hash, title, known,
                           fields, excerpt)
                         select record_id, changed_run,
                           coalesce((select started from run where id = ?2), first_seen),
                           hash, title, known, fields, coalesce(?3, excerpt)
                         from record where record_id = ?1",
                        rusqlite::params![record_id, changed, excerpt],
                    )
                    .map_err(|e| e.to_string())?;
            }
            None => {
                // It has one: give it this receipt where it had none.
                self.db
                    .execute(
                        "update revision set excerpt = ?2 where record_id = ?1 and excerpt is null
                           and hash = (select hash from record where record_id = ?1)",
                        rusqlite::params![record_id, excerpt],
                    )
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    /// Every version of every claim, claim by claim and oldest first. A published history is
    /// written while the store is read, as the claims are.
    pub fn for_each_version(
        &self,
        mut each: impl FnMut(&str, Version) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut stmt = self
            .db
            .prepare("select distinct record_id from revision order by record_id")
            .map_err(|e| e.to_string())?;
        let ids: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .flatten()
            .collect();
        for id in ids {
            for v in self.versions(&id) {
                each(&id, v)?;
            }
        }
        Ok(())
    }

    /// Every version a claim was at, oldest first, each with its receipt.
    pub fn versions(&self, record_id: &str) -> Vec<Version> {
        let Ok(mut stmt) = self.db.prepare(
            "select run, at, hash, title, known, fields, excerpt from revision
             where record_id = ?1 order by run",
        ) else {
            return Vec::new();
        };
        let rows = stmt.query_map(rusqlite::params![record_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        });
        let Ok(rows) = rows else {
            return Vec::new();
        };
        rows.flatten()
            .map(|(update, at, hash, title, known, fields, excerpt)| Version {
                update,
                at,
                hash,
                title,
                known,
                properties: serde_json::from_str(&fields).unwrap_or(J::Null),
                excerpt: excerpt.and_then(|d| self.excerpt(&d)),
            })
            .collect()
    }

    /// The versions a publisher kept, as they arrived. They replace what this store held for the
    /// claim, because the publisher's history is the claim's history and a subscriber's own
    /// updates only ever saw what the publisher had already said.
    pub fn put_versions(
        &self,
        record_id: &str,
        versions: &[Version],
        replace: bool,
    ) -> Result<(), String> {
        if replace {
            self.db
                .execute(
                    "delete from revision where record_id = ?1",
                    rusqlite::params![record_id],
                )
                .map_err(|e| e.to_string())?;
        }
        for v in versions {
            let excerpt = match &v.excerpt {
                Some(e) => Some(self.keep_excerpt(e)?),
                None => None,
            };
            self.db
                .execute(
                    "insert or replace into revision(record_id, run, at, hash, title, known, fields,
                       excerpt)
                     values(?1,?2,?3,?4,?5,?6,?7,?8)",
                    rusqlite::params![
                        record_id,
                        v.update,
                        v.at,
                        v.hash,
                        v.title,
                        v.known,
                        v.properties.to_string(),
                        excerpt,
                    ],
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn get(&self, record_id: &str) -> Option<Claim> {
        let (mut claim, digest) = self
            .db
            .query_row(
                "select record_id, kind, title, url, text, known, valid_from, valid_to,
                        ids, fields, origin, attachments, hash, excerpt
                 from record where record_id = ?1",
                rusqlite::params![record_id],
                |r| {
                    let ids: String = r.get(8)?;
                    let fields: String = r.get(9)?;
                    let origin: String = r.get(10)?;
                    let valid_from: Option<String> = r.get(6)?;
                    let valid_to: Option<String> = r.get(7)?;
                    let o: J = serde_json::from_str(&origin).unwrap_or(J::Null);
                    let digest: Option<String> = r.get(13)?;
                    Ok((Claim {
                        record_id: r.get(0)?,
                        dataset: String::new(),
                        kind: r.get(1)?,
                        title: r.get(2)?,
                        url: r.get(3)?,
                        text: r.get(4)?,
                        known: r.get(5)?,
                        valid: if valid_from.is_some() || valid_to.is_some() {
                            Some((valid_from, valid_to))
                        } else {
                            None
                        },
                        ids: parse_ids(&ids),
                        fields: parse_fields(&fields),
                        from: Origin {
                            url: o.get("url").and_then(J::as_str).map(str::to_string),
                            file: o.get("file").and_then(J::as_str).map(str::to_string),
                            row: o.get("row").and_then(J::as_u64),
                            span: None,
                        },
                        attachments: Vec::new(),
                        hash: r.get(12)?,
                        excerpt: None,
                        versions: Vec::new(),
                    }, digest))
                },
            )
            .ok()?;
        claim.excerpt = digest.and_then(|d| self.excerpt(&d));
        Some(claim)
    }

    /// The claims a thing's key names, `isbn:9780441172719` as a tracker writes it, however this
    /// source spells the value: `978-0-441-17271-9` is the same book.
    pub fn by_key(&self, key: &str) -> Vec<String> {
        let Some((written, value)) = key.split_once(':') else {
            return Vec::new();
        };
        let scheme = written.to_lowercase();
        let scheme = scheme.as_str();
        let wanted = crate::schemes::key(scheme, value);
        let Ok(mut stmt) = self.db.prepare("select record_id, value from ident where scheme = ?1") else {
            return Vec::new();
        };
        stmt.query_map(rusqlite::params![scheme], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map(|rows| {
                rows.flatten()
                    .filter(|(_, v)| crate::schemes::key(scheme, v) == wanted)
                    .map(|(id, _)| id)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn by_identifier(&self, value: &str) -> Vec<String> {
        let Ok(mut stmt) = self
            .db
            .prepare("select record_id from ident where lower(value) = lower(?1)")
        else {
            return Vec::new();
        };
        stmt.query_map(rusqlite::params![value], |r| r.get(0))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    /// What moved and what went, as ids and titles. The field-level difference is the
    /// source's to work out, because only it knows which revisions to hold against each
    /// other.
    pub fn changes(&self, since: i64, limit: usize) -> (Vec<Moved>, Vec<Gone>) {
        let mut changed = Vec::new();
        if let Ok(mut stmt) = self.db.prepare(
            "select record_id, title, case when first_run > ?1 then 'added' else 'changed' end
             from record where changed_run > ?1 order by changed_run desc limit ?2",
        ) {
            if let Ok(rows) = stmt.query_map(rusqlite::params![since, limit as i64], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            }) {
                changed = rows.flatten().collect();
            }
        }
        let mut gone = Vec::new();
        if let Ok(mut stmt) = self.db.prepare(
            "select record_id, title from removed where run > ?1 order by run desc limit ?2",
        ) {
            if let Ok(rows) = stmt.query_map(rusqlite::params![since, limit as i64], |r| {
                Ok((r.get(0)?, r.get(1)?))
            }) {
                gone = rows.flatten().collect();
            }
        }
        (changed, gone)
    }

    pub fn run_report(&self, id: i64) -> Option<RunReport> {
        self.db
            .query_row(
                "select id, started, finished, complete, added, changed, removed, unchanged,
                        no_text, no_known, unparsed, note, error, refused, duplicates
                 from run where id = ?1",
                rusqlite::params![id],
                |r| {
                    Ok(RunReport {
                        id: r.get(0)?,
                        started: r.get(1)?,
                        finished: r.get(2)?,
                        complete: r.get::<_, i64>(3)? != 0,
                        added: r.get::<_, i64>(4)? as u64,
                        changed: r.get::<_, i64>(5)? as u64,
                        removed: r.get::<_, i64>(6)? as u64,
                        unchanged: r.get::<_, i64>(7)? as u64,
                        no_text: r.get::<_, i64>(8)? as u64,
                        no_known: r.get::<_, i64>(9)? as u64,
                        unparsed: r.get::<_, i64>(10)? as u64,
                        duplicates: r.get::<_, i64>(14).unwrap_or(0) as u64,
                        refused: r.get(13)?,
                        note: r.get(11)?,
                        error: r.get(12)?,
                    })
                },
            )
            .ok()
    }
}

#[derive(Debug, Clone)]
pub struct RunReport {
    pub id: i64,
    pub started: String,
    pub finished: Option<String>,
    pub complete: bool,
    pub added: u64,
    pub changed: u64,
    pub removed: u64,
    pub unchanged: u64,
    pub no_text: u64,
    pub no_known: u64,
    pub unparsed: u64,
    pub duplicates: u64,
    pub note: Option<String>,
    pub error: Option<String>,
    /// Why the store was not replaced, where the shape moved too far.
    pub refused: Option<String>,
}

fn order_by(
    sort: Option<&str>,
    types: &BTreeMap<String, PropertyType>,
    params: &mut Vec<S>,
) -> (String, String) {
    let Some(sort) = sort else {
        return (String::new(), "r.known desc".into());
    };
    let mut parts = sort.split_whitespace();
    let field = parts.next().unwrap_or("known");
    let dir = match parts.next().map(|d| d.to_ascii_lowercase()) {
        Some(d) if d == "asc" => "asc",
        _ => "desc",
    };
    if matches!(field, "known" | "title" | "kind") {
        return (String::new(), format!("r.{field} {dir}"));
    }
    if !types.contains_key(field) {
        return (String::new(), "r.known desc".into());
    }
    params.push(S::Text(field.to_string()));
    let n = params.len();
    (
        format!("left join field sf on sf.record_id = r.record_id and sf.name = ?{n}"),
        format!("coalesce(sf.n, sf.d, sf.s) {dir}"),
    )
}

/// The filter was built numbering from ?1. Text search takes that slot, so every reference moves.
fn renumber(sql: &str, shift: usize) -> String {
    if shift == 0 {
        return sql.to_string();
    }
    let re = regex::Regex::new(r"\?(\d+)").unwrap();
    re.replace_all(sql, |c: &regex::Captures| {
        let n: usize = c[1].parse().unwrap_or(1);
        format!("?{}", n + shift)
    })
    .into_owned()
}

pub fn parse_ids(raw: &str) -> Vec<Id> {
    serde_json::from_str::<J>(raw)
        .ok()
        .and_then(|j| j.as_array().cloned())
        .map(|a| {
            a.iter()
                .filter_map(|v| {
                    Some(Id {
                        scheme: v.get("scheme")?.as_str()?.to_string(),
                        value: v.get("value")?.as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_fields(raw: &str) -> BTreeMap<String, Value> {
    serde_json::from_str::<J>(raw)
        .ok()
        .and_then(|j| j.as_object().cloned())
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| Some((k.clone(), Value::from_json(v)?)))
                .collect()
        })
        .unwrap_or_default()
}

impl Store {
    /// What the run saw, so the next one can be held against it.
    /// The store as the last complete run left it, and the fields it carried. A run is held
    /// against the store rather than against another run: one that reads a slice adds
    /// twenty claims to twenty thousand, and comparing the two counts compares nothing.
    pub fn last_shape(&self, before: i64) -> Option<(u64, Vec<String>)> {
        self.db
            .query_row(
                "select coalesce(records, added + changed + unchanged), coalesce(fields, '')
                 from run where complete = 1 and id < ?1 order by id desc limit 1",
                rusqlite::params![before],
                |r| {
                    let total: i64 = r.get(0)?;
                    let fields: String = r.get(1)?;
                    Ok((
                        total.max(0) as u64,
                        fields
                            .split(',')
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                            .collect(),
                    ))
                },
            )
            .ok()
    }

    /// A run producing forty per cent fewer claims than the last complete one, or missing a field
    /// that every previous run carried, does not replace the store. It alerts.
    ///
    /// A schema that changed, a crawler that half broke, a feed that answered short: each of those
    /// leaves a store with less in it and nothing saying so. This is what says so.
    pub fn shape_refusal(
        &self,
        run: i64,
        records: u64,
        read: u64,
        fields: &std::collections::BTreeSet<String>,
        whole: bool,
        declared: Option<&std::collections::BTreeSet<String>>,
    ) -> Option<String> {
        let (before, had) = self.last_shape(run)?;
        if before == 0 {
            return None;
        }
        if records * 5 < before * 3 {
            return Some(format!(
                "{records} claims against {before} on the last complete update, which is more than \
                 forty per cent fewer"
            ));
        }
        // An update that read nothing carries no fields, and has nothing to say about them. Nor
        // does one that read a slice: nine claims changed since yesterday need not carry every
        // property twenty thousand do, and holding them to it refused every small update Red Hat
        // made from 2026-09-26 to 2026-09-29, each rolled back and none of them seen.
        if read == 0 || !whole {
            return None;
        }
        // A property the declaration no longer names was taken out on purpose, or renamed.
        let lost: Vec<&String> = had
            .iter()
            .filter(|f| !fields.contains(*f))
            .filter(|f| declared.is_none_or(|d| d.contains(*f)))
            .collect();
        if !lost.is_empty() {
            return Some(format!(
                "the last complete update carried {} and this one does not",
                lost.iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        None
    }

    pub fn refuse_run(&self, run: i64, why: &str, saw: u64) -> Result<(), String> {
        self.db
            .execute(
                "update run set finished = ?2, complete = 0, refused = ?3, unchanged = ?4
                 where id = ?1",
                rusqlite::params![run, crate::iso_stamp(crate::now()), why, saw as i64],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// What a claim looked like at a date, from the revisions it kept.
    pub fn as_of(&self, record_id: &str, at: &str) -> Option<(String, BTreeMap<String, Value>)> {
        self.db
            .query_row(
                "select title, fields from revision where record_id = ?1 and at <= ?2
                 order by at desc limit 1",
                rusqlite::params![record_id, at],
                |r| {
                    let title: String = r.get(0)?;
                    let fields: String = r.get(1)?;
                    Ok((title, parse_fields(&fields)))
                },
            )
            .ok()
    }

    /// The version before this one, which is what a change is shown against.
    pub fn previous(&self, record_id: &str, run: i64) -> Option<(String, BTreeMap<String, Value>)> {
        self.db
            .query_row(
                "select title, fields from revision where record_id = ?1 and run < ?2
                 order by run desc limit 1",
                rusqlite::params![record_id, run],
                |r| {
                    let title: String = r.get(0)?;
                    let fields: String = r.get(1)?;
                    Ok((title, parse_fields(&fields)))
                },
            )
            .ok()
    }

    pub fn changed_since(&self, since: i64, limit: usize) -> Vec<(String, i64, bool)> {
        let Ok(mut stmt) = self.db.prepare(
            "select record_id, changed_run, first_run > ?1 from record
             where changed_run > ?1 order by changed_run desc, record_id limit ?2",
        ) else {
            return Vec::new();
        };
        stmt.query_map(rusqlite::params![since, limit as i64], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0))
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }
}

/// A claim that was added or changed: its id, its title, and which of the two.
pub type Moved = (String, String, String);
/// A claim that was removed: its id and the title it had.
pub type Gone = (String, String);

impl Store {
    /// Every claim, in one pass, handed over one at a time. A published artifact is written
    /// while the store is read, so a source larger than memory publishes the same way a small
    /// one does.
    pub fn for_each_record(
        &self,
        mut each: impl FnMut(Claim) -> Result<(), String>,
    ) -> Result<u64, String> {
        let mut stmt = self
            .db
            .prepare(
                "select record_id, kind, title, url, text, known, valid_from, valid_to,
                        ids, fields, origin, hash, excerpt
                 from record order by record_id",
            )
            .map_err(|e| e.to_string())?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut n = 0u64;
        while let Some(r) = rows.next().map_err(|e| e.to_string())? {
            let ids: String = r.get(8).map_err(|e| e.to_string())?;
            let fields: String = r.get(9).map_err(|e| e.to_string())?;
            let origin: String = r.get(10).map_err(|e| e.to_string())?;
            let o: J = serde_json::from_str(&origin).unwrap_or(J::Null);
            let valid_from: Option<String> = r.get(6).map_err(|e| e.to_string())?;
            let valid_to: Option<String> = r.get(7).map_err(|e| e.to_string())?;
            let digest: Option<String> = r.get(12).map_err(|e| e.to_string())?;
            each(Claim {
                record_id: r.get(0).map_err(|e| e.to_string())?,
                dataset: String::new(),
                kind: r.get(1).map_err(|e| e.to_string())?,
                title: r.get(2).map_err(|e| e.to_string())?,
                url: r.get(3).map_err(|e| e.to_string())?,
                text: r.get(4).map_err(|e| e.to_string())?,
                known: r.get(5).map_err(|e| e.to_string())?,
                valid: if valid_from.is_some() || valid_to.is_some() {
                    Some((valid_from, valid_to))
                } else {
                    None
                },
                ids: parse_ids(&ids),
                fields: parse_fields(&fields),
                from: Origin {
                    url: o.get("url").and_then(J::as_str).map(str::to_string),
                    file: o.get("file").and_then(J::as_str).map(str::to_string),
                    row: o.get("row").and_then(J::as_u64),
                    span: None,
                },
                attachments: Vec::new(),
                hash: r.get(11).map_err(|e| e.to_string())?,
                excerpt: digest.and_then(|d| self.excerpt(&d)),
                versions: Vec::new(),
            })?;
            n += 1;
        }
        Ok(n)
    }

    /// Per identifier scheme, how many distinct values this store holds, folded. A tracker joins
    /// on a scheme and on the folded value, so this is the count a curator reads to see whether
    /// it can. `schemes` counts claims instead, which is the number a reader wants on a page.
    /// Every value of one scheme this source holds, as a tracker keys it. What two sources would
    /// meet on, counted before either is joined to the other.
    /// What this source says in one field, by the thing it is about: the identifier's key (so
    /// `978-0-441-17271-9` and `9780441172719` are one book) and the value as text.
    pub fn values_by_identifier(&self, scheme: &str, field: &str) -> BTreeMap<String, String> {
        let Ok(mut stmt) = self.db.prepare(
            "select i.value, coalesce(f.s, cast(f.n as text), cast(f.b as text), f.d)
             from ident i join field f on f.record_id = i.record_id
             where i.scheme = ?1 and f.name = ?2",
        ) else {
            return BTreeMap::new();
        };
        stmt.query_map([scheme, field], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))
            .map(|rows| {
                rows.flatten()
                    .filter_map(|(id, v)| Some((crate::schemes::key(scheme, &id), v?)))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn identifiers(&self, scheme: &str) -> BTreeSet<String> {
        let Ok(mut stmt) = self.db.prepare("select distinct value from ident where scheme = ?1") else {
            return BTreeSet::new();
        };
        stmt.query_map([scheme], |r| r.get::<_, String>(0))
            .map(|rows| rows.flatten().map(|v| crate::schemes::key(scheme, &v)).collect())
            .unwrap_or_default()
    }


    /// The properties whose every claim has a value and no two claims the same one: what could
    /// name a claim where no identifier was found.
    pub fn unique_fields(&self) -> Vec<String> {
        let total = self.count() as i64;
        if total == 0 {
            return Vec::new();
        }
        let Ok(mut stmt) = self.db.prepare(
            "select name from field group by name
             having count(distinct record_id) = ?1 and count(distinct coalesce(s, cast(n as text), d)) = ?1 order by name",
        ) else {
            return Vec::new();
        };
        stmt.query_map([total], |r| r.get(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
    }
    /// Fields on every claim that are different on most of them: an item a page shows twice has
    /// its id twice, and it is still what names the item.
    pub fn nearly_unique_fields(&self) -> Vec<String> {
        let total = self.count() as i64;
        if total == 0 {
            return Vec::new();
        }
        let Ok(mut stmt) = self.db.prepare(
            "select name from field group by name
             having count(distinct record_id) = ?1 and count(distinct coalesce(s, cast(n as text), d)) * 2 >= ?1 order by name",
        ) else {
            return Vec::new();
        };
        stmt.query_map([total], |r| r.get(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
    }
    pub fn distinct_identifiers(&self) -> BTreeMap<String, u64> {
        let mut out = BTreeMap::new();
        let Ok(mut stmt) = self
            .db
            .prepare("select scheme, count(distinct lower(value)) from ident group by scheme")
        else {
            return out;
        };
        let Ok(rows) = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
        }) else {
            return out;
        };
        for row in rows.flatten() {
            out.insert(row.0, row.1);
        }
        out
    }

    /// The first and last `known` in the store, which is the coverage in time.
    pub fn known_span(&self) -> (Option<String>, Option<String>) {
        self.db
            .query_row(
                "select min(known), max(known) from record where known <> ''",
                [],
                |r| Ok((r.get(0).ok(), r.get(1).ok())),
            )
            .unwrap_or((None, None))
    }
}

impl Store {
    /// One claim, gone, named rather than swept. A delta says which identifiers left, so there
    /// is nothing to infer from a run that did not mention them.
    pub fn remove(&self, record_id: &str, run: i64, at: &str) -> Result<bool, String> {
        let title: Option<String> = self
            .db
            .query_row(
                "select title from record where record_id = ?1",
                rusqlite::params![record_id],
                |r| r.get(0),
            )
            .ok();
        let Some(title) = title else {
            return Ok(false);
        };
        self.db
            .execute(
                "insert or replace into removed(record_id, run, at, title) values(?1,?2,?3,?4)",
                rusqlite::params![record_id, run, at, title],
            )
            .map_err(|e| e.to_string())?;
        for table in ["ident", "field", "fts"] {
            self.db
                .execute(
                    &format!("delete from {table} where record_id = ?1"),
                    rusqlite::params![record_id],
                )
                .map_err(|e| e.to_string())?;
        }
        self.db
            .execute(
                "delete from record where record_id = ?1",
                rusqlite::params![record_id],
            )
            .map_err(|e| e.to_string())?;
        Ok(true)
    }
}

impl Store {
    /// Every field name any claim carries. The shape check holds a run's fields against the
    /// last one's, so a run that read nothing has to say what is there rather than nothing.
    pub fn field_names(&self) -> std::collections::BTreeSet<String> {
        let Ok(mut stmt) = self.db.prepare("select distinct name from field") else {
            return Default::default();
        };
        stmt.query_map([], |r| r.get::<_, String>(0))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }
}

// -- two copies of one source, brought together (sync.rs) ---------------------------------------
//
// A claim is what a source said at a time. Two copies of a world that each read the same source
// hold two lists of such observations, and neither is wrong: brought together, every observation
// stays, in the order it was made, and what the claim says now is what was observed last. So two
// copies merge without anybody deciding anything, which is what lets a world be worked on in two
// places at once with no lock (sync.rs).
//
// A copy's runs are numbered by it. The other's arrive as runs of this one, appended, each noting
// where it came from (`merged from <instance> run <n>`), so it is not sent back, and the highest
// of the other's taken is kept in `meta` (`sync.<instance>.run`), so it is not taken twice.

/// What a merge brought: claims made current, versions kept in the history only, claims taken
/// away, and the runs it added.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Merged {
    pub current: u64,
    pub history: u64,
    pub removed: u64,
    pub runs: u64,
}

pub fn merged_note(peer: &str, run: i64) -> String {
    format!("merged from {peer} run {run}")
}

impl Store {
    /// The runs after `after`, and what they observed, as a store of its own in `to` (a
    /// directory): the runs, the claims they made or saw, their versions, their removals and the
    /// receipts those name. A run merged from `skip` is left out: it came from there. The highest
    /// run here, for the next delta to start after.
    pub fn delta(&self, after: i64, skip: &str, to: &Path) -> Result<i64, String> {
        std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
        drop(Store::open(to)?);
        let file = to.join("claims.db");
        self.db.execute("attach database ?1 as d", rusqlite::params![file.to_string_lossy()]).map_err(|e| e.to_string())?;
        let skip_like = format!("merged from {skip} run %");
        let copied = (|| -> Result<(), String> {
            let ex = |sql: &str, p: &[&dyn rusqlite::ToSql]| self.db.execute(sql, p).map(|_| ()).map_err(|e| format!("{e}: {sql}"));
            ex("insert into d.run select * from main.run where id > ?1 and coalesce(note, '') not like ?2", &[&after, &skip_like])?;
            ex("insert into d.record select * from main.record where last_run in (select id from d.run) or changed_run in (select id from d.run)", &[])?;
            ex("insert into d.revision select * from main.revision where run in (select id from d.run)", &[])?;
            ex("insert into d.removed select * from main.removed where run in (select id from d.run)", &[])?;
            ex("insert or ignore into d.excerpt select * from main.excerpt where digest in (select excerpt from d.record union select excerpt from d.revision)", &[])?;
            // What each claim here was before these runs: the other side, seeing its own there,
            // knows these came after it, whatever the clocks say to the second.
            ex("create table if not exists d.was(record_id text not null, hash text not null)", &[])?;
            ex("insert into d.was select distinct record_id, hash from main.revision where record_id in (select record_id from d.record) and run not in (select id from d.run)", &[])?;
            Ok(())
        })();
        let _ = self.db.execute("detach database d", []);
        copied?;
        Ok(self.last_run())
    }

    /// The time a run of this store observed what it did: when it finished, or began.
    fn run_time(&self, run: i64) -> String {
        self.db
            .query_row("select coalesce(finished, started) from run where id = ?1", rusqlite::params![run], |r| r.get::<_, String>(0))
            .unwrap_or_default()
    }

    fn run_note(&self, run: i64) -> String {
        self.db.query_row("select coalesce(note, '') from run where id = ?1", rusqlite::params![run], |r| r.get::<_, String>(0)).unwrap_or_default()
    }

    /// Another copy's observations of this source (a delta, or a whole store), brought in: its
    /// runs appended as this one's, each claim current here where it was observed later there,
    /// every version of it in the history, a removal only where nothing here saw the claim since.
    pub fn merge_from(&self, from: &Path, peer: &str, history: bool) -> Result<Merged, String> {
        let other = Store::open(from)?;
        let done: i64 = self.meta(&format!("sync.{peer}.run")).and_then(|v| v.parse().ok()).unwrap_or(0);
        let base: i64 = self.meta(&format!("sync.{peer}.base")).and_then(|v| v.parse().ok()).unwrap_or(0);
        let mut m = Merged::default();
        // Their runs, as runs of this store.
        let mut map: BTreeMap<i64, i64> = BTreeMap::new();
        let runs: Vec<(i64, String, Option<String>, i64, i64, i64, i64, i64)> = {
            let mut stmt = other
                .db
                .prepare("select id, started, finished, complete, coalesce(added,0), coalesce(changed,0), coalesce(removed,0), coalesce(unchanged,0) from run where id > ?1 order by id")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(rusqlite::params![done], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)))
                .map_err(|e| e.to_string())?;
            rows.flatten().collect()
        };
        if runs.is_empty() {
            return Ok(m);
        }
        self.db.execute_batch("begin").map_err(|e| e.to_string())?;
        let result = (|| -> Result<(), String> {
            for (id, started, finished, complete, added, changed, removed, unchanged) in &runs {
                let local = self.last_run() + 1;
                self.db
                    .execute(
                        "insert into run(id, started, finished, complete, added, changed, removed, unchanged, note) values(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                        rusqlite::params![local, started, finished, complete, added, changed, removed, unchanged, merged_note(peer, *id)],
                    )
                    .map_err(|e| e.to_string())?;
                map.insert(*id, local);
                m.runs += 1;
            }
            let mapped = |run: i64| map.get(&run).copied();
            // Every claim they observed in those runs.
            let ids: Vec<(String, i64, Option<String>)> = {
                let mut stmt = other.db.prepare("select record_id, last_run, excerpt from record").map_err(|e| e.to_string())?;
                let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(|e| e.to_string())?;
                rows.flatten().collect()
            };
            for (id, their_run, digest) in ids {
                let Some(run) = mapped(their_run) else { continue };
                let Some(mut claim) = other.get(&id) else { continue };
                claim.excerpt = digest.as_deref().and_then(|d| other.excerpt(d));
                let their_at = other.run_time(their_run);
                let ours: Option<(i64, String)> = self.db.query_row("select last_run, hash from record where record_id = ?1", rusqlite::params![id], |r| Ok((r.get(0)?, r.get(1)?))).ok();
                let our_at = ours.as_ref().map(|(r, _)| self.run_time(*r)).unwrap_or_default();
                // In the same second, the clocks say nothing: what each side saw before decides. Theirs
                // came after ours where ours is among what theirs was; ours after theirs where theirs
                // is in our history. Otherwise what came from them gives way to what they saw after
                // it, and two observations of their own each give way to the same one on both sides.
                let tie_theirs = their_at == our_at
                    && ours.as_ref().is_some_and(|(r, h)| {
                        if *h == claim.hash {
                            return false;
                        }
                        let they_saw_ours = other.db.query_row("select 1 from was where record_id = ?1 and hash = ?2", rusqlite::params![id, h], |_| Ok(())).is_ok();
                        let we_saw_theirs = self.db.query_row("select 1 from revision where record_id = ?1 and hash = ?2 and run < ?3", rusqlite::params![id, claim.hash, r], |_| Ok(())).is_ok();
                        they_saw_ours || (!we_saw_theirs && (*r <= base || self.run_note(*r).starts_with(&format!("merged from {peer} run ")) || claim.hash > *h))
                    });
                if ours.is_none() || their_at > our_at || tie_theirs {
                    self.put(&claim, run, &their_at, history)?;
                    m.current += 1;
                } else if history && ours.as_ref().is_some_and(|(_, h)| *h != claim.hash) {
                    // Observed there before it was here: a version in the history, not what it says now.
                    let excerpt = match &claim.excerpt { Some(e) => Some(self.keep_excerpt(e)?), None => None };
                    self.db
                        .execute(
                            "insert or ignore into revision(record_id, run, at, hash, title, known, fields, excerpt) values(?1,?2,?3,?4,?5,?6,?7,?8)",
                            rusqlite::params![id, run, their_at, claim.hash, claim.title, claim.known, claim.fields_json().to_string(), excerpt],
                        )
                        .map_err(|e| e.to_string())?;
                    m.history += 1;
                }
            }
            // Their earlier versions, where the history is kept.
            if history {
                let versions: Vec<(String, i64, String, String, String, String, String, Option<String>)> = {
                    let mut stmt = other.db.prepare("select record_id, run, at, hash, title, known, fields, excerpt from revision").map_err(|e| e.to_string())?;
                    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?))).map_err(|e| e.to_string())?;
                    rows.flatten().collect()
                };
                for (id, run, at, hash, title, known, fields, digest) in versions {
                    let Some(run) = mapped(run) else { continue };
                    let excerpt = match digest.as_deref().and_then(|d| other.excerpt(d)) { Some(e) => Some(self.keep_excerpt(&e)?), None => None };
                    let n = self
                        .db
                        .execute(
                            "insert or ignore into revision(record_id, run, at, hash, title, known, fields, excerpt) values(?1,?2,?3,?4,?5,?6,?7,?8)",
                            rusqlite::params![id, run, at, hash, title, known, fields, excerpt],
                        )
                        .map_err(|e| e.to_string())?;
                    m.history += n as u64;
                }
            }
            // Their removals, where nothing here has seen the claim since.
            let removals: Vec<(String, i64, String, String)> = {
                let mut stmt = other.db.prepare("select record_id, run, at, title from removed").map_err(|e| e.to_string())?;
                let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).map_err(|e| e.to_string())?;
                rows.flatten().collect()
            };
            for (id, run, at, title) in removals {
                let Some(run) = mapped(run) else { continue };
                let ours: Option<i64> = self.db.query_row("select last_run from record where record_id = ?1", rusqlite::params![id], |r| r.get(0)).ok();
                let Some(our_run) = ours else { continue };
                if self.run_time(our_run) >= at {
                    continue;
                }
                for table in ["ident", "field", "fts"] {
                    self.db.execute(&format!("delete from {table} where record_id = ?1"), rusqlite::params![id]).map_err(|e| e.to_string())?;
                }
                self.db.execute("delete from record where record_id = ?1", rusqlite::params![id]).map_err(|e| e.to_string())?;
                self.db
                    .execute("insert or replace into removed(record_id, run, at, title) values(?1,?2,?3,?4)", rusqlite::params![id, run, at, title])
                    .map_err(|e| e.to_string())?;
                m.removed += 1;
            }
            let highest = runs.last().map(|r| r.0).unwrap_or(done);
            self.db
                .execute("insert or replace into meta(key, value) values(?1, ?2)", rusqlite::params![format!("sync.{peer}.run"), highest.to_string()])
                .map_err(|e| e.to_string())?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.db.execute_batch("commit").map_err(|e| e.to_string())?;
                Ok(m)
            }
            Err(e) => {
                let _ = self.db.execute_batch("rollback");
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_copies_of_a_source_merge_every_observation_and_the_latest_is_current() {
        let base = std::env::temp_dir().join(format!("zetlyn-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let decl = "name: t/adv\nkind: vulnerability\nfetch:\n  type: csv\n  path: a.csv\nclaims:\n  id:\n    scheme: cve\n    from: field:cve\n  title: field:title\n  known: field:published\n  properties:\n    cvss:\n      type: number\n      from: field:cvss\nretention:\n  history: true\n";
        let make = |name: &str, csv: &str| {
            let d = base.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("source.yaml"), decl).unwrap();
            std::fs::write(d.join("a.csv"), csv).unwrap();
            d
        };
        let rows = |first: &str, skip_two: bool| -> String { let mut s = format!("cve,title,cvss,published\nCVE-1,one,{first},2026-01-01\n"); for i in 2..=12 { if !(skip_two && i == 2) { s.push_str(&format!("CVE-{i},n{i},6.0,2026-01-01\n")); } } s };
        let csv1 = rows("5.0", false);
        // Two copies of one world, read once alike: here (a) and there (b).
        let a = make("a", &csv1);
        let b = make("b", &csv1);
        let sa = crate::source::Source::open(&a).unwrap();
        sa.run().unwrap();
        let sb = crate::source::Source::open(&b).unwrap();
        sb.run().unwrap();
        let cvss = |s: &crate::source::Source, id: &str| {
            let q = crate::source::Query { text: String::new(), pred: None, ids: vec![id.into()], seen_before: None, view: None, sort: None, limit: 1, offset: 0 };
            let rec = s.search(&q).unwrap().1.into_iter().next().map(|h| h.record_id);
            rec.and_then(|r| s.store.get(&r)).and_then(|c| c.fields.get("cvss").map(|v| v.display()))
        };
        // b starts from where a was: everything a had is b's already.
        sb.store.db.execute("insert or replace into meta(key, value) values('sync.A.run', '1')", []).unwrap();
        // Later, there, the source says otherwise, and CVE-2 is gone.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(b.join("a.csv"), rows("9.8", true)).unwrap();
        sb.run().unwrap();
        // Their delta, brought here.
        let delta = base.join("delta");
        sb.store.delta(1, "A", &delta).unwrap();
        let m = sa.store.merge_from(&delta, "B", true).unwrap();
        assert_eq!(m.runs, 1);
        assert_eq!(cvss(&sa, "CVE-1").as_deref(), Some("9.8"), "observed later there: current here");
        assert_eq!(cvss(&sa, "CVE-2"), None, "removed there after it was last seen here");
        let versions: i64 = sa.store.db.query_row("select count(*) from revision where record_id = (select record_id from ident where value = 'CVE-1')", [], |r| r.get(0)).unwrap_or(0);
        assert!(versions >= 2, "both values in the history: {versions}");
        // Not taken twice.
        assert_eq!(sa.store.merge_from(&delta, "B", true).unwrap().runs, 0);
        // A delta for b from here leaves out what came from b.
        let back = base.join("back");
        sa.store.delta(1, "B", &back).unwrap();
        let other = Store::open(&back).unwrap();
        assert_eq!(other.last_run(), 0, "nothing of b's goes back to b");
        let _ = std::fs::remove_dir_all(&base);
    }

    fn store(name: &str) -> (Store, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("zetlyn-store-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (Store::open(&dir).unwrap(), dir)
    }


    /// A tracker takes a source's mark to know whether to look again. A run that has started and
    /// not finished is not the mark: its claims are in a transaction nobody else can read yet.
    #[test]
    fn a_run_still_writing_is_not_the_mark() {
        let (s, dir) = store("mark");
        let none = std::collections::BTreeSet::new();
        let first = s.begin_run().unwrap();
        s.finish_run(first, true, 1, 0, 0, 0, &none, &crate::build::Notes::default(), None).unwrap();
        let second = s.begin_run().unwrap();
        assert_eq!((s.last_run(), s.last_finished_run()), (second, first));
        s.finish_run(second, true, 1, 0, 0, 0, &none, &crate::build::Notes::default(), None).unwrap();
        assert_eq!(s.last_finished_run(), second);
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// A slice is not the whole, and nine claims need not carry every property twenty thousand do.
    #[test]
    fn a_slice_is_not_held_to_every_property() {
        let (s, dir) = store("slice");
        let all: std::collections::BTreeSet<String> =
            ["cvss", "packages", "severity"].iter().map(|s| s.to_string()).collect();
        let first = s.begin_run().unwrap();
        s.finish_run(first, true, 20, 0, 0, 0, &all, &crate::build::Notes::default(), None)
            .unwrap();
        // The store holds twenty, as it would have after that update.
        s.db.execute("update run set records = 20 where id = ?1", [first]).unwrap();
        let second = s.begin_run().unwrap();
        let some: std::collections::BTreeSet<String> = ["cvss".to_string()].into_iter().collect();
        assert_eq!(s.shape_refusal(second, 20, 9, &some, false, None), None);
        // Read whole, the same loss is the source dropping a property, and it is refused.
        assert!(s
            .shape_refusal(second, 20, 20, &some, true, None)
            .is_some_and(|why| why.contains("packages")));
        // Unless the declaration no longer names it: renamed or taken out on purpose.
        let declared: std::collections::BTreeSet<String> = ["cvss", "severity"].iter().map(|s| s.to_string()).collect();
        assert!(s
            .shape_refusal(second, 20, 20, &some, true, Some(&declared))
            .is_some_and(|why| why.contains("severity") && !why.contains("packages")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
