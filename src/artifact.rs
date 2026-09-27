//! A dataset, when it travels.
//!
//! What a publisher ships is the records, not the instructions for producing them. A subscriber
//! therefore needs none of the publisher's credentials, is not subject to the source's rate
//! limits, and cannot re-run the source at all: `zetlyn dataset run` on a subscribed dataset
//! checks the hub for a newer version.
//!
//! What does not travel is the publisher's operating history. The revisions, the runs and the
//! watermarks stay with them; the full-text, identifier and field indexes are derived from the
//! records and are rebuilt on arrival. Over the eleven datasets this was measured on, the stores
//! were 398 MB and the records in them 61 MB.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value as J};

use crate::dataset::Dataset;
use crate::place::{sha256, Place};
use crate::record::Record;
use crate::store::Store;

pub const SPEC_VERSION: &str = "1.0";

/// `[host/]owner/name[@tag]`.
#[derive(Debug, Clone)]
pub struct Reference {
    pub host: Option<String>,
    pub owner: String,
    pub name: String,
    pub tag: String,
}

impl Reference {
    pub fn parse(raw: &str) -> Result<Reference, String> {
        let (body, tag) = match raw.rsplit_once('@') {
            Some((b, t)) if !t.is_empty() => (b, t.to_string()),
            _ => (raw, "latest".to_string()),
        };
        if tag.contains('/') || tag.starts_with('.') || tag.len() > 64 {
            return Err(format!("{tag}: not a tag"));
        }
        let parts: Vec<&str> = body.split('/').filter(|s| !s.is_empty()).collect();
        // The first segment is a host when it contains a dot, which is why an owner may not.
        let (host, rest) = match parts.first() {
            Some(first) if first.contains('.') => (Some(first.to_string()), &parts[1..]),
            _ => (None, &parts[..]),
        };
        match rest {
            [owner, name] => Ok(Reference {
                host,
                owner: owner.to_string(),
                name: name.to_string(),
                tag,
            }),
            _ => Err(format!(
                "{raw}: a reference is owner/name, with an optional host"
            )),
        }
    }

    fn under(&self, tree: &str) -> String {
        format!("{tree}/{}/{}", self.owner, self.name)
    }

    pub fn tag_path(&self, tree: &str) -> String {
        format!("{}/tags/{}", self.under(tree), self.tag)
    }

    pub fn version_path(&self, tree: &str, version: &str, file: &str) -> String {
        format!("{}/versions/{version}/{file}", self.under(tree))
    }

    /// A delta sits inside the version it produces, under the version it applies to. The
    /// directory names what a subscriber holds after applying it; `from` names what to apply it
    /// to.
    pub fn delta_path(&self, tree: &str, version: &str, from: &str, file: &str) -> String {
        format!("{}/versions/{version}/from/{from}/{file}", self.under(tree))
    }
}

impl std::fmt::Display for Reference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(h) = &self.host {
            write!(f, "{h}/")?;
        }
        write!(f, "{}/{}@{}", self.owner, self.name, self.tag)
    }
}

/// The content hash of a version: sha256 over the payload names and their hashes, sorted, so two
/// builds of the same records are one version whatever order the files were written in.
pub fn version_of(payloads: &BTreeMap<String, (u64, String)>) -> String {
    let mut listing = String::new();
    for (name, (_, hash)) in payloads {
        listing.push_str(name);
        listing.push('\n');
        listing.push_str(hash);
        listing.push('\n');
    }
    sha256(listing.as_bytes())[..24].to_string()
}

