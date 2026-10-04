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
//!
//! A reader of the workspace's published pages proposes from the browser instead, with no key of
//! their own: they are signed in by a link sent to their address, the source names them (or
//! `signed-in`) in `readers`, and the workspace signs for them with its operator key. The
//! proposal names a pseudonym, `reader:` and twelve hex digits, never the address; which address
//! is behind it is kept in the workspace's `accounts.db` alone, to tell them what was decided.

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
/// What one reader may have waiting at one source. A reader with more is waiting on the owner,
/// and a script that signed somebody in is stopped here rather than in the owner's queue.
pub const MAX_PENDING: usize = 20;
/// In `readers`: anybody signed in.
pub const SIGNED_IN: &str = "signed-in";

/// One proposal as it is kept: the body exactly as it was signed, and who signed it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Kept {
    pub by: String,
    pub signature: String,
    pub received: String,
    pub body: String,
    /// What the proposer asked to be called. A reader's only.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// `browser`: a reader proposed through the pages, and `signature` is the workspace's, by
    /// `vouched_by`, over `vouched(by, name, body)`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub via: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub vouched_by: String,
}

/// Who proposes from the browser: a signed-in reader, or the owner at their own pages.
#[derive(Debug, Clone)]
pub struct Reader {
    /// `reader:<12 hex>`: one account in one workspace is always the same one, and nothing about
    /// it says who they are.
    pub id: String,
    pub name: String,
    /// What `readers` may name. Empty for the owner.
    pub email: String,
    /// The owner needs no invitation to their own source.
    pub owner: bool,
}

/// What the workspace signs for a reader: the proposal, and whose it is.
pub fn vouched(by: &str, name: &str, body: &str) -> String {
    format!("zetlyn proposal\nby {by}\nname {name}\n{body}")
}

/// The workspace's operator key, made the first time something needs signing.
fn operator_key(root: &Path) -> Result<String, String> {
    match crate::key::public(root, crate::grant::OPERATOR_KEY) {
        Some(k) => Ok(k),
        None => crate::key::new(root, crate::grant::OPERATOR_KEY),
    }
}

/// A reader's pseudonym in this workspace. Derived from the workspace's key, so the same account
/// in two workspaces is two strangers.
pub fn pseudonym(root: &Path, account: i64) -> Result<String, String> {
    let key = operator_key(root)?;
    Ok(format!("reader:{}", &crate::place::sha256(format!("{key}\n{account}").as_bytes())[..12]))
}

/// Whether a kept proposal is signed as it says: by the proposer's key, or by the workspace's
/// for a reader.
pub fn verify(kept: &Kept) -> Result<(), String> {
    if kept.via == "browser" {
        crate::key::verify(&kept.vouched_by, vouched(&kept.by, &kept.name, &kept.body).as_bytes(), &kept.signature)
    } else {
        crate::key::verify(&kept.by, kept.body.as_bytes(), &kept.signature)
    }
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
    /// What a reader asked to be called; empty for a key.
    pub name: String,
    /// `browser` where a reader proposed through the pages.
    pub via: String,
    /// Signed as it says. A kept file that does not verify was changed after it arrived.
    pub verified: bool,
}

/// The keys this source takes proposals from. Only a proposals source has any.
fn invited(dir: &Path) -> Result<Vec<String>, String> {
    let decl = crate::sourcedecl::SourceDecl::load(dir)?;
    match decl.source {
        Fetch::Proposals { from, .. } => Ok(from),
        _ => Err(format!("{} takes no proposals", decl.name)),
    }
}

/// Who among the readers may propose here: `signed-in`, addresses, or nobody.
pub fn readers(dir: &Path) -> Result<Vec<String>, String> {
    let decl = crate::sourcedecl::SourceDecl::load(dir)?;
    match decl.source {
        Fetch::Proposals { readers, .. } => Ok(readers),
        _ => Err(format!("{} takes no proposals", decl.name)),
    }
}

/// Says who among the readers may propose, in the source's own declaration.
pub fn set_readers(dir: &Path, readers: Vec<String>) -> Result<(), String> {
    let mut decl = crate::sourcedecl::SourceDecl::load(dir)?;
    let Fetch::Proposals { readers: now, .. } = &mut decl.source else {
        return Err(format!("{} takes no proposals", decl.name));
    };
    *now = readers;
    let path = dir.join(crate::sourcedecl::FILE);
    std::fs::write(&path, crate::yaml::to_string(&decl)?).map_err(|e| format!("{}: {e}", path.display()))
}

