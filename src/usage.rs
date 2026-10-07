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

/// One more of a kind, this month.
pub fn count(kind: &str) {
    let Some(d) = dir() else { return };
    let _ = std::fs::create_dir_all(&d);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(d.join(format!("{kind}-{}", month()))) {
        let _ = f.write_all(b".");
    }
}

/// How many of a kind a cell's directory counted in a month.
pub fn used(cell: &Path, kind: &str, month: &str) -> u64 {
    std::fs::metadata(cell.join("usage").join(format!("{kind}-{month}"))).map(|m| m.len()).unwrap_or(0)
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
        std::env::remove_var("ZETLYN_USAGE");
        let _ = std::fs::remove_dir_all(&cell);
    }
}
