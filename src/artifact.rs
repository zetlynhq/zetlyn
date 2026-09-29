//! A source, when it travels.
//!
//! What a publisher ships is the claims, not the instructions for producing them. A subscriber
//! therefore needs none of the publisher's credentials, is not thing to the source's rate
//! limits, and cannot re-run the source at all: `zetlyn source run` on a subscribed source
//! checks the hub for a newer version.
//!
//! What does not travel is the publisher's operating history. The revisions, the runs and the
//! watermarks stay with them; the full-text, identifier and field indexes are derived from the
//! claims and are rebuilt on arrival. Over the eleven sources this was measured on, the stores
//! were 398 MB and the claims in them 61 MB.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value as J};

use crate::source::Source;
use crate::place::{sha256, Place};
use crate::claim::Claim;
use crate::store::Store;

pub const SPEC_VERSION: &str = "2.0";

/// Where a reference with no host is fetched from and published to.
///
/// A reference names a host or it does not, and one that does not means this one. Naming it in a
/// flag as well would be saying the same thing twice, so `--from` and `--to` are for the other
/// cases: a folder, a mount, a bucket, or somebody else's hub.
pub const DEFAULT_HUB: &str = "https://hub.zetlyn.com";

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
/// builds of the same claims are one version whatever order the files were written in.
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

