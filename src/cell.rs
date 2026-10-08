//! One world, one cell, on whichever server it is placed: `zetlyn node`, run as root on a server
//! that holds cells (CELLS.md in zetlyn-ops).
//!
//! A cell is a directory and two systemd units. The directory is a hosting directory with one
//! world in it, under `/var/lib/private/zetlyn-cells/<name>`, owned by the cell's own dynamic user;
//! `zetlyn-cell@<name>` serves it on a port of its own, `zetlyn-cell-run@<name>` reads its sources
//! on a timer. What it may use is said in `/etc/zetlyn/cells/<name>.env` and a drop-in beside the
//! unit. Nothing else on the server is the cell's.
//!
//! What a cell is worth keeping is in S3, not on the server: `s3://<bucket>/cells/<name>/`, a
//! snapshot a day and one before every move, upgrade or removal, encrypted with the key every
//! server holds in `/etc/zetlyn/cells.key` before it leaves. A server is a place a cell happens to
//! run; moving one is a snapshot here and a restore there.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};

/// Where the cells' directories are, as root sees them: a dynamic user's state lives under private/.
pub const STATE: &str = "/var/lib/private/zetlyn-cells";
/// A file per cell saying its port, its version and whether it is to run.
pub const CONFIG: &str = "/etc/zetlyn/cells";
pub const RELEASES: &str = "/srv/zetlyn/releases";
pub const KEY: &str = "/etc/zetlyn/cells.key";
pub const NODE: &str = "/etc/zetlyn/node.yaml";
/// The bucket's credentials, root's alone, read by `node` itself: an SSH forced command and a timer
/// start with no environment of their own.
pub const S3_ENV: &str = "/etc/zetlyn/s3.env";
/// The routes this server's Caddy takes from the main server's: a path or a domain to a port.
pub const CADDY_ROUTES: &str = "/etc/caddy/node-cells.caddy";

/// What a server says about itself.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    /// Its name in the main server's register: `n2`.
    pub name: String,
    /// Where cells are kept: `s3://zetlyn`.
    pub bucket: String,
    /// The address every cell's links are made from: `https://zetlyn.com`.
    pub url: String,
    /// The first and the last port a cell may take.
    #[serde(default = "first_port")]
    pub first_port: u16,
    #[serde(default = "last_port")]
    pub last_port: u16,
}

fn first_port() -> u16 {
    2401
}
fn last_port() -> u16 {
    2999
}

pub fn node() -> Result<Node, String> {
    crate::yaml::read(Path::new(NODE)).map_err(|e| format!("{NODE}: {e}; is this a server for cells?"))
}

/// What a cell may run, written by the main server from its plan: `cell.yaml` in its directory.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Terms {
    /// Updated at all: false while it is not paid for.
    #[serde(default = "yes")]
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<usize>,
    /// The shortest cadence, `1h`; empty for none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub every: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mails: Option<u64>,
    /// Source reads this month at most: what the spending limit allows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reads: Option<u64>,
    /// May answer at a domain of its own.
    #[serde(default = "yes")]
    pub domain: bool,
    /// The plan as its owner sees it in the cell's settings, written by the main server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanShown>,
}

/// What a cell's owner is shown of its plan: what it includes, the spending limit, and the
/// month's storage so far as the main server counts it (megabyte-days).
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlanShown {
    pub title: String,
    pub storage_gb: u64,
    pub reads: u64,
    pub mails: u64,
    pub cap: u64,
    pub mb_days: u64,
}

fn yes() -> bool {
    true
}

impl Default for Terms {
    fn default() -> Terms {
        Terms { active: true, sources: None, every: String::new(), mails: None, reads: None, domain: true, plan: None }
    }
}

pub const TERMS: &str = "cell.yaml";

/// A cell's terms, where its directory has them: None for a hosting directory that is not a cell.
pub fn terms(dir: &Path) -> Option<Terms> {
    let p = dir.join(TERMS);
    p.exists().then(|| crate::yaml::read(&p).unwrap_or_default())
}

/// A cell's name: an organisation's, short enough for the user systemd makes of it (`zc-<name>`).
pub fn name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 28
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

fn dir_of(name: &str) -> PathBuf {
    Path::new(STATE).join(name)
}

fn env_of(name: &str) -> PathBuf {
    Path::new(CONFIG).join(format!("{name}.env"))
}

