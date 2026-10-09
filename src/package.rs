//! A tracker, when it travels as one file.
//!
//! A package is a SQLite file holding a part of a workspace: the tracker's statement and its own
//! store, and for each of its sources a declaration and a claim store. Opened, it is written back
//! out as those files, and everything that reads a tracker reads it as it reads any other.
//!
//! Sealed, which is the default, it carries what the tracker answers with and not how it was
//! made. Each source's claims are translated into the tracker's names and words before they are
//! packed, so the statement needs no `from:` and no word maps. Where a source was read, the
//! request, the file, the receipts and the source's own field names stay with the publisher. A
//! receipt's hash travels, so the publisher can show later what was read without showing it now.
//!
//! Open, it carries the files as they are, recipes included: for a tracker whose point is to be
//! copied and changed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};
use serde_json::{json, Value as J};

use crate::artifact::Reference;
use crate::place::{sha256, Place};
use crate::trackerdecl::{Packaged, TrackerDecl};

pub const FORMAT: &str = "zetlyn-package/1";
/// What a package file ends in.
pub const EXT: &str = ".zetlyn";
/// Where packages sit on a hub, beside `sources` and `trackers`.
pub const TREE: &str = "packages";
/// What an opened tracker keeps of the package it came in.
const HELD: &str = "package.json";
/// A claim's origin once it is sealed: it came from somewhere, and where is the publisher's.
const NO_ORIGIN: &str = r#"{"file":null,"row":null,"span":null,"url":null}"#;
/// Of a source store's own notes, the ones a reader needs. The rest is how it was fetched.
const KEPT_META: &[&str] = &["named", "fts_rowid", "fts_orphans"];

pub struct Options {
    pub sealed: bool,
    /// Sealed, a claim's own link is left out too, because a link names where it was read.
    pub links: bool,
}

pub struct Packed {
    pub bytes: Vec<u8>,
    /// The manifest exactly as signed.
    pub manifest: String,
    pub signature: Option<String>,
    pub version: String,
}

/// One source of the tracker, as it is translated: its own field names onto the tracker's, and
/// per tracker property what its words mean.
struct Translation {
    renames: BTreeMap<String, String>,
    words: BTreeMap<String, BTreeMap<String, String>>,
}

impl Translation {
    fn of(decl: &TrackerDecl, member: &str) -> Translation {
        let mut renames = BTreeMap::new();
        let mut words = BTreeMap::new();
        for (property, align) in &decl.normalise {
            let own = align.field_in(member, property);
            if own != *property {
                renames.insert(own, property.clone());
            }
            if let Some(map) = align.members.get(member) {
                words.insert(property.clone(), map.clone());
            }
        }
        Translation {
            renames,
            words,
        }
    }
    fn name(&self, field: &str) -> String {
        self.renames.get(field).cloned().unwrap_or_else(|| field.to_string())
    }
    /// What the source said, in the tracker's word, as `Align::means` reads it.
    fn word(&self, property: &str, raw: &str) -> String {
        self.words
            .get(property)
            .and_then(|m| m.get(&raw.to_lowercase()).or_else(|| m.get(raw)))
            .cloned()
            .unwrap_or_else(|| raw.to_string())
    }
    /// A claim's `fields`, renamed and in the tracker's words.
    fn fields(&self, raw: &str) -> String {
        let Ok(J::Object(map)) = serde_json::from_str::<J>(raw) else {
            return raw.to_string();
        };
        let out: serde_json::Map<String, J> = map
            .into_iter()
            .map(|(k, v)| {
                let k = self.name(&k);
                let v = self.value(&k, v);
                (k, v)
            })
            .collect();
        J::Object(out).to_string()
    }
    fn value(&self, property: &str, v: J) -> J {
        match v {
            J::Array(a) => J::Array(a.into_iter().map(|x| self.value(property, x)).collect()),
            J::Object(mut o) => {
                for key in ["code", "text"] {
                    if let Some(J::String(s)) = o.get(key) {
                        let s = self.word(property, s);
                        o.insert(key.into(), J::String(s));
                    }
                }
                J::Object(o)
            }
            J::String(s) => J::String(self.word(property, &s)),
            other => other,
        }
    }
}