/// What a published source says about itself, without fetching its claims.
pub fn manifest_of(ds: &Source, payloads: &BTreeMap<String, (u64, String)>) -> J {
    let d = &ds.decl;
    let last = ds.store.run_report(ds.store.last_run());
    let (first_known, last_known) = ds.store.known_span();
    let fields: Vec<J> = ds
        .store
        .fields(d)
        .into_iter()
        .map(|f| {
            json!({ "name": f.name, "type": f.kind, "claims": f.records,
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
        // Whose signature to expect beside this. A subscriber who pinned nothing pins this on the
        // first fetch, so a key that changes under them afterwards is caught; on that first
        // fetch there is nothing to catch it with, which is what pinning by hand is for.
        "signed_by": crate::identity::or_local(&ds.dir, KEY_FILE),
        "source": d.name,
        "version": version_of(payloads),
        "built_at": crate::now(),
        "kind": d.kind,
        "title": d.title,
        "about": d.about,

        "claims": ds.store.count(),
        "identifiers": J::Object(ds.store.distinct_identifiers().into_iter()
            .map(|(k, v)| (k, json!(v))).collect()),
        "properties": fields,
        "known": { "first": first_known, "last": last_known },

        // What the subscriber inherits. A run that reached 578 of 2,684 things ships those 578,
        // and a tracker naming this source is partial for the same reason its publisher's is.
        "complete": last.as_ref().map(|r| r.complete).unwrap_or(false),
        "reached": last.as_ref().and_then(|r| r.error.clone()),
        "finished": last.as_ref().and_then(|r| r.finished.clone()),
        "every": d.schedule.every,

        // Fetching, indexing and republishing are three acts. When bytes travel the publisher
        // performs the third on the subscriber's behalf, and this is where that is visible.
        "fetched_from": d.source.address(),
        "text_is": d.source.text_is(),
        "terms": d.terms,

        // The source says how it wants to be read, and a subscriber who lost that would hold a
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


/// Whether a manifest already on a hub says anything different from the one just built.
///
/// `built_at` is taken off both sides: it comes from a clock, so it differs on every run and a
/// publication that changed nothing would rewrite the manifest and its signature for ever.
fn differs(held: &[u8], built: &J) -> bool {
    let Ok(mut old) = serde_json::from_slice::<J>(held) else {
        return true;
    };
    let mut new = built.clone();
    for side in [&mut old, &mut new] {
        if let Some(o) = side.as_object_mut() {
            o.remove("built_at");
        }
    }
    old != new
}

/// Write `claims.jsonl`, the manifest and the tag. Returns the version.
pub fn publish(
    ds: &Source,
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
        "claims.jsonl".to_string(),
        (body.len() as u64, sha256(&body)),
    );
    let manifest = manifest_of(ds, &payloads);
    let version = manifest["version"].as_str().unwrap_or_default().to_string();
    let reference = Reference::parse(&format!("{}@{tag}", ds.decl.name))?;

    // A version's payloads are written once and never changed: the version is their hash, so
    // anything that would change them is a different version. The manifest describing them can be
    // corrected — a title the publisher fixed is not a different set of claims, and the payload
    // hashes inside it are the same either way.
    let manifest_path = reference.version_path("sources", &version, "manifest.json");
    // The signature is over the manifest exactly as it is served, so the bytes are made once and
    // both the put and the signing use the same ones.
    let served = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    let held = place.get(&manifest_path).ok();
    if held.is_none() {
        place.put(
            &reference.version_path("sources", &version, "claims.jsonl"),
            &body,
        )?;
    }
    if held.as_deref().map(|b| differs(b, &manifest)).unwrap_or(true) {
        place.put(&manifest_path, served.as_bytes())?;
        // The source's own key where it has one, because subscribers pinned that and a key that
        // changes under them is a publisher they stop trusting. Your identity otherwise, which is
        // what a source made today signs with.
        let signature = match crate::key::sign(&ds.dir, KEY_FILE, served.as_bytes())? {
            Some(s) => Some(s),
            None => crate::identity::sign(served.as_bytes())?,
        };
        if let Some(signature) = signature {
            place.put(
                &reference.version_path("sources", &version, "manifest.sig"),
                signature.as_bytes(),
            )?;
        }
    }

    // What the tag pointed at before is what most subscribers hold, so that is the one delta
    // worth writing. Where it is missing or unreadable the publication still stands: a delta is
    // a saving and never the only way to the claims.
    let held = place
        .get(&reference.tag_path("sources"))
        .ok()
        .map(|b| String::from_utf8_lossy(&b).trim().to_string())
        .filter(|p| !p.is_empty() && *p != version);
    if let Some(previous) = held {
        if let Err(e) = write_delta(ds, place, &reference, &previous, &version, &manifest) {
            eprintln!("no delta from {previous}: {e}");
        }
    }

    move_tag(place, &reference.tag_path("sources"), &version, expect)?;
    Ok(version)
}

/// What changed between the version on the hub and the one in this store, written so a subscriber
/// holding the first can reach the second without the whole of it.
///
/// The publisher reads their own last publication back rather than keeping a copy: the hub is
/// what subscribers hold, so it is the thing to compute against.
fn write_delta(
    ds: &Source,
    place: &dyn Place,
    reference: &Reference,
    previous: &str,
    version: &str,
    full: &J,
) -> Result<(), String> {
    let before = place.get(&reference.version_path("sources", previous, "claims.jsonl"))?;
    let mut held: BTreeMap<String, String> = BTreeMap::new();
    for line in String::from_utf8_lossy(&before).lines() {
        if line.trim().is_empty() {
            continue;
        }
        let j: J = serde_json::from_str(line).map_err(|e| e.to_string())?;
        let (Some(id), Some(hash)) = (j["claim_id"].as_str(), j["hash"].as_str()) else {
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
            gone.push(format!("{}\n", serde_json::json!({ "claim_id": id })));
        }
    }
    let removed_body = gone.concat().into_bytes();

    // A delta nobody gains from is not written. The whole is one fetch and the delta is two.
    let cost = body.len() + removed_body.len();
    let whole = full["payloads"]["claims.jsonl"]["bytes"]
        .as_u64()
        .unwrap_or(u64::MAX) as usize;
    if cost >= whole {
        return Err(format!(
            "{cost} bytes against {whole} for the whole, so the whole is the cheaper fetch"
        ));
    }

    let mut payloads = BTreeMap::new();
    payloads.insert(
        "claims.jsonl".to_string(),
        (body.len() as u64, sha256(&body)),
    );
    payloads.insert(
        "removed.jsonl".to_string(),
        (removed_body.len() as u64, sha256(&removed_body)),
    );
    let manifest = serde_json::json!({
        "spec_version": SPEC_VERSION,
        "built_by": concat!("zetlyn ", env!("CARGO_PKG_VERSION")),
        "source": full["source"],
        "version": version,
        "applies_to": previous,
        "built_at": crate::now(),
        "added": added,
        "changed": changed,
        "removed": gone.len(),
        "claims": full["claims"],
        "payloads": J::Object(payloads.iter()
            .map(|(n, (bytes, hash))| (n.clone(), serde_json::json!({ "bytes": bytes, "sha256": hash })))
            .collect()),
    });

    place.put(
        &reference.delta_path("sources", version, previous, "claims.jsonl"),
        &body,
    )?;
    place.put(
        &reference.delta_path("sources", version, previous, "removed.jsonl"),
        &removed_body,
    )?;
    place.put(
        &reference.delta_path("sources", version, previous, "manifest.json"),
        serde_json::to_string_pretty(&manifest)
            .map_err(|e| e.to_string())?
            .as_bytes(),
    )?;
    Ok(())
}

/// Moving a tag says which version it expects to replace. Two publishers of one source pull a
/// tag against each other otherwise, which is not a hypothetical: the second tree did it to
/// itself on the day its hub was set up, a local workspace publishing a newer version and a
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
/// cannot produce.
///
/// Where nothing is pinned, the manifest's own `signed_by` is used and handed back, so a caller
/// can keep it and be protected from the second fetch onward. On the first there is nothing to
/// catch a hub that lied about both, which is what pinning the key by hand is for.
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
    let manifest: J = serde_json::from_slice(&raw).map_err(|e| format!("{reference}: {e}"))?;

    let pinned = pinned.map(str::trim).filter(|k| !k.is_empty());
    let claimed = manifest["signed_by"].as_str().unwrap_or_default().trim();
    let against = pinned.unwrap_or(claimed);
    if !against.is_empty() {
        let signature = place
            .get(&reference.version_path(tree, &version, "manifest.sig"))
            .map_err(|_| {
                if pinned.is_some() {
                    format!("{reference} {version} is not signed, and you pinned a key for it")
                } else {
                    format!("{reference} {version} says {against} signed it and is not signed")
                }
            })?;
        verify(against, &raw, &String::from_utf8_lossy(&signature))
            .map_err(|e| format!("{reference} {version}: {e}"))?;
    }
    Ok(manifest)
}

/// Fetch, verify, and build a source directory from what arrived.
pub fn subscribe(
    place: &dyn Place,
    reference: &Reference,
    into: &Path,
    location: &str,
    pinned: Option<&str>,
) -> Result<(u64, String), String> {
    let manifest = manifest_signed_by(place, reference, "sources", pinned)?;
    let version = manifest["version"].as_str().unwrap_or_default().to_string();
    let spec = manifest["spec_version"].as_str().unwrap_or_default();
    if spec != SPEC_VERSION {
        return Err(format!(
            "{reference}: built against specification {spec}, and this is {SPEC_VERSION}"
        ));
    }

    let declared = manifest["payloads"]["claims.jsonl"].clone();
    let body = place.get(&reference.version_path("sources", &version, "claims.jsonl"))?;
    let want = declared["sha256"].as_str().unwrap_or_default();
    let got = sha256(&body);
    if want != got {
        return Err(format!(
            "{reference}: claims.jsonl is {got} and the manifest says {want}"
        ));
    }

    std::fs::create_dir_all(into).map_err(|e| format!("{}: {e}", into.display()))?;
    std::fs::write(
        into.join(crate::sourcedecl::FILE),
        // The key it was pinned to, or the one that actually signed what arrived. Written down
        // either way, so the next fetch is held against this one.
        declaration(
            &manifest,
            location,
            reference,
            pinned
                .filter(|k| !k.trim().is_empty())
                .unwrap_or_else(|| manifest["signed_by"].as_str().unwrap_or_default()),
        )?,
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
    let name = manifest["source"].as_str().unwrap_or_default();
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
        let record = Claim::from_json(name, &j).map_err(|e| format!("line {}: {e}", n + 1))?;
        for f in record.fields.keys() {
            fields.insert(f.clone());
        }
        match store.put(&record, run, &at, false)? {
            "added" => added += 1,
            "changed" => changed += 1,
            _ => unchanged += 1,
        }
    }
    // A subscription replaces the source. A claim the new version does not carry is a claim
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
    // What a reader is told about freshness is the age of the claims, so this run carries the
    // time the publisher's run finished and not the time it was fetched. A publisher whose
    // manifest names none leaves the fetch time, which is the only thing there is.
    if let Some(theirs) = manifest["finished"].as_str() {
        store.set_finished(run, theirs)?;
    }
    store.set_meta("subscribed", &format!("{location} {reference} {version}"))?;
    Ok((held, version))
}

/// The declaration a subscribed source carries. It holds no source expressions, because nothing
/// is extracted here: the claims arrived built. What it does carry is what the source says
/// about how to read it, which is the publisher's and travels with them.
fn declaration(
    manifest: &J,
    location: &str,
    reference: &Reference,
    key: &str,
) -> Result<String, String> {
    let s = |k: &str| manifest[k].as_str().unwrap_or_default();
    let mut properties = serde_json::Map::new();
    for f in manifest["properties"].as_array().unwrap_or(&Vec::new()) {
        let mut spec = serde_json::Map::new();
        spec.insert("type".into(), json!(f["type"].as_str().unwrap_or("text")));
        if let Some(v) = f["vocabulary"].as_str() {
            spec.insert("vocabulary".into(), json!(v));
        }
        properties.insert(f["name"].as_str().unwrap_or_default().to_string(), J::Object(spec));
    }
    let mut fetch = json!({ "type": "hub", "at": location, "ref": reference.to_string() });
    if !key.trim().is_empty() {
        fetch["key"] = json!(key.trim());
    }
    let built = json!({
        "name": s("source"),
        "title": s("title"),
        "kind": s("kind"),
        "about": s("about"),
        "fetch": fetch,
        "claims": { "title": "field:title", "properties": properties },
        "views": manifest["read"]["views"].as_array().cloned().unwrap_or_default(),
        "search": manifest["read"]["search"].clone(),
    });
    // Read back as a declaration before it is written, so what lands on disk is one this program
    // opens: a hand-assembled file was a file that could say something no declaration says.
    let decl: crate::sourcedecl::SourceDecl = serde_json::from_value(built)
        .map_err(|e| format!("{reference}: the manifest does not make a declaration: {e}"))?;
    Ok(format!(
        "# Subscribed, not fetched. `zetlyn source update` on this asks the hub whether there is a\n\
         # newer version and applies it; the source belongs to whoever published these claims.\n{}",
        crate::yaml::to_string(&decl)?
    ))
}

// ---------------------------------------------------------------------------------------------
// A tracker, which has no payload.

/// A tracker holds no index and has no claims, so what it publishes is the statement and the
/// statement is the whole of it. Its version is the content hash of that statement, which makes
/// two publications of the same composition one version.
pub fn publish_scope(
    dir: &Path,
    datasets: &Path,
    place: &dyn Place,
    tag: &str,
    expect: Option<&str>,
) -> Result<String, String> {
    let path = dir.join(crate::trackerdecl::FILE);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let decl = crate::trackerdecl::TrackerDecl::load(dir)?;
    let version = sha256(text.as_bytes())[..24].to_string();

    // Which version of each source the curator last checked this against. Information and not a
    // pin: a subscriber assembling it from newer sources gets newer sources, which is what
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
        "signed_by": crate::identity::key(),
        "tracker": decl.name,
        "version": version,
        "built_at": crate::now(),
        "title": decl.title,
        "about": decl.about,
        "sources": J::Array(decl.members.iter()
            .map(|m| json!(if m.dataset.is_empty() { m.remote.clone().unwrap_or_default() } else { m.dataset.clone() }))
            .collect()),
        "identified_by": J::Array(decl.keys().iter().map(|k| json!(k)).collect()),
        "promise": {
            "fresh_within": decl.promise.fresh_within,
            "covers": decl.promise.covers,
            "excludes": decl.promise.excludes,
        },
        "checked_against": J::Object(checked.into_iter().collect()),
        "declaration": text,
    });

    let reference = Reference::parse(&format!("{}@{tag}", decl.name))?;
    let manifest_path = reference.version_path("trackers", &version, "manifest.json");
    if !place.exists(&manifest_path) {
        let served = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
        place.put(&manifest_path, served.as_bytes())?;
        // A tracker is signed for the same reason a source is, and rather more: the manifest
        // carries the composition whole, so whoever can change it can change which sources a
        // subscriber assembles and what their words are taken to mean.
        if let Some(signature) = crate::identity::sign(served.as_bytes())? {
            place.put(
                &reference.version_path("trackers", &version, "manifest.sig"),
                signature.as_bytes(),
            )?;
        }
    }
    move_tag(place, &reference.tag_path("trackers"), &version, expect)?;
    Ok(version)
}

/// The statement, and then each source it names that is not held already.
pub fn subscribe_scope(
    place: &dyn Place,
    reference: &Reference,
    into: &Path,
    datasets: &Path,
    location: &str,
) -> Result<(String, Vec<String>), String> {
    let manifest = manifest_at(place, reference, "trackers")?;
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
    std::fs::write(into.join(crate::trackerdecl::FILE), text)
        .map_err(|e| format!("{}: {e}", into.display()))?;
    std::fs::write(
        into.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("{}: {e}", into.display()))?;

    let mut taken = Vec::new();
    for member in manifest["sources"].as_array().unwrap_or(&Vec::new()) {
        let Some(name) = member.as_str().filter(|n| !n.starts_with("http")) else {
            continue;
        };
        let here = datasets.join(name.rsplit('/').next().unwrap_or(name));
        if here.join(crate::sourcedecl::FILE).exists() {
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
    let path = reference.delta_path("sources", to, from, "manifest.json");
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
    for name in ["claims.jsonl", "removed.jsonl"] {
        let bytes = place.get(&reference.delta_path("sources", to, from, name))?;
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
    let name = full["source"].as_str().unwrap_or_default();
    let mut added = 0u64;
    let mut changed = 0u64;
    let mut fields = std::collections::BTreeSet::new();
    for (n, line) in String::from_utf8_lossy(&fetched[0]).lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let j: J = serde_json::from_str(line).map_err(|e| format!("line {}: {e}", n + 1))?;
        let record = Claim::from_json(name, &j).map_err(|e| format!("line {}: {e}", n + 1))?;
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
        if let Some(id) = j["claim_id"].as_str() {
            if store.remove(id, run, &at)? {
                removed += 1;
            }
        }
    }

    // No sweep. A delta says what left, and a claim it did not mention is a claim that stayed.
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
    // The same reason as a full subscribe: freshness is the age of the claims.
    if let Some(theirs) = full["finished"].as_str() {
        store.set_finished(run, theirs)?;
    }
    // The declaration too, and not only the claims. A publisher may have added a field, changed
    // a view or replaced a search example between the two versions, and a subscriber who took the
    // claims and kept the old declaration would hold a source that fails its own check.
    let (location, pinned) = match &crate::sourcedecl::SourceDecl::load(into)?.source {
        crate::sourcedecl::Fetch::Hub { at, key, .. } => (at.clone(), key.clone()),
        _ => (String::new(), String::new()),
    };
    std::fs::write(
        into.join(crate::sourcedecl::FILE),
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
    let want = full["claims"].as_u64().unwrap_or_default();
    let got = store.count();
    if want != got {
        return Err(format!(
            "after the delta this holds {got} claims and the manifest says {want}"
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

pub fn new_key(dir: &Path) -> Result<String, String> {
    crate::key::new(dir, KEY_FILE)
}

/// Held against the key the subscriber pinned, over the manifest exactly as it was served.
pub fn verify(pinned: &str, manifest: &[u8], signature: &str) -> Result<(), String> {
    crate::key::verify(pinned, manifest, signature)
}
