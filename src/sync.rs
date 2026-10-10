//! One world in two places, kept together without a lock: `zetlyn world sync <address>`.
//!
//! A world on one's own machine and the same world on zetlyn.com are copies; either may be worked
//! on at any time. A sync brings them together in three parts, each the way it can be:
//!
//! - **Definitions** (each source's and tracker's files, and what the world says of itself) by
//!   three-way merge against what both last agreed on, kept on this side in
//!   `.zetlyn/sync/<instance>/base/`. Changes to different files, or different keys of one YAML
//!   file, merge by themselves; the same key changed both ways is a conflict, decided here, by
//!   a person (`--take ours|theirs`, or asked). The hosted side merges nothing itself: it takes a
//!   push only where nothing changed there since it was last asked, and says which files did.
//! - **Proposals and decisions**, which are only ever added to: the union of both, and what follows
//!   from the decisions (corrections, rows) worked out again.
//! - **Data**: each source's observations, merged as `store::merge_from` says: every observation
//!   kept in the order it was made, the latest current. Nothing to decide.
//!
//! Each copy is an instance with an id of its own (`.zetlyn/instance`), which is what the other
//! keeps its bookkeeping under. What is not part of a world's definition stays where it is: its
//! address, its mailer, its assist, who may do what, its readers' accounts, its keys.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{json, Value as J};

/// The keys of workspace.yaml that travel: what the world is and who may do what in it, not where
/// or how it runs. An owner a machine names in its members.yaml stays one there whatever `access:` says.
const WORLD_KEYS: [&str; 5] = ["title", "contact", "profile", "update", "access"];
/// Kept beside each source, merged as unions, never three ways.
const UNIONS: [&str; 2] = [crate::propose::DIR, crate::propose::DECISIONS];

/// This copy's id: made the first time it is asked for, and made anew by an import, so two copies
/// of one world are two instances.
pub fn instance(root: &Path) -> String {
    let path = root.join(".zetlyn").join("instance");
    if let Ok(id) = std::fs::read_to_string(&path) {
        if id.trim().len() == 16 {
            return id.trim().to_string();
        }
    }
    let id = crate::jwt::random().chars().filter(|c| c.is_ascii_alphanumeric()).take(16).collect::<String>().to_lowercase();
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(root));
    let _ = std::fs::write(&path, &id);
    id
}

fn sha(bytes: &[u8]) -> String {
    crate::place::sha256(bytes)[..32].to_string()
}

/// Whether a file under a source or a tracker is part of its definition: not a database, not
/// what is worked out from others, not hidden, not what a sync merges as a union.
fn defining(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let parts: Vec<&str> = rel.split('/').collect();
    !(name.starts_with('.')
        || name.ends_with(".db")
        || name.ends_with(".db-shm")
        || name.ends_with(".db-wal")
        || name.ends_with(".partial")
        || name.ends_with(".arriving")
        || parts.iter().any(|p| matches!(*p, "inbox" | "corrected" | "added" | "proposals" | "decisions.jsonl" | "cache" | "pages")))
}

fn walk(dir: &Path, base: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, base, out);
        } else if p.is_file() {
            out.push(p.strip_prefix(base).map(Path::to_path_buf).unwrap_or(p));
        }
    }
}

/// What the world says of itself, as it travels: its own keys of workspace.yaml.
fn world_part(root: &Path) -> Vec<u8> {
    let text = std::fs::read_to_string(root.join(crate::account::WORKSPACE)).unwrap_or_default();
    let all: J = crate::yaml::parse(&text).unwrap_or(J::Null);
    let mut kept = serde_json::Map::new();
    for k in WORLD_KEYS {
        if let Some(v) = all.get(k).filter(|v| !v.is_null()) {
            kept.insert(k.to_string(), v.clone());
        }
    }
    crate::yaml::to_string(&J::Object(kept)).unwrap_or_default().into_bytes()
}

/// Every definition of a world, by its path in it, with its bytes.
pub fn definitions(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    out.insert(crate::account::WORKSPACE.to_string(), world_part(root));
    for top in ["sources", "trackers"] {
        let mut files = Vec::new();
        walk(&root.join(top), root, &mut files);
        for rel in files {
            let r = rel.to_string_lossy().replace('\\', "/");
            if defining(&r) {
                if let Ok(b) = std::fs::read(root.join(&rel)) {
                    out.insert(r, b);
                }
            }
        }
    }
    out
}

/// Every proposal and every decision file, by path: what is merged as a union.
pub fn proposals(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    for (_, dir) in crate::tracker::registry(&root.join("sources")) {
        for name in UNIONS {
            let p = dir.join(name);
            let mut files = Vec::new();
            if p.is_dir() {
                walk(&p, root, &mut files);
            } else if p.is_file() {
                files.push(p.strip_prefix(root).map(Path::to_path_buf).unwrap_or(p.clone()));
            }
            for rel in files {
                if let Ok(b) = std::fs::read(root.join(&rel)) {
                    out.insert(rel.to_string_lossy().replace('\\', "/"), b);
                }
            }
        }
    }
    out
}

