//! A source people read for it: each row proposed by somebody who is not its owner, and taken only
//! when the owner accepts it.
//!
//! A proposal arrives at the workspace's `/propose/<source>`, signed with the proposer's own key
//! (ed25519 over the body, the key from `zetlyn id`), and is kept as it arrived in `proposals/`.
//! The owner's decisions are lines in `decisions.jsonl`, appended and never rewritten, the shape
//! of `matches.rs`. Accepting puts the row in `inbox/`, which an update reads as it reads a
//! webhook's; rejecting takes it out again, so a claim that was accepted is withdrawn at the next
//! update and both decisions stay on the record.
//!
//! Nothing here makes a source fetchable that refuses to be fetched. The proposer says per body
//! how they came by it (`attest`), and the receipt names them.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};

use crate::sourcedecl::Fetch;

pub const DIR: &str = "proposals";
pub const DECISIONS: &str = "decisions.jsonl";
/// What a proposal may weigh. A row read off a page is a few hundred bytes.
pub const MAX_BODY: usize = 64 * 1024;
/// What the inbox row carries beside the proposed one, so a declaration can read it
/// (`known: field:read_at`). A proposed row may not use these names itself.
pub const RESERVED: [&str; 7] = ["proposed_by", "read_at", "read_from", "attest", "proposal", "accepted_by", "accepted_at"];
const ATTEST: [&str; 2] = ["read", "relayed"];

/// One proposal as it is kept: the body exactly as it was signed, and who signed it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Kept {
    pub by: String,
    pub signature: String,
    pub received: String,
    pub body: String,
}

/// One owner's decision about one proposal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Decision {
    pub at: String,
    pub by: String,
    pub proposal: String,
    /// `accept` or `reject`.
    pub decision: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub why: String,
}

/// A proposal as the queue shows it.
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub file: String,
    pub by: String,
    pub received: String,
    pub row: J,
    pub read_at: String,
    pub read_from: String,
    pub attest: String,
    pub note: String,
    /// `pending`, `accepted` or `rejected`: the last decision about it.
    pub status: String,
    /// Other keys that proposed exactly this row.
    pub agreeing: Vec<String>,
}

/// The keys this source takes proposals from. Only a proposals source has any.
fn invited(dir: &Path) -> Result<Vec<String>, String> {
    let decl = crate::sourcedecl::SourceDecl::load(dir)?;
    match decl.source {
        Fetch::Proposals { from } => Ok(from),
        _ => Err(format!("{} takes no proposals", decl.name)),
    }
}

/// The body, checked: what a proposal has to say for itself before anybody looks at it.
pub fn check(body: &[u8]) -> Result<J, String> {
    if body.len() > MAX_BODY {
        return Err(format!("a proposal is at most {MAX_BODY} bytes"));
    }
    let j: J = serde_json::from_slice(body).map_err(|e| format!("not JSON: {e}"))?;
    let row = j["row"].as_object().ok_or("no `row`: the values proposed, as an object")?;
    if row.is_empty() {
        return Err("`row` is empty".into());
    }
    if let Some(taken) = row.keys().find(|k| RESERVED.contains(&k.as_str())) {
        return Err(format!("`row` may not name `{taken}`: the proposal itself says that"));
    }
    let read_at = j["read_at"].as_str().unwrap_or("");
    let dated = read_at.len() >= 10 && read_at.as_bytes()[..10].iter().enumerate().all(|(i, b)| if i == 4 || i == 7 { *b == b'-' } else { b.is_ascii_digit() });
    if !dated {
        return Err("`read_at`: when it was read, as 2026-10-02 or 2026-10-02T09:14:00Z".into());
    }
    if j["read_from"].as_str().is_none_or(|s| s.trim().is_empty()) {
        return Err("`read_from`: where it was read".into());
    }
    if !j["attest"].as_str().is_some_and(|a| ATTEST.contains(&a)) {
        return Err("`attest`: `read` (you looked yourself) or `relayed` (somebody who did allows it, named in `note`)".into());
    }
    Ok(j)
}

