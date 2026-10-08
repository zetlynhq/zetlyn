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
    /// The operator's own words about it, on the admin pages only.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// Run without Stripe: a pilot, a friend, a gift. Its terms come from the plan and `quota`;
    /// past `free_until`, where set, its updates stop.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub free: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub free_until: String,
    /// What it may use where its plan's numbers are not the right ones.
    #[serde(default, skip_serializing_if = "Quota::is_empty")]
    pub quota: Quota,
    /// A contract ended: the day it is deleted, unless `keep` holds that off.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub delete_on: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub keep: bool,
}

/// A cell's own numbers over its plan's; each unset is the plan's.
#[derive(Debug, Default, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Quota {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_gb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reads: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mails: Option<u64>,
    /// The month's spending limit beyond the plan, in euros.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cap: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<usize>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub every: String,
}

impl Quota {
    pub fn is_empty(&self) -> bool {
        *self == Quota::default()
    }
    /// A plan with these numbers over its own.
    pub fn over(&self, plan: &crate::billing::Plan) -> crate::billing::Plan {
        let mut p = plan.clone();
        if let Some(v) = self.storage_gb {
            p.storage_gb = v;
        }
        if let Some(v) = self.reads {
            p.reads = v;
        }
        if let Some(v) = self.mails {
            p.mails = v;
        }
        if let Some(v) = self.cap {
            p.cap = v;
        }
        if let Some(v) = self.sources {
            p.sources = v;
        }
        if !self.every.is_empty() {
            p.every = self.every.clone();
        }
        p
    }
}

/// What was done, by whom, when: every job asked and answered, every mail, every deletion.
pub fn audit(control: &Path, by: &str, what: &str, cell: &str, said: &str) {
    let line = json!({ "at": crate::iso_stamp(crate::now()), "by": by, "what": what, "cell": cell, "said": said });
    let path = ops_dir(control).join("audit.jsonl");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{line}");
    }
    // Written by root (the jobs) and by the pages (mail): it stays the directory's owner's.
    use std::os::unix::fs::MetadataExt;
    if let Ok(m) = std::fs::metadata(ops_dir(control)) {
        let _ = std::os::unix::fs::chown(&path, Some(m.uid()), Some(m.gid()));
    }
}

