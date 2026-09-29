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
}

/// Every thing a tracker knows, by `scheme:value` with the value's case folded.
#[derive(Default)]
pub struct Snapshot {
    pub things: BTreeMap<String, Snap>,
    /// Source, its state as it described itself: `current`, `stale`, `failing`, `empty`.
    pub states: BTreeMap<String, String>,
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
  key text not null, source text not null, claims text not null, primary key(key, source));
create table if not exists conflict(
  key text not null, property text not null, since text not null, sources text not null,
  primary key(key, property));
create table if not exists signal(
  id integer primary key, at text not null, kind text not null, key text,
  property text, source text, was text, is_now text);
create index if not exists signal_key on signal(key);
create table if not exists reader(
  reader text not null, key text not null, property text not null, state text not null,
  at text not null, primary key(reader, key, property));
create table if not exists meta(key text primary key, value text not null);
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
}

impl ThingStore {
    pub fn open(dir: &Path) -> Result<ThingStore, String> {
        let db = Connection::open(dir.join("tracker.db")).map_err(|e| e.to_string())?;
        db.execute_batch("pragma journal_mode=wal; pragma synchronous=normal;")
            .map_err(|e| e.to_string())?;
        db.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        Ok(ThingStore { db })
    }

    pub fn meta(&self, key: &str) -> Option<String> {
        self.db
            .query_row("select value from meta where key = ?1", [key], |r| r.get(0))
            .ok()
    }

    /// The snapshot this store holds, as the last refresh left it.
    fn held(&self) -> Result<Snapshot, String> {
        let mut snap = Snapshot::default();
        let mut stmt = self
            .db
            .prepare("select key, scheme, value, title from thing")
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
            .prepare("select key, source, property, raw, means, kind, understood from said")
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
            .prepare("select key, source, claims from speaks")
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
    pub fn refresh(
        &mut self,
        decl: &TrackerDecl,
        now: &Snapshot,
        rebuild: bool,
    ) -> Result<Refreshed, String> {
        let at = crate::iso_stamp(crate::now());
        let first = rebuild || self.meta("refreshed").is_none();
        let before = if first { Snapshot::default() } else { self.held()? };
        let held_conflicts: BTreeMap<(String, String), String> = if rebuild {
            BTreeMap::new()
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

        let tx = self.db.transaction().map_err(|e| e.to_string())?;
        tx.execute_batch("delete from said; delete from speaks; delete from conflict;")
            .map_err(|e| e.to_string())?;
        if rebuild {
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
            let mut put_said = tx
                .prepare(
                    "insert into said(key, source, property, raw, means, kind, understood)
                     values(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                )
                .map_err(|e| e.to_string())?;
            let mut put_speaks = tx
                .prepare("insert into speaks(key, source, claims) values(?1, ?2, ?3)")
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
                    }
                }
                for (source, claims) in &snap.claims {
                    put_speaks
                        .execute(rusqlite::params![key, source, json!(claims).to_string()])
                        .map_err(|e| e.to_string())?;
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
                    "insert into signal(at, kind, key, property, source, was, is_now)
                     values(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
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
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(Refreshed {
            things: now.things.len() as u64,
            conflicts: conflicts.len() as u64,
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
