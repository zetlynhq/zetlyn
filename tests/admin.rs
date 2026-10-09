//! The main server's admin pages, run as the operator runs them: a copy of
//! `tests/fixtures/control` served by the binary on a port of its own, signed in to by the link
//! the sign-in prints, each page asked for, each form sent. What a form asks for is a job in
//! `ops/jobs/`, which here nobody runs: the jobs themselves need servers, SSH and the bucket, and
//! are tried on the real ones by `deploy/selftest.sh` in zetlyn-ops.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

const OPERATOR: &str = "op@example.org";

/// A copy of the fixture, served until it is dropped.
struct Control {
    dir: PathBuf,
    port: u16,
    child: Child,
}

impl Control {
    fn start(name: &str, maintenance: Option<&str>) -> Control {
        let dir = std::env::temp_dir().join(format!("zetlyn-admin-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        copy(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/control"), &dir);
        if let Some(mode) = maintenance {
            let n = format!(r#"{{"text":"Testing.","level":"info","mode":"{mode}","target":"all","announce_from":"2000-01-01T00:00:00Z","from":"2000-01-01T00:00:00Z","until":"2999-01-01T00:00:00Z"}}"#);
            std::fs::write(dir.join("ops/maintenance.json"), n).unwrap();
        }
        // A port of this run's own for each control: a port the system calls free is free for
        // every test running beside this one too, until one of them takes it. Taken meanwhile,
        // the control cannot listen and ends, and is started again on another.
        static NEXT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);
        for _ in 0..5 {
            let port = (0..200)
                .map(|_| 30_000 + (std::process::id() % 1_000) as u16 * 10 + NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst) % 200)
                .find(|p| std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok())
                .expect("a free port");
            std::fs::write(dir.join("workspace.yaml"), format!("title: Zetlyn\nurl: http://127.0.0.1:{port}\naccess:\n  owners: [{OPERATOR}]\n")).unwrap();
            let log = std::fs::File::create(dir.join("log")).unwrap();
            let child = Command::new(env!("CARGO_BIN_EXE_zetlyn"))
                .args(["hosting", "serve", &dir.display().to_string(), "--addr", &format!("127.0.0.1:{port}"), "--no-updates"])
                .stdout(Stdio::from(log.try_clone().unwrap()))
                .stderr(Stdio::from(log))
                .spawn()
                .expect("the binary runs");
            let mut c = Control { dir: dir.clone(), port, child };
            for _ in 0..100 {
                if let Ok(Some(_)) = c.child.try_wait() {
                    break;
                }
                // This process answering, not one of a test beside it that took the port meanwhile.
                if std::fs::read_to_string(c.dir.join("log")).is_ok_and(|l| l.contains(&format!("127.0.0.1:{port}"))) && TcpStream::connect(("127.0.0.1", port)).is_ok() {
                    return c;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            // Not this one: ended, or never answered. Its folder stays for the next try.
            let _ = c.child.kill();
            let _ = c.child.wait();
            std::mem::forget(c);
        }
        panic!("the control did not answer in five tries: {}", std::fs::read_to_string(dir.join("log")).unwrap_or_default());
    }

    /// One request, by hand: status, headers, body. No redirect is followed.
    fn ask(&self, method: &str, path: &str, cookie: Option<&str>, form: &str) -> (u16, String, String) {
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(20))).unwrap();
        let mut head = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n", self.port);
        if let Some(c) = cookie {
            head.push_str(&format!("Cookie: zs={c}\r\n"));
        }
        if method == "POST" {
            head.push_str(&format!("Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n", form.len()));
        }
        head.push_str("\r\n");
        s.write_all(head.as_bytes()).unwrap();
        s.write_all(form.as_bytes()).unwrap();
        let mut raw = Vec::new();
        let _ = s.read_to_end(&mut raw);
        let text = String::from_utf8_lossy(&raw).into_owned();
        let (h, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let status = h.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
        (status, h.to_string(), body.to_string())
    }

    fn get(&self, path: &str, cookie: Option<&str>) -> (u16, String) {
        let (s, _, b) = self.ask("GET", path, cookie, "");
        (s, b)
    }