/// `PORT=2401`, `VERSION=0.3.62`, `STATE=running`: a cell's file, read back.
fn read_env(name: &str) -> BTreeMap<String, String> {
    std::fs::read_to_string(env_of(name))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

fn write_env(name: &str, env: &BTreeMap<String, String>) -> Result<(), String> {
    std::fs::create_dir_all(CONFIG).map_err(|e| e.to_string())?;
    let text: String = env.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    let path = env_of(name);
    let partial = path.with_extension("partial");
    std::fs::write(&partial, text).map_err(|e| format!("{}: {e}", partial.display()))?;
    std::fs::rename(&partial, &path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Every cell this server holds, by its file.
pub fn cells() -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(CONFIG)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_string_lossy().strip_suffix(".env").map(str::to_string))
        .filter(|n| name_ok(n))
        .collect();
    out.sort();
    out
}

fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program).args(args).output().map_err(|e| format!("{program}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(format!("{program} {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

fn systemctl(args: &[&str]) -> Result<String, String> {
    run("systemctl", args)
}

/// The bucket's credentials into this process's environment, from root's file.
fn load_s3_env() -> Result<(), String> {
    let text = std::fs::read_to_string(S3_ENV).map_err(|e| format!("{S3_ENV}: {e}"))?;
    for (k, v) in text.lines().filter_map(|l| l.trim().strip_prefix("export ").unwrap_or(l.trim()).split_once('=')) {
        std::env::set_var(k.trim(), v.trim().trim_matches('"'));
    }
    Ok(())
}

fn bucket(node: &Node) -> Result<crate::place::S3, String> {
    load_s3_env()?;
    let rest = node.bucket.strip_prefix("s3://").ok_or_else(|| format!("{}: not an s3:// address", node.bucket))?;
    crate::place::S3::new(rest)
}

// -- encryption ---------------------------------------------------------------------------------

const MAGIC: &[u8] = b"ZCELL1\n";
const CHUNK: usize = 1 << 20;

/// The cells' key, 32 bytes, as 64 hex characters in its file.
pub(crate) fn key() -> Result<ring::aead::LessSafeKey, String> {
    let text = std::fs::read_to_string(KEY).map_err(|e| format!("{KEY}: {e}"))?;
    key_from_hex(text.trim())
}

fn key_from_hex(hex: &str) -> Result<ring::aead::LessSafeKey, String> {
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| hex.get(i..i + 2).and_then(|b| u8::from_str_radix(b, 16).ok()))
        .collect::<Option<Vec<u8>>>()
        .filter(|b| b.len() == 32)
        .ok_or("the cells' key is not 64 hex characters")?;
    let unbound = ring::aead::UnboundKey::new(&ring::aead::AES_256_GCM, &bytes).map_err(|_| "not a key")?;
    Ok(ring::aead::LessSafeKey::new(unbound))
}

fn nonce(prefix: &[u8; 8], counter: u32) -> ring::aead::Nonce {
    let mut n = [0u8; 12];
    n[..8].copy_from_slice(prefix);
    n[8..].copy_from_slice(&counter.to_be_bytes());
    ring::aead::Nonce::assume_unique_for_key(n)
}

/// A file sealed in chunks of a megabyte: the magic, a random prefix for the nonces, then each
/// chunk's length and its ciphertext. Each chunk is bound to its place and to whether it is the
/// last, so chunks cannot be reordered, dropped or the file cut short without the open failing.
pub(crate) fn seal(key: &ring::aead::LessSafeKey, from: &Path, to: &Path) -> Result<(), String> {
    let mut prefix = [0u8; 8];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut prefix).map_err(|_| "no randomness")?;
    let mut input = std::fs::File::open(from).map_err(|e| format!("{}: {e}", from.display()))?;
    let mut out = std::io::BufWriter::new(std::fs::File::create(to).map_err(|e| format!("{}: {e}", to.display()))?);
    out.write_all(MAGIC).and_then(|_| out.write_all(&prefix)).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; CHUNK];
    let mut next = vec![0u8; CHUNK];
    let mut filled = fill(&mut input, &mut buf)?;
    let mut counter = 0u32;
    loop {
        let ahead = if filled == CHUNK { fill(&mut input, &mut next)? } else { 0 };
        let last = ahead == 0;
        let mut chunk = buf[..filled].to_vec();
        let aad = ring::aead::Aad::from(if last { b"last" } else { b"more" });
        key.seal_in_place_append_tag(nonce(&prefix, counter), aad, &mut chunk).map_err(|_| "sealing failed")?;
        out.write_all(&(chunk.len() as u32).to_be_bytes()).and_then(|_| out.write_all(&chunk)).map_err(|e| e.to_string())?;
        if last {
            break;
        }
        counter = counter.checked_add(1).ok_or("too large")?;
        std::mem::swap(&mut buf, &mut next);
        filled = ahead;
    }
    out.flush().map_err(|e| e.to_string())
}

fn fill(input: &mut impl Read, buf: &mut [u8]) -> Result<usize, String> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = input.read(&mut buf[filled..]).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    Ok(filled)
}

pub(crate) fn open(key: &ring::aead::LessSafeKey, from: &Path, to: &Path) -> Result<(), String> {
    let mut input = std::io::BufReader::new(std::fs::File::open(from).map_err(|e| format!("{}: {e}", from.display()))?);
    let mut head = vec![0u8; MAGIC.len() + 8];
    input.read_exact(&mut head).map_err(|_| "not a cell's snapshot")?;
    if &head[..MAGIC.len()] != MAGIC {
        return Err("not a cell's snapshot".into());
    }
    let mut prefix = [0u8; 8];
    prefix.copy_from_slice(&head[MAGIC.len()..]);
    let mut out = std::io::BufWriter::new(std::fs::File::create(to).map_err(|e| format!("{}: {e}", to.display()))?);
    let mut counter = 0u32;
    loop {
        let mut len = [0u8; 4];
        input.read_exact(&mut len).map_err(|_| "the snapshot ends before its last chunk")?;
        let len = u32::from_be_bytes(len) as usize;
        if len > CHUNK + 16 {
            return Err("a chunk larger than any this writes".into());
        }
        let mut chunk = vec![0u8; len];
        input.read_exact(&mut chunk).map_err(|_| "the snapshot ends inside a chunk")?;
        // Last or not is what the chunk was sealed as; trying both is how it says which.
        let mut tried = chunk.clone();
        let (plain, last) = match key.open_in_place(nonce(&prefix, counter), ring::aead::Aad::from(b"more"), &mut tried) {
            Ok(p) => (p.to_vec(), false),
            Err(_) => {
                let p = key
                    .open_in_place(nonce(&prefix, counter), ring::aead::Aad::from(b"last"), &mut chunk)
                    .map_err(|_| "the snapshot does not open with this key, or was changed")?;
                (p.to_vec(), true)
            }
        };
        out.write_all(&plain).map_err(|e| e.to_string())?;
        if last {
            let mut rest = [0u8; 1];
            if input.read(&mut rest).map_err(|e| e.to_string())? != 0 {
                return Err("bytes after the last chunk".into());
            }
            break;
        }
        counter = counter.checked_add(1).ok_or("too large")?;
    }
    out.flush().map_err(|e| e.to_string())
}

// -- snapshots ----------------------------------------------------------------------------------

pub(crate) fn stamp() -> String {
    crate::iso_stamp(crate::now()).replace([':', '-'], "")
}

/// Root wrote into a cell's directory (an export's database copies, a restore): what it wrote goes
/// back to whoever owns the directory, so the cell can still write it.
fn give_back(dir: &Path) {
    let _ = run("chown", &["-R", "--reference", &dir.to_string_lossy(), &dir.to_string_lossy()]);
}

