//! Updates in the background, for a workspace that asks for them: `update: { every: 1h }` in
//! workspace.yaml. Absent, it is off, and a person updates with Update now.
//!
//! What runs by itself is only what cannot surprise anybody: a source that has been read in full
//! before, read again for what is new. A first read, a trial of one page, a read somebody stopped
//! and a read of a list further back are all a person's to start. A source that fails three times
//! running is paused with the reason, rather than asked again and again.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::source::Source;

/// The workspace updates nothing more often than this: a quarter of an hour is often for a file.
pub const FLOOR: i64 = 15 * 60;

/// The rhythms offered, for every source and for one: one list, so both say the same.
pub const INTERVALS: [(&str, &str); 7] = [("5m", "Every 5 minutes"), ("15m", "Every 15 minutes"), ("30m", "Every 30 minutes"), ("1h", "Every hour"), ("6h", "Every 6 hours"), ("12h", "Every 12 hours"), ("1d", "Once a day")];

/// The shortest rhythm here: what the plan allows in a hosted cell (`every:` in cell.yaml, five
/// minutes where it says none), a quarter of an hour on one's own machine.
pub fn floor() -> i64 {
    match crate::usage::cell_dir() {
        Some(cell) => crate::cell::terms(&cell).and_then(|t| crate::fetch::duration(&t.every)).unwrap_or(5 * 60),
        None => FLOOR,
    }
}
/// Failures in a row before a source waits for a person.
pub const PATIENCE: i64 = 3;

/// An update in the background is running in this process now.
pub static RUNNING: AtomicBool = AtomicBool::new(false);

pub fn running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

/// `update:` in workspace.yaml.
#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// `1h`, `6h`, `1d`. Absent, nothing updates by itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<String>,
    /// The offer to turn it on has been made once, and answered.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub offered: bool,
}

/// How often the workspace updates its sources, in seconds; `None` when it is off.
pub fn every(root: &Path) -> Option<i64> {
    let site = crate::account::Site::load(root);
    site.update.every.as_deref().and_then(crate::fetch::duration).map(|s| s.max(floor()))
}

pub fn offered(root: &Path) -> bool {
    crate::account::Site::load(root).update.offered
}

/// Every hour, every 6 hours, once a day: as the choice was offered.
pub fn words(seconds: i64) -> String {
    match seconds {
        s if s % 86_400 == 0 && s / 86_400 == 1 => "once a day".into(),
        s if s % 86_400 == 0 => format!("every {} days", s / 86_400),
        3600 => "every hour".into(),
        s if s % 3600 == 0 => format!("every {} hours", s / 3600),
        s => format!("every {} minutes", s / 60),
    }
}

/// The workspace's `update:` block, written as text so the rest of the file stays as it was.
pub fn set(root: &Path, every: Option<&str>, offered: bool) -> Result<(), String> {
    let path = root.join(crate::account::WORKSPACE);
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut kept: Vec<&str> = Vec::new();
    let mut skipping = false;
    for l in text.lines() {
        if l.starts_with("update:") {
            skipping = true;
            continue;
        }
        if skipping && (l.starts_with(' ') || l.is_empty()) {
            continue;
        }
        skipping = false;
        kept.push(l);
    }
    let mut block = String::new();
    if every.is_some() || offered {
        block.push_str("update:\n");
        if let Some(e) = every {
            block.push_str(&format!("  every: {e}\n"));
        }
        if offered {
            block.push_str("  offered: true\n");
        }
    }
    let t = format!("{}\n{block}", kept.join("\n").trim_end());
    let _: crate::account::Site = crate::yaml::parse(&t)?;
    std::fs::write(&path, format!("{}\n", t.trim())).map_err(|e| format!("{}: {e}", path.display()))
}

/// How often this source is updated: its own `schedule.every`, `never`, or the workspace's.
pub fn source_every(ds: &Source, workspace: Option<i64>) -> Option<i64> {
    // Claims that arrive built are not fetched here, so there is no rhythm to keep.
    if matches!(ds.decl.source, crate::sourcedecl::Fetch::Hub { .. } | crate::sourcedecl::Fetch::Package { .. }) {
        return None;
    }
    match ds.decl.schedule.every.as_deref() {
        Some("never") => None,
        // A rhythm written into the declaration by hand is the author's to answer for.
        Some(own) => crate::fetch::duration(own),
        None => workspace,
    }
}

