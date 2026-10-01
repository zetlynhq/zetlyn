//! A source: one source, one lifecycle, one directory. And the six calls it answers.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{json, Value as J};

use crate::build::{self, Notes};
use crate::sourcedecl::{SourceDecl, PropertyType, View};
use crate::expr::Pred;
use crate::claim::Claim;
use crate::store::{Filter, Hit, RunReport, Store, Unanswered};

pub struct Source {
    pub dir: PathBuf,
    pub decl: SourceDecl,
    pub store: Store,
}

#[derive(Default)]
pub struct Query {
    pub text: String,
    pub pred: Option<Pred>,
    pub view: Option<String>,
    /// Identifier values. Exact, and the cheapest way in.
    pub ids: Vec<String>,
    /// Only claims this workspace first held at or before this stamp. The paywall, and one
    /// condition in one query.
    pub seen_before: Option<String>,
    pub sort: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

impl Source {
    pub fn open(dir: &Path) -> Result<Source, String> {
        let decl = SourceDecl::load(dir)?;
        let store = Store::open(dir)?;
        Ok(Source {
            dir: dir.to_path_buf(),
            decl,
            store,
        })
    }

    pub fn types(&self) -> BTreeMap<String, PropertyType> {
        self.decl
            .records
            .fields
            .iter()
            .map(|(k, v)| (k.clone(), v.kind))
            .collect()
    }

    // -- the run ------------------------------------------------------------------------------

    pub fn run(&self) -> Result<RunReport, String> {
        self.run_with(false, false)
    }

    /// With `reread`, the source is read even where it says it has not changed. Once, to take the
    /// receipts a store written before 0.2 does not have: a claim that did not change gains the
    /// receipt of what the source handed over for it, and an unchanged source hands over nothing.
    ///
    /// With `from_start`, the stored mark is set aside as well and the whole of the declared
    /// coverage is read, as on the first update. That update sweeps, so a claim held under a name
    /// no update gives it any more is removed rather than kept beside its twin.
    pub fn run_with(&self, reread: bool, from_start: bool) -> Result<RunReport, String> {
        let reread = reread || from_start;
        // Where the source is one address and says it has not changed, there is nothing to read.
        // An hourly cadence against a file that changes twice a week is mostly this.
        let (short, fresh) = if reread { (None, None) } else { self.nothing_changed()? };
        if let Some(short) = short {
            return Ok(short);
        }
        self.decl.source.prepare(&self.dir)?;
        // A run that started from a stored mark read a slice of the source, not the whole
        // of it. It may not remove: every claim it did not touch is one it never asked
        // for. Only a run that read from the beginning of the declared coverage sweeps.
        let mark = if from_start { None } else { self.store.meta("mark") };
        // A read that goes on where one stopped did not start at the beginning either.
        let whole = mark.is_none() && !self.dir.join(crate::web::RESUME).exists();
        let run = self.store.begin_run()?;
        let at = crate::iso_stamp(crate::now());
        let history = self.decl.retention.history;
        let root = self.decl.source.root(&self.dir);
        let mut notes = Notes::default();
        let mut seen_fields: std::collections::BTreeSet<String> = Default::default();
        let mut written: std::collections::BTreeSet<String> = Default::default();
        let (mut added, mut changed, mut unchanged) = (0u64, 0u64, 0u64);
        let mut failure: Option<String> = None;

        self.store
            .db
            .execute_batch("begin")
            .map_err(|e| e.to_string())?;
        let outcome = crate::rows::each_row(
            &self.decl,
            &self.dir,
            &root,
            mark.clone(),
            |produced: crate::rows::Produced| {
                for (sub, origin) in
                    build::expand(&self.decl, produced.row, produced.origin, produced.expanded)
                {
                    let Some(rec) = build::build(&self.decl, sub, origin, &mut notes) else {
                        continue;
                    };
                    // A claim id seen twice in one run is the source repeating itself
                    // under one key, not a change. Written through, each pair would
                    // report a change on every run for ever.
                    if !written.insert(rec.record_id.clone()) {
                        notes.duplicates += 1;
                        continue;
                    }
                    seen_fields.extend(rec.fields.keys().cloned());
                    match self.store.put(&rec, run, &at, history)? {
                        "added" => added += 1,
                        "changed" => changed += 1,
                        _ => unchanged += 1,
                    }
                }
                Ok(())
            },
        );
        let mut high: Option<String> = None;
        match outcome {
            Ok(mark) => high = mark,
            Err(e) => failure = Some(e),
        }

        // A partial run never removes a claim. A run that read the whole source and saw
        // nothing is not a source that emptied itself, so it is partial too — but a run
        // that read a slice and saw nothing is the normal answer to `what changed since
        // yesterday`, and saying otherwise would make a quiet day look like a fault.
        let complete = failure.is_none()
            && !self.decl.source.truncating()
            && (!whole || added + changed + unchanged > 0);

        // The shape check, before anything is kept. A run whose shape moved too far does not
        // replace the store: it is rolled back and it says why.
        //
        // Held against what the store will hold after this run. A run over the whole source
        // sweeps what it did not see, so that is what it saw; the store's count here is from
        // before the sweep and never falls, which let a source that lost half its rows empty
        // half the store. A run over a slice keeps the rest, so there it is the store's count.
        let read = added + changed + unchanged;
        let after = if whole { read } else { self.store.count() };
        let refusal = if complete {
            self.store.shape_refusal(run, after, read, &seen_fields, whole)
        } else {
            None
        };
        if let Some(why) = refusal {
            self.store
                .db
                .execute_batch("rollback")
                .map_err(|e| e.to_string())?;
            self.store.begin_run_at(run, &at)?;
            self.store
                .refuse_run(run, &why, added + changed + unchanged)?;
            return self
                .store
                .run_report(run)
                .ok_or_else(|| "the update left no report".to_string());
        }

        let removed = if complete && whole {
            self.store.sweep(run)?
        } else {
            0
        };
        // Only a complete run advances the mark.
        if complete {
            // And only a complete run keeps what the source said its version is. Kept when it
            // was asked, an update stopped half way would be told next time that nothing had
            // changed, and the half would stand.
            if let Some(v) = &fresh {
                self.store.set_meta("validators", v)?;
            }
            if let Some(h) = &high {
                self.store.set_meta("mark", h)?;
            }
        }
        self.store
            .db
            .execute_batch("commit")
            .map_err(|e| e.to_string())?;

        self.store.finish_run(
            run,
            complete,
            added,
            changed,
            removed,
            unchanged,
            &seen_fields,
            &notes,
            failure.as_deref(),
        )?;
        self.store
            .run_report(run)
            .ok_or_else(|| "the update left no report".to_string())
    }

