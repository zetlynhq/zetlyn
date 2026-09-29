//! A match a person made: this thing is `relation` that, said by somebody, on a day, for a reason.
//!
//! Where no claim states both identifiers, a relation is still sometimes plain to a person: the
//! CNA wrote `Microsoft · Windows Server 2025` and NVD has not analysed it yet. Zetlyn never makes
//! that match itself (D9). A person does, and the match is kept as their claim, with its receipt,
//! beside the tracker: an append-only file, one line per confirmation or withdrawal, so every match
//! there ever was is on the record and none of them appeared silently.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub const FILE: &str = "matches.jsonl";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Match {
    pub at: String,
    /// Who confirmed it, as they gave their name.
    pub by: String,
    /// The thing, as the tracker keys it: `cve:cve-2026-12345`.
    pub key: String,
    pub relation: String,
    pub target: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub why: String,
    /// A later line that takes an earlier one back. The earlier one stays in the file.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub withdrawn: bool,
}

/// Every match that stands: the last line for each thing, relation and target, where it was not
/// a withdrawal.
pub fn standing(dir: &Path) -> Vec<Match> {
    let Ok(text) = std::fs::read_to_string(dir.join(FILE)) else {
        return Vec::new();
    };
    let mut last: BTreeMap<(String, String, String), Match> = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if let Ok(m) = serde_json::from_str::<Match>(line) {
            last.insert((m.key.clone(), m.relation.clone(), m.target.clone()), m);
        }
    }
    last.into_values().filter(|m| !m.withdrawn).collect()
}

/// Written down: a confirmation, or its withdrawal.
pub fn record(dir: &Path, m: &Match) -> Result<(), String> {
    use std::io::Write;
    if m.by.trim().is_empty() {
        return Err("a match is somebody's: say who with --by".into());
    }
    let line = serde_json::to_string(m).map_err(|e| e.to_string())?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(FILE))
        .map_err(|e| format!("{}: {e}", dir.join(FILE).display()))?;
    writeln!(f, "{line}").map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_withdrawal_takes_a_match_back_and_both_stay_on_record() {
        let dir = std::env::temp_dir().join(format!("zetlyn-matches-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = std::fs::remove_file(dir.join(FILE));
        let m = Match { at: "2026-09-29T00:00:00Z".into(), by: "someone".into(), key: "cve:cve-1".into(),
                        relation: "affects".into(), target: "linux/linux".into(), why: "the CNA says so".into(), withdrawn: false };
        record(&dir, &m).unwrap();
        assert_eq!(standing(&dir), [m.clone()]);
        record(&dir, &Match { withdrawn: true, ..m.clone() }).unwrap();
        assert!(standing(&dir).is_empty());
        assert_eq!(std::fs::read_to_string(dir.join(FILE)).unwrap().lines().count(), 2);
        assert!(record(&dir, &Match { by: " ".into(), ..m }).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