/// What a published dataset says about itself, without fetching its records.
pub fn manifest_of(ds: &Dataset, payloads: &BTreeMap<String, (u64, String)>) -> J {
    let d = &ds.decl;
    let last = ds.store.run_report(ds.store.last_run());
    let (first_known, last_known) = ds.store.known_span();
    let fields: Vec<J> = ds
        .store
        .fields(d)
        .into_iter()
        .map(|f| {
            json!({ "name": f.name, "type": f.kind, "records": f.records,
                    "vocabulary": f.vocabulary })
        })
        .collect();
    let views: Vec<J> = d
        .view
        .iter()
        .map(|v| {
            json!({ "name": v.name, "title": v.title, "default": v.default,
                    "where": v.filter, "group": v.group,
                    "columns": v.columns, "facets": v.facets, "sort": v.sort })
        })
        .collect();
    json!({
        "spec_version": SPEC_VERSION,
        "built_by": concat!("zetlyn ", env!("CARGO_PKG_VERSION")),
        "dataset": d.name,
        "version": version_of(payloads),
        "built_at": crate::now(),
        "kind": d.kind,
        "title": d.title,
        "about": d.about,

        "records": ds.store.count(),
        "identifiers": J::Object(ds.store.distinct_identifiers().into_iter()
            .map(|(k, v)| (k, json!(v))).collect()),
        "fields": fields,
        "known": { "first": first_known, "last": last_known },

        // What the subscriber inherits. A run that reached 578 of 2,684 subjects ships those 578,
        // and a scope naming this dataset is partial for the same reason its publisher's is.
        "complete": last.as_ref().map(|r| r.complete).unwrap_or(false),
        "reached": last.as_ref().and_then(|r| r.error.clone()),
        "finished": last.as_ref().and_then(|r| r.finished.clone()),
        "every": d.schedule.every,

        // Fetching, indexing and republishing are three acts. When bytes travel the publisher
        // performs the third on the subscriber's behalf, and this is where that is visible.
        "source": d.source.address(),
        "text_is": d.source.text_is(),
        "terms": d.terms,

        // The dataset says how it wants to be read, and a subscriber who lost that would hold a
        // worse thing than the publisher does.
        "read": {
            "views": views,
            "search": { "text": d.search.text, "compare": d.search.compare,
                        "suggest": d.search.suggest, "examples": d.search.examples },
        },

        "payloads": J::Object(payloads.iter()
            .map(|(n, (bytes, hash))| (n.clone(), json!({ "bytes": bytes, "sha256": hash })))
            .collect()),
    })
}

/// Write `records.jsonl`, the manifest and the tag. Returns the version.
pub fn publish(
    ds: &Dataset,
    place: &dyn Place,
    tag: &str,
    expect: Option<&str>,
) -> Result<String, String> {
    let mut body = Vec::new();
    ds.store.for_each_record(|r| {
        let line = serde_json::to_string(&r.to_json()).map_err(|e| e.to_string())?;
        body.extend_from_slice(line.as_bytes());
        body.push(b'\n');
        Ok(())
    })?;

    let mut payloads = BTreeMap::new();
    payloads.insert(
        "records.jsonl".to_string(),
        (body.len() as u64, sha256(&body)),
    );
    let manifest = manifest_of(ds, &payloads);
    let version = manifest["version"].as_str().unwrap_or_default().to_string();
    let reference = Reference::parse(&format!("{}@{tag}", ds.decl.name))?;

    // A version directory is written once and never changed, so a republish of the same records
    // moves the tag and writes nothing else.
    let manifest_path = reference.version_path("datasets", &version, "manifest.json");
    if !place.exists(&manifest_path) {
        place.put(
            &reference.version_path("datasets", &version, "records.jsonl"),
            &body,
        )?;
        // The signature is over the manifest exactly as it is served, so the bytes are written
        // once and both the put and the signing use the same ones.
        let served = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
        place.put(&manifest_path, served.as_bytes())?;
        if let Some(signature) = sign(&ds.dir, served.as_bytes())? {
            place.put(
                &reference.version_path("datasets", &version, "manifest.sig"),
                signature.as_bytes(),
            )?;
        }
    }

    // What the tag pointed at before is what most subscribers hold, so that is the one delta
    // worth writing. Where it is missing or unreadable the publication still stands: a delta is
    // a saving and never the only way to the records.
    let held = place
        .get(&reference.tag_path("datasets"))
        .ok()
        .map(|b| String::from_utf8_lossy(&b).trim().to_string())
        .filter(|p| !p.is_empty() && *p != version);
    if let Some(previous) = held {
        if let Err(e) = write_delta(ds, place, &reference, &previous, &version, &manifest) {
            eprintln!("no delta from {previous}: {e}");
        }
    }

    move_tag(place, &reference.tag_path("datasets"), &version, expect)?;
    Ok(version)
}

