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
zetlyn world up <domain> --owner <address> [--from <archive>] [--title …] [--smtp host[:port] --smtp-user … --mail-from …]
                [--port 2500] [--dry-run] [--root <prefix>] [--no-services]
zetlyn world serve <workspace> [--addr 127.0.0.1:2500]
zetlyn world export <workspace> --to <file.tar.gz>
zetlyn world import <file.tar.gz> --to <dir> [--url <address>] [--owner <address>]
zetlyn world move <workspace> --to <address> [--unchecked] | --back
zetlyn world register <workspace> [--at https://zetlyn.com/directory]
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
        Some("import") => {
            let rest = crate::positional(args, 2);
            let file = PathBuf::from(rest.first().ok_or("which archive? one `zetlyn world export` wrote")?.as_str());
            let dir = PathBuf::from(crate::flag(args, "--to").ok_or("--to <dir>, an empty directory")?);
            let n = import(&file, &dir, crate::flag(args, "--url"), crate::flag(args, "--owner"))?;
            println!("{n} files in {}. Serve it there, then on the old machine: zetlyn world move <workspace> --to <its address>", dir.display());
            Ok(())
        }
        Some("register") => {
            let dir = PathBuf::from(crate::positional(args, 2).first().ok_or("which workspace?")?.as_str());
            let at = crate::flag(args, "--at").unwrap_or("https://zetlyn.com/directory");
            let world = crate::directory::ask_to_be_listed(&dir, at)?;
            println!("{world} is listed at {at}");
            Ok(())
        }
        Some("move") => {
            let dir = PathBuf::from(crate::positional(args, 2).first().ok_or("which workspace?")?.as_str());
            if args.iter().any(|a| a == "--back") {
                stay(&dir)?;
                println!("{} answers where it is again", dir.display());
                return Ok(());
            }
            let to = crate::flag(args, "--to").ok_or("--to <address>, where it is now")?;
            move_to(&dir, to, args.iter().any(|a| a == "--unchecked"))?;
            println!("{} says it is at {to} now: its document says so, and every page redirects there", dir.display());
            Ok(())
        }
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
    /// An exported world to make it from, instead of an empty one.
    pub from: Option<PathBuf>,
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
            from: crate::flag(args, "--from").map(PathBuf::from),
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
        "# A world of its own, made by `zetlyn world up`. Its owners sign in at {url}/signin.\ntitle: {}\nurl: {}\ncontact: {}\nowners:\n- {}\n# What it publishes goes to its own hub, which it serves at {url}/hub/.\npublish:\n  to: hub\n  app: {}\n",
        q(&w.title),
        q(&w.url()),
        q(&w.owner),
        q(&w.owner),
        q(&w.url()),
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
    {
        use std::io::Write;
        let mut f = create_private(&tmp, mode)?;
        f.write_all(content.as_bytes()).map_err(|e| format!("{}: {e}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(true)
}

/// A new file that is never, for a moment, readable by more than `mode` says: a password is not
/// written first and hidden after. Whatever was at the name before is gone, a link included.
fn create_private(path: &Path, mode: u32) -> Result<std::fs::File, String> {
    let _ = std::fs::remove_file(path);
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(mode);
    }
    let _ = mode;
    o.open(path).map_err(|e| format!("{}: {e}", path.display()))
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
    // A site whose address list names the domain itself: not one that only ends in it.
    let names_it = |l: &str| l.trim_end().strip_suffix('{').is_some_and(|names| names.split([',', ' ']).map(str::trim).any(|n| n == w.domain || n == format!("https://{}", w.domain)));
    if text.lines().any(names_it) {
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

    // What this run changed, so that a run that changed nothing restarts nothing.
    let (mut changed, mut caddy_changed) = (false, false);

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
        changed = true;
    }

    // The workspace, with its address and its owner.
    let world = paths.world();
    if !world.join(crate::account::WORKSPACE).exists() {
        match &w.from {
            // A world brought along: made again here, at this address, run by this owner.
            Some(archive) => step(plan, &format!("make the world in {} from {}, at {}", world.display(), archive.display(), w.url()), || {
                import(archive, &world, Some(&w.url()), Some(&w.owner)).map(|_| ())
            })?,
            None => step(plan, &format!("make the workspace in {}, owned by {}", world.display(), w.owner), || {
                init(&world, w).map(|_| ())
            })?,
        }
        changed = true;
    }
    // A key of its own to sign what it publishes, inside the world (its unit's ZETLYN_HOME), so it
    // goes where the world goes and a subscriber's pin holds after a move. One brought along in an
    // archive is kept: a second key is a publisher everybody who pinned the first stops trusting.
    let signs = world.join(".zetlyn");
    if !signs.join(crate::identity::KEY_FILE).exists() {
        step(plan, &format!("give the world a key to sign what it publishes, in {}", signs.display()), || {
            let title = crate::account::Site::load(&world).title;
            crate::identity::new_in(&signs, if title.is_empty() { &w.domain } else { &title }, &w.owner).map(|_| ())
        })?;
        changed = true;
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
                step(plan, &format!("keep the mailer's password in {}, readable by zetlyn alone", paths.password().display()), || -> Result<(), String> {
                    changed |= ensure_file(&paths.password(), &password, 0o640)?;
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
    // Git too: a source can be a repository, and is read with it.
    if machine && (run("which", &["caddy"]).is_err() || run("which", &["git"]).is_err()) {
        step(plan, "install Caddy and git from the distribution", || {
            run("apt-get", &["update", "-q"])?;
            run("apt-get", &["install", "-y", "-q", "caddy", "git"]).map(|_| ())
        })?;
    }
    let held = std::fs::read_to_string(paths.caddyfile()).ok();
    match caddy_is(held.as_deref(), w) {
        // Written by an earlier run, perhaps with another port, perhaps with sites added since: said,
        // and not written over.
        CaddyIs::Ours => {
            if !held.as_deref().unwrap_or("").contains(&caddy_block(w)) {
                println!("  (the site for {} in {} is not what this run would write, with port {}: left as it is; change it there)", w.domain, paths.caddyfile().display(), w.port);
            }
        }
        CaddyIs::Placeholder => {
            step(plan, &format!("make {} the world's", paths.caddyfile().display()), || {
                ensure_file(&paths.caddyfile(), &format!("{{\n\temail {}\n}}\n\n{}", w.owner, caddy_block(w)), 0o644).map(|_| ())
            })?;
            caddy_changed = true;
        }
        CaddyIs::Theirs => {
            step(plan, &format!("put the world beside the sites already in {}, in {}", paths.caddyfile().display(), paths.caddy_own().display()), || {
                caddy_changed |= ensure_file(&paths.caddy_own(), &caddy_block(w), 0o644)?;
                let import = format!("import {}", "/etc/caddy/zetlyn-world.caddy");
                let text = held.clone().unwrap_or_default();
                if !text.lines().any(|l| l.trim() == import) {
                    caddy_changed |= ensure_file(&paths.caddyfile(), &format!("{}\n{import}\n", text.trim_end()), 0o644)?;
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
            changed = true;
        }
    }
    if machine && plan.services {
        step(plan, "start the world, its backup and its upgrade, and have Caddy read its configuration", || {
            run("systemctl", &["daemon-reload"])?;
            run("systemctl", &["enable", "--now", "zetlyn-world.service", "zetlyn-backup.timer", "zetlyn-upgrade.timer"])?;
            // Restarted only for something new: a run that changed nothing interrupts nobody.
            if changed {
                run("systemctl", &["restart", "zetlyn-world.service"])?;
            }
            if caddy_changed || changed {
                run("systemctl", &["reload-or-restart", "caddy"])?;
            }
            Ok(())
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
    let scratch = std::env::temp_dir().join(format!("zetlyn-export-{}", crate::jwt::random()));
    std::fs::create_dir(&scratch).map_err(|e| e.to_string())?;
    let partial = to.with_extension("partial");
    let result = (|| {
        let file = create_private(&partial, 0o600)?;
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

// ---------------------------------------------------------------------------------------------
// A world describing itself (FEDERATION.md, M12): `<url>/.well-known/zetlyn.json`, signed with
// the world's operator key, and its own hub at `<url>/hub/`.

/// What a world document says it is.
pub const DOCUMENT: &str = "zetlyn-world/1";

/// What this world is, for a stranger who knows only its address: its key, what it publishes and
/// where, its public sources and trackers, where it takes proposals. Unsigned.
pub fn document(root: &Path) -> Result<serde_json::Value, String> {
    use serde_json::json;
    let site = crate::account::Site::for_workspace(root);
    let url = site.url.trim_end_matches('/').to_string();
    let key = crate::propose::operator_key(root)?;
    let hub = match &site.publish {
        Some(p) if !p.read_at.trim().is_empty() => Some(p.read_at.trim().trim_end_matches('/').to_string()),
        Some(p) if p.to.trim() == "hub" => Some(format!("{url}/hub")),
        _ if root.join("hub").is_dir() => Some(format!("{url}/hub")),
        _ => None,
    };
    let mut sources = Vec::new();
    for (name, dir) in crate::tracker::registry(&root.join("sources")) {
        let Ok(decl) = crate::sourcedecl::SourceDecl::load(&dir) else { continue };
        // What its licence lets anybody see, and nothing it keeps to itself.
        if !matches!(decl.licence.republish.as_str(), "yes" | "summary") {
            continue;
        }
        let at = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut s = json!({ "name": name, "title": decl.title, "kind": decl.kind });
        if let crate::sourcedecl::Fetch::Proposals { readers, .. } = &decl.source {
            if !readers.is_empty() {
                s["readers"] = json!(readers);
                s["propose"] = json!(format!("{url}/propose/{at}"));
            }
        }
        sources.push(s);
    }
    let mut trackers = Vec::new();
    for (name, dir) in crate::tracker::scope_registry(&root.join("trackers")) {
        let Ok(decl) = crate::trackerdecl::TrackerDecl::load(&dir) else { continue };
        if decl.visibility == "private" {
            continue;
        }
        let at = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        trackers.push(json!({ "name": name, "title": decl.title, "at": format!("{url}/trackers/{at}/") }));
    }
    Ok(json!({
        "zetlyn": DOCUMENT,
        "world": url,
        "title": site.title,
        "key": key,
        "publishes_with": crate::identity::key_at(root),
        "hub": hub,
        "sources": sources,
        "trackers": trackers,
        "directories": site.directories,
        "moved_to": Some(site.moved_to.trim().trim_end_matches('/').to_string()).filter(|m| !m.is_empty()),
    }))
}

/// The bytes a world document's signature is over: the document without its signature, as JSON
/// with its keys in order.
fn signed_bytes(doc: &serde_json::Value) -> Vec<u8> {
    let mut d = doc.clone();
    if let Some(o) = d.as_object_mut() {
        o.remove("signature");
    }
    serde_json::to_vec(&sorted(&d)).unwrap_or_default()
}

/// Keys in order at every depth, whatever order a parser kept them in.
fn sorted(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(m) => {
            let ordered: std::collections::BTreeMap<String, serde_json::Value> = m.iter().map(|(k, v)| (k.clone(), sorted(v))).collect();
            serde_json::to_value(ordered).unwrap_or_default()
        }
        serde_json::Value::Array(a) => serde_json::Value::Array(a.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

/// The document, signed with the operator key it names.
pub fn signed_document(root: &Path) -> Result<serde_json::Value, String> {
    let mut doc = document(root)?;
    let signature = crate::key::sign(root, crate::grant::OPERATOR_KEY, &signed_bytes(&doc))?.ok_or("this world has no key to sign with")?;
    doc["signature"] = serde_json::Value::String(signature);
    Ok(doc)
}

/// Whether a document is signed by the key it names, and says it is a world document. The key is
/// the world's word for itself: whoever keeps it pins it, as a subscriber pins a publisher's.
pub fn verify(doc: &serde_json::Value) -> Result<(), String> {
    if doc["zetlyn"].as_str() != Some(DOCUMENT) {
        return Err("not a zetlyn world document".into());
    }
    let key = doc["key"].as_str().ok_or("the document names no key")?;
    let signature = doc["signature"].as_str().ok_or("the document is not signed")?;
    crate::key::verify(key, &signed_bytes(doc), signature)
}

/// The document of the world at `url`, verified. `None` where the address answers but is no world
/// (a hub that is only storage, as zetlyn.com's is).
pub fn fetch(url: &str) -> Result<Option<serde_json::Value>, String> {
    // A world this very process serves is asked here, not over HTTP to itself.
    if let Some(root) = crate::oidc::served_world(url.trim_end_matches('/')) {
        return signed_document(&root).map(Some);
    }
    let at = format!("{}/.well-known/zetlyn.json", url.trim_end_matches('/'));
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(concat!("zetlyn/", env!("CARGO_PKG_VERSION")))
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut r = agent.get(&at).call().map_err(|e| format!("{at}: {e}"))?;
    if !r.status().is_success() {
        return Ok(None);
    }
    let Ok(doc) = r.body_mut().read_json::<serde_json::Value>() else { return Ok(None) };
    if doc["zetlyn"].as_str() != Some(DOCUMENT) {
        return Ok(None);
    }
    verify(&doc).map_err(|e| format!("{at}: {e}"))?;
    Ok(Some(doc))
}

/// Where an address says to fetch from.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    /// The hub: a world's own, or the address itself where it is a plain hub.
    pub hub: String,
    /// The key the world publishes with, to pin.
    pub publishes_with: Option<String>,
    /// The world, and the key its document is signed with, where the address is one: what a
    /// subscription remembers, so it can follow the world when it moves.
    pub world: Option<(String, String)>,
}

/// Where to fetch from, given an address: a world's own hub and the key it publishes with, where
/// the address is a world; the address itself, and nothing pinned, where it is a plain hub.
pub fn resolve(address: &str) -> Result<Resolved, String> {
    let plain = || Resolved { hub: address.to_string(), publishes_with: None, world: None };
    if !(address.starts_with("https://") || address.starts_with("http://")) {
        return Ok(plain());
    }
    match fetch(address)? {
        Some(doc) => {
            let hub = doc["hub"].as_str().ok_or_else(|| format!("{address} is a world that publishes nothing"))?.to_string();
            if !on_the_web(&hub) {
                return Err(format!("{address} says it publishes at {hub}, which is not an address on the web"));
            }
            Ok(Resolved {
                hub,
                publishes_with: doc["publishes_with"].as_str().map(str::to_string),
                world: Some((doc["world"].as_str().unwrap_or(address).to_string(), doc["key"].as_str().unwrap_or_default().to_string())),
            })
        }
        None => Ok(plain()),
    }
}

/// A hub another world names is fetched from the web, never read from this machine's own disk.
fn on_the_web(hub: &str) -> bool {
    hub.starts_with("https://") || hub.starts_with("http://")
}

/// Beside a subscription: which world it came from, and that world's key.
pub const FOLLOWED: &str = "world.json";

pub fn remember(dir: &Path, world: &str, key: &str) -> Result<(), String> {
    let text = serde_json::to_string_pretty(&serde_json::json!({ "world": world, "key": key })).unwrap_or_default();
    std::fs::write(dir.join(FOLLOWED), text).map_err(|e| format!("{}: {e}", dir.display()))
}

/// Before a subscription fetches: ask the world it came from where it publishes now. A world that
/// moved is followed to where its document says, only when the world there is signed by the same
/// key; a world that publishes somewhere new is followed there. What was done, where anything was.
pub fn follow(dir: &Path) -> Result<Option<String>, String> {
    let Ok(text) = std::fs::read_to_string(dir.join(FOLLOWED)) else { return Ok(None) };
    let held: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", dir.join(FOLLOWED).display()))?;
    let (world, key) = (held["world"].as_str().unwrap_or_default().to_string(), held["key"].as_str().unwrap_or_default().to_string());
    let doc = match fetch(&world) {
        Ok(Some(d)) => d,
        Ok(None) => return Ok(Some(format!("{world} does not answer as a world now; fetched from where it was"))),
        Err(e) => return Ok(Some(format!("{e}; fetched from where it was"))),
    };
    if doc["key"].as_str() != Some(key.as_str()) {
        return Ok(Some(format!("{world} answers with another key than the one it had, so nothing about it was followed")));
    }
    let (doc, moved) = match doc["moved_to"].as_str().filter(|m| !m.is_empty()) {
        Some(to) => match fetch(to) {
            Ok(Some(there)) if there["key"].as_str() == Some(key.as_str()) && there["moved_to"].is_null() => (there, true),
            _ => return Ok(Some(format!("{world} says it moved to {to}, which does not answer as the same world; not followed"))),
        },
        None => (doc, false),
    };
    let Some(hub) = doc["hub"].as_str().map(str::to_string).filter(|h| on_the_web(h)) else {
        return Ok(Some(format!("{} publishes nothing now; fetched from where it was", doc["world"].as_str().unwrap_or(&world))));
    };
    let mut decl = crate::sourcedecl::SourceDecl::load(dir)?;
    let mut said = None;
    if let crate::sourcedecl::Fetch::Hub { at, .. } = &mut decl.source {
        if *at != hub {
            said = Some(if moved { format!("{world} moved to {}; fetching from {hub}", doc["world"].as_str().unwrap_or_default()) } else { format!("{world} publishes at {hub} now") });
            *at = hub;
            let path = dir.join(crate::sourcedecl::FILE);
            std::fs::write(&path, crate::yaml::to_string(&decl)?).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    if moved {
        remember(dir, doc["world"].as_str().unwrap_or(&world), &key)?;
        said.get_or_insert_with(|| format!("{world} moved to {}", doc["world"].as_str().unwrap_or_default()));
    }
    Ok(said)
}

/// A top-level `key: value` in a workspace.yaml, set where it is said and added where it is not,
/// everything else in the file as it was. `None` takes it out.
fn set_top(file: &Path, key: &str, value: Option<&str>) -> Result<(), String> {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    let line = value.map(|v| format!("{key}: {}", serde_json::to_string(v).unwrap_or_default()));
    let mut out: Vec<String> = Vec::new();
    let mut done = false;
    for l in text.lines() {
        if l.starts_with(&format!("{key}:")) {
            if let (Some(new), false) = (&line, done) {
                out.push(new.clone());
            }
            done = true;
        } else {
            out.push(l.to_string());
        }
    }
    if let (Some(new), false) = (&line, done) {
        out.push(new.clone());
    }
    std::fs::write(file, format!("{}\n", out.join("\n").trim_end())).map_err(|e| format!("{}: {e}", file.display()))
}

/// More than any world this exports, and less than fills a disk.
const IMPORT_MAX_FILES: usize = 2_000_000;
const IMPORT_MAX_BYTES: u64 = 64 << 30;

/// An exported world made again in an empty directory: at its new address where one is given,
/// run by `owner` where one is given, and no longer moved anywhere. How many files it holds.
pub fn import(file: &Path, dir: &Path, url: Option<&str>, owner: Option<&str>) -> Result<usize, String> {
    if dir.exists() && std::fs::read_dir(dir).map(|mut d| d.next().is_some()).unwrap_or(true) {
        return Err(format!("{}: not empty; a world is imported into an empty directory", dir.display()));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let reader = std::fs::File::open(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(reader));
    let (mut n, mut about) = (0usize, false);
    let mut bytes = 0u64;
    for entry in tar.entries().map_err(|e| format!("{}: {e}", file.display()))? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        if path == Path::new("EXPORT.json") {
            about = true;
            continue;
        }
        let Ok(rel) = path.strip_prefix("world") else { continue };
        // Files and directories, nothing else: a link in an archive points wherever its maker liked,
        // and the next file written "inside" it lands there.
        let kind = entry.header().entry_type();
        if !kind.is_file() && !kind.is_dir() {
            return Err(format!("{}: {} is a link or a device, which no export holds", file.display(), path.display()));
        }
        bytes += entry.size();
        if n >= IMPORT_MAX_FILES || bytes > IMPORT_MAX_BYTES {
            return Err(format!("{}: more than {IMPORT_MAX_FILES} files or {} GiB; not a world this imports", file.display(), IMPORT_MAX_BYTES >> 30));
        }
        if rel.as_os_str().is_empty() || rel.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
            return Err(format!("{}: {} is not a path inside the world", file.display(), path.display()));
        }
        let target = dir.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        entry.unpack(&target).map_err(|e| format!("{}: {e}", target.display()))?;
        n += 1;
    }
    let ws = dir.join(crate::account::WORKSPACE);
    if !about || !ws.exists() {
        return Err(format!("{}: not an exported world", file.display()));
    }
    if let Some(u) = url {
        set_top(&ws, "url", Some(u.trim_end_matches('/')))?;
    }
    set_top(&ws, "moved_to", None)?;
    // A domain is a name a machine answers for, and not this machine's to claim because an archive
    // says so: it is said again here, where it is wanted.
    set_top(&ws, "domain", None)?;
    if let Some(o) = owner {
        let site = crate::account::Site::load(dir);
        if !site.owners.iter().any(|x| x.eq_ignore_ascii_case(o)) {
            if site.owners.is_empty() {
                let text = std::fs::read_to_string(&ws).unwrap_or_default();
                std::fs::write(&ws, format!("{}\nowners:\n- {}\n", text.trim_end(), serde_json::to_string(o).unwrap_or_default())).map_err(|e| e.to_string())?;
            } else {
                return Err(format!("imported, but {o} is not among its owners ({}): add them in {} by hand", site.owners.join(", "), ws.display()));
            }
        }
    }
    Ok(n)
}

/// The world at `dir` says it is at `to` now. Only once `to` answers as this same world, signed
/// with this world's key and not itself moved, unless that check is waived.
pub fn move_to(dir: &Path, to: &str, unchecked: bool) -> Result<(), String> {
    let to = to.trim().trim_end_matches('/');
    if !unchecked {
        let key = crate::propose::operator_key(dir)?;
        match fetch(to)? {
            Some(doc) if doc["key"].as_str() == Some(key.as_str()) && doc["moved_to"].is_null() => {}
            Some(_) => return Err(format!("{to} answers as another world, or one that has moved itself: not moved")),
            None => return Err(format!("{to} does not answer as a world yet. Import it there and start it first, or say --unchecked")),
        }
    }
    set_top(&dir.join(crate::account::WORKSPACE), "moved_to", Some(to))
}

/// The world at `dir` answers where it is again: whatever it said about moving, unsaid.
pub fn stay(dir: &Path) -> Result<(), String> {
    set_top(&dir.join(crate::account::WORKSPACE), "moved_to", None)
}

/// A file of the world's own hub, `<root>/hub/<rest>`, as a reader asks for it: a directory is its
/// `index.html`. Nothing outside it.
pub fn hub_file(root: &Path, rest: &[String], prefix: &str) -> Option<(Vec<u8>, &'static str)> {
    if rest.iter().any(|s| s.is_empty() || s == "." || s == ".." || s.contains(['/', '\\', '\0'])) {
        return None;
    }
    let mut path = root.join("hub");
    for s in rest {
        path.push(s);
    }
    if path.is_dir() {
        path.push("index.html");
    }
    let mut bytes = std::fs::read(&path).ok()?;
    // The pages are written for a hub at the root of its host (hubpages.rs); here it is at
    // `<prefix>`, and what they link to is said under it. Only the pages and the list: a manifest
    // or an archive is signed as it is, and goes out byte for byte.
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name == "index.html" || (name == "index.json" && rest.len() == 1) {
        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        if name == "index.html" {
            for at in ["/hub/", "/sources/", "/trackers/", "/packages/", "/style.css", "/app.js", "/mark.png", "/favicon.png"] {
                let to = if at == "/hub/" { format!("{prefix}/") } else { format!("{prefix}{at}") };
                text = text.replace(&format!("=\"{at}"), &format!("=\"{to}"));
            }
            text = text.replace("fetch(\"/hub/index.json\")", &format!("fetch(\"{prefix}/index.json\")"));
        } else {
            text = text.replace("\"page\":\"/hub/", &format!("\"page\":\"{prefix}/"));
        }
        bytes = text.into_bytes();
    }
    let kind = match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("json") => "application/json",
        Some("gz") => "application/gzip",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    };
    Some((bytes, kind))
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
        assert_eq!(caddy_is(Some("shop.prices.example {\n\treverse_proxy :3000\n}\n"), &w), CaddyIs::Theirs, "a site that only ends in the name");
        assert_eq!(caddy_is(Some("www.prices.example, prices.example {\n\treverse_proxy :3000\n}\n"), &w), CaddyIs::Ours);
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
        assert!(crate::key::public(&paths.world().join(".zetlyn"), crate::identity::KEY_FILE).is_some(), "a key of its own to publish with");
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
    fn up_from_an_archive_makes_that_world_again_at_the_new_address() {
        let base = std::env::temp_dir().join(format!("zetlyn-world-up-from-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let old = base.join("old");
        std::fs::create_dir_all(old.join("sources/kept")).unwrap();
        std::fs::write(old.join("workspace.yaml"), "title: Brought along\nurl: https://old.example\n").unwrap();
        std::fs::write(old.join("sources/kept/note.txt"), "still here").unwrap();
        export(&old, &base.join("old.tar.gz")).unwrap();
        let mut w = wanted();
        w.smtp = None;
        w.from = Some(base.join("old.tar.gz"));
        up(&w, &Plan { root: base.join("machine"), dry: false, services: false }).unwrap();
        let world = Paths { root: base.join("machine") }.world();
        assert_eq!(std::fs::read_to_string(world.join("sources/kept/note.txt")).unwrap(), "still here");
        let site = crate::account::Site::load(&world);
        assert_eq!((site.title.as_str(), site.url.as_str()), ("Brought along", "https://prices.example"));
        assert_eq!(site.owners, vec!["ann@example.org".to_string()]);
        let _ = std::fs::remove_dir_all(&base);
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
    fn an_archive_with_a_link_in_it_is_refused_and_a_domain_is_not_carried_in() {
        let dir = std::env::temp_dir().join(format!("zetlyn-world-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let build = |name: &str, link: bool| {
            let file = dir.join(name);
            let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(std::fs::File::create(&file).unwrap(), flate2::Compression::default()));
            let mut add = |path: &str, body: &[u8]| {
                let mut h = tar::Header::new_gnu();
                h.set_size(body.len() as u64);
                h.set_mode(0o644);
                h.set_cksum();
                tar.append_data(&mut h, path, body).unwrap();
            };
            add("EXPORT.json", b"{}");
            add("world/workspace.yaml", b"title: t\ndomain: data.example.org\n");
            if link {
                let mut h = tar::Header::new_gnu();
                h.set_entry_type(tar::EntryType::Symlink);
                h.set_size(0);
                h.set_mode(0o777);
                tar.append_link(&mut h, "world/sources", "/etc").unwrap();
            }
            tar.into_inner().unwrap().finish().unwrap();
            file
        };
        let err = import(&build("bad.tar.gz", true), &dir.join("bad"), None, None).unwrap_err();
        assert!(err.contains("a link"), "{err}");
        assert!(!dir.join("bad/sources").exists());
        import(&build("good.tar.gz", false), &dir.join("good"), None, None).unwrap();
        assert_eq!(crate::account::Site::load(&dir.join("good")).domain, "");
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

    #[test]
    fn a_hub_file_is_one_of_its_own_and_a_directory_is_its_page() {
        let root = std::env::temp_dir().join(format!("zetlyn-world-hubfile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("hub/sources/a/b")).unwrap();
        std::fs::write(root.join("hub/index.json"), "[]").unwrap();
        std::fs::write(root.join("hub/sources/a/b/index.html"), "<p>b</p>").unwrap();
        std::fs::write(root.join("workspace.yaml"), "secret").unwrap();
        let p = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(hub_file(&root, &p(&["index.json"]), "/hub").unwrap(), (b"[]".to_vec(), "application/json"));
        assert_eq!(hub_file(&root, &p(&["sources", "a", "b"]), "/hub").unwrap().0, b"<p>b</p>");
        assert!(hub_file(&root, &p(&["..", "workspace.yaml"]), "/hub").is_none());
        assert!(hub_file(&root, &p(&["nothing"]), "/hub").is_none());
        // Its pages link under where it is; what is signed goes out as it is.
        std::fs::write(root.join("hub/index.html"), r#"<a href="/hub/#x"></a><a href="/hub/sources/a/b/"></a><link href="/style.css"><script>fetch("/hub/index.json")</script>"#).unwrap();
        std::fs::write(root.join("hub/index.json"), r#"[{"page":"/hub/sources/a/b/"}]"#).unwrap();
        std::fs::write(root.join("hub/sources/a/b/manifest.json"), r#"{"page":"/sources/a/b/"}"#).unwrap();
        let page = String::from_utf8(hub_file(&root, &[], "/acme/hub").unwrap().0).unwrap();
        assert_eq!(page, r#"<a href="/acme/hub/#x"></a><a href="/acme/hub/sources/a/b/"></a><link href="/acme/hub/style.css"><script>fetch("/acme/hub/index.json")</script>"#);
        assert_eq!(hub_file(&root, &p(&["index.json"]), "/acme/hub").unwrap().0, br#"[{"page":"/acme/hub/sources/a/b/"}]"#);
        assert_eq!(hub_file(&root, &p(&["sources", "a", "b", "manifest.json"]), "/acme/hub").unwrap().0, br#"{"page":"/sources/a/b/"}"#);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_world_says_what_it_is_and_another_takes_its_source_from_it_with_no_hub_between() {
        let tmp = |tag: &str| {
            let d = std::env::temp_dir().join(format!("zetlyn-world-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            d
        };
        // The key a world publishes with is its own, in its own home.
        let publisher = publisher();

        let a = tmp("doc-a");
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}");
        std::fs::write(a.join("workspace.yaml"), format!("title: World A\nurl: {url}\nowners: [ann@example.org]\npublish:\n  to: hub\n  app: {url}\n")).unwrap();
        let dir = a.join("sources/prices");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(a.join("sources/private")).unwrap();
        std::fs::write(
            dir.join("source.yaml"),
            "name: t/prices\ntitle: Car prices\nkind: price\nfetch:\n  type: proposals\n  from: []\n  readers: [signed-in]\nlicence:\n  republish: yes\nclaims:\n  id:\n    scheme: price\n    from: \"const:{country}-{week}\"\n  title: \"const:{country} {week}\"\n  known: field:read_at\n  properties:\n    price:\n      type: number\n      from: field:price\n",
        )
        .unwrap();
        // A source that says nothing about being shown is in no document.
        std::fs::write(a.join("sources/private/source.yaml"), std::fs::read_to_string(dir.join("source.yaml")).unwrap().replace("t/prices", "t/private").replace("licence:\n  republish: yes\n", "")).unwrap();
        let ann = crate::propose::Reader { id: crate::propose::pseudonym(&a, 1).unwrap(), name: "Ann".into(), email: "ann@example.org".into(), owner: false, issuers: Vec::new() };
        let row = br#"{"row": {"country": "DEU", "week": "2026-W40", "price": 44990}, "read_at": "2026-10-04", "read_from": "https://example.com", "attest": "read"}"#;
        let file = crate::propose::receive_from_reader(&dir, &a, row, &ann).unwrap();
        crate::propose::decide(&dir, &file, true, "ann", "").unwrap();
        crate::source::Source::open(&dir).unwrap().run().unwrap();
        // Published to its own hub, and nowhere else.
        let moved = crate::app::publish_root(&a, "a", |_| std::collections::BTreeMap::new()).unwrap();
        assert!(moved >= 1);
        assert!(a.join("hub/sources/t/prices/tags/latest").exists());

        // What it says about itself, signed.
        let doc = signed_document(&a).unwrap();
        verify(&doc).unwrap();
        assert_eq!(doc["world"], url);
        assert_eq!(doc["hub"], format!("{url}/hub"));
        assert_eq!(doc["publishes_with"], publisher);
        let sources = doc["sources"].as_array().unwrap();
        assert_eq!(sources.len(), 1, "{sources:?}");
        assert_eq!(sources[0]["name"], "t/prices");
        assert_eq!(sources[0]["readers"], serde_json::json!(["signed-in"]));
        assert_eq!(sources[0]["propose"], format!("{url}/propose/prices"));
        // Changed after it was signed, it is not the world's word any more.
        let mut forged = doc.clone();
        forged["hub"] = serde_json::json!("https://elsewhere.example/hub");
        assert!(verify(&forged).is_err());
        // Reordered by whatever parsed it, it still is.
        let reparsed: serde_json::Value = serde_json::from_str(&serde_json::to_string_pretty(&doc).unwrap()).unwrap();
        verify(&reparsed).unwrap();

        // Served, and taken by somebody who knows only its address.
        let args: Vec<String> = ["world", "serve", a.to_str().unwrap(), "--addr", &format!("127.0.0.1:{port}")].iter().map(|s| s.to_string()).collect();
        std::thread::spawn(move || crate::app::world_serve(&args));
        for _ in 0..50 {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let r = resolve(&url).unwrap();
        assert_eq!((r.hub.as_str(), r.publishes_with.as_deref()), (format!("{url}/hub").as_str(), Some(publisher.as_str())));
        assert_eq!(r.world.as_ref().map(|(w, _)| w.as_str()), Some(url.as_str()));
        let (hub, key) = (r.hub, r.publishes_with);
        let b = tmp("doc-b");
        let place = crate::place::at(&hub).unwrap();
        let reference = crate::artifact::Reference::parse("t/prices").unwrap();
        let (held, _) = crate::artifact::subscribe(place.as_ref(), &reference, &b.join("prices"), &hub, key.as_deref()).unwrap();
        assert_eq!(held, 1);
        assert!(std::fs::read_to_string(b.join("prices/source.yaml")).unwrap().contains(&publisher), "the publisher's key is pinned");
        // Not an address at all: nothing to look up.
        assert_eq!(resolve("/srv/hub").unwrap(), Resolved { hub: "/srv/hub".into(), publishes_with: None, world: None });
        for d in [&a, &b] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    /// One identity for every test that publishes: `ZETLYN_HOME` is the process's, and two tests
    /// each pointing it at their own would sign with each other's keys.
    fn publisher() -> String {
        static KEY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        KEY.get_or_init(|| {
            let home = std::env::temp_dir().join(format!("zetlyn-world-home-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&home);
            std::env::set_var("ZETLYN_HOME", &home);
            crate::identity::new("A world", "ann@example.org").unwrap()
        })
        .clone()
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    /// A world at `url` with one public source and one row a reader proposed and its owner took,
    /// published to its own hub.
    fn a_world_with_a_row(dir: &Path, url: &str) {
        let _ = std::fs::remove_dir_all(dir);
        let source = dir.join("sources/prices");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(dir.join("trackers")).unwrap();
        std::fs::write(dir.join("workspace.yaml"), format!("# kept by hand\ntitle: World A\nurl: {url}\nowners: [ann@example.org]\npublish:\n  to: hub\n  app: {url}\n")).unwrap();
        std::fs::write(
            source.join("source.yaml"),
            "name: t/prices\ntitle: Car prices\nkind: price\nfetch:\n  type: proposals\n  from: []\n  readers: [signed-in]\nlicence:\n  republish: yes\nclaims:\n  id:\n    scheme: price\n    from: \"const:{country}-{week}\"\n  title: \"const:{country} {week}\"\n  known: field:read_at\n  properties:\n    price:\n      type: number\n      from: field:price\n",
        )
        .unwrap();
        let accounts = crate::account::Accounts::open(dir).unwrap();
        let reader = accounts.ensure("ben@example.org").unwrap();
        accounts.set_name(reader.id, "Ben").unwrap();
        let ben = crate::propose::Reader { id: crate::propose::pseudonym(dir, reader.id).unwrap(), name: "Ben".into(), email: reader.email.clone(), owner: false, issuers: Vec::new() };
        let row = br#"{"row": {"country": "DEU", "week": "2026-W40", "price": 44990}, "read_at": "2026-10-04", "read_from": "https://example.com", "attest": "read"}"#;
        let file = crate::propose::receive_from_reader(&source, dir, row, &ben).unwrap();
        accounts.record_proposal(reader.id, "t/prices", &file).unwrap();
        crate::propose::decide(&source, &file, true, "ann", "").unwrap();
        crate::source::Source::open(&source).unwrap().run().unwrap();
        crate::app::publish_root(dir, "a", |_| std::collections::BTreeMap::new()).unwrap();
    }

    fn serve(dir: &Path, port: u16) {
        let args: Vec<String> = ["world", "serve", dir.to_str().unwrap(), "--addr", &format!("127.0.0.1:{port}")].iter().map(|s| s.to_string()).collect();
        // Ends with the test process; nothing outlives it.
        std::thread::spawn(move || crate::app::world_serve(&args));
        for _ in 0..50 {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("the world at {port} never answered");
    }

    #[test]
    fn a_world_moves_to_another_machine_and_keeps_its_readers_its_subscribers_and_its_receipts() {
        publisher();
        let base = std::env::temp_dir().join(format!("zetlyn-world-move-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (pa, pb) = (free_port(), free_port());
        let (url_a, url_b) = (format!("http://127.0.0.1:{pa}"), format!("http://127.0.0.1:{pb}"));
        let (a, b, c) = (base.join("a"), base.join("b"), base.join("c"));
        a_world_with_a_row(&a, &url_a);
        serve(&a, pa);

        // Somebody subscribed, knowing only where the world was.
        let r = resolve(&url_a).unwrap();
        let place = crate::place::at(&r.hub).unwrap();
        let reference = crate::artifact::Reference::parse("t/prices").unwrap();
        crate::artifact::subscribe(place.as_ref(), &reference, &c.join("prices"), &r.hub, r.publishes_with.as_deref()).unwrap();
        let (world, key) = r.world.clone().unwrap();
        remember(&c.join("prices"), &world, &key).unwrap();
        assert_eq!(follow(&c.join("prices")).unwrap(), None, "nothing has moved yet");

        // Exported, taken to the other machine, and started there.
        let archive = base.join("a.tar.gz");
        export(&a, &archive).unwrap();
        assert!(import(&archive, &a, None, None).unwrap_err().contains("not empty"));
        import(&archive, &b, Some(&url_b), Some("ann@example.org")).unwrap();
        let site_b = crate::account::Site::load(&b);
        assert_eq!((site_b.url.as_str(), site_b.moved_to.as_str()), (url_b.as_str(), ""));
        assert!(std::fs::read_to_string(b.join("workspace.yaml")).unwrap().starts_with("# kept by hand"), "the rest of the file as it was");
        serve(&b, pb);

        // A world that is not this one is no place to move to.
        let stranger = base.join("stranger");
        let ps = free_port();
        a_world_with_a_row(&stranger, &format!("http://127.0.0.1:{ps}"));
        serve(&stranger, ps);
        assert!(move_to(&a, &format!("http://127.0.0.1:{ps}"), false).unwrap_err().contains("another world"));
        move_to(&a, &url_b, false).unwrap();

        // The old address says where it went, signed by the same key, and sends every page there.
        let doc_a = fetch(&url_a).unwrap().unwrap();
        assert_eq!(doc_a["moved_to"], url_b);
        assert_eq!(doc_a["key"], fetch(&url_b).unwrap().unwrap()["key"]);
        let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).max_redirects(0).build().into();
        let r = agent.get(&format!("{url_a}/trackers/prices/things?q=deu")).call().unwrap();
        assert_eq!(r.status().as_u16(), 302);
        assert_eq!(r.headers().get("location").unwrap().to_str().unwrap(), format!("{url_b}/trackers/prices/things?q=deu"));
        let r = agent.post(&format!("{url_a}/propose/prices")).send("{}").unwrap();
        assert_eq!(r.status().as_u16(), 410);

        // The subscriber follows it, and takes what it publishes there.
        let said = follow(&c.join("prices")).unwrap().unwrap();
        assert!(said.contains("moved"), "{said}");
        match crate::sourcedecl::SourceDecl::load(&c.join("prices")).unwrap().source {
            crate::sourcedecl::Fetch::Hub { at, .. } => assert_eq!(at, format!("{url_b}/hub")),
            _ => panic!("not a subscription"),
        }
        assert!(std::fs::read_to_string(c.join("prices/world.json")).unwrap().contains(&url_b));
        assert_eq!(follow(&c.join("prices")).unwrap(), None, "followed once, settled");

        // Its reader is still its reader, under the same name, and the receipt still names them.
        let accounts = crate::account::Accounts::open(&b).unwrap();
        let ben = accounts.by_email("ben@example.org").unwrap();
        assert_eq!(accounts.name_of(ben.id), "Ben");
        assert_eq!(accounts.proposals_of(ben.id).len(), 1);
        assert_eq!(crate::propose::pseudonym(&b, ben.id).unwrap(), crate::propose::pseudonym(&a, ben.id).unwrap(), "the same key, so the same pseudonym");
        let ds = crate::source::Source::open(&b.join("sources/prices")).unwrap();
        let q = crate::source::Query { text: String::new(), pred: None, ids: vec!["DEU-2026-W40".into()], seen_before: None, view: None, sort: None, limit: 5, offset: 0 };
        let ids: Vec<String> = ds.search(&q).unwrap().1.into_iter().map(|h| h.record_id).collect();
        let claim = ds.fetch(&ids, false)[0].to_json().to_string();
        assert!(claim.contains("Ben (reader:"), "{claim}");
        let _ = std::fs::remove_dir_all(&base);
    }
}
