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

/// What a watch would tell, from `watch check`'s report: `kind key` for a tracker's watch, the
/// identifier of each claim that moved for a source's.
fn delivered(report: &str, watch: &str) -> Vec<String> {
    let start = report.find(&format!("{watch}: ")).expect("the watch reported");
    let body = &report[start..];
    let Some(json_at) = body.find('{') else {
        return Vec::new();
    };
    let mut de = serde_json::Deserializer::from_str(&body[json_at..]).into_iter::<serde_json::Value>();
    let j = de.next().unwrap().unwrap();
    let mut out: Vec<String> = match j["signals"].as_array() {
        Some(signals) => signals
            .iter()
            .map(|s| {
                format!(
                    "{} {}{}",
                    s["kind"].as_str().unwrap_or(""),
                    s["value"].as_str().or(s["key"].as_str()).unwrap_or(""),
                    s["property"].as_str().map(|p| format!(" {p}")).unwrap_or_default()
                )
            })
            .collect(),
        None => j["things"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["ids"][0]["value"].as_str().unwrap_or("").to_string())
            .collect(),
    };
    out.sort();
    out
}

#[test]
fn watches() {
    let ws = Workspace::new("watches");
    let root = ws.root.display().to_string();
    std::fs::write(
        ws.root.join("watches/foo.yaml"),
        "name: foo\ntracker: test/cve\nthing: CVE-2026-0001\ndeliver:\n- to: feed\n",
    )
    .unwrap();
    // The tracker looks, and every watch takes its first look, which tells nothing and
    // remembers what a view held.
    ws.z(&["tracker", "refresh", &ws.scope()]);
    ws.z(&["watch", "check", &root, "--deliver"]);

    ws.update();
    let report = ws.z(&["watch", "check", &root]);

    // A view on the scale, not in words: 0004 became critical at vendor A and entered it;
    // `urgent` is on no scale and 0005 never did. Its members' signals come with it.
    assert_eq!(
        delivered(&report, "severe"),
        [
            "changed CVE-2026-0001 cvss",
            "changed CVE-2026-0004 cvss",
            "changed CVE-2026-0004 severity",
            "conflict CVE-2026-0001 cvss",
            "entered CVE-2026-0004",
        ]
    );
    // One thing, and everything about it.
    assert_eq!(
        delivered(&report, "foo"),
        ["changed CVE-2026-0001 cvss", "conflict CVE-2026-0001 cvss"]
    );
    // A watch over a source reads its query.
    assert_eq!(delivered(&report, "vendor-a-critical"), ["CVE-2026-0004"]);
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

fn json_of(printed: &str) -> serde_json::Value {
    serde_json::from_str(printed).unwrap()
}

/// `key property` of every open conflict, sorted.
fn open_conflicts(ws: &Workspace) -> Vec<String> {
    let mut out: Vec<String> = json_of(&ws.z(&["tracker", "conflicts", &ws.scope()]))
        .as_array()
        .unwrap()
        .iter()
        .map(|c| format!("{} {}", c["key"].as_str().unwrap(), c["property"].as_str().unwrap()))
        .collect();
    out.sort();
    out
}

/// `kind key property` of every signal, sorted.
fn signals(ws: &Workspace) -> Vec<String> {
    let mut out: Vec<String> = json_of(&ws.z(&["tracker", "signals", &ws.scope()]))
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            format!(
                "{} {} {}",
                s["kind"].as_str().unwrap_or(""),
                s["key"].as_str().unwrap_or("-"),
                s["property"].as_str().unwrap_or("-")
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn a_tracker_keeps_its_conflicts_and_says_what_changed() {
    let ws = Workspace::new("signals");
    let first = ws.z(&["tracker", "refresh", &ws.scope()]);
    assert!(first.contains("the first look, so none"), "{first}");
    // Two sources, two judgements: 0002 is critical to one and medium to the other, 9.8 and 7.5.
    // 0001 is `important` and `high`, which the map says are one word.
    assert_eq!(
        open_conflicts(&ws),
        ["cve:cve-2026-0002 cvss", "cve:cve-2026-0002 severity"]
    );
    assert!(signals(&ws).is_empty());

    ws.update();
    ws.z(&["tracker", "refresh", &ws.scope()]);
    assert_eq!(
        open_conflicts(&ws),
        [
            "cve:cve-2026-0001 cvss",
            "cve:cve-2026-0002 cvss",
            "cve:cve-2026-0002 severity"
        ]
    );
    assert_eq!(
        signals(&ws),
        [
            "changed cve:cve-2026-0001 cvss",
            "changed cve:cve-2026-0004 cvss",
            "changed cve:cve-2026-0004 severity",
            "conflict cve:cve-2026-0001 cvss",
        ]
    );

    // Back as it was, and the conflict that appeared resolves itself.
    std::fs::copy(
        fixtures().join("workspace/sources/vendor-a/advisories.csv"),
        ws.root.join("sources/vendor-a/advisories.csv"),
    )
    .unwrap();
    ws.z(&["source", "update", &ws.dataset("vendor-a")]);
    ws.z(&["tracker", "refresh", &ws.scope()]);
    assert!(signals(&ws).contains(&"resolved cve:cve-2026-0001 cvss".to_string()));

    // A rebuild starts the log again and judges the same.
    let rebuilt = ws.z(&["tracker", "refresh", &ws.scope(), "--rebuild"]);
    assert!(rebuilt.contains("2 conflicts"), "{rebuilt}");
    assert!(signals(&ws).is_empty());
}

#[test]
fn a_tolerance_and_a_word_nobody_mapped_are_not_conflicts() {
    let ws = Workspace::new("tolerance");
    let declared = ws.root.join("trackers/cve/tracker.yaml");
    let text = std::fs::read_to_string(&declared).unwrap();
    std::fs::write(&declared, text.replacen("  cvss: {}\n", "  cvss:\n    tolerance: \"2\"\n", 1))
        .unwrap();
    // Vendor B calls 0002 something no map covers.
    let b = ws.root.join("sources/vendor-b/advisories.csv");
    let rows = std::fs::read_to_string(&b).unwrap();
    std::fs::write(&b, rows.replace("Heap overflow in bar,medium", "Heap overflow in bar,severe")).unwrap();
    ws.z(&["source", "update", &ws.dataset("vendor-b")]);
    ws.update();

    let said = ws.z(&["tracker", "refresh", &ws.scope()]);
    // 9.8 against 7.5 is more than 2 apart, 8.1 against 9.8 is not; `severe` is wording.
    assert_eq!(open_conflicts(&ws), ["cve:cve-2026-0002 cvss"]);
    assert!(said.contains("1 that differ only in wording"), "{said}");
}

#[test]
fn a_question_about_things_is_answered_or_refused_by_name() {
    let ws = Workspace::new("questions");
    ws.z(&["tracker", "refresh", &ws.scope()]);
    let ask = |q: &str| -> Vec<String> {
        ws.z(&["tracker", "things", &ws.scope(), q]).lines().map(str::to_string).collect()
    };
    assert_eq!(ask("conflict:severity"), ["cve:cve-2026-0002"]);
    assert_eq!(ask("has:kev"), ["cve:cve-2026-0001", "cve:cve-2026-0004"]);
    assert_eq!(ask("only:exploits"), ["cve:cve-2026-0003"]);
    // A source by the end of its name, a number compared as a number.
    assert_eq!(ask("a.cvss>9"), ["cve:cve-2026-0001", "cve:cve-2026-0002"]);
    assert_eq!(ask("has:kev and not has:exploits"), ["cve:cve-2026-0004"]);

    // What cannot be answered says why, rather than answering with nothing.
    for (q, why) in [
        ("nosuch.cvss>1", "no source here is called that"),
        ("conflict:title", "only aligned properties are compared: cvss, exploited, severity"),
        ("a.cvss>b.cvss", "compared with = or != only"),
        // A property or kind no source has is refused with what there is.
        ("exploits.cvss>1", "says no cvss. It says:"),
        ("severty=high", "no source here says severty"),
        ("appeared:exploits<7d", "no source here makes claims of kind exploits"),
    ] {
        let (ok, said) = run(&ws.root, &["tracker", "things", &ws.scope(), q]);
        assert!(!ok && said.contains(why), "{q}: {said}");
    }
}

#[test]
fn a_rebuild_does_not_hide_what_a_watch_has_yet_to_hear() {
    let ws = Workspace::new("rebuild-watch");
    let root = ws.root.display().to_string();
    std::fs::write(
        ws.root.join("watches/foo.yaml"),
        "name: foo\ntracker: test/cve\nthing: CVE-2026-0001\ndeliver:\n- to: feed\n",
    )
    .unwrap();
    ws.z(&["tracker", "refresh", &ws.scope()]);
    ws.z(&["watch", "check", &root, "--deliver"]);
    // The watch hears the update and remembers the last signal it heard.
    ws.update();
    ws.z(&["watch", "check", &root, "--deliver"]);

    // The log starts again; the next change must still be past where the watch stopped.
    ws.z(&["tracker", "refresh", &ws.scope(), "--rebuild"]);
    std::fs::copy(
        fixtures().join("workspace/sources/vendor-a/advisories.csv"),
        ws.root.join("sources/vendor-a/advisories.csv"),
    )
    .unwrap();
    ws.z(&["source", "update", &ws.dataset("vendor-a")]);
    let report = ws.z(&["watch", "check", &root]);
    assert_eq!(
        delivered(&report, "foo"),
        ["changed CVE-2026-0001 cvss", "resolved CVE-2026-0001 cvss"]
    );
}

#[test]
fn a_claim_about_two_things_is_part_of_both() {
    let ws = Workspace::new("two-at-once");
    // One exploit for two vulnerabilities, as Exploit-DB writes it.
    let csv = ws.root.join("sources/exploits/exploits.csv");
    let mut rows = std::fs::read_to_string(&csv).unwrap();
    rows.push_str("104,Foo and Bar at once,CVE-2026-0004;CVE-2026-0005,linux,2026-09-06\n");
    std::fs::write(&csv, rows).unwrap();
    ws.z(&["source", "update", &ws.dataset("exploits")]);
    ws.z(&["tracker", "refresh", &ws.scope()]);
    let with_code: Vec<String> = ws
        .z(&["tracker", "things", &ws.scope(), "has:exploits"])
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(
        with_code,
        ["cve:cve-2026-0001", "cve:cve-2026-0002", "cve:cve-2026-0003", "cve:cve-2026-0004", "cve:cve-2026-0005"]
    );
    // And the measurement agrees with the store: five things carry an exploit.
    let m = json_of(&ws.z(&["tracker", "measure", &ws.scope()]));
    assert_eq!(m["with_an_exploit"], 5, "{m}");
}

/// The assist, answered from a recorded answer: the same run a model's answer would make, and
/// nothing on the network.
fn assisted(ws: &Workspace, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_zetlyn"))
        .args(args)
        .env("ZETLYN_ASSIST_REPLAY", fixtures().join("assist"))
        .env("ZETLYN_HOME", ws.root.join("home"))
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ZETLYN_ASSIST_URL")
        .output()
        .unwrap();
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    (out.status.success(), said)
}

#[test]
fn an_alignment_is_proposed_counted_and_kept_with_its_comments() {
    let ws = Workspace::new("align");
    let file = ws.root.join("trackers/cve/tracker.yaml");
    // Without its map, and with a comment somebody wrote.
    let text = std::fs::read_to_string(&file).unwrap();
    let mut kept = String::new();
    let mut skipping = false;
    for line in text.lines() {
        if line == "  severity:" {
            kept.push_str("  # Ours, by hand.\n  severity: {}\n");
            skipping = true;
            continue;
        }
        if skipping && line.starts_with("   ") {
            continue;
        }
        skipping = false;
        kept.push_str(line);
        kept.push('\n');
    }
    std::fs::write(&file, &kept).unwrap();
    let scope = ws.scope();

    // Nothing leaves before the person has said so, and what would leave is said.
    let (ok, said) = assisted(&ws, &["assist", "align", &scope, "severity"]);
    assert!(!ok && said.contains("the words 2 sources use for severity"), "{said}");

    let (ok, said) = assisted(&ws, &["assist", "align", &scope, "severity", "--send", "--apply"]);
    assert!(ok, "{said}");
    // important and moderate are vendor A's words for high and medium; urgent means nothing on
    // the scale and stays unmapped, so 0002's critical against medium is the one left.
    assert!(said.contains("2 disagreements after the map now, 1 with this one"), "{said}");
    let after = std::fs::read_to_string(&file).unwrap();
    assert!(after.contains("  # Ours, by hand.\n  severity:\n    scale:\n    - critical"), "{after}");
    assert!(after.contains("      important: high"), "{after}");
    assert!(!after.contains("urgent"), "{after}");
    let record = std::fs::read_to_string(ws.root.join("trackers/cve/assist.yaml")).unwrap();
    assert!(record.contains("allowed: true") && record.contains("task: align"), "{record}");
}

#[test]
fn a_question_in_words_becomes_a_filter_or_is_refused() {
    let ws = Workspace::new("ask");
    ws.z(&["tracker", "refresh", &ws.scope()]);
    let scope = ws.scope();
    let (ok, said) = assisted(&ws, &["assist", "ask", &scope, "exploited ones that also have exploit code", "--send"]);
    assert!(ok, "{said}");
    assert!(said.starts_with("has:kev and has:exploits\n"), "{said}");
    assert!(said.contains("1 thing\n"), "{said}");
    // An answer naming a source that is not here goes back once with the parser's words, and a
    // second that is still wrong is refused rather than run.
    let (ok, said) = assisted(&ws, &["assist", "ask", &scope, "What does vendor C say is high?", "--send"]);
    assert!(!ok && said.contains("no filter came out of it") && said.contains("no source here is called that"), "{said}");
}

#[test]
fn a_check_that_finds_something_untrue_fails() {
    let ws = Workspace::new("check");
    // Held and true: zero.
    let (ok, said) = run(&ws.root, &["source", "check", &ws.dataset("kev")]);
    assert!(ok && said.contains("nothing it claims is untrue"), "{said}");
    // Never updated, so it holds nothing it could be checked on, which a CI must not pass.
    let empty = ws.root.join("sources/empty");
    std::fs::create_dir_all(&empty).unwrap();
    std::fs::copy(ws.root.join("sources/kev/source.yaml"), empty.join("source.yaml")).unwrap();
    let (ok, said) = run(&ws.root, &["source", "check", &empty.display().to_string()]);
    assert!(!ok && said.contains("holds no claims"), "{said}");
}

#[test]
fn a_declaration_read_again_is_not_news() {
    let ws = Workspace::new("reshaped");
    ws.z(&["tracker", "refresh", &ws.scope()]);
    // A property the source always had and the declaration never read: every claim now says
    // something new, and nothing in the world has changed.
    let file = ws.root.join("sources/vendor-a/source.yaml");
    let text = std::fs::read_to_string(&file).unwrap();
    let text = text.replacen("  properties:\n", "  properties:\n    headline:\n      type: text\n      from: field:title\n", 1);
    std::fs::write(&file, text).unwrap();
    ws.z(&["source", "update", &ws.dataset("vendor-a"), "--reread"]);
    ws.z(&["tracker", "refresh", &ws.scope()]);
    assert_eq!(signals(&ws), ["health - -"], "one health signal, and no change for every claim");
    // The next real change is news again.
    ws.update();
    ws.z(&["tracker", "refresh", &ws.scope()]);
    assert!(signals(&ws).contains(&"changed cve:cve-2026-0001 cvss".to_string()), "{:?}", signals(&ws));
}

#[test]
fn a_relation_is_what_a_claim_states_or_a_person_signs() {
    let ws = Workspace::new("relations");
    // Exploit-DB's entries name their own number beside the CVE, so a claim states both.
    let file = ws.root.join("trackers/cve/tracker.yaml");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, format!("{text}relations:\n- name: listed_as\n  to: edb\n")).unwrap();
    ws.z(&["tracker", "refresh", &ws.scope()]);
    let ask = |q: &str| -> Vec<String> { ws.z(&["tracker", "things", &ws.scope(), q]).lines().map(str::to_string).collect() };
    assert_eq!(ask("listed_as:*"), ["cve:cve-2026-0001", "cve:cve-2026-0002", "cve:cve-2026-0003"]);
    assert_eq!(ask("listed_as:102"), ["cve:cve-2026-0003"]);

    // Where no claim states it, a person may, and it counts as theirs.
    let scope = ws.scope();
    let (ok, said) = run(&ws.root, &["tracker", "match", &scope, "CVE-2026-0004", "listed_as", "999"]);
    assert!(!ok && said.contains("say who with --by"), "{said}");
    ws.z(&["tracker", "match", &scope, "CVE-2026-0004", "listed_as", "999", "--by", "a reviewer", "--why", "the advisory links it"]);
    assert_eq!(ask("listed_as:999"), ["cve:cve-2026-0004"]);
    ws.z(&["tracker", "match", &scope, "CVE-2026-0004", "listed_as", "999", "--by", "a reviewer", "--withdraw"]);
    assert!(ask("listed_as:999").is_empty());
    let record = std::fs::read_to_string(ws.root.join("trackers/cve/matches.jsonl")).unwrap();
    assert_eq!(record.lines().count(), 2, "the match and its withdrawal both stay");

    let (ok, said) = run(&ws.root, &["tracker", "match", &scope, "CVE-2026-0004", "fixes", "x", "--by", "a"]);
    assert!(!ok && said.contains("this tracker's relations are listed_as"), "{said}");
}

#[test]
fn a_public_tracker_needs_every_source_to_say_it_may_be() {
    let ws = Workspace::new("licence");
    // Published at all, a tracker says what it covers; that check is not this test's.
    let promised = ws.root.join("trackers/cve/tracker.yaml");
    let t = std::fs::read_to_string(&promised).unwrap();
    std::fs::write(&promised, format!("{t}promise:\n  fresh_within: 24h\n  covers: Four advisories.\n  excludes: Everything else.\n")).unwrap();
    let hub = ws.root.join("hub");
    let hub = hub.display().to_string();
    let scope = ws.scope();
    let publish = || run(&ws.root, &["tracker", "publish", &scope, "--to", &hub]);
    let (ok, said) = publish();
    assert!(!ok && said.contains("test/kev has not said whether it may be republished"), "{said}");

    // Private, it is its accounts' alone, and needs nobody's permission to be shown to them.
    let file = ws.root.join("trackers/cve/tracker.yaml");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, format!("{text}visibility: private\n")).unwrap();
    let (ok, said) = publish();
    assert!(ok, "{said}");
    std::fs::write(&file, &text).unwrap();

    let declare = |source: &str, republish: &str| {
        let f = ws.root.join(format!("sources/{source}/source.yaml"));
        let t = std::fs::read_to_string(&f).unwrap();
        let t = t.split("\nlicence:").next().unwrap().to_string();
        std::fs::write(&f, format!("{t}\nlicence:\n  republish: {republish}\n")).unwrap();
    };
    for s in ["kev", "vendor-a", "vendor-b", "exploits"] {
        declare(s, "yes");
    }
    let (ok, said) = publish();
    assert!(ok, "{said}");
    declare("exploits", "no");
    let (ok, said) = publish();
    assert!(!ok && said.contains("test/exploits may not be republished"), "{said}");
}

#[test]
fn a_licence_corrected_reaches_a_subscriber_without_a_new_version() {
    let ws = Workspace::new("hub-licence");
    let hub = ws.root.join("hub").display().to_string();
    let held = ws.root.join("elsewhere").display().to_string();
    let with_home = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_zetlyn")).args(args).env("ZETLYN_HOME", ws.root.join("home")).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    with_home(&["source", "publish", &ws.dataset("vendor-a"), "--to", &hub]);
    with_home(&["source", "subscribe", "test/vendor-a", "--from", &hub, "--at", &held]);
    let theirs = std::path::Path::new(&held).join("source.yaml");
    assert!(!std::fs::read_to_string(&theirs).unwrap().contains("licence:"));

    // The same claims, and now a word on showing them.
    let file = ws.root.join("sources/vendor-a/source.yaml");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, format!("{text}licence:\n  republish: summary\n  terms: https://example.org/terms\n")).unwrap();
    with_home(&["source", "publish", &ws.dataset("vendor-a"), "--to", &hub]);
    let pulled = with_home(&["source", "pull", &held]);
    assert!(pulled.contains("licence and terms, taken") && pulled.contains("which is what you hold"), "{pulled}");
    let now = std::fs::read_to_string(&theirs).unwrap();
    assert!(now.contains("republish: summary") && now.contains("https://example.org/terms"), "{now}");
}