/// A cell, consistent while it runs, sealed and in the bucket. Its stamp.
pub fn snapshot(name: &str, why: &str) -> Result<String, String> {
    let node = node()?;
    let dir = dir_of(name);
    if !dir.is_dir() {
        return Err(format!("{name}: no such cell on this server"));
    }
    let key = key()?;
    let s3 = bucket(&node)?;
    let at = stamp();
    let work = Path::new("/var/tmp").join(format!("zetlyn-snapshot-{name}-{at}"));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = (|| {
        let plain = work.join("cell.tar.gz");
        let files = crate::world::export(&dir, &plain)?;
        give_back(&dir);
        let sealed = work.join("cell.zcell");
        seal(&key, &plain, &sealed)?;
        std::fs::remove_file(&plain).ok();
        let size = s3.upload_private(&format!("cells/{name}/snapshots/{at}.zcell"), &sealed)?;
        let about = json!({ "cell": name, "stamp": at, "why": why, "files": files, "bytes": size, "node": node.name, "zetlyn": env!("CARGO_PKG_VERSION") });
        s3.put_private(&format!("cells/{name}/snapshots/{at}.json"), about.to_string().as_bytes())?;
        s3.put_private(&format!("cells/{name}/latest.json"), about.to_string().as_bytes())?;
        let mut env = read_env(name);
        env.insert("SNAPSHOT".into(), at.clone());
        if env_of(name).exists() {
            write_env(name, &env)?;
        }
        prune(&s3, name);
        Ok(at.clone())
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// The fourteen newest snapshots, and the newest of each of the last twelve months; the rest go.
pub(crate) fn prune(s3: &crate::place::S3, name: &str) {
    prune_under(s3, &format!("cells/{name}/snapshots"));
}

/// The same keeping, for any folder of snapshots: the cells' and the main server's own.
pub(crate) fn prune_under(s3: &crate::place::S3, folder: &str) {
    use crate::place::Place;
    let Ok(all) = s3.list(&format!("{folder}/")) else { return };
    let mut stamps: Vec<String> = all.iter().filter_map(|k| k.rsplit('/').next()?.strip_suffix(".zcell").map(str::to_string)).collect();
    stamps.sort();
    stamps.reverse();
    let mut keep: std::collections::BTreeSet<String> = stamps.iter().take(14).cloned().collect();
    let mut months = std::collections::BTreeSet::new();
    for s in &stamps {
        if months.len() >= 12 {
            break;
        }
        if months.insert(s.get(..6).unwrap_or("").to_string()) {
            keep.insert(s.clone());
        }
    }
    for s in stamps.iter().filter(|s| !keep.contains(*s)) {
        let _ = s3.delete(&format!("{folder}/{s}.zcell"));
        let _ = s3.delete(&format!("{folder}/{s}.json"));
    }
}

/// A cell from the bucket into this server, under its name, and started.
pub fn restore(name: &str, from: Option<&str>, port: Option<u16>, version: Option<&str>) -> Result<String, String> {
    let node = node()?;
    if env_of(name).exists() || dir_of(name).exists() {
        return Err(format!("{name}: already on this server"));
    }
    let key = key()?;
    let s3 = bucket(&node)?;
    let at = match from {
        Some(s) if s != "latest" => s.to_string(),
        _ => {
            use crate::place::Place;
            let latest: J = serde_json::from_slice(&s3.get(&format!("cells/{name}/latest.json"))?).map_err(|e| e.to_string())?;
            latest["stamp"].as_str().ok_or("no snapshot named as the latest")?.to_string()
        }
    };
    let work = Path::new("/var/tmp").join(format!("zetlyn-restore-{name}-{}", stamp()));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = (|| {
        let sealed = work.join("cell.zcell");
        s3.download(&format!("cells/{name}/snapshots/{at}.zcell"), &sealed)?;
        let plain = work.join("cell.tar.gz");
        open(&key, &sealed, &plain)?;
        std::fs::remove_file(&sealed).ok();
        std::fs::create_dir_all(STATE).map_err(|e| e.to_string())?;
        crate::world::import(&plain, &dir_of(name), Some(&node.url), None)?;
        let version = version.map(str::to_string).unwrap_or_else(|| current_version().unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string()));
        // The snapshot it came from is its latest: it needs no other until a day has passed.
        let mut env = read_env(name);
        env.insert("SNAPSHOT".into(), at.clone());
        write_env(name, &env)?;
        install(name, port, &version)?;
        Ok(at.clone())
    })();
    let _ = std::fs::remove_dir_all(&work);
    if result.is_err() && !env_of(name).exists() {
        let _ = std::fs::remove_dir_all(dir_of(name));
    }
    result
}

// -- the units ----------------------------------------------------------------------------------

/// The release `/srv/zetlyn/bin/zetlyn` points at: what a new cell runs unless told otherwise.
fn current_version() -> Option<String> {
    let target = std::fs::read_link("/srv/zetlyn/bin/zetlyn").ok()?;
    target.parent()?.file_name().map(|n| n.to_string_lossy().to_string())
}

fn free_port(node: &Node) -> Result<u16, String> {
    let taken: std::collections::BTreeSet<u16> = cells().iter().filter_map(|c| read_env(c).get("PORT")?.parse().ok()).collect();
    (node.first_port..=node.last_port).find(|p| !taken.contains(p)).ok_or_else(|| "no port left on this server".to_string())
}

/// A cell's file, its limits and its units, started. The directory is already there.
fn install(name: &str, port: Option<u16>, version: &str) -> Result<(), String> {
    let node = node()?;
    if !Path::new(RELEASES).join(version).join("zetlyn").exists() {
        return Err(format!("release {version} is not on this server: `zetlyn node install {version}`"));
    }
    let port = match port {
        Some(p) => p,
        None => free_port(&node)?,
    };
    let mut env = read_env(name);
    env.insert("PORT".into(), port.to_string());
    env.insert("VERSION".into(), version.to_string());
    env.entry("STATE".into()).or_insert_with(|| "running".into());
    write_env(name, &env)?;
    ensure_mail_token(name)?;
    systemctl(&["daemon-reload"])?;
    start(name)?;
    sync_routes()?;
    Ok(())
}

/// Memory and processor for a cell, as a drop-in beside its units: `512M`, `100%`.
pub fn limit(name: &str, memory: &str, cpu: &str) -> Result<(), String> {
    let ok = |s: &str, units: &[char]| !s.is_empty() && s.trim_end_matches(units).chars().all(|c| c.is_ascii_digit());
    if !ok(memory, &['K', 'M', 'G']) || !ok(cpu, &['%']) {
        return Err("--memory like 512M or 2G, --cpu like 100%".into());
    }
    for unit in ["zetlyn-cell", "zetlyn-cell-run"] {
        let d = PathBuf::from(format!("/etc/systemd/system/{unit}@{name}.service.d"));
        std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
        std::fs::write(d.join("limits.conf"), format!("[Service]\nMemoryMax={memory}\nCPUQuota={cpu}\n")).map_err(|e| e.to_string())?;
    }
    systemctl(&["daemon-reload"])?;
    if read_env(name).get("STATE").map(String::as_str) == Some("running") {
        systemctl(&["restart", &format!("zetlyn-cell@{name}")])?;
    }
    Ok(())
}

pub fn start(name: &str) -> Result<(), String> {
    let mut env = read_env(name);
    env.insert("STATE".into(), "running".into());
    write_env(name, &env)?;
    systemctl(&["enable", "--now", &format!("zetlyn-cell@{name}"), &format!("zetlyn-cell-run@{name}.timer")])?;
    Ok(())
}

pub fn stop(name: &str) -> Result<(), String> {
    let mut env = read_env(name);
    env.insert("STATE".into(), "stopped".into());
    write_env(name, &env)?;
    systemctl(&["disable", "--now", &format!("zetlyn-cell@{name}"), &format!("zetlyn-cell-run@{name}.timer"), &format!("zetlyn-cell-run@{name}.service")])?;
    Ok(())
}

/// A new cell: an empty world under its name, its owner, its terms, started.
pub fn create(name: &str, title: &str, owner: &str, version: Option<&str>) -> Result<u16, String> {
    if !name_ok(name) {
        return Err(format!("{name}: a cell's name is at most 28 lower case letters, digits and hyphens"));
    }
    let node = node()?;
    if env_of(name).exists() || dir_of(name).exists() {
        return Err(format!("{name}: already on this server"));
    }
    let dir = dir_of(name);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let made = (|| {
        std::fs::write(dir.join(crate::account::WORKSPACE), format!("url: {}\n", node.url)).map_err(|e| e.to_string())?;
        crate::app::make_org(&dir, name, title)?;
        if !owner.is_empty() {
            crate::app::set_member(&dir, name, owner, Some("owner"))?;
        }
        set_terms(name, &Terms::default())
    })();
    if let Err(e) = made {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e);
    }
    let version = version.map(str::to_string).or_else(current_version).unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    install(name, None, &version)?;
    Ok(read_env(name).get("PORT").and_then(|p| p.parse().ok()).unwrap_or(0))
}

pub fn set_terms(name: &str, t: &Terms) -> Result<(), String> {
    let dir = dir_of(name);
    let path = dir.join(TERMS);
    std::fs::write(&path, crate::yaml::to_string(t)?).map_err(|e| format!("{}: {e}", path.display()))?;
    give_back(&dir);
    Ok(())
}

/// A cell gone from this server, after a snapshot says where it went, unless the snapshot is
/// waived because one was just taken.
pub fn remove(name: &str, snapshot_first: bool) -> Result<(), String> {
    if !env_of(name).exists() && !dir_of(name).exists() {
        return Err(format!("{name}: not on this server"));
    }
    if snapshot_first && dir_of(name).is_dir() {
        snapshot(name, "removed")?;
    }
    let _ = stop(name);
    let _ = std::fs::remove_file(env_of(name));
    for unit in ["zetlyn-cell", "zetlyn-cell-run"] {
        let _ = std::fs::remove_dir_all(format!("/etc/systemd/system/{unit}@{name}.service.d"));
    }
    let _ = std::fs::remove_dir_all(dir_of(name));
    let _ = std::fs::remove_file(Path::new("/var/lib/zetlyn-cells").join(name));
    systemctl(&["daemon-reload"])?;
    sync_routes()
}

pub fn set_version(name: &str, version: &str) -> Result<(), String> {
    if !Path::new(RELEASES).join(version).join("zetlyn").exists() {
        return Err(format!("release {version} is not on this server"));
    }
    let mut env = read_env(name);
    env.insert("VERSION".into(), version.to_string());
    write_env(name, &env)?;
    ensure_mail_token(name)?;
    if env.get("STATE").map(String::as_str) == Some("running") {
        systemctl(&["restart", &format!("zetlyn-cell@{name}")])?;
    }
    Ok(())
}

/// A release onto this server from standard input, checked against its hash; with `--current`,
/// the one `zetlyn node` itself and every new cell run.
pub fn install_release(version: &str, sha: &str, current: bool) -> Result<(), String> {
    if crate::world::version(version).is_none() {
        return Err(format!("{version}: not a version"));
    }
    let mut bytes = Vec::new();
    std::io::stdin().read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let got = crate::place::sha256(&bytes);
    if !crate::place::same(&got, sha.trim_start_matches("sha256:")) {
        return Err(format!("the release arrived as {got}, not {sha}"));
    }
    let d = Path::new(RELEASES).join(version);
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let partial = d.join("zetlyn.partial");
    std::fs::write(&partial, &bytes).map_err(|e| e.to_string())?;
    run("chmod", &["0755", &partial.to_string_lossy()])?;
    std::fs::rename(&partial, d.join("zetlyn")).map_err(|e| e.to_string())?;
    if current {
        std::fs::create_dir_all("/srv/zetlyn/bin").map_err(|e| e.to_string())?;
        let link = Path::new("/srv/zetlyn/bin/zetlyn.next");
        let _ = std::fs::remove_file(link);
        std::os::unix::fs::symlink(d.join("zetlyn"), link).map_err(|e| e.to_string())?;
        std::fs::rename(link, "/srv/zetlyn/bin/zetlyn").map_err(|e| e.to_string())?;
    }
    Ok(())
}

// -- what a server says about its cells ---------------------------------------------------------

fn show(unit: &str, property: &str) -> String {
    systemctl(&["show", unit, "-p", property, "--value"]).unwrap_or_default().trim().to_string()
}

/// A cell's own address answered on its port, as the main server's request would be.
fn probe(port: &str, name: &str) -> u16 {
    let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", port.parse().unwrap_or(0))) else { return 0 };
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(10)));
    if write!(s, "GET /{name}/ HTTP/1.1\r\nHost: zetlyn.com\r\nConnection: close\r\n\r\n").is_err() {
        return 0;
    }
    let mut head = [0u8; 32];
    let n = s.read(&mut head).unwrap_or(0);
    String::from_utf8_lossy(&head[..n]).split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0)
}

