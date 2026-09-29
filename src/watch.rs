//! A saved query is a subscription. The scope replays it at each run and says what entered, what
//! left, and what changed inside an entry.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};

use crate::dataset::{Dataset, Member};
use crate::expr::{self, Pred};
use crate::scope::Scope;

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
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub query: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deliver: Vec<Deliver>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Deliver {
    /// `feed`, `webhook` or `command`.
    pub to: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
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
        let body = json!({ "mark": state.mark, "delivered": kept });
        std::fs::write(
            self.state_path(),
            serde_json::to_string_pretty(&body).unwrap_or_default(),
        )
        .map_err(|e| format!("{}: {e}", self.state_path().display()))
    }

    fn pred(&self) -> Option<Pred> {
        expr::parse_pred(&self.decl.query)
    }

    /// Everything that moved since the mark, narrowed to what the saved query selects.
    pub fn check(&self, root: &Path) -> Result<(J, String), String> {
        let state = self.state();
        if let Some(name) = &self.decl.scope {
            let dir = crate::scope::scope_registry(&root.join("trackers"))
                .get(name)
                .cloned()
                .ok_or_else(|| format!("{name} is not installed here"))?;
            let scope = Scope::open(&dir, &root.join("sources"))?;
            let since = if state.mark.is_empty() {
                scope.mark_before()
            } else {
                state.mark.clone()
            };
            let report = scope.changes(&since, 500);
            let mark = report["mark"].as_str().unwrap_or("").to_string();
            let pred = self.pred();
            let empty = Vec::new();
            let mut kept = Vec::new();
            for entry in report["things"].as_array().unwrap_or(&empty) {
                if let Some(p) = &pred {
                    let Some(key) = entry["identifier"].as_object() else {
                        continue;
                    };
                    let (scheme, value) = (
                        key["scheme"].as_str().unwrap_or(""),
                        key["value"].as_str().unwrap_or(""),
                    );
                    let Some(full) = scope.entry(scheme, value) else {
                        continue;
                    };
                    // The scope's own reading of the query, on its scale. A watch that compared
                    // the words as strings put `critical` below `high` and `urgent` above it.
                    if !scope.entry_holds(&full, p) {
                        continue;
                    }
                }
                kept.push(entry.clone());
            }
            return Ok((
                json!({ "watch": self.decl.name, "tracker": name, "since": since,
                        "things": J::Array(kept) }),
                mark,
            ));
        }

        let name = self.decl.dataset.clone().unwrap_or_default();
        let dir = crate::scope::registry(&root.join("sources"))
            .get(&name)
            .cloned()
            .ok_or_else(|| format!("{name} is not installed here"))?;
        let ds = Dataset::open(&dir)?;
        let since: i64 = state.mark.parse().unwrap_or((ds.mark() - 1).max(0));
        let report = Member::changes(&ds, since, 500);
        // Narrowed by the query as a scope's watch is. Without this a watch over a dataset
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
        let entries = report["things"].as_array().unwrap_or(&empty);
        let mut done = Vec::new();
        if entries.is_empty() {
            return Ok(done);
        }
        for d in &self.decl.deliver {
            match d.to.as_str() {
                "feed" => done.push("feed".into()),
                "webhook" => {
                    let url = d.url.as_deref().ok_or("a webhook needs a url")?;
                    let url = crate::fetch::resolve(url)?.unwrap_or_default();
                    let agent = ureq::Agent::config_builder()
                        .user_agent(crate::decl::AGENT)
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
                other => return Err(format!("no delivery called {other}")),
            }
        }
        let mut state = self.state();
        state.mark = mark.to_string();
        state.delivered.extend(entries.iter().cloned());
        self.write_state(&state)?;
        Ok(done)
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