    // -- describe -----------------------------------------------------------------------------

    /// When it will look again, from its declared cadence and when it last finished.
    pub fn next_run(&self) -> Option<String> {
        let every = self
            .decl
            .schedule
            .every
            .as_deref()
            .and_then(crate::fetch::duration)?;
        let last = self
            .store
            .run_report(self.store.last_run())
            .and_then(|r| r.finished)
            .map(|f| crate::fetch::seconds_of(&f))?;
        Some(crate::iso_stamp(last + every))
    }

    pub fn state(&self) -> &'static str {
        match self.store.run_report(self.store.last_run()) {
            None => "empty",
            Some(r) if r.refused.is_some() => "refused",
            Some(r) if r.error.is_some() => "failing",
            Some(r) if !r.complete => "partial",
            Some(_) => "current",
        }
    }

    /// What I am, what I hold, what I can be asked.
    pub fn describe(&self) -> J {
        let d = &self.decl;
        let last = self.store.run_report(self.store.last_run());
        let fields: Vec<J> = self
            .store
            .fields(d)
            .into_iter()
            .map(|f| {
                let mut o = json!({
                    "name": f.name, "type": f.kind, "claims": f.records,
                });
                if let Some(v) = f.vocabulary {
                    o["vocabulary"] = json!(v);
                }
                if !f.values.is_empty() {
                    o["values"] = J::Array(
                        f.values
                            .iter()
                            .map(|(v, c)| json!({ "value": v, "claims": c }))
                            .collect(),
                    );
                }
                if f.min.is_some() || f.max.is_some() {
                    o["min"] = json!(f.min);
                    o["max"] = json!(f.max);
                }
                o
            })
            .collect();
        json!({
            "source": d.name,
            "kind": d.kind,
            "title": d.title,
            "about": d.about,
            "claims": self.store.count(),
            "state": self.state(),
            "last_update": last.as_ref().map(|r| json!({
                "id": r.id, "at": r.started, "finished": r.finished, "complete": r.complete,
                "added": r.added, "changed": r.changed, "removed": r.removed,
                "unchanged": r.unchanged,
            })),
            "next_update": self.next_run(),
            "cadence": d.schedule.every,
            "licence": d.licence,

            "history": d.retention.history,
            "schemes": J::Array(self.store.schemes().iter()
                .map(|(s, n)| json!({ "scheme": s, "claims": n })).collect()),
            "properties": J::Array(fields),
            "vocabulary": json!(d.vocabulary),
            // The whole shape, because a tracker that adopts one reaches it only through here.
            "views": J::Array(d.view.iter().map(|v| json!({
                "name": v.name,
                "title": if v.title.is_empty() { v.name.clone() } else { v.title.clone() },
                "default": v.default,
                "group": v.group,
                "where": v.filter,
                "columns": v.columns,
                "facets": v.facets,
                "sort": v.sort,
            })).collect()),
            "search": json!({
                "text": d.search.text,
                "compare": d.search.compare,
                "suggest": d.search.suggest,
                "examples": d.search.examples,
            }),
            "can": self.can(),
        })
    }

