//! The store. Browsing, faceting, filtering and full text are one query over one file.

use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::types::Value as S;
use rusqlite::{params_from_iter, Connection};
use serde_json::Value as J;

use crate::decl::{Declaration, FieldType};
use crate::expr::{Lit, Op, Pred};
use crate::record::{Attachment, Id, Origin, Record, Value};

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

create virtual table if not exists fts using fts5(record_id unindexed, title, text);
";

/// What a member could not answer, which is shown rather than swallowed.
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

/// A dataset is a directory somebody moves, so a store written by an earlier build has to
/// open under a later one. Each of these is a column that arrived after the first release;
/// SQLite refuses a duplicate and that refusal is the whole of the check.
fn migrate(db: &Connection) {
    // Existing full-text rows carry arbitrary rowids. Rebuilt once, aligned to the record
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
    // A changed record used to be given a new rowid, and its old full-text row stayed behind
    // matching searches under a record that no longer said that. `cve/kev` carried 3,453 index
    // rows for 1,726 records, and a search for one word answered eleven where five was the truth.
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
        // Records held before the column existed take the stamp of the run that first saw them.
        "update record set first_seen = coalesce((select started from run where id = first_run), '')
         where first_seen = ''",
    ] {
        let _ = db.execute_batch(statement);
    }
}

impl Store {
    pub fn open(dir: &Path) -> Result<Store, String> {
        let db = Connection::open(dir.join("records.db")).map_err(|e| e.to_string())?;
        db.execute_batch("pragma journal_mode=wal; pragma synchronous=normal;")
            .map_err(|e| e.to_string())?;
        db.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        migrate(&db);
        Ok(Store { db })
    }

    /// The high-water mark of the last complete run. A partial run does not advance it, so
    /// the records it failed to reach are fetched again rather than skipped forever.
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