/// A path a sync may write: inside the world, under its sources or trackers, or its own file.
fn safe(rel: &str) -> bool {
    let p = Path::new(rel);
    (rel == crate::account::WORKSPACE || rel.starts_with("sources/") || rel.starts_with("trackers/"))
        && p.components().all(|c| matches!(c, std::path::Component::Normal(_)))
}

/// One definition written into the world, or taken out where it is gone: workspace.yaml as its
/// own keys only, everything else as it is.
fn write(root: &Path, rel: &str, bytes: Option<&[u8]>) -> Result<(), String> {
    if !safe(rel) {
        return Err(format!("{rel}: not a path a sync writes"));
    }
    let path = root.join(rel);
    if rel == crate::account::WORKSPACE {
        let given: J = crate::yaml::parse(&String::from_utf8_lossy(bytes.unwrap_or_default())).unwrap_or(J::Null);
        for k in WORLD_KEYS {
            let v = given.get(k).cloned().filter(|v| !v.is_null());
            match (k, v) {
                ("title" | "contact", Some(J::String(s))) => crate::world::set_top(&path, k, Some(&s))?,
                ("title" | "contact", _) => crate::world::set_top(&path, k, None)?,
                (_, Some(v)) => crate::app::set_block(&path, k, &v)?,
                (_, None) => crate::app::set_block(&path, k, &J::Object(Default::default()))?,
            }
        }
        return Ok(());
    }
    match bytes {
        Some(b) => {
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            let partial = path.with_extension("syncing.partial");
            std::fs::write(&partial, b).map_err(|e| e.to_string())?;
            std::fs::rename(&partial, &path).map_err(|e| e.to_string())
        }
        None => {
            let _ = std::fs::remove_file(&path);
            // A source or tracker whose declaration went goes, with what it held.
            let mut parts = rel.split('/');
            if let (Some(top), Some(name), Some(file)) = (parts.next(), parts.next(), parts.next()) {
                if (file == crate::sourcedecl::FILE || file == crate::trackerdecl::FILE) && matches!(top, "sources" | "trackers") {
                    let _ = std::fs::remove_dir_all(root.join(top).join(name));
                }
            }
            Ok(())
        }
    }
}

/// The union of what both copies hold of proposals and decisions, written here; then what the
/// decisions mean (accepted corrections and rows) worked out again. How many files came in.
pub fn take_proposals(root: &Path, theirs: &BTreeMap<String, Vec<u8>>) -> Result<usize, String> {
    let mut n = 0;
    let mut touched: BTreeSet<PathBuf> = BTreeSet::new();
    for (rel, bytes) in theirs {
        if !safe(rel) || !rel.starts_with("sources/") {
            continue;
        }
        let path = root.join(rel);
        if rel.ends_with(crate::propose::DECISIONS) {
            // Lines, as a union, in the order they were decided.
            let mine = std::fs::read_to_string(&path).unwrap_or_default();
            let mut lines: BTreeSet<(String, String)> = BTreeSet::new();
            for l in mine.lines().chain(String::from_utf8_lossy(bytes).lines()).filter(|l| !l.trim().is_empty()) {
                let at = serde_json::from_str::<J>(l).ok().and_then(|j| j["at"].as_str().map(str::to_string)).unwrap_or_default();
                lines.insert((at, l.to_string()));
            }
            let text: String = lines.into_iter().map(|(_, l)| format!("{l}\n")).collect();
            if text != mine {
                std::fs::write(&path, text).map_err(|e| e.to_string())?;
                n += 1;
            }
        } else if !path.exists() {
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
            n += 1;
        } else {
            continue;
        }
        if let Some(source) = rel.split('/').nth(1) {
            touched.insert(root.join("sources").join(source));
        }
    }
    for dir in touched {
        crate::propose::rebuild(&dir)?;
    }
    Ok(n)
}

// -- three ways ---------------------------------------------------------------------------------

/// What came of merging one file: the bytes to keep (None: gone), and the keys both sides
/// changed differently, each with ours and theirs.
pub struct Merge {
    pub result: Option<Vec<u8>>,
    pub conflicts: Vec<(String, String, String)>,
}

/// One file, three ways: what both last agreed on, ours, theirs. A side that did not change gives
/// way; a YAML file both changed is merged key by key; anything else both changed is a conflict.
pub fn merge3(rel: &str, base: Option<&[u8]>, ours: Option<&[u8]>, theirs: Option<&[u8]>) -> Merge {
    let done = |r: Option<&[u8]>| Merge { result: r.map(<[u8]>::to_vec), conflicts: Vec::new() };
    if ours == theirs || theirs == base {
        return done(ours);
    }
    if ours == base {
        return done(theirs);
    }
    let yaml = rel.ends_with(".yaml") || rel.ends_with(".yml");
    let parse = |b: Option<&[u8]>| -> Option<J> { b.and_then(|b| crate::yaml::parse::<J>(&String::from_utf8_lossy(b)).ok()) };
    if yaml {
        if let (Some(o), Some(t)) = (parse(ours), parse(theirs)) {
            let b = parse(base).unwrap_or(J::Null);
            let mut conflicts = Vec::new();
            let merged = merge_value("", &b, &o, &t, &mut conflicts);
            let bytes = crate::yaml::to_string(&merged).ok().map(String::into_bytes);
            return Merge { result: bytes.or_else(|| ours.map(<[u8]>::to_vec)), conflicts };
        }
    }
    let show = |b: Option<&[u8]>| b.map(|b| String::from_utf8_lossy(b).chars().take(200).collect::<String>()).unwrap_or_else(|| "(gone)".into());
    Merge { result: ours.map(<[u8]>::to_vec), conflicts: vec![(String::new(), show(ours), show(theirs))] }
}