/// What changed between the version on the hub and the one in this store, written so a subscriber
/// holding the first can reach the second without the whole of it.
///
/// The publisher reads their own last publication back rather than keeping a copy: the hub is
/// what subscribers hold, so it is the thing to compute against.
fn write_delta(
    ds: &Dataset,
    place: &dyn Place,
    reference: &Reference,
    previous: &str,
    version: &str,
    full: &J,
) -> Result<(), String> {
    let before = place.get(&reference.version_path("datasets", previous, "records.jsonl"))?;
    let mut held: BTreeMap<String, String> = BTreeMap::new();
    for line in String::from_utf8_lossy(&before).lines() {
        if line.trim().is_empty() {
            continue;
        }
        let j: J = serde_json::from_str(line).map_err(|e| e.to_string())?;
        let (Some(id), Some(hash)) = (j["record_id"].as_str(), j["hash"].as_str()) else {
            continue;
        };
        held.insert(id.to_string(), hash.to_string());
    }

    let mut body = Vec::new();
    let mut added = 0u64;
    let mut changed = 0u64;
    let mut seen = std::collections::BTreeSet::new();
    ds.store.for_each_record(|r| {
        seen.insert(r.record_id.clone());
        match held.get(&r.record_id) {
            Some(hash) if *hash == r.hash => return Ok(()),
            Some(_) => changed += 1,
            None => added += 1,
        }
        let line = serde_json::to_string(&r.to_json()).map_err(|e| e.to_string())?;
        body.extend_from_slice(line.as_bytes());
        body.push(b'\n');
        Ok(())
    })?;

    let mut gone = Vec::new();
    for id in held.keys() {
        if !seen.contains(id) {
            gone.push(format!("{}\n", serde_json::json!({ "record_id": id })));
        }
    }
    let removed_body = gone.concat().into_bytes();

    // A delta nobody gains from is not written. The whole is one fetch and the delta is two.
    let cost = body.len() + removed_body.len();
    let whole = full["payloads"]["records.jsonl"]["bytes"]
        .as_u64()
        .unwrap_or(u64::MAX) as usize;
    if cost >= whole {
        return Err(format!(
            "{cost} bytes against {whole} for the whole, so the whole is the cheaper fetch"
        ));
    }

    let mut payloads = BTreeMap::new();
    payloads.insert(
        "records.jsonl".to_string(),
        (body.len() as u64, sha256(&body)),
    );
    payloads.insert(
        "removed.jsonl".to_string(),
        (removed_body.len() as u64, sha256(&removed_body)),
    );
    let manifest = serde_json::json!({
        "spec_version": SPEC_VERSION,
        "built_by": concat!("zetlyn ", env!("CARGO_PKG_VERSION")),
        "dataset": full["dataset"],
        "version": version,
        "applies_to": previous,
        "built_at": crate::now(),
        "added": added,
        "changed": changed,
        "removed": gone.len(),
        "records": full["records"],
        "payloads": J::Object(payloads.iter()
            .map(|(n, (bytes, hash))| (n.clone(), serde_json::json!({ "bytes": bytes, "sha256": hash })))
            .collect()),
    });

    place.put(
        &reference.delta_path("datasets", version, previous, "records.jsonl"),
        &body,
    )?;
    place.put(
        &reference.delta_path("datasets", version, previous, "removed.jsonl"),
        &removed_body,
    )?;
    place.put(
        &reference.delta_path("datasets", version, previous, "manifest.json"),
        serde_json::to_string_pretty(&manifest)
            .map_err(|e| e.to_string())?
            .as_bytes(),
    )?;
    Ok(())
}