fn sql(e: rusqlite::Error) -> String {
    e.to_string()
}

/// A copy of a store, made with SQLite's own copy so a store being written is read whole.
fn copy_of(db: &Path, into: &Path) -> Result<Connection, String> {
    let _ = std::fs::remove_file(into);
    let from = Connection::open(db).map_err(|e| format!("{}: {e}", db.display()))?;
    from.execute("vacuum into ?1", params![into.to_string_lossy()])
        .map_err(|e| format!("{}: {e}", db.display()))?;
    drop(from);
    let c = Connection::open(into).map_err(sql)?;
    // What is removed below is overwritten, not only unlinked from the tree.
    c.execute_batch("pragma journal_mode=delete; pragma secure_delete=on;").map_err(sql)?;
    Ok(c)
}

/// A source's claims, sealed: in the tracker's names and words, and nothing of where they were
/// read but a hash per receipt.
fn seal_claims(c: &Connection, t: &Translation, links: bool, row_text: bool) -> Result<(), String> {
    let tx = c.unchecked_transaction().map_err(sql)?;
    {
        let rows: Vec<(i64, String)> = {
            let mut q = tx.prepare("select rowid, fields from record").map_err(sql)?;
            let r = q
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(sql)?
                .collect::<Result<_, _>>()
                .map_err(sql)?;
            r
        };
        for (rowid, fields) in rows {
            tx.execute("update record set fields = ?1 where rowid = ?2", params![t.fields(&fields), rowid])
                .map_err(sql)?;
        }
        tx.execute("update record set origin = ?1, attachments = '[]'", params![NO_ORIGIN]).map_err(sql)?;
        if !links {
            tx.execute("update record set url = null", []).map_err(sql)?;
        }
        if row_text {
            tx.execute_batch(
                "update record set text = title;
                 drop table fts;
                 create virtual table fts using fts5(record_id unindexed, title, text);
                 insert into fts(rowid, record_id, title, text) select rowid, record_id, title, text from record;",
            )
            .map_err(sql)?;
        }

        let revisions: Vec<(String, i64, String)> = {
            let mut q = tx.prepare("select record_id, run, fields from revision").map_err(sql)?;
            let r = q
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map_err(sql)?
                .collect::<Result<_, _>>()
                .map_err(sql)?;
            r
        };
        for (id, run, fields) in revisions {
            tx.execute(
                "update revision set fields = ?1 where record_id = ?2 and run = ?3",
                params![t.fields(&fields), id, run],
            )
            .map_err(sql)?;
        }

        let values: Vec<(i64, String, Option<String>)> = {
            let mut q = tx.prepare("select rowid, name, s from field").map_err(sql)?;
            let r = q
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map_err(sql)?
                .collect::<Result<_, _>>()
                .map_err(sql)?;
            r
        };
        for (rowid, name, s) in values {
            let name = t.name(&name);
            let s = s.map(|s| t.word(&name, &s));
            tx.execute("update field set name = ?1, s = ?2 where rowid = ?3", params![name, s, rowid])
                .map_err(sql)?;
        }

        // What each run saw, by the tracker's names; what it said went wrong was said about where
        // it read, and stays there.
        let runs: Vec<(i64, Option<String>)> = {
            let mut q = tx.prepare("select id, fields from run").map_err(sql)?;
            let r = q
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(sql)?
                .collect::<Result<_, _>>()
                .map_err(sql)?;
            r
        };
        for (id, fields) in runs {
            let fields = fields.map(|f| f.split(',').map(|n| t.name(n)).collect::<Vec<_>>().join(","));
            tx.execute(
                "update run set fields = ?1, note = null, error = null, refused = null where id = ?2",
                params![fields, id],
            )
            .map_err(sql)?;
        }

        tx.execute("delete from excerpt", []).map_err(sql)?;
        let kept = KEPT_META.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(",");
        tx.execute(&format!("delete from meta where key not in ({kept})"), []).map_err(sql)?;
    }
    tx.commit().map_err(sql)?;
    c.execute_batch("vacuum").map_err(sql)
}

