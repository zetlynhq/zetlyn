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

pub const SPEC_VERSION: &str = "2.1";

/// Where a reference with no host is fetched from and published to.
///
/// A reference names a host or it does not, and one that does not means this one. Naming it in a
/// flag as well would be saying the same thing twice, so `--from` and `--to` are for the other
/// cases: a folder, a mount, a bucket, or somebody else's hub.
/// The hub a reference naming no host means: zetlyn.com's, at /hub on the name it shares with the
/// site and the worlds (before 2026-10-05 at its root, which still redirects).
pub const DEFAULT_HUB: &str = "https://zetlyn.com/hub";

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
/// `content` names what the version is: every payload by its plain name and the hash of its plain
/// bytes, so a version is its claims and not the compressor that packed them. `payloads` is what is
/// served, packed, and what a subscriber holds each file against.
pub fn manifest_of(
    ds: &Source,
    content: &BTreeMap<String, (u64, String)>,
    payloads: &BTreeMap<String, (u64, String)>,
) -> J {
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
        // Where it runs: the world it is published from, which a hub names beside it.
        "world": world_of(&ds.dir),
        "source": d.name,
        "version": version_of(content),
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
        "licence": d.licence,

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

/// Whether an artifact built against `spec` can be read here: the same major version, and a minor
/// one this build knows. 2.1 added receipts and history and took nothing away, so a 2.0 artifact
/// is a 2.1 one with neither.
pub fn readable(spec: &str) -> bool {
    let parse = |s: &str| {
        let (a, b) = s.split_once('.')?;
        Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?))
    };
    match (parse(spec), parse(SPEC_VERSION)) {
        (Some((ma, mi)), Some((oa, oi))) => ma == oa && mi <= oi,
        _ => false,
    }
}

/// A payload as it is read: under whichever of its two names the manifest lists, held against
/// its hash, and unpacked. `None` where the manifest lists neither. A 2.0 artifact is plain.
fn unpacked(
    place: &dyn Place,
    path: impl Fn(&str) -> String,
    manifest: &J,
    name: &str,
) -> Result<Option<Vec<u8>>, String> {
    let gz = format!("{name}.gz");
    if !manifest["payloads"][&gz].is_null() {
        let raw = payload(place, &path(&gz), &manifest["payloads"][&gz], &gz)?;
        let mut out = Vec::new();
        use std::io::Read;
        flate2::read::GzDecoder::new(&raw[..])
            .read_to_end(&mut out)
            .map_err(|e| format!("{gz}: {e}"))?;
        return Ok(Some(out));
    }
    if !manifest["payloads"][name].is_null() {
        return payload(place, &path(name), &manifest["payloads"][name], name).map(Some);
    }
    Ok(None)
}

/// How many bytes a manifest says a payload is, under either of its names.
pub fn declared_bytes(manifest: &J, name: &str) -> Option<u64> {
    manifest["payloads"][format!("{name}.gz")]["bytes"]
        .as_u64()
        .or_else(|| manifest["payloads"][name]["bytes"].as_u64())
}

/// A payload the manifest names, fetched and held against its hash.
fn payload(place: &dyn Place, path: &str, declared: &J, name: &str) -> Result<Vec<u8>, String> {
    let body = place.get(path)?;
    let want = declared["sha256"].as_str().unwrap_or_default();
    let got = sha256(&body);
    if want != got {
        return Err(format!("{name} is {got} and the manifest says {want}"));
    }
    Ok(body)
}

/// Versions as they arrived, claim by claim. `replace` for a whole version, which is the whole
/// history; a delta's versions are added to what is held.
fn take_history(store: &Store, body: &[u8], replace: bool) -> Result<u64, String> {
    let mut by: BTreeMap<String, Vec<crate::claim::Version>> = BTreeMap::new();
    for (n, line) in String::from_utf8_lossy(body).lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let j: J = serde_json::from_str(line).map_err(|e| format!("history line {}: {e}", n + 1))?;
        let id = j["claim_id"].as_str().unwrap_or_default().to_string();
        by.entry(id).or_default().push(crate::claim::Version::from_json(&j));
    }
    let mut n = 0u64;
    for (id, versions) in &by {
        store.put_versions(id, versions, replace)?;
        n += versions.len() as u64;
    }
    Ok(n)
}