/// Two changes to one value, against what both started from: a map key by key, anything else
/// whole. Where both changed it differently, ours stands and the key is said.
fn merge_value(path: &str, base: &J, ours: &J, theirs: &J, conflicts: &mut Vec<(String, String, String)>) -> J {
    if ours == theirs || theirs == base {
        return ours.clone();
    }
    if ours == base {
        return theirs.clone();
    }
    if let (Some(o), Some(t)) = (ours.as_object(), theirs.as_object()) {
        let empty = serde_json::Map::new();
        let b = base.as_object().unwrap_or(&empty);
        let keys: BTreeSet<&String> = o.keys().chain(t.keys()).chain(b.keys()).collect();
        let mut out = serde_json::Map::new();
        for k in keys {
            let (bv, ov, tv) = (b.get(k).cloned().unwrap_or(J::Null), o.get(k).cloned().unwrap_or(J::Null), t.get(k).cloned().unwrap_or(J::Null));
            let v = merge_value(&if path.is_empty() { k.clone() } else { format!("{path}.{k}") }, &bv, &ov, &tv, conflicts);
            if !v.is_null() {
                out.insert(k.clone(), v);
            }
        }
        return J::Object(out);
    }
    conflicts.push((path.to_string(), ours.to_string(), theirs.to_string()));
    ours.clone()
}

/// A value set at a dotted key of a YAML file's bytes: how a conflict decided for theirs is kept.
fn set_at(bytes: &[u8], key: &str, value: &J) -> Vec<u8> {
    let mut doc: J = crate::yaml::parse(&String::from_utf8_lossy(bytes)).unwrap_or(J::Null);
    let mut at = &mut doc;
    let parts: Vec<&str> = key.split('.').collect();
    for (i, p) in parts.iter().enumerate() {
        if !at.is_object() {
            *at = json!({});
        }
        let map = at.as_object_mut().expect("an object");
        if i + 1 == parts.len() {
            if value.is_null() {
                map.remove(*p);
            } else {
                map.insert(p.to_string(), value.clone());
            }
            break;
        }
        at = map.entry(p.to_string()).or_insert_with(|| json!({}));
    }
    crate::yaml::to_string(&doc).map(String::into_bytes).unwrap_or_else(|_| bytes.to_vec())
}

// -- the hosted side ----------------------------------------------------------------------------

/// What this copy holds, for another to compare with: its instance, each definition's hash, and
/// each source's highest run.
pub fn state(root: &Path) -> J {
    let files: serde_json::Map<String, J> = definitions(root).into_iter().map(|(k, v)| (k, J::String(sha(&v)))).collect();
    let sources: serde_json::Map<String, J> = crate::tracker::registry(&root.join("sources"))
        .into_iter()
        .filter_map(|(_, d)| {
            let slug = d.file_name()?.to_string_lossy().into_owned();
            let ds = crate::source::Source::open(&d).ok()?;
            Some((slug, json!(ds.store.last_run())))
        })
        .collect();
    // Its clock, for the other side to see how far apart the two are (USECASES K4).
    // `ZETLYN_CLOCK_SKEW` (seconds) says it off by that much, and nothing else: how a test makes a
    // machine whose clock is wrong.
    let skew: i64 = std::env::var("ZETLYN_CLOCK_SKEW").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    json!({ "instance": instance(root), "files": files, "sources": sources, "now": crate::now() + skew })
}

/// A set of files as a tar.gz: `defs/…` and `props/…`, and a manifest.
fn pack(files: &[(&str, &BTreeMap<String, Vec<u8>>)], manifest: &J, data: &[(String, PathBuf)]) -> Result<Vec<u8>, String> {
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default()));
    let mut add = |name: &str, bytes: &[u8]| -> Result<(), String> {
        let mut h = tar::Header::new_gnu();
        h.set_size(bytes.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        tar.append_data(&mut h, name, bytes).map_err(|e| e.to_string())
    };
    add("manifest.json", manifest.to_string().as_bytes())?;
    for (prefix, set) in files {
        for (rel, bytes) in set.iter() {
            add(&format!("{prefix}/{rel}"), bytes)?;
        }
    }
    for (slug, dir) in data {
        let b = std::fs::read(dir.join("claims.db")).map_err(|e| e.to_string())?;
        add(&format!("data/{slug}.db"), &b)?;
    }
    let gz = tar.into_inner().map_err(|e| e.to_string())?;
    gz.finish().map_err(|e| e.to_string())
}