/// The tracker's own store, sealed: every value in the tracker's words, nobody's reading state,
/// and no shape that names a source's own fields.
fn seal_things(c: &Connection, by: &BTreeMap<String, Translation>) -> Result<(), String> {
    let tx = c.unchecked_transaction().map_err(sql)?;
    {
        let said: Vec<(i64, String, String)> = {
            let mut q = tx.prepare("select rowid, source, property from said").map_err(sql)?;
            let r = q
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map_err(sql)?
                .collect::<Result<_, _>>()
                .map_err(sql)?;
            r
        };
        for (rowid, source, property) in said {
            let name = by.get(&source).map(|t| t.name(&property)).unwrap_or(property);
            tx.execute("update said set property = ?1, raw = means where rowid = ?2", params![name, rowid])
                .map_err(sql)?;
        }

        let signals: Vec<(i64, String, Option<String>, Option<String>, Option<String>, Option<String>)> = {
            let mut q = tx
                .prepare("select id, kind, property, source, was, is_now from signal")
                .map_err(sql)?;
            let r = q
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
                .map_err(sql)?
                .collect::<Result<_, _>>()
                .map_err(sql)?;
            r
        };
        for (id, kind, property, source, was, is_now) in signals {
            let t = source.as_ref().and_then(|s| by.get(s));
            let property = match (&property, t) {
                (Some(p), Some(t)) => Some(t.name(p)),
                (p, _) => p.clone(),
            };
            let p = property.clone().unwrap_or_default();
            let words = |raw: Option<String>| -> Option<String> {
                let raw = raw?;
                let Ok(v) = serde_json::from_str::<J>(&raw) else { return Some(raw) };
                let v = match (kind.as_str(), t, v) {
                    // A conflict holds what each source said: each in its own source's words.
                    ("conflict" | "resolved", _, J::Object(o)) => J::Object(
                        o.into_iter()
                            .map(|(s, v)| {
                                let v = match by.get(&s) {
                                    Some(t) => t.value(&p, v),
                                    None => v,
                                };
                                (s, v)
                            })
                            .collect(),
                    ),
                    ("changed", Some(t), v) => t.value(&p, v),
                    // A source read again names the shape it was read as, which is its own fields.
                    ("health", _, J::String(s)) if s.starts_with("read again as") => json!("read again"),
                    (_, _, v) => v,
                };
                Some(v.to_string())
            };
            let (was, is_now) = (words(was), words(is_now));
            tx.execute(
                "update signal set property = ?1, was = ?2, is_now = ?3 where id = ?4",
                params![property, was, is_now, id],
            )
            .map_err(sql)?;
        }

        let conflicts: Vec<(String, String, String)> = {
            let mut q = tx.prepare("select key, property, sources from conflict").map_err(sql)?;
            let r = q
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map_err(sql)?
                .collect::<Result<_, _>>()
                .map_err(sql)?;
            r
        };
        for (key, property, sources) in conflicts {
            let mapped = match serde_json::from_str::<J>(&sources) {
                Ok(J::Object(o)) => J::Object(
                    o.into_iter()
                        .map(|(s, v)| {
                            let v = match by.get(&s) {
                                Some(t) => t.value(&property, v),
                                None => v,
                            };
                            (s, v)
                        })
                        .collect(),
                )
                .to_string(),
                _ => sources,
            };
            tx.execute(
                "update conflict set sources = ?1 where key = ?2 and property = ?3",
                params![mapped, key, property],
            )
            .map_err(sql)?;
        }

        tx.execute("delete from reader", []).map_err(sql)?;
        tx.execute("delete from meta where key = 'shapes'", []).map_err(sql)?;
    }
    tx.commit().map_err(sql)?;
    c.execute_batch("vacuum").map_err(sql)
}