/// Moving a tag says which version it expects to replace. Two publishers of one dataset pull a
/// tag against each other otherwise, which is not a hypothetical: the second tree did it to
/// itself on the day its hub was set up, a local deployment publishing a newer version and a
/// tenant then writing its older one over the top.
///
/// Where a place cannot compare and write in one step, this is a read and then a write, and the
/// publisher is the lock. It catches the mistake, not a race.
fn move_tag(
    place: &dyn Place,
    path: &str,
    version: &str,
    expect: Option<&str>,
) -> Result<(), String> {
    let held = place
        .get(path)
        .ok()
        .map(|b| String::from_utf8_lossy(&b).trim().to_string());
    match (expect, held.as_deref()) {
        (Some(want), Some(there)) if want != there => {
            return Err(format!(
                "{path} is at {there}, not {want}. Somebody else published since you last looked"
            ))
        }
        (Some(want), None) if want != "-" => {
            return Err(format!("{path} does not exist, so it is not at {want}"))
        }
        _ => {}
    }
    place.put(path, format!("{version}\n").as_bytes())
}

/// What a subscriber reads before deciding to fetch 61 MB.
pub fn manifest_at(place: &dyn Place, reference: &Reference, tree: &str) -> Result<J, String> {
    manifest_signed_by(place, reference, tree, None)
}

/// The manifest, held against the key a subscriber pinned.
///
/// A hash per payload says the bytes are the ones this manifest describes, and whoever serves one
/// serves the other. The key says who wrote the manifest, and it is the only thing here a hub
/// cannot produce. Where nothing is pinned, nothing is checked and the fetch is what it was
/// before.
pub fn manifest_signed_by(
    place: &dyn Place,
    reference: &Reference,
    tree: &str,
    pinned: Option<&str>,
) -> Result<J, String> {
    let tag = place.get(&reference.tag_path(tree))?;
    let version = String::from_utf8_lossy(&tag).trim().to_string();
    if version.is_empty() {
        return Err(format!("{reference}: the tag names no version"));
    }
    let raw = place.get(&reference.version_path(tree, &version, "manifest.json"))?;
    if let Some(key) = pinned.filter(|k| !k.trim().is_empty()) {
        let signature = place
            .get(&reference.version_path(tree, &version, "manifest.sig"))
            .map_err(|_| {
                format!("{reference} {version} is not signed, and you pinned a key for it")
            })?;
        verify(key, &raw, &String::from_utf8_lossy(&signature))
            .map_err(|e| format!("{reference} {version}: {e}"))?;
    }
    let manifest: J = serde_json::from_slice(&raw).map_err(|e| format!("{reference}: {e}"))?;
    Ok(manifest)
}

/// Fetch, verify, and build a dataset directory from what arrived.
pub fn subscribe(
    place: &dyn Place,
    reference: &Reference,
    into: &Path,
    location: &str,
    pinned: Option<&str>,
) -> Result<(u64, String), String> {
    let manifest = manifest_signed_by(place, reference, "datasets", pinned)?;
    let version = manifest["version"].as_str().unwrap_or_default().to_string();
    let spec = manifest["spec_version"].as_str().unwrap_or_default();
    if spec != SPEC_VERSION {
        return Err(format!(
            "{reference}: built against specification {spec}, and this is {SPEC_VERSION}"
        ));
    }

    let declared = manifest["payloads"]["records.jsonl"].clone();
    let body = place.get(&reference.version_path("datasets", &version, "records.jsonl"))?;
    let want = declared["sha256"].as_str().unwrap_or_default();
    let got = sha256(&body);
    if want != got {
        return Err(format!(
            "{reference}: records.jsonl is {got} and the manifest says {want}"
        ));
    }

    std::fs::create_dir_all(into).map_err(|e| format!("{}: {e}", into.display()))?;
    std::fs::write(
        into.join("dataset.toml"),
        declaration(&manifest, location, reference, pinned.unwrap_or(""))?,
    )
    .map_err(|e| format!("{}: {e}", into.display()))?;
    std::fs::write(
        into.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("{}: {e}", into.display()))?;

    // The indexes are derived, so they are built here rather than shipped.
    let store = Store::open(into)?;
    let run = store.begin_run()?;
    let name = manifest["dataset"].as_str().unwrap_or_default();
    let at = crate::iso_stamp(crate::now());
    let mut added = 0u64;
    let mut changed = 0u64;
    let mut unchanged = 0u64;
    let mut fields = std::collections::BTreeSet::new();
    for (n, line) in String::from_utf8_lossy(&body).lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let j: J = serde_json::from_str(line).map_err(|e| format!("line {}: {e}", n + 1))?;
        let record = Record::from_json(name, &j).map_err(|e| format!("line {}: {e}", n + 1))?;
        for f in record.fields.keys() {
            fields.insert(f.clone());
        }
        match store.put(&record, run, &at, false)? {
            "added" => added += 1,
            "changed" => changed += 1,
            _ => unchanged += 1,
        }
    }
    // A subscription replaces the dataset. A record the new version does not carry is a record
    // the publisher removed, and the sweep is what says so.
    let removed = store.sweep(run)?;
    let held = added + changed + unchanged;
    store.finish_run(
        run,
        manifest["complete"].as_bool().unwrap_or(false),
        added,
        changed,
        removed,
        unchanged,
        &fields,
        &crate::build::Notes::default(),
        manifest["reached"].as_str(),
    )?;
    store.set_meta("subscribed", &format!("{location} {reference} {version}"))?;
    Ok((held, version))
}

