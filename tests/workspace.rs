//! The program, run over a small workspace that is built to make each thing a scope is for
//! visible once: two publishers agreeing in different words, two disagreeing, one source saying
//! two things, an identifier written in lower case, a word nobody mapped, and a second update that
//! moves three values.
//!
//! The binary is run as a person runs it, against a copy of `tests/fixtures/workspace`. What it
//! prints is held against `tests/golden/`; `ZETLYN_BLESS=1 cargo test` writes those files again,
//! and the diff is then the thing to read.

use std::path::{Path, PathBuf};
use std::process::Command;

const MEMBERS: [&str; 4] = ["kev", "vendor-a", "vendor-b", "exploits"];

/// A copy of the fixture workspace, removed when the test ends.
struct Workspace {
    root: PathBuf,
}

impl Workspace {
    /// Every member run once.
    fn new(name: &str) -> Workspace {
        let root = std::env::temp_dir().join(format!("zetlyn-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        copy(&fixtures().join("workspace"), &root);
        let ws = Workspace { root };
        for m in MEMBERS {
            ws.z(&["source", "update", &ws.dataset(m)]);
        }
        ws
    }

    fn dataset(&self, name: &str) -> String {
        self.root.join("sources").join(name).display().to_string()
    }

    fn scope(&self) -> String {
        self.root.join("trackers/cve").display().to_string()
    }

    /// The second update: vendor A rates CVE-2026-0004 critical and moves two scores.
    fn update(&self) {
        std::fs::copy(
            fixtures().join("update-2/vendor-a.csv"),
            self.root.join("sources/vendor-a/advisories.csv"),
        )
        .unwrap();
        self.z(&["source", "update", &self.dataset("vendor-a")]);
    }

    fn z(&self, args: &[&str]) -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_zetlyn"))
            .args(args)
            .output()
            .expect("the binary runs");
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            out.status.success(),
            "zetlyn {}\n{text}\n{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
        text.replace(&self.root.display().to_string(), "WORKSPACE")
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(e.file_name());
        if e.path().is_dir() {
            copy(&e.path(), &target);
        } else {
            std::fs::copy(e.path(), target).unwrap();
        }
    }
}

/// Held against the file, or written to it when blessing.
fn golden(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(name);
    if std::env::var_os("ZETLYN_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{} is missing. ZETLYN_BLESS=1 writes it", path.display()));
    assert!(
        expected == actual,
        "{name} differs from {}\n--- expected\n{expected}\n--- actual\n{actual}",
        path.display()
    );
}

/// The lines an entry prints, from its heading to the next one.
fn entry<'a>(listing: &'a str, key: &str) -> Vec<&'a str> {
    let mut lines = listing.lines().skip_while(|l| !l.contains(&format!("[{key}]")));
    let mut out: Vec<&str> = lines.next().into_iter().collect();
    out.extend(lines.take_while(|l| l.starts_with("     ")));
    out
}

#[test]
fn entries() {
    let ws = Workspace::new("entries");
    let listing = ws.z(&["tracker", "search", &ws.scope(), "--limit", "20"]);

    // Two words for one judgement: `important` is Red Hat's `high`.
    let foo = entry(&listing, "CVE-2026-0001");
    assert!(foo.iter().any(|l| l.contains("severity: test/vendor-a=important→high")));
    assert!(!foo.iter().any(|l| l.contains("conflict")), "{foo:#?}");

    // Two publishers, two judgements.
    let bar = entry(&listing, "CVE-2026-0002");
    assert!(bar.iter().any(|l| l.contains("severity (conflict)")), "{bar:#?}");
    assert!(bar.iter().any(|l| l.contains("cvss (conflict)")), "{bar:#?}");
    // The lower-case reference joined the entry.
    assert!(bar.iter().any(|l| l.contains("Bar heap overflow PoC (test/exploits)")), "{bar:#?}");

    // One source saying two things is not a disagreement with itself.
    let qux = entry(&listing, "CVE-2026-0003");
    assert!(!qux.iter().any(|l| l.contains("conflict")), "{qux:#?}");

    // The named checks first, so that a failure says which rule broke; then everything else.
    golden("entries.txt", &listing);
}

