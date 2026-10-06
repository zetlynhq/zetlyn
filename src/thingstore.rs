//! A tracker's own store: every thing it knows, what each source says about each of its
//! properties, where they disagree, and what changed since the last time it looked.
//!
//! Derived and disposable. It holds nothing a source does not, `zetlyn tracker rebuild` makes it
//! again from the sources, and it is never published. It exists because four things cannot be
//! derived at read time: a list of every conflict, a conflict that appeared (which needs the state
//! before it), a watch on one thing (which needs the thing to be the same thing tomorrow), and the
//! counts a tracker's overview gives without paging every source through `search`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use rusqlite::Connection;
use serde_json::{json, Value as J};

use crate::claim::Value;
use crate::trackerdecl::TrackerDecl;

/// What one source says about one property of one thing, over every claim it holds about it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Said {
    /// The source's own words, as each claim displays them.
    pub raw: BTreeSet<String>,
    /// What the tracker makes of them, where it aligns the property; the words otherwise.
    pub means: BTreeSet<String>,
    /// The kind of value: `code`, `number`, `bool`, `date`, `text` or `interval`.
    pub kind: String,
    /// Whether every word was one the alignment names or its scale holds. A word no map covers
    /// is a difference of wording, and not yet one of judgement.
    pub understood: bool,
}

/// One thing, as the sources say it is.
#[derive(Clone, Debug, Default)]
pub struct Snap {
    pub scheme: String,
    pub value: String,
    pub title: String,
    /// Source, property, what it says.
    pub by: BTreeMap<String, BTreeMap<String, Said>>,
    /// Source, the claims it holds about the thing.
    pub claims: BTreeMap<String, BTreeSet<String>>,
    /// Relation, the other side, and who says so: a source, or `person:<who>` for a match.
    pub related: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
}

/// Every thing a tracker knows, by `scheme:value` with the value's case folded.
#[derive(Default)]
pub struct Snapshot {
    pub things: BTreeMap<String, Snap>,
    /// Source, its state as it described itself: `current`, `stale`, `failing`, `empty`.
    pub states: BTreeMap<String, String>,
    /// Source, the kind of claim it makes.
    pub kinds: BTreeMap<String, String>,
    /// Source, the shape of its claims: the properties it says, with their types, and the
    /// identifier schemes. A source whose shape changed was read again under a new declaration.
    pub shapes: BTreeMap<String, String>,
}

impl Snap {
    /// The things' properties said by two sources or more, and whether they conflict, differ only
    /// in wording, or agree.
    pub fn verdicts(&self, decl: &TrackerDecl) -> Vec<(String, Verdict, BTreeMap<String, Vec<String>>)> {
        let mut names: BTreeSet<&String> = BTreeSet::new();
        for props in self.by.values() {
            names.extend(props.keys());
        }
        let mut out = Vec::new();
        for name in names {
            let said: Vec<(&String, &Said)> = self
                .by
                .iter()
                .filter_map(|(s, props)| props.get(name).map(|p| (s, p)))
                .collect();
            // Compared only where the tracker says the property is one property across its
            // sources. The same name is not the same meaning: a quantisation's downloads and its
            // base model's are two counts of two things, and weighing them made 598 conflicts.
            if decl.normalise_for(name).is_none() {
                continue;
            }
            if said.len() < 2 {
                continue;
            }
            let values: BTreeMap<String, Vec<String>> = said
                .iter()
                .map(|(s, p)| ((*s).clone(), p.means.iter().cloned().collect()))
                .collect();
            out.push((name.clone(), verdict(decl, name, &said), values));
        }
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Agree,
    /// The words differ and no map says what they mean together.
    Wording,
    Conflict,
}

/// Whether sources that say a property of one thing disagree about it.
fn verdict(decl: &TrackerDecl, name: &str, said: &[(&String, &Said)]) -> Verdict {
    let align = decl.normalise_for(name);
    let kind = said.iter().map(|(_, s)| s.kind.as_str()).find(|k| *k != "list").unwrap_or("text");
    match kind {
        // Prose is shown side by side and never weighed.
        "text" => Verdict::Agree,
        "number" => {
            let t = align.map(|a| a.number_tolerance()).unwrap_or(0.0);
            let sets: Vec<Vec<f64>> = said
                .iter()
                .map(|(_, s)| s.means.iter().filter_map(|v| v.parse::<f64>().ok()).collect())
                .collect();
            if pairwise_differ(&sets, |a, b| (a - b).abs() <= t + f64::EPSILON) {
                Verdict::Conflict
            } else {
                Verdict::Agree
            }
        }
        "date" => {
            let t = align.map(|a| a.day_tolerance()).unwrap_or(0);
            let sets: Vec<Vec<i64>> = said
                .iter()
                .map(|(_, s)| s.means.iter().filter_map(|v| days(v)).collect())
                .collect();
            if pairwise_differ(&sets, |a, b| (a - b).abs() <= t) {
                Verdict::Conflict
            } else {
                Verdict::Agree
            }
        }
        _ => {
            let differ = said.windows(2).any(|w| w[0].1.means != w[1].1.means);
            if !differ {
                return Verdict::Agree;
            }
            // A code is weighed only where the tracker says what its words mean and every word
            // is one it covers. Otherwise `linux` against `linux, unix` is two vocabularies, and
            // calling it a conflict would bury the real ones: 1,470 of 1,470 in the CVE tracker.
            let coded = kind == "code";
            // `align` is always there by now; a word it does not cover is still wording.
            if coded && said.iter().any(|(_, s)| !s.understood) {
                Verdict::Wording
            } else {
                Verdict::Conflict
            }
        }
    }
}

/// Every value one source gives has a match in every other's, within `same`, and the other way.
fn pairwise_differ<T: Copy>(sets: &[Vec<T>], same: impl Fn(T, T) -> bool) -> bool {
    for (i, a) in sets.iter().enumerate() {
        for b in sets.iter().skip(i + 1) {
            let covered = |x: &Vec<T>, y: &Vec<T>| x.iter().all(|v| y.iter().any(|w| same(*v, *w)));
            if !covered(a, b) || !covered(b, a) {
                return true;
            }
        }
    }
    false
}

/// Days since the epoch, for `YYYY-MM-DD` and anything that starts with it.
fn days(s: &str) -> Option<i64> {
    let d = s.get(..10)?;
    let mut parts = d.split('-');
    let (y, m, day) = (
        parts.next()?.parse::<i64>().ok()?,
        parts.next()?.parse::<i64>().ok()?,
        parts.next()?.parse::<i64>().ok()?,
    );
    // Howard Hinnant's days-from-civil.
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146097 + doe - 719468)
}