/// A pack, read: its manifest, its definitions, its proposals, and its data written to `work`.
type Unpacked = (J, BTreeMap<String, Vec<u8>>, BTreeMap<String, Vec<u8>>, Vec<(String, PathBuf)>);
fn unpack(body: &mut dyn Read, work: &Path) -> Result<Unpacked, String> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(body));
    let (mut manifest, mut defs, mut props, mut data) = (J::Null, BTreeMap::new(), BTreeMap::new(), Vec::new());
    for e in tar.entries().map_err(|e| e.to_string())? {
        let mut e = e.map_err(|e| e.to_string())?;
        if !e.header().entry_type().is_file() {
            continue;
        }
        let name = e.path().map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/");
        let mut bytes = Vec::new();
        e.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if name == "manifest.json" {
            manifest = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        } else if let Some(rel) = name.strip_prefix("defs/") {
            defs.insert(rel.to_string(), bytes);
        } else if let Some(rel) = name.strip_prefix("props/") {
            props.insert(rel.to_string(), bytes);
        } else if let Some(slug) = name.strip_prefix("data/").and_then(|n| n.strip_suffix(".db")) {
            if !crate::cell::name_ok(slug) && !slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                continue;
            }
            let dir = work.join(slug);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            std::fs::write(dir.join("claims.db"), &bytes).map_err(|e| e.to_string())?;
            data.push((slug.to_string(), dir));
        }
    }
    Ok((manifest, defs, props, data))
}

/// Everything this copy defines and every proposal, packed: what another pulls.
pub fn serve_pull(root: &Path) -> Result<Vec<u8>, String> {
    let (defs, props) = (definitions(root), proposals(root));
    pack(&[("defs", &defs), ("props", &props)], &json!({ "instance": instance(root) }), &[])
}

/// One source's observations after a run, leaving out what came from `peer`: what another pulls.
pub fn serve_data(root: &Path, slug: &str, after: i64, peer: &str) -> Result<Vec<u8>, String> {
    if slug.contains(['/', '\\', '.']) {
        return Err("not a source".into());
    }
    let ds = crate::source::Source::open(&root.join("sources").join(slug))?;
    let work = std::env::temp_dir().join(format!("zetlyn-sync-out-{}", crate::jwt::random()));
    let r = (|| {
        ds.store.delta(after, peer, &work)?;
        std::fs::read(work.join("claims.db")).map_err(|e| e.to_string())
    })();
    let _ = std::fs::remove_dir_all(&work);
    r
}