/// The declaration a subscribed dataset carries. It holds no source expressions, because nothing
/// is extracted here: the records arrived built. What it does carry is what the dataset says
/// about how to read it, which is the publisher's and travels with them.
fn declaration(
    manifest: &J,
    location: &str,
    reference: &Reference,
    key: &str,
) -> Result<String, String> {
    let s = |k: &str| manifest[k].as_str().unwrap_or_default();
    let mut out = String::new();
    out.push_str(&format!("name  = {}\n", quoted(s("dataset"))));
    out.push_str(&format!("title = {}\n", quoted(s("title"))));
    out.push_str(&format!("kind  = {}\n", quoted(s("kind"))));
    out.push_str(&format!("about = {}\n\n", quoted(s("about"))));
    out.push_str("# Subscribed, not fetched. `zetlyn dataset run` on this asks the hub whether\n");
    out.push_str("# there is a newer version and applies it; the source belongs to whoever\n");
    out.push_str("# published these records.\n");
    out.push_str("[source]\n");
    out.push_str("type = \"hub\"\n");
    out.push_str(&format!("at   = {}\n", quoted(location)));
    out.push_str(&format!("ref  = {}\n", quoted(&reference.to_string())));
    if !key.trim().is_empty() {
        out.push_str(&format!("key  = {}\n", quoted(key.trim())));
    }
    out.push('\n');

    out.push_str("[records]\n");
    out.push_str(&format!("title = {}\n", quoted("field:title")));
    if let Some(fields) = manifest["fields"].as_array() {
        if !fields.is_empty() {
            out.push_str("\n[records.fields]\n");
            for f in fields {
                let name = f["name"].as_str().unwrap_or_default();
                let kind = f["type"].as_str().unwrap_or("text");
                match f["vocabulary"].as_str() {
                    Some(v) => out.push_str(&format!(
                        "{name} = {{ type = {}, vocabulary = {} }}\n",
                        quoted(kind),
                        quoted(v)
                    )),
                    None => out.push_str(&format!("{name} = {{ type = {} }}\n", quoted(kind))),
                }
            }
        }
    }
    if let Some(views) = manifest["read"]["views"].as_array() {
        for v in views {
            out.push_str("\n[[view]]\n");
            out.push_str(&format!(
                "name    = {}\n",
                quoted(v["name"].as_str().unwrap_or(""))
            ));
            if let Some(t) = v["title"].as_str().filter(|t| !t.is_empty()) {
                out.push_str(&format!("title   = {}\n", quoted(t)));
            }
            if v["default"].as_bool().unwrap_or(false) {
                out.push_str("default = true\n");
            }
            if let Some(w) = v["where"].as_str() {
                out.push_str(&format!("where   = {}\n", quoted(w)));
            }
            if let Some(g) = v["group"].as_str() {
                out.push_str(&format!("group   = {}\n", quoted(g)));
            }
            out.push_str(&format!("columns = {}\n", list(&v["columns"])));
            out.push_str(&format!("facets  = {}\n", list(&v["facets"])));
            if let Some(sort) = v["sort"].as_str() {
                out.push_str(&format!("sort    = {}\n", quoted(sort)));
            }
        }
    }
    let search = &manifest["read"]["search"];
    out.push_str("\n[search]\n");
    for key in ["text", "compare", "suggest", "examples"] {
        out.push_str(&format!("{key:<8} = {}\n", list(&search[key])));
    }
    Ok(out)
}

