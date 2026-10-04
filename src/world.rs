//! A world of one's own: one workspace on a domain of its own, run by its owners, kept current,
//! backed up and upgraded with nobody watching (FEDERATION.md, M11).
//!
//! `zetlyn world up <domain> --owner <address>` makes one on an empty Ubuntu machine: a system
//! user, the binary, the workspace with its address, Caddy with a certificate, the mail its sign-in
//! links go out by, and three units: the world itself, a daily backup and a daily upgrade. Every
//! step looks before it acts, so running it again changes only what is missing or different, and
//! `--dry-run` says what it would do and does none of it.
//!
//! What lives where, on the machine:
//!
//! ```text
//! /usr/local/bin/zetlyn                 the binary, replaced by `world upgrade`
//! /srv/zetlyn/world/                    the workspace: sources, trackers, accounts, keys
//! /srv/zetlyn/world/.zetlyn/            the key it publishes and signs with, so it moves with it
//! /srv/zetlyn/backups/world-<stamp>.tar.gz
//! /etc/zetlyn/smtp-password             0640 root:zetlyn, where a mailer needs one
//! /etc/caddy/Caddyfile                  or /etc/caddy/zetlyn-world.caddy, imported from it
//! /etc/systemd/system/zetlyn-world.service, zetlyn-backup.{service,timer}, zetlyn-upgrade.{service,timer}
//! ```

use std::io::Read;
use std::path::{Path, PathBuf};

const USAGE: &str = "zetlyn world init <dir> --domain <domain> --owner <address> [--title …]
zetlyn world up <domain> --owner <address> [--title …] [--smtp host[:port] --smtp-user … --mail-from …]
                [--port 2500] [--dry-run] [--root <prefix>] [--no-services]
zetlyn world serve <workspace> [--addr 127.0.0.1:2500]
zetlyn world export <workspace> --to <file.tar.gz>
zetlyn world backup <workspace> <dir> [--keep 14]
zetlyn world upgrade [--check] [--restart]";

/// Where the releases are, for `world upgrade`.
const RELEASES: &str = "https://api.github.com/repos/zetlynhq/zetlyn/releases/latest";
const DOWNLOAD: &str = "https://github.com/zetlynhq/zetlyn/releases/download";

pub fn command(args: &[String]) -> Result<(), String> {
    match args.get(1).map(String::as_str) {
        Some("serve") => crate::app::world_serve(args),
        Some("init") => {
            let dir = PathBuf::from(crate::positional(args, 2).first().ok_or("which directory?")?.as_str());
            let w = Wanted::from_args(args, crate::flag(args, "--domain").ok_or("--domain, where the world will be")?)?;
            let wrote = init(&dir, &w)?;
            println!("{} {}", if wrote { "made" } else { "already there:" }, dir.join(crate::account::WORKSPACE).display());
            Ok(())
        }
        Some("up") => {
            let domain = crate::positional(args, 2).first().ok_or("which domain? `zetlyn world up prices.example --owner you@example.org`")?.to_string();
            let w = Wanted::from_args(args, &domain)?;
            let plan = Plan {
                root: PathBuf::from(crate::flag(args, "--root").unwrap_or("/")),
                dry: args.iter().any(|a| a == "--dry-run"),
                services: !args.iter().any(|a| a == "--no-services"),
            };
            up(&w, &plan)
        }
        Some("export") => {
            let dir = PathBuf::from(crate::positional(args, 2).first().ok_or("which workspace?")?.as_str());
            let to = PathBuf::from(crate::flag(args, "--to").ok_or("--to <file.tar.gz>")?);
            let n = export(&dir, &to)?;
            println!("{n} files in {}", to.display());
            Ok(())
        }
        Some("backup") => {
            let rest = crate::positional(args, 2);
            let dir = PathBuf::from(rest.first().ok_or("which workspace?")?.as_str());
            let into = PathBuf::from(rest.get(1).ok_or("into which directory?")?.as_str());
            let keep: usize = crate::flag(args, "--keep").and_then(|k| k.parse().ok()).unwrap_or(14);
            let (file, n, gone) = backup(&dir, &into, keep)?;
            println!("{n} files in {}{}", file.display(), if gone == 0 { String::new() } else { format!("; {gone} older ones removed") });
            Ok(())
        }
        Some("upgrade") => upgrade(args.iter().any(|a| a == "--check"), args.iter().any(|a| a == "--restart")),
        _ => Err(USAGE.into()),
    }
}

// ---------------------------------------------------------------------------------------------
// What a world is made of, before it is made.

