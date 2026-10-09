//! Who is asking, and what they are entitled to.
//!
//! No passwords. A person types an address, receives a link, and is signed in; there is nothing to
//! store that can be reused elsewhere and nothing to reset. The link and the API key are 256 bits
//! from the system's own source and are kept as their SHA-256: a high-entropy token needs no
//! stretching, because there is no smaller space to search than the whole of it.

use std::path::Path;

use rusqlite::Connection;
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub struct Accounts {
    db: Connection,
}

const SCHEMA: &str = "
create table if not exists account(
  id      integer primary key,
  email   text not null unique,
  created text not null,
  -- free, active, past_due or cancelled, and what a provider called this subscription.
  state      text not null default 'free',
  paid_until text,
  scopes     text not null default '',
  reference  text,
  -- Whether this account may add a dataset or compose a scope. Nobody may by default: a
  -- catalogue anybody can write to is a catalogue nobody can promise anything about.
  curator    integer not null default 0);

create table if not exists session(
  hash    text primary key,
  account integer not null,
  -- member (zs), reader (zr) or provider (zo): a session is good for the cookie it was made for.
  kind    text not null default '',
  created text not null,
  expires text not null);

create table if not exists link(
  hash    text primary key,
  account integer not null,
  expires text not null);

create table if not exists apikey(
  hash      text primary key,
  account   integer not null,
  name      text not null,
  created   text not null,
  last_used text);

-- Who an account is elsewhere: signed in through another world, GitHub, Google or Apple, as
-- that issuer's subject. The address is the one the issuer said it had verified, where it said one.
create table if not exists identity(
  issuer  text not null,
  subject text not null,
  account integer not null,
  email   text,
  created text not null,
  primary key(issuer, subject));

-- This world as a provider: a code handed to a relying party once, for a minute, and the token it
-- is exchanged for. Both kept as their hashes.
create table if not exists oauth_code(
  hash      text primary key,
  account   integer not null,
  client    text not null,
  redirect  text not null,
  nonce     text not null,
  challenge text not null,
  scope     text not null,
  expires   text not null);
create table if not exists oauth_token(
  hash    text primary key,
  account integer not null,
  client  text not null,
  scope   text not null,
  expires text not null);

-- This world as a relying party: a sign-in sent elsewhere and not back yet, by its state.
create table if not exists oauth_pending(
  state    text primary key,
  provider text not null,
  verifier text not null,
  nonce    text not null,
  next     text not null,
  purpose  text not null,
  expires  text not null);

-- Which reader made which proposal from the browser. The proposal names a pseudonym; this is
-- the only place it meets an address, so the reader can be told what became of it.
create table if not exists proposal(
  source  text not null,
  file    text not null,
  account integer not null,
  at      text not null,
  primary key(source, file));
";

/// How far behind a free reader stands. One number, and the whole of the paywall.
pub const FREE_DELAY_DAYS: i64 = 30;