fn bytes_under(dir: &Path) -> u64 {
    run("du", &["-sb", &dir.to_string_lossy()]).ok().and_then(|o| o.split_whitespace().next()?.parse().ok()).unwrap_or(0)
}

/// Everything the main server asks about this server and its cells, as JSON.
pub fn status() -> J {
    let node = node().unwrap_or_default();
    let mut list = Vec::new();
    for name in cells() {
        let env = read_env(&name);
        let unit = format!("zetlyn-cell@{name}");
        let run_unit = format!("zetlyn-cell-run@{name}.service");
        let port = env.get("PORT").cloned().unwrap_or_default();
        let dir = dir_of(&name);
        let domain = crate::account::Site::load(&dir.join("orgs").join(&name)).domain.trim().to_lowercase();
        let t = terms(&dir).unwrap_or_default();
        list.push(json!({
            "name": name,
            "state": env.get("STATE"),
            "port": port,
            "version": env.get("VERSION"),
            "snapshot": env.get("SNAPSHOT"),
            "active": show(&unit, "ActiveState"),
            "since": show(&unit, "ActiveEnterTimestamp"),
            "restarts": show(&unit, "NRestarts"),
            "memory": show(&unit, "MemoryCurrent").parse::<u64>().ok(),
            "memory_max": show(&unit, "MemoryMax").parse::<u64>().ok(),
            "run_result": show(&run_unit, "Result"),
            "run_finished": show(&run_unit, "ExecMainExitTimestamp"),
            "answers": if env.get("STATE").map(String::as_str) == Some("running") { probe(&port, &name) } else { 0 },
            "bytes": bytes_under(&dir),
            "usage": json!({
                "month": crate::usage::month(),
                "reads": crate::usage::used(&dir, crate::usage::READS, &crate::usage::month()),
                "mails": crate::usage::used(&dir, crate::usage::MAILS, &crate::usage::month()),
                "previous": crate::usage::previous_month(),
                "previous_reads": crate::usage::used(&dir, crate::usage::READS, &crate::usage::previous_month()),
                "previous_mails": crate::usage::used(&dir, crate::usage::MAILS, &crate::usage::previous_month()),
            }),
            "domain": (!domain.is_empty() && t.domain).then_some(domain),
            "terms": t,
        }));
    }
    let free = run("df", &["-B1", "--output=avail,size", STATE.trim_end_matches("/zetlyn-cells")]).unwrap_or_default();
    let nums: Vec<u64> = free.split_whitespace().filter_map(|w| w.parse().ok()).collect();
    let mem: BTreeMap<String, u64> = std::fs::read_to_string("/proc/meminfo")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.to_string(), v.split_whitespace().next()?.parse::<u64>().ok()? * 1024))
        })
        .collect();
    json!({
        "node": node.name,
        "at": crate::iso_stamp(crate::now()),
        "zetlyn": env!("CARGO_PKG_VERSION"),
        "current": current_version(),
        "disk_free": nums.first(),
        "disk_size": nums.get(1),
        "memory_total": mem.get("MemTotal"),
        "memory_available": mem.get("MemAvailable"),
        "load": std::fs::read_to_string("/proc/loadavg").unwrap_or_default().split_whitespace().next().unwrap_or("").to_string(),
        "cells": list,
    })
}