#[test]
fn measure() {
    let ws = Workspace::new("measure");
    let m = ws.z(&["tracker", "measure", &ws.scope()]);
    let j: serde_json::Value = serde_json::from_str(&m).unwrap();
    // `cve-2026-0002` and `CVE-2026-0002` are one subject, named by three members.
    assert_eq!(j["things"], 5);
    assert_eq!(j["by_sources"]["3"], 1);
    // Both ratings of 0001 and 0002 differ in words; only 0002 differs after the map.
    assert_eq!(j["properties"]["severity"]["differ_in_words"], 2);
    assert_eq!(j["properties"]["severity"]["differ_after_the_map"], 1);
    // Two platforms from one source is not a field two members carry.
    assert!(j["properties"]["platform"].is_null(), "{}", j["properties"]);
    golden("measure.json", &m);
}

#[test]
fn changes() {
    let ws = Workspace::new("changes");
    ws.update();
    let dataset = ws.z(&["changes", &ws.dataset("vendor-a")]);
    let j: serde_json::Value = serde_json::from_str(&dataset).unwrap();
    let moved: Vec<String> = j["changed"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|c| c["properties"].as_array().cloned().unwrap_or_default())
        .map(|f| format!("{} {} → {}", f["property"], f["was"], f["is"]))
        .collect();
    assert_eq!(
        moved,
        [
            r#""cvss" "9.8" → "8.1""#,
            r#""cvss" "6.5" → "9.1""#,
            r#""severity" "moderate" → "critical""#,
        ]
    );

    golden("changes-dataset.json", &dataset);
    let since = "test/kev=1,test/vendor-a=1,test/vendor-b=1,test/exploits=1";
    golden(
        "changes-scope.json",
        &ws.z(&["changes", &ws.scope(), "--since", since]),
    );
}

/// The keys a watch would deliver, from `watch check`'s report.
fn delivered(report: &str, watch: &str) -> Vec<String> {
    let start = report.find(&format!("{watch}: ")).expect("the watch reported");
    let body = &report[start..];
    let json_at = body.find('{').expect("entries to report");
    let mut de = serde_json::Deserializer::from_str(&body[json_at..]).into_iter::<serde_json::Value>();
    let j = de.next().unwrap().unwrap();
    let mut keys: Vec<String> = j["things"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            let key = if e["identifier"].is_object() { &e["identifier"] } else { &e["ids"][0] };
            key["value"].as_str().unwrap_or("").to_string()
        })
        .collect();
    keys.sort();
    keys
}

#[test]
fn watches() {
    let ws = Workspace::new("watches");
    ws.update();
    let report = ws.z(&["watch", "check", &ws.root.display().to_string()]);

    // On the scale, not as strings: `critical` is above `high`, and `urgent` is on no scale.
    assert_eq!(
        delivered(&report, "severe"),
        ["CVE-2026-0001", "CVE-2026-0002", "CVE-2026-0004"]
    );
    // A watch over a dataset reads its query.
    assert_eq!(delivered(&report, "vendor-a-critical"), ["CVE-2026-0004"]);
    golden("watches.txt", &report);
}

#[test]
fn a_shrunken_source_is_refused() {
    let ws = Workspace::new("refused");
    // Half the catalogue gone at once looks exactly like a source that broke, and the store
    // keeps what it had.
    std::fs::write(
        ws.root.join("sources/kev/kev.csv"),
        "cveID,vulnerabilityName,dateAdded,vendorProject\n\
         CVE-2026-0001,Foo Server remote code execution,2026-09-01,Foo\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_zetlyn"))
        .args(["source", "update", &ws.dataset("kev")])
        .output()
        .unwrap();
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(said.contains("the store was not replaced"), "{said}");
    let held = ws.z(&["search", &ws.dataset("kev")]);
    assert!(held.starts_with("2 claims"), "{held}");
}

/// A copy of the workspace as 0.1 wrote it, with its own identity home.
fn before(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("zetlyn-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    copy(&fixtures().join("before-0.2"), &root.join("workspace"));
    std::fs::create_dir_all(root.join("home")).unwrap();
    root
}

fn run(root: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_zetlyn"))
        .args(args)
        .env("ZETLYN_HOME", root.join("home"))
        .output()
        .unwrap();
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), said.replace(&root.display().to_string(), "ROOT"))
}