/// A value as the snapshot holds it: displayed, and of which kind.
pub fn kind_of(v: &Value) -> &'static str {
    match v {
        Value::List(items) => items.first().map(kind_of).unwrap_or("text"),
        other => other.type_name(),
    }
}

const SCHEMA: &str = "
create table if not exists thing(
  key text primary key, scheme text not null, value text not null, title text not null,
  first_seen text not null, changed text not null);
create table if not exists said(
  key text not null, source text not null, property text not null,
  raw text not null, means text not null, kind text not null, understood integer not null,
  primary key(key, source, property));
create table if not exists speaks(
  key text not null, source text not null, claims text not null, first_seen text, kind text,
  primary key(key, source));
create table if not exists conflict(
  key text not null, property text not null, since text not null, sources text not null,
  primary key(key, property));
create table if not exists related(
  key text not null, name text not null, target text not null, sources text not null,
  primary key(key, name, target));
create index if not exists related_target on related(name, target);
create table if not exists signal(
  id integer primary key, at text not null, kind text not null, key text,
  property text, source text, was text, is_now text);
create index if not exists signal_key on signal(key);
create table if not exists reader(
  reader text not null, key text not null, property text not null, state text not null,
  at text not null, primary key(reader, key, property));
create table if not exists meta(key text primary key, value text not null);
-- Which things a source says a property is a word of, the word case folded: the index a filter is
-- answered by, written again with `said` on every refresh.
create table if not exists word(property text not null, word text not null, key text not null);
create index if not exists word_at on word(property, word);
create index if not exists speaks_source on speaks(lower(source));
create index if not exists speaks_kind on speaks(lower(kind));
-- Which thing each claim of each source is about, so a refresh that reads only the claims moved
-- since the last one finds the things they were about before they moved. Written with `speaks`.
create table if not exists claim(source text not null, record_id text not null, key text not null);
create index if not exists claim_at on claim(source, record_id);
create index if not exists claim_key on claim(key);
-- And the words found by thing, for the same refresh to take out what it writes again.
create index if not exists word_key on word(key);
";

pub struct ThingStore {
    pub db: Connection,
}

/// What a refresh found: how many things, conflicts and signals.
#[derive(Debug, Default)]
pub struct Refreshed {
    pub things: u64,
    pub conflicts: u64,
    pub wording: u64,
    pub signals: u64,
    pub first: bool,
    /// Where the refresh read only the things a moved claim is about: how many.
    pub read_again: Option<u64>,
}

impl ThingStore {
    pub fn open(dir: &Path) -> Result<ThingStore, String> {
        let db = Connection::open(dir.join("tracker.db")).map_err(|e| e.to_string())?;
        db.execute_batch("pragma journal_mode=wal; pragma synchronous=normal;")
            .map_err(|e| e.to_string())?;
        db.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        // Stores written before a source's first word about a thing was kept.
        for statement in [
            "alter table speaks add column first_seen text",
            "alter table speaks add column kind text",
        ] {
            let _ = db.execute_batch(statement);
        }
        Ok(ThingStore { db })
    }

    pub fn meta(&self, key: &str) -> Option<String> {
        self.db
            .query_row("select value from meta where key = ?1", [key], |r| r.get(0))
            .ok()
    }

    /// The things these claims of this source were about at the last refresh.
    pub fn about_claims(&self, source: &str, record_ids: &[String]) -> BTreeSet<String> {
        let Ok(mut stmt) = self.db.prepare("select key from claim where source = ?1 and record_id = ?2") else {
            return BTreeSet::new();
        };
        record_ids
            .iter()
            .flat_map(|id| {
                stmt.query_map(rusqlite::params![source, id], |r| r.get::<_, String>(0))
                    .map(|rows| rows.flatten().collect::<Vec<_>>())
                    .unwrap_or_default()
            })
            .collect()
    }

    /// Every claim each source spoke of these things with at the last refresh.
    pub fn claims_about(&self, keys: &BTreeSet<String>) -> BTreeMap<String, BTreeSet<String>> {
        let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let Ok(mut stmt) = self.db.prepare("select source, record_id from claim where key = ?1") else {
            return out;
        };
        for k in keys {
            if let Ok(rows) = stmt.query_map([k], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) {
                for (source, id) in rows.flatten() {
                    out.entry(source).or_default().insert(id);
                }
            }
        }
        out
    }