/// 256 bits, from the system. `/dev/urandom` is the whole of what this needs.
pub fn token() -> String {
    use std::io::Read;
    let mut bytes = [0u8; 32];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut bytes);
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn digest(raw: &str) -> String {
    let mut h = Sha256::new();
    h.update(raw.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Claims first held at or before this, or published before it, are free to read.
pub fn free_edge() -> String {
    crate::iso_stamp(crate::now() - FREE_DELAY_DAYS * 86_400)
}

/// What a viewer may reach, as a bound on the query rather than a check around it.
pub fn bound(viewer: &Viewer, scope: &str) -> Option<String> {
    if viewer.free || viewer.entitled(scope) {
        None
    } else {
        Some(free_edge())
    }
}

#[derive(Clone, Debug)]
pub struct Account {
    pub id: i64,
    pub email: String,
    pub state: String,
    pub paid_until: Option<String>,
    /// Which trackers this subscription covers. Empty means all of them.
    pub scopes: Vec<String>,
    pub curator: bool,
}

impl Account {
    /// Paid for, and not past the day it was paid to.
    ///
    /// A cancelled subscription keeps what was paid for until the period ends, which is what the
    /// terms say and therefore what this has to do. `past_due` does not: a payment that failed is
    /// a payment that has not been made.
    pub fn entitled(&self, scope: &str) -> bool {
        if !matches!(self.state.as_str(), "active" | "cancelled") {
            return false;
        }
        if self.state == "cancelled" && self.paid_until.is_none() {
            return false;
        }
        if let Some(until) = &self.paid_until {
            if until.as_str() < crate::iso_date(crate::now()).as_str() {
                return false;
            }
        }
        self.scopes.is_empty() || self.scopes.iter().any(|s| s == scope)
    }
}

/// What the surface knows about whoever is asking.
#[derive(Clone, Debug, Default)]
pub struct Viewer {
    pub account: Option<Account>,
    /// True where the request came with an API key rather than a session.
    pub by_key: bool,
    /// Nothing here costs anything: no free edge, no paywall, nothing said about subscribing.
    /// Who may see a private tracker is a different question, and this does not answer it.
    pub free: bool,
}

impl Viewer {
    /// May read all of it, now: because nothing here costs anything, or because they pay. Not
    /// whether they may see a private tracker, which is `entitled` alone.
    pub fn reads(&self, scope: &str) -> bool {
        self.free || self.entitled(scope)
    }
    pub fn entitled(&self, scope: &str) -> bool {
        self.account.as_ref().is_some_and(|a| a.entitled(scope))
    }
    pub fn email(&self) -> Option<&str> {
        self.account.as_ref().map(|a| a.email.as_str())
    }
}

impl Accounts {
    pub fn open(root: &Path) -> Result<Accounts, String> {
        let db = Connection::open(root.join("accounts.db")).map_err(|e| e.to_string())?;
        db.execute_batch("pragma journal_mode=wal; pragma synchronous=full;")
            .map_err(|e| e.to_string())?;
        db.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        // A store written by an earlier build opens under a later one. SQLite refuses a
        // duplicate column, and that refusal is the whole of the check.
        let _ =
            db.execute_batch("alter table account add column curator integer not null default 0");
        let _ = db.execute_batch("alter table account add column name text not null default ''");
        // Sessions from before kinds were kept answer to none of them: whoever held one signs in again.
        let _ = db.execute_batch("alter table session add column kind text not null default ''");
        Ok(Accounts { db })
    }

    fn read(&self, sql: &str, param: &str) -> Option<Account> {
        self.db
            .query_row(sql, rusqlite::params![param], |r| {
                let scopes: String = r.get(4)?;
                let curator: i64 = r.get(5).unwrap_or(0);
                Ok(Account {
                    id: r.get(0)?,
                    email: r.get(1)?,
                    state: r.get(2)?,
                    paid_until: r.get(3)?,
                    scopes: scopes
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect(),
                    curator: curator != 0,
                })
            })
            .ok()
    }

    pub fn by_email(&self, email: &str) -> Option<Account> {
        self.read(
            "select id, email, state, paid_until, scopes, curator from account where email = ?1",
            &email.trim().to_lowercase(),
        )
    }

    pub fn ensure(&self, email: &str) -> Result<Account, String> {
        let email = email.trim().to_lowercase();
        if !email.contains('@') || email.len() < 4 {
            return Err("that is not an address".into());
        }
        if let Some(a) = self.by_email(&email) {
            return Ok(a);
        }
        self.db
            .execute(
                "insert into account(email, created) values(?1, ?2)",
                rusqlite::params![email, crate::iso_stamp(crate::now())],
            )
            .map_err(|e| e.to_string())?;
        self.by_email(&email)
            .ok_or_else(|| "the account did not stay".into())
    }

    /// A link good for one sign-in and a quarter of an hour.
    pub fn new_link(&self, account: i64) -> Result<String, String> {
        let raw = token();
        self.db
            .execute(
                "insert into link(hash, account, expires) values(?1, ?2, ?3)",
                rusqlite::params![digest(&raw), account, crate::iso_stamp(crate::now() + 900)],
            )
            .map_err(|e| e.to_string())?;
        Ok(raw)
    }

    /// Spends the link and hands back a session. A link that was used is gone, so a copy of the
    /// mail in somebody else's hands is worth nothing.
    pub fn spend_link(&self, raw: &str, kind: Kind) -> Option<String> {
        let hash = digest(raw);
        let (account, expires): (i64, String) = self
            .db
            .query_row(
                "select account, expires from link where hash = ?1",
                rusqlite::params![hash],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok()?;
        let _ = self
            .db
            .execute("delete from link where hash = ?1", rusqlite::params![hash]);
        if expires.as_str() < crate::iso_stamp(crate::now()).as_str() {
            return None;
        }
        self.new_session(account, kind).ok()
    }

    /// The account behind a session, where it was made for this kind of cookie: a reader's or a
    /// provider's session copied into a member's cookie is no session at all.
    pub fn by_session(&self, raw: &str, kind: Kind) -> Option<Account> {
        let hash = digest(raw);
        let expires: String = self
            .db
            .query_row(
                "select expires from session where hash = ?1 and kind = ?2",
                rusqlite::params![hash, kind.name()],
                |r| r.get(0),
            )
            .ok()?;
        if expires.as_str() < crate::iso_stamp(crate::now()).as_str() {
            return None;
        }
        self.read(
            "select a.id, a.email, a.state, a.paid_until, a.scopes, a.curator from account a
             join session s on s.account = a.id where s.hash = ?1",
            &hash,
        )
    }

    pub fn end_session(&self, raw: &str) {
        let _ = self.db.execute(
            "delete from session where hash = ?1",
            rusqlite::params![digest(raw)],
        );
    }

    pub fn new_key(&self, account: i64, name: &str) -> Result<String, String> {
        let raw = format!("zk_{}", token());
        self.db
            .execute(
                "insert into apikey(hash, account, name, created) values(?1,?2,?3,?4)",
                rusqlite::params![digest(&raw), account, name, crate::iso_stamp(crate::now())],
            )
            .map_err(|e| e.to_string())?;
        Ok(raw)
    }

    pub fn by_key(&self, raw: &str) -> Option<Account> {
        let hash = digest(raw);
        let _ = self.db.execute(
            "update apikey set last_used = ?2 where hash = ?1",
            rusqlite::params![hash, crate::iso_stamp(crate::now())],
        );
        self.read(
            "select a.id, a.email, a.state, a.paid_until, a.scopes, a.curator from account a
             join apikey k on k.account = a.id where k.hash = ?1",
            &hash,
        )
    }

    pub fn keys(&self, account: i64) -> Vec<(String, String, Option<String>)> {
        let Ok(mut stmt) = self.db.prepare(
            "select name, created, last_used from apikey where account = ?1 order by created",
        ) else {
            return Vec::new();
        };
        stmt.query_map(rusqlite::params![account], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }

    pub fn drop_key(&self, account: i64, name: &str) {
        let _ = self.db.execute(
            "delete from apikey where account = ?1 and name = ?2",
            rusqlite::params![account, name],
        );
    }

    /// What a payment provider's webhook moves, and what an operator moves by hand for the first
    /// customers, who arrive before any provider does.
    pub fn set_subscription(
        &self,
        email: &str,
        state: &str,
        paid_until: Option<&str>,
        scopes: &[String],
        reference: Option<&str>,
    ) -> Result<Account, String> {
        let account = self.ensure(email)?;
        self.db
            .execute(
                "update account set state = ?2, paid_until = ?3, scopes = ?4,
                        reference = coalesce(?5, reference) where id = ?1",
                rusqlite::params![account.id, state, paid_until, scopes.join(","), reference],
            )
            .map_err(|e| e.to_string())?;
        self.by_email(&account.email)
            .ok_or_else(|| "the account did not stay".into())
    }

    pub fn all(&self) -> Vec<Account> {
        let Ok(mut stmt) = self.db.prepare(
            "select id, email, state, paid_until, scopes, curator from account order by email",
        ) else {
            return Vec::new();
        };
        stmt.query_map([], |r| {
            let scopes: String = r.get(4)?;
            let curator: i64 = r.get(5).unwrap_or(0);
            Ok(Account {
                id: r.get(0)?,
                email: r.get(1)?,
                state: r.get(2)?,
                paid_until: r.get(3)?,
                scopes: scopes
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect(),
                curator: curator != 0,
            })
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }

    /// Everyone whose subscription lapses within the next week, which is what a dunning run reads.
    pub fn lapsing(&self, within_days: i64) -> Vec<Account> {
        let edge = crate::iso_date(crate::now() + within_days * 86_400);
        self.all()
            .into_iter()
            .filter(|a| a.state == "active")
            .filter(|a| a.paid_until.as_deref().is_some_and(|u| u <= edge.as_str()))
            .collect()
    }
}

/// What a workspace says about itself. Absent, everything still runs and the sign-in link is
/// printed where the operator can see it, which is what a workspace on a laptop wants.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Site {
    #[serde(default)]
    pub title: String,
    /// The address this workspace answers on, for the link in a sign-in mail.
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub contact: String,
    /// Who runs this world where it is served on its own (`zetlyn world serve`): they sign in with
    /// a link to their address and may change everything. Everybody else reads what it publishes.
    #[serde(default)]
    pub owners: Vec<String>,
    /// Who may do more than read, by the words a source's `readers` uses: owners change everything,
    /// editors its sources and trackers and decide proposals, proposers propose. Everybody else reads
    /// what is public. Here and nowhere else: zetlyn.com, or any other world, only says who somebody
    /// is, and this world says what they may.
    #[serde(default)]
    pub access: Access,
    /// Where this world is now, once it has moved (`zetlyn world move`). Its document says so,
    /// signed with the same key, and every page redirects there.
    #[serde(default)]
    pub moved_to: String,
    /// Who it takes identities from, besides a link to an address (FEDERATION.md, M14). Not said,
    /// it is "Sign in with zetlyn.com"; `identity: []` is nobody else.
    #[serde(default)]
    pub identity: Option<Vec<IdentityDecl>>,
    /// A hosted world's own domain (M16): it is served at the root of it, its address is
    /// `https://<domain>`, and the machine takes a certificate for it when it is first asked.
    #[serde(default)]
    pub domain: String,
    /// Whether this world keeps a directory of others, at `<url>/directory` (M15).
    #[serde(default)]
    pub directory: bool,
    /// The directories it is listed in, as `zetlyn world register` left them.
    #[serde(default)]
    pub directories: Vec<String>,
    /// What a subscription costs. A workspace that names none charges nothing: every reader reads
    /// all of it, now, and nothing on its pages speaks of paying.
    #[serde(default)]
    pub price: Option<Price>,
    #[serde(default)]
    pub mail: Mail,
    /// Who the assist asks, if anyone. See assist.rs.
    #[serde(default)]
    pub assist: crate::assist::Config,
    /// Updates in the background, and how often. See autoupdate.rs.
    #[serde(default)]
    pub update: crate::autoupdate::Config,
    /// Where what this workspace reads is published after an update moved it, and its hub's pages
    /// written again: `publish: { to: s3://bucket/prefix, app: https://zetlyn.com }`.
    #[serde(default)]
    pub publish: Option<Publish>,
    /// What the world says about itself on its About page: what it is, who runs it, where to
    /// write, its imprint. `contact:` above is the address to write to.
    #[serde(default)]
    pub profile: Profile,
}

#[derive(Debug, Default, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub about: String,
    /// Who runs it: a person or an organisation.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub operator: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub website: String,
    /// Where to write about personal data, where it is not `contact`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub privacy: String,
    /// The legal notice its operator owes, as they write it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub imprint: String,
}

/// Where a world is signed in from when nobody said: zetlyn.com, as it is the hub when nobody said.
pub const DEFAULT_IDENTITY: &str = "https://zetlyn.com";

/// One provider a world takes identities from.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IdentityDecl {
    /// Another zetlyn world, by its address, or `any`: whichever world the person names.
    Zetlyn(String),
    Github(GithubDecl),
    Google(GoogleDecl),
    Apple(AppleDecl),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GithubDecl {
    pub client: String,
    pub secret: String,
    /// Where GitHub is, for a GitHub Enterprise or a test: `https://github.com` and its API.
    #[serde(default)]
    pub web: String,
    #[serde(default)]
    pub api: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoogleDecl {
    pub client: String,
    pub secret: String,
    /// Only people of this Google Workspace domain (`hd`).
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub issuer: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppleDecl {
    /// The Services ID.
    pub client: String,
    pub team: String,
    pub key_id: String,
    /// The `.p8` it signs its own client secret with, PEM, usually `${APPLE_PRIVATE_KEY}`.
    pub key: String,
    #[serde(default)]
    pub issuer: String,
}

impl Site {
    /// Who this world takes identities from.
    pub fn identities(&self) -> Vec<IdentityDecl> {
        self.identity.clone().unwrap_or_else(|| vec![IdentityDecl::Zetlyn(DEFAULT_IDENTITY.into())])
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Publish {
    /// A hub: a folder, `s3://bucket/prefix`, or an address that takes signed writes. A relative
    /// folder is the workspace's own, and `hub` is the one a world serves at `<url>/hub/`.
    pub to: String,
    /// Where its trackers can be opened, for the hub's pages: `<app>/<org>/t/<tracker>/`.
    #[serde(default)]
    pub app: String,
    /// Where what is published there can be read by anybody, where that is not the world's own
    /// `<url>/hub`: `https://zetlyn.com` for what goes to its bucket. The world's document says it.
    #[serde(default)]
    pub read_at: String,
}

impl Publish {
    /// Where it is written: a relative folder under the workspace, anything else as it is said.
    pub fn place_for(&self, root: &Path) -> String {
        let to = self.to.trim();
        let addressed = to.contains("://") || Path::new(to).is_absolute();
        if addressed { to.to_string() } else { root.join(to).to_string_lossy().into_owned() }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Price {
    pub one: String,
    pub team: String,
    pub currency: String,
    /// Where a person goes to pay. A provider's hosted page, because this program holds no card.
    #[serde(default)]
    pub buy: String,
}

impl Default for Price {
    fn default() -> Self {
        Price {
            one: "19".into(),
            team: "99".into(),
            currency: "€".into(),
            buy: String::new(),
        }
    }
}

/// Zetlyn ships no mail client. An operator names the one that already knows how to reach them,
/// and the address is appended to the argv.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mail {
    #[serde(default)]
    pub run: Vec<String>,
    /// SMTP, where Zetlyn sends mail itself: `smtp: { host, port, tls, user, password: ${SMTP_PASSWORD}, from }`.
    #[serde(default)]
    pub smtp: Option<crate::mail::Smtp>,
}

/// The workspace's own file: what it calls itself, its address, its mailer.
pub const WORKSPACE: &str = "workspace.yaml";

impl Site {
    pub fn load(root: &Path) -> Site {
        let mut site: Site = crate::yaml::read_or_default(&root.join(WORKSPACE));
        // One mailer for the machine: every workspace on it that names none of its own sends
        // through /etc/zetlyn/mail.yaml (or the file ZETLYN_MAIL names), as noreply@ the host.
        if site.mail.smtp.is_none() && site.mail.run.is_empty() {
            let central = std::env::var("ZETLYN_MAIL").unwrap_or_else(|_| "/etc/zetlyn/mail.yaml".into());
            let path = Path::new(&central);
            if path.exists() {
                match crate::yaml::read::<Mail>(path) {
                    Ok(m) => site.mail = m,
                    Err(e) => eprintln!("{central}: {e}"),
                }
            }
        }
        site
    }

    /// What a workspace says about itself, its address included where it names none of its own:
    /// an organisation on a hosting machine (`<dir>/orgs/<org>`) is at the machine's address,
    /// under its own name. Never the address a request says it was sent to: a sign-in link built
    /// from that goes wherever whoever asked for it says.
    pub fn for_workspace(root: &Path) -> Site {
        let mut site = Site::load(root);
        // A world on a domain of its own is at that domain.
        if site.url.is_empty() && !site.domain.trim().is_empty() {
            site.url = format!("https://{}", site.domain.trim().trim_end_matches('/'));
        }
        if site.url.is_empty() {
            let org = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if let Some(hosting) = root.parent().filter(|p| p.file_name().is_some_and(|n| n == "orgs")).and_then(Path::parent) {
                let machine = Site::load(hosting).url;
                if !machine.is_empty() {
                    site.url = format!("{}/{org}", machine.trim_end_matches('/'));
                }
            }
        }
        site
    }

    /// An address on this site: `path` as a browser asks for it here, mount and all, under the
    /// address the workspace names. Where that address carries the path's beginning already (an
    /// organisation's `https://zetlyn.com/zetlyn`, a page under `/zetlyn/…`), it is said once.
    /// Empty where the workspace names no address.
    pub fn link(&self, path: &str) -> String {
        let url = self.url.trim().trim_end_matches('/');
        if url.is_empty() {
            return String::new();
        }
        let host_from = url.find("://").map(|i| i + 3).unwrap_or(0);
        let (origin, own) = match url[host_from..].find('/') {
            Some(i) => url.split_at(host_from + i),
            None => (url, ""),
        };
        if !own.is_empty() && (path == own || path.starts_with(&format!("{own}/")) || path.starts_with(&format!("{own}?"))) {
            format!("{origin}{path}")
        } else {
            format!("{url}{path}")
        }
    }

    /// Hands the mailer the message on its standard input. Where none is named, the link goes to
    /// the operator's own terminal and the page says where to look.
    pub fn send(&self, to: &str, subject: &str, body: &str) -> Result<bool, String> {
        // A cell counts what it sends, and past what its month allows sends nothing more.
        if !crate::usage::allowed(crate::usage::MAILS) {
            return Err("this world has sent every mail its month allows".into());
        }
        // A cell holds no mailer of its own and runs none: its server's relay sends and counts.
        if crate::usage::in_cell() {
            crate::cell::relay_mail(to, subject, body)?;
            return Ok(true);
        }
        crate::usage::count(crate::usage::MAILS);
        if let Some(smtp) = &self.mail.smtp {
            crate::mail::send(smtp, to, subject, body)?;
            return Ok(true);
        }
        if self.mail.run.is_empty() {
            println!("--- {subject} → {to} ---\n{body}\n---");
            return Ok(false);
        }
        use std::io::Write;
        let mut argv = self.mail.run.clone();
        argv.push(to.to_string());
        let (program, rest) = argv
            .split_first()
            .ok_or("a mailer needs something to run")?;
        let mut child = std::process::Command::new(program)
            .args(rest)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("{program}: {e}"))?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin
                .write_all(format!("Subject: {subject}\n\n{body}\n").as_bytes())
                .map_err(|e| format!("{program}: {e}"))?;
        }
        let status = child.wait().map_err(|e| format!("{program}: {e}"))?;
        if !status.success() {
            return Err(format!("{program}: exited {status}"));
        }
        Ok(true)
    }
}

/// A name as it is shown and signed: one line, no control characters, eighty at most. A line break
/// in a name would read, in what the workspace signs for a proposal, as a line of its own.
pub fn clean_name(name: &str) -> String {
    name.trim().chars().filter(|c| !c.is_control()).take(80).collect::<String>().trim().to_string()
}

/// A reader's session on a tracker's pages. Not `zs`, which is a member's session in the app: the
/// two are kept in different `accounts.db` files where a workspace is hosted, and one cookie under
/// one name and path would sign a person out of the one by signing them in to the other.
pub const READER_COOKIE: &str = "zr";

/// What a session was made for, which is which cookie carries it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `zs`: a member in the app, who may change things.
    Member,
    /// `zr`: a reader on a tracker's pages, who may read and propose.
    Reader,
    /// `zo`: somebody this world is signing in somewhere else, for as long as that takes.
    Provider,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Member => "member",
            Kind::Reader => "reader",
            Kind::Provider => "provider",
        }
    }
    /// As long as its cookie: a provider's cookie lives an hour, and so does what it carries.
    pub fn lasts(self) -> i64 {
        match self {
            Kind::Provider => 60 * 60,
            _ => 60 * 60 * 24 * 30,
        }
    }
}

/// The reader's session cookie, or an API key. A key is for a system and carries no cookie; a
/// session is for a person and carries nothing else.
pub fn viewer_of(accounts: &Accounts, cookie: Option<&str>, authorization: Option<&str>) -> Viewer {
    if let Some(key) = authorization.and_then(|a| a.strip_prefix("Bearer ")) {
        if let Some(account) = accounts.by_key(key.trim()) {
            return Viewer {
                account: Some(account),
                by_key: true,
                free: false,
            };
        }
    }
    // In a cell, the main server's sign-in is the one: whoever it says the session is, is the
    // reader here, and nobody else, so signing out there is signing out here.
    if remote_identity() {
        let account = cookie.and_then(session_cookie).and_then(|s| remote_member(&s)).and_then(|email| accounts.ensure(&email).ok());
        return Viewer { account, by_key: false, free: false };
    }
    let session = cookie.and_then(|c| {
        c.split(';')
            .filter_map(|p| p.trim().split_once('='))
            .find(|(k, _)| *k == READER_COOKIE)
            .map(|(_, v)| v.to_string())
    });
    match session.and_then(|s| accounts.by_session(&s, Kind::Reader)) {
        Some(account) => Viewer {
            account: Some(account),
            by_key: false,
            free: false,
        },
        None => Viewer::default(),
    }
}

impl Accounts {
    /// Who may add a source or compose a tracker. Granted from the terminal, because a catalogue
    /// anybody can write to is a catalogue nobody can promise anything about.
    pub fn set_curator(&self, email: &str, yes: bool) -> Result<Account, String> {
        let account = self.ensure(email)?;
        self.db
            .execute(
                "update account set curator = ?2 where id = ?1",
                rusqlite::params![account.id, yes as i64],
            )
            .map_err(|e| e.to_string())?;
        self.by_email(&account.email)
            .ok_or_else(|| "the account did not stay".into())
    }
}

impl Viewer {
    /// The person at the machine, in the local app. There are no accounts there: whoever runs
    /// the program owns everything it holds, and sees every page of it.
    pub fn operator() -> Viewer {
        Viewer {
            account: Some(Account {
                id: 0,
                email: "you".into(),
                state: "active".into(),
                paid_until: None,
                scopes: Vec::new(),
                curator: true,
            }),
            by_key: false,
            free: false,
        }
    }
}

/// A proposal a reader made from the browser, and whose it is. Kept here and nowhere else: the
/// proposal itself names a pseudonym, because a receipt is public and an address is not.
#[derive(Clone, Debug)]
pub struct Proposed {
    /// The source's name, as its declaration says it.
    pub source: String,
    /// The file the proposal is kept under in the source's `proposals/`.
    pub file: String,
    pub at: String,
}

impl Accounts {
    /// What a reader asked to be called beside what they propose. Empty until they say.
    pub fn name_of(&self, account: i64) -> String {
        self.db
            .query_row("select name from account where id = ?1", rusqlite::params![account], |r| r.get::<_, String>(0))
            .unwrap_or_default()
    }

    pub fn set_name(&self, account: i64, name: &str) -> Result<(), String> {
        self.db
            .execute("update account set name = ?2 where id = ?1", rusqlite::params![account, clean_name(name)])
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// An account closed: the row, and everything kept against it here. What it proposed stays in
    /// the sources it was proposed to, under its pseudonym and the name it gave, because a source's
    /// history is not rewritten; nothing here leads from that back to an address any more.
    pub fn delete(&self, account: i64) -> Result<(), String> {
        for table in ["session", "link", "apikey", "identity", "oauth_code", "oauth_token", "proposal"] {
            self.db.execute(&format!("delete from {table} where account = ?1"), rusqlite::params![account]).map_err(|e| e.to_string())?;
        }
        self.db.execute("delete from account where id = ?1", rusqlite::params![account]).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn record_proposal(&self, account: i64, source: &str, file: &str) -> Result<(), String> {
        self.db
            .execute(
                "insert or ignore into proposal(source, file, account, at) values(?1, ?2, ?3, ?4)",
                rusqlite::params![source, file, account, crate::iso_stamp(crate::now())],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Newest first.
    pub fn proposals_of(&self, account: i64) -> Vec<Proposed> {
        let Ok(mut stmt) = self.db.prepare("select source, file, at from proposal where account = ?1 order by at desc, file desc") else {
            return Vec::new();
        };
        stmt.query_map(rusqlite::params![account], |r| Ok(Proposed { source: r.get(0)?, file: r.get(1)?, at: r.get(2)? }))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    /// Who proposed it, where a reader did: the one to tell what became of it.
    pub fn proposer_of(&self, source: &str, file: &str) -> Option<Account> {
        let id: i64 = self
            .db
            .query_row("select account from proposal where source = ?1 and file = ?2", rusqlite::params![source, file], |r| r.get(0))
            .ok()?;
        self.read("select id, email, state, paid_until, scopes, curator from account where id = ?1", &id.to_string())
    }
}

/// A code this world handed a relying party, as it is taken back.
#[derive(Debug, Clone)]
pub struct Code {
    pub account: i64,
    pub client: String,
    pub redirect: String,
    pub nonce: String,
    pub challenge: String,
    pub scope: String,
}

/// A sign-in sent to another provider, as it comes back.
#[derive(Debug, Clone)]
pub struct Pending {
    pub provider: String,
    pub verifier: String,
    pub nonce: String,
    pub next: String,
    pub purpose: String,
}

impl Accounts {
    /// A session for an account, without a link: what signing in elsewhere ends in.
    pub fn new_session(&self, account: i64, kind: Kind) -> Result<String, String> {
        let session = token();
        self.db
            .execute(
                "insert into session(hash, account, kind, created, expires) values(?1,?2,?3,?4,?5)",
                rusqlite::params![digest(&session), account, kind.name(), crate::iso_stamp(crate::now()), crate::iso_stamp(crate::now() + kind.lasts())],
            )
            .map_err(|e| e.to_string())?;
        Ok(session)
    }

    pub fn by_id(&self, account: i64) -> Option<Account> {
        self.read("select id, email, state, paid_until, scopes, curator from account where id = ?1", &account.to_string())
    }

    /// The account an issuer's subject is, where it is one here.
    pub fn by_identity(&self, issuer: &str, subject: &str) -> Option<Account> {
        let id: i64 = self
            .db
            .query_row("select account from identity where issuer = ?1 and subject = ?2", rusqlite::params![issuer, subject], |r| r.get(0))
            .ok()?;
        self.by_id(id)
    }

    /// Every issuer an account has signed in through.
    pub fn issuers_of(&self, account: i64) -> Vec<String> {
        let Ok(mut stmt) = self.db.prepare("select issuer from identity where account = ?1 order by created") else { return Vec::new() };
        stmt.query_map(rusqlite::params![account], |r| r.get::<_, String>(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
    }

    /// The account somebody signed in elsewhere is here: the one already linked to that issuer's
    /// subject; else the one with the address the issuer verified; else a new one. A new one with
    /// no verified address gets one that is not an address (`….invalid`) and is never written to.
    pub fn for_identity(&self, issuer: &str, subject: &str, verified_email: Option<&str>) -> Result<Account, String> {
        if let Some(a) = self.by_identity(issuer, subject) {
            return Ok(a);
        }
        let account = match verified_email.map(|e| e.trim().to_lowercase()).filter(|e| e.contains('@')) {
            Some(email) => self.ensure(&email)?,
            None => {
                let host = issuer.split("://").nth(1).unwrap_or(issuer).split('/').next().unwrap_or("issuer");
                self.ensure(&format!("{}@{host}.invalid", &digest(&format!("{issuer}\n{subject}"))[..16]))?
            }
        };
        self.db
            .execute(
                "insert or ignore into identity(issuer, subject, account, email, created) values(?1,?2,?3,?4,?5)",
                rusqlite::params![issuer, subject, account.id, verified_email, crate::iso_stamp(crate::now())],
            )
            .map_err(|e| e.to_string())?;
        Ok(account)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn put_code(&self, raw: &str, account: i64, client: &str, redirect: &str, nonce: &str, challenge: &str, scope: &str) -> Result<(), String> {
        self.db
            .execute(
                "insert into oauth_code(hash, account, client, redirect, nonce, challenge, scope, expires) values(?1,?2,?3,?4,?5,?6,?7,?8)",
                rusqlite::params![digest(raw), account, client, redirect, nonce, challenge, scope, crate::iso_stamp(crate::now() + 60)],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// A code taken back, once: it is gone whether or not it is still good.
    pub fn take_code(&self, raw: &str) -> Option<Code> {
        let hash = digest(raw);
        let row = self
            .db
            .query_row(
                "select account, client, redirect, nonce, challenge, scope, expires from oauth_code where hash = ?1",
                rusqlite::params![hash],
                |r| Ok((Code { account: r.get(0)?, client: r.get(1)?, redirect: r.get(2)?, nonce: r.get(3)?, challenge: r.get(4)?, scope: r.get(5)? }, r.get::<_, String>(6)?)),
            )
            .ok();
        let _ = self.db.execute("delete from oauth_code where hash = ?1", rusqlite::params![hash]);
        let (code, expires) = row?;
        (expires.as_str() >= crate::iso_stamp(crate::now()).as_str()).then_some(code)
    }

    pub fn put_token(&self, raw: &str, account: i64, client: &str, scope: &str) -> Result<(), String> {
        self.db
            .execute(
                "insert into oauth_token(hash, account, client, scope, expires) values(?1,?2,?3,?4,?5)",
                rusqlite::params![digest(raw), account, client, scope, crate::iso_stamp(crate::now() + 600)],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// The account and scope a token was given for, while it is good.
    pub fn by_token(&self, raw: &str) -> Option<(i64, String)> {
        let (account, scope, expires): (i64, String, String) = self
            .db
            .query_row("select account, scope, expires from oauth_token where hash = ?1", rusqlite::params![digest(raw)], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .ok()?;
        (expires.as_str() >= crate::iso_stamp(crate::now()).as_str()).then_some((account, scope))
    }

    pub fn put_pending(&self, state: &str, p: &Pending) -> Result<(), String> {
        self.db
            .execute(
                "insert into oauth_pending(state, provider, verifier, nonce, next, purpose, expires) values(?1,?2,?3,?4,?5,?6,?7)",
                rusqlite::params![digest(state), p.provider, p.verifier, p.nonce, p.next, p.purpose, crate::iso_stamp(crate::now() + 900)],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// A sign-in that went elsewhere, taken back once by its state.
    pub fn take_pending(&self, state: &str) -> Option<Pending> {
        let hash = digest(state);
        let row = self
            .db
            .query_row(
                "select provider, verifier, nonce, next, purpose, expires from oauth_pending where state = ?1",
                rusqlite::params![hash],
                |r| Ok((Pending { provider: r.get(0)?, verifier: r.get(1)?, nonce: r.get(2)?, next: r.get(3)?, purpose: r.get(4)? }, r.get::<_, String>(5)?)),
            )
            .ok();
        let _ = self.db.execute("delete from oauth_pending where state = ?1", rusqlite::params![hash]);
        let (p, expires) = row?;
        (expires.as_str() >= crate::iso_stamp(crate::now()).as_str()).then_some(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(url: &str) -> Site {
        Site { url: url.into(), ..Site::default() }
    }

    #[test]
    fn a_session_is_good_only_for_the_cookie_it_was_made_for() {
        let dir = std::env::temp_dir().join(format!("zetlyn-kinds-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let accounts = Accounts::open(&dir).unwrap();
        let ann = accounts.ensure("ann@example.org").unwrap();
        let reader = accounts.spend_link(&accounts.new_link(ann.id).unwrap(), Kind::Reader).unwrap();
        let provider = accounts.new_session(ann.id, Kind::Provider).unwrap();
        assert_eq!(accounts.by_session(&reader, Kind::Reader).unwrap().id, ann.id);
        assert!(accounts.by_session(&reader, Kind::Member).is_none(), "a reader's session is not a member's");
        assert!(accounts.by_session(&provider, Kind::Member).is_none());
        assert!(accounts.by_session(&provider, Kind::Reader).is_none());
        // Closed, nothing of it answers any more.
        accounts.record_proposal(ann.id, "t/prices", "p.json").unwrap();
        accounts.delete(ann.id).unwrap();
        assert!(accounts.by_session(&reader, Kind::Reader).is_none());
        assert!(accounts.by_email("ann@example.org").is_none());
        assert!(accounts.proposer_of("t/prices", "p.json").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_path_the_address_already_carries_is_said_once() {
        // An organisation whose own address names its path, and a tracker mounted under it.
        let org = at("https://zetlyn.com/zetlyn");
        assert_eq!(org.link("/zetlyn/t/cve/signin/abc"), "https://zetlyn.com/zetlyn/t/cve/signin/abc");
        assert_eq!(org.link("/zetlyn"), "https://zetlyn.com/zetlyn");
        assert_eq!(org.link("/proposals/prices"), "https://zetlyn.com/zetlyn/proposals/prices", "a path inside it, not mounted");
        assert_eq!(org.link("/zetlynx/t/a"), "https://zetlyn.com/zetlyn/zetlynx/t/a", "a longer name is another name");
        let trailing = at("https://zetlyn.com/zetlyn/");
        assert_eq!(trailing.link("/zetlyn/t/cve/"), "https://zetlyn.com/zetlyn/t/cve/");
        // A machine, or a workspace standing alone, with no path of its own.
        assert_eq!(at("https://zetlyn.com").link("/zetlyn/t/cve/"), "https://zetlyn.com/zetlyn/t/cve/");
        assert_eq!(at("http://127.0.0.1:4747/").link("/t/cve/"), "http://127.0.0.1:4747/t/cve/");
        assert_eq!(at("").link("/t/cve/"), "");
    }

    #[test]
    fn an_organisation_with_no_address_is_at_the_machines_under_its_name() {
        let dir = std::env::temp_dir().join(format!("zetlyn-site-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let org = dir.join("orgs/acme");
        std::fs::create_dir_all(&org).unwrap();
        assert_eq!(Site::for_workspace(&org).url, "");
        std::fs::write(dir.join(WORKSPACE), "url: https://app.example.org/\n").unwrap();
        let site = Site::for_workspace(&org);
        assert_eq!(site.url, "https://app.example.org/acme");
        assert_eq!(site.link("/acme/trackers/prices/signin/x"), "https://app.example.org/acme/trackers/prices/signin/x");
        // Not an organisation: a workspace that names nothing has no address.
        assert_eq!(Site::for_workspace(&dir.join("orgs")).url, "");
        std::fs::write(org.join(WORKSPACE), "url: https://acme.example.org\n").unwrap();
        assert_eq!(Site::for_workspace(&org).url, "https://acme.example.org");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// What people may do in a world beyond reading it, each a list of the words a source's
/// `readers` uses: an address, `domain:example.com`, `@<world>` (whoever that world vouches for),
/// or `signed-in`.
#[derive(Debug, Default, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Access {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owners: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub editors: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposers: Vec<String>,
}

impl Site {
    /// Who owns the world: `owners:` as it was written before `access:`, and `access.owners`.
    pub fn all_owners(&self) -> Vec<String> {
        let mut out = self.owners.clone();
        out.extend(self.access.owners.iter().cloned());
        out
    }
    /// Who may change its sources and trackers: its owners and its editors.
    pub fn all_editors(&self) -> Vec<String> {
        let mut out = self.all_owners();
        out.extend(self.access.editors.iter().cloned());
        out
    }
}

/// Whether one of `patterns` names somebody: `signed-in` for anybody signed in, their address,
/// `domain:<domain>` for a verified address there, or `@<world>` for whoever that world, or that
/// provider, vouched for. `issuers` are the worlds and providers they signed in through.
pub fn admits(patterns: &[String], email: &str, issuers: &[String]) -> bool {
    let host = |u: &str| u.split("://").nth(1).unwrap_or(u).trim_end_matches('/').to_string();
    patterns.iter().any(|r| {
        let r = r.trim();
        r == "signed-in"
            || (!email.is_empty() && r.eq_ignore_ascii_case(email.trim()))
            || r.strip_prefix('@').is_some_and(|world| {
                let world = world.trim_end_matches('/');
                issuers.iter().any(|i| i.trim_end_matches('/') == world || host(i) == world)
            })
            || r.strip_prefix("domain:").is_some_and(|d| {
                let d = d.trim().to_lowercase();
                !d.is_empty() && !email.ends_with(".invalid") && email.to_lowercase().ends_with(&format!("@{d}"))
            })
    })
}

/// Whether a word may stand in `access:` or `readers`: an address, `domain:<domain>`, `@<world>`,
/// or `signed-in`. What it is not is said, so a page can say it back.
pub fn pattern_problem(p: &str) -> Option<String> {
    let p = p.trim();
    if p.is_empty() || p == "signed-in" {
        return None;
    }
    if let Some(d) = p.strip_prefix("domain:") {
        return (!d.contains('.') || d.contains('@') || d.contains(' ')).then(|| format!("{p}: a domain is written domain:example.org"));
    }
    if let Some(w) = p.strip_prefix('@') {
        return (w.is_empty() || w.contains(' ') || !w.contains('.')).then(|| format!("{p}: a world is written @zetlyn.com or @https://prices.example"));
    }
    let ok = p.split_once('@').is_some_and(|(local, host)| !local.is_empty() && host.contains('.') && !p.contains(' '));
    (!ok).then(|| format!("{p}: not an address, domain:…, @… or signed-in"))
}

/// The world's `access:` written into its workspace.yaml in place of the one there, everything
/// else in the file as it was. Read back before it is kept: a file that would not load is not.
pub fn set_access(root: &Path, access: &Access) -> Result<(), String> {
    let path = root.join(WORKSPACE);
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut kept: Vec<&str> = Vec::new();
    let mut skipping = false;
    for l in text.lines() {
        if l.starts_with("access:") {
            skipping = true;
            continue;
        }
        if skipping && (l.starts_with(' ') || l.is_empty()) {
            continue;
        }
        skipping = false;
        kept.push(l);
    }
    let quoted = |list: &[String]| list.iter().map(|p| serde_json::to_string(p.trim()).unwrap_or_default()).collect::<Vec<_>>().join(", ");
    let mut block = String::new();
    if !access.owners.is_empty() || !access.editors.is_empty() || !access.proposers.is_empty() {
        block.push_str("access:\n");
        for (key, list) in [("owners", &access.owners), ("editors", &access.editors), ("proposers", &access.proposers)] {
            if !list.is_empty() {
                block.push_str(&format!("  {key}: [{}]\n", quoted(list)));
            }
        }
    }
    let t = format!("{}\n{block}", kept.join("\n").trim_end());
    let _: Site = crate::yaml::parse(&t)?;
    std::fs::write(&path, format!("{}\n", t.trim())).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod access_tests {
    use super::*;

    #[test]
    fn a_world_s_access_is_written_in_place_and_only_its_words_are_taken() {
        assert_eq!(pattern_problem("ann@example.org"), None);
        assert_eq!(pattern_problem("domain:example.org"), None);
        assert_eq!(pattern_problem("@zetlyn.com"), None);
        assert_eq!(pattern_problem("signed-in"), None);
        assert!(pattern_problem("partner").is_some());
        assert!(pattern_problem("domain:ann@example.org").is_some());
        let dir = std::env::temp_dir().join(format!("zetlyn-access-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(WORKSPACE), "title: Acme\n# kept\naccess:\n  owners: [old@example.org]\nupdate:\n  every: 1h\n").unwrap();
        let access = Access { owners: vec!["ann@example.org".into()], editors: vec!["domain:example.org".into()], proposers: Vec::new() };
        set_access(&dir, &access).unwrap();
        let text = std::fs::read_to_string(dir.join(WORKSPACE)).unwrap();
        assert!(text.contains("# kept") && text.contains("every: 1h") && !text.contains("old@example.org"), "{text}");
        let site = Site::load(&dir);
        assert_eq!(site.access.editors, vec!["domain:example.org".to_string()]);
        assert!(admits(&site.all_owners(), "ann@example.org", &[]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// -- one sign-in for every cell ----------------------------------------------------------------

/// Where a cell asks who a session is: the main server's `/account/me`. Set where a hosting
/// directory is a cell; nowhere else is anybody asked.
static IDENTITY: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn ask_identity_at(url: &str) {
    let _ = IDENTITY.set(url.to_string());
}

pub fn remote_identity() -> bool {
    IDENTITY.get().is_some()
}

/// The address the main server says a session cookie is signed in as, asked at most once a minute
/// for the same session.
pub fn remote_member(session: &str) -> Option<String> {
    let url = IDENTITY.get()?;
    if session.is_empty() || session.len() > 200 || !session.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    type Kept = std::collections::BTreeMap<String, (i64, Option<String>)>;
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Kept>> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(Kept::new()));
    let now = crate::now();
    if let Some((at, email)) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(session).cloned() {
        if now - at < 60 {
            return email;
        }
    }
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(5))).build().into();
    let email = agent
        .get(url)
        .header("Cookie", &format!("zs={session}"))
        .call()
        .ok()
        .and_then(|mut r| r.body_mut().read_json::<serde_json::Value>().ok())
        .and_then(|j| {
            let email = j["email"].as_str().map(str::to_lowercase)?;
            // The main server says who runs zetlyn.com; their menu here leads to its admin pages.
            if j["operator"].as_bool() == Some(true) {
                operators().lock().unwrap_or_else(|e| e.into_inner()).insert(email.clone());
            }
            Some(email)
        });
    let mut kept = cache.lock().unwrap_or_else(|e| e.into_inner());
    if kept.len() > 10_000 {
        kept.clear();
    }
    kept.insert(session.to_string(), (now, email.clone()));
    email
}

/// The `zs` session in a Cookie header.
pub fn session_cookie(cookie: &str) -> Option<String> {
    cookie.split(';').filter_map(|p| p.trim().split_once('=')).find(|(k, _)| *k == "zs").map(|(_, v)| v.to_string())
}

/// The main server's address a cell sends people to for signing in and out: `https://zetlyn.com`.
pub fn identity_origin() -> Option<String> {
    IDENTITY.get().map(|u| u.trim_end_matches("/account/me").to_string())
}

/// Who runs zetlyn.com, as far as this process knows: the main server's owners, or whom the main
/// server said so of when a cell asked.
fn operators() -> &'static std::sync::Mutex<std::collections::BTreeSet<String>> {
    static OPERATORS: std::sync::OnceLock<std::sync::Mutex<std::collections::BTreeSet<String>>> = std::sync::OnceLock::new();
    OPERATORS.get_or_init(Default::default)
}

/// The main server's owners, noted as its operators.
pub fn note_operators(owners: &[String]) {
    let set = owners.iter().filter(|o| o.contains('@') && !o.contains(':')).map(|o| o.to_lowercase()).collect();
    *operators().lock().unwrap_or_else(|e| e.into_inner()) = set;
}

/// Whether an address runs zetlyn.com.
pub fn is_operator(email: &str) -> bool {
    operators().lock().unwrap_or_else(|e| e.into_inner()).contains(&email.to_lowercase())
}