    pub fn can(&self) -> Vec<&'static str> {
        let mut can = vec!["text", "changes"];
        if !self.decl.records.fields.is_empty() {
            can.push("property");
            can.push("facet");
        }
        if self.decl.ids().is_some() {
            can.push("ids");
        }
        if self.decl.retention.history {
            can.push("as_of");
        }
        can
    }

    // -- search -------------------------------------------------------------------------------

    fn view_of(&self, q: &Query) -> Option<&View> {
        match &q.view {
            Some(name) => self.decl.view(name),
            None => None,
        }
    }

    fn combined(&self, q: &Query) -> (Option<Pred>, Option<&View>) {
        let view = self.view_of(q);
        let view_pred = view
            .and_then(|v| v.filter.as_deref())
            .and_then(crate::expr::parse_pred);
        let pred = match (q.pred.clone(), view_pred) {
            (Some(a), Some(b)) => Some(Pred::And(Box::new(a), Box::new(b))),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        (pred, view)
    }

    pub fn search(&self, q: &Query) -> Result<(u64, Vec<Hit>, Unanswered), String> {
        let types = self.types();
        let (pred, view) = self.combined(q);
        let filter: Option<Filter> = pred.as_ref().map(|p| self.store.filter(p, &types));
        let sort = q.sort.clone().or_else(|| view.and_then(|v| v.sort.clone()));
        let limit = if q.limit == 0 { 50 } else { q.limit };

        // An identifier is the cheapest way in, so it is tried before the index. A tracker asks by
        // identifier to complete a thing its other sources selected.
        let named: Vec<String> = if !q.ids.is_empty() {
            q.ids.clone()
        } else if q.pred.is_none() && !q.text.trim().is_empty() {
            vec![q.text.trim().to_string()]
        } else {
            Vec::new()
        };
        if !named.is_empty() {
            // A tracker completing a page hands over every key on it, which is thousands. Keeping
            // the order but checking membership against a set: the same list, without asking
            // whether each new claim is already in it by reading the whole list again.
            let mut ids = Vec::new();
            let mut held = BTreeSet::new();
            for value in &named {
                for record_id in self.store.by_identifier(value) {
                    if held.insert(record_id.clone()) {
                        ids.push(record_id);
                    }
                }
            }
            if !ids.is_empty() {
                let mut hits = Vec::new();
                for (i, id) in ids.iter().take(limit.max(1)).enumerate() {
                    if let Some(r) = self.store.get(id) {
                        hits.push(Hit {
                            record_id: r.record_id,
                            rank: i + 1,
                            title: r.title,
                            url: r.url,
                            kind: r.kind,
                            known: r.known,
                            why_id: r.ids.first().cloned(),
                            ids: r.ids,
                            fields: r.fields,
                            why_text: Vec::new(),
                            why_field: Vec::new(),
                            snippet: String::new(),
                        });
                    }
                }
                let n = hits.len() as u64;
                return Ok((n, hits, Unanswered::default()));
            }
            if !q.ids.is_empty() {
                return Ok((0, Vec::new(), Unanswered::default()));
            }
        }

        let (total, mut hits) = self.store.search(
            &q.text,
            filter.as_ref(),
            sort.as_deref(),
            &types,
            q.seen_before.as_deref(),
            limit,
            q.offset,
        )?;
        if let Some(p) = &pred {
            let mut named = Vec::new();
            crate::expr::fields_named(p, &mut named);
            for h in &mut hits {
                h.why_field = named
                    .iter()
                    .filter(|n| h.fields.contains_key(*n))
                    .cloned()
                    .collect();
            }
        }
        Ok((
            total,
            hits,
            filter.map(|f| f.unanswered).unwrap_or_default(),
        ))
    }

    pub fn facet(&self, q: &Query, field: &str, limit: usize) -> Vec<(String, u64)> {
        let types = self.types();
        let (pred, _) = self.combined(q);
        let filter = pred.as_ref().map(|p| self.store.filter(p, &types));
        self.store
            .facet(field, filter.as_ref(), limit)
            .unwrap_or_default()
    }

    /// The claims by id, and with `versions` every version each was at and its receipt.
    pub fn fetch(&self, ids: &[String], versions: bool) -> Vec<Claim> {
        ids.iter()
            .filter_map(|i| self.store.get(i))
            .map(|mut c| {
                c.dataset = self.decl.name.clone();
                c
            })
            .map(|mut c| {
                if versions {
                    c.versions = self.store.versions(&c.record_id);
                }
                c
            })
            .collect()
    }

    /// What was added, changed and removed since a mark, and for a change, which fields moved.
    ///
    /// The previous value comes from the revisions the source kept. Without `retention.history`
    /// a change says that a claim changed and cannot say what in it did.
    pub fn changes(&self, since: i64, limit: usize) -> J {
        let history = self.decl.retention.history;
        let mut out = Vec::new();
        for (record_id, run, is_new) in self.store.changed_since(since, limit) {
            let Some(now) = self.store.get(&record_id) else {
                continue;
            };
            let before = if history && !is_new {
                self.store.previous(&record_id, run)
            } else {
                None
            };
            let mut moved = Vec::new();
            if let Some((old_title, old_fields)) = &before {
                if old_title != &now.title {
                    moved.push(json!({ "property": "title", "was": old_title, "is": now.title }));
                }
                let mut names: std::collections::BTreeSet<&String> = old_fields.keys().collect();
                names.extend(now.fields.keys());
                for name in names {
                    let was = old_fields.get(name).map(|v| v.display());
                    let is = now.fields.get(name).map(|v| v.display());
                    if was != is {
                        moved.push(json!({ "property": name, "was": was, "is": is }));
                    }
                }
            }
            out.push(json!({
                "claim_id": record_id,
                "title": now.title,
                "url": now.url,
                "kind": now.kind,
                "known": now.known,
                "ids": now.ids_json(),
                "how": if is_new { "added" } else { "changed" },
                "update": run,
                "properties": if moved.is_empty() { J::Null } else { J::Array(moved) },
            }));
        }
        let (_, gone) = self.store.changes(since, limit);
        json!({
            "source": self.decl.name,
            "since": since,
            "mark": self.mark(),
            "history": history,
            "changed": J::Array(out),
            "removed": J::Array(gone.iter().map(|(id, title)| json!({
                "claim_id": id, "title": title, "how": "removed",
            })).collect()),
        })
    }
    /// Whether a claim this source holds satisfies a query, read the way a search reads it.
    pub fn holds(&self, record_id: &str, pred: &Pred) -> bool {
        let filter = self.store.filter(pred, &self.types());
        self.store.satisfies(record_id, &filter)
    }

    /// The current mark, to be handed back to `changes` later. A run number, because that is what
    /// this source counts in.
    pub fn mark(&self) -> i64 {
        self.store.last_run()
    }
}