fn quoted(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ");
    format!("\"{escaped}\"")
}

fn list(j: &J) -> String {
    let items: Vec<String> = j
        .as_array()
        .map(|a| a.iter().filter_map(J::as_str).map(quoted).collect())
        .unwrap_or_default();
    format!("[{}]", items.join(", "))
}

// ---------------------------------------------------------------------------------------------
// A scope, which has no payload.

/// A scope holds no index and has no records, so what it publishes is the statement and the
/// statement is the whole of it. Its version is the content hash of that statement, which makes
/// two publications of the same composition one version.
pub fn publish_scope(
    dir: &Path,
    datasets: &Path,
    place: &dyn Place,
    tag: &str,
    expect: Option<&str>,
) -> Result<String, String> {
    let path = dir.join("scope.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let decl = crate::scopedecl::ScopeDecl::load(dir)?;
    let version = sha256(text.as_bytes())[..24].to_string();

    // Which version of each member the curator last checked this against. Information and not a
    // pin: a subscriber assembling it from newer members gets newer members, which is what
    // following a tag is for.
    let mut checked = BTreeMap::new();
    for m in &decl.members {
        if !m.dataset.is_empty() {
            let name = &m.dataset;
            let held = datasets.join(name.rsplit('/').next().unwrap_or(name));
            if let Ok(raw) = std::fs::read_to_string(held.join("manifest.json")) {
                if let Ok(j) = serde_json::from_str::<J>(&raw) {
                    if let Some(v) = j["version"].as_str() {
                        checked.insert(name.clone(), json!(v));
                    }
                }
            }
        }
    }

    let manifest = json!({
        "spec_version": SPEC_VERSION,
        "built_by": concat!("zetlyn ", env!("CARGO_PKG_VERSION")),
        "scope": decl.name,
        "version": version,
        "built_at": crate::now(),
        "title": decl.title,
        "about": decl.about,
        "members": J::Array(decl.members.iter()
            .map(|m| json!(if m.dataset.is_empty() { m.remote.clone().unwrap_or_default() } else { m.dataset.clone() }))
            .collect()),
        "join": J::Array(decl.keys().iter().map(|k| json!(k)).collect()),
        "promise": {
            "fresh_within": decl.promise.fresh_within,
            "covers": decl.promise.covers,
            "excludes": decl.promise.excludes,
        },
        "checked_against": J::Object(checked.into_iter().collect()),
        "declaration": text,
    });

    let reference = Reference::parse(&format!("{}@{tag}", decl.name))?;
    let manifest_path = reference.version_path("scopes", &version, "manifest.json");
    if !place.exists(&manifest_path) {
        place.put(
            &manifest_path,
            serde_json::to_string_pretty(&manifest)
                .map_err(|e| e.to_string())?
                .as_bytes(),
        )?;
    }
    move_tag(place, &reference.tag_path("scopes"), &version, expect)?;
    Ok(version)
}

/// The statement, and then each member it names that is not held already.
pub fn subscribe_scope(
    place: &dyn Place,
    reference: &Reference,
    into: &Path,
    datasets: &Path,
    location: &str,
) -> Result<(String, Vec<String>), String> {
    let manifest = manifest_at(place, reference, "scopes")?;
    let spec = manifest["spec_version"].as_str().unwrap_or_default();
    if spec != SPEC_VERSION {
        return Err(format!(
            "{reference}: built against specification {spec}, and this is {SPEC_VERSION}"
        ));
    }
    let text = manifest["declaration"]
        .as_str()
        .ok_or_else(|| format!("{reference}: the manifest carries no declaration"))?;
    std::fs::create_dir_all(into).map_err(|e| format!("{}: {e}", into.display()))?;
    std::fs::write(into.join("scope.toml"), text)
        .map_err(|e| format!("{}: {e}", into.display()))?;
    std::fs::write(
        into.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("{}: {e}", into.display()))?;

    let mut taken = Vec::new();
    for member in manifest["members"].as_array().unwrap_or(&Vec::new()) {
        let Some(name) = member.as_str().filter(|n| !n.starts_with("http")) else {
            continue;
        };
        let here = datasets.join(name.rsplit('/').next().unwrap_or(name));
        if here.join("dataset.toml").exists() {
            continue;
        }
        let member_ref = Reference::parse(name)?;
        subscribe(place, &member_ref, &here, location, None)?;
        taken.push(name.to_string());
    }
    Ok((
        manifest["version"].as_str().unwrap_or_default().to_string(),
        taken,
    ))
}

/// What a subscriber holds, read from the manifest they kept when they last fetched.
pub fn held_version(dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(dir.join("manifest.json")).ok()?;
    let j: J = serde_json::from_str(&raw).ok()?;
    j["version"].as_str().map(str::to_string)
}

/// Apply the delta from what is held to what the hub offers. `Ok(None)` where there is no delta
/// to apply, which is not a failure: the caller takes the whole instead.
pub fn apply_delta(
    place: &dyn Place,
    reference: &Reference,
    into: &Path,
    from: &str,
    to: &str,
    full: &J,
) -> Result<Option<(u64, u64, u64)>, String> {
    let path = reference.delta_path("datasets", to, from, "manifest.json");
    let Ok(raw) = place.get(&path) else {
        return Ok(None);
    };
    let manifest: J = serde_json::from_slice(&raw).map_err(|e| format!("{path}: {e}"))?;
    if manifest["applies_to"].as_str() != Some(from) || manifest["version"].as_str() != Some(to) {
        return Err(format!(
            "{path}: it does not say it goes from {from} to {to}"
        ));
    }

    let mut fetched = Vec::new();
    for name in ["records.jsonl", "removed.jsonl"] {
        let bytes = place.get(&reference.delta_path("datasets", to, from, name))?;
        let want = manifest["payloads"][name]["sha256"]
            .as_str()
            .unwrap_or_default();
        let got = sha256(&bytes);
        if want != got {
            return Err(format!("{name} is {got} and the manifest says {want}"));
        }
        fetched.push(bytes);
    }

    let store = Store::open(into)?;
    let run = store.begin_run()?;
    let at = crate::iso_stamp(crate::now());
    let name = full["dataset"].as_str().unwrap_or_default();
    let mut added = 0u64;
    let mut changed = 0u64;
    let mut fields = std::collections::BTreeSet::new();
    for (n, line) in String::from_utf8_lossy(&fetched[0]).lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let j: J = serde_json::from_str(line).map_err(|e| format!("line {}: {e}", n + 1))?;
        let record = Record::from_json(name, &j).map_err(|e| format!("line {}: {e}", n + 1))?;
        for f in record.fields.keys() {
            fields.insert(f.clone());
        }
        match store.put(&record, run, &at, false)? {
            "added" => added += 1,
            _ => changed += 1,
        }
    }
    let mut removed = 0u64;
    for line in String::from_utf8_lossy(&fetched[1]).lines() {
        if line.trim().is_empty() {
            continue;
        }
        let j: J = serde_json::from_str(line).map_err(|e| e.to_string())?;
        if let Some(id) = j["record_id"].as_str() {
            if store.remove(id, run, &at)? {
                removed += 1;
            }
        }
    }

    // No sweep. A delta says what left, and a record it did not mention is a record that stayed.
    let unchanged = store.count().saturating_sub(added + changed);
    store.finish_run(
        run,
        full["complete"].as_bool().unwrap_or(false),
        added,
        changed,
        removed,
        unchanged,
        &fields,
        &crate::build::Notes::default(),
        full["reached"].as_str(),
    )?;
    // The declaration too, and not only the records. A publisher may have added a field, changed
    // a view or replaced a search example between the two versions, and a subscriber who took the
    // records and kept the old declaration would hold a dataset that fails its own check.
    let (location, pinned) = match &crate::decl::Declaration::load(into)?.source {
        crate::decl::Source::Hub { at, key, .. } => (at.clone(), key.clone()),
        _ => (String::new(), String::new()),
    };
    std::fs::write(
        into.join("dataset.toml"),
        declaration(full, &location, reference, &pinned)?,
    )
    .map_err(|e| format!("{}: {e}", into.display()))?;
    std::fs::write(
        into.join("manifest.json"),
        serde_json::to_string_pretty(full).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("{}: {e}", into.display()))?;

    // The count the publisher declared is what the store must now hold. A delta that leaves it
    // somewhere else has been applied to something other than what it was computed against.
    let want = full["records"].as_u64().unwrap_or_default();
    let got = store.count();
    if want != got {
        return Err(format!(
            "after the delta this holds {got} records and the manifest says {want}"
        ));
    }
    Ok(Some((added, changed, removed)))
}