/// Why this source is not updated by itself, in words; `None` when it is.
pub fn held(ds: &Source, dir: &Path) -> Option<String> {
    use crate::sourcedecl::Fetch;
    match &ds.decl.source {
        Fetch::Hub { .. } => return Some("subscribed from a hub: `zetlyn source pull` takes what is new".into()),
        Fetch::Package { tracker, .. } => return Some(format!("came in the package {tracker}: `zetlyn tracker pull` takes what is new")),
        Fetch::Webhook { .. } => return Some("pushed to, so there is nothing to fetch".into()),
        Fetch::Proposals { .. } => return Some("proposed to, and taken as its owner accepts: `zetlyn source update` reads what was accepted".into()),
        _ => {}
    }
    if ds.decl.source.truncating() {
        return Some("still a trial: choose how much of it to read first".into());
    }
    if let Some(r) = crate::web::resume_of(dir) {
        return Some(format!("a read of it is not finished ({}): go on with it first", r.why));
    }
    if !ever_complete(ds) {
        return Some("never read in full yet".into());
    }
    if failures(ds) >= PATIENCE {
        let why = ds.store.meta("auto_error").unwrap_or_default();
        return Some(format!("paused after {PATIENCE} failed tries: {why}"));
    }
    None
}

fn ever_complete(ds: &Source) -> bool {
    ds.store.db.query_row("select 1 from run where complete = 1 limit 1", [], |_| Ok(())).is_ok()
}

pub fn failures(ds: &Source) -> i64 {
    ds.store.meta("auto_failures").and_then(|n| n.parse().ok()).unwrap_or(0)
}

/// What an update came to, kept for the next pass: a success clears the count, a failure adds
/// to it and keeps the reason.
pub fn record(ds: &Source, outcome: &Result<crate::store::RunReport, String>) {
    let failed = match outcome {
        Ok(r) => r.error.clone(),
        Err(e) => Some(e.clone()),
    };
    match failed {
        // Read by another process meanwhile: that one's to count, not this one.
        Some(e) if e.contains(crate::source::BUSY) => {}
        None => {
            let _ = ds.store.set_meta("auto_failures", "0");
        }
        Some(e) => {
            let _ = ds.store.set_meta("auto_failures", &(failures(ds) + 1).to_string());
            let _ = ds.store.set_meta("auto_error", &e);
        }
    }
}

/// A person says try again: the count starts over.
pub fn forgive(dir: &Path) {
    if let Ok(ds) = Source::open(dir) {
        let _ = ds.store.set_meta("auto_failures", "0");
    }
}

pub(crate) fn last_finished(ds: &Source) -> Option<i64> {
    ds.store.run_report(ds.store.last_run()).and_then(|r| r.finished).map(|f| crate::fetch::seconds_of(&f))
}

/// The sources due now, soonest first, with their titles. A laptop that slept through six
/// hours finds each source due once, not six times.
pub fn due(root: &Path, now: i64) -> Vec<(String, String, PathBuf)> {
    let workspace = every(root);
    let mut out: Vec<(i64, String, String, PathBuf)> = Vec::new();
    for (name, dir) in crate::tracker::registry(&root.join("sources")) {
        let Ok(ds) = Source::open(&dir) else { continue };
        let Some(every) = source_every(&ds, workspace) else { continue };
        if held(&ds, &dir).is_some() {
            continue;
        }
        let at = last_finished(&ds).map(|f| f + every).unwrap_or(0);
        if at <= now {
            out.push((at, name, ds.decl.title.clone(), dir));
        }
    }
    out.sort();
    out.into_iter().map(|(_, n, t, d)| (n, t, d)).collect()
}

/// When this source is next updated by itself, if it is.
pub fn next_at(ds: &Source, workspace: Option<i64>) -> Option<i64> {
    let every = source_every(ds, workspace)?;
    Some(last_finished(ds).map(|f| f + every).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_rhythm_is_said_as_it_was_offered() {
        assert_eq!(super::words(3600), "every hour");
        assert_eq!(super::words(6 * 3600), "every 6 hours");
        assert_eq!(super::words(86_400), "once a day");
        assert_eq!(super::words(15 * 60), "every 15 minutes");
    }

    #[test]
    fn the_setting_is_written_beside_what_the_file_already_says() {
        let root = std::env::temp_dir().join(format!("zetlyn-autoupdate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("workspace.yaml"), "title: Mine\nupdate:\n  every: 6h\n").unwrap();
        super::set(&root, Some("1h"), true).unwrap();
        let text = std::fs::read_to_string(root.join("workspace.yaml")).unwrap();
        assert_eq!(text, "title: Mine\nupdate:\n  every: 1h\n  offered: true\n");
        assert_eq!(super::every(&root), Some(3600));
        super::set(&root, None, true).unwrap();
        assert_eq!(super::every(&root), None);
        assert!(super::offered(&root));
        let _ = std::fs::remove_dir_all(&root);
    }
}