/// A push, taken: where any definition it changes was changed here too since the pusher last
/// pulled, nothing is taken and the files are named (409); otherwise its definitions are written,
/// its proposals joined, its data merged, a copy of what it replaced kept in
/// `.zetlyn/sync/replaced/`. What came of it.
pub fn take_push(root: &Path, body: &mut dyn Read) -> Result<J, (u16, J)> {
    let work = std::env::temp_dir().join(format!("zetlyn-sync-in-{}", crate::jwt::random()));
    let fail = |e: String| (400u16, json!({ "error": e }));
    let result = (|| {
        let (manifest, defs, props, data) = unpack(body, &work).map_err(fail)?;
        let peer = manifest["instance"].as_str().unwrap_or("").to_string();
        if peer.len() != 16 || !peer.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(fail("the push names no instance".into()));
        }
        let here = definitions(root);
        let base = manifest["base"].as_object().cloned().unwrap_or_default();
        let gone: BTreeSet<String> = manifest["gone"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect();
        // Every file the push changes, checked against what it was when the pusher pulled.
        let mut moved = Vec::new();
        for rel in defs.keys().chain(gone.iter()) {
            let was = base.get(rel).and_then(|v| v.as_str()).unwrap_or("");
            let now = here.get(rel).map(|b| sha(b)).unwrap_or_default();
            if was != now {
                moved.push(rel.clone());
            }
        }
        if !moved.is_empty() {
            return Err((409, json!({ "error": "changed here since you pulled; sync again", "files": moved })));
        }
        let keep = root.join(".zetlyn").join("sync").join("replaced").join(crate::iso_stamp(crate::now()).replace(':', ""));
        for rel in defs.keys().chain(gone.iter()) {
            if let Some(old) = here.get(rel) {
                let p = keep.join(rel);
                let _ = std::fs::create_dir_all(p.parent().unwrap_or(&keep));
                let _ = std::fs::write(p, old);
            }
        }
        for (rel, bytes) in &defs {
            write(root, rel, Some(bytes)).map_err(fail)?;
        }
        for rel in &gone {
            write(root, rel, None).map_err(fail)?;
        }
        let proposals = take_proposals(root, &props).map_err(fail)?;
        let mut merged = serde_json::Map::new();
        for (slug, dir) in &data {
            let sdir = root.join("sources").join(slug);
            let Ok(ds) = crate::source::Source::open(&sdir) else { continue };
            let m = ds.store.merge_from(dir, &peer, ds.decl.retention.history).map_err(fail)?;
            merged.insert(slug.clone(), json!({ "current": m.current, "history": m.history, "removed": m.removed, "runs": m.runs }));
        }
        Ok(json!({ "definitions": defs.len(), "gone": gone.len(), "proposals": proposals, "data": merged, "state": state(root) }))
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

// -- this side ----------------------------------------------------------------------------------

/// Where this copy keeps what it last agreed on with another.
fn base_dir(root: &Path, peer: &str) -> PathBuf {
    root.join(".zetlyn").join("sync").join(peer)
}

fn read_base(root: &Path, peer: &str) -> BTreeMap<String, Vec<u8>> {
    let dir = base_dir(root, peer).join("base");
    let mut files = Vec::new();
    walk(&dir, &dir, &mut files);
    files.into_iter().filter_map(|rel| Some((rel.to_string_lossy().replace('\\', "/"), std::fs::read(dir.join(&rel)).ok()?))).collect()
}

fn write_base(root: &Path, peer: &str, files: &BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    let dir = base_dir(root, peer).join("base");
    let _ = std::fs::remove_dir_all(&dir);
    for (rel, bytes) in files {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap_or(&dir)).map_err(|e| e.to_string())?;
        std::fs::write(p, bytes).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The hosted world's address and the key it is asked with.
struct Remote {
    url: String,
    key: String,
    agent: ureq::Agent,
}

impl Remote {
    fn get(&self, path: &str) -> Result<Vec<u8>, String> {
        let mut r = self
            .agent
            .get(&format!("{}{path}", self.url))
            .header("Authorization", &format!("Bearer {}", self.key))
            .call()
            .map_err(|e| format!("{}{path}: {e}", self.url))?;
        let code = r.status().as_u16();
        let mut body = Vec::new();
        r.body_mut().as_reader().read_to_end(&mut body).map_err(|e| e.to_string())?;
        if code != 200 {
            return Err(format!("{}{path}: {code} {}", self.url, String::from_utf8_lossy(&body)));
        }
        Ok(body)
    }
    fn state(&self) -> Result<J, String> {
        serde_json::from_slice(&self.get("/sync/state")?).map_err(|e| e.to_string())
    }
}

/// `zetlyn world sync <address> [<workspace>] [--key zk_…] [--take ours|theirs] [--no-data]`.
pub fn command(args: &[String]) -> Result<(), String> {
    let rest = crate::positional(args, 2);
    let url = rest.first().ok_or("which world? zetlyn world sync https://zetlyn.com/<name> [<workspace>]")?.trim_end_matches('/').to_string();
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err(format!("{url}: the world's address, https://…"));
    }
    let root = PathBuf::from(rest.get(1).map(|s| s.as_str()).unwrap_or("."));
    let take = crate::flag(args, "--take").map(str::to_string);
    if take.as_deref().is_some_and(|t| t != "ours" && t != "theirs") {
        return Err("--take ours or --take theirs".into());
    }
    let with_data = !args.iter().any(|a| a == "--no-data");
    let keyfile = |root: &Path| root.join(".zetlyn").join("sync").join("key");
    let key = crate::flag(args, "--key")
        .map(str::to_string)
        .or_else(|| std::env::var("ZETLYN_SYNC_KEY").ok())
        .or_else(|| std::fs::read_to_string(keyfile(&root)).ok().map(|k| k.trim().to_string()))
        .filter(|k| k.starts_with("zk_"))
        .ok_or("a key: in the world's Settings, Moving, Make a sync key; then --key zk_…")?;
    let said = run(&root, &url, &key, take.as_deref(), with_data, true)?;
    println!("{said}");
    Ok(())
}

/// Where how the last sync went is kept, for the page and for the next scheduled one.
fn last_file(root: &Path) -> PathBuf {
    root.join(".zetlyn").join("sync").join("last.json")
}

/// A small file written whole or not at all: beside it first, then in its place, so whoever reads
/// it meanwhile reads the one before rather than half of this one.
pub(crate) fn write_whole(file: &Path, value: &J) {
    let beside = file.with_extension("writing");
    if std::fs::write(&beside, value.to_string()).is_ok() {
        let _ = std::fs::rename(&beside, file);
    }
}

/// How the last sync went, or the one under way: `state` running, ok or failed.
pub fn last(root: &Path) -> Option<J> {
    std::fs::read(last_file(root)).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

/// A sync asked of the world at `url`, from start to end, said in `last.json` as it goes: what the
/// page shows, and when a scheduled one ran last. Where both sides changed one thing and `take`
/// does not decide, nothing is synced and the state names each.
pub fn run_recorded(root: &Path, url: &str, key: &str, take: Option<&str>) -> J {
    let file = last_file(root);
    if let Some(d) = file.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let started = crate::now();
    write_whole(&file, &json!({ "state": "running", "at": crate::iso_stamp(started), "t": started, "url": url }));
    let done = match run(root, url, key, take, true, false) {
        Ok(said) => json!({ "state": "ok", "at": crate::iso_stamp(crate::now()), "t": started, "url": url, "said": said }),
        Err(e) => json!({ "state": "failed", "at": crate::iso_stamp(crate::now()), "t": started, "url": url, "conflicts": e.starts_with("not synced: decide"), "error": e }),
    };
    write_whole(&file, &done);
    done
}

/// Clocks this far apart (seconds) are said at a sync, and this far apart stop it.
pub const CLOCKS_SAID: i64 = 120;
pub const CLOCKS_REFUSED: i64 = 3600;

/// How often this world syncs by itself with the one it last synced with: `.zetlyn/sync/every`,
/// `15m`, `1h`, `6h` or `1d`; none, only when asked.
pub const RHYTHMS: [(&str, &str); 4] = [("15m", "Every 15 minutes"), ("1h", "Every hour"), ("6h", "Every 6 hours"), ("1d", "Once a day")];

pub fn every(root: &Path) -> Option<String> {
    std::fs::read_to_string(root.join(".zetlyn").join("sync").join("every")).ok().map(|s| s.trim().to_string()).filter(|s| RHYTHMS.iter().any(|(r, _)| r == s))
}

pub fn set_every(root: &Path, every: Option<&str>) -> Result<(), String> {
    let file = root.join(".zetlyn").join("sync").join("every");
    match every.filter(|e| RHYTHMS.iter().any(|(r, _)| r == e)) {
        Some(e) => {
            std::fs::create_dir_all(file.parent().unwrap_or(root)).map_err(|e| e.to_string())?;
            std::fs::write(&file, e).map_err(|e| e.to_string())
        }
        None => {
            let _ = std::fs::remove_file(&file);
            Ok(())
        }
    }
}

/// The sync this world does by itself, where one is set and due: done now, with the world it last
/// synced with and the key kept from then. When the next is due, where one is set.
pub fn scheduled(root: &Path) -> Option<i64> {
    let every = crate::fetch::duration(&every(root)?)?;
    let (url, has_key) = last_peer(root)?;
    let key = std::fs::read_to_string(root.join(".zetlyn").join("sync").join("key")).ok().map(|k| k.trim().to_string()).filter(|k| has_key && k.starts_with("zk_"))?;
    let last = last(root);
    // One asked from the page meanwhile is that one's to finish.
    if last.as_ref().is_some_and(|l| l["state"] == "running" && l["t"].as_i64().is_some_and(|t| crate::now() - t < 3600)) {
        return Some(crate::now() + 60);
    }
    let due = last.as_ref().and_then(|l| l["t"].as_i64()).map_or(0, |t| t + every);
    if due > crate::now() {
        return Some(due);
    }
    let done = run_recorded(root, &url, &key, None);
    match done["state"].as_str() {
        Some("ok") => println!("sync: {}", done["said"].as_str().unwrap_or("")),
        _ => eprintln!("sync with {url}: {}", done["error"].as_str().unwrap_or("")),
    }
    Some(crate::now() + every)
}

/// The address and key this workspace last synced with, kept after the first sync.
pub fn last_peer(root: &Path) -> Option<(String, bool)> {
    let key = root.join(".zetlyn").join("sync").join("key");
    let dir = root.join(".zetlyn").join("sync");
    let address = std::fs::read_dir(&dir).ok()?.flatten().filter_map(|e| std::fs::read_to_string(e.path().join("address")).ok()).next()?;
    Some((address.trim().to_string(), key.exists()))
}

/// A sync of the workspace at `root` with the world at `url`: what came and went, in a sentence.
/// Where both changed one thing, `take` decides; without it, a terminal is asked where
/// `interactive`, and elsewhere nothing is synced and the error names each.
pub fn run(root: &Path, url: &str, key: &str, take: Option<&str>, with_data: bool, interactive: bool) -> Result<String, String> {
    let (root, url, key, take) = (root.to_path_buf(), url.trim_end_matches('/').to_string(), key.to_string(), take.map(str::to_string));
    let mut log: Vec<String> = Vec::new();
    let agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(600))).http_status_as_error(false).user_agent(crate::sourcedecl::AGENT).build().new_agent();
    let remote = Remote { url: url.clone(), key: key.clone(), agent };
    let asked = crate::now();
    let theirs_state = remote.state()?;
    // How far apart the two clocks are, the time the asking took halved: within the same second
    // what each side saw decides which observation is current, beyond it the clocks do, so a clock
    // far off is said, and one very far off stops the sync until it is right.
    if let Some(theirs) = theirs_state["now"].as_i64() {
        let here = (asked + crate::now()) / 2;
        let apart = (theirs - here).abs();
        if apart > CLOCKS_REFUSED {
            return Err(format!("the clocks here and at {url} are {} apart: set the one that is wrong (NTP), then sync again. Nothing was synced", crate::web::duration(apart as f64)));
        }
        if apart > CLOCKS_SAID {
            log.push(format!("The clocks here and at {url} are {} apart; which observation is current goes by them, so set the one that is wrong.", crate::web::duration(apart as f64)));
        }
    }
    let peer = theirs_state["instance"].as_str().filter(|p| p.len() == 16).ok_or("the world did not say who it is")?.to_string();

    // A first sync into an empty directory: the whole world, as it is there.
    if !root.join(crate::account::WORKSPACE).exists() {
        let archive = remote.get("/sync/export")?;
        let file = std::env::temp_dir().join(format!("zetlyn-sync-{}.tar.gz", crate::jwt::random()));
        std::fs::write(&file, archive).map_err(|e| e.to_string())?;
        let r = crate::world::import(&file, &root, None, None);
        let _ = std::fs::remove_file(&file);
        r?;
        crate::world::set_top(&root.join(crate::account::WORKSPACE), "url", None)?;
        let _ = std::fs::remove_file(root.join(".zetlyn").join("instance"));
        // What both hold now is what both agree on: every run there is had here.
        write_base(&root, &peer, &definitions(&root))?;
        for (_, dir) in crate::tracker::registry(&root.join("sources")) {
            if let Ok(ds) = crate::source::Source::open(&dir) {
                let last = ds.store.last_run().to_string();
                ds.store.set_meta(&format!("sync.{peer}.run"), &last)?;
                ds.store.set_meta(&format!("sync.{peer}.sent"), &last)?;
                ds.store.set_meta(&format!("sync.{peer}.base"), &last)?;
            }
        }
        remember(&root, &peer, &url, &key)?;
        log.push(format!("Taken: {url} is here, in {}. Work on it, then sync again to bring both together.", root.display()));
        return Ok(log.join("\n"));
    }
    let me = instance(&root);
    for attempt in 1..=3 {
        // 1. Theirs, merged into ours.
        let pulled = remote.get("/sync/pull")?;
        let work = std::env::temp_dir().join(format!("zetlyn-sync-{}", crate::jwt::random()));
        let (_, their_defs, their_props, _) = unpack(&mut pulled.as_slice(), &work)?;
        let base = read_base(&root, &peer);
        let ours = definitions(&root);
        let mut agreed: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut unresolved: Vec<String> = Vec::new();
        let paths: BTreeSet<&String> = base.keys().chain(ours.keys()).chain(their_defs.keys()).collect();
        for rel in paths {
            let m = merge3(rel, base.get(rel).map(Vec::as_slice), ours.get(rel).map(Vec::as_slice), their_defs.get(rel).map(Vec::as_slice));
            let mut result = m.result;
            for (key, o, t) in &m.conflicts {
                let choice = match take.as_deref() {
                    Some(c) => c.to_string(),
                    None if !interactive => String::new(),
                    None => ask(&format!("{rel}{}{key}\n  ours:   {o}\n  theirs: {t}\nKeep [o]urs or [t]heirs? ", if key.is_empty() { "" } else { ": " })),
                };
                if choice.starts_with('t') {
                    result = if key.is_empty() {
                        their_defs.get(rel).cloned()
                    } else {
                        let tv: J = serde_json::from_str(t).unwrap_or(J::Null);
                        result.map(|r| set_at(&r, key, &tv))
                    };
                } else if !choice.starts_with('o') {
                    unresolved.push(format!("{rel} {key}"));
                }
            }
            if result.as_ref() != ours.get(rel) {
                write(&root, rel, result.as_deref())?;
            }
            if let Some(r) = result {
                agreed.insert(rel.clone(), r);
            }
        }
        if !unresolved.is_empty() {
            let _ = std::fs::remove_dir_all(&work);
            return Err(format!("not synced: decide these, with --take ours or --take theirs, or run it in a terminal:\n  {}", unresolved.join("\n  ")));
        }
        let came = take_proposals(&root, &their_props)?;
        // 2. Their observations, merged into ours.
        let mut data_in = 0u64;
        if with_data {
            for (slug, their_last) in theirs_state["sources"].as_object().into_iter().flatten() {
                let dir = root.join("sources").join(slug);
                let Ok(ds) = crate::source::Source::open(&dir) else { continue };
                let done: i64 = ds.store.meta(&format!("sync.{peer}.run")).and_then(|v| v.parse().ok()).unwrap_or(0);
                if their_last.as_i64().unwrap_or(0) <= done {
                    continue;
                }
                let bytes = remote.get(&format!("/sync/data?source={slug}&after={done}&peer={me}"))?;
                let d = work.join(format!("in-{slug}"));
                std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
                std::fs::write(d.join("claims.db"), bytes).map_err(|e| e.to_string())?;
                let m = ds.store.merge_from(&d, &peer, ds.decl.retention.history)?;
                data_in += m.current + m.history + m.removed;
            }
        }
        // 3. Ours, pushed: what changed here since we last agreed, our proposals, our observations.
        let now = definitions(&root);
        let changed: BTreeMap<String, Vec<u8>> = now.iter().filter(|(k, v)| their_defs.get(*k) != Some(*v)).map(|(k, v)| (k.clone(), v.clone())).collect();
        let gone: Vec<String> = their_defs.keys().filter(|k| !now.contains_key(*k)).cloned().collect();
        let base_hashes: serde_json::Map<String, J> = their_defs.iter().map(|(k, v)| (k.clone(), J::String(sha(v)))).collect();
        let mut deltas: Vec<(String, PathBuf)> = Vec::new();
        let mut sent_to: Vec<(PathBuf, i64)> = Vec::new();
        if with_data {
            for (_, dir) in crate::tracker::registry(&root.join("sources")) {
                let Ok(ds) = crate::source::Source::open(&dir) else { continue };
                let slug = dir.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                let sent: i64 = ds.store.meta(&format!("sync.{peer}.sent")).and_then(|v| v.parse().ok()).unwrap_or(0);
                if ds.store.last_run() <= sent {
                    continue;
                }
                let d = work.join(format!("out-{slug}"));
                let last = ds.store.delta(sent, &peer, &d)?;
                deltas.push((slug, d));
                sent_to.push((dir.clone(), last));
            }
        }
        let props = proposals(&root);
        let manifest = json!({ "instance": me, "base": base_hashes, "gone": gone });
        let body = pack(&[("defs", &changed), ("props", &props)], &manifest, &deltas)?;
        let mut answer = remote
            .agent
            .post(&format!("{url}/sync/push"))
            .header("Authorization", &format!("Bearer {key}"))
            .header("Content-Type", "application/gzip")
            .send(&body[..])
            .map_err(|e| format!("{url}/sync/push: {e}"))?;
        let code = answer.status().as_u16();
        let said: J = answer.body_mut().read_json().unwrap_or(J::Null);
        let _ = std::fs::remove_dir_all(&work);
        match code {
            200 => {
                for (dir, last) in sent_to {
                    if let Ok(ds) = crate::source::Source::open(&dir) {
                        ds.store.set_meta(&format!("sync.{peer}.sent"), &last.to_string())?;
                    }
                }
                write_base(&root, &peer, &definitions(&root))?;
                remember(&root, &peer, &url, &key)?;
                log.push(format!(
                    "Synced with {url}: {} definitions there from here, {} gone, {} proposals here from there, {} observations here from there, {} sources' observations there from here.",
                    changed.len(),
                    gone.len(),
                    came,
                    data_in,
                    said["data"].as_object().map_or(0, |d| d.len())
                ));
                return Ok(log.join("\n"));
            }
            409 if attempt < 3 => {
                log.push(format!("Changed there meanwhile ({}); synced again.", said["files"]));
                continue;
            }
            _ => return Err(format!("{url} did not take it: {code} {}", said["error"].as_str().unwrap_or(&said.to_string()))),
        }
    }
    Err("changed there three times while syncing; try again".into())
}

/// The key a sync was made with, readable by its owner alone, and the address it was made with:
/// the next sync needs neither said again.
fn remember(root: &Path, peer: &str, url: &str, key: &str) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let file = root.join(".zetlyn").join("sync").join("key");
    std::fs::create_dir_all(file.parent().unwrap_or(root)).map_err(|e| e.to_string())?;
    std::fs::write(&file, key).map_err(|e| e.to_string())?;
    let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
    let dir = base_dir(root, peer);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("address"), url).map_err(|e| e.to_string())
}

