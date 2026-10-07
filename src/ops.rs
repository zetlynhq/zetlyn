//! The main server's hold on every cell: `zetlyn ops`, run as root there (CELLS.md in zetlyn-ops).
//!
//! It keeps the register, `/srv/zetlyn/control/ops/register.yaml`: which servers there are and
//! which cell is on which. It reaches a server only through `zetlyn node` over SSH, with a key that
//! may run that and nothing else. Once a minute it asks every server how its cells are, writes what
//! it heard where the admin pages read it, says what is wrong to the alarm address, and keeps the
//! main server's routes in step. What the admin pages ask for (start, move, upgrade…) they write as
//! a job, and this runs it: the pages are served without root, and only this holds the key.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};

/// The machine's own hosting directory on the main server: its accounts, its billing, its pages.
pub const CONTROL: &str = "/srv/zetlyn/control";

pub fn ops_dir(control: &Path) -> PathBuf {
    control.join("ops")
}

/// Whether this hosting directory is the main server's, holding a register of cells.
pub fn is_control(dir: &Path) -> bool {
    ops_dir(dir).join("register.yaml").exists()
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NodeEntry {
    /// Its address, by which SSH and the main server's Caddy reach it.
    pub host: String,
    /// Its Caddy's own root certificate, which the main server trusts for that hop alone.
    #[serde(default)]
    pub ca: String,
    /// No new cell is placed on it while it is draining.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub draining: bool,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CellEntry {
    pub node: String,
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created: String,
    /// The operator's own, unlimited and never billed: `zetlyn`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub house: bool,
    /// Memory and processor for it, where its plan's default is not enough.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub memory: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cpu: String,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Register {
    /// Where alarms go.
    #[serde(default)]
    pub alarm: String,
    /// A URL asked once a minute while the watching works, so that something outside notices
    /// when it does not (healthchecks.io and the like). Empty for none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub heartbeat: String,
    #[serde(default)]
    pub nodes: BTreeMap<String, NodeEntry>,
    #[serde(default)]
    pub cells: BTreeMap<String, CellEntry>,
}

pub fn register(control: &Path) -> Result<Register, String> {
    crate::yaml::read(&ops_dir(control).join("register.yaml"))
}

pub fn save_register(control: &Path, r: &Register) -> Result<(), String> {
    let path = ops_dir(control).join("register.yaml");
    let partial = path.with_extension("partial");
    std::fs::write(&partial, crate::yaml::to_string(r)?).map_err(|e| e.to_string())?;
    std::fs::rename(&partial, &path).map_err(|e| e.to_string())?;
    // The register is the one file the cells cannot be rebuilt without: it goes to the bucket as
    // it changes, beside the snapshots.
    let _ = keep_in_bucket(control, "control/register.yaml", &path);
    Ok(())
}

fn keep_in_bucket(_control: &Path, key: &str, file: &Path) -> Result<(), String> {
    load_env(crate::cell::S3_ENV)?;
    let bucket = std::env::var("ZETLYN_CELLS_BUCKET").unwrap_or_else(|_| "zetlyn".into());
    crate::place::S3::new(&bucket)?.upload_private(key, file).map(|_| ())
}

fn load_env(path: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    for (k, v) in text.lines().filter_map(|l| l.trim().strip_prefix("export ").unwrap_or(l.trim()).split_once('=')) {
        std::env::set_var(k.trim(), v.trim().trim_matches('"'));
    }
    Ok(())
}

// -- reaching a server --------------------------------------------------------------------------

const SSH_KEY: &str = "/root/.ssh/zetlyn-ops";

/// `zetlyn node <words>` on a server, its output, or what it said went wrong.
pub fn on(node: &NodeEntry, words: &[&str], input: Option<&Path>) -> Result<String, String> {
    let mut cmd = Command::new("ssh");
    cmd.args(["-i", SSH_KEY, "-o", "BatchMode=yes", "-o", "ConnectTimeout=15", "-o", "StrictHostKeyChecking=accept-new", "-o", "ServerAliveInterval=30"])
        .arg(format!("root@{}", node.host))
        .arg("node")
        .args(words);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    if input.is_some() {
        cmd.stdin(Stdio::piped());
    }
    let mut child = cmd.spawn().map_err(|e| format!("ssh: {e}"))?;
    if let Some(path) = input {
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        std::io::copy(&mut f, &mut stdin).map_err(|e| e.to_string())?;
        drop(stdin);
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(format!("{}: {}", node.host, err.trim().lines().last().unwrap_or("failed")))
    }
}

fn node_of<'a>(r: &'a Register, cell: &str) -> Result<(&'a str, &'a NodeEntry), String> {
    let c = r.cells.get(cell).ok_or_else(|| format!("{cell}: no such cell in the register"))?;
    let n = r.nodes.get(&c.node).ok_or_else(|| format!("{cell} is on {}, which the register does not hold", c.node))?;
    Ok((c.node.as_str(), n))
}

// -- what the servers say -----------------------------------------------------------------------

fn status_path(control: &Path, node: &str) -> PathBuf {
    ops_dir(control).join("status").join(format!("{node}.json"))
}