/// The statement a sealed tracker travels as: what it compares and on what scale, and not which
/// column of which source a property was, nor what a source's words were taken to mean.
fn sealed_statement(decl: &TrackerDecl) -> Result<String, String> {
    let mut v = serde_json::to_value(decl).map_err(|e| e.to_string())?;
    if let Some(J::Object(align)) = v.get_mut("align") {
        for (_, a) in align.iter_mut() {
            if let J::Object(o) = a {
                o.retain(|k, _| k == "scale" || k == "tolerance");
            }
        }
    }
    if let Some(J::Array(relations)) = v.get_mut("relations") {
        for r in relations.iter_mut() {
            if let J::Object(o) = r {
                o.remove("suggest_from");
            }
        }
    }
    if let Some(J::Array(members)) = v.get_mut("sources") {
        for m in members.iter_mut() {
            if let J::Object(o) = m {
                o.remove("remote");
                o.remove("key");
            }
        }
    }
    if let J::Object(o) = &mut v {
        o.remove("package");
    }
    let decl: TrackerDecl = serde_json::from_value(v).map_err(|e| format!("the sealed statement: {e}"))?;
    crate::yaml::to_string(&decl)
}

/// A source's declaration as it travels sealed: what it is called, what it says, its terms. Its
/// properties under the tracker's names and with no expression, because nothing is read here.
fn sealed_declaration(ds: &crate::source::Source, t: &Translation, tracker: &str) -> Result<String, String> {
    let d = &ds.decl;
    let mut properties = serde_json::Map::new();
    for f in ds.store.fields(d) {
        let mut spec = serde_json::Map::new();
        spec.insert("type".into(), json!(f.kind));
        if let Some(v) = &f.vocabulary {
            spec.insert("vocabulary".into(), json!(v));
        }
        properties.insert(t.name(&f.name), J::Object(spec));
    }
    let built = json!({
        "name": d.name,
        "title": d.title,
        "kind": d.kind,
        "about": d.about,
        "fetch": { "type": "package", "tracker": tracker, "text_is": d.source.text_is() },
        "claims": { "title": "field:title", "properties": properties },
        "terms": d.terms,
        "licence": d.licence,
    });
    let decl: crate::sourcedecl::SourceDecl =
        serde_json::from_value(built).map_err(|e| format!("{}: the sealed declaration: {e}", d.name))?;
    Ok(format!(
        "# Part of the package {tracker}, sealed: its claims arrived built, in the tracker's words,\n\
         # and how they were read stays with whoever published them. `zetlyn tracker pull` takes a\n\
         # newer version.\n{}",
        crate::yaml::to_string(&decl)?
    ))
}

fn gzip(body: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Write;
    let mut z = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
    z.write_all(body).map_err(|e| e.to_string())?;
    z.finish().map_err(|e| e.to_string())
}

