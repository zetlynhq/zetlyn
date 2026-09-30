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
    if viewer.entitled(scope) {
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
}

impl Viewer {
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
    pub fn spend_link(&self, raw: &str) -> Option<String> {
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
        let session = token();
        self.db
            .execute(
                "insert into session(hash, account, created, expires) values(?1,?2,?3,?4)",
                rusqlite::params![
                    digest(&session),
                    account,
                    crate::iso_stamp(crate::now()),
                    crate::iso_stamp(crate::now() + 60 * 60 * 24 * 30)
                ],
            )
            .ok()?;
        Some(session)
    }

    pub fn by_session(&self, raw: &str) -> Option<Account> {
        let hash = digest(raw);
        let expires: String = self
            .db
            .query_row(
                "select expires from session where hash = ?1",
                rusqlite::params![hash],
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
    #[serde(default)]
    pub price: Price,
    #[serde(default)]
    pub mail: Mail,
    /// Who the assist asks, if anyone. See assist.rs.
    #[serde(default)]
    pub assist: crate::assist::Config,
}

#[derive(Debug, Deserialize)]
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

    /// Hands the mailer the message on its standard input. Where none is named, the link goes to
    /// the operator's own terminal and the page says where to look.
    pub fn send(&self, to: &str, subject: &str, body: &str) -> Result<bool, String> {
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

/// The session cookie, or an API key. A key is for a system and carries no cookie; a session is
/// for a person and carries nothing else.
pub fn viewer_of(accounts: &Accounts, cookie: Option<&str>, authorization: Option<&str>) -> Viewer {
    if let Some(key) = authorization.and_then(|a| a.strip_prefix("Bearer ")) {
        if let Some(account) = accounts.by_key(key.trim()) {
            return Viewer {
                account: Some(account),
                by_key: true,
            };
        }
    }
    let session = cookie.and_then(|c| {
        c.split(';')
            .filter_map(|p| p.trim().split_once('='))
            .find(|(k, _)| *k == "zs")
            .map(|(_, v)| v.to_string())
    });
    match session.and_then(|s| accounts.by_session(&s)) {
        Some(account) => Viewer {
            account: Some(account),
            by_key: false,
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
        }
    }
}