// -- this server's routes -----------------------------------------------------------------------

/// This server's Caddy, told which path and which domain is which cell's port, and reloaded only
/// when that changed.
pub fn sync_routes() -> Result<(), String> {
    let mut text = String::from("# Written by `zetlyn node`: which cell answers which path or domain. Not edited by hand.\n");
    for name in cells() {
        let env = read_env(&name);
        if env.get("STATE").map(String::as_str) != Some("running") {
            continue;
        }
        let Some(port) = env.get("PORT") else { continue };
        let dir = dir_of(&name);
        let domain = crate::account::Site::load(&dir.join("orgs").join(&name)).domain.trim().to_lowercase();
        if !domain.is_empty() && terms(&dir).unwrap_or_default().domain && domain.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
            text.push_str(&format!(
                "@domain_{n} header X-Forwarded-Host {domain}\nhandle @domain_{n} {{\n\treverse_proxy 127.0.0.1:{port}\n}}\n",
                n = name.replace('-', "_")
            ));
        }
        text.push_str(&format!(
            "@cell_{n} path /{name} /{name}/* /worlds/{name} /worlds/{name}/*\nhandle @cell_{n} {{\n\treverse_proxy 127.0.0.1:{port}\n}}\n",
            n = name.replace('-', "_")
        ));
    }
    if std::fs::read_to_string(CADDY_ROUTES).ok().as_deref() == Some(text.as_str()) {
        return Ok(());
    }
    let partial = format!("{CADDY_ROUTES}.partial");
    std::fs::write(&partial, &text).map_err(|e| e.to_string())?;
    std::fs::rename(&partial, CADDY_ROUTES).map_err(|e| e.to_string())?;
    systemctl(&["reload", "caddy"]).map(|_| ())
}

/// Once a minute: every cell that is to run runs, the routes say what is there, and the cell whose
/// last snapshot is oldest, if older than a day, is taken now. One snapshot a minute at most, so a
/// server of many cells does not take them all at once.
pub fn sync() -> Result<(), String> {
    for name in cells() {
        let env = read_env(&name);
        // A cell made before the relay gets its token, and is started again to see it.
        if ensure_mail_token(&name).unwrap_or(false) && env.get("STATE").map(String::as_str) == Some("running") {
            let _ = systemctl(&["restart", &format!("zetlyn-cell@{name}")]);
            continue;
        }
        if env.get("STATE").map(String::as_str) == Some("running") && show(&format!("zetlyn-cell@{name}"), "ActiveState") != "active" {
            let _ = systemctl(&["start", &format!("zetlyn-cell@{name}"), &format!("zetlyn-cell-run@{name}.timer")]);
        }
    }
    sync_routes()?;
    // An import an owner asked for, one a minute.
    if let Some(name) = cells().into_iter().find(|n| dir_of(n).join(INCOMING).join(ASKED).exists()) {
        match import_into(&name) {
            Ok(n) => println!("{name}: imported, {n} files"),
            Err(e) => eprintln!("{name}: import: {e}"),
        }
        return Ok(());
    }
    let day_ago = crate::iso_stamp(crate::now() - 86_400).replace([':', '-'], "");
    let due = cells()
        .into_iter()
        .filter(|n| dir_of(n).is_dir())
        .map(|n| (read_env(&n).get("SNAPSHOT").cloned().unwrap_or_default(), n))
        .filter(|(s, _)| *s < day_ago)
        .min();
    if let Some((_, name)) = due {
        snapshot(&name, "daily")?;
    }
    Ok(())
}

// -- a world brought into a cell ----------------------------------------------------------------

/// Where an owner's upload waits for the server, inside the cell's directory.
const INCOMING: &str = "incoming";
const ASKED: &str = "import.json";
const ARCHIVE: &str = "world.tar.gz";
/// What the last import came to, for the settings page.
pub const LAST_IMPORT: &str = "last-import.json";