fn gunzip(body: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(body).read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(what: &str) -> Result<Scratch, String> {
        let p = std::env::temp_dir().join(format!("zetlyn-{what}-{}-{}", std::process::id(), crate::now()));
        std::fs::create_dir_all(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        Ok(Scratch(p))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn last(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

/// Pack the tracker at `dir`, whose sources are in `datasets`.
pub fn pack(dir: &Path, datasets: &Path, opts: &Options) -> Result<Packed, String> {
    let tracker = crate::tracker::Tracker::open(dir, datasets)?;
    let decl = &tracker.decl;
    if let Some(p) = &decl.package {
        return Err(format!(
            "{} came as a package, signed by {}. A package is packed by whoever made it",
            decl.name,
            if p.key.is_empty() { "nobody" } else { p.key.as_str() }
        ));
    }
    if !tracker.missing.is_empty() {
        return Err(format!("{}: not every source is here: {}", decl.name, tracker.missing.join(", ")));
    }
    if decl.members.iter().any(|m| m.remote.is_some()) {
        return Err(format!("{}: a source somebody else answers for is not packed with it", decl.name));
    }
    // What the package answers with is the tracker as it stands now.
    tracker.refresh_if_moved()?;

    let scratch = Scratch::new("pack")?;
    let registry = crate::tracker::registry(datasets);
    let tdir = last(&decl.name).to_string();
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut translations = BTreeMap::new();
    let mut sources = Vec::new();

    for m in &decl.members {
        let at = registry.get(&m.dataset).ok_or_else(|| format!("{}: not here", m.dataset))?;
        let ds = crate::source::Source::open(at)?;
        let sdir = at.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| last(&m.dataset).into());
        let t = Translation::of(decl, &m.dataset);
        if opts.sealed {
            // A field the source keeps under the name another of its fields is renamed to would
            // be two things under one name.
            let own: BTreeSet<String> = ds.store.field_names().into_iter().collect();
            for (from, to) in &t.renames {
                if own.contains(to) && !t.renames.contains_key(to) {
                    return Err(format!(
                        "{}: its {from} becomes the tracker's {to}, and it already has a {to} of its own",
                        m.dataset
                    ));
                }
            }
        }
        let copy = scratch.0.join(format!("{sdir}.db"));
        let c = copy_of(&at.join("claims.db"), &copy)?;
        if opts.sealed {
            // A claim with no text of its own was given its row, which is the source in its own words.
            seal_claims(&c, &t, opts.links, ds.decl.records.text.is_empty())?;
        }
        drop(c);
        let statement = if opts.sealed {
            sealed_declaration(&ds, &t, &decl.name)?
        } else {
            std::fs::read_to_string(at.join(crate::sourcedecl::FILE)).map_err(|e| e.to_string())?
        };
        files.insert(format!("sources/{sdir}/{}", crate::sourcedecl::FILE), statement.into_bytes());
        files.insert(format!("sources/{sdir}/claims.db"), std::fs::read(&copy).map_err(|e| e.to_string())?);
        sources.push(json!({ "name": ds.decl.name, "title": ds.decl.title, "claims": ds.store.count() }));
        translations.insert(m.dataset.clone(), t);
    }

    let things = scratch.0.join("tracker.db");
    let c = copy_of(&dir.join("tracker.db"), &things)?;
    if opts.sealed {
        seal_things(&c, &translations)?;
    }
    drop(c);
    files.insert(format!("trackers/{tdir}/tracker.db"), std::fs::read(&things).map_err(|e| e.to_string())?);
    let statement = if opts.sealed {
        sealed_statement(decl)?
    } else {
        std::fs::read_to_string(dir.join(crate::trackerdecl::FILE)).map_err(|e| e.to_string())?
    };
    files.insert(format!("trackers/{tdir}/{}", crate::trackerdecl::FILE), statement.into_bytes());
    // Matches a person confirmed are results, and travel as what they are.
    if let Ok(m) = std::fs::read(dir.join("matches.jsonl")) {
        files.insert(format!("trackers/{tdir}/matches.jsonl"), m);
    }

    let listed: BTreeMap<String, J> = files
        .iter()
        .map(|(p, b)| (p.clone(), json!({ "bytes": b.len(), "sha256": sha256(b) })))
        .collect();
    let lines: String = listed.iter().map(|(p, f)| format!("{p} {}\n", f["sha256"].as_str().unwrap_or(""))).collect();
    let version = sha256(lines.as_bytes())[..24].to_string();
    let withheld: Vec<&str> = if opts.sealed {
        let mut w = vec![
            "where each source is read, and how",
            "each source's own field names and words",
            "which column of which source each property is, and what its words were taken to mean",
            "the receipts: what each source handed over (their hashes travel)",
        ];
        if !opts.links {
            w.push("each claim's link");
        }
        w
    } else {
        Vec::new()
    };
    let manifest = json!({
        "format": FORMAT,
        "built_by": concat!("zetlyn ", env!("CARGO_PKG_VERSION")),
        "signed_by": crate::identity::key_at(dir),
        "tracker": decl.name,
        "title": decl.title,
        "about": decl.about,
        "version": version,
        "built_at": crate::now(),
        "sealed": opts.sealed,
        "sources": sources,
        "withheld": withheld,
        "files": listed,
    });
    let manifest = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    let signature = crate::identity::sign_at(dir, manifest.as_bytes())?;

    let out = scratch.0.join("package.zetlyn");
    let p = Connection::open(&out).map_err(sql)?;
    p.execute_batch(
        "pragma journal_mode=delete;
         create table package(key text primary key, value text not null);
         create table file(path text primary key, bytes blob not null);",
    )
    .map_err(sql)?;
    p.execute("insert into package values('format', ?1)", params![FORMAT]).map_err(sql)?;
    p.execute("insert into package values('manifest', ?1)", params![manifest]).map_err(sql)?;
    if let Some(s) = &signature {
        p.execute("insert into package values('signature', ?1)", params![s]).map_err(sql)?;
    }
    for (path, bytes) in &files {
        p.execute("insert into file values(?1, ?2)", params![path, gzip(bytes)?]).map_err(sql)?;
    }
    drop(p);
    Ok(Packed {
        bytes: std::fs::read(&out).map_err(|e| e.to_string())?,
        manifest,
        signature,
        version,
    })
}

/// A package read and held against itself: every file against the manifest, and the manifest
/// against the key it names, or the one the subscriber pinned.
pub struct Opened {
    pub manifest: J,
    pub raw_manifest: String,
    pub files: BTreeMap<String, Vec<u8>>,
}

pub fn read(bytes: &[u8], pinned: Option<&str>) -> Result<Opened, String> {
    let scratch = Scratch::new("read")?;
    let at = scratch.0.join("package.zetlyn");
    std::fs::write(&at, bytes).map_err(|e| e.to_string())?;
    let c = Connection::open(&at).map_err(|e| format!("not a package: {e}"))?;
    let get = |k: &str| -> Option<String> {
        c.query_row("select value from package where key = ?1", params![k], |r| r.get(0)).ok()
    };
    let format = get("format").ok_or("not a package: it says nothing about what it is")?;
    if format != FORMAT {
        return Err(format!("a {format} package, and this program reads {FORMAT}"));
    }
    let raw_manifest = get("manifest").ok_or("a package with no manifest")?;
    let manifest: J = serde_json::from_str(&raw_manifest).map_err(|e| format!("the manifest: {e}"))?;
    let name = manifest["tracker"].as_str().unwrap_or("the package").to_string();

    let pinned = pinned.map(str::trim).filter(|k| !k.is_empty());
    let claimed = manifest["signed_by"].as_str().unwrap_or_default().trim().to_string();
    let against = pinned.unwrap_or(&claimed);
    if !against.is_empty() {
        let signature = get("signature").ok_or_else(|| {
            if pinned.is_some() {
                format!("{name} is not signed, and you pinned a key for it")
            } else {
                format!("{name} says {against} signed it, and it is not signed")
            }
        })?;
        crate::artifact::verify(against, raw_manifest.as_bytes(), &signature).map_err(|e| format!("{name}: {e}"))?;
    }

    let mut files = BTreeMap::new();
    {
        let mut q = c.prepare("select path, bytes from file").map_err(sql)?;
        let rows = q
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?)))
            .map_err(sql)?;
        for row in rows {
            let (path, packed) = row.map_err(sql)?;
            let body = gunzip(&packed).map_err(|e| format!("{path}: {e}"))?;
            let want = manifest["files"][&path]["sha256"].as_str().unwrap_or_default();
            if want.is_empty() {
                return Err(format!("{name}: {path} is in the package and not in its manifest"));
            }
            if sha256(&body) != want {
                return Err(format!("{name}: {path} is not the file its manifest describes"));
            }
            files.insert(path, body);
        }
    }
    for listed in manifest["files"].as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()).unwrap_or_default() {
        if !files.contains_key(&listed) {
            return Err(format!("{name}: its manifest lists {listed}, and the package does not hold it"));
        }
    }
    for path in files.keys() {
        let safe = path.split('/').all(|s| !s.is_empty() && s != "." && s != "..")
            && (path.starts_with("sources/") || path.starts_with("trackers/"));
        if !safe {
            return Err(format!("{name}: {path} is not somewhere a package may write"));
        }
    }
    Ok(Opened { manifest, raw_manifest, files })
}