/// Whether this reader may propose to this source from the browser.
pub fn may(dir: &Path, reader: &Reader) -> Result<(), String> {
    let readers = readers(dir)?;
    if reader.owner {
        return Ok(());
    }
    if readers.iter().any(|r| r == SIGNED_IN || (!reader.email.is_empty() && r.eq_ignore_ascii_case(reader.email.trim()))) {
        return Ok(());
    }
    Err("this source takes no proposals from readers".into())
}

/// One field a proposed row has, as the declaration reads it: the name in the row, and what
/// kind of value it is taken as.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub kind: crate::sourcedecl::PropertyType,
    /// The property it becomes, where it becomes one under another name.
    pub property: Option<String>,
    /// Part of what identifies a row: a proposal without it names nothing.
    pub identifies: bool,
}

/// What a proposal for this source is made of: every `{field}` its identifier is built from,
/// then every property read straight from a field. What the proposal says about itself is not
/// among them.
pub fn fields(decl: &crate::sourcedecl::SourceDecl) -> Vec<Field> {
    use crate::sourcedecl::PropertyType;
    let simple = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    let mut out: Vec<Field> = Vec::new();
    let mut named_in_id: Vec<String> = Vec::new();
    for id in decl.ids().map(|i| i.each().into_iter().map(|s| s.from.clone()).collect::<Vec<_>>()).unwrap_or_default() {
        if let Some(f) = id.strip_prefix("field:").filter(|f| simple(f)) {
            named_in_id.push(f.to_string());
        }
        let mut rest = id.as_str();
        while let Some(open) = rest.find('{') {
            let Some(close) = rest[open..].find('}') else { break };
            let f = &rest[open + 1..open + close];
            if simple(f) && !named_in_id.iter().any(|n| n == f) {
                named_in_id.push(f.to_string());
            }
            rest = &rest[open + close + 1..];
        }
    }
    let read_from = |name: &str| decl.records.fields.iter().find(|(_, p)| p.from == format!("field:{name}"));
    for name in &named_in_id {
        let (kind, property) = match read_from(name) {
            Some((p, spec)) => (spec.kind, (p != name).then(|| p.clone())),
            None => (PropertyType::Code, None),
        };
        out.push(Field { name: name.clone(), kind, property, identifies: true });
    }
    for (property, spec) in &decl.records.fields {
        let Some(name) = spec.from.strip_prefix("field:").filter(|f| simple(f)) else { continue };
        if RESERVED.contains(&name) || out.iter().any(|f| f.name == name) {
            continue;
        }
        out.push(Field { name: name.to_string(), kind: spec.kind, property: (property != name).then(|| property.clone()), identifies: false });
    }
    out
}

/// The row an accepted proposal said, from a claim's origin (`proposal#<file>`): where a
/// correction starts, field by field as it was proposed, the ones that identify it included.
pub fn row_behind(dir: &Path, origin: &str) -> Option<serde_json::Map<String, J>> {
    let kept = load(dir, origin.strip_prefix("proposal#")?).ok()?;
    let j: J = serde_json::from_str(&kept.body).ok()?;
    j["row"].as_object().cloned()
}

/// A row from what a form said: each field as its kind reads it, empty ones left out. A number
/// may be written with a decimal comma.
pub fn row_from(fields: &[Field], said: &BTreeMap<String, String>) -> Result<serde_json::Map<String, J>, String> {
    use crate::sourcedecl::PropertyType;
    let mut row = serde_json::Map::new();
    for f in fields {
        let v = said.get(&f.name).map(|s| s.trim()).unwrap_or("");
        if v.is_empty() {
            if f.identifies {
                return Err(format!("`{}` is part of what identifies a row, so it is needed", f.name));
            }
            continue;
        }
        let value = match f.kind {
            PropertyType::Number => {
                let n: f64 = v.replace(' ', "").replace(',', ".").parse().map_err(|_| format!("`{}`: {v} is not a number", f.name))?;
                // Whole, it stays whole: 44990, as a proposer writing the file would say it.
                if n.fract() == 0.0 && n.abs() < 9e15 {
                    J::from(n as i64)
                } else {
                    serde_json::Number::from_f64(n).map(J::Number).ok_or_else(|| format!("`{}`: {v} is not a number", f.name))?
                }
            }
            _ => J::String(v.to_string()),
        };
        row.insert(f.name.clone(), value);
    }
    if row.is_empty() {
        return Err("Nothing was said.".into());
    }
    Ok(row)
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
    keep(dir, Kept { by: key.to_string(), signature: signature.to_string(), received: String::new(), body: text, name: String::new(), via: String::new(), vouched_by: String::new() })
}