/// What a server said last, and when; Null where it has not answered.
pub fn last_status(control: &Path, node: &str) -> J {
    std::fs::read(status_path(control, node)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(J::Null)
}

/// A cell as its server last described it.
pub fn cell_status(control: &Path, r: &Register, cell: &str) -> J {
    let Some(c) = r.cells.get(cell) else { return J::Null };
    last_status(control, &c.node)["cells"].as_array().and_then(|a| a.iter().find(|x| x["name"] == cell).cloned()).unwrap_or(J::Null)
}

/// Once a minute: every server asked, what it said written down, alarms raised and cleared, the
/// main server's routes kept in step, waiting jobs run, and the heartbeat said.
pub fn poll(control: &Path) -> Result<(), String> {
    let r = register(control)?;
    std::fs::create_dir_all(ops_dir(control).join("status")).map_err(|e| e.to_string())?;
    let mut problems: BTreeMap<String, String> = BTreeMap::new();
    for (name, node) in &r.nodes {
        match on(node, &["status", "--json"], None).and_then(|s| serde_json::from_str::<J>(&s).map_err(|e| e.to_string())) {
            Ok(mut s) => {
                s["heard"] = json!(crate::now());
                let _ = std::fs::write(status_path(control, name), s.to_string());
                problems.extend(judge(name, &s, &r));
            }
            Err(e) => {
                let mut s = last_status(control, name);
                if s.is_null() {
                    s = json!({ "node": name });
                }
                s["error"] = json!(e);
                s["failed_at"] = json!(crate::now());
                let _ = std::fs::write(status_path(control, name), s.to_string());
                problems.insert(format!("node:{name}"), format!("{name} ({}) does not answer: {e}", node.host));
            }
        }
    }
    // A cell in the register that no server holds is a cell that is nowhere.
    for (cell, c) in &r.cells {
        let s = last_status(control, &c.node);
        if s.get("cells").is_some() && cell_status(control, &r, cell).is_null() {
            problems.insert(format!("cell:{cell}:missing"), format!("{cell} is registered on {}, which does not hold it", c.node));
        }
    }
    // The main server's own, kept once a day, and said when it is not.
    let day_ago = crate::iso_stamp(crate::now() - 86_400).replace([':', '-'], "");
    if last_backup(control) < day_ago {
        if let Err(e) = backup(control) {
            eprintln!("backup: {e}");
        }
    }
    let late = crate::iso_stamp(crate::now() - 26 * 3600).replace([':', '-'], "");
    if last_backup(control) < late {
        problems.insert("control:backup".into(), format!("the main server's own has no backup since {}", if last_backup(control).is_empty() { "ever".to_string() } else { last_backup(control) }));
    }
    alarms(control, &r, problems);
    if let Err(e) = routes(control) {
        eprintln!("routes: {e}");
    }
    reconcile(control);
    if let Err(e) = enforce_terms(control) {
        eprintln!("terms: {e}");
    }
    work(control);
    if !r.heartbeat.is_empty() {
        let _ = ureq::get(&r.heartbeat).call();
    }
    Ok(())
}

/// What is wrong on one server, by a key that stays the same while it stays wrong.
fn judge(node: &str, s: &J, r: &Register) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let (free, size) = (s["disk_free"].as_f64().unwrap_or(0.0), s["disk_size"].as_f64().unwrap_or(1.0));
    if size > 0.0 && free / size < 0.15 {
        out.insert(format!("node:{node}:disk"), format!("{node} has {:.0} % of its disk left", 100.0 * free / size));
    }
    let (avail, total) = (s["memory_available"].as_f64().unwrap_or(0.0), s["memory_total"].as_f64().unwrap_or(1.0));
    if total > 0.0 && avail / total < 0.10 {
        out.insert(format!("node:{node}:memory"), format!("{node} has {:.0} % of its memory left", 100.0 * avail / total));
    }
    let day_ago = crate::iso_stamp(crate::now() - 26 * 3600).replace([':', '-'], "");
    for c in s["cells"].as_array().into_iter().flatten() {
        let name = c["name"].as_str().unwrap_or("");
        if !r.cells.contains_key(name) {
            out.insert(format!("cell:{name}:stray"), format!("{node} holds {name}, which the register does not"));
            continue;
        }
        if c["state"] != "running" {
            continue;
        }
        if c["active"] != "active" {
            out.insert(format!("cell:{name}:down"), format!("{name} on {node} is {}", c["active"].as_str().unwrap_or("?")));
        } else if !matches!(c["answers"].as_u64(), Some(200..=399)) {
            out.insert(format!("cell:{name}:answers"), format!("{name} on {node} answers {}", c["answers"]));
        }
        if matches!(c["run_result"].as_str(), Some(r) if !r.is_empty() && r != "success") {
            out.insert(format!("cell:{name}:run"), format!("{name}: its last reading ended {}", c["run_result"].as_str().unwrap_or("")));
        }
        if c["snapshot"].as_str().unwrap_or("") < day_ago.as_str() {
            out.insert(format!("cell:{name}:snapshot"), format!("{name}: no snapshot since {}", c["snapshot"].as_str().unwrap_or("ever")));
        }
        if let (Some(m), Some(max)) = (c["memory"].as_f64(), c["memory_max"].as_f64()) {
            if max > 0.0 && m / max > 0.9 {
                out.insert(format!("cell:{name}:memory"), format!("{name} uses {:.0} % of its memory", 100.0 * m / max));
            }
        }
    }
    out
}

/// Mail the alarm address once when something goes wrong and once when it is right again. A
/// problem has to be seen on two polls in a row before it is said, so one slow answer is not news.
fn alarms(control: &Path, r: &Register, now: BTreeMap<String, String>) {
    let path = ops_dir(control).join("alarms.json");
    let mut held: BTreeMap<String, J> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let mut said: Vec<String> = Vec::new();
    for (key, text) in &now {
        let entry = held.entry(key.clone()).or_insert_with(|| json!({ "text": text, "since": crate::iso_stamp(crate::now()), "seen": 0, "mailed": false }));
        entry["text"] = json!(text);
        entry["seen"] = json!(entry["seen"].as_u64().unwrap_or(0) + 1);
        if entry["seen"].as_u64().unwrap_or(0) >= 2 && entry["mailed"] != true {
            entry["mailed"] = json!(true);
            said.push(format!("WRONG  {text}"));
        }
    }
    let gone: Vec<String> = held.keys().filter(|k| !now.contains_key(*k)).cloned().collect();
    for key in gone {
        if let Some(e) = held.remove(&key) {
            if e["mailed"] == true {
                said.push(format!("RIGHT  {}", e["text"].as_str().unwrap_or(&key)));
            }
        }
    }
    let _ = std::fs::write(&path, serde_json::to_vec_pretty(&held).unwrap_or_default());
    if !said.is_empty() && !r.alarm.is_empty() {
        let site = crate::account::Site::load(control);
        let subject = if said.iter().any(|s| s.starts_with("WRONG")) { "Zetlyn: something is wrong" } else { "Zetlyn: it is right again" };
        let text = format!("{}\n\nhttps://zetlyn.com/account/admin/\n", said.join("\n"));
        if let Err(e) = site.send(&r.alarm, subject, &text) {
            eprintln!("alarm mail: {e}");
        }
    }
}