/// Write `claims.jsonl`, the manifest and the tag. Returns the version.
pub fn publish(
    ds: &Source,
    place: &dyn Place,
    tag: &str,
    expect: Option<&str>,
) -> Result<String, String> {
    // Its turn among the world's heavy work, waited for: a refresh of a tracker is not made beside
    // it. The world is the directory above `sources/`; a source on its own is its own.
    let world = ds
        .dir
        .parent()
        .filter(|p| p.file_name().is_some_and(|n| n == "sources"))
        .and_then(Path::parent)
        .unwrap_or(&ds.dir);
    let _turn = crate::source::heavy_turn(world, true)?;
    // Made on disk, a claim at a time, beside the source: a version made in memory took NVD's
    // 404,000 claims, their history and their packings at once, and a cell of two gigabytes was
    // killed every ten minutes (2026-10-11). The delta from the version this store published last
    // is made in the same pass, from what it wrote down then, not by reading that version back.
    let spool = ds.dir.join(".publishing");
    let _ = std::fs::remove_dir_all(&spool);
    std::fs::create_dir_all(&spool).map_err(|e| format!("{}: {e}", spool.display()))?;
    let made = (|| -> Result<String, String> {
    let last_published = ds.store.meta("published_version");
    let history_from: i64 = ds.store.meta("published_history_run").and_then(|r| r.parse().ok()).unwrap_or(0);
    let mut claims = Spool::new(spool.join("claims.jsonl.gz"))?;
    let mut delta_claims = Spool::new(spool.join("delta-claims.jsonl.gz"))?;
    let mut changes = std::io::BufWriter::new(
        std::fs::File::create(spool.join("changes.tsv")).map_err(|e| e.to_string())?,
    );
    let (mut added, mut changed) = (0u64, 0u64);
    ds.store.for_each_record(|r| {
        use std::io::Write;
        let line = serde_json::to_string(&r.to_json()).map_err(|e| e.to_string())?;
        claims.line(line.as_bytes())?;
        // The line as it was served, and not only its hash: a claim that did not change can still
        // have gained its receipt, and the receipt is what a subscriber would otherwise never get.
        let digest = sha256(line.as_bytes());
        match ds.store.published_as(&r.record_id) {
            Some((was, _)) if was == digest => return Ok(()),
            Some((_, hash)) if hash != r.hash => changed += 1,
            Some(_) => {}
            None => added += 1,
        }
        delta_claims.line(line.as_bytes())?;
        writeln!(changes, "{}\t{digest}\t{}", r.record_id, r.hash).map_err(|e| e.to_string())
    })?;
    {
        use std::io::Write;
        changes.flush().map_err(|e| e.to_string())?;
    }
    let claims = claims.finish()?;
    let delta_claims = delta_claims.finish()?;
    let mut history = Spool::new(spool.join("history.jsonl.gz"))?;
    let mut delta_history = Spool::new(spool.join("delta-history.jsonl.gz"))?;
    ds.store.for_each_version(|id, v| {
        let newer = v.update > history_from;
        let mut line = v.to_json();
        line["claim_id"] = json!(id);
        let line = serde_json::to_string(&line).map_err(|e| e.to_string())?;
        history.line(line.as_bytes())?;
        if newer {
            delta_history.line(line.as_bytes())?;
        }
        Ok(())
    })?;
    let history = history.finish()?;
    let delta_history = delta_history.finish()?;
    let gone = ds.store.published_gone()?;

    // What the version is, by content, and what is served, packed. A source that keeps no
    // history ships none, and the manifest does not name a payload that is not there.
    let mut content = BTreeMap::new();
    let mut files: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut payloads: BTreeMap<String, (u64, String)> = BTreeMap::new();
    for (name, s) in [("claims.jsonl", &claims), ("history.jsonl", &history)] {
        if name == "history.jsonl" && s.plain_len == 0 {
            continue;
        }
        content.insert(name.to_string(), (s.plain_len, s.plain_sha.clone()));
        payloads.insert(format!("{name}.gz"), (s.packed_len, s.packed_sha.clone()));
        files.push((format!("{name}.gz"), s.path.clone()));
    }
    let manifest = manifest_of(ds, &content, &payloads);
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
    // The same claims can already be there in another packing: a version is its content, and one
    // published plain is the version a packed publication makes again. Its files are written
    // wherever the manifest there does not name the ones this one does.
    let listed = held
        .as_deref()
        .and_then(|b| serde_json::from_slice::<J>(b).ok())
        .map(|m| m["payloads"].clone());
    if listed.as_ref() != Some(&manifest["payloads"]) {
        for (name, file) in &files {
            place.put_file(&reference.version_path("sources", &version, name), file)?;
        }
    }
    if held.as_deref().map(|b| differs(b, &manifest)).unwrap_or(true) {
        place.put(&manifest_path, served.as_bytes())?;
        // The source's own key where it has one, because subscribers pinned that and a key that
        // changes under them is a publisher they stop trusting. Your identity otherwise, which is
        // what a source made today signs with.
        let signature = match crate::key::sign(&ds.dir, KEY_FILE, served.as_bytes())? {
            Some(s) => Some(s),
            None => crate::identity::sign_at(&ds.dir, served.as_bytes())?,
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
        // Only from the version this store published last: what it wrote down then is what that
        // version holds. A tag another publisher moved since has no delta from here, only the whole.
        if last_published.as_deref() != Some(previous.as_str()) {
            eprintln!("no delta from {previous}: this store did not publish it");
        } else {
            let parts = DeltaParts { claims: &delta_claims, history: &delta_history, gone: &gone, added, changed };
            if let Err(e) = write_delta(place, &reference, &previous, &version, &manifest, &parts) {
                eprintln!("no delta from {previous}: {e}");
            }
        }
    }

    move_tag(place, &reference.tag_path("sources"), &version, expect)?;
    // What the hub now holds, written down for the next delta.
    ds.store.mark_published(&spool.join("changes.tsv"), &gone)?;
    ds.store.set_meta("published_version", &version)?;
    ds.store.set_meta("published_history_run", &ds.store.last_run().to_string())?;
    Ok(version)
    })();
    let _ = std::fs::remove_dir_all(&spool);
    made
}

/// What a delta is made of, spooled beside the whole in the same pass.
struct DeltaParts<'a> {
    claims: &'a Spooled,
    history: &'a Spooled,
    gone: &'a [String],
    added: u64,
    changed: u64,
}