/// The six calls. In process here; the same shapes over HTTP for a source somewhere else.
///
/// A tracker reaches its sources through this and nothing else. A tracker that read a source's store
/// would have to be taken apart to reach the first source that is not ours.
// The interface is six calls. A tracker uses four of them today and reaches for `changes` and
// `mark` at M3, so the two are defined and not yet called.
#[allow(dead_code)]
pub trait Interface {
    fn name(&self) -> &str;
    fn describe(&self) -> J;
    fn search(&self, q: &Query) -> Result<(u64, Vec<Hit>, Unanswered), String>;
    fn facet(&self, q: &Query, field: &str, limit: usize) -> Vec<(String, u64)>;
    fn fetch(&self, ids: &[String], versions: bool) -> Vec<Claim>;
    fn changes(&self, since: i64, limit: usize) -> J;
    fn mark(&self) -> i64;
}

impl Interface for Source {
    fn name(&self) -> &str {
        &self.decl.name
    }
    fn describe(&self) -> J {
        Source::describe(self)
    }
    fn search(&self, q: &Query) -> Result<(u64, Vec<Hit>, Unanswered), String> {
        Source::search(self, q)
    }
    fn facet(&self, q: &Query, field: &str, limit: usize) -> Vec<(String, u64)> {
        Source::facet(self, q, field, limit)
    }
    fn fetch(&self, ids: &[String], versions: bool) -> Vec<Claim> {
        Source::fetch(self, ids, versions)
    }
    fn changes(&self, since: i64, limit: usize) -> J {
        Source::changes(self, since, limit)
    }
    fn mark(&self) -> i64 {
        Source::mark(self)
    }
}