// -- the main server's routes -------------------------------------------------------------------

pub const MAIN_ROUTES: &str = "/etc/caddy/cells.caddy";
pub const MAIN_DOMAINS: &str = "/etc/caddy/domains.caddy";

/// Which path and which domain goes to which server, as the main server's Caddy reads it, written
/// and reloaded only when it changed.
pub fn routes(control: &Path) -> Result<(), String> {
    let r = register(control)?;
    let mut paths = String::from("# Written by `zetlyn ops`: which server holds which cell. Not edited by hand.\n");
    let mut domains = String::from("# Written by `zetlyn ops`: the domains cells answer at, each with its certificate. Not edited by hand.\n");
    let mut seen = BTreeSet::new();
    for (cell, c) in &r.cells {
        let Some(n) = r.nodes.get(&c.node) else { continue };
        let id = cell.replace('-', "_");
        paths.push_str(&format!(
            "@cell_{id} path /{cell} /{cell}/* /worlds/{cell} /worlds/{cell}/*\nhandle @cell_{id} {{\n\timport cellproxy {} {}\n}}\n",
            n.host, n.ca
        ));
        let s = cell_status(control, &r, cell);
        if let Some(d) = s["domain"].as_str().filter(|d| d.contains('.') && d.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '-')) {
            if seen.insert(d.to_string()) {
                domains.push_str(&format!("{d} {{\n\timport domainproxy {} {}\n}}\n", n.host, n.ca));
            }
        }
    }
    let mut changed = false;
    for (path, text) in [(MAIN_ROUTES, &paths), (MAIN_DOMAINS, &domains)] {
        if std::fs::read_to_string(path).ok().as_deref() != Some(text.as_str()) {
            let partial = format!("{path}.partial");
            std::fs::write(&partial, text).map_err(|e| e.to_string())?;
            std::fs::rename(&partial, path).map_err(|e| e.to_string())?;
            changed = true;
        }
    }
    if changed {
        let out = Command::new("systemctl").args(["reload", "caddy"]).output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!("caddy did not take the routes: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
    }
    Ok(())
}

// -- terms from the plan ------------------------------------------------------------------------

/// What a billed cell used this month as the main server counts it, and what it has told Stripe.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct Usage {
    pub month: String,
    /// Megabyte-days: each day's storage, counted once that day.
    pub mb_days: u64,
    pub day: String,
    pub hour: String,
    pub reads: u64,
    pub mails: u64,
    #[serde(default)]
    pub reported: BTreeMap<String, u64>,
}

fn usage_path(control: &Path, cell: &str) -> PathBuf {
    ops_dir(control).join("usage").join(format!("{cell}.json"))
}