/// Write an opened package into the workspace at `root`, as a tracker and its sources. A version
/// of the same package that is there already is replaced; anything else in the way is refused.
pub fn install(opened: &Opened, root: &Path, from: &str, reference: Option<&str>) -> Result<PathBuf, String> {
    let m = &opened.manifest;
    let name = m["tracker"].as_str().unwrap_or_default().to_string();
    let tdir = root.join("trackers").join(last(&name));
    let held: Option<J> = std::fs::read_to_string(tdir.join(HELD)).ok().and_then(|s| serde_json::from_str(&s).ok());
    if tdir.join(crate::trackerdecl::FILE).exists() {
        match &held {
            Some(h) if h["tracker"].as_str() == Some(name.as_str()) => {}
            _ => {
                return Err(format!(
                    "{} is a tracker of this workspace already, and not one that came in {name}",
                    tdir.display()
                ))
            }
        }
    }
    let ours: BTreeSet<String> = held
        .as_ref()
        .and_then(|h| h["files"].as_object())
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    for path in opened.files.keys() {
        let Some(sdir) = path.strip_prefix("sources/").and_then(|p| p.split('/').next()) else { continue };
        let here = root.join("sources").join(sdir);
        let decl_path = format!("sources/{sdir}/{}", crate::sourcedecl::FILE);
        if here.join(crate::sourcedecl::FILE).exists() && !ours.contains(&decl_path) {
            return Err(format!("{} is a source of this workspace already, and not one that came in {name}", here.display()));
        }
    }

    for (path, bytes) in &opened.files {
        let to = root.join(path);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        // A store replaced under its journal would be read as the journal says.
        for suffix in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", to.display()));
        }
        let tmp = to.with_extension("arriving");
        std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &to).map_err(|e| format!("{}: {e}", to.display()))?;
    }

    let mut decl = TrackerDecl::load(&tdir)?;
    decl.package = Some(Packaged {
        from: from.to_string(),
        reference: reference.map(str::to_string),
        version: m["version"].as_str().unwrap_or_default().to_string(),
        sealed: m["sealed"].as_bool().unwrap_or(false),
        key: m["signed_by"].as_str().unwrap_or_default().to_string(),
    });
    let note = if decl.package.as_ref().is_some_and(|p| p.sealed) {
        "# From a sealed package: what it answers with, not how it was made. It is not built here;\n\
         # `zetlyn tracker pull` takes a newer version.\n"
    } else {
        "# From a package. `zetlyn tracker pull` takes a newer version.\n"
    };
    std::fs::write(tdir.join(crate::trackerdecl::FILE), format!("{note}{}", crate::yaml::to_string(&decl)?))
        .map_err(|e| format!("{}: {e}", tdir.display()))?;
    std::fs::write(tdir.join(HELD), &opened.raw_manifest).map_err(|e| format!("{}: {e}", tdir.display()))?;
    Ok(tdir)
}

