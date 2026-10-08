//! What a cell used this month, counted where it happens: a source asked (a read) and a mail
//! sent. Each is one byte appended to `<cell>/usage/<kind>-<YYYY-MM>`, so the count is the file's
//! length, two processes counting at once cannot lose one, and the server reads it without
//! opening anything of the cell's. Outside a cell (`ZETLYN_USAGE` unset) nothing is counted.

use std::io::Write;
use std::path::{Path, PathBuf};

pub const READS: &str = "reads";
pub const MAILS: &str = "mails";

/// `2026-10`.
pub fn month() -> String {
    crate::iso_date(crate::now()).get(..7).unwrap_or("").to_string()
}

fn dir() -> Option<PathBuf> {
    std::env::var_os("ZETLYN_USAGE").map(PathBuf::from).filter(|p| !p.as_os_str().is_empty())
}

/// Whether this process is a cell's: a cell sends its mail through its server's relay, runs no
/// commands and reads none of the server's own variables.
pub fn in_cell() -> bool {
    dir().is_some()
}

/// The cell's own directory, where its terms and its counts are; None outside a cell.
pub fn cell_dir() -> Option<PathBuf> {
    dir()?.parent().map(Path::to_path_buf)
}

fn append(file: &Path) {
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
        let _ = f.write_all(b".");
    }
}

/// One more of a kind, this month.
pub fn count(kind: &str) {
    let Some(d) = dir() else { return };
    let _ = std::fs::create_dir_all(&d);
    append(&d.join(format!("{kind}-{}", month())));
}

/// One more read, this month, and one more for that source, so the owner sees which source the
/// reads went to.
pub fn count_read(source: &str) {
    count(READS);
    let Some(d) = dir() else { return };
    let per = d.join(format!("{READS}-{}.d", month()));
    let _ = std::fs::create_dir_all(&per);
    append(&per.join(file_of(source)));
}

/// One more of a kind for a cell, counted from outside it: its server's mail relay. What it makes
/// stays the cell's, so the cell still writes there.
pub fn count_in(cell: &Path, kind: &str) {
    use std::os::unix::fs::MetadataExt;
    let d = cell.join("usage");
    let owner = std::fs::metadata(cell).ok().map(|m| (m.uid(), m.gid()));
    let made = !d.exists();
    let _ = std::fs::create_dir_all(&d);
    let file = d.join(format!("{kind}-{}", month()));
    let new = !file.exists();
    append(&file);
    if let Some((uid, gid)) = owner {
        for (p, fresh) in [(&d, made), (&file, new)] {
            if fresh {
                let _ = std::os::unix::fs::chown(p, Some(uid), Some(gid));
            }
        }
    }
}

/// A source's name as a file's: `models/gguf` as `models%2Fgguf`.
fn file_of(source: &str) -> String {
    source.replace('%', "%25").replace('/', "%2F")
}

/// How many of a kind a cell's directory counted in a month.
pub fn used(cell: &Path, kind: &str, month: &str) -> u64 {
    std::fs::metadata(cell.join("usage").join(format!("{kind}-{month}"))).map(|m| m.len()).unwrap_or(0)
}

/// A month's reads, source by source, the most first.
pub fn reads_by_source(cell: &Path, month: &str) -> Vec<(String, u64)> {
    let mut out: Vec<(String, u64)> = std::fs::read_dir(cell.join("usage").join(format!("{READS}-{month}.d")))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((e.file_name().to_string_lossy().replace("%2F", "/").replace("%25", "%"), e.metadata().ok()?.len())))
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

/// Whether one more is allowed this month: the cell's terms may cap a kind (`reads:`, `mails:`
/// in cell.yaml), set by the main server where the month's spending limit would be passed.
pub fn allowed(kind: &str) -> bool {
    let Some(d) = dir() else { return true };
    let Some(cell) = d.parent() else { return true };
    let Some(t) = crate::cell::terms(cell) else { return true };
    let cap = match kind {
        READS => t.reads,
        MAILS => t.mails,
        _ => None,
    };
    cap.is_none_or(|c| used(cell, kind, &month()) < c)
}

/// The month before this one: `2026-09` in October.
pub fn previous_month() -> String {
    let m = month();
    let (y, mo): (i32, u32) = (m.get(..4).and_then(|s| s.parse().ok()).unwrap_or(2026), m.get(5..7).and_then(|s| s.parse().ok()).unwrap_or(1));
    if mo == 1 { format!("{:04}-12", y - 1) } else { format!("{y:04}-{:02}", mo - 1) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cell_counts_and_stops_at_its_cap() {
        let cell = std::env::temp_dir().join(format!("zetlyn-usage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cell);
        std::fs::create_dir_all(&cell).unwrap();
        std::fs::write(cell.join(crate::cell::TERMS), "active: true\nmails: 2\n").unwrap();
        std::env::set_var("ZETLYN_USAGE", cell.join("usage"));
        assert!(allowed(MAILS));
        count(MAILS);
        count(MAILS);
        assert_eq!(used(&cell, MAILS, &month()), 2);
        assert!(!allowed(MAILS), "two of two sent");
        assert!(allowed(READS), "reads have no cap here");
        count_read("models/gguf");
        count_read("models/gguf");
        count_read("cve");
        assert_eq!(used(&cell, READS, &month()), 3);
        assert_eq!(reads_by_source(&cell, &month()), vec![("models/gguf".to_string(), 2), ("cve".to_string(), 1)]);
        count_in(&cell, MAILS);
        assert_eq!(used(&cell, MAILS, &month()), 3);
        std::env::remove_var("ZETLYN_USAGE");
        let _ = std::fs::remove_dir_all(&cell);
    }
}