#[test]
fn migrate_keeps_what_a_workspace_answers() {
    let root = before("migrate");
    let ws = root.join("workspace");
    let (ok, said) = run(&root, &["migrate", &ws.display().to_string()]);
    assert!(ok, "{said}");
    assert!(said.contains("datasets → ROOT/workspace/sources"), "{said}");
    assert!(ws.join("sources/kev/source.yaml").exists());
    assert!(!ws.join("sources/kev/dataset.toml").exists());
    assert!(ws.join("watches/severe.yaml").exists());

    // What it answers afterwards is what the workspace written in 0.2 answers.
    for m in MEMBERS {
        let (ok, said) = run(&root, &["source", "update", &ws.join("sources").join(m).display().to_string()]);
        assert!(ok, "{said}");
    }
    let (ok, measured) = run(&root, &["tracker", "measure", &ws.join("trackers/cve").display().to_string()]);
    assert!(ok, "{measured}");
    let fresh = Workspace::new("migrate-fresh");
    assert_eq!(measured, fresh.z(&["tracker", "measure", &fresh.scope()]));

    // Once is enough, and a second time says so rather than doing something.
    let (ok, said) = run(&root, &["migrate", &ws.display().to_string()]);
    assert!(!ok && said.contains("nothing here is from before 0.2"), "{said}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_file_from_before_is_refused_by_name() {
    let root = before("refused-old");
    let kev = root.join("workspace/datasets/kev");
    let (ok, said) = run(&root, &["source", "update", &kev.display().to_string()]);
    assert!(!ok);
    assert!(said.contains("dataset.toml is from before 0.2. `zetlyn migrate` rewrites it"), "{said}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_half_migration_is_refused() {
    let root = before("half");
    let ws = root.join("workspace");
    std::fs::create_dir_all(ws.join("sources")).unwrap();
    let (ok, said) = run(&root, &["migrate", &ws.display().to_string()]);
    assert!(!ok && said.contains("stopped half way"), "{said}");
    assert!(ws.join("datasets/kev/dataset.toml").exists(), "nothing is touched");
    let _ = std::fs::remove_dir_all(&root);
}

/// A source published to a hub, subscribed to, changed and published again: the subscriber pulls
/// the delta and holds what the publisher holds.
#[test]
fn a_hub_carries_a_source_and_what_changed() {
    let ws = Workspace::new("hub");
    let hub = ws.root.join("hub");
    let hub_s = hub.display().to_string();
    let subscriber = ws.root.join("elsewhere");
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_zetlyn"))
            .args(args)
            .env("ZETLYN_HOME", ws.root.join("home"))
            .output()
            .unwrap();
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.status.success(), "zetlyn {}\n{said}", args.join(" "));
        said
    };
    run(&["source", "publish", &ws.dataset("vendor-a"), "--to", &hub_s]);
    assert!(hub.join("sources/test/vendor-a").is_dir(), "published under sources/");
    run(&[
        "source", "subscribe", "test/vendor-a", "--from", &hub_s,
        "--at", &subscriber.display().to_string(),
    ]);
    let held = subscriber.display().to_string();
    assert!(run(&["search", &held]).starts_with("3 claims"));

    ws.update();
    run(&["source", "publish", &ws.dataset("vendor-a"), "--to", &hub_s]);
    let pulled = run(&["source", "pull", &held]);
    // Two claims changed, and only they travel.
    assert!(pulled.contains("by delta: +0 ~2 −0"), "{pulled}");

    // What the subscriber holds now is what the publisher holds.
    let theirs = ws.z(&["search", &ws.dataset("vendor-a"), "severity=critical"]);
    let ours = run(&["search", &held, "severity=critical"]);
    assert_eq!(theirs.lines().next(), ours.lines().next(), "{ours}");
    assert!(ours.starts_with("2 claims"), "{ours}");
}

/// `zetlyn claim` on a source, as JSON: one claim per object, printed one after another.
fn claims_of(printed: &str) -> Vec<serde_json::Value> {
    serde_json::Deserializer::from_str(printed)
        .into_iter::<serde_json::Value>()
        .map(|v| v.unwrap())
        .collect()
}