/// Pack and put on a hub, under `packages/<owner>/<name>`, with the tag moved to it.
pub fn publish(dir: &Path, datasets: &Path, place: &dyn Place, tag: &str, opts: &Options, expect: Option<&str>) -> Result<(String, J), String> {
    let decl = TrackerDecl::load(dir)?;
    let packed = pack(dir, datasets, opts)?;
    let reference = Reference::parse(&format!("{}@{tag}", decl.name))?;
    let v = &packed.version;
    if !place.exists(&reference.version_path(TREE, v, "manifest.json")) {
        place.put(&reference.version_path(TREE, v, "package.zetlyn"), &packed.bytes)?;
        if let Some(s) = &packed.signature {
            place.put(&reference.version_path(TREE, v, "manifest.sig"), s.as_bytes())?;
        }
        // Last, because a manifest that is there says the version is whole.
        place.put(&reference.version_path(TREE, v, "manifest.json"), packed.manifest.as_bytes())?;
    }
    crate::artifact::move_tag(place, &reference.tag_path(TREE), v, expect)?;
    let manifest: J = serde_json::from_str(&packed.manifest).map_err(|e| e.to_string())?;
    Ok((packed.version, manifest))
}

/// Whether a hub holds a package under this reference.
pub fn on_hub(place: &dyn Place, reference: &Reference) -> bool {
    place.exists(&reference.tag_path(TREE))
}

