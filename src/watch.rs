//! A saved query is a subscription. The tracker replays it at each run and says what entered, what
//! left, and what changed inside a thing.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};

use crate::source::{Source, Interface};
use crate::expr::{self, Pred};
use crate::tracker::Tracker;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WatchDecl {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// One of the two, because a watch is over a tracker or over a source on its own.
    #[serde(rename = "tracker", skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(rename = "source", skip_serializing_if = "Option::is_none")]
    pub dataset: Option<String>,
    /// One thing, by its identifier: `cve:CVE-2021-44228`, or the value alone where the tracker
    /// has one scheme. Every signal about it is delivered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thing: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub query: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deliver: Vec<Deliver>,
    /// Words, any of which a thing's title must hold for what happens to it to be told: a
    /// query asks what a thing is, and a title is what it says. Case does not count.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Deliver {
    /// `feed`, `webhook`, `mail` or `command`.
    pub to: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// For `mail`: where to, sent through the workspace's own `mail:`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// Argv. Zetlyn holds no mail credentials: an operator names their own mailer here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub run: Vec<String>,
}

pub struct Watch {
    pub decl: WatchDecl,
    pub path: PathBuf,
}

#[derive(Default)]
pub struct State {
    pub mark: String,
    pub delivered: Vec<J>,
    /// The things a watch on a view held at its last delivery, to say who entered and who left.
    pub members: Vec<String>,
    /// Whether there was a last delivery. A first one has nothing to have entered from.
    pub known: bool,
}

const KEEP: usize = 200;

impl Watch {
    pub fn load(path: &Path) -> Result<Watch, String> {
        let decl: WatchDecl = crate::yaml::read(path)?;
        if decl.scope.is_none() && decl.dataset.is_none() {
            return Err(format!("{}: name a tracker or a source", path.display()));
        }
        Ok(Watch {
            decl,
            path: path.to_path_buf(),
        })
    }

    fn state_path(&self) -> PathBuf {
        self.path.with_extension("state.json")
    }