/// What the person asked for.
#[derive(Debug, Clone)]
pub struct Wanted {
    pub domain: String,
    pub owner: String,
    pub title: String,
    pub port: u16,
    pub smtp: Option<(String, u16)>,
    pub smtp_user: String,
    pub mail_from: String,
}

impl Wanted {
    fn from_args(args: &[String], domain: &str) -> Result<Wanted, String> {
        let domain = domain.trim().trim_start_matches("https://").trim_end_matches('/').to_lowercase();
        if domain.is_empty() || domain.contains('/') || !domain.contains('.') || domain.contains(char::is_whitespace) {
            return Err(format!("{domain}: a domain, as prices.example"));
        }
        let owner = crate::flag(args, "--owner").ok_or("--owner, the address the first sign-in link goes to")?.trim().to_lowercase();
        if !owner.contains('@') {
            return Err(format!("{owner}: an address"));
        }
        let smtp = match crate::flag(args, "--smtp") {
            Some(s) => {
                let (host, port) = match s.rsplit_once(':') {
                    Some((h, p)) => (h.to_string(), p.parse().map_err(|_| format!("{s}: host:port"))?),
                    None => (s.to_string(), 587),
                };
                Some((host, port))
            }
            None => None,
        };
        Ok(Wanted {
            title: crate::flag(args, "--title").map(str::to_string).unwrap_or_else(|| domain.clone()),
            port: crate::flag(args, "--port").and_then(|p| p.parse().ok()).unwrap_or(2500),
            smtp,
            smtp_user: crate::flag(args, "--smtp-user").unwrap_or("").to_string(),
            mail_from: crate::flag(args, "--mail-from").map(str::to_string).unwrap_or_else(|| format!("Zetlyn <noreply@{domain}>")),
            owner,
            domain,
        })
    }

    pub fn url(&self) -> String {
        format!("https://{}", self.domain)
    }
}

/// The world's own workspace.yaml.
pub fn workspace_yaml(w: &Wanted, password_file: &str) -> String {
    let q = |s: &str| serde_json::to_string(s).unwrap_or_default();
    let mut y = format!(
        "# A world of its own, made by `zetlyn world up`. Its owners sign in at {url}/signin.\ntitle: {}\nurl: {}\ncontact: {}\nowners:\n- {}\n",
        q(&w.title),
        q(&w.url()),
        q(&w.owner),
        q(&w.owner),
        url = w.url(),
    );
    if let Some((host, port)) = &w.smtp {
        y.push_str(&format!(
            "mail:\n  smtp:\n    host: {}\n    port: {port}\n    user: {}\n    password_file: {}\n    from: {}\n",
            q(host),
            q(&w.smtp_user),
            q(password_file),
            q(&w.mail_from),
        ));
    }
    y
}

/// Caddy's part: the domain, its certificate, and the world behind it.
pub fn caddy_block(w: &Wanted) -> String {
    format!(
        "# The world at {domain}, made by `zetlyn world up`.\n{domain} {{\n\tencode zstd gzip\n\treverse_proxy 127.0.0.1:{port}\n\theader {{\n\t\tX-Content-Type-Options nosniff\n\t\tReferrer-Policy strict-origin-when-cross-origin\n\t\t-Server\n\t}}\n}}\n",
        domain = w.domain,
        port = w.port,
    )
}

/// The three units, each a file name and what it says.
pub fn units(w: &Wanted, paths: &Paths) -> Vec<(&'static str, String)> {
    let bin = paths.bin_on_machine();
    let world = paths.world_on_machine();
    let backups = paths.backups_on_machine();
    vec![
        (
            "zetlyn-world.service",
            format!(
                "[Unit]\nDescription=The world at {domain}\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nUser=zetlyn\nGroup=zetlyn\nEnvironment=HOME=/srv/zetlyn\nEnvironment=ZETLYN_HOME={world}/.zetlyn\nExecStart={bin} world serve {world} --addr 127.0.0.1:{port}\nRestart=on-failure\nRestartSec=5\nNoNewPrivileges=true\nProtectSystem=strict\nProtectHome=true\nReadWritePaths=/srv/zetlyn\nPrivateTmp=true\n\n[Install]\nWantedBy=multi-user.target\n",
                domain = w.domain,
                port = w.port,
            ),
        ),
        (
            "zetlyn-backup.service",
            format!(
                "[Unit]\nDescription=A copy of the world at {domain}, kept for a fortnight\n\n[Service]\nType=oneshot\nUser=zetlyn\nGroup=zetlyn\nEnvironment=HOME=/srv/zetlyn\nExecStart={bin} world backup {world} {backups} --keep 14\n",
                domain = w.domain,
            ),
        ),
        ("zetlyn-backup.timer", "[Unit]\nDescription=Back up the world every day\n\n[Timer]\nOnCalendar=daily\nRandomizedDelaySec=1h\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n".into()),
        (
            "zetlyn-upgrade.service",
            format!("[Unit]\nDescription=The next release of Zetlyn, where there is one\nAfter=network-online.target\n\n[Service]\nType=oneshot\nExecStart={bin} world upgrade --restart\n"),
        ),
        ("zetlyn-upgrade.timer", "[Unit]\nDescription=Look for a new release every day\n\n[Timer]\nOnCalendar=daily\nRandomizedDelaySec=6h\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n".into()),
    ]
}