/// Fetch the version a tag names, verified, and install it.
pub fn subscribe(place: &dyn Place, reference: &Reference, root: &Path, location: &str, pinned: Option<&str>) -> Result<(PathBuf, J), String> {
    let manifest = crate::artifact::manifest_signed_by(place, reference, TREE, pinned)?;
    let version = manifest["version"].as_str().unwrap_or_default();
    let bytes = place.get(&reference.version_path(TREE, version, "package.zetlyn"))?;
    let pin = pinned.map(str::to_string).or_else(|| manifest["signed_by"].as_str().map(str::to_string));
    let opened = read(&bytes, pin.as_deref())?;
    if opened.manifest != manifest {
        return Err(format!("{reference} {version}: the package is not the one its manifest on the hub describes"));
    }
    let at = install(&opened, root, location, Some(&reference.to_string()))?;
    Ok((at, opened.manifest))
}

/// What `tracker pull` does: ask where the package came from for a newer version.
pub fn pull(dir: &Path, root: &Path) -> Result<String, String> {
    let decl = TrackerDecl::load(dir)?;
    let p = decl.package.ok_or_else(|| format!("{} did not come as a package", decl.name))?;
    let Some(reference) = &p.reference else {
        return Err(format!(
            "{} came as the file {}; open a newer one with `zetlyn tracker subscribe <file>`",
            decl.name, p.from
        ));
    };
    let r = Reference::parse(reference)?;
    let place = crate::place::at(&p.from)?;
    // Withdrawn by whoever published it: what is held here stays, and nothing newer comes.
    if let Some(since) = crate::artifact::withdrawn(place.as_ref(), TREE, &format!("{}/{}", r.owner, r.name)) {
        return Ok(format!("{r} is no longer published at {} (withdrawn {since}); what you hold stays as it is", p.from));
    }
    let pinned = Some(p.key.as_str()).filter(|k| !k.is_empty());
    let manifest = crate::artifact::manifest_signed_by(place.as_ref(), &r, TREE, pinned)?;
    let offered = manifest["version"].as_str().unwrap_or_default();
    if offered == p.version {
        return Ok(format!("{r} is at {offered}, which is what you hold"));
    }
    let (_, m) = subscribe(place.as_ref(), &r, root, &p.from, pinned)?;
    Ok(format!("{r} {} → {}", p.version, m["version"].as_str().unwrap_or_default()))
}