    /// Added, changed or unchanged. A record whose hash matches the one held is not rewritten.
    pub fn put(
        &self,
        rec: &Record,
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
        if held.as_deref() == Some(rec.hash.as_str()) {
            self.db
                .execute(
                    "update record set last_run = ?2 where record_id = ?1",
                    rusqlite::params![rec.record_id, run],
                )
                .map_err(|e| e.to_string())?;
            return Ok("unchanged");
        }
        let verdict = if held.is_some() { "changed" } else { "added" };
        // The row keeps the rowid it already had. `insert or replace` deletes the old row and
        // assigns a new one otherwise, and the full-text row is keyed on this rowid: a changed
        // record would leave its old index entry behind, matching a search forever under a
        // record that no longer says that. One generation of that doubled a store's index.
        let kept_rowid: Option<i64> = held.as_ref().and_then(|_| {
            self.db
                .query_row(
                    "select rowid from record where record_id = ?1",
                    rusqlite::params![rec.record_id],
                    |r| r.get(0),
                )
                .ok()
        });
        // Every version, where the dataset asked for history. Nothing is written for a
        // record whose hash matched, so an unchanged source costs nothing.
        if history {
            self.db
                .execute(
                    "insert or replace into revision(record_id, run, at, hash, title, known, fields)
                     values(?1,?2,?3,?4,?5,?6,?7)",
                    rusqlite::params![
                        rec.record_id,
                        run,
                        at,
                        rec.hash,
                        rec.title,
                        rec.known,
                        rec.fields_json().to_string(),
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
                   first_run, changed_run, last_run)
                 values(?17,?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?16,?14,?15,?15)",
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

        // The full-text row carries the record's own rowid, so replacing it is a lookup rather
        // than a scan. `record_id` cannot be indexed inside an FTS table, and deleting by it is
        // linear: the day an upstream change rewrites 46,000 records, a scan per record turns a
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

    /// A partial run never removes a record: a source that answered half its pages looks exactly
    /// like a source that deleted half its records.
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

    // -- reading ------------------------------------------------------------------------------

    /// A predicate becomes SQL. What it names and this dataset cannot answer is returned rather
    /// than dropped, because a filter applied to some records and not others is a wrong count.
    pub fn filter(&self, pred: &Pred, types: &BTreeMap<String, FieldType>) -> Filter {
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
        types: &BTreeMap<String, FieldType>,
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
                    unanswered.push(format!("{left}: no such field here"));
                    return "1=0".into();
                };
                if !kind.ordered() && !matches!(op, Op::Eq | Op::Ne) {
                    unanswered.push(format!(
                        "{left} {} {}: a {} has no order until a scope declares a scale",
                        op.sql(),
                        right.display(),
                        kind.name()
                    ));
                    return "1=0".into();
                }
                let name = lit(params, S::Text(left.clone()));
                let cmp = match kind {
                    FieldType::Number => {
                        let n = match right {
                            Lit::Num(n) => *n,
                            other => other.display().parse().unwrap_or(f64::NAN),
                        };
                        let p = lit(params, S::Real(n));
                        format!("f.n {} {p}", op.sql())
                    }
                    FieldType::Date | FieldType::Interval => {
                        let p = lit(params, S::Text(right.display()));
                        format!("f.d {} {p}", op.sql())
                    }
                    FieldType::Bool => {
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
                // `in`, not a correlated `exists`. An `exists` makes SQLite walk the record table
                // and probe the field index once per row, and a count walks all of it; this way
                // the index on (name, value) picks the few matching records first and the record
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
        types: &BTreeMap<String, FieldType>,
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
            // Free where this deployment has held it a month, or where the publisher said
            // it a month ago. A deployment that started yesterday learned everything
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

    /// Per field: how many records carry it, every value with its count for a code or a bool, the
    /// range for a number or a date. A faceted browse before a query has been asked.
    pub fn fields(&self, decl: &Declaration) -> Vec<FieldSummary> {
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
                FieldType::Code | FieldType::Bool | FieldType::Text => {
                    values = self.facet(name, None, 24).unwrap_or_default();
                }
                FieldType::Number => {
                    if let Ok((lo, hi)) = self.db.query_row(
                        "select min(n), max(n) from field where name = ?1",
                        rusqlite::params![name],
                        |r| Ok((r.get::<_, Option<f64>>(0)?, r.get::<_, Option<f64>>(1)?)),
                    ) {
                        min = lo.map(|v| Value::Number(v).display());
                        max = hi.map(|v| Value::Number(v).display());
                    }
                }
                FieldType::Date | FieldType::Interval => {
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

    pub fn get(&self, record_id: &str) -> Option<Record> {
        self.db
            .query_row(
                "select record_id, kind, title, url, text, known, valid_from, valid_to,
                        ids, fields, origin, attachments, hash
                 from record where record_id = ?1",
                rusqlite::params![record_id],
                |r| {
                    let ids: String = r.get(8)?;
                    let fields: String = r.get(9)?;
                    let origin: String = r.get(10)?;
                    let valid_from: Option<String> = r.get(6)?;
                    let valid_to: Option<String> = r.get(7)?;
                    let o: J = serde_json::from_str(&origin).unwrap_or(J::Null);
                    Ok(Record {
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
                    })
                },
            )
            .ok()
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
    /// dataset's to work out, because only it knows which revisions to hold against each
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
    types: &BTreeMap<String, FieldType>,
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
    /// twenty records to twenty thousand, and comparing the two counts compares nothing.
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

    /// A run producing forty per cent fewer records than the last complete one, or missing a field
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
    ) -> Option<String> {
        let (before, had) = self.last_shape(run)?;
        if before == 0 {
            return None;
        }
        if records * 5 < before * 3 {
            return Some(format!(
                "{records} records against {before} on the last complete run, which is more than \
                 forty per cent fewer"
            ));
        }
        // A run that read nothing carries no fields, and has nothing to say about them.
        if read == 0 {
            return None;
        }
        let lost: Vec<&String> = had.iter().filter(|f| !fields.contains(*f)).collect();
        if !lost.is_empty() {
            return Some(format!(
                "the last complete run carried {} and this one does not",
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

    /// What a record looked like at a date, from the revisions it kept.
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

/// A record that was added or changed: its id, its title, and which of the two.
pub type Moved = (String, String, String);
/// A record that was removed: its id and the title it had.
pub type Gone = (String, String);

impl Store {
    /// Every record, in one pass, handed over one at a time. A published artifact is written
    /// while the store is read, so a dataset larger than memory publishes the same way a small
    /// one does.
    pub fn for_each_record(
        &self,
        mut each: impl FnMut(Record) -> Result<(), String>,
    ) -> Result<u64, String> {
        let mut stmt = self
            .db
            .prepare(
                "select record_id, kind, title, url, text, known, valid_from, valid_to,
                        ids, fields, origin, hash
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
            each(Record {
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
            })?;
            n += 1;
        }
        Ok(n)
    }

    /// Per identifier scheme, how many distinct values this store holds, folded. A scope joins
    /// on a scheme and on the folded value, so this is the count a curator reads to see whether
    /// it can. `schemes` counts records instead, which is the number a reader wants on a page.
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