/// A reader's proposal from the browser, signed by the workspace at `root` for them, and kept.
/// The name it was kept under; the same body from the same reader twice is one file.
pub fn receive_from_reader(dir: &Path, root: &Path, body: &[u8], reader: &Reader) -> Result<String, String> {
    may(dir, reader)?;
    check(body)?;
    let text = String::from_utf8(body.to_vec()).map_err(|_| "not UTF-8")?;
    let waiting = list(dir).iter().filter(|e| e.by == reader.id && e.status == "pending").count();
    if !reader.owner && waiting >= MAX_PENDING {
        return Err(format!("{waiting} of your proposals here are still waiting for the owner; more once they are decided"));
    }
    let name = reader.name.trim().to_string();
    let signature = crate::key::sign(root, crate::grant::OPERATOR_KEY, vouched(&reader.id, &name, &text).as_bytes())?
        .ok_or("this workspace has no key to sign with")?;
    let vouched_by = operator_key(root)?;
    keep(dir, Kept { by: reader.id.clone(), signature, received: String::new(), body: text, name, via: "browser".into(), vouched_by })
}

/// Kept under when it came and what it is, from whom.
fn keep(dir: &Path, mut kept: Kept) -> Result<String, String> {
    let at = crate::now();
    let hash = &crate::place::sha256(format!("{}\n{}", kept.by, kept.body).as_bytes())[..12];
    let store = dir.join(DIR);
    std::fs::create_dir_all(&store).map_err(|e| e.to_string())?;
    if let Some(name) = names(dir).into_iter().find(|n| n.ends_with(&format!("-{hash}.json"))) {
        return Ok(name);
    }
    let name = format!("{}-{hash}.json", crate::iso_stamp(at).replace(':', ""));
    kept.received = crate::iso_stamp(at);
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
            let verified = verify(&kept).is_ok();
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
                verified,
                name: kept.name,
                via: kept.via,
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
        row.insert("proposed_by".into(), json!(if kept.name.is_empty() { kept.by.clone() } else { format!("{} ({})", kept.name, kept.by) }));
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

/// The reader who proposed it, told what became of it, where a reader did. A key's proposer has
/// no address here and hears nothing; neither does anybody when the workspace names no mailer.
pub fn tell_proposer(root: &Path, dir: &Path, file: &str, accept: bool, why: &str) {
    // A workspace with no readers has no accounts to open, and opening would make one.
    if !root.join("accounts.db").exists() {
        return;
    }
    let Ok(decl) = crate::sourcedecl::SourceDecl::load(dir) else { return };
    let Ok(accounts) = crate::account::Accounts::open(root) else { return };
    let Some(reader) = accounts.proposer_of(&decl.name, file) else { return };
    let Ok(kept) = load(dir, file) else { return };
    let row = serde_json::from_str::<J>(&kept.body).map(|j| j["row"].to_string()).unwrap_or_default();
    let title = if decl.title.is_empty() { decl.name.clone() } else { decl.title.clone() };
    let mut text = format!(
        "Your proposal for {title} was {}.\n\n{row}\n",
        if accept { "accepted, and is part of it from its next update" } else { "not taken" }
    );
    if !why.trim().is_empty() {
        text.push_str(&format!("\nWhy: {}\n", why.trim()));
    }
    let subject = format!("{title}: your proposal was {}", if accept { "accepted" } else { "not taken" });
    if let Err(e) = crate::account::Site::load(root).send(&reader.email, &subject, &text) {
        eprintln!("{}: the proposer was not told: {e}", decl.name);
    }
}

/// The owner told that something waits, once: when the first proposal arrives at an empty queue.
/// More while they have not looked would be the same news again. The address is the
/// workspace's `contact`, where it is one.
pub fn tell_owner(root: &Path, dir: &Path) {
    let site = crate::account::Site::load(root);
    if !site.contact.contains('@') || list(dir).iter().filter(|e| e.status == "pending").count() != 1 {
        return;
    }
    let Ok(decl) = crate::sourcedecl::SourceDecl::load(dir) else { return };
    let title = if decl.title.is_empty() { decl.name.clone() } else { decl.title.clone() };
    let base = site.url.trim_end_matches('/');
    let at = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let text = format!("A reader proposed a row for {title}. It waits for you at\n\n{base}/proposals/{at}\n\nMore may arrive before you look; this is the only mail until the queue is empty again.\n");
    if let Err(e) = site.send(site.contact.trim(), &format!("{title}: a proposal waits"), &text) {
        eprintln!("{}: the owner was not told: {e}", decl.name);
    }
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

    /// A workspace with one proposals source in it, at `<root>/sources/prices`.
    fn workspace(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("zetlyn-propose-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("sources/prices");
        source(&dir, "");
        (root, dir)
    }

    fn reader(root: &Path, account: i64, email: &str, name: &str) -> Reader {
        Reader { id: pseudonym(root, account).unwrap(), name: name.into(), email: email.into(), owner: false }
    }

    #[test]
    fn a_reader_proposes_only_where_readers_are_named_and_the_workspace_signs_for_them() {
        let (root, dir) = workspace("r");
        let ann = reader(&root, 7, "ann@example.org", "Ann");
        assert!(receive_from_reader(&dir, &root, BODY, &ann).unwrap_err().contains("no proposals from readers"));

        set_readers(&dir, vec!["ben@example.org".into()]).unwrap();
        assert!(receive_from_reader(&dir, &root, BODY, &ann).is_err(), "only the address named");
        set_readers(&dir, vec!["ANN@example.org".into()]).unwrap();
        let one = receive_from_reader(&dir, &root, BODY, &ann).unwrap();
        assert_eq!(receive_from_reader(&dir, &root, BODY, &ann).unwrap(), one, "the same body twice is one file");

        set_readers(&dir, vec![SIGNED_IN.into()]).unwrap();
        let ben = reader(&root, 8, "ben@example.org", "");
        let two = receive_from_reader(&dir, &root, BODY, &ben).unwrap();
        assert_ne!(one, two);
        // Still the declaration it was, with readers said once.
        let decl = crate::sourcedecl::SourceDecl::load(&dir).unwrap();
        assert!(matches!(decl.source, Fetch::Proposals { ref readers, .. } if readers == &vec![SIGNED_IN.to_string()]));

        // Signed by the workspace, under a pseudonym; the address is nowhere in it.
        let text = std::fs::read_to_string(dir.join(DIR).join(&one)).unwrap();
        assert!(!text.contains("ann@example.org"), "{text}");
        let kept: Kept = serde_json::from_str(&text).unwrap();
        assert_eq!(kept.via, "browser");
        assert_eq!(kept.name, "Ann");
        assert!(kept.by.starts_with("reader:") && kept.by.len() == "reader:".len() + 12, "{}", kept.by);
        assert_eq!(kept.vouched_by, crate::key::public(&root, crate::grant::OPERATOR_KEY).unwrap());
        assert!(verify(&kept).is_ok());
        let queue = list(&dir);
        assert!(queue.iter().all(|e| e.verified && e.via == "browser"));
        assert_eq!(queue.iter().find(|e| e.file == one).unwrap().agreeing, vec![ben.id.clone()]);

        // A name changed in the file after it arrived no longer verifies.
        std::fs::write(dir.join(DIR).join(&one), text.replace("\"Ann\"", "\"Somebody else\"")).unwrap();
        assert!(!list(&dir).iter().find(|e| e.file == one).unwrap().verified);

        // The owner needs no invitation to their own source.
        set_readers(&dir, Vec::new()).unwrap();
        let owner = Reader { id: "owner".into(), name: "the owner".into(), email: String::new(), owner: true };
        assert!(receive_from_reader(&dir, &root, br#"{"row": {"country": "AUT", "week": "2026-W40", "price": 1}, "read_at": "2026-10-02", "read_from": "x", "attest": "read"}"#, &owner).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pseudonym_is_one_account_in_one_workspace() {
        let (a, _) = workspace("pa");
        let (b, _) = workspace("pb");
        assert_eq!(pseudonym(&a, 1).unwrap(), pseudonym(&a, 1).unwrap());
        assert_ne!(pseudonym(&a, 1).unwrap(), pseudonym(&a, 2).unwrap());
        assert_ne!(pseudonym(&a, 1).unwrap(), pseudonym(&b, 1).unwrap(), "two workspaces, two strangers");
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn a_reader_with_too_much_waiting_waits() {
        let (root, dir) = workspace("m");
        set_readers(&dir, vec![SIGNED_IN.into()]).unwrap();
        let ann = reader(&root, 7, "ann@example.org", "Ann");
        let body = |n: usize| format!(r#"{{"row": {{"country": "C{n}", "week": "2026-W40", "price": {n}}}, "read_at": "2026-10-02", "read_from": "x", "attest": "read"}}"#);
        let mut first = String::new();
        for n in 0..MAX_PENDING {
            let name = receive_from_reader(&dir, &root, body(n).as_bytes(), &ann).unwrap();
            if n == 0 {
                first = name;
            }
        }
        assert!(receive_from_reader(&dir, &root, body(MAX_PENDING).as_bytes(), &ann).unwrap_err().contains("still waiting"));
        // Somebody else is not held up by Ann, and a decision makes room.
        assert!(receive_from_reader(&dir, &root, body(MAX_PENDING).as_bytes(), &reader(&root, 8, "ben@example.org", "")).is_ok());
        decide(&dir, &first, false, "owner", "").unwrap();
        assert!(receive_from_reader(&dir, &root, body(MAX_PENDING).as_bytes(), &ann).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_correction_accepted_after_the_row_it_corrects_is_the_claim() {
        let (root, dir) = workspace("c");
        set_readers(&dir, vec![SIGNED_IN.into()]).unwrap();
        let ann = reader(&root, 7, "ann@example.org", "Ann");
        let ben = reader(&root, 8, "ben@example.org", "Ben");
        let first = receive_from_reader(&dir, &root, BODY, &ann).unwrap();
        // Kept a second later, so the two file names sort as they arrived.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let fixed = String::from_utf8_lossy(BODY).replace("44990", "45990");
        let second = receive_from_reader(&dir, &root, fixed.as_bytes(), &ben).unwrap();
        assert!(first < second);
        // Accepted in the other order: what counts is which came later, not which was decided later.
        decide(&dir, &second, true, "owner", "").unwrap();
        decide(&dir, &first, true, "owner", "").unwrap();
        let ds = crate::source::Source::open(&dir).unwrap();
        ds.run().unwrap();
        assert_eq!(ds.store.count(), 1);
        let q = crate::source::Query { text: String::new(), pred: None, ids: vec!["DEU-2026-W40".into()], seen_before: None, view: None, sort: None, limit: 5, offset: 0 };
        let ids: Vec<String> = ds.search(&q).unwrap().1.into_iter().map(|h| h.record_id).collect();
        let claim = ds.fetch(&ids, false);
        let c = claim[0].to_json().to_string();
        assert!(c.contains("45990") && !c.contains("44990"), "{c}");
        assert!(c.contains("Ben (reader:"), "the receipt names the reader by name and pseudonym: {c}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_form_is_made_of_the_fields_a_row_is_read_from() {
        let (root, dir) = workspace("f");
        let decl = crate::sourcedecl::SourceDecl::load(&dir).unwrap();
        use crate::sourcedecl::PropertyType;
        let fs = fields(&decl);
        let named: Vec<(&str, PropertyType, bool)> = fs.iter().map(|f| (f.name.as_str(), f.kind, f.identifies)).collect();
        assert_eq!(named, vec![("country", PropertyType::Code, true), ("week", PropertyType::Code, true), ("price", PropertyType::Number, false)]);

        let said = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<BTreeMap<_, _>>();
        let row = row_from(&fs, &said(&[("country", " DEU "), ("week", "2026-W40"), ("price", "44 990,5")])).unwrap();
        assert_eq!(J::Object(row), json!({"country": "DEU", "week": "2026-W40", "price": 44990.5}));
        assert!(row_from(&fs, &said(&[("country", "DEU"), ("price", "1")])).unwrap_err().contains("`week`"));
        assert!(row_from(&fs, &said(&[("country", "DEU"), ("week", "W"), ("price", "a lot")])).unwrap_err().contains("not a number"));
        // Left empty, a property is not said rather than said empty.
        assert_eq!(J::Object(row_from(&fs, &said(&[("country", "DEU"), ("week", "W"), ("price", "")])).unwrap()), json!({"country": "DEU", "week": "W"}));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_reader_is_told_and_only_where_there_are_readers() {
        let (root, dir) = workspace("t");
        set_readers(&dir, vec![SIGNED_IN.into()]).unwrap();
        // No accounts here: nothing to tell anybody, and nothing made by trying.
        tell_proposer(&root, &dir, "nothing.json", true, "");
        assert!(!root.join("accounts.db").exists());

        let accounts = crate::account::Accounts::open(&root).unwrap();
        let a = accounts.ensure("ann@example.org").unwrap();
        accounts.set_name(a.id, "Ann").unwrap();
        assert_eq!(accounts.name_of(a.id), "Ann");
        let ann = reader(&root, a.id, &a.email, "Ann");
        let file = receive_from_reader(&dir, &root, BODY, &ann).unwrap();
        accounts.record_proposal(a.id, "t/prices", &file).unwrap();
        accounts.record_proposal(a.id, "t/prices", &file).unwrap();
        assert_eq!(accounts.proposals_of(a.id).len(), 1);
        assert_eq!(accounts.proposer_of("t/prices", &file).unwrap().email, "ann@example.org");
        assert!(accounts.proposer_of("t/prices", "other.json").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