/// A proposal, checked against its signature and the source's invitations, and kept. The name it
/// was kept under; the same body from the same key twice is one file.
pub fn receive(dir: &Path, body: &[u8], key: Option<&str>, signature: Option<&str>) -> Result<String, String> {
    let from = invited(dir)?;
    let key = key.map(str::trim).filter(|k| !k.is_empty()).ok_or("unsigned: say who proposes it with X-Zetlyn-Key")?;
    let signature = signature.map(str::trim).filter(|s| !s.is_empty()).ok_or("unsigned: X-Zetlyn-Signature")?;
    if !from.iter().any(|k| k == key) {
        return Err(format!("{key} is not invited to propose here"));
    }
    crate::key::verify(key, body, signature).map_err(|e| format!("the signature does not verify: {e}"))?;
    check(body)?;
    let text = String::from_utf8(body.to_vec()).map_err(|_| "not UTF-8")?;
    let at = crate::now();
    let hash = &crate::place::sha256(format!("{key}\n{text}").as_bytes())[..12];
    let store = dir.join(DIR);
    std::fs::create_dir_all(&store).map_err(|e| e.to_string())?;
    if let Some(name) = names(dir).into_iter().find(|n| n.ends_with(&format!("-{hash}.json"))) {
        return Ok(name);
    }
    let name = format!("{}-{hash}.json", crate::iso_stamp(at).replace(':', ""));
    let kept = Kept { by: key.to_string(), signature: signature.to_string(), received: crate::iso_stamp(at), body: text };
    std::fs::write(store.join(&name), serde_json::to_string_pretty(&kept).unwrap_or_default()).map_err(|e| e.to_string())?;
    Ok(name)
}

fn names(dir: &Path) -> Vec<String> {
    let mut all: Vec<String> = std::fs::read_dir(dir.join(DIR))
        .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".json")).collect())
        .unwrap_or_default();
    all.sort();
    all
}

fn load(dir: &Path, name: &str) -> Result<Kept, String> {
    if name.contains(['/', '\\']) || name.starts_with('.') {
        return Err(format!("{name}: not a proposal"));
    }
    let text = std::fs::read_to_string(dir.join(DIR).join(name)).map_err(|_| format!("{name}: no such proposal"))?;
    serde_json::from_str(&text).map_err(|e| format!("{name}: {e}"))
}

/// The last decision about each proposal.
fn standing(dir: &Path) -> BTreeMap<String, Decision> {
    let text = std::fs::read_to_string(dir.join(DECISIONS)).unwrap_or_default();
    let mut last = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if let Ok(d) = serde_json::from_str::<Decision>(line) {
            last.insert(d.proposal.clone(), d);
        }
    }
    last
}

/// Every proposal there is, oldest first, with where it stands and who else said the same.
pub fn list(dir: &Path) -> Vec<Entry> {
    let decided = standing(dir);
    let mut out: Vec<Entry> = names(dir)
        .into_iter()
        .filter_map(|name| {
            let kept = load(dir, &name).ok()?;
            let j: J = serde_json::from_str(&kept.body).ok()?;
            let text = |k: &str| j[k].as_str().unwrap_or("").to_string();
            Some(Entry {
                status: match decided.get(&name).map(|d| d.decision.as_str()) {
                    Some("accept") => "accepted".into(),
                    Some("reject") => "rejected".into(),
                    _ => "pending".into(),
                },
                file: name,
                by: kept.by,
                received: kept.received,
                row: j["row"].clone(),
                read_at: text("read_at"),
                read_from: text("read_from"),
                attest: text("attest"),
                note: text("note"),
                agreeing: Vec::new(),
            })
        })
        .collect();
    // The same row from another key is agreement; a map's keys are sorted, so the text is the row.
    let rows: Vec<(String, String)> = out.iter().map(|e| (e.row.to_string(), e.by.clone())).collect();
    for e in &mut out {
        let mine = e.row.to_string();
        let mut others: Vec<String> = rows.iter().filter(|(r, by)| *r == mine && *by != e.by).map(|(_, by)| by.clone()).collect();
        others.sort();
        others.dedup();
        e.agreeing = others;
    }
    out
}