    pub fn state(&self) -> State {
        let Ok(text) = std::fs::read_to_string(self.state_path()) else {
            return State::default();
        };
        let Ok(j) = serde_json::from_str::<J>(&text) else {
            return State::default();
        };
        State {
            mark: j["mark"].as_str().unwrap_or("").to_string(),
            delivered: j["delivered"].as_array().cloned().unwrap_or_default(),
            members: j["members"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
            known: j.get("members").is_some(),
        }
    }

    fn write_state(&self, state: &State) -> Result<(), String> {
        let kept: Vec<J> = state
            .delivered
            .iter()
            .rev()
            .take(KEEP)
            .rev()
            .cloned()
            .collect();
        let body = json!({ "mark": state.mark, "delivered": kept, "members": state.members });
        std::fs::write(
            self.state_path(),
            serde_json::to_string_pretty(&body).unwrap_or_default(),
        )
        .map_err(|e| format!("{}: {e}", self.state_path().display()))
    }

    fn pred(&self) -> Option<Pred> {
        expr::parse_pred(&self.decl.query)
    }

    /// Everything that moved since the mark, narrowed to the thing or the view the watch is on.
    pub fn check(&self, root: &Path) -> Result<(J, String), String> {
        let state = self.state();
        if let Some(name) = &self.decl.scope {
            let dir = crate::tracker::scope_registry(&root.join("trackers"))
                .get(name)
                .cloned()
                .ok_or_else(|| format!("{name} is not installed here"))?;
            let tracker = Tracker::open(&dir, &root.join("sources"))?;
            // What changed since the tracker last looked is a signal before it is asked about.
            tracker.refresh_if_moved()?;
            let store = crate::thingstore::ThingStore::open(&dir)?;
            let since: i64 = state.mark.parse().unwrap_or(0);
            let now_mark = store.last_signal();
            let fresh: Vec<J> = store
                .signals(Some(since), 100_000)
                .into_iter()
                .rev()
                .collect();

            let (kept, members) = if let Some(thing) = &self.decl.thing {
                let key = thing_key(thing, tracker.decl.keys().first().copied().unwrap_or("id"));
                let kept: Vec<J> = fresh.into_iter().filter(|s| s["key"].as_str() == Some(&key)).collect();
                (kept, Vec::new())
            } else if !self.decl.query.trim().is_empty() {
                let cx = tracker.context();
                let q = crate::thingquery::parse(&self.decl.query, &cx)?;
                let members = store.matching(&q, &cx)?;
                let now: BTreeSet<&String> = members.iter().collect();
                let before: BTreeSet<&String> = state.members.iter().collect();
                let mut kept: Vec<J> = Vec::new();
                // Who entered and who left, from the second check on: the first has nothing to
                // have entered from, and a first delivery of every member would say nothing.
                if state.known {
                    for key in now.difference(&before) {
                        kept.push(membership(&store, key, "entered"));
                    }
                    for key in before.difference(&now) {
                        kept.push(membership(&store, key, "left"));
                    }
                }
                kept.extend(fresh.into_iter().filter(|s| {
                    s["key"].as_str().is_some_and(|k| now.contains(&k.to_string()) || before.contains(&k.to_string()))
                }));
                (kept, members)
            } else {
                (fresh, Vec::new())
            };
            let words: Vec<String> = self.decl.words.iter().map(|w| w.to_lowercase()).filter(|w| !w.is_empty()).collect();
            let kept: Vec<J> = if words.is_empty() {
                kept
            } else {
                kept.into_iter()
                    .filter(|s| {
                        let title = s["title"].as_str().unwrap_or("").to_lowercase();
                        words.iter().any(|w| title.contains(w.as_str()))
                    })
                    .collect()
            };
            return Ok((
                json!({ "watch": self.decl.name, "tracker": name, "since": since,
                        "signals": J::Array(kept), "members": members }),
                now_mark.to_string(),
            ));
        }

        let name = self.decl.dataset.clone().unwrap_or_default();
        let dir = crate::tracker::registry(&root.join("sources"))
            .get(&name)
            .cloned()
            .ok_or_else(|| format!("{name} is not installed here"))?;
        let ds = Source::open(&dir)?;
        let since: i64 = state.mark.parse().unwrap_or((ds.mark() - 1).max(0));
        let report = Interface::changes(&ds, since, 500);
        // Narrowed by the query as a tracker's watch is. Without this a watch over a source
        // delivered every change and its query was decoration.
        let pred = self.pred();
        let empty = Vec::new();
        let kept: Vec<J> = report["changed"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter(|c| match &pred {
                Some(p) => ds.holds(c["claim_id"].as_str().unwrap_or(""), p),
                None => true,
            })
            .cloned()
            .collect();
        Ok((
            json!({ "watch": self.decl.name, "source": name, "since": since,
                    "things": J::Array(kept) }),
            ds.mark().to_string(),
        ))
    }

    /// Delivered first, and the mark advanced only after. A webhook that was not reached is one a
    /// subscriber has not been told about, and the next check tells them.
    pub fn deliver(&self, report: &J, mark: &str) -> Result<Vec<String>, String> {
        let empty = Vec::new();
        // A tracker's watch reports signals, a source's the claims that moved.
        let entries = report["signals"]
            .as_array()
            .or_else(|| report["things"].as_array())
            .unwrap_or(&empty);
        let mut done = Vec::new();
        // Nothing to say is still a look taken: the mark moves, and a view's members are kept so
        // the next look can say who entered and who left.
        if entries.is_empty() {
            self.remember(report, mark, entries)?;
            return Ok(done);
        }
        for d in &self.decl.deliver {
            match d.to.as_str() {
                "feed" => done.push("feed".into()),
                "webhook" => {
                    let url = d.url.as_deref().ok_or("a webhook needs a url")?;
                    let url = crate::fetch::resolve(url)?.unwrap_or_default();
                    let agent = ureq::Agent::config_builder()
                        .user_agent(crate::sourcedecl::AGENT)
                        .timeout_global(Some(std::time::Duration::from_secs(20)))
                        .build()
                        .new_agent();
                    agent
                        .post(&url)
                        .header("content-type", "application/json")
                        .send(report.to_string())
                        .map_err(|e| format!("{url}: {e}"))?;
                    done.push(format!("webhook {url}"));
                }
                "command" => {
                    // A hosted world runs no programs on the server; a webhook reaches yours.
                    if crate::usage::in_cell() {
                        return Err("a hosted world delivers by mail, feed or webhook, not by running a command".into());
                    }
                    let argv: Vec<String> = d
                        .run
                        .iter()
                        .map(|a| crate::fetch::resolve(a).map(|v| v.unwrap_or_default()))
                        .collect::<Result<_, _>>()?;
                    let Some((program, rest)) = argv.split_first() else {
                        return Err("a command needs something to run".into());
                    };
                    use std::io::Write;
                    let mut child = std::process::Command::new(program)
                        .args(rest)
                        .stdin(std::process::Stdio::piped())
                        .spawn()
                        .map_err(|e| format!("{program}: {e}"))?;
                    if let Some(stdin) = child.stdin.as_mut() {
                        stdin
                            .write_all(report.to_string().as_bytes())
                            .map_err(|e| format!("{program}: {e}"))?;
                    }
                    let status = child.wait().map_err(|e| format!("{program}: {e}"))?;
                    if !status.success() {
                        return Err(format!("{program}: exited {status}"));
                    }
                    done.push(format!("command {program}"));
                }
                "mail" => {
                    let to = d.address.as_deref().ok_or("mail needs an address")?;
                    let to = crate::fetch::resolve(to)?.unwrap_or_default();
                    let root = self.path.parent().and_then(|p| p.parent()).unwrap_or(std::path::Path::new("."));
                    let site = crate::account::Site::load(root);
                    let title = if self.decl.title.is_empty() { self.decl.name.clone() } else { self.decl.title.clone() };
                    let lines: Vec<String> = entries
                        .iter()
                        .map(|e| if e["kind"].is_string() { crate::thingstore::say(e) } else { e["title"].as_str().unwrap_or("").to_string() })
                        .collect();
                    let subject = format!("{title}: {} new", entries.len());
                    let body = format!("{}\n\n— the watch {}, in {}", lines.join("\n"), self.decl.name, root.display());
                    // Not sent is not delivered: the mark stays, and the next check says it again.
                    if !site.send(&to, &subject, &body)? {
                        return Err("no mailer: workspace.yaml names no mail: smtp: or run:".into());
                    }
                    done.push(format!("mail {to}"));
                }
                other => return Err(format!("no delivery called {other}")),
            }
        }
        self.remember(report, mark, entries)?;
        Ok(done)
    }

    pub fn remember(&self, report: &J, mark: &str, entries: &[J]) -> Result<(), String> {
        let mut state = self.state();
        state.mark = mark.to_string();
        state.delivered.extend(entries.iter().cloned());
        if let Some(m) = report["members"].as_array() {
            state.members = m.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
        }
        self.write_state(&state)?;
        Ok(())
    }
}

/// Every watch in a workspace. One that does not read is said on stderr rather than dropped: a
/// watch that silently stopped is a subscriber who is never told, and nobody finds out.
pub fn all(root: &Path) -> Vec<Watch> {
    let dir = root.join("watches");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for p in entries.flatten().map(|e| e.path()) {
        match p.extension().and_then(|e| e.to_str()) {
            Some("yaml") => match Watch::load(&p) {
                Ok(w) => out.push(w),
                Err(e) => eprintln!("zetlyn: {e}"),
            },
            Some("toml") => eprintln!(
                "zetlyn: {} is from before 0.2 and is not read. `zetlyn migrate` rewrites it",
                p.display()
            ),
            _ => {}
        }
    }
    out.sort_by(|a, b| a.decl.name.cmp(&b.decl.name));
    out
}

/// `cve:CVE-2021-44228`, or `CVE-2021-44228` alone, as the tracker store keys a thing.
fn thing_key(named: &str, scheme: &str) -> String {
    match named.split_once(':') {
        Some((scheme, value)) if !scheme.is_empty() && !scheme.contains('-') => {
            crate::schemes::key(scheme, value)
        }
        _ => crate::schemes::key(scheme, named),
    }
}

/// A thing entering or leaving a view, as a signal is written.
fn membership(store: &crate::thingstore::ThingStore, key: &str, kind: &str) -> J {
    let (title, scheme, value) = store
        .named(key)
        .unwrap_or_else(|| (key.to_string(), String::new(), String::new()));
    json!({ "kind": kind, "key": key, "title": title, "scheme": scheme, "value": value,
            "at": crate::iso_stamp(crate::now()) })
}