/// The newest lines of one of the ops logs, newest first.
pub fn log_lines(control: &Path, file: &str, n: usize) -> Vec<J> {
    let text = std::fs::read_to_string(ops_dir(control).join(file)).unwrap_or_default();
    text.lines().rev().take(n).filter_map(|l| serde_json::from_str(l).ok()).collect()
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
    if let Some(e) = s3_check(control, &r) {
        problems.insert("control:s3".into(), e);
    }
    alarms(control, &r, mute(control, &r, problems));
    push_maintenance(control, &r);
    if let Err(e) = routes(control) {
        eprintln!("routes: {e}");
    }
    reconcile(control);
    if let Err(e) = enforce_terms(control) {
        eprintln!("terms: {e}");
    }
    if let Err(e) = expire(control) {
        eprintln!("expire: {e}");
    }
    clean_downloads(control);
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
            history(control, "wrong", key, text);
        }
    }
    let gone: Vec<String> = held.keys().filter(|k| !now.contains_key(*k)).cloned().collect();
    for key in gone {
        if let Some(e) = held.remove(&key) {
            if e["mailed"] == true {
                said.push(format!("RIGHT  {}", e["text"].as_str().unwrap_or(&key)));
                history(control, "right", &key, e["text"].as_str().unwrap_or(""));
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
        // Billed by Stripe, or run free by the operator's word; anything else is left alone.
        let customer = book.get(cell).filter(|_| !c.free);
        if customer.is_none() && !c.free {
            continue;
        }
        let base = customer.as_ref().and_then(|cu| plans.get(&cu.plan)).or_else(|| plans.values().next()).cloned().unwrap_or_default();
        let plan = c.quota.over(&base);
        let plan_name = customer.as_ref().map(|cu| cu.plan.clone()).unwrap_or_else(|| "free".into());
        let stripe_customer = customer.as_ref().and_then(|cu| cu.stripe_customer.clone());
        let today = crate::iso_date(crate::now());
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
                tell_stripe(&mut u, ids.as_ref(), stripe_customer.as_deref(), cell, "final");
            }
            u = Usage { month: month.clone(), ..Usage::default() };
        }
        u.reads = s["usage"]["reads"].as_u64().unwrap_or(0);
        u.mails = s["usage"]["mails"].as_u64().unwrap_or(0);
        if u.day != today {
            u.mb_days += s["bytes"].as_u64().unwrap_or(0).div_ceil(1 << 20);
            u.day = today.clone();
        }
        let hour = crate::iso_stamp(crate::now()).get(..13).unwrap_or("").replace([':', '-', 'T'], "");
        if u.hour != hour {
            tell_stripe(&mut u, ids.as_ref(), stripe_customer.as_deref(), cell, &hour);
            u.hour = hour;
        }
        let _ = std::fs::write(usage_path(control, cell), serde_json::to_vec_pretty(&u).unwrap_or_default());
        // What the spending limit leaves for reads and mails, in whole euros, so the terms change
        // only when a euro has gone.
        let (storage, reads, mails) = overage(&u, &plan);
        // Run free, nothing beyond the plan is billed, so nothing beyond it is used.
        let (read_cap, mail_cap) = if customer.is_none() {
            (Some(plan.reads), Some(plan.mails))
        } else if plan.cap > 0 {
            let left_for_reads = (plan.cap as f64 - storage.ceil() - mails.ceil()).max(0.0);
            let left_for_mails = (plan.cap as f64 - storage.ceil() - reads.ceil()).max(0.0);
            (Some(plan.reads + (left_for_reads * 10_000.0) as u64), Some(plan.mails + (left_for_mails * 1_000.0) as u64))
        } else {
            (None, Some(plan.mails))
        };
        let want = crate::cell::Terms {
            active: match &customer {
                Some(cu) => cu.in_good_standing(grace),
                None => c.free_until.is_empty() || today <= c.free_until,
            },
            sources: (plan.sources > 0).then_some(plan.sources),
            every: plan.every.clone(),
            mails: mail_cap,
            reads: read_cap,
            domain: plan.domain,
            plan: (plan.storage_gb > 0).then(|| crate::cell::PlanShown {
                title: if plan.title.is_empty() { plan_name.clone() } else { plan.title.clone() },
                storage_gb: plan.storage_gb,
                reads: plan.reads,
                mails: plan.mails,
                cap: plan.cap,
                mb_days: u.mb_days,
            }),
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
            if let Some(p) = &want.plan {
                words.extend(["--plan".into(), p.title.replace(' ', "\u{a0}")]);
                for (f, v) in [("--plan-storage-gb", p.storage_gb), ("--plan-reads", p.reads), ("--plan-mails", p.mails), ("--plan-cap", p.cap), ("--mb-days", p.mb_days)] {
                    words.extend([f.to_string(), v.to_string()]);
                }
            }
            let refs: Vec<&str> = words.iter().map(String::as_str).collect();
            on(n, &refs, None)?;
        }
    }
    Ok(())
}

/// The day a cell whose contract ended is deleted: 30 days past what was paid for.
pub fn delete_day(paid_until: &str) -> Option<String> {
    crate::thingstore::days(paid_until).map(|d| crate::iso_date((d + 30) * 86_400))
}

