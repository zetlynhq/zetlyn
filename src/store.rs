//! Everything an instance knows, in one SQLite file: its accounts, their watches, every check and
//! every change, the links it mailed and the sessions they opened.
//!
//! An instance is one running Zetlyn with its own file. More servers are more instances, each
//! with its own accounts; the instance's name (`meta.instance`) goes with everything it hands out,
//! so a watch is `name/id` wherever it is seen. Run on its own, an instance has one local account
//! and nobody signs in.
//!
//! A watch is `pending` until its address is confirmed, `waiting` while it needs a subscription,
//! `running`, and `ended`. Its `status` is something else: how its last check came out.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::anchor::Anchor;

pub struct Store {
    conn: Connection,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    /// One marked place.
    Place(Anchor),
    /// The whole page, without the shapes of the lines that change on their own.
    Page { volatile: Vec<String> },
}

#[derive(Clone, Debug)]
pub struct Watch {
    pub id: i64,
    pub account: i64,
    pub url: String,
    pub mode: Mode,
    pub interval_min: i64,
    pub created: i64,
    /// Watched until then; none while the account pays or on a local instance.
    pub until: Option<i64>,
    pub last_checked: Option<i64>,
    pub last_value: Option<String>,
    /// The outcome of the last check: `new`, `same`, `changed`, `unclear`, `gone`, `failed`.
    pub status: String,
    pub state: String,
    pub reminded: bool,
    /// The last outcome the account was mailed about, so a place that stays unclear is said once.
    pub told: String,
    /// Read in Chrome after its scripts ran (`render.rs`), not as served.
    pub render: bool,
    /// The watch's name in addresses and the API (`crypto::short_id`): not its number, which would
    /// say how many there are and invite counting through them.
    pub key: String,
    /// The values a place takes in turn on its own (an A/B test, a box that says one thing or
    /// another on each read): moving between them is no change.
    pub variants: Vec<String>,
    /// When a change is told (`when.rs`); none for any change.
    pub condition: Option<crate::when::Condition>,
}

#[derive(Clone, Debug)]
pub struct Channel {
    pub id: i64,
    pub account: i64,
    /// `telegram`, `slack`, `discord` or `webhook`.
    pub kind: String,
    /// The chat id, or the webhook's address.
    pub target: String,
    pub secret: String,
    pub created: i64,
    pub last_at: Option<i64>,
    pub last_status: String,
}

#[derive(Clone, Debug)]
pub struct Account {
    pub id: i64,
    pub email: String,
    pub created: i64,
    pub trial_used: bool,
    pub quota: i64,
    pub confirmed: bool,
    pub paid: bool,
    pub stripe_customer: String,
    pub stripe_subscription: String,
    /// Given its subscription by an admin, not bought: no Stripe event takes it away.
    pub free: bool,
    /// Blocked by an admin: nobody signs in to it, its keys do not open it.
    pub blocked: bool,
}

pub struct Check {
    pub at: i64,
    pub outcome: String,
    pub moved: bool,
    pub detail: String,
}

pub struct Change {
    pub at: i64,
    pub before: String,
    pub after: String,
}