/// An owner's upload, taken inside the cell: checked to be an export, kept, and asked of the
/// server, which brings it in within a minute. What to tell the owner.
pub fn take_upload(body: &mut dyn Read, by: &str) -> Result<String, String> {
    let cell = crate::usage::cell_dir().ok_or("Only a world on Zetlyn Managed imports from here. On your own machine: zetlyn world import.")?;
    let incoming = cell.join(INCOMING);
    if incoming.join(ASKED).exists() {
        return Err("An import is waiting already; it starts within a minute.".into());
    }
    std::fs::create_dir_all(&incoming).map_err(|e| e.to_string())?;
    let gb = terms(&cell).and_then(|t| t.plan).map(|p| p.storage_gb).filter(|g| *g > 0).unwrap_or(2);
    let limit = gb << 30;
    let partial = incoming.join(format!("{ARCHIVE}.partial"));
    let result = (|| {
        let mut out = std::fs::File::create(&partial).map_err(|e| e.to_string())?;
        let n = std::io::copy(&mut body.take(limit + 1), &mut out).map_err(|e| format!("the upload broke off: {e}"))?;
        if n > limit {
            return Err(format!("Larger than the {gb} GB the plan includes. Write to hello@zetlyn.com and we bring it in for you."));
        }
        if n == 0 {
            return Err("Nothing arrived.".into());
        }
        let files = crate::world::check_export(&partial)?;
        std::fs::rename(&partial, incoming.join(ARCHIVE)).map_err(|e| e.to_string())?;
        let asked = json!({ "by": by, "at": crate::iso_stamp(crate::now()), "files": files, "bytes": n });
        std::fs::write(incoming.join(ASKED), asked.to_string()).map_err(|e| e.to_string())?;
        Ok(format!("Received, {files} files. Within a minute this world is replaced by it: a snapshot of it is taken first, and a mail says when it is done."))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

/// Whether an import waits, and what the last one came to: for the settings page.
pub fn import_state() -> (bool, Option<J>) {
    let Some(cell) = crate::usage::cell_dir() else { return (false, None) };
    let incoming = cell.join(INCOMING);
    (incoming.join(ASKED).exists(), std::fs::read(incoming.join(LAST_IMPORT)).ok().and_then(|b| serde_json::from_slice(&b).ok()))
}

/// An import a cell asked for, done as root on its server: a snapshot, the cell stopped, its world
/// set aside and the upload put in its place, the cell started again. Where the upload does not
/// import, the world set aside goes back.
fn import_into(name: &str) -> Result<usize, String> {
    let dir = dir_of(name);
    let incoming = dir.join(INCOMING);
    let asked: J = std::fs::read(incoming.join(ASKED)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let by = asked["by"].as_str().unwrap_or("").to_string();
    let world = dir.join("orgs").join(name);
    let running = read_env(name).get("STATE").map(String::as_str) == Some("running");
    let result = (|| {
        snapshot(name, "before import")?;
        let _ = systemctl(&["stop", &format!("zetlyn-cell-run@{name}.timer"), &format!("zetlyn-cell-run@{name}.service"), &format!("zetlyn-cell@{name}")]);
        let aside = dir.join(format!("replaced-{}", stamp()));
        std::fs::rename(&world, &aside).map_err(|e| format!("{}: {e}", world.display()))?;
        match crate::world::import_hosted(&incoming.join(ARCHIVE), &world) {
            Ok(n) => {
                let _ = std::fs::remove_dir_all(&aside);
                Ok(n)
            }
            Err(e) => {
                let _ = std::fs::remove_dir_all(&world);
                let _ = std::fs::rename(&aside, &world);
                Err(e)
            }
        }
    })();
    let _ = std::fs::remove_file(incoming.join(ARCHIVE));
    let _ = std::fs::remove_file(incoming.join(ASKED));
    let last = match &result {
        Ok(n) => json!({ "at": crate::iso_stamp(crate::now()), "by": by, "ok": true, "files": n }),
        Err(e) => json!({ "at": crate::iso_stamp(crate::now()), "by": by, "ok": false, "error": e }),
    };
    let _ = std::fs::write(incoming.join(LAST_IMPORT), last.to_string());
    give_back(&dir);
    if running {
        let _ = systemctl(&["start", &format!("zetlyn-cell@{name}"), &format!("zetlyn-cell-run@{name}.timer")]);
    }
    if by.contains('@') {
        let url = node().map(|n| format!("{}/{name}/", n.url.trim_end_matches('/'))).unwrap_or_default();
        let text = match &result {
            Ok(n) => format!("Hello,\n\nthe world you uploaded is now the one at {url}: {n} files, its sources, trackers and their history.\n\nWhat was there before is kept as a snapshot; write to hello@zetlyn.com within fourteen days if you want it back.\n\nBest regards,\nThe Zetlyn team\n\n--\nZetlyn · https://zetlyn.com · hello@zetlyn.com\n"),
            Err(e) => format!("Hello,\n\nthe world you uploaded for {url} could not be brought in:\n\n  {e}\n\nNothing changed there; it runs as before. Write to hello@zetlyn.com if you need a hand.\n\nBest regards,\nThe Zetlyn team\n\n--\nZetlyn · https://zetlyn.com · hello@zetlyn.com\n"),
        };
        let subject = if result.is_ok() { "Your world is imported" } else { "Your import did not go through" };
        if let Err(e) = server_mail(&by, subject, &text) {
            eprintln!("{name}: import mail to {by}: {e}");
        }
    }
    result
}

/// A mail from the server itself, through its mailer.
fn server_mail(to: &str, subject: &str, body: &str) -> Result<(), String> {
    let central = std::env::var("ZETLYN_MAIL").unwrap_or_else(|_| "/etc/zetlyn/mail.yaml".into());
    match crate::yaml::read::<crate::account::Mail>(Path::new(&central))? {
        crate::account::Mail { smtp: Some(s), .. } => crate::mail::send(&s, to, subject, body),
        _ => Err(format!("{central} names no SMTP server")),
    }
}

// -- mail, sent for the cells -------------------------------------------------------------------

/// The key a server makes each cell's mail token from, root's alone.
pub const MAIL_KEY: &str = "/etc/zetlyn/mail.key";
/// Where a server's mail relay listens, for its own cells only.
pub const MAIL_RELAY: &str = "127.0.0.1:2525";

/// The mail key, made the first time it is asked for.
fn mail_key() -> Result<ring::hmac::Key, String> {
    let path = Path::new(MAIL_KEY);
    if !path.exists() {
        let mut raw = [0u8; 32];
        ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut raw).map_err(|_| "no randomness")?;
        let partial = path.with_extension("partial");
        std::fs::write(&partial, crate::key::hex(&raw)).map_err(|e| format!("{}: {e}", partial.display()))?;
        run("chmod", &["0600", &partial.to_string_lossy()])?;
        std::fs::rename(&partial, path).map_err(|e| e.to_string())?;
    }
    let hex = std::fs::read_to_string(path).map_err(|e| format!("{MAIL_KEY}: {e}"))?;
    Ok(ring::hmac::Key::new(ring::hmac::HMAC_SHA256, hex.trim().as_bytes()))
}

/// What a cell shows the relay to send as itself: the key's mark of its name.
fn mail_token(key: &ring::hmac::Key, name: &str) -> String {
    crate::key::hex(ring::hmac::sign(key, format!("mail:{name}").as_bytes()).as_ref())
}

/// A cell's file given its mail token where it has none. Whether it was given one.
fn ensure_mail_token(name: &str) -> Result<bool, String> {
    let mut env = read_env(name);
    let token = mail_token(&mail_key()?, name);
    if env.get("ZETLYN_MAIL_TOKEN") == Some(&token) {
        return Ok(false);
    }
    env.insert("ZETLYN_MAIL_TOKEN".into(), token);
    write_env(name, &env)?;
    Ok(true)
}

/// The relay, `zetlyn node mail`: a cell's mail, sent through the server's mailer
/// (/etc/zetlyn/mail.yaml), so no cell holds the mailer's password. A cell names itself and shows
/// its token; what it sends is counted for it and stops at its month's limit.
pub fn serve_mail() -> Result<(), String> {
    let key = std::sync::Arc::new(mail_key()?);
    let server = tiny_http::Server::http(MAIL_RELAY).map_err(|e| format!("{MAIL_RELAY}: {e}"))?;
    println!("mail relay on {MAIL_RELAY}");
    for mut request in server.incoming_requests() {
        let key = key.clone();
        std::thread::spawn(move || {
            let (code, said) = relay(&key, &mut request);
            if code != 200 {
                eprintln!("mail relay: {code} {said}");
            }
            let _ = request.respond(tiny_http::Response::from_string(said).with_status_code(code));
        });
    }
    Ok(())
}

fn relay(key: &ring::hmac::Key, request: &mut tiny_http::Request) -> (u16, String) {
    if request.method() != &tiny_http::Method::Post || request.url() != "/send" {
        return (404, "POST /send".into());
    }
    let header = |name: &str| request.headers().iter().find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name)).map(|h| h.value.as_str().to_string()).unwrap_or_default();
    let name = header("X-Zetlyn-Cell");
    let token = header("Authorization").strip_prefix("Bearer ").unwrap_or("").to_string();
    if !name_ok(&name) || !env_of(&name).exists() || !crate::place::same(&token, &mail_token(key, &name)) {
        return (403, "not a cell of this server".into());
    }
    let mut body = String::new();
    if std::io::Read::read_to_string(&mut std::io::Read::take(request.as_reader(), 512 << 10), &mut body).is_err() {
        return (400, "not text".into());
    }
    let Ok(m) = serde_json::from_str::<J>(&body) else { return (400, "not JSON".into()) };
    let (to, subject, text) = (m["to"].as_str().unwrap_or(""), m["subject"].as_str().unwrap_or(""), m["body"].as_str().unwrap_or(""));
    if to.len() > 320 || subject.len() > 300 || subject.contains(['\r', '\n']) {
        return (400, "not a mail to send".into());
    }
    let dir = dir_of(&name);
    let month = crate::usage::month();
    if let Some(cap) = terms(&dir).and_then(|t| t.mails) {
        if crate::usage::used(&dir, crate::usage::MAILS, &month) >= cap {
            return (429, "this world has sent every mail its month allows".into());
        }
    }
    let central = std::env::var("ZETLYN_MAIL").unwrap_or_else(|_| "/etc/zetlyn/mail.yaml".into());
    let smtp = match crate::yaml::read::<crate::account::Mail>(Path::new(&central)) {
        Ok(crate::account::Mail { smtp: Some(s), .. }) => s,
        Ok(_) => return (503, format!("{central} names no SMTP server")),
        Err(e) => return (503, format!("{central}: {e}")),
    };
    match crate::mail::send(&smtp, to, subject, text) {
        Ok(()) => {
            crate::usage::count_in(&dir, crate::usage::MAILS);
            (200, "sent".into())
        }
        Err(e) => (502, e),
    }
}

/// A cell's mail, handed to its server's relay: what a cell's `Site::send` does instead of
/// speaking SMTP itself.
pub fn relay_mail(to: &str, subject: &str, body: &str) -> Result<(), String> {
    let name = std::env::var("ZETLYN_CELL").map_err(|_| "ZETLYN_CELL is not set: is this a cell?")?;
    let token = std::env::var("ZETLYN_MAIL_TOKEN").map_err(|_| "this cell has no mail token yet; it gets one when its server next syncs")?;
    let at = std::env::var("ZETLYN_MAIL_RELAY").unwrap_or_else(|_| format!("http://{MAIL_RELAY}"));
    let agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(90))).http_status_as_error(false).build().new_agent();
    let mut answer = agent
        .post(&format!("{at}/send"))
        .header("X-Zetlyn-Cell", &name)
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .send(json!({ "to": to, "subject": subject, "body": body }).to_string())
        .map_err(|e| format!("the mail relay: {e}"))?;
    let code = answer.status().as_u16();
    if code == 200 {
        return Ok(());
    }
    let said = answer.body_mut().read_to_string().unwrap_or_default();
    Err(format!("the mail relay: {code} {said}"))
}