/// A contract ended: its cell stays 30 days past what was paid for its owner to take it away, as
/// the terms say, and is then deleted, unless the operator keeps it. The owner is told the day once.
pub fn expire(control: &Path) -> Result<(), String> {
    let mut r = register(control)?;
    let Ok(book) = crate::billing::Book::read(&control.join("billing")) else { return Ok(()) };
    let today = crate::iso_date(crate::now());
    let (mut changed, mut due, mut tell) = (false, Vec::new(), Vec::new());
    for (cell, c) in r.cells.iter_mut() {
        if c.house || c.free {
            continue;
        }
        let Some(cu) = book.get(cell) else { continue };
        let until = cu.paid_until.clone().unwrap_or_default();
        let ended = cu.state == "cancelled" && !until.is_empty() && until < today;
        if !ended {
            if !c.delete_on.is_empty() {
                c.delete_on.clear();
                changed = true;
            }
            continue;
        }
        if c.delete_on.is_empty() {
            let Some(day) = delete_day(&until) else { continue };
            c.delete_on = day;
            changed = true;
            audit(control, "ops", "deletion planned", cell, &c.delete_on);
            tell.push((cell.clone(), c.owner.clone(), c.delete_on.clone()));
        }
        if !c.keep && today >= c.delete_on {
            due.push(cell.clone());
        }
    }
    if changed {
        save_register(control, &r)?;
    }
    let base = crate::account::Site::load(control).url.trim_end_matches('/').to_string();
    for (cell, owner, on) in tell {
        if owner.is_empty() {
            continue;
        }
        let text = format!(
            "Hello,\n\nyour plan for {base}/{cell}/ has ended. Until {on} you can still download all of it from its settings (Download all of it); on {on} it is deleted.\n\nIf that is a mistake, write to hello@zetlyn.com before then.\n\nBest regards,\nThe Zetlyn team\n\n--\nZetlyn · https://zetlyn.com · hello@zetlyn.com\n"
        );
        let _ = mail(control, &[owner], "Your Zetlyn organisation is deleted soon", &text, "ops");
    }
    for cell in due {
        match remove(control, &cell) {
            Ok(said) => {
                audit(control, "ops", "deleted", &cell, &said);
                if !r.alarm.is_empty() {
                    let _ = crate::account::Site::load(control).send(&r.alarm, &format!("Zetlyn: {cell} deleted"), &format!("{said}\n\nIts contract ended; 30 days passed.\n"));
                }
            }
            Err(e) => eprintln!("expire {cell}: {e}"),
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

// -- the bucket ---------------------------------------------------------------------------------

/// The bucket as the admin pages show it, looked at every ten minutes: whether it answers and how
/// fast, what it holds by top-level folder, each cell's snapshots, the main server's backups, and
/// snapshots of cells no longer registered. `ops/s3.json`. What went wrong, for the alarms.
fn s3_check(control: &Path, r: &Register) -> Option<String> {
    let path = ops_dir(control).join("s3.json");
    let last: J = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    if crate::now() - last["checked"].as_i64().unwrap_or(0) < 600 {
        return last["error"].as_str().map(str::to_string);
    }
    let started = std::time::Instant::now();
    let listed = control_bucket().and_then(|s3| s3.list_sized(""));
    let ms = started.elapsed().as_millis() as u64;
    let out = match listed {
        Err(e) => json!({ "checked": crate::now(), "at": crate::iso_stamp(crate::now()), "ok": false, "error": e, "ms": ms }),
        Ok(all) => {
            let mut folders: BTreeMap<String, (u64, u64)> = BTreeMap::new();
            let mut cells: BTreeMap<String, J> = BTreeMap::new();
            let (mut control_n, mut control_latest) = (0u64, String::new());
            for (key, size, _) in &all {
                let top = key.split('/').next().unwrap_or("").to_string();
                let f = folders.entry(top).or_default();
                f.0 += 1;
                f.1 += size;
                let parts: Vec<&str> = key.split('/').collect();
                if let ["cells", cell, "snapshots", file] = parts.as_slice() {
                    if let Some(stamp) = file.strip_suffix(".zcell") {
                        let e = cells.entry(cell.to_string()).or_insert_with(|| json!({ "snapshots": 0, "bytes": 0, "latest": "" }));
                        e["snapshots"] = json!(e["snapshots"].as_u64().unwrap_or(0) + 1);
                        e["bytes"] = json!(e["bytes"].as_u64().unwrap_or(0) + size);
                        if stamp > e["latest"].as_str().unwrap_or("") {
                            e["latest"] = json!(stamp);
                        }
                    }
                }
                if let ["control", "snapshots", file] = parts.as_slice() {
                    if let Some(stamp) = file.strip_suffix(".zcell") {
                        control_n += 1;
                        if stamp > control_latest.as_str() {
                            control_latest = stamp.to_string();
                        }
                    }
                }
            }
            let orphans: Vec<&String> = cells.keys().filter(|c| !r.cells.contains_key(*c)).collect();
            json!({
                "checked": crate::now(), "at": crate::iso_stamp(crate::now()), "ok": true, "ms": ms,
                "objects": all.len(), "bytes": all.iter().map(|(_, s, _)| s).sum::<u64>(),
                "folders": folders.iter().map(|(k, (n, b))| (k.clone(), json!({ "objects": n, "bytes": b }))).collect::<serde_json::Map<_, _>>(),
                "cells": cells, "orphans": orphans,
                "control": { "backups": control_n, "latest": control_latest },
            })
        }
    };
    let _ = std::fs::write(&path, serde_json::to_vec_pretty(&out).unwrap_or_default());
    out["error"].as_str().map(|e| format!("the bucket does not answer: {e}"))
}

/// What the last look at the bucket found.
pub fn s3_status(control: &Path) -> J {
    std::fs::read(ops_dir(control).join("s3.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(J::Null)
}

/// A removed cell's snapshots gone from the bucket too, for good. Only for a cell the register no
/// longer holds, and only when its name is typed again.
pub fn purge(control: &Path, cell: &str, confirm: &str) -> Result<String, String> {
    if confirm != cell {
        return Err(format!("{cell}: not purged, its name was not typed to confirm"));
    }
    if register(control)?.cells.contains_key(cell) {
        return Err(format!("{cell} is still registered; remove it first"));
    }
    use crate::place::Place;
    let s3 = control_bucket()?;
    let keys = s3.list(&format!("cells/{cell}/"))?;
    for k in &keys {
        s3.delete(k)?;
    }
    let _ = std::fs::remove_file(ops_dir(control).join("s3.json"));
    Ok(format!("{cell}: {} objects gone from the bucket", keys.len()))
}

// -- snapshots, from the admin pages ------------------------------------------------------------

/// A cell's snapshots in the bucket, newest first, as their `.json` beside each says.
pub fn snapshots(cell: &str) -> Result<Vec<J>, String> {
    use crate::place::Place;
    let s3 = control_bucket()?;
    let mut keys: Vec<String> = s3.list(&format!("cells/{cell}/snapshots/"))?.into_iter().filter(|k| k.ends_with(".json")).collect();
    keys.sort();
    keys.reverse();
    Ok(keys.iter().filter_map(|k| s3.get(k).ok().and_then(|b| serde_json::from_slice(&b).ok())).collect())
}

/// Where the admin pages offer what was asked for download, for a day.
pub fn downloads_dir(control: &Path) -> PathBuf {
    ops_dir(control).join("downloads")
}

/// One snapshot opened, as the archive it was made from, for the operator to download: the whole
/// cell (its hosting directory, `world/orgs/<cell>/` the organisation).
pub fn download(control: &Path, cell: &str, stamp: &str) -> Result<String, String> {
    if !stamp.chars().all(|c| c.is_ascii_alphanumeric()) || stamp.is_empty() {
        return Err("which snapshot?".into());
    }
    let s3 = control_bucket()?;
    let key = crate::cell::key()?;
    let dir = downloads_dir(control);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let sealed = Path::new("/var/tmp").join(format!("zetlyn-download-{cell}-{stamp}.zcell"));
    let name = format!("{cell}-{stamp}.tar.gz");
    let result = (|| {
        s3.download(&format!("cells/{cell}/snapshots/{stamp}.zcell"), &sealed)?;
        crate::cell::open(&key, &sealed, &dir.join(&name))?;
        let _ = Command::new("chown").args(["-R", "zetlyn:zetlyn", &dir.to_string_lossy()]).status();
        Ok(format!("{name} ready for a day"))
    })();
    let _ = std::fs::remove_file(&sealed);
    result
}

/// Downloads older than a day, gone.
fn clean_downloads(control: &Path) {
    for e in std::fs::read_dir(downloads_dir(control)).into_iter().flatten().flatten() {
        let old = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|d| d.as_secs() > 86_400);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// A cell back to one of its snapshots, in place: a snapshot of it as it is first, then the one
/// asked for restored on the same server; where that does not answer, the one just taken.
pub fn restore_to(control: &Path, cell: &str, stamp: &str) -> Result<String, String> {
    let r = register(control)?;
    let (node, n) = node_of(&r, cell)?;
    let (node, n) = (node.to_string(), n.clone());
    let version = cell_status(control, &r, cell)["version"].as_str().unwrap_or("").to_string();
    let before = on(&n, &["snapshot", cell, "--why", "before-restore"], None)?.trim().to_string();
    let c = r.cells.get(cell).cloned().unwrap_or_default();
    let put = |from: &str| -> Result<(), String> {
        on(&n, &["remove", cell, "--no-snapshot"], None)?;
        let mut words = vec!["restore", cell, "--from", from];
        if !version.is_empty() {
            words.extend(["--version", version.as_str()]);
        }
        on(&n, &words, None)?;
        let memory = if c.memory.is_empty() { "512M" } else { c.memory.as_str() };
        let cpu = if c.cpu.is_empty() { "50%" } else { c.cpu.as_str() };
        let _ = on(&n, &["limit", cell, "--memory", memory, "--cpu", cpu], None);
        Ok(())
    };
    let answers = || {
        (0..30).any(|_| {
            std::thread::sleep(std::time::Duration::from_secs(4));
            on(&n, &["status", "--json"], None).ok().and_then(|s| serde_json::from_str::<J>(&s).ok()).is_some_and(|s| {
                s["cells"].as_array().into_iter().flatten().any(|x| x["name"] == cell && matches!(x["answers"].as_u64(), Some(200..=399)))
            })
        })
    };
    if put(stamp).is_ok() && answers() {
        let _ = poll_one(control, &node);
        routes(control)?;
        return Ok(format!("{cell} is back at {stamp}; as it was is in {before}"));
    }
    put(&before)?;
    let _ = poll_one(control, &node);
    routes(control)?;
    Err(format!("{cell} did not answer at {stamp}; it is as it was again ({before})"))
}

// -- changes, from the admin pages --------------------------------------------------------------

/// A cell's title, owners or domain, here and in the cell: `owners` the whole list, the first the
/// one the register names; a domain empty for none.
pub fn set(control: &Path, cell: &str, title: Option<&str>, owners: Option<&[String]>, domain: Option<&str>, note: Option<&str>) -> Result<String, String> {
    let mut r = register(control)?;
    let (_, n) = node_of(&r, cell)?;
    let n = n.clone();
    let mut words: Vec<String> = vec!["set".into(), cell.into()];
    let mut said = Vec::new();
    if let Some(t) = title.filter(|t| !t.trim().is_empty()) {
        words.extend(["--title".into(), t.trim().replace(char::is_whitespace, "\u{a0}")]);
        said.push(format!("title {t}"));
    }
    if let Some(d) = domain {
        let d = d.trim().to_lowercase();
        if !d.is_empty() && !d.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
            return Err(format!("{d}: not a domain"));
        }
        words.extend(["--domain".into(), if d.is_empty() { "-".into() } else { d.clone() }]);
        said.push(format!("domain {}", if d.is_empty() { "none" } else { &d }));
    }
    let members = crate::app::owners_of(control, cell);
    if let Some(list) = owners {
        let want: Vec<String> = list.iter().map(|o| o.trim().to_lowercase()).filter(|o| o.contains('@')).collect();
        if want.is_empty() {
            return Err("at least one owner".into());
        }
        for o in want.iter().filter(|o| !members.contains(o)) {
            words.extend(["--owner".into(), o.clone()]);
            crate::app::set_member(control, cell, o, Some("owner"))?;
        }
        for o in members.iter().filter(|o| !want.contains(o)) {
            words.extend(["--not-owner".into(), o.clone()]);
            crate::app::set_member(control, cell, o, None)?;
        }
        said.push(format!("owners {}", want.join(", ")));
        if let Some(e) = r.cells.get_mut(cell) {
            e.owner = want[0].clone();
        }
    }
    if words.len() > 2 {
        let refs: Vec<&str> = words.iter().map(String::as_str).collect();
        on(&n, &refs, None)?;
    }
    if let Some(e) = r.cells.get_mut(cell) {
        if let Some(t) = title.filter(|t| !t.trim().is_empty()) {
            e.title = t.trim().to_string();
        }
        if let Some(nt) = note {
            e.note = nt.trim().to_string();
            said.push("note".into());
        }
    }
    save_register(control, &r)?;
    let _ = poll_one(control, &r.cells.get(cell).map(|c| c.node.clone()).unwrap_or_default());
    routes(control)?;
    Ok(format!("{cell}: {}", if said.is_empty() { "nothing changed".to_string() } else { said.join("; ") }))
}

/// A cell's limits: memory and processor at its server, its numbers over its plan's, and whether
/// it runs free and until when; its terms sent at once.
pub fn limits(control: &Path, cell: &str, args: &BTreeMap<String, String>) -> Result<String, String> {
    let mut r = register(control)?;
    let n = node_of(&r, cell)?.1.clone();
    let get = |k: &str| args.get(k).map(|v| v.trim().to_string()).unwrap_or_default();
    let num = |k: &str| -> Result<Option<u64>, String> {
        let v = get(k);
        if v.is_empty() { Ok(None) } else { v.parse().map(Some).map_err(|_| format!("{k}: a number")) }
    };
    let quota = Quota {
        storage_gb: num("storage_gb")?,
        reads: num("reads")?,
        mails: num("mails")?,
        cap: num("cap")?,
        sources: num("sources")?.map(|v| v as usize),
        every: get("every"),
    };
    let e = r.cells.get_mut(cell).ok_or("no such cell")?;
    let (memory, cpu) = (get("memory"), get("cpu"));
    if !memory.is_empty() || !cpu.is_empty() {
        let m = if memory.is_empty() { if e.memory.is_empty() { "512M".to_string() } else { e.memory.clone() } } else { memory.clone() };
        let c = if cpu.is_empty() { if e.cpu.is_empty() { "50%".to_string() } else { e.cpu.clone() } } else { cpu.clone() };
        on(&n, &["limit", cell, "--memory", &m, "--cpu", &c], None)?;
        e.memory = m;
        e.cpu = c;
    }
    e.quota = quota;
    if args.contains_key("billing") {
        e.free = get("billing") == "free";
        e.free_until = get("free_until");
        if !e.free_until.is_empty() && crate::thingstore::days(&e.free_until).is_none() {
            return Err("free until: a date, 2026-12-31".into());
        }
    }
    let said = format!("{cell}: memory {}, cpu {}, {}{}", e.memory, e.cpu, if e.free { "free" } else { "billed" }, if e.free_until.is_empty() { String::new() } else { format!(" until {}", e.free_until) });
    save_register(control, &r)?;
    enforce_terms(control)?;
    Ok(said)
}

/// A server joins: registered, and its Caddy's root kept here for the hop to it.
pub fn node_add(control: &Path, name: &str, host: &str) -> Result<String, String> {
    if !crate::cell::name_ok(name) || host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == ':' || c == '-') {
        return Err("a name like n3 and an address".into());
    }
    let mut r = register(control)?;
    let entry = NodeEntry { host: host.to_string(), ca: format!("/etc/caddy/nodes/{name}.crt"), draining: false };
    let ca = on(&entry, &["ca"], None)?;
    std::fs::create_dir_all("/etc/caddy/nodes").map_err(|e| e.to_string())?;
    std::fs::write(&entry.ca, ca).map_err(|e| e.to_string())?;
    r.nodes.insert(name.to_string(), entry);
    save_register(control, &r)?;
    poll_one(control, name)?;
    Ok(format!("{name} at {host} joined"))
}

pub fn drain(control: &Path, name: &str, on_: bool) -> Result<String, String> {
    let mut r = register(control)?;
    r.nodes.get_mut(name).ok_or("no such node")?.draining = on_;
    save_register(control, &r)?;
    Ok(format!("{name}: {}", if on_ { "takes no new cells" } else { "takes new cells again" }))
}

// -- maintenance --------------------------------------------------------------------------------

/// The notice of the admin pages, where it is.
pub fn maintenance_path(control: &Path) -> PathBuf {
    ops_dir(control).join(crate::maintenance::FILE)
}

/// Each cell given the notice that applies to it while it stands, and none after: sent to its
/// server only when that changed.
fn push_maintenance(control: &Path, r: &Register) {
    let notice = crate::maintenance::read(&maintenance_path(control));
    let now = crate::iso_stamp(crate::now());
    let pushed_path = ops_dir(control).join("maintenance-pushed.json");
    let mut pushed: BTreeMap<String, String> = std::fs::read(&pushed_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    for (cell, c) in &r.cells {
        let want = notice.as_ref().filter(|n| n.applies(&c.node, cell) && n.state(&now).is_some()).map(|n| {
            let mut n = n.clone();
            n.target.clear();
            n
        });
        let text = want.as_ref().map(|n| serde_json::to_string(n).unwrap_or_default()).unwrap_or_default();
        if pushed.get(cell).map(String::as_str).unwrap_or("") == text {
            continue;
        }
        let Some(node) = r.nodes.get(&c.node) else { continue };
        let hex = crate::key::hex(text.as_bytes());
        let words: Vec<&str> = if text.is_empty() { vec!["maintenance", cell, "--clear"] } else { vec!["maintenance", cell, "--set", hex.as_str()] };
        match on(node, &words, None) {
            Ok(_) => {
                pushed.insert(cell.clone(), text);
            }
            Err(e) => eprintln!("maintenance {cell}: {e}"),
        }
    }
    let _ = std::fs::write(&pushed_path, serde_json::to_vec(&pushed).unwrap_or_default());
}

/// What would be an alarm, without what the maintenance in force covers: nothing is said of a
/// server or cell while it is being worked on, and it starts counting again after.
fn mute(control: &Path, r: &Register, problems: BTreeMap<String, String>) -> BTreeMap<String, String> {
    let Some(n) = crate::maintenance::read(&maintenance_path(control)) else { return problems };
    if n.state(&crate::iso_stamp(crate::now())) != Some("active") {
        return problems;
    }
    problems
        .into_iter()
        .filter(|(key, _)| {
            if n.target.is_empty() || n.target == "all" {
                return false;
            }
            if let Some(rest) = key.strip_prefix("node:") {
                return !n.applies(rest.split(':').next().unwrap_or(""), "");
            }
            if let Some(rest) = key.strip_prefix("cell:") {
                let cell = rest.split(':').next().unwrap_or("");
                let node = r.cells.get(cell).map(|c| c.node.as_str()).unwrap_or("");
                return !n.applies(node, cell);
            }
            true
        })
        .collect()
}

// -- jobs, from the admin pages -----------------------------------------------------------------

pub const ACTIONS: [&str; 23] = [
    "start", "stop", "restart", "snapshot", "move", "upgrade", "suspend", "resume", "logs", "create", "remove", "set", "limits", "snapshots", "restore", "download",
    "keep", "unkeep", "node-add", "drain", "undrain", "upgrade-all", "purge",
];

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
    audit(control, by, &format!("asked {action}"), cell, &serde_json::to_string(args).unwrap_or_default());
    Ok(id)
}

/// The jobs done, newest first, and those waiting.
pub fn jobs(control: &Path, limit: usize) -> Vec<J> {
    let mut out: Vec<J> = Vec::new();
    for sub in ["jobs", "jobs/done"] {
        for e in std::fs::read_dir(ops_dir(control).join(sub)).into_iter().flatten().flatten() {
            if e.path().extension().is_some_and(|x| x == "json" || x == "running") {
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
    for waiting_path in waiting {
        // Taken by renaming it: the minute's poll and the watcher both run this, and only the one
        // whose rename succeeds runs the job.
        let path = waiting_path.with_extension("running");
        if std::fs::rename(&waiting_path, &path).is_err() {
            continue;
        }
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
                    let owners: Vec<String> = arg("owners").split([',', '\n', ' ']).map(|o| o.trim().to_lowercase()).filter(|o| o.contains('@')).collect();
                    let owner = owners.first().cloned().unwrap_or_else(|| arg("owner"));
                    let node = create(control, &cell, &arg("title"), &owner, Some(arg("node")).filter(|n| !n.is_empty()).as_deref(), false)?;
                    let mut said = vec![format!("{cell} made on {node}")];
                    if owners.len() > 1 {
                        set(control, &cell, None, Some(&owners), None, None)?;
                        said.push(format!("owners {}", owners.join(", ")));
                    }
                    let mut la: BTreeMap<String, String> = BTreeMap::new();
                    for k in ["memory", "cpu", "storage_gb", "reads", "mails", "cap", "sources", "every", "billing", "free_until"] {
                        la.insert(k.to_string(), arg(k));
                    }
                    if !arg("billing").is_empty() || la.values().any(|v| !v.is_empty()) {
                        if arg("billing").is_empty() {
                            la.remove("billing");
                        }
                        said.push(limits(control, &cell, &la)?);
                    }
                    if !arg("note").is_empty() {
                        set(control, &cell, None, None, None, Some(&arg("note")))?;
                    }
                    // A first world, uploaded with the order: the server brings it in as an
                    // owner's import would be, a snapshot first.
                    let upload = ops_dir(control).join("uploads").join(format!("{cell}.tar.gz"));
                    if arg("archive") == "yes" && upload.exists() {
                        let n = register(control)?.nodes.get(&node).cloned().ok_or("no such node")?;
                        let r = on(&n, &["import", &cell, "--by", &owner], Some(&upload));
                        let _ = std::fs::remove_file(&upload);
                        said.push(r.map(|o| o.trim().to_string()).unwrap_or_else(|e| format!("not imported: {e}")));
                    }
                    if !owner.is_empty() && arg("welcome") != "no" {
                        welcome(control, &cell, &arg("title"), &owner)?;
                        said.push(format!("welcome mail to {owner}"));
                    }
                    Ok(said.join("; "))
                }
                "set" => {
                    let owners: Option<Vec<String>> = job["args"].get("owners").and_then(|v| v.as_str()).map(|s| s.split([',', '\n', ' ']).map(str::to_string).filter(|o| !o.trim().is_empty()).collect());
                    let opt = |k: &str| job["args"].get(k).and_then(|v| v.as_str()).map(str::to_string);
                    set(control, &cell, opt("title").as_deref(), owners.as_deref(), opt("domain").as_deref(), opt("note").as_deref())
                }
                "limits" => {
                    let la: BTreeMap<String, String> = job["args"].as_object().into_iter().flatten().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect();
                    limits(control, &cell, &la)
                }
                "snapshots" => snapshots(&cell).map(|l| J::Array(l).to_string()),
                "restore" if arg("confirm") == cell => restore_to(control, &cell, &arg("stamp")),
                "restore" => Err(format!("{cell}: not restored, its name was not typed to confirm")),
                "download" => download(control, &cell, &arg("stamp")),
                a @ ("keep" | "unkeep") => {
                    let mut r = register(control)?;
                    let e = r.cells.get_mut(&cell).ok_or("no such cell")?;
                    e.keep = a == "keep";
                    save_register(control, &r)?;
                    Ok(format!("{cell}: {}", if a == "keep" { "kept, not deleted" } else { "deleted on its day again" }))
                }
                "node-add" => node_add(control, &cell, &arg("host")),
                "purge" => purge(control, &cell, &arg("confirm")),
                "drain" => drain(control, &cell, true),
                "undrain" => drain(control, &cell, false),
                "upgrade-all" => {
                    let v = arg("version");
                    let cells: Vec<String> = r.cells.keys().cloned().collect();
                    Ok(cells.iter().map(|c| upgrade(control, c, &v).unwrap_or_else(|e| e)).collect::<Vec<_>>().join("\n"))
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
        let by = job["by"].as_str().unwrap_or("").to_string();
        let what = format!("done {}", job["action"].as_str().unwrap_or(""));
        let outcome = job["said"].as_str().map(|s| s.lines().next().unwrap_or("").to_string()).or_else(|| job["error"].as_str().map(|e| format!("failed: {e}"))).unwrap_or_default();
        if job["action"] != "snapshots" && job["action"] != "logs" {
            audit(control, &by, &what, &cell, &outcome);
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
| release <v> [--current] | start|stop|restart|snapshot|logs <cell> | node-add <name> --host <address> | drain <node> [--off] | purge <cell> --confirm <cell>";

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
        Some("purge") => {
            let cell = rest.first().ok_or("which cell?")?;
            println!("{}", purge(&control, cell, flag("--confirm").unwrap_or(""))?);
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
    fn a_cells_own_numbers_go_over_its_plans_and_only_those() {
        let plan = crate::billing::Plan { storage_gb: 2, reads: 25_000, mails: 1_000, cap: 50, every: "5m".into(), ..Default::default() };
        let q = Quota { reads: Some(5_000), cap: Some(0), ..Default::default() };
        let p = q.over(&plan);
        assert_eq!((p.storage_gb, p.reads, p.mails, p.cap, p.every.as_str()), (2, 5_000, 1_000, 0, "5m"));
        assert!(Quota::default().is_empty() && !q.is_empty());
    }

    #[test]
    fn a_cell_is_deleted_thirty_days_after_what_was_paid() {
        assert_eq!(delete_day("2026-10-31").as_deref(), Some("2026-11-30"));
        assert_eq!(delete_day("2026-12-15").as_deref(), Some("2027-01-14"));
        assert_eq!(delete_day("soon"), None);
    }

    #[test]
    fn maintenance_silences_the_alarms_of_what_it_covers_only() {
        let control = std::env::temp_dir().join(format!("zetlyn-mute-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&control);
        std::fs::create_dir_all(ops_dir(&control)).unwrap();
        let mut r = Register::default();
        r.cells.insert("acme".into(), CellEntry { node: "n1".into(), ..Default::default() });
        r.cells.insert("other".into(), CellEntry { node: "n2".into(), ..Default::default() });
        let problems: BTreeMap<String, String> = [("node:n1:disk", "full"), ("node:n2", "down"), ("cell:acme:down", "down"), ("cell:other:down", "down"), ("control:s3", "gone")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        // No notice: everything is said.
        assert_eq!(mute(&control, &r, problems.clone()).len(), 5);
        let notice = crate::maintenance::Notice { target: "node:n1".into(), announce_from: "2000-01-01T00:00:00Z".into(), from: "2000-01-01T00:00:00Z".into(), until: "2999-01-01T00:00:00Z".into(), ..Default::default() };
        std::fs::write(maintenance_path(&control), serde_json::to_vec(&notice).unwrap()).unwrap();
        let left: Vec<String> = mute(&control, &r, problems.clone()).into_keys().collect();
        assert_eq!(left, vec!["cell:other:down", "control:s3", "node:n2"]);
        // Everything under maintenance: nothing is said.
        let all = crate::maintenance::Notice { target: "all".into(), ..notice };
        std::fs::write(maintenance_path(&control), serde_json::to_vec(&all).unwrap()).unwrap();
        assert!(mute(&control, &r, problems).is_empty());
        let _ = std::fs::remove_dir_all(&control);
    }

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

/// Every alarm raised and cleared, for the admin pages' history.
fn history(control: &Path, what: &str, key: &str, text: &str) {
    let line = json!({ "at": crate::iso_stamp(crate::now()), "what": what, "key": key, "text": text });
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(ops_dir(control).join("alarms.jsonl")) {
        let _ = writeln!(f, "{line}");
    }
}