/// A question in the terminal, its answer in lower case; nothing where there is no terminal.
fn ask(question: &str) -> String {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return String::new();
    }
    print!("{question}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    line.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_both_changed_merges_key_by_key_and_says_what_it_cannot() {
        let base = b"title: Prices\nschedule:\n  every: 1h\nclaims:\n  title: field:name\n";
        let ours = b"title: Prices\nschedule:\n  every: 15m\nclaims:\n  title: field:name\n";
        let theirs = b"title: Car prices\nschedule:\n  every: 1h\nclaims:\n  title: field:name\n";
        let m = merge3("sources/p/source.yaml", Some(base), Some(ours), Some(theirs));
        assert!(m.conflicts.is_empty());
        let r: J = crate::yaml::parse(&String::from_utf8_lossy(&m.result.unwrap())).unwrap();
        assert_eq!((r["title"].as_str(), r["schedule"]["every"].as_str()), (Some("Car prices"), Some("15m")), "each side's change kept");
        let theirs2 = b"title: Prices\nschedule:\n  every: 6h\nclaims:\n  title: field:name\n";
        let m = merge3("sources/p/source.yaml", Some(base), Some(ours), Some(theirs2));
        assert_eq!(m.conflicts.len(), 1);
        assert_eq!(m.conflicts[0].0, "schedule.every");
        let kept = set_at(&m.result.unwrap(), "schedule.every", &J::String("6h".into()));
        let r: J = crate::yaml::parse(&String::from_utf8_lossy(&kept)).unwrap();
        assert_eq!(r["schedule"]["every"], "6h", "theirs, where theirs was chosen");
        // One side unchanged gives way, a file gone on one side only is gone.
        assert_eq!(merge3("a.csv", Some(b"1"), Some(b"1"), Some(b"2")).result.as_deref(), Some(&b"2"[..]));
        assert_eq!(merge3("a.csv", Some(b"1"), None, Some(b"1")).result, None);
        assert_eq!(merge3("a.csv", Some(b"1"), Some(b"2"), Some(b"3")).conflicts.len(), 1);
    }

    #[test]
    fn what_a_world_is_travels_and_what_runs_it_stays() {
        assert!(defining("sources/p/source.yaml") && defining("sources/p/a.csv") && defining("trackers/t/tracker.yaml"));
        assert!(!defining("sources/p/claims.db") && !defining("sources/p/proposals/x.json") && !defining("sources/p/corrected/x.json") && !defining("trackers/t/tracker.db") && !defining("sources/p/.cache"));
        assert!(safe("sources/p/source.yaml") && safe("workspace.yaml") && !safe("../x") && !safe("accounts.db") && !safe("sources/../../etc/passwd"));
    }
}