    /// Signed in as `email` by the link the control prints: its session.
    fn sign_in(&self, email: &str) -> String {
        let (s, _, _) = self.ask("POST", "/account/signin", None, &format!("email={}", email.replace('@', "%40")));
        assert!(s == 200 || s == 303, "sign-in answered {s}");
        let log = std::fs::read_to_string(self.dir.join("log")).unwrap();
        let link = log.lines().filter(|l| l.contains("/account/signin/")).last().expect("a sign-in link in the log").to_string();
        let raw = link.split("/account/signin/").nth(1).unwrap().split_whitespace().next().unwrap().to_string();
        let (s, h, _) = self.ask("GET", &format!("/account/signin/{raw}"), None, "");
        assert_eq!(s, 303, "the link signs in");
        h.lines().find_map(|l| l.strip_prefix("Set-Cookie: zs=").or_else(|| l.strip_prefix("set-cookie: zs="))).and_then(|v| v.split(';').next()).expect("a session").to_string()
    }

    /// Every job the pages wrote, as the root side would read them.
    fn jobs(&self) -> Vec<serde_json::Value> {
        let mut out: Vec<serde_json::Value> = std::fs::read_dir(self.dir.join("ops/jobs")).into_iter().flatten().flatten().filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()).collect();
        out.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        out
    }

    fn job(&self, action: &str) -> serde_json::Value {
        self.jobs().into_iter().rev().find(|j| j["action"] == action).unwrap_or_else(|| panic!("no {action} job in {:?}", self.jobs()))
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy(&e.path(), &target);
        } else {
            std::fs::copy(e.path(), target).unwrap();
        }
    }
}

#[test]
fn the_admin_pages_are_the_operators_alone() {
    let c = Control::start("alone", None);
    assert_eq!(c.get("/account/admin/", None).0, 404, "nobody signed in");
    let somebody = c.sign_in("someone@example.org");
    assert_eq!(c.get("/account/admin/", Some(&somebody)).0, 404, "signed in, not the operator");
    let op = c.sign_in(OPERATOR);
    assert_eq!(c.get("/account/admin/", Some(&op)).0, 200);
    // The operator's account menu leads there; nobody else's does.
    assert!(c.get("/account/", Some(&op)).1.contains("/account/admin/"));
    assert!(!c.get("/account/", Some(&somebody)).1.contains("/account/admin/\""));
}

#[test]
fn every_admin_page_shows_what_it_is_for() {
    let c = Control::start("pages", None);
    let op = c.sign_in(OPERATOR);
    let pages: [(&str, &[&str]); 7] = [
        ("/account/admin/", &["Servers", "Bucket", "Cells", "acme", "pilot", "pilot on n1 is failed", "no new cells", "Left by removed cells", "gone"]),
        ("/account/admin/cell/acme", &["Acme Research", "Status", "Actions", "What it is", "What it may use", "Billing", "Snapshots", "Remove"]),
        ("/account/admin/cell/pilot", &["Free until 2099-12-31", "value=\"5000\"", "Plan and usage", "Zetlyn Managed", "5,000", "25000 (plan)", "RAM", "Storage used"]),
        ("/account/admin/new", &["New cell", "name=\"owners\"", "name=\"billing\"", "type=\"file\""]),
        ("/account/admin/customers", &["Customers", "Held for a checkout", "Cancellations"]),
        ("/account/admin/activity", &["Activity", "Alarms"]),
        ("/account/admin/maintenance", &["Maintenance", "name=\"mode\"", "readonly", "closed"]),
    ];
    for (path, wanted) in pages {
        let (status, body) = c.get(path, Some(&op));
        assert_eq!(status, 200, "{path}");
        for w in wanted {
            assert!(body.contains(w), "{path} does not say {w:?}");
        }
    }
    assert_eq!(c.get("/account/admin/mail", Some(&op)).0, 200);
    assert_eq!(c.get("/account/admin/cell/nowhere", Some(&op)).0, 404);
    // The house is never offered for removal.
    assert!(!c.get("/account/admin/cell/house", Some(&op)).1.contains("/remove\""));
}