// ---------------------------------------------------------------------------------------------
// Who published this.

/// A hash per payload catches a fetch that went wrong. It does not catch a hub that served
/// something else on purpose, because whoever serves the payload serves the manifest that
/// describes it. A signature over the manifest is what separates the two, and it is only worth
/// anything to a subscriber who knows whose signature to expect.
pub const KEY_FILE: &str = "publishing.key";

fn key_bytes(raw: &str) -> Result<[u8; 32], String> {
    let hex = raw.trim().trim_start_matches("ed25519:");
    if hex.len() != 64 {
        return Err("a key is 32 bytes, written as 64 hex characters".into());
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|_| "a key is hexadecimal".to_string())?;
    }
    Ok(out)
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A new pair. The private half is written where the publisher keeps it and nowhere else; the
/// public half is what they tell subscribers, and it is not a secret.
pub fn new_key(dir: &Path) -> Result<String, String> {
    let path = dir.join(KEY_FILE);
    if path.exists() {
        return Err(format!(
            "{} exists. A second key makes every subscriber who pinned the first stop trusting you",
            path.display()
        ));
    }
    let seed = key_bytes(&crate::account::token())?;
    let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::write(&path, format!("ed25519:{}\n", hex_of(&seed)))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    // Readable by nobody else, on the systems that can say so.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(format!(
        "ed25519:{}",
        hex_of(signing.verifying_key().as_bytes())
    ))
}