/// What changed between the version on the hub and the one in this store, written so a subscriber
/// holding the first can reach the second without the whole of it: the claims whose line changed,
/// the versions newer than the last publication, and the claims gone, all spooled already.
fn write_delta(
    place: &dyn Place,
    reference: &Reference,
    previous: &str,
    version: &str,
    full: &J,
    parts: &DeltaParts,
) -> Result<(), String> {
    let removed_body: Vec<u8> = parts
        .gone
        .iter()
        .map(|id| format!("{}\n", serde_json::json!({ "claim_id": id })))
        .collect::<String>()
        .into_bytes();
    let mut payloads: BTreeMap<String, (u64, String)> = BTreeMap::new();
    payloads.insert("claims.jsonl.gz".into(), (parts.claims.packed_len, parts.claims.packed_sha.clone()));
    payloads.insert("removed.jsonl".into(), (removed_body.len() as u64, sha256(&removed_body)));
    if parts.history.plain_len > 0 {
        payloads.insert("history.jsonl.gz".into(), (parts.history.packed_len, parts.history.packed_sha.clone()));
    }
    // Packed as the whole is, and compared packed, since that is what a subscriber fetches. A delta
    // nobody gains from is not written: the whole is one fetch and the delta is two.
    let cost: u64 = payloads.values().map(|(b, _)| *b).sum();
    let whole = declared_bytes(full, "claims.jsonl")
        .unwrap_or(u64::MAX)
        .saturating_add(declared_bytes(full, "history.jsonl").unwrap_or(0));
    if cost >= whole {
        return Err(format!(
            "{cost} bytes against {whole} for the whole, so the whole is the cheaper fetch"
        ));
    }

    let manifest = serde_json::json!({
        "spec_version": SPEC_VERSION,
        "built_by": concat!("zetlyn ", env!("CARGO_PKG_VERSION")),
        "source": full["source"],
        "version": version,
        "applies_to": previous,
        "built_at": crate::now(),
        "added": parts.added,
        "changed": parts.changed,
        "removed": parts.gone.len(),
        "claims": full["claims"],
        "payloads": J::Object(payloads.iter()
            .map(|(n, (bytes, hash))| (n.clone(), serde_json::json!({ "bytes": bytes, "sha256": hash })))
            .collect()),
    });

    place.put_file(&reference.delta_path("sources", version, previous, "claims.jsonl.gz"), &parts.claims.path)?;
    place.put(&reference.delta_path("sources", version, previous, "removed.jsonl"), &removed_body)?;
    if parts.history.plain_len > 0 {
        place.put_file(&reference.delta_path("sources", version, previous, "history.jsonl.gz"), &parts.history.path)?;
    }
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
pub(crate) fn move_tag(
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
    if !readable(spec) {
        return Err(format!(
            "{reference}: built against specification {spec}, and this is {SPEC_VERSION}"
        ));
    }

    let at_version = |n: &str| reference.version_path("sources", &version, n);
    let body = unpacked(place, at_version, &manifest, "claims.jsonl")
        .map_err(|e| format!("{reference}: {e}"))?
        .ok_or_else(|| format!("{reference}: the manifest names no claims"))?;

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
        if let Some(e) = &record.excerpt {
            store.set_excerpt(&record.record_id, e)?;
        }
    }
    // The publisher's history, where the version carries one. It replaces what was held, because
    // a whole version is the whole of the claim's past as its publisher kept it.
    if let Some(h) = unpacked(place, at_version, &manifest, "history.jsonl")
        .map_err(|e| format!("{reference}: {e}"))?
    {
        take_history(&store, &h, true)?;
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
        "terms": s("terms"),
    });
    // Read back as a declaration before it is written, so what lands on disk is one this program
    // opens: a hand-assembled file was a file that could say something no declaration says.
    // The licence travels with the claims: the subscriber republishes on the same terms or not at all.
    let mut built = built;
    if manifest["licence"].is_object() {
        built["licence"] = manifest["licence"].clone();
    }
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
/// The world a thing is published from: the address its workspace names (a hosted one's, on the
/// machine it is hosted on), for a hub to say where it runs. None where the workspace names none.
fn world_of(dir: &Path) -> Option<String> {
    let ws = dir.ancestors().find(|p| p.join(crate::account::WORKSPACE).exists())?;
    let url = crate::account::Site::for_workspace(ws).url;
    (!url.is_empty()).then(|| url.trim_end_matches('/').to_string())
}