#[test]
fn one_mailer_serves_every_workspace_that_names_none() {
    let ws = Workspace::new("central-mail");
    let root = ws.root.display().to_string();
    std::fs::write(ws.root.join("watches/by-mail.yaml"), "name: by-mail\ntracker: test/cve\nthing: CVE-2026-0001\ndeliver:\n- to: mail\n  address: reader@example.org\n").unwrap();
    ws.z(&["tracker", "refresh", &ws.scope()]);
    ws.z(&["watch", "check", &root, "--deliver"]);
    ws.update();
    ws.z(&["tracker", "refresh", &ws.scope()]);
    let check = |central: Option<&std::path::Path>| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_zetlyn"));
        c.args(["watch", "check", &root, "--deliver"]).env("ZETLYN_MAIL", central.map(|p| p.display().to_string()).unwrap_or_else(|| "/nonexistent".into()));
        let out = c.output().unwrap();
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
    };
    assert!(check(None).contains("no mailer"), "without one, nothing is sent");
    // A server on this machine that is not there: the attempt is the proof the file was read.
    let file = ws.root.join("mail.yaml");
    std::fs::write(&file, "smtp:\n  host: 127.0.0.1\n  port: 9\n  tls: none\n  from: Zetlyn <noreply@zetlyn.test>\n").unwrap();
    let said = check(Some(&file));
    assert!(said.contains("127.0.0.1:9"), "{said}");
}
