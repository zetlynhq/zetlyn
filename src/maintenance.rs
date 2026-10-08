//! Maintenance, announced and in progress (CELLS.md): one notice the operator writes on the admin
//! pages, `ops/maintenance.json` on the main server, given by `zetlyn ops poll` to every cell it
//! applies to as `maintenance.json` in the cell's directory. Before `from` it is announced, a
//! banner; from `from` to `until` it is in force, a banner and its mode:
//!
//! - `notice`: only the banner.
//! - `nologin`: no new sign-in and no order; whoever is signed in stays.
//! - `readonly`: pages open, nothing changed and no source read.
//! - `closed`: a maintenance page for everybody but the operator.

use std::path::{Path, PathBuf};

use maud::{html, Markup};
use serde::{Deserialize, Serialize};

pub const FILE: &str = "maintenance.json";

#[derive(Debug, Default, Clone, PartialEq, Deserialize, Serialize)]
pub struct Notice {
    pub text: String,
    /// `info` or `warning`.
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub mode: String,
    /// `all`, `node:<name>` or `cell:<name>`; empty is all.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target: String,
    /// `2026-10-08T22:00:00Z`, each.
    pub announce_from: String,
    pub from: String,
    pub until: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub by: String,
}

impl Notice {
    /// `announced`, `active`, or nothing at `now`.
    pub fn state(&self, now: &str) -> Option<&'static str> {
        if now < self.announce_from.as_str() || now >= self.until.as_str() {
            None
        } else if now >= self.from.as_str() {
            Some("active")
        } else {
            Some("announced")
        }
    }
    pub fn applies(&self, node: &str, cell: &str) -> bool {
        self.target.is_empty() || self.target == "all" || self.target == format!("node:{node}") || self.target == format!("cell:{cell}")
    }
}

static PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Where this process reads its notice: the main server's own file. A cell's is in its directory.
pub fn watch(path: PathBuf) {
    let _ = PATH.set(path);
}

pub fn read(path: &Path) -> Option<Notice> {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

/// The notice here now, with its state, read again every fifteen seconds. On the main server only
/// one for everything counts; a cell's is there only where it applies.
pub fn now() -> Option<(Notice, &'static str)> {
    type Kept = (i64, Option<Notice>);
    static CACHE: std::sync::Mutex<Kept> = std::sync::Mutex::new((0, None));
    let t = crate::now();
    let notice = {
        let mut kept = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if t - kept.0 >= 15 {
            let main = PATH.get().cloned();
            let path = main.clone().or_else(|| crate::usage::cell_dir().map(|c| c.join(FILE)));
            kept.1 = path.and_then(|p| read(&p)).filter(|n| main.is_none() || n.target.is_empty() || n.target == "all");
            kept.0 = t;
        }
        kept.1.clone()
    }?;
    let state = notice.state(&crate::iso_stamp(t))?;
    Some((notice, state))
}

/// The mode in force now, where maintenance is.
pub fn mode() -> Option<String> {
    now().filter(|(_, s)| *s == "active").map(|(n, _)| n.mode)
}

/// `2026-10-08T22:00:00Z` as `8 Oct, 22:00 UTC`.
pub fn when(stamp: &str) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let m: usize = stamp.get(5..7).and_then(|m| m.parse().ok()).unwrap_or(1);
    let d: u32 = stamp.get(8..10).and_then(|d| d.parse().ok()).unwrap_or(1);
    format!("{d} {}, {} UTC", MONTHS.get(m.wrapping_sub(1)).unwrap_or(&""), stamp.get(11..16).unwrap_or(""))
}

/// The words of the banner.
pub fn words(n: &Notice, state: &str) -> String {
    let span = format!("{} to {}", when(&n.from), when(&n.until));
    let what = if n.text.trim().is_empty() { String::new() } else { format!(" {}", n.text.trim()) };
    if state == "active" {
        format!("Maintenance until {}.{what}", when(&n.until))
    } else {
        format!("Planned maintenance, {span}.{what}")
    }
}

/// The banner every page carries while a notice stands.
pub fn banner() -> Markup {
    match now() {
        Some((n, state)) => html! {
            div.maintenance-banner.(if n.level == "warning" || state == "active" { "warning" } else { "info" }) role="status" { (words(&n, state)) }
        },
        None => html! {},
    }
}

/// The page everybody but the operator gets while it is closed.
pub fn closed_page(n: &Notice) -> String {
    let body = html! {
        (maud::DOCTYPE)
        html lang="en" {
            head { meta charset="utf-8"; meta name="viewport" content="width=device-width, initial-scale=1"; title { "Maintenance · Zetlyn" }
                style { "body{font:16px/1.5 system-ui,sans-serif;max-width:36rem;margin:15vh auto;padding:0 1rem;color:#111;background:#fff}@media(prefers-color-scheme:dark){body{color:#eee;background:#111}}h1{font-size:1.6rem}" } }
            body {
                h1 { "Back soon" }
                p { (words(n, "active")) }
                p { "Questions: " a href="mailto:hello@zetlyn.com" { "hello@zetlyn.com" } }
            }
        }
    };
    body.into_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_is_announced_then_in_force_then_gone() {
        let n = Notice { announce_from: "2026-10-08T10:00:00Z".into(), from: "2026-10-08T22:00:00Z".into(), until: "2026-10-08T23:00:00Z".into(), ..Notice::default() };
        assert_eq!(n.state("2026-10-08T09:59:59Z"), None);
        assert_eq!(n.state("2026-10-08T12:00:00Z"), Some("announced"));
        assert_eq!(n.state("2026-10-08T22:30:00Z"), Some("active"));
        assert_eq!(n.state("2026-10-08T23:00:00Z"), None);
        assert!(n.applies("n1", "acme"));
        let only = Notice { target: "node:n2".into(), ..n };
        assert!(!only.applies("n1", "acme") && only.applies("n2", "acme"));
        assert_eq!(when("2026-10-08T22:00:00Z"), "8 Oct, 22:00 UTC");
    }
}