#[test]
fn every_form_asks_for_the_job_it_says() {
    let c = Control::start("forms", None);
    let op = c.sign_in(OPERATOR);
    let post = |path: &str, form: &str| {
        let (s, h, _) = c.ask("POST", path, Some(&op), form);
        assert_eq!(s, 303, "{path}: {h}");
    };
    post("/account/admin/cell/acme/limits", "billing=free&free_until=2026-12-31&reads=5000&memory=1G&ignored=x");
    let j = c.job("limits");
    assert_eq!((j["cell"].as_str(), j["args"]["reads"].as_str(), j["args"]["memory"].as_str(), j["args"]["billing"].as_str()), (Some("acme"), Some("5000"), Some("1G"), Some("free")));
    assert!(j["args"].get("ignored").is_none(), "only the form's own fields travel");
    assert_eq!(j["by"], OPERATOR);

    post("/account/admin/cell/acme/set", "title=Acme&owners=a%40x.org%0Ab%40x.org&domain=&note=hi");
    let j = c.job("set");
    assert_eq!((j["args"]["title"].as_str(), j["args"]["owners"].as_str()), (Some("Acme"), Some("a@x.org\nb@x.org")));

    post("/account/admin/new", "cell=fresh&title=Fresh&owners=f%40x.org&node=n1&billing=free&welcome=no&welcome=yes");
    let j = c.job("create");
    assert_eq!((j["cell"].as_str(), j["args"]["owners"].as_str(), j["args"]["welcome"].as_str()), (Some("fresh"), Some("f@x.org"), Some("yes")));

    for (path, form, action, target) in [
        ("/account/admin/cell/acme/restart", "", "restart", "acme"),
        ("/account/admin/cell/acme/snapshots", "", "snapshots", "acme"),
        ("/account/admin/cell/acme/restore", "stamp=20261008T030000Z&confirm=acme", "restore", "acme"),
        ("/account/admin/cell/acme/download", "stamp=20261008T030000Z", "download", "acme"),
        ("/account/admin/cell/acme/remove", "confirm=acme", "remove", "acme"),
        ("/account/admin/node/n2/undrain", "", "undrain", "n2"),
        ("/account/admin/nodes", "name=n3&host=203.0.113.3", "node-add", "n3"),
        ("/account/admin/upgrade-all", "version=0.3.80", "upgrade-all", "all"),
        ("/account/admin/purge/gone", "confirm=gone", "purge", "gone"),
    ] {
        post(path, form);
        assert_eq!(c.job(action)["cell"], target, "{path}");
    }
    // Nothing that is not an action becomes a job.
    let (s, _, _) = c.ask("POST", "/account/admin/cell/acme/format-the-disk", Some(&op), "");
    assert_eq!(s, 400);
    // What was asked is in the activity, by whom.
    let audit: String = std::fs::read_dir(c.dir.join("ops")).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("audit-")).map(|e| std::fs::read_to_string(e.path()).unwrap()).collect();
    assert!(audit.contains("asked limits") && audit.contains(OPERATOR));
    let activity = c.get("/account/admin/activity", Some(&op)).1;
    assert!(activity.contains(">restore<") && activity.contains("waiting"), "asked, not yet answered");
    // Nobody else's form does anything.
    let somebody = c.sign_in("someone@example.org");
    let before = c.jobs().len();
    let _ = c.ask("POST", "/account/admin/cell/acme/remove", Some(&somebody), "confirm=acme");
    assert_eq!(c.jobs().len(), before);
}

#[test]
fn a_maintenance_is_written_and_announced() {
    let c = Control::start("announce", None);
    let op = c.sign_in(OPERATOR);
    let (s, _, _) = c.ask("POST", "/account/admin/maintenance", Some(&op), "text=Faster+disks&from=2999-01-01T22%3A00&until=2999-01-01T23%3A00&mode=readonly&target=node%3An1&level=warning");
    assert_eq!(s, 303);
    let n: serde_json::Value = serde_json::from_slice(&std::fs::read(c.dir.join("ops/maintenance.json")).unwrap()).unwrap();
    assert_eq!((n["mode"].as_str(), n["target"].as_str(), n["from"].as_str()), (Some("readonly"), Some("node:n1"), Some("2999-01-01T22:00:00Z")));
    // From before until, or nothing.
    let (s, _, _) = c.ask("POST", "/account/admin/maintenance", Some(&op), "from=2999-01-01T23%3A00&until=2999-01-01T22%3A00");
    assert_eq!(s, 400);
    let (s, _, _) = c.ask("POST", "/account/admin/maintenance", Some(&op), "clear=1");
    assert_eq!(s, 303);
    assert!(!c.dir.join("ops/maintenance.json").exists());
}