/// The owner's word on one proposal. Accepting puts its row where an update reads it; rejecting
/// takes it out, so a row accepted before is withdrawn at the next update.
pub fn decide(dir: &Path, name: &str, accept: bool, by: &str, why: &str) -> Result<(), String> {
    use std::io::Write;
    invited(dir)?;
    if by.trim().is_empty() {
        return Err("a decision is somebody's: say who with --by".into());
    }
    let kept = load(dir, name)?;
    let inbox = dir.join(crate::hook::INBOX);
    let at = crate::iso_stamp(crate::now());
    if accept {
        let j = check(kept.body.as_bytes())?;
        let mut row = j["row"].as_object().cloned().unwrap_or_default();
        row.insert("proposed_by".into(), json!(kept.by));
        row.insert("read_at".into(), j["read_at"].clone());
        row.insert("read_from".into(), j["read_from"].clone());
        row.insert("attest".into(), j["attest"].clone());
        row.insert("proposal".into(), json!(name));
        row.insert("accepted_by".into(), json!(by.trim()));
        row.insert("accepted_at".into(), json!(at));
        std::fs::create_dir_all(&inbox).map_err(|e| e.to_string())?;
        std::fs::write(inbox.join(name), J::Object(row).to_string()).map_err(|e| e.to_string())?;
    } else {
        let _ = std::fs::remove_file(inbox.join(name));
    }
    let d = Decision { at, by: by.trim().to_string(), proposal: name.to_string(), decision: if accept { "accept" } else { "reject" }.into(), why: why.trim().to_string() };
    let line = serde_json::to_string(&d).map_err(|e| e.to_string())?;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(DECISIONS)).map_err(|e| format!("{}: {e}", dir.join(DECISIONS).display()))?;
    writeln!(f, "{line}").map_err(|e| e.to_string())
}