// -- the command, and the one way the main server reaches it ------------------------------------

pub const USAGE: &str = "zetlyn node status [--json] | sync | create <cell> --title … --owner … [--version v] | start|stop|restart <cell> \
| snapshot <cell> [--why …] | restore <cell> [--from <stamp>|latest] [--port p] [--version v] | remove <cell> [--no-snapshot] \
| terms <cell> --active yes|no [--sources n] [--every 1h] [--mails n] [--reads n] [--domain yes|no] \
[--plan <title> --plan-storage-gb n --plan-reads n --plan-mails n --plan-cap n --mb-days n] | mail | limit <cell> --memory 512M --cpu 100% \
| version <cell> <v> | install <v> --sha256 <hash> [--current] | logs <cell> [--lines n] | key-check | ca";

pub fn command(args: &[String]) -> Result<(), String> {
    let rest = crate::positional(args, 2);
    let cell = || -> Result<String, String> {
        let n = rest.first().ok_or("which cell?")?.to_string();
        name_ok(&n).then_some(n.clone()).ok_or_else(|| format!("{n}: not a cell's name"))
    };
    let flag = |f: &str| crate::flag(args, f);
    match args.get(1).map(String::as_str) {
        Some("status") => {
            let s = status();
            if args.iter().any(|a| a == "--json") {
                println!("{s}");
            } else {
                println!("{} · {} cells · load {}", s["node"].as_str().unwrap_or("?"), s["cells"].as_array().map_or(0, Vec::len), s["load"].as_str().unwrap_or(""));
                for c in s["cells"].as_array().into_iter().flatten() {
                    println!("  {:<24} {:<8} {:<8} port {:<5} {:<8} answers {}", c["name"].as_str().unwrap_or(""), c["state"].as_str().unwrap_or(""), c["active"].as_str().unwrap_or(""), c["port"].as_str().unwrap_or(""), c["version"].as_str().unwrap_or(""), c["answers"]);
                }
            }
            Ok(())
        }
        Some("sync") => sync(),
        Some("mail") => serve_mail(),
        Some("create") => {
            let n = cell()?;
            // A title crosses SSH as one word, its spaces as no-break spaces.
            let title = flag("--title").unwrap_or(&n).replace('\u{a0}', " ");
            let port = create(&n, &title, flag("--owner").unwrap_or(""), flag("--version"))?;
            println!("{n} on port {port}");
            Ok(())
        }
        Some("start") => start(&cell()?).and_then(|_| sync_routes()),
        Some("stop") => stop(&cell()?).and_then(|_| sync_routes()),
        Some("restart") => systemctl(&["restart", &format!("zetlyn-cell@{}", cell()?)]).map(|_| ()),
        Some("snapshot") => {
            let at = snapshot(&cell()?, flag("--why").unwrap_or("asked"))?;
            println!("{at}");
            Ok(())
        }
        Some("restore") => {
            let n = cell()?;
            let port = flag("--port").map(|p| p.parse::<u16>().map_err(|_| "--port is a number")).transpose()?;
            let at = restore(&n, flag("--from"), port, flag("--version"))?;
            println!("{n} restored from {at}");
            Ok(())
        }
        Some("remove") => remove(&cell()?, !args.iter().any(|a| a == "--no-snapshot")),
        Some("terms") => {
            let n = cell()?;
            let yes_no = |f: &str, default: bool| flag(f).map(|v| v == "yes").unwrap_or(default);
            let t = Terms {
                active: yes_no("--active", true),
                sources: flag("--sources").and_then(|v| v.parse().ok()),
                every: flag("--every").unwrap_or("").to_string(),
                mails: flag("--mails").and_then(|v| v.parse().ok()),
                reads: flag("--reads").and_then(|v| v.parse().ok()),
                domain: yes_no("--domain", true),
                // A plan's title crosses SSH as one word, its spaces as no-break spaces.
                plan: flag("--plan").map(|title| {
                    let n = |f: &str| flag(f).and_then(|v| v.parse().ok()).unwrap_or(0);
                    PlanShown { title: title.replace('\u{a0}', " "), storage_gb: n("--plan-storage-gb"), reads: n("--plan-reads"), mails: n("--plan-mails"), cap: n("--plan-cap"), mb_days: n("--mb-days") }
                }),
            };
            set_terms(&n, &t)?;
            sync_routes()
        }
        Some("limit") => limit(&cell()?, flag("--memory").unwrap_or(""), flag("--cpu").unwrap_or("")),
        Some("version") => set_version(&cell()?, rest.get(1).ok_or("which version?")?),
        Some("install") => install_release(rest.first().ok_or("which version?")?, flag("--sha256").ok_or("--sha256 of the release")?, args.iter().any(|a| a == "--current")),
        Some("logs") => {
            let n = cell()?;
            let lines = flag("--lines").and_then(|v| v.parse::<u32>().ok()).unwrap_or(200).min(5000).to_string();
            print!("{}", run("journalctl", &["-u", &format!("zetlyn-cell@{n}"), "-u", &format!("zetlyn-cell-run@{n}"), "-n", &lines, "--no-pager", "-o", "short-iso"])?);
            Ok(())
        }
        // The certificate this server's Caddy signs its internal name with, for the main server to trust.
        Some("ca") => {
            print!("{}", std::fs::read_to_string("/var/lib/caddy/.local/share/caddy/pki/authorities/local/root.crt").map_err(|e| format!("Caddy's local root: {e}"))?);
            Ok(())
        }
        // The key opens what it sealed: a server set up with the wrong one finds out here, not in
        // the middle of a restore.
        Some("key-check") => {
            let k = key()?;
            let dir = Path::new("/var/tmp").join(format!("zetlyn-key-check-{}", stamp()));
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let r = (|| {
                std::fs::write(dir.join("a"), b"zetlyn").map_err(|e| e.to_string())?;
                seal(&k, &dir.join("a"), &dir.join("b"))?;
                open(&k, &dir.join("b"), &dir.join("c"))?;
                let digest = crate::place::sha256(std::fs::read_to_string(KEY).map_err(|e| e.to_string())?.trim().as_bytes());
                println!("the key opens what it seals; its fingerprint is {}", digest.get(..16).unwrap_or(""));
                Ok(())
            })();
            let _ = std::fs::remove_dir_all(&dir);
            r
        }
        // The main server's key may run this and nothing else (`command=` in authorized_keys): the
        // words it asked for, checked against what `node` takes, never a shell.
        Some("ssh") => {
            let asked = std::env::var("SSH_ORIGINAL_COMMAND").unwrap_or_default();
            let words: Vec<String> = asked.split_whitespace().map(str::to_string).collect();
            let allowed = ["status", "ca", "sync", "create", "start", "stop", "restart", "snapshot", "restore", "remove", "terms", "limit", "version", "install", "logs", "key-check"];
            if words.first().map(String::as_str) != Some("node") || !words.get(1).is_some_and(|w| allowed.contains(&w.as_str())) {
                return Err(format!("only `node <{}>` is taken here", allowed.join("|")));
            }
            if words.iter().any(|w| w.chars().any(|c| c.is_control())) {
                return Err("control characters in the command".into());
            }
            command(&words)
        }
        _ => Err(USAGE.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_opens_with_its_key_and_not_when_cut_or_changed() {
        let dir = std::env::temp_dir().join(format!("zetlyn-seal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let key = key_from_hex(&"ab".repeat(32)).unwrap();
        // Two chunks and a bit, so the order and the last mark both count.
        let plain: Vec<u8> = (0..(CHUNK * 2 + 777)).map(|i| (i % 251) as u8).collect();
        std::fs::write(dir.join("p"), &plain).unwrap();
        seal(&key, &dir.join("p"), &dir.join("s")).unwrap();
        open(&key, &dir.join("s"), &dir.join("o")).unwrap();
        assert_eq!(std::fs::read(dir.join("o")).unwrap(), plain);
        // Another key does not open it.
        assert!(open(&key_from_hex(&"cd".repeat(32)).unwrap(), &dir.join("s"), &dir.join("x")).is_err());
        // Cut after the first chunk: that chunk was sealed as one with more to come.
        let sealed = std::fs::read(dir.join("s")).unwrap();
        let first = MAGIC.len() + 8 + 4 + CHUNK + 16;
        std::fs::write(dir.join("cut"), &sealed[..first]).unwrap();
        assert!(open(&key, &dir.join("cut"), &dir.join("x")).is_err());
        // One byte changed.
        let mut changed = sealed.clone();
        changed[first + 100] ^= 1;
        std::fs::write(dir.join("changed"), &changed).unwrap();
        assert!(open(&key, &dir.join("changed"), &dir.join("x")).is_err());
        // An empty file seals and opens too.
        std::fs::write(dir.join("e"), b"").unwrap();
        seal(&key, &dir.join("e"), &dir.join("es")).unwrap();
        open(&key, &dir.join("es"), &dir.join("eo")).unwrap();
        assert!(std::fs::read(dir.join("eo")).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_name_is_one_systemd_can_make_a_user_of() {
        assert!(name_ok("acme") && name_ok("reading-circle") && name_ok("a1"));
        assert!(!name_ok("") && !name_ok("-a") && !name_ok("a-") && !name_ok("Acme") && !name_ok("a/b") && !name_ok(&"a".repeat(29)));
    }
}