pub const LOCAL: &str = "local";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS accounts (
  id INTEGER PRIMARY KEY,
  email TEXT NOT NULL UNIQUE,
  created INTEGER NOT NULL,
  trial_used INTEGER NOT NULL DEFAULT 0,
  quota INTEGER NOT NULL DEFAULT 10,
  status TEXT NOT NULL DEFAULT 'active'
);
CREATE TABLE IF NOT EXISTS watches (
  id INTEGER PRIMARY KEY,
  account INTEGER NOT NULL REFERENCES accounts(id),
  url TEXT NOT NULL,
  mode TEXT NOT NULL,
  anchor TEXT,
  volatile TEXT NOT NULL DEFAULT '[]',
  interval_min INTEGER NOT NULL,
  created INTEGER NOT NULL,
  until INTEGER,
  last_checked INTEGER,
  last_value TEXT,
  status TEXT NOT NULL DEFAULT 'new'
);
CREATE TABLE IF NOT EXISTS checks (
  id INTEGER PRIMARY KEY,
  watch INTEGER NOT NULL REFERENCES watches(id) ON DELETE CASCADE,
  at INTEGER NOT NULL,
  outcome TEXT NOT NULL,
  moved INTEGER NOT NULL DEFAULT 0,
  detail TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS changes (
  id INTEGER PRIMARY KEY,
  watch INTEGER NOT NULL REFERENCES watches(id) ON DELETE CASCADE,
  at INTEGER NOT NULL,
  before TEXT NOT NULL,
  after TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS tokens (
  digest TEXT PRIMARY KEY,
  account INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  watch INTEGER,
  expires INTEGER NOT NULL,
  used INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS sessions (
  digest TEXT PRIMARY KEY,
  account INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  expires INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS events (id TEXT PRIMARY KEY, at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS api_keys (
  id INTEGER PRIMARY KEY,
  account INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  digest TEXT NOT NULL UNIQUE,
  name TEXT NOT NULL,
  hint TEXT NOT NULL,
  created INTEGER NOT NULL,
  last_used INTEGER
);
CREATE TABLE IF NOT EXISTS admin_log (
  id INTEGER PRIMARY KEY,
  at INTEGER NOT NULL,
  admin TEXT NOT NULL,
  action TEXT NOT NULL,
  target TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS oauth_clients (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  redirect_uris TEXT NOT NULL,
  created INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS oauth_grants (
  digest TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  client TEXT NOT NULL REFERENCES oauth_clients(id) ON DELETE CASCADE,
  account INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  redirect_uri TEXT NOT NULL DEFAULT '',
  challenge TEXT NOT NULL DEFAULT '',
  created INTEGER NOT NULL,
  expires INTEGER NOT NULL,
  used INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS channels (
  id INTEGER PRIMARY KEY,
  account INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  target TEXT NOT NULL,
  secret TEXT NOT NULL DEFAULT '',
  created INTEGER NOT NULL,
  last_at INTEGER,
  last_status TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS oauth_grants_account ON oauth_grants(account, client);
CREATE INDEX IF NOT EXISTS checks_watch ON checks(watch, at);
CREATE INDEX IF NOT EXISTS checks_at ON checks(at);
CREATE INDEX IF NOT EXISTS changes_watch ON changes(watch, at);
";

/// Columns added since the first trial file was made, added to a file that lacks them.
const ADDED: &[(&str, &str, &str)] = &[
    ("accounts", "confirmed", "INTEGER NOT NULL DEFAULT 0"),
    ("accounts", "paid", "INTEGER NOT NULL DEFAULT 0"),
    ("accounts", "stripe_customer", "TEXT NOT NULL DEFAULT ''"),
    ("accounts", "stripe_subscription", "TEXT NOT NULL DEFAULT ''"),
    ("watches", "state", "TEXT NOT NULL DEFAULT 'running'"),
    ("watches", "reminded", "INTEGER NOT NULL DEFAULT 0"),
    ("watches", "told", "TEXT NOT NULL DEFAULT ''"),
    ("accounts", "free", "INTEGER NOT NULL DEFAULT 0"),
    ("events", "kind", "TEXT NOT NULL DEFAULT ''"),
    ("watches", "render", "INTEGER NOT NULL DEFAULT 0"),
    ("watches", "key", "TEXT NOT NULL DEFAULT ''"),
    ("accounts", "checkout_session", "TEXT NOT NULL DEFAULT ''"),
    ("tokens", "payload", "TEXT NOT NULL DEFAULT ''"),
    ("watches", "variants", "TEXT NOT NULL DEFAULT '[]'"),
    ("watches", "condition", "TEXT NOT NULL DEFAULT ''"),
    ("accounts", "mail_changes", "INTEGER NOT NULL DEFAULT 1"),
];

const WATCH: &str = "SELECT id, account, url, mode, anchor, volatile, interval_min, created, until, last_checked, last_value, status, state, reminded, told, render, key, variants, condition FROM watches";
const ACCOUNT: &str = "SELECT id, email, created, trial_used, quota, confirmed, paid, stripe_customer, stripe_subscription, free, status = 'blocked' FROM accounts";

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn e(err: rusqlite::Error) -> String {
    err.to_string()
}

impl Store {
    pub fn open(dir: &Path) -> Result<Store, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let conn = Connection::open(dir.join("zetlyn.db")).map_err(e)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;").map_err(e)?;
        Store::ready(conn)
    }

    #[cfg(test)]
    pub fn in_memory() -> Store {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        Store::ready(conn).unwrap()
    }

    fn ready(conn: Connection) -> Result<Store, String> {
        conn.execute_batch(SCHEMA).map_err(e)?;
        for (table, column, kind) in ADDED {
            let has: bool = conn.prepare(&format!("SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?1")).and_then(|mut s| s.exists([column])).map_err(e)?;
            if !has {
                conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {kind}")).map_err(e)?;
            }
        }
        // Watches made before they had a key get one.
        let keyless: Vec<i64> = conn.prepare("SELECT id FROM watches WHERE key = ''").and_then(|mut s| s.query_map([], |r| r.get(0))?.collect()).map_err(e)?;
        for id in keyless {
            conn.execute("UPDATE watches SET key = ?2 WHERE id = ?1", params![id, crate::crypto::short_id()]).map_err(e)?;
        }
        conn.execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS watches_key ON watches(key)").map_err(e)?;
        Ok(Store { conn })
    }

    /// The instance's name, set once; `local` until it is given one.
    pub fn instance(&self) -> String {
        self.conn.query_row("SELECT value FROM meta WHERE key = 'instance'", [], |r| r.get(0)).optional().ok().flatten().unwrap_or_else(|| LOCAL.to_string())
    }

    pub fn name_instance(&self, name: &str) -> Result<(), String> {
        self.conn.execute("INSERT INTO meta (key, value) VALUES ('instance', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [name]).map(|_| ()).map_err(e)
    }

    /// The account of an instance nobody signs in to.
    pub fn local_account(&self) -> Result<i64, String> {
        self.account(LOCAL)
    }

    /// The account of an address, made the first time the address is seen.
    pub fn account(&self, email: &str) -> Result<i64, String> {
        let email = email.trim().to_lowercase();
        self.conn.execute("INSERT OR IGNORE INTO accounts (email, created) VALUES (?1, ?2)", params![email, now()]).map_err(e)?;
        self.conn.query_row("SELECT id FROM accounts WHERE email = ?1", [email], |r| r.get(0)).map_err(e)
    }

    pub fn account_by_id(&self, id: i64) -> Result<Option<Account>, String> {
        self.accounts(&format!("{ACCOUNT} WHERE id = ?1"), [id]).map(|mut v| v.pop())
    }

    pub fn account_by_customer(&self, customer: &str) -> Result<Option<Account>, String> {
        self.accounts(&format!("{ACCOUNT} WHERE stripe_customer = ?1 AND stripe_customer != ''"), [customer]).map(|mut v| v.pop())
    }

    pub fn confirm_account(&self, id: i64) -> Result<(), String> {
        self.conn.execute("UPDATE accounts SET confirmed = 1 WHERE id = ?1", [id]).map(|_| ()).map_err(e)
    }

    /// Takes the account's one free trial, if it is still there.
    pub fn take_trial(&self, id: i64) -> Result<bool, String> {
        self.conn.execute("UPDATE accounts SET trial_used = 1 WHERE id = ?1 AND trial_used = 0", [id]).map(|n| n == 1).map_err(e)
    }

    pub fn set_paid(&self, id: i64, paid: bool, customer: &str, subscription: &str) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE accounts SET paid = ?2, stripe_customer = CASE WHEN ?3 = '' THEN stripe_customer ELSE ?3 END, stripe_subscription = CASE WHEN ?4 = '' THEN stripe_subscription ELSE ?4 END WHERE id = ?1",
                params![id, paid, customer, subscription],
            )
            .map(|_| ())
            .map_err(e)
    }

    pub fn add_watch(&self, account: i64, url: &str, mode: &Mode, interval_min: i64, until: Option<i64>, value: &str) -> Result<i64, String> {
        self.add_watch_as(account, url, mode, interval_min, until, value, "running")
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_watch_as(&self, account: i64, url: &str, mode: &Mode, interval_min: i64, until: Option<i64>, value: &str, state: &str) -> Result<i64, String> {
        let (kind, anchor, volatile) = match mode {
            Mode::Place(a) => ("place", Some(serde_json::to_string(a).map_err(|x| x.to_string())?), "[]".to_string()),
            Mode::Page { volatile } => ("page", None, serde_json::to_string(volatile).map_err(|x| x.to_string())?),
        };
        let at = now();
        self.conn
            .execute(
                "INSERT INTO watches (account, url, mode, anchor, volatile, interval_min, created, until, last_checked, last_value, state, key) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?7, ?9, ?10, ?11)",
                params![account, url, kind, anchor, volatile, interval_min, at, until, value, state, crate::crypto::short_id()],
            )
            .map_err(e)?;
        let id = self.conn.last_insert_rowid();
        self.conn.execute("INSERT INTO checks (watch, at, outcome, detail) VALUES (?1, ?2, 'new', ?3)", params![id, at, value]).map_err(e)?;
        Ok(id)
    }

    /// A watch moved to `state`, watched until `until`, its clock started again when it starts.
    pub fn set_state(&self, watch: i64, state: &str, until: Option<i64>) -> Result<(), String> {
        let at = now();
        self.conn.execute("UPDATE watches SET state = ?2, until = ?3, reminded = 0, last_checked = CASE WHEN ?2 = 'running' AND state != 'running' THEN ?4 ELSE last_checked END WHERE id = ?1", params![watch, state, until, at]).map(|_| ()).map_err(e)
    }

    /// The place marked again: a new anchor (or the whole page) and the value read with it.
    pub fn remark(&self, watch: i64, mode: &Mode, value: &str) -> Result<(), String> {
        let (kind, anchor, volatile) = match mode {
            Mode::Place(a) => ("place", Some(serde_json::to_string(a).map_err(|x| x.to_string())?), "[]".to_string()),
            Mode::Page { volatile } => ("page", None, serde_json::to_string(volatile).map_err(|x| x.to_string())?),
        };
        self.conn.execute("UPDATE watches SET mode = ?2, anchor = ?3, volatile = ?4, last_value = ?5, status = 'new', told = '', variants = '[]' WHERE id = ?1", params![watch, kind, anchor, volatile, value]).map_err(e)?;
        self.conn.execute("INSERT INTO checks (watch, at, outcome, detail) VALUES (?1, ?2, 'new', ?3)", params![watch, now(), value]).map(|_| ()).map_err(e)
    }

    /// More shapes of lines that change on their own, learned after the watch began.
    pub fn add_volatile(&self, watch: i64, shapes: &[String]) -> Result<(), String> {
        let Some(w) = self.watch(watch)? else { return Ok(()) };
        let Mode::Page { mut volatile } = w.mode else { return Ok(()) };
        for s in shapes {
            if !volatile.contains(s) {
                volatile.push(s.clone());
            }
        }
        volatile.sort();
        let json = serde_json::to_string(&volatile).map_err(|x| x.to_string())?;
        self.conn.execute("UPDATE watches SET volatile = ?2 WHERE id = ?1", params![watch, json]).map(|_| ()).map_err(e)
    }

    /// Whether changes are mailed to the account's address; another channel may take its place.
    pub fn mails_changes(&self, account: i64) -> Result<bool, String> {
        self.conn.query_row("SELECT mail_changes FROM accounts WHERE id = ?1", [account], |r| r.get(0)).map_err(e)
    }

    pub fn set_mails_changes(&self, account: i64, on: bool) -> Result<(), String> {
        self.conn.execute("UPDATE accounts SET mail_changes = ?2 WHERE id = ?1", params![account, on]).map(|_| ()).map_err(e)
    }

    /// Another place changes are told (`tell.rs`): a Telegram chat, a Slack or Discord webhook, a
    /// webhook of the account's own with the secret its deliveries are signed with.
    pub fn add_channel(&self, account: i64, kind: &str, target: &str, secret: &str) -> Result<i64, String> {
        self.conn.execute("DELETE FROM channels WHERE account = ?1 AND kind = ?2 AND target = ?3", params![account, kind, target]).map_err(e)?;
        self.conn.execute("INSERT INTO channels (account, kind, target, secret, created) VALUES (?1, ?2, ?3, ?4, ?5)", params![account, kind, target, secret, now()]).map_err(e)?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn channels_of(&self, account: i64) -> Result<Vec<Channel>, String> {
        let mut st = self.conn.prepare("SELECT id, account, kind, target, secret, created, last_at, last_status FROM channels WHERE account = ?1 ORDER BY id").map_err(e)?;
        let rows = st
            .query_map([account], |r| Ok(Channel { id: r.get(0)?, account: r.get(1)?, kind: r.get(2)?, target: r.get(3)?, secret: r.get(4)?, created: r.get(5)?, last_at: r.get(6)?, last_status: r.get(7)? }))
            .map_err(e)?;
        rows.collect::<Result<_, _>>().map_err(e)
    }

    pub fn delete_channel(&self, account: i64, id: i64) -> Result<(), String> {
        self.conn.execute("DELETE FROM channels WHERE id = ?1 AND account = ?2", params![id, account]).map(|_| ()).map_err(e)
    }

    /// A Telegram chat that said /stop: let go of, whichever account it was with.
    pub fn delete_telegram_chat(&self, chat: &str) -> Result<usize, String> {
        self.conn.execute("DELETE FROM channels WHERE kind = 'telegram' AND target = ?1", [chat]).map_err(e)
    }

    /// How the last delivery to a channel went: `ok`, or what went wrong.
    pub fn set_channel_status(&self, id: i64, status: &str) -> Result<(), String> {
        self.conn.execute("UPDATE channels SET last_at = ?2, last_status = ?3 WHERE id = ?1", params![id, now(), status]).map(|_| ()).map_err(e)
    }

    /// When a change of the watch is told; none for any change.
    pub fn set_condition(&self, watch: i64, condition: Option<&crate::when::Condition>) -> Result<(), String> {
        let json = condition.map(serde_json::to_string).transpose().map_err(|x| x.to_string())?.unwrap_or_default();
        self.conn.execute("UPDATE watches SET condition = ?2 WHERE id = ?1", params![watch, json]).map(|_| ()).map_err(e)
    }

    /// More values a place was seen flipping between on its own (`check::variants_of`).
    pub fn add_variants(&self, watch: i64, shapes: &[String]) -> Result<(), String> {
        if shapes.is_empty() {
            return Ok(());
        }
        let Some(mut w) = self.watch(watch)? else { return Ok(()) };
        for s in shapes {
            if !w.variants.contains(s) {
                w.variants.push(s.clone());
            }
        }
        let json = serde_json::to_string(&w.variants).map_err(|x| x.to_string())?;
        self.conn.execute("UPDATE watches SET variants = ?2 WHERE id = ?1", params![watch, json]).map(|_| ()).map_err(e)
    }

    pub fn set_last_value(&self, watch: i64, value: &str) -> Result<(), String> {
        self.conn.execute("UPDATE watches SET last_value = ?2 WHERE id = ?1", params![watch, value]).map(|_| ()).map_err(e)
    }

    /// A consistent copy of the whole file, taken while the instance runs. Refuses to write over
    /// a file that is there.
    pub fn backup(&self, to: &Path) -> Result<(), String> {
        if to.exists() {
            return Err(format!("{}: there is a file there already", to.display()));
        }
        self.conn.execute("VACUUM INTO ?1", [to.to_string_lossy()]).map(|_| ()).map_err(e)
    }

    /// The running watches whose check is more than `grace` seconds late at `at`: checks that
    /// have stopped.
    pub fn overdue(&self, at: i64, grace: i64) -> Result<Vec<Watch>, String> {
        Ok(self.select(&format!("{WATCH} WHERE state = 'running' ORDER BY id"), [])?.into_iter().filter(|w| w.until.map_or(true, |u| u > at) && w.last_checked.is_some_and(|c| at - c > w.interval_min * 60 + grace)).collect())
    }

    /// How many checks there were since `since`, and how many of them could not read their page.
    pub fn outcomes_since(&self, since: i64) -> Result<(i64, i64), String> {
        self.conn
            .query_row("SELECT COUNT(*), COALESCE(SUM(outcome = 'failed'), 0) FROM checks WHERE outcome != 'new' AND at >= ?1", [since], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(e)
    }

    /// Whether a watch is read in Chrome, after the page's scripts ran.
    pub fn set_render(&self, watch: i64, render: bool) -> Result<(), String> {
        self.conn.execute("UPDATE watches SET render = ?2 WHERE id = ?1", params![watch, render]).map(|_| ()).map_err(e)
    }

    /// How often a watch is checked, in minutes.
    pub fn set_interval(&self, watch: i64, minutes: i64) -> Result<(), String> {
        self.conn.execute("UPDATE watches SET interval_min = ?2 WHERE id = ?1", params![watch, minutes]).map(|_| ()).map_err(e)
    }

    pub fn set_reminded(&self, watch: i64) -> Result<(), String> {
        self.conn.execute("UPDATE watches SET reminded = 1 WHERE id = ?1", [watch]).map(|_| ()).map_err(e)
    }

    pub fn set_told(&self, watch: i64, told: &str) -> Result<(), String> {
        self.conn.execute("UPDATE watches SET told = ?2 WHERE id = ?1", params![watch, told]).map(|_| ()).map_err(e)
    }

    pub fn watches(&self) -> Result<Vec<Watch>, String> {
        self.select(&format!("{WATCH} ORDER BY id DESC"), [])
    }

    pub fn watches_of(&self, account: i64) -> Result<Vec<Watch>, String> {
        self.select(&format!("{WATCH} WHERE account = ?1 ORDER BY id DESC"), [account])
    }

    pub fn watch_by_key(&self, key: &str) -> Result<Option<Watch>, String> {
        self.select(&format!("{WATCH} WHERE key = ?1"), [key]).map(|mut v| v.pop())
    }

    pub fn watch(&self, id: i64) -> Result<Option<Watch>, String> {
        Ok(self.select(&format!("{WATCH} WHERE id = ?1"), [id])?.pop())
    }

    /// The running watches whose time has come at `at`: not past their end, not checked for an
    /// interval.
    pub fn due(&self, at: i64) -> Result<Vec<Watch>, String> {
        Ok(self.select(&format!("{WATCH} WHERE state = 'running' ORDER BY id"), [])?.into_iter().filter(|w| w.until.map_or(true, |u| u > at) && w.last_checked.map_or(true, |c| at - c >= w.interval_min * 60)).collect())
    }

    /// The running watches that are past their end at `at`.
    pub fn expired(&self, at: i64) -> Result<Vec<Watch>, String> {
        self.select(&format!("{WATCH} WHERE state = 'running' AND until IS NOT NULL AND until <= ?1 ORDER BY id"), [at])
    }

    /// The running watches that end within `within` seconds of `at` and whose account has not
    /// been told.
    pub fn ending(&self, at: i64, within: i64) -> Result<Vec<Watch>, String> {
        self.select(&format!("{WATCH} WHERE state = 'running' AND reminded = 0 AND until IS NOT NULL AND until > ?1 AND until <= ?2 ORDER BY id"), [at, at + within])
    }

    /// One check: its outcome, and the watch's new value and the change when there is one.
    pub fn record(&self, watch: i64, at: i64, outcome: &str, moved: bool, detail: &str, change: Option<(&str, &str)>) -> Result<(), String> {
        self.conn.execute("INSERT INTO checks (watch, at, outcome, moved, detail) VALUES (?1, ?2, ?3, ?4, ?5)", params![watch, at, outcome, moved, detail]).map_err(e)?;
        self.conn.execute("UPDATE watches SET last_checked = ?2, status = ?3 WHERE id = ?1", params![watch, at, outcome]).map_err(e)?;
        if let Some((before, after)) = change {
            self.conn.execute("INSERT INTO changes (watch, at, before, after) VALUES (?1, ?2, ?3, ?4)", params![watch, at, before, after]).map_err(e)?;
            self.conn.execute("UPDATE watches SET last_value = ?2 WHERE id = ?1", params![watch, after]).map_err(e)?;
        }
        Ok(())
    }

    pub fn checks(&self, watch: i64, limit: i64) -> Result<Vec<Check>, String> {
        let mut st = self.conn.prepare("SELECT at, outcome, moved, detail FROM checks WHERE watch = ?1 ORDER BY at DESC, id DESC LIMIT ?2").map_err(e)?;
        let rows = st.query_map(params![watch, limit], |r| Ok(Check { at: r.get(0)?, outcome: r.get(1)?, moved: r.get(2)?, detail: r.get(3)? })).map_err(e)?;
        rows.collect::<Result<_, _>>().map_err(e)
    }

    pub fn check_count(&self, watch: i64) -> Result<i64, String> {
        self.conn.query_row("SELECT COUNT(*) FROM checks WHERE watch = ?1 AND outcome != 'new'", [watch], |r| r.get(0)).map_err(e)
    }

    /// The changes on an account's watches since `since`, newest first, each with its watch's key
    /// and address.
    pub fn changes_of(&self, account: i64, since: i64, limit: i64) -> Result<Vec<(String, String, Change)>, String> {
        let mut st = self
            .conn
            .prepare("SELECT w.key, w.url, c.at, c.before, c.after FROM changes c JOIN watches w ON w.id = c.watch WHERE w.account = ?1 AND c.at >= ?2 ORDER BY c.at DESC, c.id DESC LIMIT ?3")
            .map_err(e)?;
        let rows = st.query_map(params![account, since, limit], |r| Ok((r.get(0)?, r.get(1)?, Change { at: r.get(2)?, before: r.get(3)?, after: r.get(4)? }))).map_err(e)?;
        rows.collect::<Result<_, _>>().map_err(e)
    }

    pub fn changes(&self, watch: i64, limit: i64) -> Result<Vec<Change>, String> {
        let mut st = self.conn.prepare("SELECT at, before, after FROM changes WHERE watch = ?1 ORDER BY at DESC, id DESC LIMIT ?2").map_err(e)?;
        let rows = st.query_map(params![watch, limit], |r| Ok(Change { at: r.get(0)?, before: r.get(1)?, after: r.get(2)? })).map_err(e)?;
        rows.collect::<Result<_, _>>().map_err(e)
    }

    /// How often each outcome came up, over every check after the first: what the trial of
    /// finding places again is measured by.
    pub fn tally(&self) -> Result<Vec<(String, i64, i64)>, String> {
        let mut st = self.conn.prepare("SELECT outcome, COUNT(*), SUM(moved) FROM checks WHERE outcome != 'new' GROUP BY outcome ORDER BY COUNT(*) DESC").map_err(e)?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<i64>>(2)?.unwrap_or(0)))).map_err(e)?;
        rows.collect::<Result<_, _>>().map_err(e)
    }

    pub fn delete(&self, id: i64) -> Result<(), String> {
        self.conn.execute("DELETE FROM watches WHERE id = ?1", [id]).map(|_| ()).map_err(e)
    }

    /// A link mailed for one use: confirming a watch, signing in. Returns the token, of which
    /// only the digest is kept.
    pub fn new_token(&self, account: i64, kind: &str, watch: Option<i64>, ttl: i64) -> Result<String, String> {
        let token = crate::crypto::token();
        self.conn.execute("INSERT INTO tokens (digest, account, kind, watch, expires) VALUES (?1, ?2, ?3, ?4, ?5)", params![crate::crypto::digest(&token), account, kind, watch, now() + ttl]).map_err(e)?;
        Ok(token)
    }

    /// The account (and watch) a link was for, once, while it is fresh.
    pub fn take_token(&self, token: &str, kind: &str) -> Result<Option<(i64, Option<i64>)>, String> {
        let digest = crate::crypto::digest(token);
        let found: Option<(i64, Option<i64>)> = self
            .conn
            .query_row("SELECT account, watch FROM tokens WHERE digest = ?1 AND kind = ?2 AND used = 0 AND expires > ?3", params![digest, kind, now()], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
            .map_err(e)?;
        if found.is_some() {
            self.conn.execute("UPDATE tokens SET used = 1 WHERE digest = ?1", [digest]).map_err(e)?;
        }
        Ok(found)
    }

    /// A link mailed for one use that carries something with it: a new address to confirm.
    pub fn new_token_for(&self, account: i64, kind: &str, payload: &str, ttl: i64) -> Result<String, String> {
        let token = crate::crypto::token();
        self.conn
            .execute("INSERT INTO tokens (digest, account, kind, expires, payload) VALUES (?1, ?2, ?3, ?4, ?5)", params![crate::crypto::digest(&token), account, kind, now() + ttl, payload])
            .map_err(e)?;
        Ok(token)
    }

    /// The account a link is for, while it is fresh, without using it up: a link that may be
    /// opened again (a person who left Stripe's checkout and comes back).
    pub fn token_holder(&self, token: &str, kind: &str) -> Result<Option<i64>, String> {
        self.conn
            .query_row("SELECT account FROM tokens WHERE digest = ?1 AND kind = ?2 AND used = 0 AND expires > ?3", params![crate::crypto::digest(token), kind, now()], |r| r.get(0))
            .optional()
            .map_err(e)
    }

    /// The account a link was for and what it carries, once, while it is fresh.
    pub fn take_token_payload(&self, token: &str, kind: &str) -> Result<Option<(i64, String)>, String> {
        let digest = crate::crypto::digest(token);
        let found: Option<(i64, String)> = self
            .conn
            .query_row("SELECT account, payload FROM tokens WHERE digest = ?1 AND kind = ?2 AND used = 0 AND expires > ?3", params![digest, kind, now()], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
            .map_err(e)?;
        if found.is_some() {
            self.conn.execute("UPDATE tokens SET used = 1 WHERE digest = ?1", [digest]).map_err(e)?;
        }
        Ok(found)
    }

    /// An account's address changed; false when another account has it already.
    pub fn set_email(&self, id: i64, email: &str) -> Result<bool, String> {
        let email = email.trim().to_lowercase();
        let taken: bool = self.conn.query_row("SELECT EXISTS(SELECT 1 FROM accounts WHERE email = ?1 AND id != ?2)", params![email, id], |r| r.get(0)).map_err(e)?;
        if taken {
            return Ok(false);
        }
        self.conn.execute("UPDATE accounts SET email = ?2 WHERE id = ?1", params![id, email]).map_err(e)?;
        Ok(true)
    }

    pub fn new_session(&self, account: i64) -> Result<String, String> {
        let token = crate::crypto::token();
        self.conn.execute("INSERT INTO sessions (digest, account, expires) VALUES (?1, ?2, ?3)", params![crate::crypto::digest(&token), account, now() + 90 * 86400]).map_err(e)?;
        Ok(token)
    }

    pub fn session(&self, token: &str) -> Result<Option<i64>, String> {
        self.conn.query_row("SELECT s.account FROM sessions s JOIN accounts a ON a.id = s.account WHERE s.digest = ?1 AND s.expires > ?2 AND a.status != 'blocked'", params![crate::crypto::digest(token), now()], |r| r.get(0)).optional().map_err(e)
    }

    pub fn end_session(&self, token: &str) -> Result<(), String> {
        self.conn.execute("DELETE FROM sessions WHERE digest = ?1", [crate::crypto::digest(token)]).map(|_| ()).map_err(e)
    }

    /// A watch never confirmed goes after two days. Returns how many went.
    pub fn clean_pending(&self, at: i64) -> Result<usize, String> {
        self.conn.execute("DELETE FROM watches WHERE state = 'pending' AND created < ?1", [at - 2 * 86400]).map_err(e)
    }

    /// For a plan that takes payment: a watch that waited a month for a subscription goes, and an
    /// account without one goes, with all it holds, 30 days after its last watch ended. Returns
    /// how many watches and accounts went.
    pub fn clean_unpaid(&self, at: i64) -> Result<(usize, usize), String> {
        let watches = self.conn.execute("DELETE FROM watches WHERE state = 'waiting' AND created < ?1", [at - 30 * 86400]).map_err(e)?;
        let stale: Vec<i64> = {
            let mut st = self
                .conn
                .prepare(
                    "SELECT a.id FROM accounts a WHERE a.email != ?1 AND a.paid = 0
                       AND NOT EXISTS (SELECT 1 FROM watches w WHERE w.account = a.id AND w.state IN ('pending', 'waiting', 'running'))
                       AND COALESCE((SELECT MAX(COALESCE(w.until, w.last_checked, w.created)) FROM watches w WHERE w.account = a.id), a.created) < ?2",
                )
                .map_err(e)?;
            let rows = st.query_map(params![LOCAL, at - 30 * 86400], |r| r.get(0)).map_err(e)?;
            rows.collect::<Result<_, _>>().map_err(e)?
        };
        for id in &stale {
            self.conn.execute("DELETE FROM watches WHERE account = ?1", [id]).map_err(e)?;
            self.conn.execute("DELETE FROM accounts WHERE id = ?1", [id]).map_err(e)?;
        }
        Ok((watches, stale.len()))
    }

    /// A key for the API and MCP, for `account`: `zk_` and 64 hex characters, shown once; only its
    /// digest and its last four characters are kept.
    pub fn new_api_key(&self, account: i64, name: &str) -> Result<String, String> {
        let key = format!("zk_{}", crate::crypto::token());
        let name: String = name.trim().chars().take(60).collect();
        let name = if name.is_empty() { "API key".to_string() } else { name };
        self.conn
            .execute("INSERT INTO api_keys (account, digest, name, hint, created) VALUES (?1, ?2, ?3, ?4, ?5)", params![account, crate::crypto::digest(&key), name, &key[key.len() - 4..], now()])
            .map_err(e)?;
        Ok(key)
    }

    /// The account a key belongs to, noting that it was used.
    pub fn api_account(&self, key: &str) -> Result<Option<i64>, String> {
        let digest = crate::crypto::digest(key.trim());
        let found: Option<i64> = self.conn.query_row("SELECT k.account FROM api_keys k JOIN accounts a ON a.id = k.account WHERE k.digest = ?1 AND a.status != 'blocked'", [&digest], |r| r.get(0)).optional().map_err(e)?;
        if found.is_some() {
            self.conn.execute("UPDATE api_keys SET last_used = ?2 WHERE digest = ?1", params![digest, now()]).map_err(e)?;
        }
        Ok(found)
    }

    /// An account's keys: id, name, last four characters, made, last used.
    pub fn api_keys_of(&self, account: i64) -> Result<Vec<(i64, String, String, i64, Option<i64>)>, String> {
        let mut st = self.conn.prepare("SELECT id, name, hint, created, last_used FROM api_keys WHERE account = ?1 ORDER BY id").map_err(e)?;
        let rows = st.query_map([account], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).map_err(e)?;
        rows.collect::<Result<_, _>>().map_err(e)
    }

    // OAuth for the MCP server (`serve/authz.rs`): clients that registered themselves, and what
    // they were given: a code to trade once, access for an hour, a refresh for 90 days. Of each,
    // only the digest is kept.

    /// A client registered: its id.
    pub fn add_client(&self, name: &str, redirect_uris: &[String]) -> Result<String, String> {
        let id = format!("zc_{}", &crate::crypto::token()[..32]);
        let uris = serde_json::to_string(redirect_uris).map_err(|x| x.to_string())?;
        self.conn.execute("INSERT INTO oauth_clients (id, name, redirect_uris, created) VALUES (?1, ?2, ?3, ?4)", params![id, name, uris, now()]).map_err(e)?;
        Ok(id)
    }

    /// A client's name and the addresses it may be sent back to.
    pub fn client(&self, id: &str) -> Result<Option<(String, Vec<String>)>, String> {
        let row: Option<(String, String)> = self.conn.query_row("SELECT name, redirect_uris FROM oauth_clients WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).optional().map_err(e)?;
        Ok(row.map(|(name, uris)| (name, serde_json::from_str(&uris).unwrap_or_default())))
    }

    /// Something given to a client for an account: `code`, `access` or `refresh`. Returns it,
    /// with a prefix that says which.
    pub fn new_grant(&self, kind: &str, client: &str, account: i64, redirect_uri: &str, challenge: &str, ttl: i64) -> Result<String, String> {
        let prefix = match kind {
            "code" => "zcd_",
            "access" => "zat_",
            _ => "zrt_",
        };
        let token = format!("{prefix}{}", crate::crypto::token());
        self.conn
            .execute(
                "INSERT INTO oauth_grants (digest, kind, client, account, redirect_uri, challenge, created, expires) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![crate::crypto::digest(&token), kind, client, account, redirect_uri, challenge, now(), now() + ttl],
            )
            .map_err(e)?;
        Ok(token)
    }

    /// A code or a refresh, taken once while fresh: its client, account, redirect address and
    /// challenge.
    pub fn take_grant(&self, token: &str, kind: &str) -> Result<Option<(String, i64, String, String)>, String> {
        let digest = crate::crypto::digest(token);
        let found: Option<(String, i64, String, String)> = self
            .conn
            .query_row(
                "SELECT client, account, redirect_uri, challenge FROM oauth_grants WHERE digest = ?1 AND kind = ?2 AND used = 0 AND expires > ?3",
                params![digest, kind, now()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(e)?;
        if found.is_some() {
            self.conn.execute("UPDATE oauth_grants SET used = 1 WHERE digest = ?1", [digest]).map_err(e)?;
        }
        Ok(found)
    }

    /// The account an access token opens, while it is fresh and its account not blocked.
    pub fn access_account(&self, token: &str) -> Result<Option<i64>, String> {
        self.conn
            .query_row(
                "SELECT g.account FROM oauth_grants g JOIN accounts a ON a.id = g.account WHERE g.digest = ?1 AND g.kind = 'access' AND g.expires > ?2 AND a.status != 'blocked'",
                params![crate::crypto::digest(token), now()],
                |r| r.get(0),
            )
            .optional()
            .map_err(e)
    }

    /// The clients an account let in, with when it first did, newest first.
    pub fn clients_of(&self, account: i64) -> Result<Vec<(String, String, i64)>, String> {
        let mut st = self
            .conn
            .prepare("SELECT c.id, c.name, MIN(g.created) FROM oauth_grants g JOIN oauth_clients c ON c.id = g.client WHERE g.account = ?1 AND g.kind != 'code' AND g.expires > ?2 GROUP BY c.id ORDER BY 3 DESC")
            .map_err(e)?;
        let rows = st.query_map(params![account, now()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(e)?;
        rows.collect::<Result<_, _>>().map_err(e)
    }

    /// A client let out of an account: all it was given goes.
    pub fn disconnect(&self, account: i64, client: &str) -> Result<(), String> {
        self.conn.execute("DELETE FROM oauth_grants WHERE account = ?1 AND client = ?2", params![account, client]).map(|_| ()).map_err(e)
    }

    /// What expired a day ago goes; clients nobody uses go after 30 days.
    pub fn clean_grants(&self, at: i64) -> Result<(), String> {
        self.conn.execute("DELETE FROM oauth_grants WHERE expires < ?1", [at - 86400]).map_err(e)?;
        self.conn
            .execute("DELETE FROM oauth_clients WHERE created < ?1 AND NOT EXISTS (SELECT 1 FROM oauth_grants g WHERE g.client = oauth_clients.id)", [at - 30 * 86400])
            .map(|_| ())
            .map_err(e)
    }

    pub fn delete_api_key(&self, account: i64, id: i64) -> Result<(), String> {
        self.conn.execute("DELETE FROM api_keys WHERE id = ?1 AND account = ?2", params![id, account]).map(|_| ()).map_err(e)
    }

    /// Whether a Stripe event is seen for the first time; Stripe sends some more than once.
    pub fn first_event(&self, id: &str, kind: &str) -> Result<bool, String> {
        self.conn.execute("INSERT OR IGNORE INTO events (id, at, kind) VALUES (?1, ?2, ?3)", params![id, now(), kind]).map(|n| n == 1).map_err(e)
    }

    // What the admin pages read and do (`serve/admin.rs`).

    /// Accounts whose address contains `q` (all with an empty one), newest first.
    pub fn find_accounts(&self, q: &str, limit: i64) -> Result<Vec<Account>, String> {
        self.accounts(&format!("{ACCOUNT} WHERE email LIKE ?1 ESCAPE '\\' ORDER BY id DESC LIMIT ?2"), params![format!("%{}%", q.trim().to_lowercase().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")), limit])
    }

    /// Per account: its watches running, and all it has.
    pub fn watch_counts(&self) -> Result<std::collections::HashMap<i64, (i64, i64)>, String> {
        let mut st = self.conn.prepare("SELECT account, SUM(state = 'running'), COUNT(*) FROM watches GROUP BY account").map_err(e)?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?)))).map_err(e)?;
        rows.collect::<Result<_, _>>().map_err(e)
    }

    /// How many rows a query counts, for the overview.
    pub fn count(&self, sql: &str, p: impl rusqlite::Params) -> Result<i64, String> {
        self.conn.query_row(sql, p, |r| r.get(0)).map_err(e)
    }

    /// Rows of (key, count) from a query that groups, for the overview.
    pub fn counts(&self, sql: &str, p: impl rusqlite::Params) -> Result<Vec<(String, i64)>, String> {
        let mut st = self.conn.prepare(sql).map_err(e)?;
        let rows = st.query_map(p, |r| Ok((r.get(0)?, r.get(1)?))).map_err(e)?;
        rows.collect::<Result<_, _>>().map_err(e)
    }

    /// The checks since `since` that did not read what they should: (url, outcome, detail, at).
    pub fn misses_since(&self, since: i64) -> Result<Vec<(String, String, String, i64)>, String> {
        let mut st = self
            .conn
            .prepare("SELECT w.url, c.outcome, c.detail, c.at FROM checks c JOIN watches w ON w.id = c.watch WHERE c.at >= ?1 AND c.outcome IN ('failed', 'unclear', 'gone') ORDER BY c.at DESC")
            .map_err(e)?;
        let rows = st.query_map([since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).map_err(e)?;
        rows.collect::<Result<_, _>>().map_err(e)
    }

    /// The Stripe checkout an account went to last, until it is settled; empty when there is none.
    pub fn checkout_session(&self, id: i64) -> String {
        self.conn.query_row("SELECT checkout_session FROM accounts WHERE id = ?1", [id], |r| r.get(0)).optional().ok().flatten().unwrap_or_default()
    }

    pub fn set_checkout_session(&self, id: i64, session: &str) -> Result<(), String> {
        self.conn.execute("UPDATE accounts SET checkout_session = ?2 WHERE id = ?1", params![id, session]).map(|_| ()).map_err(e)
    }

    pub fn set_trial_used(&self, id: i64, used: bool) -> Result<(), String> {
        self.conn.execute("UPDATE accounts SET trial_used = ?2 WHERE id = ?1", params![id, used]).map(|_| ()).map_err(e)
    }

    pub fn set_quota(&self, id: i64, quota: i64) -> Result<(), String> {
        self.conn.execute("UPDATE accounts SET quota = ?2 WHERE id = ?1", params![id, quota]).map(|_| ()).map_err(e)
    }

    /// An account given its subscription, or no longer: `paid` goes with it, unless Stripe has a
    /// subscription of the account's own.
    pub fn set_free(&self, id: i64, free: bool) -> Result<(), String> {
        self.conn
            .execute("UPDATE accounts SET free = ?2, paid = CASE WHEN ?2 THEN 1 WHEN stripe_subscription != '' THEN paid ELSE 0 END WHERE id = ?1", params![id, free])
            .map(|_| ())
            .map_err(e)
    }

    pub fn set_blocked(&self, id: i64, blocked: bool) -> Result<(), String> {
        self.conn.execute("UPDATE accounts SET status = ?2 WHERE id = ?1", params![id, if blocked { "blocked" } else { "active" }]).map(|_| ()).map_err(e)
    }

    /// Every session and API key of an account ended. Returns how many there were.
    pub fn end_access(&self, id: i64) -> Result<usize, String> {
        let s = self.conn.execute("DELETE FROM sessions WHERE account = ?1", [id]).map_err(e)?;
        let k = self.conn.execute("DELETE FROM api_keys WHERE account = ?1", [id]).map_err(e)?;
        Ok(s + k)
    }

    /// Until when the account's reviewer link works, if it has one (`/review/<token>`).
    pub fn review_until(&self, id: i64) -> Result<Option<i64>, String> {
        self.conn.query_row("SELECT MAX(expires) FROM tokens WHERE account = ?1 AND kind = 'review' AND used = 0 AND expires > ?2", params![id, now()], |r| r.get(0)).map_err(e)
    }

    /// The account's reviewer links ended, and the sessions they opened with them.
    pub fn end_review(&self, id: i64) -> Result<usize, String> {
        let n = self.conn.execute("DELETE FROM tokens WHERE account = ?1 AND kind = 'review'", [id]).map_err(e)?;
        self.conn.execute("DELETE FROM sessions WHERE account = ?1", [id]).map_err(e)?;
        Ok(n)
    }

    pub fn sessions_of(&self, id: i64) -> Result<i64, String> {
        self.count("SELECT COUNT(*) FROM sessions WHERE account = ?1 AND expires > ?2", params![id, now()])
    }

    /// An account gone with everything it holds: watches with their checks and changes, links,
    /// sessions, keys.
    pub fn delete_account(&self, id: i64) -> Result<(), String> {
        self.conn.execute("DELETE FROM watches WHERE account = ?1", [id]).map_err(e)?;
        self.conn.execute("DELETE FROM accounts WHERE id = ?1", [id]).map(|_| ()).map_err(e)
    }

    pub fn log_admin(&self, admin: &str, action: &str, target: &str) -> Result<(), String> {
        self.conn.execute("INSERT INTO admin_log (at, admin, action, target) VALUES (?1, ?2, ?3, ?4)", params![now(), admin, action, target]).map(|_| ()).map_err(e)
    }

    /// The admin log, newest first: (at, admin, action, target).
    pub fn admin_log(&self, limit: i64) -> Result<Vec<(i64, String, String, String)>, String> {
        let mut st = self.conn.prepare("SELECT at, admin, action, target FROM admin_log ORDER BY id DESC LIMIT ?1").map_err(e)?;
        let rows = st.query_map([limit], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).map_err(e)?;
        rows.collect::<Result<_, _>>().map_err(e)
    }

    /// Stripe's events as they came, newest first: (id, kind, at).
    pub fn events(&self, limit: i64) -> Result<Vec<(String, String, i64)>, String> {
        let mut st = self.conn.prepare("SELECT id, kind, at FROM events ORDER BY at DESC LIMIT ?1").map_err(e)?;
        let rows = st.query_map([limit], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(e)?;
        rows.collect::<Result<_, _>>().map_err(e)
    }

    pub fn meta(&self, key: &str) -> String {
        self.conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional().ok().flatten().unwrap_or_default()
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), String> {
        self.conn.execute("INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [key, value]).map(|_| ()).map_err(e)
    }

    fn accounts<P: rusqlite::Params>(&self, sql: &str, p: P) -> Result<Vec<Account>, String> {
        let mut st = self.conn.prepare(sql).map_err(e)?;
        let rows = st
            .query_map(p, |r| {
                Ok(Account {
                    id: r.get(0)?,
                    email: r.get(1)?,
                    created: r.get(2)?,
                    trial_used: r.get(3)?,
                    quota: r.get(4)?,
                    confirmed: r.get(5)?,
                    paid: r.get(6)?,
                    stripe_customer: r.get(7)?,
                    stripe_subscription: r.get(8)?,
                    free: r.get(9)?,
                    blocked: r.get(10)?,
                })
            })
            .map_err(e)?;
        rows.collect::<Result<_, _>>().map_err(e)
    }

    fn select<P: rusqlite::Params>(&self, sql: &str, p: P) -> Result<Vec<Watch>, String> {
        let mut st = self.conn.prepare(sql).map_err(e)?;
        let rows = st
            .query_map(p, |r| {
                let kind: String = r.get(3)?;
                let anchor: Option<String> = r.get(4)?;
                let volatile: String = r.get(5)?;
                let mode = if kind == "place" {
                    Mode::Place(anchor.and_then(|a| serde_json::from_str(&a).ok()).unwrap_or_default())
                } else {
                    Mode::Page { volatile: serde_json::from_str(&volatile).unwrap_or_default() }
                };
                Ok(Watch {
                    id: r.get(0)?,
                    account: r.get(1)?,
                    url: r.get(2)?,
                    mode,
                    interval_min: r.get(6)?,
                    created: r.get(7)?,
                    until: r.get(8)?,
                    last_checked: r.get(9)?,
                    last_value: r.get(10)?,
                    status: r.get(11)?,
                    state: r.get(12)?,
                    reminded: r.get(13)?,
                    told: r.get(14)?,
                    render: r.get(15)?,
                    key: r.get(16)?,
                    variants: serde_json::from_str(&r.get::<_, String>(17)?).unwrap_or_default(),
                    condition: serde_json::from_str(&r.get::<_, String>(18)?).ok(),
                })
            })
            .map_err(e)?;
        rows.collect::<Result<_, _>>().map_err(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_account_is_made_once_per_address() {
        let s = Store::in_memory();
        let a = s.account("A@example.com ").unwrap();
        assert_eq!(s.account("a@example.com").unwrap(), a);
        assert_ne!(s.local_account().unwrap(), a);
        assert_eq!(s.instance(), "local");
        s.name_instance("n2").unwrap();
        assert_eq!(s.instance(), "n2");
        assert!(s.take_trial(a).unwrap());
        assert!(!s.take_trial(a).unwrap(), "one trial");
    }

    #[test]
    fn a_watch_is_due_after_its_interval_and_a_change_moves_its_value() {
        let s = Store::in_memory();
        let acc = s.local_account().unwrap();
        let id = s.add_watch(acc, "https://example.com/", &Mode::Page { volatile: vec!["#:#".into()] }, 60, None, "a").unwrap();
        let w = s.watch(id).unwrap().unwrap();
        assert_eq!(w.mode, Mode::Page { volatile: vec!["#:#".into()] });
        let created = w.created;
        assert!(s.due(created + 59 * 60).unwrap().is_empty());
        assert_eq!(s.due(created + 60 * 60).unwrap().len(), 1);
        s.record(id, created + 3600, "changed", false, "b", Some(("a", "b"))).unwrap();
        let w = s.watch(id).unwrap().unwrap();
        assert_eq!((w.last_value.as_deref(), w.status.as_str()), (Some("b"), "changed"));
        assert_eq!(s.changes(id, 10).unwrap().len(), 1);
        assert_eq!(s.tally().unwrap(), vec![("changed".to_string(), 1, 0)]);
    }

    #[test]
    fn only_running_watches_are_checked_and_a_trial_ends() {
        let s = Store::in_memory();
        let acc = s.account("a@example.com").unwrap();
        let id = s.add_watch_as(acc, "https://example.com/", &Mode::Page { volatile: vec![] }, 60, None, "a", "pending").unwrap();
        let later = now() + 7200;
        assert!(s.due(later).unwrap().is_empty(), "pending is not checked");
        s.set_state(id, "running", Some(now() + 86400)).unwrap();
        assert_eq!(s.due(later).unwrap().len(), 1);
        assert_eq!(s.ending(now() + 86400 - 7000, 7200).unwrap().len(), 1);
        s.set_reminded(id).unwrap();
        assert!(s.ending(now() + 86400 - 7000, 7200).unwrap().is_empty(), "told once");
        assert_eq!(s.expired(now() + 86400).unwrap().len(), 1);
    }

    #[test]
    fn what_was_never_confirmed_or_paid_for_is_cleaned_away() {
        let s = Store::in_memory();
        let page = Mode::Page { volatile: vec![] };
        let a = s.account("a@example.com").unwrap();
        s.add_watch_as(a, "https://example.com/", &page, 60, None, "x", "pending").unwrap();
        let b = s.account("b@example.com").unwrap();
        let ended = s.add_watch_as(b, "https://example.com/", &page, 60, None, "x", "pending").unwrap();
        s.set_state(ended, "ended", Some(now())).unwrap();
        let paying = s.account("c@example.com").unwrap();
        s.set_paid(paying, true, "cus", "sub").unwrap();
        s.local_account().unwrap();
        assert_eq!((s.clean_pending(now()).unwrap(), s.clean_unpaid(now()).unwrap()), (0, (0, 0)), "nothing old yet");
        let later = now() + 31 * 86400;
        assert_eq!(s.clean_pending(later).unwrap(), 1, "a's unconfirmed watch");
        assert_eq!(s.clean_unpaid(later).unwrap(), (0, 2), "then a and b");
        assert!(s.account_by_id(paying).unwrap().is_some() && s.account_by_id(b).unwrap().is_none());
    }

    #[test]
    fn an_api_key_opens_its_own_account_only() {
        let s = Store::in_memory();
        let a = s.account("a@example.com").unwrap();
        let b = s.account("b@example.com").unwrap();
        let key = s.new_api_key(a, "Claude").unwrap();
        assert!(key.starts_with("zk_") && key.len() == 67);
        assert_eq!(s.api_account(&key).unwrap(), Some(a));
        assert_eq!(s.api_account("zk_nothing").unwrap(), None);
        let keys = s.api_keys_of(a).unwrap();
        assert_eq!((keys[0].1.as_str(), keys[0].2.as_str()), ("Claude", &key[63..]));
        assert!(keys[0].4.is_some(), "last used noted");
        s.delete_api_key(b, keys[0].0).unwrap();
        assert_eq!(s.api_account(&key).unwrap(), Some(a), "another account cannot delete it");
        s.delete_api_key(a, keys[0].0).unwrap();
        assert_eq!(s.api_account(&key).unwrap(), None);
    }

    #[test]
    fn a_link_works_once() {
        let s = Store::in_memory();
        let acc = s.account("a@example.com").unwrap();
        let t = s.new_token(acc, "signin", None, 1800).unwrap();
        assert_eq!(s.take_token(&t, "confirm").unwrap(), None, "not for another purpose");
        assert_eq!(s.take_token(&t, "signin").unwrap(), Some((acc, None)));
        assert_eq!(s.take_token(&t, "signin").unwrap(), None, "used");
        let session = s.new_session(acc).unwrap();
        assert_eq!(s.session(&session).unwrap(), Some(acc));
        s.end_session(&session).unwrap();
        assert_eq!(s.session(&session).unwrap(), None);
    }

    #[test]
    fn an_admin_blocks_frees_and_deletes_an_account() {
        let s = Store::in_memory();
        let acc = s.account("a@example.com").unwrap();
        let session = s.new_session(acc).unwrap();
        let key = s.new_api_key(acc, "k").unwrap();
        let w = s.add_watch(acc, "https://example.com/", &Mode::Page { volatile: vec![] }, 60, None, "x").unwrap();
        s.record(w, now(), "same", false, "x", None).unwrap();
        s.set_blocked(acc, true).unwrap();
        assert_eq!(s.session(&session).unwrap(), None, "a blocked account is signed out");
        assert_eq!(s.api_account(&key).unwrap(), None, "and its keys open nothing");
        s.set_blocked(acc, false).unwrap();
        assert_eq!(s.session(&session).unwrap(), Some(acc));
        s.set_free(acc, true).unwrap();
        let a = s.account_by_id(acc).unwrap().unwrap();
        assert!(a.free && a.paid);
        s.set_free(acc, false).unwrap();
        assert!(!s.account_by_id(acc).unwrap().unwrap().paid, "no subscription of its own: no longer paid");
        s.log_admin("admin@example.com", "delete", "a@example.com").unwrap();
        s.delete_account(acc).unwrap();
        assert!(s.account_by_id(acc).unwrap().is_none() && s.watch(w).unwrap().is_none());
        assert_eq!(s.count("SELECT COUNT(*) FROM checks", []).unwrap(), 0);
        assert_eq!(s.session(&session).unwrap(), None);
        assert_eq!(s.admin_log(10).unwrap().len(), 1);
    }
}