/// Where everything goes, under a prefix that is `/` on the machine and anything else for a try.
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    fn at(&self, on_machine: &str) -> PathBuf {
        self.root.join(on_machine.trim_start_matches('/'))
    }
    pub fn bin(&self) -> PathBuf {
        self.at(&self.bin_on_machine())
    }
    pub fn world(&self) -> PathBuf {
        self.at(&self.world_on_machine())
    }
    pub fn backups(&self) -> PathBuf {
        self.at(&self.backups_on_machine())
    }
    pub fn password(&self) -> PathBuf {
        self.at("/etc/zetlyn/smtp-password")
    }
    pub fn caddyfile(&self) -> PathBuf {
        self.at("/etc/caddy/Caddyfile")
    }
    pub fn caddy_own(&self) -> PathBuf {
        self.at("/etc/caddy/zetlyn-world.caddy")
    }
    pub fn unit(&self, name: &str) -> PathBuf {
        self.at(&format!("/etc/systemd/system/{name}"))
    }
    fn bin_on_machine(&self) -> String {
        "/usr/local/bin/zetlyn".into()
    }
    fn world_on_machine(&self) -> String {
        "/srv/zetlyn/world".into()
    }
    fn backups_on_machine(&self) -> String {
        "/srv/zetlyn/backups".into()
    }
}

/// A workspace for a world, where there is none yet. True when it was made now.
pub fn init(dir: &Path, w: &Wanted) -> Result<bool, String> {
    let file = dir.join(crate::account::WORKSPACE);
    if file.exists() {
        return Ok(false);
    }
    for d in ["sources", "trackers"] {
        std::fs::create_dir_all(dir.join(d)).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&file, workspace_yaml(w, "/etc/zetlyn/smtp-password")).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(true)
}

// ---------------------------------------------------------------------------------------------
// Making it, on a machine.

pub struct Plan {
    /// `/` on the machine. Anything else writes the files under it and runs nothing.
    pub root: PathBuf,
    pub dry: bool,
    pub services: bool,
}

impl Plan {
    fn machine(&self) -> bool {
        self.root == Path::new("/")
    }
}

/// One thing done, or said that it would be.
fn step(plan: &Plan, what: &str, act: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    if plan.dry {
        println!("  would {what}");
        return Ok(());
    }
    println!("  {what}");
    act()
}

fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new(program).args(args).output().map_err(|e| format!("{program}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{program} {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A file with this content, written only where it is missing or says something else. Whether it
/// was written.
fn ensure_file(path: &Path, content: &str, mode: u32) -> Result<bool, String> {
    if std::fs::read_to_string(path).is_ok_and(|held| held == content) {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("zetlyn-new");
    std::fs::write(&tmp, content).map_err(|e| format!("{}: {e}", tmp.display()))?;
    set_mode(&tmp, mode);
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(true)
}

fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    let _ = (path, mode);
}

/// What the Caddyfile is, as far as a world goes: the package's own placeholder (ours to replace),
/// already ours, or somebody else's (ours goes beside it and is imported).
#[derive(Debug, PartialEq)]
pub enum CaddyIs {
    Placeholder,
    Ours,
    Theirs,
}

pub fn caddy_is(held: Option<&str>, w: &Wanted) -> CaddyIs {
    let Some(text) = held else { return CaddyIs::Placeholder };
    if text.contains(&format!("{} {{", w.domain)) {
        return CaddyIs::Ours;
    }
    // The file Ubuntu's package ships: a site on :80 serving its welcome page, and comments.
    let meaningful: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
    let placeholder = meaningful.iter().any(|l| l.starts_with(":80")) && meaningful.iter().all(|l| [":80 {", "root * /usr/share/caddy", "file_server", "}"].contains(l) || l.starts_with("reverse_proxy") || l.starts_with("php_fastcgi"));
    if meaningful.is_empty() || placeholder { CaddyIs::Placeholder } else { CaddyIs::Theirs }
}

pub fn up(w: &Wanted, plan: &Plan) -> Result<(), String> {
    let paths = Paths { root: plan.root.clone() };
    let machine = plan.machine();
    println!("A world at {} for {}{}", w.url(), w.owner, if plan.dry { ", as it would be made" } else { "" });
    if machine && !plan.dry {
        #[cfg(unix)]
        {
            let uid = run("id", &["-u"]).unwrap_or_default();
            if uid.trim() != "0" {
                return Err("as root: it makes a user, installs a service and writes Caddy's configuration".into());
            }
        }
        if run("which", &["apt-get"]).is_err() {
            return Err("this is for Ubuntu or Debian: there is no apt-get here".into());
        }
    }

    // The user the world runs as, and its home.
    if machine && run("id", &["zetlyn"]).is_err() {
        step(plan, "make the system user zetlyn, at home in /srv/zetlyn", || {
            run("useradd", &["--system", "--home-dir", "/srv/zetlyn", "--create-home", "--shell", "/usr/sbin/nologin", "zetlyn"]).map(|_| ())
        })?;
    }

    // The binary: this one, where the units name it.
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let bin = paths.bin();
    let same = std::fs::read(&bin).ok() == std::fs::read(&exe).ok();
    if !same {
        step(plan, &format!("put this zetlyn ({}) at {}", env!("CARGO_PKG_VERSION"), bin.display()), || {
            if let Some(d) = bin.parent() {
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            let tmp = bin.with_extension("zetlyn-new");
            std::fs::copy(&exe, &tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
            set_mode(&tmp, 0o755);
            std::fs::rename(&tmp, &bin).map_err(|e| format!("{}: {e}", bin.display()))
        })?;
    }

    // The workspace, with its address and its owner.
    let world = paths.world();
    if !world.join(crate::account::WORKSPACE).exists() {
        step(plan, &format!("make the workspace in {}, owned by {}", world.display(), w.owner), || {
            init(&world, w).map(|_| ())
        })?;
    }
    if !paths.backups().is_dir() {
        step(plan, &format!("make {} for the daily copies", paths.backups().display()), || {
            std::fs::create_dir_all(paths.backups()).map_err(|e| e.to_string())
        })?;
    }
    if machine {
        step(plan, "give /srv/zetlyn to zetlyn", || run("chown", &["-R", "zetlyn:zetlyn", "/srv/zetlyn"]).map(|_| ()))?;
    }

    // The mailer's password, where there is a mailer and a password was given.
    if w.smtp.is_some() {
        match std::env::var("SMTP_PASSWORD").ok().filter(|p| !p.is_empty()) {
            Some(password) => {
                step(plan, &format!("keep the mailer's password in {}, readable by zetlyn alone", paths.password().display()), || {
                    ensure_file(&paths.password(), &password, 0o640)?;
                    if machine {
                        run("chown", &["root:zetlyn", &paths.password().to_string_lossy()])?;
                    }
                    Ok(())
                })?;
            }
            None if !paths.password().exists() => println!("  (no SMTP_PASSWORD in the environment, so the mailer is named but has no password yet: put it in {})", paths.password().display()),
            None => {}
        }
    } else {
        println!("  (no --smtp: sign-in links are written to the world's log, `journalctl -u zetlyn-world`, until a mailer is named in its workspace.yaml)");
    }

    // Caddy, and the world behind it.
    if machine && run("which", &["caddy"]).is_err() {
        step(plan, "install Caddy from the distribution", || {
            run("apt-get", &["update", "-q"])?;
            run("apt-get", &["install", "-y", "-q", "caddy"]).map(|_| ())
        })?;
    }
    let held = std::fs::read_to_string(paths.caddyfile()).ok();
    match caddy_is(held.as_deref(), w) {
        CaddyIs::Ours => {}
        CaddyIs::Placeholder => {
            step(plan, &format!("make {} the world's", paths.caddyfile().display()), || {
                ensure_file(&paths.caddyfile(), &format!("{{\n\temail {}\n}}\n\n{}", w.owner, caddy_block(w)), 0o644).map(|_| ())
            })?;
        }
        CaddyIs::Theirs => {
            step(plan, &format!("put the world beside the sites already in {}, in {}", paths.caddyfile().display(), paths.caddy_own().display()), || {
                ensure_file(&paths.caddy_own(), &caddy_block(w), 0o644)?;
                let import = format!("import {}", "/etc/caddy/zetlyn-world.caddy");
                let text = held.clone().unwrap_or_default();
                if !text.lines().any(|l| l.trim() == import) {
                    ensure_file(&paths.caddyfile(), &format!("{}\n{import}\n", text.trim_end()), 0o644)?;
                }
                Ok(())
            })?;
        }
    }

    // The units.
    for (name, content) in units(w, &paths) {
        let path = paths.unit(name);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(content.as_str()) {
            step(plan, &format!("write {}", path.display()), || ensure_file(&path, &content, 0o644).map(|_| ()))?;
        }
    }
    if machine && plan.services {
        step(plan, "start the world, its backup and its upgrade, and reload Caddy", || {
            run("systemctl", &["daemon-reload"])?;
            run("systemctl", &["enable", "--now", "zetlyn-world.service", "zetlyn-backup.timer", "zetlyn-upgrade.timer"])?;
            run("systemctl", &["restart", "zetlyn-world.service"])?;
            run("systemctl", &["reload-or-restart", "caddy"]).map(|_| ())
        })?;
        if !plan.dry {
            answered(w)?;
        }
    }
    if plan.dry {
        println!("Nothing was done. Without --dry-run, as root, it is.");
    } else if machine {
        println!("\nThe world is at {}. Sign in as {} at {}/signin.", w.url(), w.owner, w.url());
    } else {
        println!("\nWritten under {}; nothing was started.", plan.root.display());
    }
    Ok(())
}

/// The world asked from outside until it answers over https, which is when Caddy has its
/// certificate: a minute or two the first time.
fn answered(w: &Wanted) -> Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(10))).http_status_as_error(false).build().into();
    let url = format!("{}/", w.url());
    for _ in 0..36 {
        if let Ok(r) = agent.get(&url).call() {
            if r.status().is_success() {
                println!("  {url} answers");
                return Ok(());
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
    Err(format!(
        "{url} did not answer in three minutes. Does {} resolve to this machine, and are ports 80 and 443 open? `journalctl -u caddy` says what Caddy tried",
        w.domain
    ))
}

// ---------------------------------------------------------------------------------------------
// One archive with everything in it.

/// Every file of a workspace in one `.tar.gz`, under `world/`: each SQLite database as a copy taken
/// in one transaction (`VACUUM INTO`), so a world in use exports as one moment of itself. How many
/// files it holds.
pub fn export(dir: &Path, to: &Path) -> Result<usize, String> {
    if !dir.join(crate::account::WORKSPACE).exists() {
        return Err(format!("{}: not a workspace, there is no {}", dir.display(), crate::account::WORKSPACE));
    }
    if let Some(d) = to.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let scratch = std::env::temp_dir().join(format!("zetlyn-export-{}-{}", std::process::id(), crate::now()));
    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    let partial = to.with_extension("partial");
    let result = (|| {
        let file = std::fs::File::create(&partial).map_err(|e| format!("{}: {e}", partial.display()))?;
        set_mode(&partial, 0o600);
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(file, flate2::Compression::default()));
        let mut n = 0usize;
        for path in files(dir)? {
            let rel = path.strip_prefix(dir).map_err(|e| e.to_string())?;
            let name = rel.to_string_lossy().replace('\\', "/");
            if name.ends_with("-wal") || name.ends_with("-shm") || name.ends_with("-journal") || name.ends_with(".arriving") || name.ends_with(".partial") {
                continue;
            }
            let inside = format!("world/{name}");
            if is_sqlite(&path) {
                let copy = scratch.join(format!("{n}.db"));
                let db = rusqlite::Connection::open(&path).map_err(|e| format!("{name}: {e}"))?;
                db.execute("vacuum into ?1", rusqlite::params![copy.to_string_lossy()]).map_err(|e| format!("{name}: {e}"))?;
                tar.append_path_with_name(&copy, &inside).map_err(|e| format!("{name}: {e}"))?;
                let _ = std::fs::remove_file(&copy);
            } else {
                tar.append_path_with_name(&path, &inside).map_err(|e| format!("{name}: {e}"))?;
            }
            n += 1;
        }
        let about = serde_json::json!({
            "zetlyn": env!("CARGO_PKG_VERSION"),
            "exported_at": crate::iso_stamp(crate::now()),
            "url": crate::account::Site::for_workspace(dir).url,
            "files": n,
        });
        let text = serde_json::to_vec_pretty(&about).unwrap_or_default();
        let mut header = tar::Header::new_gnu();
        header.set_size(text.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(crate::now().max(0) as u64);
        header.set_cksum();
        tar.append_data(&mut header, "EXPORT.json", text.as_slice()).map_err(|e| e.to_string())?;
        tar.into_inner().map_err(|e| e.to_string())?.finish().map_err(|e| e.to_string())?;
        std::fs::rename(&partial, to).map_err(|e| format!("{}: {e}", to.display()))?;
        Ok(n)
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

/// Every file under a directory, in a stable order.
fn files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?.flatten() {
            let p = e.path();
            let Ok(kind) = e.file_type() else { continue };
            if kind.is_dir() {
                stack.push(p);
            } else if kind.is_file() {
                out.push(p);
            }
        }
    }
    out.sort();
    Ok(out)
}

fn is_sqlite(path: &Path) -> bool {
    let mut head = [0u8; 16];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && &head == b"SQLite format 3\0"
}

/// An export into a directory, and the oldest beyond `keep` removed. The file, how many it holds,
/// and how many were removed.
pub fn backup(dir: &Path, into: &Path, keep: usize) -> Result<(PathBuf, usize, usize), String> {
    let stamp = crate::iso_stamp(crate::now()).replace([':', '-'], "");
    let file = into.join(format!("world-{stamp}.tar.gz"));
    let n = export(dir, &file)?;
    let mut held: Vec<PathBuf> = std::fs::read_dir(into)
        .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("world-") && n.to_string_lossy().ends_with(".tar.gz"))).collect())
        .unwrap_or_default();
    held.sort();
    let mut gone = 0;
    while held.len() > keep.max(1) {
        let old = held.remove(0);
        if std::fs::remove_file(&old).is_ok() {
            gone += 1;
        }
    }
    Ok((file, n, gone))
}

// ---------------------------------------------------------------------------------------------
// The next release.

/// `0.3.21` from `v0.3.21`, as three numbers.
pub fn version(s: &str) -> Option<(u64, u64, u64)> {
    let mut parts = s.trim().trim_start_matches('v').split('.').map(|p| p.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

fn upgrade(check: bool, restart: bool) -> Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(concat!("zetlyn/", env!("CARGO_PKG_VERSION")))
        .timeout_global(Some(std::time::Duration::from_secs(120)))
        .build()
        .into();
    let latest: serde_json::Value = agent
        .get(RELEASES)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("{RELEASES}: {e}"))?
        .body_mut()
        .read_json()
        .map_err(|e| format!("{RELEASES}: {e}"))?;
    let tag = latest["tag_name"].as_str().ok_or("the latest release names no tag")?.to_string();
    let (have, there) = (version(env!("CARGO_PKG_VERSION")), version(&tag));
    if there.is_none() || there <= have {
        println!("{} is current: the latest release is {tag}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if check {
        println!("{tag} is out; this is {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if !(cfg!(target_os = "linux") && cfg!(target_arch = "x86_64")) {
        return Err(format!("{tag} is out. Upgrading in place is for Linux on x86_64; elsewhere, the install script: curl -fsSL https://zetlyn.com/install.sh | sh"));
    }
    let asset = "zetlyn-linux-x86_64.tar.gz";
    let get = |name: &str| -> Result<Vec<u8>, String> {
        let url = format!("{DOWNLOAD}/{tag}/{name}");
        let mut body = Vec::new();
        agent.get(&url).call().map_err(|e| format!("{url}: {e}"))?.body_mut().as_reader().read_to_end(&mut body).map_err(|e| format!("{url}: {e}"))?;
        Ok(body)
    };
    let archive = get(asset)?;
    let sums = String::from_utf8(get("SHA256SUMS")?).map_err(|_| "SHA256SUMS is not text")?;
    let want = sums.lines().find_map(|l| l.strip_suffix(&format!(" {asset}")).map(|h| h.trim().to_string())).ok_or("SHA256SUMS does not name the archive")?;
    if crate::place::sha256(&archive) != want {
        return Err(format!("{asset} does not match its checksum, so nothing was installed"));
    }
    let binary = unpack_binary(&archive)?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let tmp = exe.with_extension("zetlyn-new");
    std::fs::write(&tmp, &binary).map_err(|e| format!("{}: {e}", tmp.display()))?;
    set_mode(&tmp, 0o755);
    std::fs::rename(&tmp, &exe).map_err(|e| format!("{}: {e}", exe.display()))?;
    println!("{} is {tag} now", exe.display());
    if restart {
        run("systemctl", &["try-restart", "zetlyn-world.service"])?;
        println!("zetlyn-world restarted");
    }
    Ok(())
}

/// The `zetlyn` inside a release's archive.
pub fn unpack_binary(archive: &[u8]) -> Result<Vec<u8>, String> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let is_it = entry.path().ok().is_some_and(|p| p.file_name().is_some_and(|n| n == "zetlyn"));
        if is_it {
            let mut out = Vec::new();
            entry.read_to_end(&mut out).map_err(|e| e.to_string())?;
            return Ok(out);
        }
    }
    Err("the archive holds no zetlyn".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wanted() -> Wanted {
        let args: Vec<String> = ["world", "up", "prices.example", "--owner", "Ann@Example.org", "--smtp", "smtp.example:465", "--smtp-user", "ann", "--title", "Car prices"].iter().map(|s| s.to_string()).collect();
        Wanted::from_args(&args, "prices.example").unwrap()
    }

    #[test]
    fn what_is_asked_for_is_checked_before_anything_is_made() {
        let w = wanted();
        assert_eq!((w.domain.as_str(), w.owner.as_str(), w.port, w.smtp.clone()), ("prices.example", "ann@example.org", 2500, Some(("smtp.example".to_string(), 465))));
        let args = |more: &[&str]| ["world", "up"].iter().chain(more).map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(Wanted::from_args(&args(&["--owner", "a@b.c"]), "localhost").is_err(), "a domain has a dot");
        assert!(Wanted::from_args(&args(&["--owner", "a@b.c"]), "https://prices.example/x").is_err());
        assert!(Wanted::from_args(&args(&[]), "prices.example").unwrap_err().contains("--owner"));
        assert_eq!(Wanted::from_args(&args(&["--owner", "a@b.c"]), "https://Prices.Example/").unwrap().domain, "prices.example");
    }

    #[test]
    fn the_workspace_names_its_address_its_owner_and_its_mailer() {
        let y = workspace_yaml(&wanted(), "/etc/zetlyn/smtp-password");
        let site: crate::account::Site = serde_saphyr::from_str(&y).unwrap();
        assert_eq!(site.url, "https://prices.example");
        assert_eq!(site.owners, vec!["ann@example.org".to_string()]);
        assert_eq!(site.title, "Car prices");
        let smtp = site.mail.smtp.unwrap();
        assert_eq!((smtp.host.as_str(), smtp.port, smtp.password_file.as_str()), ("smtp.example", 465, "/etc/zetlyn/smtp-password"));
        // No mailer asked for: none named, and the links go to the log.
        let mut w = wanted();
        w.smtp = None;
        let site: crate::account::Site = serde_saphyr::from_str(&workspace_yaml(&w, "x")).unwrap();
        assert!(site.mail.smtp.is_none());
    }

    #[test]
    fn caddy_is_ours_to_replace_only_where_it_is_the_packages_placeholder() {
        let w = wanted();
        let ubuntu = "# The Caddyfile is an easy way to configure your Caddy web server.\n:80 {\n\t# Set this path to your site's directory.\n\troot * /usr/share/caddy\n\n\t# Enable the static file server.\n\tfile_server\n}\n";
        assert_eq!(caddy_is(Some(ubuntu), &w), CaddyIs::Placeholder);
        assert_eq!(caddy_is(None, &w), CaddyIs::Placeholder);
        assert_eq!(caddy_is(Some(&caddy_block(&w)), &w), CaddyIs::Ours);
        assert_eq!(caddy_is(Some("shop.example {\n\treverse_proxy :3000\n}\n"), &w), CaddyIs::Theirs);
        assert!(caddy_block(&w).contains("prices.example {") && caddy_block(&w).contains("reverse_proxy 127.0.0.1:2500"));
    }

    #[test]
    fn up_under_a_prefix_writes_everything_runs_nothing_and_again_changes_nothing() {
        let root = std::env::temp_dir().join(format!("zetlyn-world-up-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut w = wanted();
        w.smtp = None;
        let plan = Plan { root: root.clone(), dry: false, services: false };
        up(&w, &plan).unwrap();
        let paths = Paths { root: root.clone() };
        assert!(paths.bin().exists());
        assert!(paths.world().join("workspace.yaml").exists() && paths.world().join("sources").is_dir());
        assert!(std::fs::read_to_string(paths.caddyfile()).unwrap().contains("prices.example {"));
        for name in ["zetlyn-world.service", "zetlyn-backup.service", "zetlyn-backup.timer", "zetlyn-upgrade.service", "zetlyn-upgrade.timer"] {
            assert!(paths.unit(name).exists(), "{name}");
        }
        let unit = std::fs::read_to_string(paths.unit("zetlyn-world.service")).unwrap();
        assert!(unit.contains("ExecStart=/usr/local/bin/zetlyn world serve /srv/zetlyn/world --addr 127.0.0.1:2500"), "{unit}");
        assert!(unit.contains("ZETLYN_HOME=/srv/zetlyn/world/.zetlyn"));
        // Changed by hand, the workspace stays as it is; the rest is put back.
        std::fs::write(paths.world().join("workspace.yaml"), "title: mine\nurl: https://prices.example\nowners: [ann@example.org]\n").unwrap();
        up(&w, &plan).unwrap();
        assert!(std::fs::read_to_string(paths.world().join("workspace.yaml")).unwrap().starts_with("title: mine"));
        // Somebody else's sites stay, and the world is imported beside them.
        std::fs::write(paths.caddyfile(), "shop.example {\n\treverse_proxy :3000\n}\n").unwrap();
        up(&w, &plan).unwrap();
        let caddy = std::fs::read_to_string(paths.caddyfile()).unwrap();
        assert!(caddy.starts_with("shop.example {") && caddy.contains("import /etc/caddy/zetlyn-world.caddy"), "{caddy}");
        assert!(std::fs::read_to_string(paths.caddy_own()).unwrap().contains("prices.example {"));
        up(&w, &plan).unwrap();
        assert_eq!(std::fs::read_to_string(paths.caddyfile()).unwrap().matches("import /etc/caddy/zetlyn-world.caddy").count(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_dry_run_writes_nothing() {
        let root = std::env::temp_dir().join(format!("zetlyn-world-dry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        up(&wanted(), &Plan { root: root.clone(), dry: true, services: false }).unwrap();
        assert!(!root.exists());
    }

    #[test]
    fn an_export_is_every_file_and_each_database_as_one_moment() {
        let dir = std::env::temp_dir().join(format!("zetlyn-world-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sources/s")).unwrap();
        std::fs::write(dir.join("workspace.yaml"), "title: t\nurl: https://w.example\n").unwrap();
        std::fs::write(dir.join("operator.key"), "ed25519:00").unwrap();
        let db = rusqlite::Connection::open(dir.join("sources/s/claims.db")).unwrap();
        db.execute_batch("pragma journal_mode=wal; create table t(x); insert into t values (1), (2), (3);").unwrap();
        // Open and written to while it is exported: the copy is still whole.
        let to = dir.join("out/world.tar.gz");
        let n = export(&dir, &to).unwrap();
        drop(db);
        assert_eq!(n, 3);
        let mut names = Vec::new();
        let unpacked = dir.join("unpacked");
        std::fs::create_dir_all(&unpacked).unwrap();
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(std::fs::File::open(&to).unwrap()));
        for e in tar.entries().unwrap() {
            let mut e = e.unwrap();
            names.push(e.path().unwrap().to_string_lossy().into_owned());
            e.unpack_in(&unpacked).unwrap();
        }
        names.sort();
        assert_eq!(names, vec!["EXPORT.json", "world/operator.key", "world/sources/s/claims.db", "world/workspace.yaml"]);
        let copy = rusqlite::Connection::open(unpacked.join("world/sources/s/claims.db")).unwrap();
        assert_eq!(copy.query_row("select count(*) from t", [], |r| r.get::<_, i64>(0)).unwrap(), 3);
        let about: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(unpacked.join("EXPORT.json")).unwrap()).unwrap();
        assert_eq!(about["url"], "https://w.example");
        assert!(export(&dir.join("sources"), &dir.join("x.tar.gz")).unwrap_err().contains("not a workspace"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_backup_keeps_the_newest_so_many() {
        let dir = std::env::temp_dir().join(format!("zetlyn-world-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("w")).unwrap();
        std::fs::write(dir.join("w/workspace.yaml"), "title: t\n").unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        for old in ["world-20260101T000000Z.tar.gz", "world-20260102T000000Z.tar.gz", "world-20260103T000000Z.tar.gz", "notes.txt"] {
            std::fs::write(dir.join("b").join(old), "x").unwrap();
        }
        let (file, _, gone) = backup(&dir.join("w"), &dir.join("b"), 2).unwrap();
        assert_eq!(gone, 2);
        assert!(file.exists() && dir.join("b/world-20260103T000000Z.tar.gz").exists() && dir.join("b/notes.txt").exists());
        assert!(!dir.join("b/world-20260101T000000Z.tar.gz").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_a_newer_release_is_an_upgrade() {
        assert_eq!(version("v0.3.21"), Some((0, 3, 21)));
        assert_eq!(version("0.10.0"), Some((0, 10, 0)));
        assert!(version("v0.3.21") > version("0.3.9"));
        assert!(version("v0.3.20") <= version("0.3.20"));
        assert_eq!(version("latest"), None);
    }

    #[test]
    fn the_binary_is_found_inside_a_release() {
        let mut packed = Vec::new();
        {
            let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(&mut packed, flate2::Compression::default()));
            for (name, body) in [("./README", b"read me".as_slice()), ("./zetlyn", b"\x7fELF binary".as_slice())] {
                let mut h = tar::Header::new_gnu();
                h.set_size(body.len() as u64);
                h.set_mode(0o755);
                h.set_cksum();
                tar.append_data(&mut h, name, body).unwrap();
            }
            tar.into_inner().unwrap().finish().unwrap();
        }
        assert_eq!(unpack_binary(&packed).unwrap(), b"\x7fELF binary");
    }
}