/// A proposal signed as yourself and sent: to a workspace's `/propose/<source>` address, or
/// straight into a source directory on this machine. The name it was kept under.
pub fn send(to: &str, body: &[u8]) -> Result<String, String> {
    check(body)?;
    let key = crate::identity::key().ok_or("no identity here: `zetlyn id new --name … --contact …` first")?;
    let signature = crate::identity::sign(body)?.ok_or("no identity here")?;
    if !(to.starts_with("http://") || to.starts_with("https://")) {
        return receive(Path::new(to), body, Some(&key), Some(&signature));
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(60)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent
        .post(to)
        .header("Content-Type", "application/json")
        .header("X-Zetlyn-Key", &key)
        .header("X-Zetlyn-Signature", &signature)
        .send(body)
        .map_err(|e| format!("{to}: {e}"))?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string().unwrap_or_default();
    let j: J = serde_json::from_str(&text).unwrap_or(J::Null);
    match (status, j["kept"].as_str(), j["error"].as_str()) {
        (200..=299, Some(name), _) => Ok(name.to_string()),
        (_, _, Some(e)) => Err(format!("{to}: {e}")),
        _ => Err(format!("{to}: {status} {}", text.chars().take(200).collect::<String>())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(dir: &Path, from: &str) {
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("source.yaml"),
            format!("name: t/prices\nkind: price\nfetch:\n  type: proposals\n  from: [{from}]\nclaims:\n  id:\n    scheme: price\n    from: \"const:{{country}}-{{week}}\"\n  title: \"const:{{country}} {{week}}\"\n  known: field:read_at\n  properties:\n    price:\n      type: number\n      from: field:price\n"),
        )
        .unwrap();
    }

    fn signed(seed: u8, body: &[u8]) -> (String, String) {
        use ed25519_dalek::Signer;
        let k = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
        (format!("ed25519:{}", crate::key::hex(k.verifying_key().as_bytes())), format!("ed25519:{}", crate::key::hex(&k.sign(body).to_bytes())))
    }

    const BODY: &[u8] = br#"{"row": {"country": "DEU", "week": "2026-W40", "price": 44990}, "read_at": "2026-10-02T09:14:00Z", "read_from": "https://example.com/configurator", "attest": "read"}"#;

    #[test]
    fn only_an_invited_key_that_verifies_is_kept_and_once() {
        let dir = std::env::temp_dir().join(format!("zetlyn-propose-a-{}", std::process::id()));
        let (a, sig) = signed(1, BODY);
        let (b, sig_b) = signed(2, BODY);
        source(&dir, &a);
        assert!(receive(&dir, BODY, None, Some(&sig)).unwrap_err().contains("unsigned"));
        assert!(receive(&dir, BODY, Some(&b), Some(&sig_b)).unwrap_err().contains("not invited"));
        assert!(receive(&dir, BODY, Some(&a), Some(&sig_b)).unwrap_err().contains("does not verify"));
        let one = receive(&dir, BODY, Some(&a), Some(&sig)).unwrap();
        assert_eq!(receive(&dir, BODY, Some(&a), Some(&sig)).unwrap(), one);
        assert_eq!(names(&dir).len(), 1);
        let bad = br#"{"row": {"proposed_by": "me"}, "read_at": "2026-10-02", "read_from": "x", "attest": "read"}"#;
        let (_, sig_bad) = signed(1, bad);
        assert!(receive(&dir, bad, Some(&a), Some(&sig_bad)).unwrap_err().contains("may not name"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn accepting_makes_a_claim_and_rejecting_withdraws_it_with_both_on_record() {
        let dir = std::env::temp_dir().join(format!("zetlyn-propose-b-{}", std::process::id()));
        let (a, sig) = signed(1, BODY);
        let (b, sig_b) = signed(2, BODY);
        source(&dir, &format!("{a}, {b}"));
        let one = receive(&dir, BODY, Some(&a), Some(&sig)).unwrap();
        let two = receive(&dir, BODY, Some(&b), Some(&sig_b)).unwrap();
        assert_ne!(one, two);
        let queue = list(&dir);
        assert!(queue.iter().all(|e| e.status == "pending" && e.agreeing.len() == 1));

        assert!(decide(&dir, &one, true, " ", "").is_err());
        decide(&dir, &one, true, "owner", "").unwrap();
        let ds = crate::source::Source::open(&dir).unwrap();
        ds.run().unwrap();
        assert_eq!(ds.store.count(), 1);
        let q = crate::source::Query { text: String::new(), pred: None, ids: vec!["DEU-2026-W40".into()], seen_before: None, view: None, sort: None, limit: 5, offset: 0 };
        let ids: Vec<String> = ds.search(&q).unwrap().1.into_iter().map(|h| h.record_id).collect();
        let claims = ds.fetch(&ids, false);
        assert_eq!(claims.len(), 1);
        let c = claims[0].to_json();
        assert_eq!(c["known"], "2026-10-02");
        assert!(c.to_string().contains(&format!("proposal#{one}")));

        // Two more rows, so the withdrawal below is an update that read something: one that reads
        // nothing removes nothing (source.rs), and the last row standing waits for the next one.
        let mut others = Vec::new();
        for country in ["AUT", "NLD"] {
            let body = String::from_utf8_lossy(BODY).replace("DEU", country);
            let (_, s) = signed(1, body.as_bytes());
            others.push(receive(&dir, body.as_bytes(), Some(&a), Some(&s)).unwrap());
        }
        for o in &others {
            decide(&dir, o, true, "owner", "").unwrap();
        }
        ds.run().unwrap();
        assert_eq!(ds.store.count(), 3);

        decide(&dir, &one, false, "owner", "read wrong").unwrap();
        ds.run().unwrap();
        assert_eq!(ds.store.count(), 2);
        assert_eq!(std::fs::read_to_string(dir.join(DECISIONS)).unwrap().lines().count(), 4);
        assert_eq!(list(&dir).iter().find(|e| e.file == one).unwrap().status, "rejected");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