#[test]
fn closed_is_closed_but_for_the_operator() {
    let c = Control::start("closed", Some("closed"));
    let (s, body) = c.get("/order", None);
    assert_eq!(s, 503);
    assert!(body.contains("Testing."));
    assert_eq!(c.get("/account/", None).0, 503);
    // What signing in, Stripe and the website's banner need stays open.
    assert_eq!(c.get("/account/signin", None).0, 200);
    let (s, body) = c.get("/account/maintenance", None);
    assert_eq!(s, 200);
    assert!(body.contains("\"state\":\"active\"") && body.contains("closed"), "{body}");
    // Only the operator signs in, and gets through.
    let (s, _, _) = c.ask("POST", "/account/signin", None, "email=someone%40example.org");
    assert_eq!(s, 503);
    let op = c.sign_in(OPERATOR);
    assert_eq!(c.get("/account/", Some(&op)).0, 200);
    assert_eq!(c.get("/account/admin/", Some(&op)).0, 200);
}

#[test]
fn read_only_lets_reading_on_and_ordering_off() {
    let c = Control::start("readonly", Some("readonly"));
    assert_eq!(c.get("/account/signin", None).0, 200);
    assert!(c.get("/account/signin", None).1.contains("maintenance-banner"), "the banner is on the page");
    assert_eq!(c.get("/order", None).0, 503);
    let (s, _, _) = c.ask("POST", "/account/cancel", None, "world=acme&email=a%40b.c");
    assert_eq!(s, 200, "cancelling is never under maintenance");
    let (s, _, _) = c.ask("POST", "/account/new", None, "cell=x");
    assert_eq!(s, 503, "nothing else is changed");
}

#[test]
fn no_login_stops_new_sign_ins_and_orders_only() {
    let c = Control::start("nologin", Some("nologin"));
    assert_eq!(c.get("/order", None).0, 503);
    let (s, _, _) = c.ask("POST", "/account/signin", None, "email=someone%40example.org");
    assert_eq!(s, 503);
    let op = c.sign_in(OPERATOR);
    assert_eq!(c.get("/account/", Some(&op)).0, 200);
}

#[test]
fn a_cancellation_is_taken_from_anybody_but_not_from_a_script() {
    let c = Control::start("cancel", None);
    let (s, body) = c.get("/account/cancel", None);
    assert_eq!(s, 200);
    assert!(body.contains("name=\"website\""), "the field only scripts fill");
    // A script fills the hidden field: answered as if taken, held back.
    let (s, _, _) = c.ask("POST", "/account/cancel", None, "world=acme&email=anna%40acme.example&kind=ordinary&website=spam");
    assert_eq!(s, 200);
    // Five an hour from one address; one mail a ten minutes about one organisation.
    for i in 0..6 {
        let (s, _, _) = c.ask("POST", "/account/cancel", None, &format!("world=org{i}&email=x%40y.z&kind=ordinary"));
        assert_eq!(s, 200);
    }
    let _ = c.ask("POST", "/account/cancel", None, "world=org0&email=x%40y.z&kind=ordinary");
    let lines: Vec<serde_json::Value> = std::fs::read_to_string(c.dir.join("billing/cancellations.jsonl")).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let held: Vec<bool> = lines.iter().map(|l| l["throttled"] == true).collect();
    assert_eq!(held, vec![true, false, false, false, false, false, true, true], "{lines:?}");
    // Every cancellation is kept, held back or not, and none signed in.
    assert!(lines.iter().all(|l| l["signed_in"] == false));
}