/// The public half of the key in a directory, which is what a publisher hands out.
pub fn public_key(dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(dir.join(KEY_FILE)).ok()?;
    let seed = key_bytes(&raw).ok()?;
    let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
    Some(format!(
        "ed25519:{}",
        hex_of(signing.verifying_key().as_bytes())
    ))
}

fn sign(dir: &Path, message: &[u8]) -> Result<Option<String>, String> {
    let Ok(raw) = std::fs::read_to_string(dir.join(KEY_FILE)) else {
        return Ok(None);
    };
    use ed25519_dalek::Signer;
    let signing = ed25519_dalek::SigningKey::from_bytes(&key_bytes(&raw)?);
    Ok(Some(format!(
        "ed25519:{}",
        hex_of(&signing.sign(message).to_bytes())
    )))
}

/// Held against the key the subscriber pinned, over the manifest exactly as it was served.
pub fn verify(pinned: &str, manifest: &[u8], signature: &str) -> Result<(), String> {
    use ed25519_dalek::Verifier;
    let key = ed25519_dalek::VerifyingKey::from_bytes(&key_bytes(pinned)?)
        .map_err(|e| format!("the pinned key is not one: {e}"))?;
    let raw = signature.trim().trim_start_matches("ed25519:");
    if raw.len() != 128 {
        return Err("a signature is 64 bytes, written as 128 hex characters".into());
    }
    let mut bytes = [0u8; 64];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&raw[i * 2..i * 2 + 2], 16)
            .map_err(|_| "a signature is hexadecimal".to_string())?;
    }
    key.verify(manifest, &ed25519_dalek::Signature::from_bytes(&bytes))
        .map_err(|_| "the signature is not this publisher's, over these bytes".to_string())
}