    /// The snapshot this store holds, as the last refresh left it.
    fn held(&self, only: bool) -> Result<Snapshot, String> {
        // Only the things in `temp.only_keys`, where a refresh reads only what moved.
        let w = if only { " where key in (select key from temp.only_keys)" } else { "" };
        let mut snap = Snapshot::default();
        let mut stmt = self
            .db
            .prepare(&format!("select key, scheme, value, title from thing{w}"))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for (key, scheme, value, title) in rows.flatten() {
            snap.things.insert(
                key,
                Snap { scheme, value, title, ..Snap::default() },
            );
        }
        let mut stmt = self
            .db
            .prepare(&format!("select key, name, target, sources from related{w}"))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)))
            .map_err(|e| e.to_string())?;
        for (key, name, target, sources) in rows.flatten() {
            if let Some(t) = snap.things.get_mut(&key) {
                let who: BTreeSet<String> = serde_json::from_str(&sources).unwrap_or_default();
                t.related.entry(name).or_default().insert(target, who);
            }
        }
        let mut stmt = self
            .db
            .prepare(&format!("select key, source, property, raw, means, kind, understood from said{w}"))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, bool>(6)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for (key, source, property, raw, means, kind, understood) in rows.flatten() {
            if let Some(t) = snap.things.get_mut(&key) {
                t.by.entry(source).or_default().insert(
                    property,
                    Said {
                        raw: serde_json::from_str(&raw).unwrap_or_default(),
                        means: serde_json::from_str(&means).unwrap_or_default(),
                        kind,
                        understood,
                    },
                );
            }
        }
        let mut stmt = self
            .db
            .prepare(&format!("select key, source, claims from speaks{w}"))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
            })
            .map_err(|e| e.to_string())?;
        for (key, source, claims) in rows.flatten() {
            if let Some(t) = snap.things.get_mut(&key) {
                t.claims.insert(source, serde_json::from_str(&claims).unwrap_or_default());
            }
        }
        if let Some(states) = self.meta("states") {
            snap.states = serde_json::from_str(&states).unwrap_or_default();
        }
        Ok(snap)
    }

    /// The store made to hold `now`, and every difference from what it held written down as a
    /// signal. The first refresh, and a rebuild, write none: there is nothing to have changed
    /// from, and a feed that opened with sixty thousand things appearing would say nothing.
    ///
    /// With `only`, `now` holds those things and no others, read again because a claim about
    /// them moved; the rest of the store stands as it is. Everything below is said of the things
    /// in `now` and in `before`, which then are those things alone.
    pub fn refresh(
        &mut self,
        decl: &TrackerDecl,
        now: &Snapshot,
        rebuild: bool,
        only: Option<&BTreeSet<String>>,
    ) -> Result<Refreshed, String> {
        let at = crate::iso_stamp(crate::now());
        let first = rebuild || self.meta("refreshed").is_none();
        let only = if first { None } else { only };
        if let Some(keys) = only {
            self.db
                .execute_batch("create temp table if not exists only_keys(key text primary key); delete from temp.only_keys;")
                .map_err(|e| e.to_string())?;
            let mut put = self.db.prepare("insert or ignore into temp.only_keys(key) values(?1)").map_err(|e| e.to_string())?;
            for k in keys {
                put.execute([k]).map_err(|e| e.to_string())?;
            }
        }
        let before = if first { Snapshot::default() } else { self.held(only.is_some())? };
        let held_conflicts: BTreeMap<(String, String), String> = if rebuild {
            BTreeMap::new()
        } else if only.is_some() {
            let mut stmt = self
                .db
                .prepare("select key, property, since from conflict where key in (select key from temp.only_keys)")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
                })
                .map_err(|e| e.to_string())?;
            rows.flatten().map(|(k, p, s)| ((k, p), s)).collect()
        } else {
            let mut stmt = self
                .db
                .prepare("select key, property, since from conflict")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
                })
                .map_err(|e| e.to_string())?;
            rows.flatten().map(|(k, p, s)| ((k, p), s)).collect()
        };

        let mut signals: Vec<J> = Vec::new();
        let mut signal = |kind: &str, key: Option<&str>, property: Option<&str>, source: Option<&str>, was: J, is: J| {
            if !first {
                signals.push(json!({ "kind": kind, "key": key, "property": property,
                                     "source": source, "was": was, "is": is }));
            }
        };

        // The conflicts now, and the verdicts that are only wording, counted.
        let mut conflicts: BTreeMap<(String, String), BTreeMap<String, Vec<String>>> = BTreeMap::new();
        let mut wording = 0u64;
        for (key, snap) in &now.things {
            for (property, v, values) in snap.verdicts(decl) {
                match v {
                    Verdict::Conflict => {
                        conflicts.insert((key.clone(), property), values);
                    }
                    Verdict::Wording => wording += 1,
                    Verdict::Agree => {}
                }
            }
        }
        // Read in part, the count is the store's, less what these things counted before.
        if only.is_some() {
            let before_wording = before
                .things
                .values()
                .flat_map(|s| s.verdicts(decl))
                .filter(|(_, v, _)| matches!(v, Verdict::Wording))
                .count() as u64;
            let held_wording: u64 = self.meta("wording").and_then(|w| w.parse().ok()).unwrap_or(before_wording);
            wording = (held_wording + wording).saturating_sub(before_wording);
        }

        for (key, snap) in &now.things {
            let Some(old) = before.things.get(key) else {
                let from: Vec<&String> = snap.claims.keys().collect();
                signal("new_thing", Some(key), None, from.first().map(|s| s.as_str()), J::Null, json!(snap.title));
                continue;
            };
            for (source, props) in &snap.by {
                let old_props = old.by.get(source);
                if !old.claims.contains_key(source) {
                    signal("new_perspective", Some(key), None, Some(source), J::Null, json!(source));
                }
                for (property, said) in props {
                    let was = old_props.and_then(|p| p.get(property));
                    if was.map(|w| w.raw != said.raw).unwrap_or(old.claims.contains_key(source)) {
                        signal(
                            "changed",
                            Some(key),
                            Some(property),
                            Some(source),
                            was.map(|w| json!(w.raw)).unwrap_or(J::Null),
                            json!(said.raw),
                        );
                    }
                }
            }
            for source in old.claims.keys() {
                if !snap.claims.contains_key(source) {
                    signal("withdrawn", Some(key), None, Some(source), json!(source), J::Null);
                }
            }
        }
        for key in before.things.keys() {
            if !now.things.contains_key(key) {
                signal("withdrawn", Some(key), None, None, json!(before.things[key].title), J::Null);
            }
        }
        for (k, values) in &conflicts {
            if !held_conflicts.contains_key(k) {
                signal("conflict", Some(&k.0), Some(&k.1), None, J::Null, json!(values));
            }
        }
        for k in held_conflicts.keys() {
            if !conflicts.contains_key(k) {
                let now_values = now
                    .things
                    .get(&k.0)
                    .map(|s| {
                        s.verdicts(decl)
                            .into_iter()
                            .find(|(p, _, _)| *p == k.1)
                            .map(|(_, _, v)| json!(v))
                            .unwrap_or(J::Null)
                    })
                    .unwrap_or(J::Null);
                signal("resolved", Some(&k.0), Some(&k.1), None, J::Null, now_values);
            }
        }
        for (source, state) in &now.states {
            let was = before.states.get(source);
            if was.is_some() && was != Some(state) {
                signal("health", None, None, Some(source), json!(was), json!(state));
            }
        }

        // A source read again under a new declaration says new things about everything it holds,
        // and none of that is news: its properties did not change in the world, the reading of
        // them did. What it says of things is not signalled this once; one health signal says why.
        // New things and disagreements between sources are signalled as ever.
        let shaped_before: BTreeMap<String, String> =
            self.meta("shapes").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let reshaped: BTreeSet<String> = now
            .shapes
            .iter()
            .filter(|(s, shape)| shaped_before.get(*s).is_some_and(|was| was != *shape))
            .map(|(s, _)| s.clone())
            .collect();
        if !reshaped.is_empty() {
            signals.retain(|s| {
                let own = matches!(s["kind"].as_str(), Some("changed" | "new_perspective" | "withdrawn"));
                !(own && s["source"].as_str().is_some_and(|src| reshaped.contains(src)))
            });
            for source in &reshaped {
                signals.push(json!({ "kind": "health", "key": null, "property": null, "source": source,
                    "was": shaped_before.get(source), "is": format!("read again as {}", now.shapes[source]) }));
            }
        }
        // When each source first spoke of each thing, which a refresh keeps: `appeared:` asks it.
        let spoke: BTreeMap<(String, String), String> = self
            .db
            .prepare(if only.is_some() {
                "select key, source, first_seen from speaks where first_seen is not null and key in (select key from temp.only_keys)"
            } else {
                "select key, source, first_seen from speaks where first_seen is not null"
            })
            .and_then(|mut s| {
                s.query_map([], |r| Ok(((r.get::<_, String>(0)?, r.get::<_, String>(1)?), r.get::<_, String>(2)?)))
                    .map(|rows| rows.flatten().collect())
            })
            .unwrap_or_default();
        let tx = self.db.transaction().map_err(|e| e.to_string())?;
        if only.is_some() {
            tx.execute_batch(
                "delete from said where key in (select key from temp.only_keys);
                 delete from word where key in (select key from temp.only_keys);
                 delete from speaks where key in (select key from temp.only_keys);
                 delete from conflict where key in (select key from temp.only_keys);
                 delete from related where key in (select key from temp.only_keys);
                 delete from claim where key in (select key from temp.only_keys);",
            )
            .map_err(|e| e.to_string())?;
        } else {
            tx.execute_batch("delete from said; delete from word; delete from speaks; delete from conflict; delete from related; delete from claim;")
                .map_err(|e| e.to_string())?;
            // Which thing each claim is about is held from here on, and a refresh may read in part.
            tx.execute("insert or replace into meta(key, value) values('claims', '1')", []).map_err(|e| e.to_string())?;
        }
        // Said once the words are written in this same transaction: a store from before they were
        // kept is not asked by them.
        tx.execute("insert or replace into meta(key, value) values('words', '1')", []).map_err(|e| e.to_string())?;
        if rebuild {
            // A rebuild starts the log again but not its numbering: a watch or a reader holds the
            // last signal it saw by number, and one reused would hide what comes after it.
            tx.execute(
                &format!("insert or replace into meta(key, value) values('signals_before', ({NEXT}) - 1)"),
                [],
            )
            .map_err(|e| e.to_string())?;
            tx.execute_batch("delete from thing; delete from signal;")
                .map_err(|e| e.to_string())?;
        }
        {
            let mut put_thing = tx
                .prepare(
                    "insert into thing(key, scheme, value, title, first_seen, changed)
                     values(?1, ?2, ?3, ?4, ?5, ?5)
                     on conflict(key) do update set title = excluded.title",
                )
                .map_err(|e| e.to_string())?;
            let mut put_word = tx
                .prepare("insert into word(property, word, key) values(?1, ?2, ?3)")
                .map_err(|e| e.to_string())?;
            let mut put_said = tx
                .prepare(
                    "insert into said(key, source, property, raw, means, kind, understood)
                     values(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                )
                .map_err(|e| e.to_string())?;
            let mut put_related = tx
                .prepare("insert into related(key, name, target, sources) values(?1, ?2, ?3, ?4)")
                .map_err(|e| e.to_string())?;
            let mut put_speaks = tx
                .prepare("insert into speaks(key, source, claims, first_seen, kind) values(?1, ?2, ?3, ?4, ?5)")
                .map_err(|e| e.to_string())?;
            let mut put_claim = tx
                .prepare("insert into claim(source, record_id, key) values(?1, ?2, ?3)")
                .map_err(|e| e.to_string())?;
            for (key, snap) in &now.things {
                put_thing
                    .execute(rusqlite::params![key, snap.scheme, snap.value, snap.title, at])
                    .map_err(|e| e.to_string())?;
                for (source, props) in &snap.by {
                    for (property, said) in props {
                        put_said
                            .execute(rusqlite::params![
                                key,
                                source,
                                property,
                                json!(said.raw).to_string(),
                                json!(said.means).to_string(),
                                said.kind,
                                said.understood,
                            ])
                            .map_err(|e| e.to_string())?;
                        let words: BTreeSet<String> = said.means.iter().map(|w| w.to_lowercase()).collect();
                        for w in words {
                            put_word.execute(rusqlite::params![property, w, key]).map_err(|e| e.to_string())?;
                        }
                    }
                }
                for (name, targets) in &snap.related {
                    for (target, sources) in targets {
                        put_related
                            .execute(rusqlite::params![key, name, target, json!(sources).to_string()])
                            .map_err(|e| e.to_string())?;
                    }
                }
                for (source, claims) in &snap.claims {
                    put_speaks
                        .execute(rusqlite::params![
                            key,
                            source,
                            json!(claims).to_string(),
                            spoke.get(&(key.clone(), source.clone())).cloned().unwrap_or_else(|| at.clone()),
                            now.kinds.get(source).cloned().unwrap_or_default(),
                        ])
                        .map_err(|e| e.to_string())?;
                    for record_id in claims {
                        put_claim.execute(rusqlite::params![source, record_id, key]).map_err(|e| e.to_string())?;
                    }
                }
            }
            // A thing no source speaks of any more is gone from the store; the signal saying so
            // stays in the log.
            let gone: Vec<&String> = before
                .things
                .keys()
                .filter(|k| !now.things.contains_key(*k))
                .collect();
            for k in gone {
                tx.execute("delete from thing where key = ?1", [k])
                    .map_err(|e| e.to_string())?;
            }
            let mut put_conflict = tx
                .prepare(
                    "insert into conflict(key, property, since, sources) values(?1, ?2, ?3, ?4)",
                )
                .map_err(|e| e.to_string())?;
            for (k, values) in &conflicts {
                let since = held_conflicts.get(k).cloned().unwrap_or_else(|| at.clone());
                put_conflict
                    .execute(rusqlite::params![k.0, k.1, since, json!(values).to_string()])
                    .map_err(|e| e.to_string())?;
            }
            let mut put_signal = tx
                .prepare(
                    &format!("insert into signal(id, at, kind, key, property, source, was, is_now)
                     values(({NEXT}), ?1, ?2, ?3, ?4, ?5, ?6, ?7)"),
                )
                .map_err(|e| e.to_string())?;
            for s in &signals {
                put_signal
                    .execute(rusqlite::params![
                        at,
                        s["kind"].as_str(),
                        s["key"].as_str(),
                        s["property"].as_str(),
                        s["source"].as_str(),
                        s["was"].to_string(),
                        s["is"].to_string(),
                    ])
                    .map_err(|e| e.to_string())?;
            }
            // A thing that changed carries when, so a list can be ordered by it.
            for s in &signals {
                if let Some(k) = s["key"].as_str() {
                    tx.execute("update thing set changed = ?2 where key = ?1", rusqlite::params![k, at])
                        .map_err(|e| e.to_string())?;
                }
            }
            tx.execute(
                "insert or replace into meta(key, value) values('refreshed', ?1)",
                [&at],
            )
            .map_err(|e| e.to_string())?;
            tx.execute(
                "insert or replace into meta(key, value) values('states', ?1)",
                [json!(now.states).to_string()],
            )
            .map_err(|e| e.to_string())?;
            tx.execute(
                "insert or replace into meta(key, value) values('shapes', ?1)",
                [json!(now.shapes).to_string()],
            )
            .map_err(|e| e.to_string())?;
            tx.execute("insert or replace into meta(key, value) values('wording', ?1)", [wording.to_string()])
                .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        // Read in part, what the store holds is counted in the store.
        let count = |table: &str| -> u64 {
            self.db.query_row(&format!("select count(*) from {table}"), [], |r| r.get::<_, i64>(0)).unwrap_or(0) as u64
        };
        Ok(Refreshed {
            things: if only.is_some() { count("thing") } else { now.things.len() as u64 },
            conflicts: if only.is_some() { count("conflict") } else { conflicts.len() as u64 },
            read_again: only.map(|_| now.things.len().max(before.things.len()) as u64),
            wording,
            signals: signals.len() as u64,
            first,
        })
    }

    /// Every open conflict, newest first, with the reader's own state where there is a reader.
    pub fn conflicts(&self, reader: Option<&str>, limit: usize) -> Vec<J> {
        let Ok(mut stmt) = self.db.prepare(
            "select c.key, c.property, c.since, c.sources, t.title, t.scheme, t.value,
                    coalesce((select state from reader r where r.reader = ?1 and r.key = c.key
                              and r.property = c.property), 'new')
             from conflict c join thing t on t.key = c.key
             order by c.since desc, c.key limit ?2",
        ) else {
            return Vec::new();
        };
        stmt.query_map(rusqlite::params![reader.unwrap_or(""), limit as i64], |r| {
            Ok(json!({
                "key": r.get::<_, String>(0)?, "property": r.get::<_, String>(1)?,
                "since": r.get::<_, String>(2)?,
                "sources": serde_json::from_str::<J>(&r.get::<_, String>(3)?).unwrap_or(J::Null),
                "title": r.get::<_, String>(4)?, "scheme": r.get::<_, String>(5)?,
                "value": r.get::<_, String>(6)?, "state": r.get::<_, String>(7)?,
            }))
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }

    /// The signal log, newest first, from after `since` where given.
    pub fn signals(&self, since: Option<i64>, limit: usize) -> Vec<J> {
        let Ok(mut stmt) = self.db.prepare(
            "select s.id, s.at, s.kind, s.key, s.property, s.source, s.was, s.is_now, t.title,
                    t.scheme, t.value
             from signal s left join thing t on t.key = s.key
             where s.id > ?1 order by s.id desc limit ?2",
        ) else {
            return Vec::new();
        };
        stmt.query_map(rusqlite::params![since.unwrap_or(0), limit as i64], |r| {
            let parse = |s: Option<String>| {
                s.and_then(|v| serde_json::from_str::<J>(&v).ok()).unwrap_or(J::Null)
            };
            Ok(json!({
                "id": r.get::<_, i64>(0)?, "at": r.get::<_, String>(1)?,
                "kind": r.get::<_, String>(2)?, "key": r.get::<_, Option<String>>(3)?,
                "property": r.get::<_, Option<String>>(4)?, "source": r.get::<_, Option<String>>(5)?,
                "was": parse(r.get(6)?), "is": parse(r.get(7)?),
                "title": r.get::<_, Option<String>>(8)?, "scheme": r.get::<_, Option<String>>(9)?,
                "value": r.get::<_, Option<String>>(10)?,
            }))
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }

    /// How many things are named by how many sources, and how many only one source names.
    pub fn coverage(&self) -> J {
        let by: BTreeMap<String, i64> = self
            .db
            .prepare("select n, count(*) from (select key, count(*) n from speaks group by key) group by n")
            .and_then(|mut s| {
                s.query_map([], |r| Ok((r.get::<_, i64>(0)?.to_string(), r.get::<_, i64>(1)?)))
                    .map(|rows| rows.flatten().collect())
            })
            .unwrap_or_default();
        let only: BTreeMap<String, i64> = self
            .db
            .prepare(
                "select source, count(*) from speaks where key in
                   (select key from speaks group by key having count(*) = 1) group by source",
            )
            .and_then(|mut s| {
                s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
                    .map(|rows| rows.flatten().collect())
            })
            .unwrap_or_default();
        let things: i64 = self
            .db
            .query_row("select count(*) from thing", [], |r| r.get(0))
            .unwrap_or(0);
        let conflicts: i64 = self
            .db
            .query_row("select count(*) from conflict", [], |r| r.get(0))
            .unwrap_or(0);
        json!({ "things": things, "by_sources": by, "only": only, "conflicts": conflicts,
                "refreshed": self.meta("refreshed") })
    }

    /// A reader's own mark on a conflict. `new` takes it back. The conflict itself is the
    /// sources' and nobody's to resolve: it resolves when they agree.
    pub fn mark(&self, reader: &str, key: &str, property: &str, state: &str) -> Result<(), String> {
        if !matches!(state, "new" | "seen" | "muted") {
            return Err(format!("{state}: a conflict is new, seen or muted"));
        }
        self.db
            .execute(
                "insert or replace into reader(reader, key, property, state, at)
                 values(?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![reader, key, property, state, crate::iso_stamp(crate::now())],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Where a reader was on the changes page last time, and a note that they are at `latest`
    /// now. Kept beside their marks on conflicts, as theirs and nobody else's.
    pub fn visited(&self, reader: &str, latest: i64) -> Option<i64> {
        let was: Option<i64> = self
            .db
            .query_row(
                "select state from reader where reader = ?1 and key = '' and property = 'changes'",
                [reader],
                |r| r.get::<_, String>(0),
            )
            .ok()
            .and_then(|s| s.parse().ok());
        if was.map(|w| latest > w).unwrap_or(true) {
            let _ = self.db.execute(
                "insert or replace into reader(reader, key, property, state, at)
                 values(?1, '', 'changes', ?2, ?3)",
                rusqlite::params![reader, latest.to_string(), crate::iso_stamp(crate::now())],
            );
        }
        was
    }

    /// How many signals a reader has not seen on the Changes page yet, without marking them seen.
    pub fn unseen(&self, reader: &str) -> i64 {
        let mark: i64 = self
            .db
            .query_row(
                "select state from reader where reader = ?1 and key = '' and property = 'changes'",
                [reader],
                |r| r.get::<_, String>(0),
            )
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        self.db.query_row("select count(*) from signal where id > ?1", [mark], |r| r.get(0)).unwrap_or(0)
    }
}

impl ThingStore {
    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), String> {
        self.db
            .execute(
                "insert or replace into meta(key, value) values(?1, ?2)",
                rusqlite::params![key, value],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// The properties of one thing its sources disagree about now.
    pub fn conflicts_of(&self, key: &str) -> BTreeSet<String> {
        self.db
            .prepare("select property from conflict where key = ?1")
            .and_then(|mut s| {
                s.query_map([key], |r| r.get::<_, String>(0))
                    .map(|rows| rows.flatten().collect())
            })
            .unwrap_or_default()
    }

    /// How many conflicts per property, for a list that says how many before it lists them.
    pub fn conflict_counts(&self) -> Vec<(String, i64)> {
        self.db
            .prepare("select property, count(*) from conflict group by property order by count(*) desc")
            .and_then(|mut s| {
                s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
                    .map(|rows| rows.flatten().collect())
            })
            .unwrap_or_default()
    }
}

impl ThingStore {
    /// Every thing, as a question about things is asked of it.
    pub fn views(&self) -> Result<Vec<crate::thingquery::ThingView>, String> {
        let held = self.held(false)?;
        let mut speaks: BTreeMap<String, BTreeMap<String, (String, String)>> = BTreeMap::new();
        let mut stmt = self
            .db
            .prepare("select key, source, coalesce(first_seen, ''), coalesce(kind, '') from speaks")
            .map_err(|e| e.to_string())?;
        for (key, source, first, kind) in stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .flatten()
        {
            speaks.entry(key).or_default().insert(source, (first, kind));
        }
        let mut conflicts: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut stmt = self
            .db
            .prepare("select key, property from conflict")
            .map_err(|e| e.to_string())?;
        for (key, property) in stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?
            .flatten()
        {
            conflicts.entry(key).or_default().insert(property);
        }
        let mut changed: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
        let mut stmt = self
            .db
            .prepare("select key, property, at from signal where kind = 'changed' and key is not null")
            .map_err(|e| e.to_string())?;
        for (key, property, at) in stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .flatten()
        {
            changed.entry(key).or_default().entry(property).or_default().push(at);
        }
        Ok(held
            .things
            .into_iter()
            .map(|(key, snap)| crate::thingquery::ThingView {
                value: snap.value,
                by: snap.by,
                speaks: speaks.remove(&key).unwrap_or_default(),
                conflicts: conflicts.remove(&key).unwrap_or_default(),
                changed: changed.remove(&key).unwrap_or_default(),
                related: snap.related,
                key,
            })
            .collect())
    }

    /// The keys of every thing a question holds for, sorted. Each part of the question is answered
    /// from the store's own tables and only the sets of keys are put together: a comparison reads
    /// the one property it names. Reading every thing whole to ask it the question held the whole
    /// tracker in memory for every request, and crawlers asked it twice a second (2026-10-06).
    pub fn matching(
        &self,
        q: &crate::thingquery::Q,
        cx: &crate::thingquery::Context,
    ) -> Result<Vec<String>, String> {
        // The way it was, every thing read whole, where a test holds the two against each other.
        if std::env::var_os("ZETLYN_THINGS_WHOLE").is_some() {
            return Ok(self.views()?.iter().filter(|t| crate::thingquery::holds(q, t, cx)).map(|t| t.key.clone()).collect());
        }
        let mut every: Option<BTreeSet<String>> = None;
        Ok(self.keys_for(q, cx, &mut every)?.into_iter().collect())
    }

    /// The keys a query of one column, the key, returns.
    fn keys_of(&self, sql: &str, params: &[&dyn rusqlite::ToSql]) -> Result<BTreeSet<String>, String> {
        let mut stmt = self.db.prepare(sql).map_err(|e| e.to_string())?;
        let rows = stmt.query_map(params, |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
        Ok(rows.flatten().collect())
    }

    /// Key and one more column, for a part of a question the store cannot say in SQL alone.
    fn pairs_of(&self, sql: &str, params: &[&dyn rusqlite::ToSql]) -> Result<Vec<(String, String)>, String> {
        let mut stmt = self.db.prepare(sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params, |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default())))
            .map_err(|e| e.to_string())?;
        Ok(rows.flatten().collect())
    }

    /// What one source says of one property, per thing, as the words it means.
    fn means_of(&self, source: &str, property: &str) -> Result<BTreeMap<String, BTreeSet<String>>, String> {
        Ok(self
            .pairs_of("select key, means from said where source = ?1 and property = ?2", &[&source, &property])?
            .into_iter()
            .map(|(k, m)| (k, serde_json::from_str::<BTreeSet<String>>(&m).unwrap_or_default()))
            .collect())
    }

    /// The things one part of a question holds for, as `thingquery::holds` would say of each.
    fn keys_for(
        &self,
        q: &crate::thingquery::Q,
        cx: &crate::thingquery::Context,
        every: &mut Option<BTreeSet<String>>,
    ) -> Result<BTreeSet<String>, String> {
        use crate::thingquery::{seconds, Op, Q};
        let recent = |at: &str, within: i64| seconds(at).is_some_and(|s| cx.now - s <= within);
        Ok(match q {
            Q::And(a, b) => {
                let x = self.keys_for(a, cx, every)?;
                if x.is_empty() {
                    x
                } else {
                    let y = self.keys_for(b, cx, every)?;
                    x.intersection(&y).cloned().collect()
                }
            }
            Q::Or(a, b) => {
                let mut x = self.keys_for(a, cx, every)?;
                x.extend(self.keys_for(b, cx, every)?);
                x
            }
            Q::Not(a) => {
                let x = self.keys_for(a, cx, every)?;
                if every.is_none() {
                    *every = Some(self.keys_of("select key from thing", &[])?);
                }
                every.as_ref().map(|all| all.difference(&x).cloned().collect()).unwrap_or_default()
            }
            Q::Conflict(p) => self.keys_of("select key from conflict where property = ?1", &[p])?,
            Q::Has(s) => self.keys_of("select distinct key from speaks where source = ?1", &[s])?,
            Q::Only(s) => self.keys_of("select key from speaks group by key having count(*) = 1 and max(source) = ?1", &[s])?,
            Q::Appeared(kind, within) => self
                .pairs_of("select key, first_seen from speaks where kind = ?1", &[kind])?
                .into_iter()
                .filter(|(_, first)| recent(first, *within))
                .map(|(k, _)| k)
                .collect(),
            Q::Changed(p, within) => self
                .pairs_of("select key, at from signal where kind = 'changed' and key is not null and coalesce(property, '') = ?1", &[p])?
                .into_iter()
                .filter(|(_, at)| recent(at, *within))
                .map(|(k, _)| k)
                .collect(),
            Q::Id(v) => self
                .pairs_of("select key, value from thing", &[])?
                .into_iter()
                .filter(|(_, value)| value.eq_ignore_ascii_case(v))
                .map(|(k, _)| k)
                .collect(),
            Q::Related(name, want) => {
                let want = want.to_lowercase();
                self.pairs_of("select key, target from related where name = ?1", &[name])?
                    .into_iter()
                    .filter(|(_, target)| match want.strip_suffix('*') {
                        Some(prefix) => target.starts_with(prefix),
                        None => *target == want,
                    })
                    .map(|(k, _)| k)
                    .collect()
            }
            Q::Cmp { source, property, op, value } => {
                let mut stmt = self
                    .db
                    .prepare("select key, source, means from said where property = ?1")
                    .map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map([property], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
                    .map_err(|e| e.to_string())?;
                let mut out = BTreeSet::new();
                for (key, said_by, means) in rows.flatten() {
                    if source.as_ref().is_some_and(|want| *want != said_by) || out.contains(&key) {
                        continue;
                    }
                    let words: Vec<String> = serde_json::from_str(&means).unwrap_or_default();
                    if words.iter().any(|w| crate::thingquery::compare(cx, property, w, *op, value)) {
                        out.insert(key);
                    }
                }
                out
            }
            Q::Between { a, op, b } => {
                let (x, y) = (self.means_of(&a.0, &a.1)?, self.means_of(&b.0, &b.1)?);
                x.iter()
                    .filter_map(|(k, words)| y.get(k).map(|other| (k, (words == other) == (*op == Op::Eq))))
                    .filter(|(_, holds)| *holds)
                    .map(|(k, _)| k.clone())
                    .collect()
            }
        })
    }


    /// The thing the website shows: exploited if any is, in dispute, and said by as many sources
    /// as any. The same one tomorrow unless the sources change, so the page does not flicker.
    pub fn showcase(&self, prefer: &str) -> Option<String> {
        self.db
            .query_row(
                "select c.key from conflict c join speaks s on s.key = c.key
                 group by c.key
                 order by max(s.source = ?1) desc, count(distinct s.source) desc, c.key
                 limit 1",
                [prefer],
                |r| r.get(0),
            )
            .ok()
    }

    /// What each source says of one thing: source, property, its words, what they mean here.
    pub fn said_of(&self, key: &str) -> Vec<(String, String, Vec<String>, Vec<String>)> {
        let Ok(mut stmt) = self.db.prepare("select source, property, raw, means from said where key = ?1 order by property, source") else {
            return Vec::new();
        };
        let list = |s: String| serde_json::from_str::<Vec<String>>(&s).unwrap_or_default();
        stmt.query_map([key], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)))
            .map(|rows| rows.flatten().map(|(s, p, raw, means)| (s, p, list(raw), list(means))).collect())
            .unwrap_or_default()
    }


    /// How many things each source speaks about.
    pub fn things_per_source(&self) -> BTreeMap<String, u64> {
        let Ok(mut stmt) = self.db.prepare("select source, count(*) from speaks group by source") else {
            return BTreeMap::new();
        };
        stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64)))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    /// What one thing is to other things: relation, the other side, and who says so.
    pub fn related_of(&self, key: &str) -> BTreeMap<String, BTreeMap<String, BTreeSet<String>>> {
        let mut out: BTreeMap<String, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
        let Ok(mut stmt) = self.db.prepare("select name, target, sources from related where key = ?1 order by name, target") else {
            return out;
        };
        let rows = stmt.query_map([key], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)));
        for (name, target, sources) in rows.into_iter().flatten().flatten() {
            out.entry(name).or_default().insert(target, serde_json::from_str(&sources).unwrap_or_default());
        }
        out
    }
    /// Every other side of one relation, with how many things are related to it, most first.
    pub fn targets(&self, name: &str) -> Vec<(String, u64)> {
        let Ok(mut stmt) = self.db.prepare("select target, count(*) from related where name = ?1 group by target order by 2 desc, 1") else {
            return Vec::new();
        };
        stmt.query_map([name], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64)))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    /// The sources that relate anything to one other side.
    pub fn relating(&self, name: &str, target: &str) -> BTreeSet<String> {
        let Ok(mut stmt) = self.db.prepare("select sources from related where name = ?1 and target = ?2") else {
            return BTreeSet::new();
        };
        stmt.query_map([name, target], |r| r.get::<_, String>(0))
            .map(|rows| rows.flatten().flat_map(|s| serde_json::from_str::<BTreeSet<String>>(&s).unwrap_or_default()).collect())
            .unwrap_or_default()
    }

    /// What every source says of one property of each of these things, as the words it means.
    pub fn words_of(&self, keys: &[String], property: &str) -> BTreeMap<String, BTreeSet<String>> {
        let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let Ok(mut stmt) = self.db.prepare("select means from said where key = ?1 and property = ?2") else {
            return out;
        };
        for key in keys {
            let words: BTreeSet<String> = stmt
                .query_map([key.as_str(), property], |r| r.get::<_, String>(0))
                .map(|rows| rows.flatten().flat_map(|m| serde_json::from_str::<Vec<String>>(&m).unwrap_or_default()).collect())
                .unwrap_or_default();
            if !words.is_empty() {
                out.insert(key.clone(), words);
            }
        }
        out
    }

    /// The sources that speak about one thing.
    pub fn speakers(&self, key: &str) -> Vec<String> {
        let Ok(mut stmt) = self.db.prepare("select source from speaks where key = ?1 order by source") else {
            return Vec::new();
        };
        stmt.query_map([key], |r| r.get(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
    }
    /// Title, scheme and value of a thing, for a list that shows it.
    pub fn named(&self, key: &str) -> Option<(String, String, String)> {
        self.db
            .query_row(
                "select title, scheme, value from thing where key = ?1",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok()
    }

    /// The highest signal id there is, which is the mark a watch or a reader starts from.
    pub fn last_signal(&self) -> i64 {
        self.db
            .query_row(&format!("select ({NEXT}) - 1"), [], |r| r.get(0))
            .unwrap_or(0)
    }
}

/// One signal as a sentence, for a feed, a mail or a terminal. The page draws its own.
pub fn say(s: &J) -> String {
    let thing = s["title"]
        .as_str()
        .filter(|t| !t.is_empty())
        .or(s["value"].as_str())
        .or(s["key"].as_str())
        .unwrap_or("");
    let source = s["source"].as_str().unwrap_or("");
    let property = s["property"].as_str().unwrap_or("");
    let words = |v: &J| -> String {
        match v {
            J::Array(a) => a.iter().filter_map(J::as_str).collect::<Vec<_>>().join(", "),
            J::Object(o) => o
                .iter()
                .map(|(k, v)| {
                    let w = v
                        .as_array()
                        .map(|a| a.iter().filter_map(J::as_str).collect::<Vec<_>>().join(", "))
                        .unwrap_or_default();
                    format!("{k} {w}")
                })
                .collect::<Vec<_>>()
                .join("; "),
            J::String(s) => s.clone(),
            _ => "nothing".into(),
        }
    };
    match s["kind"].as_str().unwrap_or("") {
        "new_thing" => format!("New: {thing}, first said by {source}"),
        "new_perspective" => format!("{source} now speaks about {thing}"),
        "changed" => format!(
            "{thing}: {property} at {source} was {} and is {}",
            words(&s["was"]),
            words(&s["is"])
        ),
        "conflict" => format!("{thing}: sources now disagree about {property}: {}", words(&s["is"])),
        "resolved" => format!("{thing}: sources agree again about {property}"),
        "withdrawn" if source.is_empty() => format!("{thing} is no longer said by any source"),
        "withdrawn" => format!("{source} no longer says anything about {thing}"),
        "health" => format!("{source} was {} and is {}", words(&s["was"]), words(&s["is"])),
        "entered" => format!("{thing} is now in this view"),
        "left" => format!("{thing} has left this view"),
        other => format!("{thing}: {other}"),
    }
}

/// The number the next signal takes: past every signal held, and past every one a rebuild let go.
const NEXT: &str = "select max(coalesce((select max(id) from signal), 0),
    coalesce((select cast(value as integer) from meta where key = 'signals_before'), 0)) + 1";