#[test]
fn the_activity_is_filtered_paged_and_one_line_a_job() {
    let c = Control::start("activity", None);
    let op = c.sign_in(OPERATOR);
    for (cell, action) in [("acme", "restart"), ("pilot", "snapshot"), ("acme", "logs")] {
        let (s, _, _) = c.ask("POST", &format!("/account/admin/cell/{cell}/{action}"), Some(&op), "");
        assert_eq!(s, 303);
    }
    // A job the root side answered: its asking and its answer, one line.
    let restart = c.job("restart");
    let id = restart["id"].as_str().unwrap();
    let month = std::fs::read_dir(c.dir.join("ops")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).find(|n| n.starts_with("audit-") && n.ends_with(".jsonl")).expect("a log of this month");
    let mut f = std::fs::OpenOptions::new().append(true).open(c.dir.join("ops").join(&month)).unwrap();
    writeln!(f, r#"{{"at":"2999-01-01T00:00:00Z","by":"{OPERATOR}","what":"done restart","cell":"acme","said":"acme: restart ok","job":"{id}"}}"#).unwrap();
    let page = |q: &str| c.get(&format!("/account/admin/activity{q}"), Some(&op)).1;
    let all = page("?since=all");
    assert!(all.contains("acme: restart ok"), "the answer is on the row it was asked on");
    assert_eq!(all.matches(">restart<").count(), 1, "one row for the job, not two");
    assert!(all.contains(">snapshot<") && all.contains(">logs<"));
    let acme = page("?since=all&cell=acme");
    assert!(!acme.contains(">snapshot<"), "only acme's");
    let waiting = page("?since=all&kind=waiting");
    assert!(waiting.contains(">snapshot<") && !waiting.contains(">restart<"), "the answered one is not waiting");
    let found = page("?since=all&q=pilot");
    assert!(found.contains(">snapshot<") && !found.contains(">logs<"));
    assert!(page("?tab=alarms").contains("No alarm in this window"));
    assert!(page("?from=2000-01-01&to=2000-01-02").contains("Nothing in this window"));
}

/// The binary, run as a person runs it: whether it went, and what it printed.
fn z(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_zetlyn")).args(args).output().expect("the binary runs");
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

#[test]
fn a_world_and_a_copy_of_it_sync_both_ways() {
    let c = Control::start("sync", None);
    let world = c.dir.join("orgs/acme");
    copy(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workspace"), &world);
    std::fs::write(world.join("workspace.yaml"), "title: Acme\n").unwrap();
    std::fs::write(c.dir.join("members.yaml"), format!("acme:\n  - email: {OPERATOR}\n    role: owner\n")).unwrap();
    let (ok, said) = z(&["source", "update", &world.join("sources/kev").display().to_string()]);
    assert!(ok, "{said}");
    let (ok, said) = z(&["source", "update", &world.join("sources/vendor-a").display().to_string()]);
    assert!(ok, "{said}");

    let url = format!("http://127.0.0.1:{}/acme", c.port);
    assert_eq!(c.get("/acme/sync/state", None).0, 401, "nobody without a key");
    let op = c.sign_in(OPERATOR);
    let (s, _, page) = c.ask("POST", "/acme/settings/sync-key", Some(&op), "");
    assert_eq!(s, 200, "{page}");
    let key = page.split("--key ").nth(1).and_then(|k| k.split(|c: char| !c.is_ascii_alphanumeric() && c != '_').next()).expect("a key on the page").to_string();
    assert!(key.starts_with("zk_"), "{key}");

    // Into an empty folder: the whole world, data and all.
    let copy_dir = std::env::temp_dir().join(format!("zetlyn-sync-copy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&copy_dir);
    let mine = copy_dir.display().to_string();
    let (ok, said) = z(&["world", "sync", &url, &mine, "--key", &key]);
    assert!(ok, "{said}");
    assert!(copy_dir.join("sources/kev/source.yaml").exists(), "{said}");
    assert!(copy_dir.join("sources/kev/db").exists() || std::fs::read_dir(copy_dir.join("sources/kev")).unwrap().count() > 1);

    // A change on each side, to different things: both arrive on both, nobody waits for anybody.
    let theirs = world.join("trackers/cve/tracker.yaml");
    let t = std::fs::read_to_string(&theirs).unwrap();
    std::fs::write(&theirs, t.replacen("title: CVE", "title: CVE there", 1)).unwrap();
    let ours = copy_dir.join("sources/vendor-b/source.yaml");
    let o = std::fs::read_to_string(&ours).unwrap();
    std::fs::write(&ours, o.replacen("title:", "title: Mine,", 1)).unwrap();
    // Who may do what, decided on the machine, holds there too.
    let ws = copy_dir.join("workspace.yaml");
    let w = std::fs::read_to_string(&ws).unwrap();
    assert!(!w.contains("access:"), "{w}");
    std::fs::write(&ws, format!("{}\naccess:\n  editors:\n  - ed@example.org\n", w.trim_end())).unwrap();
    let (ok, said) = z(&["world", "sync", &url, &mine]);
    assert!(ok, "the key is kept after the first sync: {said}");
    assert!(std::fs::read_to_string(world.join("workspace.yaml")).unwrap().contains("ed@example.org"), "{said}");
    assert!(std::fs::read_to_string(world.join("sources/vendor-b/source.yaml")).unwrap().contains("Mine,"), "{said}");
    assert!(std::fs::read_to_string(copy_dir.join("trackers/cve/tracker.yaml")).unwrap().contains("CVE there"), "{said}");

    // A source updated there: its new values arrive here, and what it said before stays as history.
    std::fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/update-2/vendor-a.csv"), world.join("sources/vendor-a/advisories.csv")).unwrap();
    let (ok, said) = z(&["source", "update", &world.join("sources/vendor-a").display().to_string()]);
    assert!(ok, "{said}");
    let (ok, said) = z(&["world", "sync", &url, &mine]);
    assert!(ok, "{said}");
    let (ok, printed) = z(&["claim", &copy_dir.join("sources/vendor-a").display().to_string(), "CVE-2026-0001"]);
    assert!(ok, "{printed}");
    let claim: serde_json::Value = serde_json::from_str(printed.lines().find(|l| l.starts_with('{') || l.starts_with('[')).map(|_| &printed[printed.find(['{', '[']).unwrap()..]).unwrap_or("null")).unwrap_or_default();
    let claim = if claim.is_array() { claim[0].clone() } else { claim };
    assert_eq!(claim["excerpt"]["row"]["cvss"], "8.1", "{printed}");
    assert_eq!(claim["versions"].as_array().map(Vec::len), Some(2), "{printed}");

    // Nothing changed: a second sync changes nothing either.
    let (ok, said) = z(&["world", "sync", &url, &mine]);
    assert!(ok, "{said}");

    // From the app on the machine, as its page asks: the address and key kept from before. The
    // same setting changed on both sides is decided there, and nothing goes until it is.
    let app = LocalApp::start(&copy_dir);
    let theirs = world.join("trackers/cve/tracker.yaml");
    let ours = copy_dir.join("trackers/cve/tracker.yaml");
    std::fs::write(&theirs, std::fs::read_to_string(&theirs).unwrap().replacen("title: CVE there", "title: CVE from there", 1)).unwrap();
    std::fs::write(&ours, std::fs::read_to_string(&ours).unwrap().replacen("title: CVE there", "title: CVE from here", 1)).unwrap();
    let state = app.sync("");
    assert_eq!(state["state"], "failed", "{state}");
    assert_eq!(state["conflicts"], true, "{state}");
    assert!(std::fs::read_to_string(&theirs).unwrap().contains("CVE from there"), "nothing went: {state}");
    let state = app.sync("take=theirs");
    assert_eq!(state["state"], "ok", "{state}");
    assert!(std::fs::read_to_string(&ours).unwrap().contains("CVE from there"), "{state}");
    drop(app);
    let _ = std::fs::remove_dir_all(&copy_dir);
}

/// The app on one's own machine, over a workspace, on a port of its own, until it is dropped.
struct LocalApp {
    child: Child,
    port: u16,
    dir: PathBuf,
    log: PathBuf,
}

impl LocalApp {
    fn start(dir: &Path) -> LocalApp {
        let port = (0..50).map(|i| 40_000 + (std::process::id() % 2_000) as u16 * 5 + i).find(|p| std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok()).expect("a free port");
        let log = std::env::temp_dir().join(format!("zetlyn-localapp-{}-{port}.log", std::process::id()));
        let child = Command::new(env!("CARGO_BIN_EXE_zetlyn")).args(["--port", &port.to_string(), "--no-open"]).current_dir(dir).stdin(Stdio::null()).stdout(Stdio::from(std::fs::File::create(&log).unwrap())).stderr(Stdio::null()).spawn().expect("the binary runs");
        for _ in 0..100 {
            if std::fs::read_to_string(&log).is_ok_and(|l| l.contains(&format!("127.0.0.1:{port}"))) && TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return LocalApp { child, port, dir: dir.to_path_buf(), log };
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("the app did not answer on {port}");
    }

    /// Settings, Moving, Sync now, with what the form says; how it went, once it has.
    fn sync(&self, form: &str) -> serde_json::Value {
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let head = format!("POST /settings/sync HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{form}", self.port, form.len());
        s.write_all(head.as_bytes()).unwrap();
        let mut answer = String::new();
        let _ = s.read_to_string(&mut answer);
        assert!(answer.starts_with("HTTP/1.1 303"), "{answer}");
        let file = self.dir.join(".zetlyn/sync/last.json");
        for _ in 0..200 {
            if let Some(j) = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()).filter(|j| j["state"] != "running") {
                return j;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("the sync did not end");
    }
}

impl Drop for LocalApp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.log);
    }
}