pub fn usage_of(control: &Path, cell: &str) -> Usage {
    std::fs::read(usage_path(control, cell)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// What a month's usage beyond the plan costs so far, in euros: storage as megabyte-days past the
/// month's included ones, reads and mails past theirs.
pub fn overage(u: &Usage, plan: &crate::billing::Plan) -> (f64, f64, f64) {
    let gb_month_mb_days = 1024.0 * 30.0;
    let storage = (u.mb_days as f64 - plan.storage_gb as f64 * gb_month_mb_days).max(0.0) / gb_month_mb_days * 0.5;
    let reads = (u.reads as f64 - plan.reads as f64).max(0.0) / 10_000.0;
    let mails = (u.mails as f64 - plan.mails as f64).max(0.0) / 1_000.0;
    (storage, reads, mails)
}

/// Each billed cell, once a poll: its usage brought up to date from what its server said, told to
/// Stripe once an hour as what changed since it was last told, and its terms as its plan, its
/// payment and its spending limit say, sent where they differ from what its server has. A cell
/// nobody pays for, the house's or one granted by hand, is left alone.
pub fn enforce_terms(control: &Path) -> Result<(), String> {
    let r = register(control)?;
    let billing = control.join("billing");
    let Ok(book) = crate::billing::Book::read(&billing) else { return Ok(()) };
    let (_, plans) = crate::billing::plans(&billing).unwrap_or_default();
    let (_, grace) = crate::billing::terms(&billing);
    let ids = crate::stripe::ids(&billing);
    let _ = std::fs::create_dir_all(ops_dir(control).join("usage"));
    for (cell, c) in &r.cells {
        if c.house {
            continue;
        }
        let Some(customer) = book.get(cell) else { continue };
        let plan = plans.get(&customer.plan).cloned().unwrap_or_default();
        let s = cell_status(control, &r, cell);
        if s.is_null() {
            continue;
        }
        // The month's usage, from the server's counts.
        let mut u = usage_of(control, cell);
        let month = s["usage"]["month"].as_str().unwrap_or("").to_string();
        if month.is_empty() {
            continue;
        }
        if u.month != month {
            // What the last month used after its last report goes to Stripe before it is forgotten.
            if !u.month.is_empty() && s["usage"]["previous"].as_str() == Some(u.month.as_str()) {
                u.reads = s["usage"]["previous_reads"].as_u64().unwrap_or(u.reads);
                u.mails = s["usage"]["previous_mails"].as_u64().unwrap_or(u.mails);
                tell_stripe(&mut u, ids.as_ref(), customer.stripe_customer.as_deref(), cell, "final");
            }
            u = Usage { month: month.clone(), ..Usage::default() };
        }
        u.reads = s["usage"]["reads"].as_u64().unwrap_or(0);
        u.mails = s["usage"]["mails"].as_u64().unwrap_or(0);
        let today = crate::iso_date(crate::now());
        if u.day != today {
            u.mb_days += s["bytes"].as_u64().unwrap_or(0).div_ceil(1 << 20);
            u.day = today;
        }
        let hour = crate::iso_stamp(crate::now()).get(..13).unwrap_or("").replace([':', '-', 'T'], "");
        if u.hour != hour {
            tell_stripe(&mut u, ids.as_ref(), customer.stripe_customer.as_deref(), cell, &hour);
            u.hour = hour;
        }
        let _ = std::fs::write(usage_path(control, cell), serde_json::to_vec_pretty(&u).unwrap_or_default());
        // What the spending limit leaves for reads and mails, in whole euros, so the terms change
        // only when a euro has gone.
        let (storage, reads, mails) = overage(&u, &plan);
        let (read_cap, mail_cap) = if plan.cap > 0 {
            let left_for_reads = (plan.cap as f64 - storage.ceil() - mails.ceil()).max(0.0);
            let left_for_mails = (plan.cap as f64 - storage.ceil() - reads.ceil()).max(0.0);
            (Some(plan.reads + (left_for_reads * 10_000.0) as u64), Some(plan.mails + (left_for_mails * 1_000.0) as u64))
        } else {
            (None, Some(plan.mails))
        };
        let want = crate::cell::Terms {
            active: customer.in_good_standing(grace),
            sources: (plan.sources > 0).then_some(plan.sources),
            every: plan.every.clone(),
            mails: mail_cap,
            reads: read_cap,
            domain: plan.domain,
        };
        let have: crate::cell::Terms = serde_json::from_value(s["terms"].clone()).unwrap_or_default();
        if serde_json::to_value(&have).ok() != serde_json::to_value(&want).ok() {
            let n = r.nodes.get(&c.node).ok_or("no such node")?;
            let mut words: Vec<String> = vec!["terms".into(), cell.clone(), "--active".into(), if want.active { "yes" } else { "no" }.into(), "--domain".into(), if want.domain { "yes" } else { "no" }.into()];
            if let Some(n) = want.sources {
                words.extend(["--sources".into(), n.to_string()]);
            }
            if let Some(n) = want.mails {
                words.extend(["--mails".into(), n.to_string()]);
            }
            if let Some(n) = want.reads {
                words.extend(["--reads".into(), n.to_string()]);
            }
            if !want.every.is_empty() {
                words.extend(["--every".into(), want.every.clone()]);
            }
            let refs: Vec<&str> = words.iter().map(String::as_str).collect();
            on(n, &refs, None)?;
        }
    }
    Ok(())
}

/// What changed since Stripe was last told, told: each kind once per stamp, so that a retry of
/// the same hour is the same event to Stripe and never a second one.
fn tell_stripe(u: &mut Usage, ids: Option<&crate::stripe::Ids>, customer: Option<&str>, cell: &str, stamp: &str) {
    let (Some(_), Some(customer)) = (ids, customer) else { return };
    for m in &crate::stripe::METERED {
        let now = match m.key {
            "storage" => u.mb_days,
            "reads" => u.reads,
            _ => u.mails,
        };
        let told = u.reported.get(m.key).copied().unwrap_or(0);
        if now <= told {
            continue;
        }
        let id = format!("{cell}-{}-{}-{stamp}", m.key, u.month);
        // The last month's last hours belong to it: a minute before this month began.
        let at = if stamp == "final" { crate::thingstore::days(&format!("{}-01", crate::usage::month())).map(|d| d * 86_400 - 60).unwrap_or(crate::now()) } else { crate::now() };
        match crate::stripe::report(m.event, customer, now - told, &id, at) {
            Ok(()) => {
                u.reported.insert(m.key.to_string(), now);
            }
            Err(e) => eprintln!("usage {cell} {}: {e}", m.key),
        }
    }
}

// -- moving cells -------------------------------------------------------------------------------

/// The server with the most room that is not draining: where a new cell goes.
pub fn place(control: &Path, r: &Register) -> Result<String, String> {
    r.nodes
        .iter()
        .filter(|(_, n)| !n.draining)
        .map(|(name, _)| {
            let s = last_status(control, name);
            let room = s["memory_available"].as_f64().unwrap_or(0.0) / s["memory_total"].as_f64().unwrap_or(1.0).max(1.0)
                + s["disk_free"].as_f64().unwrap_or(0.0) / s["disk_size"].as_f64().unwrap_or(1.0).max(1.0);
            (name.clone(), if s.get("error").is_some() { -1.0 } else { room })
        })
        .filter(|(_, room)| *room > 0.0)
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(n, _)| n)
        .ok_or_else(|| "no server answering with room for a cell".to_string())
}

/// A new cell for a world somebody paid for or the operator asked for: placed, made, given its
/// limits, registered and routed.
pub fn create(control: &Path, cell: &str, title: &str, owner: &str, on_node: Option<&str>, house: bool) -> Result<String, String> {
    let mut r = register(control)?;
    if r.cells.contains_key(cell) {
        return Err(format!("{cell}: already registered on {}", r.cells[cell].node));
    }
    let node = match on_node {
        Some(n) => n.to_string(),
        None => place(control, &r)?,
    };
    let n = r.nodes.get(&node).ok_or_else(|| format!("{node}: no such server"))?.clone();
    on(&n, &["create", cell, "--title", &title.replace(char::is_whitespace, "\u{a0}"), "--owner", owner], None)?;
    on(&n, &["limit", cell, "--memory", if house { "2G" } else { "512M" }, "--cpu", if house { "150%" } else { "50%" }], None)?;
    r.cells.insert(cell.to_string(), CellEntry { node: node.clone(), owner: owner.to_string(), title: title.to_string(), created: crate::iso_stamp(crate::now()), house, ..Default::default() });
    save_register(control, &r)?;
    // Its owner signs in at the machine as the owner of it.
    if !owner.is_empty() {
        crate::app::set_member(control, cell, owner, Some("owner"))?;
    }
    let _ = poll_one(control, &node);
    routes(control)?;
    Ok(node)
}

fn poll_one(control: &Path, node: &str) -> Result<(), String> {
    let r = register(control)?;
    let n = r.nodes.get(node).ok_or("no such node")?;
    let mut s: J = serde_json::from_str(&on(n, &["status", "--json"], None)?).map_err(|e| e.to_string())?;
    s["heard"] = json!(crate::now());
    std::fs::write(status_path(control, node), s.to_string()).map_err(|e| e.to_string())
}

/// A cell from its server to another, through the bucket: stopped, snapshotted, restored there,
/// answering there, routed there, and only then gone from where it was. Should anything before the
/// route fail, it is started again where it was and nothing else changes.
pub fn move_cell(control: &Path, cell: &str, to: &str) -> Result<String, String> {
    let mut r = register(control)?;
    let (from_name, from) = node_of(&r, cell)?;
    let (from_name, from) = (from_name.to_string(), from.clone());
    if from_name == to {
        return Err(format!("{cell} is on {to} already"));
    }
    let target = r.nodes.get(to).ok_or_else(|| format!("{to}: no such server"))?.clone();
    let version = cell_status(control, &r, cell)["version"].as_str().unwrap_or("").to_string();
    on(&target, &["status", "--json"], None).map_err(|e| format!("{to} does not answer: {e}"))?;
    on(&from, &["stop", cell], None)?;
    let undo = |why: String| -> String {
        let _ = on(&from, &["start", cell], None);
        format!("{why}; {cell} is running on {from_name} again")
    };
    let at = on(&from, &["snapshot", cell, "--why", "move"], None).map_err(|e| undo(e))?.trim().to_string();
    let mut words = vec!["restore", cell, "--from", at.as_str()];
    if !version.is_empty() {
        words.extend(["--version", version.as_str()]);
    }
    on(&target, &words, None).map_err(|e| undo(e))?;
    let c = r.cells.get(cell).cloned().unwrap_or_default();
    let memory = if c.memory.is_empty() { if c.house { "2G".to_string() } else { "512M".to_string() } } else { c.memory.clone() };
    let cpu = if c.cpu.is_empty() { if c.house { "150%".to_string() } else { "50%".to_string() } } else { c.cpu.clone() };
    let _ = on(&target, &["limit", cell, "--memory", &memory, "--cpu", &cpu], None);
    // It answers there before anybody is sent there.
    let mut answering = false;
    for _ in 0..30 {
        if let Ok(s) = on(&target, &["status", "--json"], None) {
            let s: J = serde_json::from_str(&s).unwrap_or(J::Null);
            if s["cells"].as_array().into_iter().flatten().any(|x| x["name"] == cell && matches!(x["answers"].as_u64(), Some(200..=399))) {
                answering = true;
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(4));
    }
    if !answering {
        let _ = on(&target, &["remove", cell, "--no-snapshot"], None);
        return Err(undo(format!("{cell} did not answer on {to}")));
    }
    if let Some(e) = r.cells.get_mut(cell) {
        e.node = to.to_string();
    }
    save_register(control, &r)?;
    let _ = poll_one(control, to);
    routes(control)?;
    on(&from, &["remove", cell, "--no-snapshot"], None)?;
    let _ = poll_one(control, &from_name);
    Ok(format!("{cell} moved from {from_name} to {to} through snapshot {at}"))
}

/// A cell gone: a last snapshot, kept in the bucket with the others, then off its server, out of
/// the register and the routes. Its owner is no longer an owner at the machine.
pub fn remove(control: &Path, cell: &str) -> Result<String, String> {
    let mut r = register(control)?;
    let (node, n) = node_of(&r, cell)?;
    let (node, n) = (node.to_string(), n.clone());
    if r.cells.get(cell).is_some_and(|c| c.house) {
        return Err(format!("{cell} is the house's own; remove it from the register by hand if that is meant"));
    }
    on(&n, &["remove", cell], None)?;
    let owner = r.cells.remove(cell).map(|c| c.owner).unwrap_or_default();
    save_register(control, &r)?;
    if !owner.is_empty() {
        let _ = crate::app::set_member(control, cell, &owner, None);
    }
    let _ = poll_one(control, &node);
    routes(control)?;
    Ok(format!("{cell} removed from {node}; its snapshots stay in the bucket"))
}

/// A release, from `/srv/zetlyn/releases/<v>/zetlyn` here, onto every server.
pub fn release(_control: &Path, version: &str, current: bool) -> Result<String, String> {
    let r = register(_control)?;
    let file = Path::new(crate::cell::RELEASES).join(version).join("zetlyn");
    let bytes = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let sha = crate::place::sha256(&bytes);
    let mut said = Vec::new();
    for (name, n) in &r.nodes {
        let mut words = vec!["install", version, "--sha256", sha.as_str()];
        if current {
            words.push("--current");
        }
        match on(n, &words, Some(&file)) {
            Ok(_) => said.push(format!("{name}: {version}")),
            Err(e) => said.push(format!("{name}: {e}")),
        }
    }
    Ok(said.join("\n"))
}

/// A cell onto another release: a snapshot first, then the release, and back to the one before if
/// it does not answer within a minute.
pub fn upgrade(control: &Path, cell: &str, version: &str) -> Result<String, String> {
    let r = register(control)?;
    let (node, n) = node_of(&r, cell)?;
    let before = cell_status(control, &r, cell)["version"].as_str().unwrap_or("").to_string();
    on(n, &["snapshot", cell, "--why", "upgrade"], None)?;
    on(n, &["version", cell, version], None)?;
    for _ in 0..15 {
        std::thread::sleep(std::time::Duration::from_secs(4));
        if let Ok(s) = on(n, &["status", "--json"], None) {
            let s: J = serde_json::from_str(&s).unwrap_or(J::Null);
            if s["cells"].as_array().into_iter().flatten().any(|x| x["name"] == cell && matches!(x["answers"].as_u64(), Some(200..=399))) {
                let _ = poll_one(control, node);
                return Ok(format!("{cell}: {before} → {version}"));
            }
        }
    }
    if !before.is_empty() {
        on(n, &["version", cell, &before], None)?;
    }
    Err(format!("{cell} did not answer on {version}; back on {before}"))
}

// -- jobs, from the admin pages -----------------------------------------------------------------

pub const ACTIONS: [&str; 11] = ["start", "stop", "restart", "snapshot", "move", "upgrade", "suspend", "resume", "logs", "create", "remove"];

/// A job for the root side, written by the admin pages. Its id.
pub fn ask(control: &Path, action: &str, cell: &str, args: &BTreeMap<String, String>, by: &str) -> Result<String, String> {
    if !ACTIONS.contains(&action) {
        return Err(format!("{action}: not an action"));
    }
    let id = format!("{}-{}", crate::iso_stamp(crate::now()).replace([':', '-'], ""), crate::jwt::random().get(..8).unwrap_or(""));
    let job = json!({ "id": id, "action": action, "cell": cell, "args": args, "by": by, "at": crate::iso_stamp(crate::now()) });
    let dir = ops_dir(control).join("jobs");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let partial = dir.join(format!("{id}.partial"));
    std::fs::write(&partial, job.to_string()).map_err(|e| e.to_string())?;
    std::fs::rename(&partial, dir.join(format!("{id}.json"))).map_err(|e| e.to_string())?;
    Ok(id)
}

/// The jobs done, newest first, and those waiting.
pub fn jobs(control: &Path, limit: usize) -> Vec<J> {
    let mut out: Vec<J> = Vec::new();
    for sub in ["jobs", "jobs/done"] {
        for e in std::fs::read_dir(ops_dir(control).join(sub)).into_iter().flatten().flatten() {
            if e.path().extension().is_some_and(|x| x == "json") {
                if let Some(j) = std::fs::read(e.path()).ok().and_then(|b| serde_json::from_slice::<J>(&b).ok()) {
                    out.push(j);
                }
            }
        }
    }
    out.sort_by(|a, b| b["id"].as_str().cmp(&a["id"].as_str()));
    out.truncate(limit);
    out
}

/// Every waiting job, one after another, each answered in jobs/done/.
pub fn work(control: &Path) {
    let dir = ops_dir(control).join("jobs");
    let mut waiting: Vec<PathBuf> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
    waiting.sort();
    let _ = std::fs::create_dir_all(dir.join("done"));
    for path in waiting {
        let Some(mut job) = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<J>(&b).ok()) else {
            let _ = std::fs::remove_file(&path);
            continue;
        };
        let cell = job["cell"].as_str().unwrap_or("").to_string();
        let arg = |k: &str| job["args"][k].as_str().unwrap_or("").to_string();
        let result = (|| -> Result<String, String> {
            if !crate::cell::name_ok(&cell) {
                return Err(format!("{cell}: not a cell's name"));
            }
            let r = register(control)?;
            match job["action"].as_str().unwrap_or("") {
                "create" => {
                    let node = create(control, &cell, &arg("title"), &arg("owner"), Some(arg("node")).filter(|n| !n.is_empty()).as_deref(), false)?;
                    if !arg("owner").is_empty() {
                        welcome(control, &cell, &arg("title"), &arg("owner"))?;
                    }
                    Ok(format!("{cell} made on {node}"))
                }
                "move" => move_cell(control, &cell, &arg("to")),
                "remove" if arg("confirm") == cell => remove(control, &cell),
                "remove" => Err(format!("{cell}: not removed, its name was not typed to confirm")),
                "upgrade" => upgrade(control, &cell, &arg("version")),
                "logs" => {
                    let (_, n) = node_of(&r, &cell)?;
                    on(n, &["logs", &cell, "--lines", "300"], None)
                }
                "suspend" | "resume" => {
                    let (_, n) = node_of(&r, &cell)?;
                    let active = if job["action"] == "resume" { "yes" } else { "no" };
                    on(n, &["terms", &cell, "--active", active], None).map(|_| format!("{cell}: updates {}", if active == "yes" { "resumed" } else { "suspended" }))
                }
                a @ ("start" | "stop" | "restart" | "snapshot") => {
                    let (_, n) = node_of(&r, &cell)?;
                    let mut words = vec![a, cell.as_str()];
                    if a == "snapshot" {
                        words.extend(["--why", "asked"]);
                    }
                    on(n, &words, None).map(|o| format!("{cell}: {a} {}", o.trim()))
                }
                other => Err(format!("{other}: not an action")),
            }
        })();
        job["done"] = json!(crate::iso_stamp(crate::now()));
        match result {
            Ok(said) => job["said"] = json!(said),
            Err(e) => job["error"] = json!(e),
        }
        let id = job["id"].as_str().unwrap_or("job").to_string();
        let _ = std::fs::write(dir.join("done").join(format!("{id}.json")), job.to_string());
        let _ = std::fs::remove_file(&path);
        // Who can read the admin pages reads the answer.
        let _ = Command::new("chown").args(["-R", "zetlyn:zetlyn", &dir.to_string_lossy()]).status();
    }
}

// -- the command --------------------------------------------------------------------------------

pub const USAGE: &str = "zetlyn ops [--control <dir>] cells | nodes | poll | work | routes | terms | backup | restore-control [<stamp>|latest] --to <dir> \
| create <cell> --title … --owner … [--node n] [--house] | move <cell> --to <node> | remove <cell> | upgrade <cell> --to <v> | upgrade --all --to <v> \
| release <v> [--current] | start|stop|restart|snapshot|logs <cell> | node-add <name> --host <address> | drain <node> [--off]";

pub fn command(args: &[String]) -> Result<(), String> {
    let control = PathBuf::from(crate::flag(args, "--control").unwrap_or(CONTROL));
    let rest = crate::positional(args, 2);
    let flag = |f: &str| crate::flag(args, f);
    match args.get(1).map(String::as_str) {
        Some("cells") => {
            let r = register(&control)?;
            for (cell, c) in &r.cells {
                let s = cell_status(&control, &r, cell);
                println!("{:<24} {:<6} {:<8} {:<8} answers {:<4} snapshot {:<16} {}", cell, c.node, s["active"].as_str().unwrap_or("?"), s["version"].as_str().unwrap_or(""), s["answers"], s["snapshot"].as_str().unwrap_or("—"), c.owner);
            }
            Ok(())
        }
        Some("nodes") => {
            let r = register(&control)?;
            for (name, n) in &r.nodes {
                let s = last_status(&control, name);
                println!("{:<6} {:<16} {} cells, load {}, {}{}", name, n.host, s["cells"].as_array().map_or(0, Vec::len), s["load"].as_str().unwrap_or("?"), s["error"].as_str().unwrap_or("answering"), if n.draining { ", draining" } else { "" });
            }
            Ok(())
        }
        Some("poll") => poll(&control),
        Some("work") => {
            work(&control);
            Ok(())
        }
        Some("routes") => routes(&control),
        Some("backup") => {
            println!("{}", backup(&control)?);
            Ok(())
        }
        Some("restore-control") => {
            let to = PathBuf::from(flag("--to").ok_or("--to <an empty directory>")?);
            println!("{}", restore_control(rest.first().map(|s| s.as_str()).unwrap_or("latest"), &to)?);
            Ok(())
        }
        Some("terms") => enforce_terms(&control),
        Some("create") => {
            let cell = rest.first().ok_or("which cell?")?;
            let node = create(&control, cell, flag("--title").unwrap_or(cell), flag("--owner").unwrap_or(""), flag("--node"), args.iter().any(|a| a == "--house"))?;
            println!("{cell} on {node}");
            Ok(())
        }
        Some("move") => {
            println!("{}", move_cell(&control, rest.first().ok_or("which cell?")?, flag("--to").ok_or("--to <node>")?)?);
            Ok(())
        }
        Some("remove") => {
            println!("{}", remove(&control, rest.first().ok_or("which cell?")?)?);
            Ok(())
        }
        Some("upgrade") => {
            let to = flag("--to").ok_or("--to <version>")?;
            let cells: Vec<String> = if args.iter().any(|a| a == "--all") { register(&control)?.cells.keys().cloned().collect() } else { vec![rest.first().ok_or("which cell, or --all?")?.to_string()] };
            for c in cells {
                match upgrade(&control, &c, to) {
                    Ok(s) => println!("{s}"),
                    Err(e) => println!("{c}: {e}"),
                }
            }
            Ok(())
        }
        Some("release") => {
            println!("{}", release(&control, rest.first().ok_or("which version?")?, args.iter().any(|a| a == "--current"))?);
            Ok(())
        }
        Some(a @ ("start" | "stop" | "restart" | "snapshot" | "logs")) => {
            let cell = rest.first().ok_or("which cell?")?;
            let r = register(&control)?;
            let (_, n) = node_of(&r, cell)?;
            let mut words = vec![a, cell.as_str()];
            if a == "snapshot" {
                words.extend(["--why", "asked"]);
            }
            print!("{}", on(n, &words, None)?);
            Ok(())
        }
        // A server joins: registered, and its Caddy's root kept here for the hop to it.
        Some("node-add") => {
            let name = rest.first().ok_or("which name, n3?")?.to_string();
            let host = flag("--host").ok_or("--host <address>")?.to_string();
            let mut r = register(&control)?;
            let entry = NodeEntry { host: host.clone(), ca: format!("/etc/caddy/nodes/{name}.crt"), draining: false };
            let ca = on(&entry, &["ca"], None)?;
            std::fs::create_dir_all("/etc/caddy/nodes").map_err(|e| e.to_string())?;
            std::fs::write(&entry.ca, ca).map_err(|e| e.to_string())?;
            r.nodes.insert(name.clone(), entry);
            save_register(&control, &r)?;
            poll_one(&control, &name)?;
            println!("{name} at {host} joined");
            Ok(())
        }
        Some("drain") => {
            let name = rest.first().ok_or("which node?")?;
            let mut r = register(&control)?;
            let n = r.nodes.get_mut(*name).ok_or("no such node")?;
            n.draining = !args.iter().any(|a| a == "--off");
            save_register(&control, &r)?;
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

/// The admin pages' mail to one owner or to every one, with what was sent written down.
pub fn mail(control: &Path, to: &[String], subject: &str, text: &str, by: &str) -> Result<usize, String> {
    let site = crate::account::Site::load(control);
    let mut sent = 0;
    let log = ops_dir(control).join("mail.jsonl");
    for address in to {
        let result = site.send(address, subject, text);
        let line = json!({ "at": crate::iso_stamp(crate::now()), "to": address, "subject": subject, "by": by, "ok": result.is_ok(), "error": result.as_ref().err() });
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log) {
            let _ = writeln!(f, "{line}");
        }
        if result.is_ok() {
            sent += 1;
        }
    }
    Ok(sent)
}

/// The mail a new world's owner gets once it answers: where it is and how to sign in.
pub fn welcome(control: &Path, cell: &str, title: &str, owner: &str) -> Result<(), String> {
    let site = crate::account::Site::load(control);
    let base = site.url.trim_end_matches('/').to_string();
    let title = if title.is_empty() { cell } else { title };
    let (subject, text) = crate::mail::welcome_letter(title, &format!("{base}/{cell}/"), &format!("{base}/account/"), owner);
    site.send(owner, &subject, &text).map(|_| ())
}

// -- the main server's own --------------------------------------------------------------------

fn control_bucket() -> Result<crate::place::S3, String> {
    load_env(crate::cell::S3_ENV)?;
    let bucket = std::env::var("ZETLYN_CELLS_BUCKET").unwrap_or_else(|_| "zetlyn".into());
    crate::place::S3::new(&bucket)
}

/// The main server's own directory, as a cell's is kept: one moment of it, sealed with the cells'
/// key, in `s3://zetlyn/control/snapshots/`. Its accounts, its sign-in key, its customers, its
/// members, its register: what no cell holds and nothing else would bring back.
pub fn backup(control: &Path) -> Result<String, String> {
    let key = crate::cell::key()?;
    let s3 = control_bucket()?;
    let at = crate::cell::stamp();
    let work = Path::new("/var/tmp").join(format!("zetlyn-control-{at}"));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = (|| {
        let plain = work.join("control.tar.gz");
        let files = crate::world::export(control, &plain)?;
        let _ = Command::new("chown").args(["-R", "--reference", &control.to_string_lossy(), &control.to_string_lossy()]).status();
        let sealed = work.join("control.zcell");
        crate::cell::seal(&key, &plain, &sealed)?;
        let bytes = s3.upload_private(&format!("control/snapshots/{at}.zcell"), &sealed)?;
        let about = json!({ "stamp": at, "files": files, "bytes": bytes, "zetlyn": env!("CARGO_PKG_VERSION") });
        s3.put_private(&format!("control/snapshots/{at}.json"), about.to_string().as_bytes())?;
        s3.put_private("control/latest.json", about.to_string().as_bytes())?;
        std::fs::write(ops_dir(control).join("backup.json"), about.to_string()).map_err(|e| e.to_string())?;
        crate::cell::prune_under(&s3, "control/snapshots");
        Ok(at.clone())
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// The main server's directory from the bucket into an empty one: what a new main server starts from.
pub fn restore_control(from: &str, to: &Path) -> Result<String, String> {
    let key = crate::cell::key()?;
    let s3 = control_bucket()?;
    let at = if from == "latest" {
        use crate::place::Place;
        let latest: J = serde_json::from_slice(&s3.get("control/latest.json")?).map_err(|e| e.to_string())?;
        latest["stamp"].as_str().ok_or("no backup named as the latest")?.to_string()
    } else {
        from.to_string()
    };
    let work = Path::new("/var/tmp").join(format!("zetlyn-control-restore-{}", crate::cell::stamp()));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = (|| {
        let sealed = work.join("control.zcell");
        s3.download(&format!("control/snapshots/{at}.zcell"), &sealed)?;
        let plain = work.join("control.tar.gz");
        crate::cell::open(&key, &sealed, &plain)?;
        crate::world::import(&plain, to, None, None)?;
        Ok(at.clone())
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// When the main server's own was last kept, as its stamp; empty for never.
fn last_backup(control: &Path) -> String {
    std::fs::read(ops_dir(control).join("backup.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<J>(&b).ok())
        .and_then(|j| j["stamp"].as_str().map(str::to_string))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_beyond_the_plan_costs_what_the_page_says() {
        let plan = crate::billing::Plan { storage_gb: 2, reads: 25_000, mails: 1_000, cap: 50, ..Default::default() };
        // Within everything: nothing.
        let u = Usage { mb_days: 2 * 1024 * 30, reads: 25_000, mails: 1_000, ..Default::default() };
        assert_eq!(overage(&u, &plan), (0.0, 0.0, 0.0));
        // One GB more for the whole month, 10,000 reads and 1,000 mails more: €0.50, €1, €1.
        let u = Usage { mb_days: 3 * 1024 * 30, reads: 35_000, mails: 2_000, ..Default::default() };
        let (s, r, m) = overage(&u, &plan);
        assert!((s - 0.5).abs() < 1e-9 && (r - 1.0).abs() < 1e-9 && (m - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_megabyte_day_is_priced_as_half_a_euro_a_gigabyte_month() {
        let storage = &crate::stripe::METERED[0];
        let cents: f64 = storage.cents.parse().unwrap();
        // 1 GB for 30 days at that price is 50 cents.
        assert!((cents * 1024.0 * 30.0 - 50.0).abs() < 1e-6);
        assert_eq!(storage.included, 2 * 1024 * 30);
    }
}

/// Once an hour, what Stripe has, taken as if its webhooks had all arrived: every checkout paid in
/// the last two days that made no world yet, and every customer's subscription as it stands now.
/// A webhook lost, refused or never set up costs an hour, not a customer.
pub fn reconcile(control: &Path) {
    let billing = control.join("billing");
    if crate::stripe::ids(&billing).is_none() {
        return;
    }
    let path = ops_dir(control).join("reconciled");
    let hour = crate::iso_stamp(crate::now()).get(..13).unwrap_or("").to_string();
    if std::fs::read_to_string(&path).ok().as_deref() == Some(hour.as_str()) {
        return;
    }
    match crate::stripe::recent_sessions() {
        Ok(events) => {
            for e in events {
                match crate::app::take_event(control, &e) {
                    Ok(s) if !s.ends_with("taken already") => eprintln!("reconcile: {s}"),
                    Ok(_) => {}
                    Err(err) => eprintln!("reconcile: {err}"),
                }
            }
        }
        Err(e) => eprintln!("reconcile: {e}"),
    }
    if let Ok(book) = crate::billing::Book::read(&billing) {
        for c in book.all() {
            let Some(customer) = c.stripe_customer.as_deref() else { continue };
            match crate::stripe::customer_subscription_event(customer) {
                Ok(Some(e)) => {
                    if let Err(err) = crate::billing::apply(&billing, &e) {
                        eprintln!("reconcile {}: {err}", c.name);
                    }
                }
                Ok(None) => {}
                Err(err) => eprintln!("reconcile {}: {err}", c.name),
            }
        }
    }
    // Root wrote the book; the pages that read and write it are zetlyn's.
    let _ = Command::new("chown").args(["-R", "--reference", &control.to_string_lossy(), &billing.to_string_lossy()]).status();
    let _ = std::fs::write(&path, hour);
}