/// What a withdrawn entry leaves behind in a hub, for whoever held it to be told.
pub const WITHDRAWN: &str = "withdrawn";

/// A tracker's or a source's entry taken out of a hub: every version and tag of it gone, so nobody
/// fetches it any more, and a note in their place saying when. Whoever holds a copy keeps it; their
/// next pull is told. How many files went.
pub fn withdraw(place: &dyn Place, tree: &str, name: &str) -> Result<usize, String> {
    let reference = Reference::parse(name)?;
    let under = format!("{}/", reference.under(tree));
    let files = place.list(&under)?;
    let mut gone = 0;
    for f in files.iter().filter(|f| !f.ends_with(&format!("/{WITHDRAWN}"))) {
        place.remove(f)?;
        gone += 1;
    }
    place.put(&format!("{under}{WITHDRAWN}"), crate::iso_stamp(crate::now()).as_bytes())?;
    Ok(gone)
}

/// Whether a hub says this entry was withdrawn, and since when.
pub fn withdrawn(place: &dyn Place, tree: &str, name: &str) -> Option<String> {
    let reference = Reference::parse(name).ok()?;
    let note = place.get(&format!("{}/{WITHDRAWN}", reference.under(tree))).ok()?;
    // Published again since, it is not withdrawn whatever the note says.
    if place.exists(&reference.tag_path(tree)) {
        return None;
    }
    Some(String::from_utf8_lossy(&note).trim().to_string())
}

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
    // The licence gate. A public tracker puts every source it names on a page anyone can open,
    // so each of them has to have said that it may be; a private one is read by its accounts only.
    let opened = crate::tracker::Tracker::open(dir, datasets)?;
    if !opened.private() {
        let wrong = opened.not_public();
        if !wrong.is_empty() {
            return Err(format!(
                "{} is public, and {}. Declare `licence: {{ republish: yes | summary | no, terms: <url> }}` on each, or make the tracker `visibility: private`",
                decl.name,
                wrong.join("; ")
            ));
        }
    }

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
        "signed_by": crate::identity::key_at(dir),
        "world": world_of(dir),
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
        if let Some(signature) = crate::identity::sign_at(dir, served.as_bytes())? {
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
    if !readable(spec) {
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
    let at_delta = |n: &str| reference.delta_path("sources", to, from, n);
    fetched.push(
        unpacked(place, at_delta, &manifest, "claims.jsonl")?
            .ok_or_else(|| format!("{path}: it names no claims"))?,
    );
    fetched.push(payload(
        place,
        &at_delta("removed.jsonl"),
        &manifest["payloads"]["removed.jsonl"],
        "removed.jsonl",
    )?);

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
        if let Some(e) = &record.excerpt {
            store.set_excerpt(&record.record_id, e)?;
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

    // The versions the subscriber did not have, added to the ones it did.
    if let Some(h) = unpacked(place, at_delta, &manifest, "history.jsonl")? {
        take_history(&store, &h, false)?;
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

/// What the publisher says about the claims, taken from a manifest into a subscriber's declaration
/// whether or not the claims moved: a licence corrected is a statement, not a new version, and a
/// subscriber republishing on the old one would be republishing on terms nobody states any more.
/// True where anything changed.
pub fn take_statement(dir: &Path, manifest: &J) -> Result<bool, String> {
    let mut decl = crate::sourcedecl::SourceDecl::load(dir)?;
    let licence: crate::sourcedecl::Licence = serde_json::from_value(manifest["licence"].clone()).unwrap_or_default();
    let terms = manifest["terms"].as_str().unwrap_or("").to_string();
    if decl.licence == licence && decl.terms == terms {
        return Ok(false);
    }
    decl.licence = licence;
    decl.terms = terms;
    let text = format!(
        "# Subscribed, not fetched. `zetlyn source update` on this asks the hub whether there is a\n\
         # newer version and applies it; the source belongs to whoever published these claims.\n{}",
        crate::yaml::to_string(&decl)?
    );
    std::fs::write(dir.join(crate::sourcedecl::FILE), text).map_err(|e| e.to_string())?;
    Ok(true)
}

/// A payload made on disk as it is written: its lines packed into a file, and counted and hashed
/// both plain and packed on the way, so a source of 400,000 claims is never whole in memory.
struct Spool {
    path: std::path::PathBuf,
    gz: flate2::write::GzEncoder<Counted<std::io::BufWriter<std::fs::File>>>,
    plain: sha2::Sha256,
    plain_len: u64,
}

/// A writer that counts and hashes what passes through it.
struct Counted<W: std::io::Write> {
    inner: W,
    hash: sha2::Sha256,
    len: u64,
}

impl<W: std::io::Write> std::io::Write for Counted<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        use sha2::Digest;
        let n = self.inner.write(buf)?;
        self.hash.update(&buf[..n]);
        self.len += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// What a spool made: where it is, and its plain and packed length and hash.
struct Spooled {
    path: std::path::PathBuf,
    plain_len: u64,
    plain_sha: String,
    packed_len: u64,
    packed_sha: String,
}

fn hex_of(h: sha2::Sha256) -> String {
    use sha2::Digest;
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

impl Spool {
    fn new(path: std::path::PathBuf) -> Result<Spool, String> {
        use sha2::Digest;
        let file = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let counted = Counted { inner: std::io::BufWriter::new(file), hash: sha2::Sha256::new(), len: 0 };
        Ok(Spool { path, gz: flate2::write::GzEncoder::new(counted, flate2::Compression::new(6)), plain: sha2::Sha256::new(), plain_len: 0 })
    }

    fn line(&mut self, line: &[u8]) -> Result<(), String> {
        use sha2::Digest;
        use std::io::Write;
        for part in [line, b"\n"] {
            self.gz.write_all(part).map_err(|e| e.to_string())?;
            self.plain.update(part);
            self.plain_len += part.len() as u64;
        }
        Ok(())
    }

    fn finish(self) -> Result<Spooled, String> {
        use std::io::Write;
        let mut counted = self.gz.finish().map_err(|e| e.to_string())?;
        counted.flush().map_err(|e| e.to_string())?;
        Ok(Spooled { path: self.path, plain_len: self.plain_len, plain_sha: hex_of(self.plain), packed_len: counted.len, packed_sha: hex_of(counted.hash) })
    }
}