/// What a declaration claims about itself, held against what the store actually holds.
///
/// Every one of these is a sentence a reader is shown. An example that returns nothing is a
/// suggestion to type something that does not work; a column naming a field no claim carries is
/// an empty cell in every row. None of it is caught by the run, because none of it is wrong until
/// somebody reads it.
impl Source {
    pub fn check(&self) -> Vec<String> {
        let mut wrong = Vec::new();
        let d = &self.decl;
        let fields: Vec<String> = d.records.fields.keys().cloned().collect();
        let known = |name: &str| {
            matches!(name, "known" | "title" | "kind" | "url" | "id" | "text")
                || fields.iter().any(|f| f == name)
        };

        // A connection string written out is a password in a file that is meant to be shared.
        if let crate::sourcedecl::Fetch::Sql { dsn, .. } = &d.source {
            if !dsn.contains("${") {
                wrong.push("the connection string is written into the declaration; name a variable, as `dsn: \"${ORDERS_DSN}\"`".into());
            }
        }
        if self.store.count() == 0 {
            wrong.push("holds no claims, so nothing below could be checked".into());
            return wrong;
        }

        for example in &d.search.examples {
            let (text, pred) = crate::expr::parse_query(example);
            let q = Query {
                text,
                pred,
                limit: 1,
                ..Default::default()
            };
            match self.search(&q) {
                Ok((0, _, un)) if un.0.is_empty() => {
                    wrong.push(format!("the example {example:?} returns nothing"))
                }
                Ok((_, _, un)) if !un.0.is_empty() => wrong.push(format!(
                    "the example {example:?} cannot be answered: {}",
                    un.0.join("; ")
                )),
                Err(e) => wrong.push(format!("the example {example:?} fails: {e}")),
                _ => {}
            }
        }

        for name in d.search.compare.iter().chain(&d.search.suggest) {
            if !known(name) {
                wrong.push(format!("`search` names {name}, which no claim carries"));
            }
        }
        for v in &d.view {
            for name in v.columns.iter().chain(&v.facets) {
                if !known(name) {
                    wrong.push(format!(
                        "the view {:?} names {name}, which no claim carries",
                        v.name
                    ));
                }
            }
            if let Some(group) = &v.group {
                if !known(group) {
                    wrong.push(format!(
                        "the view {:?} groups by {group}, which no claim carries",
                        v.name
                    ));
                }
            }
            if let Some(filter) = &v.filter {
                match crate::expr::parse_pred(filter) {
                    None => wrong.push(format!(
                        "the view {:?} has a `where` nothing can parse",
                        v.name
                    )),
                    Some(p) => {
                        let mut named = Vec::new();
                        crate::expr::fields_named(&p, &mut named);
                        for n in named.iter().filter(|n| !known(n)) {
                            wrong.push(format!(
                                "the view {:?} filters on {n}, which no claim carries",
                                v.name
                            ));
                        }
                    }
                }
            }
        }

        // A field declared and never filled is a column of dashes, and the declaration is the only
        // place that says it should not be.
        for f in self.store.fields(d) {
            if f.records == 0 {
                wrong.push(format!(
                    "the property {} is declared and no claim carries it",
                    f.name
                ));
            }
        }
        wrong
    }
}

impl Source {
    /// A run that does not have to happen, and the claim of it.
    ///
    /// Only where the source is one address and answers `304`. A source that pages, crawls or
    /// reads a directory has no single thing to ask about; a source that offers neither an
    /// `ETag` nor a `Last-Modified` is asked once and then never again, because asking it every
    /// hour and learning nothing is the cost this is meant to avoid.
    ///
    /// The run is written down. A run that never happened and a run that found nothing look the
    /// same to a reader, and only one of them is true.
    fn nothing_changed(&self) -> Result<(Option<RunReport>, Option<String>), String> {
        let Some(url) = self.decl.source.single_url() else {
            return Ok((None, None));
        };
        let held = self.store.meta("validators");
        // Nothing held, or held and empty: the first says ask, the second says this source
        // cannot be asked.
        if held.as_deref() == Some("") {
            return Ok((None, None));
        }
        let answer = match crate::fetch::unchanged(url, self.decl.source.agent(), held.as_deref()) {
            Ok(a) => a,
            // A source that will not answer this is a source to fetch the old way, and the run
            // that follows will say what went wrong with it properly.
            Err(_) => return Ok((None, None)),
        };
        match answer {
            Some(fresh) => {
                Ok((None, Some(fresh)))
            }
            None => {
                let held = self.store.count();
                let run = self.store.begin_run()?;
                self.store.finish_run(
                    run,
                    true,
                    0,
                    0,
                    0,
                    held,
                    &self.store.field_names(),
                    &Notes::default(),
                    None,
                )?;
                Ok((self.store.run_report(run), None))
            }
        }
    }
}