/// The raw words a version's receipt holds for one property.
fn raw(version: &serde_json::Value, property: &str) -> Vec<String> {
    version["excerpt"]["properties"][property]["raw"]
        .as_array()
        .map(|a| a.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect())
        .unwrap_or_default()
}

#[test]
fn every_value_has_a_receipt_and_a_history() {
    let ws = Workspace::new("receipts");
    ws.update();
    let c = claims_of(&ws.z(&["claim", &ws.dataset("vendor-a"), "CVE-2026-0001"]));
    assert_eq!(c.len(), 1);
    let claim = &c[0];

    // What the source handed over, as it handed it over: the row, and per property the
    // expression that read it and the words it read.
    assert_eq!(claim["excerpt"]["row"]["cvss"], "8.1");
    assert_eq!(claim["excerpt"]["properties"]["cvss"]["from"], "field:cvss");
    assert_eq!(raw(claim, "severity"), ["important"]);

    // And every version before it, each with its own receipt and the time it was first seen.
    let versions = claim["versions"].as_array().unwrap();
    assert_eq!(versions.len(), 2, "{versions:#?}");
    assert_eq!(raw(&versions[0], "cvss"), ["9.8"]);
    assert_eq!(raw(&versions[1], "cvss"), ["8.1"]);
    assert_eq!(versions[0]["excerpt"]["row"]["cvss"], "9.8");
    assert!(versions[0]["at"].as_str().unwrap() <= versions[1]["at"].as_str().unwrap());
    assert_eq!(claim["source"], "test/vendor-a");
}

#[test]
fn history_is_kept_without_being_asked_for() {
    let ws = Workspace::new("default-history");
    let dir = ws.root.join("sources/proposed");
    let csv = ws.root.join("advisories.csv");
    std::fs::copy(ws.root.join("sources/vendor-a/advisories.csv"), &csv).unwrap();
    let dir_s = dir.display().to_string();
    ws.z(&["source", "new", "--from", &csv.display().to_string(), "--at", &dir_s, "--name", "local/proposed"]);
    let declared = std::fs::read_to_string(dir.join("source.yaml")).unwrap();
    assert!(!declared.contains("retention"), "a default is not written: {declared}");
    ws.z(&["source", "update", &dir_s]);
    std::fs::copy(fixtures().join("update-2/vendor-a.csv"), &csv).unwrap();
    ws.z(&["source", "update", &dir_s]);
    let c = claims_of(&ws.z(&["claim", &dir_s, "CVE-2026-0004"]));
    let versions = c[0]["versions"].as_array().unwrap();
    assert_eq!(versions.len(), 2, "{versions:#?}");
    assert_eq!(raw(&versions[0], "severity"), ["moderate"]);
    assert_eq!(raw(&versions[1], "severity"), ["critical"]);
}

#[test]
fn a_subscriber_holds_the_same_receipts() {
    let ws = Workspace::new("hub-receipts");
    let hub = ws.root.join("hub").display().to_string();
    let held = ws.root.join("elsewhere").display().to_string();
    let with_home = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_zetlyn"))
            .args(args)
            .env("ZETLYN_HOME", ws.root.join("home"))
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    with_home(&["source", "publish", &ws.dataset("vendor-a"), "--to", &hub]);
    with_home(&["source", "subscribe", "test/vendor-a", "--from", &hub, "--at", &held]);
    let first = claims_of(&with_home(&["claim", &held, "CVE-2026-0001"]));
    assert_eq!(first[0]["versions"].as_array().map(Vec::len), Some(1));
    assert_eq!(first[0]["excerpt"]["row"]["cvss"], "9.8");

    ws.update();
    with_home(&["source", "publish", &ws.dataset("vendor-a"), "--to", &hub]);
    let pulled = with_home(&["source", "pull", &held]);
    assert!(pulled.contains("by delta"), "{pulled}");

    let theirs = claims_of(&ws.z(&["claim", &ws.dataset("vendor-a"), "CVE-2026-0001"]));
    let ours = claims_of(&with_home(&["claim", &held, "CVE-2026-0001"]));
    assert_eq!(ours[0]["excerpt"], theirs[0]["excerpt"]);
    assert_eq!(ours[0]["versions"], theirs[0]["versions"]);
}
