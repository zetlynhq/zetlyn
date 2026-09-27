//! Zetlyn run for somebody instead of by them.
//!
//! The rule the whole file is written against: **this holds no dataset and no account.** What it
//! holds is a grant per deployment, and a grant is a statement somebody else signed. Every fact
//! it shows came from a console call it made a moment ago, and nothing it shows is stored.
//!
//! That is what keeps a managed deployment from contradicting local first. A customer's records
//! live on their machine and leave with it. Take this program away and they still have their
//! deployment; take their deployment away and this has an address that stops answering.
//!
//! There are no accounts here because there is nothing to have an account for. Whoever holds the
//! private half of the key a grant names can drive that deployment, and whoever does not, cannot.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use maud::{html, Markup, PreEscaped, DOCTYPE};
use serde::Deserialize;
use serde_json::Value as J;

use crate::console::Driver;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Held {
    /// Where its console answers.
    pub at: String,
    /// The grant file, under `grants/`. Kept apart from this one because both are TOML and a
    /// directory that holds two kinds of TOML is a directory somebody parses the wrong one from.
    pub grant: String,
    #[serde(default)]
    pub title: String,
}

/// What a platform is: a directory of grants.
pub struct Platform {
    pub root: PathBuf,
    pub held: BTreeMap<String, Held>,
}

impl Platform {
    pub fn open(root: &Path) -> Result<Platform, String> {
        let dir = root.join("deployments");
        let mut held = BTreeMap::new();
        let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e != "toml").unwrap_or(true) {
                continue;
            }
            let Some(name) = path.file_stem().map(|s| s.to_string_lossy().to_string()) else {
                continue;
            };
            let raw =
                std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let one: Held = toml::from_str(&raw).map_err(|e| format!("{}: {e}", path.display()))?;
            held.insert(name, one);
        }
        Ok(Platform {
            root: root.to_path_buf(),
            held,
        })
    }

    pub fn driver(&self, name: &str) -> Result<Driver, String> {
        let one = self
            .held
            .get(name)
            .ok_or_else(|| format!("{name}: no grant for that here"))?;
        let path = self.root.join("grants").join(&one.grant);
        let grant = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Driver::new(&one.at, grant))
    }
}

// -- the pages ----------------------------------------------------------------------------------

pub fn serve(root: &Path, addr: &str) -> Result<(), String> {
    let server = tiny_http::Server::http(addr).map_err(|e| e.to_string())?;
    let who = crate::identity::key().ok_or(
        "no identity. A platform drives consoles by signing, and `zetlyn id new` makes the key",
    )?;
    println!("a platform on http://{addr}");
    println!("it signs as {who}");

    for mut request in server.incoming_requests() {
        let url = request.url().to_string();
        let path = url.split('?').next().unwrap_or("/").to_string();
        let post = request.method().as_str() == "POST";
        let mut sent = Vec::new();
        let _ = std::io::Read::read_to_end(request.as_reader(), &mut sent);
        let body_of = String::from_utf8_lossy(&sent).to_string();
        // Re-read every time: a grant added a minute ago is drivable now.
        let platform = Platform::open(root);
        let parts: Vec<&str> = path.trim_matches('/').split('/').collect();

        let (status, body) = match (&platform, post, parts.as_slice()) {
            (Err(e), _, _) => (500, page("Nothing here", html! { h1 { (e) } })),
            (Ok(p), false, [""]) | (Ok(p), false, []) => (200, overview(p)),
            (Ok(p), false, ["d", name]) => deployment(p, name),
            (Ok(p), false, ["d", name, "dataset", ds]) => dataset(p, name, ds),
            (Ok(p), true, ["d", name, "dataset", ds, "run"]) => run(p, name, ds),
            (Ok(p), false, ["d", name, "dataset", ds, "declaration"]) => {
                declaration(p, name, ds, None, None)
            }
            (Ok(p), true, ["d", name, "dataset", ds, "draft"]) => ask_for_draft(p, name, ds),
            (Ok(p), true, ["d", name, "dataset", ds, "declaration"]) => {
                apply(p, name, ds, &body_of)
            }
            _ => (
                404,
                page(
                    "Nothing here",
                    html! { p { a href="/" { "← every deployment" } } h1 { "Nothing at that address" } },
                ),
            ),
        };
        let kind =
            tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
                .map_err(|_| "bad header".to_string())?;
        let response = tiny_http::Response::from_string(body)
            .with_status_code(status)
            .with_header(kind);
        let _ = request.respond(response);
    }
    Ok(())
}

fn page(title: &str, body: Markup) -> String {
    let markup = html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) }
                style { (PreEscaped(crate::serve::STYLE)) }
            }
            body { main { (body) } }
        }
    };
    markup.into_string()
}

fn overview(p: &Platform) -> String {
    // Every number here is asked for now. Nothing about a deployment is kept between requests,
    // which is the same rule said in the shape of the code.
    let asked: Vec<(String, String, Result<J, String>)> = p
        .held
        .iter()
        .map(|(name, one)| {
            let answer = p.driver(name).and_then(|d| d.ask("GET", "/", b""));
            (name.clone(), one.title.clone(), answer)
        })
        .collect();

    page(
        "Deployments",
        html! {
            h1 { "Deployments" }
            p.about {
                "Every one this platform holds a grant for. It holds no records and no accounts: "
                "what is below was asked for just now, and nothing of it is kept."
            }
            table {
                thead { tr { th { "Deployment" } th { "Datasets" } th { "Scopes" } th { "State" } } }
                tbody {
                    @for (name, title, answer) in &asked {
                        tr {
                            td {
                                a href={"/d/" (name)} { (if title.is_empty() { name } else { title }) }
                                div.why { (p.held[name].at) }
                            }
                            @match answer {
                                Ok(j) => {
                                    td.num { (j["datasets"].as_array().map(Vec::len).unwrap_or(0)) }
                                    td.num { (j["scopes"].as_array().map(Vec::len).unwrap_or(0)) }
                                    td { (summary(j)) }
                                }
                                Err(e) => {
                                    td.dim { "—" } td.dim { "—" }
                                    td { span.state.failing { "not answering" } div.why { (e) } }
                                }
                            }
                        }
                    }
                }
            }
            @if asked.is_empty() {
                p.dim { "No grants here yet. `zetlyn console grant` on a deployment writes one." }
            }
        },
    )
}

/// What is wrong with a deployment, in as few words as a table cell holds.
fn summary(j: &J) -> Markup {
    let empty = Vec::new();
    let datasets = j["datasets"].as_array().unwrap_or(&empty);
    let scopes = j["scopes"].as_array().unwrap_or(&empty);
    let unwell: Vec<&str> = datasets
        .iter()
        .filter_map(|d| d["state"].as_str())
        .filter(|s| *s != "current")
        .collect();
    let behind = scopes
        .iter()
        .filter(|s| !s["promise_holds"].as_bool().unwrap_or(true))
        .count();
    html! {
        @if unwell.is_empty() && behind == 0 {
            span.state.current { "current" }
        } @else {
            span.state.partial { (unwell.len()) " not current" }
            @if behind > 0 { " · " (behind) " promise" }
        }
    }
}

fn deployment(p: &Platform, name: &str) -> (u16, String) {
    let held = match p.driver(name).and_then(|d| d.ask("GET", "/", b"")) {
        Ok(j) => j,
        Err(e) => {
            return (
                502,
                page(
                    name,
                    html! {
                        p { a href="/" { "← every deployment" } }
                        h1 { (name) }
                        div.note { "It did not answer. " (e) }
                    },
                ),
            )
        }
    };
    let empty = Vec::new();
    (
        200,
        page(
            name,
            html! {
                p { a href="/" { "← every deployment" } }
                h1 { (held["deployment"].as_str().unwrap_or(name)) }
                p.state { "zetlyn " (held["zetlyn"].as_str().unwrap_or("?")) " · " (p.held[name].at) }

                h2 { "Datasets" }
                table {
                    thead { tr { th { "Dataset" } th { "Kind" } th { "Records" } th { "State" } th { "Due" } } }
                    tbody {
                        @for d in held["datasets"].as_array().unwrap_or(&empty) {
                            @let at = d["at"].as_str().unwrap_or("");
                            tr {
                                td { a href={"/d/" (name) "/dataset/" (at)} { (d["dataset"].as_str().unwrap_or("")) } }
                                td.dim { (d["kind"].as_str().unwrap_or("")) }
                                td.num { (d["records"].as_u64().unwrap_or(0)) }
                                @let state = d["state"].as_str().unwrap_or("empty");
                                td { span.state.(state) { (state) } }
                                td.dim { (d["next_run"].as_str().unwrap_or("—")) }
                            }
                        }
                    }
                }

                h2 { "Scopes" }
                table {
                    thead { tr { th { "Scope" } th { "Members" } th { "Records" } th { "Promise" } } }
                    tbody {
                        @for s in held["scopes"].as_array().unwrap_or(&empty) {
                            tr {
                                td { (s["scope"].as_str().unwrap_or("")) }
                                td.num { (s["members"].as_u64().unwrap_or(0)) }
                                td.num { (s["records"].as_u64().unwrap_or(0)) }
                                @if s["promise_holds"].as_bool().unwrap_or(true) {
                                    td { span.state.current { "holds" } }
                                } @else {
                                    td {
                                        span.state.partial { "behind" }
                                        @for late in s["behind"].as_array().unwrap_or(&empty) {
                                            div.why { (late.as_str().unwrap_or("")) }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            },
        ),
    )
}

fn dataset(p: &Platform, name: &str, at: &str) -> (u16, String) {
    let driver = match p.driver(name) {
        Ok(d) => d,
        Err(e) => return (404, page(name, html! { h1 { (e) } })),
    };
    let described = driver.ask("GET", &format!("/dataset/{at}"), b"");
    let runs = driver.ask("GET", &format!("/dataset/{at}/runs"), b"");
    let (described, runs) = match (described, runs) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            return (
                502,
                page(
                    at,
                    html! {
                        p { a href={"/d/" (name)} { "← " (name) } }
                        h1 { (at) }
                        div.note { (e) }
                    },
                ),
            )
        }
    };
    let empty = Vec::new();
    (
        200,
        page(
            described["dataset"].as_str().unwrap_or(at),
            html! {
                p { a href={"/d/" (name)} { "← " (name) } }
                h1 { (described["dataset"].as_str().unwrap_or(at)) }
                @if let Some(about) = described["about"].as_str() { p.about { (about) } }
                p.state {
                    (described["records"].as_u64().unwrap_or(0)) " records · "
                    (described["kind"].as_str().unwrap_or("")) " · "
                    @let state = described["state"].as_str().unwrap_or("empty");
                    span.state.(state) { (state) }
                }

                form.bar method="post" action={"/d/" (name) "/dataset/" (at) "/run"} {
                    button type="submit" { "Fill it now" }
                }

                h2 { "Runs" }
                table {
                    thead { tr { th { "Run" } th { "Started" } th { "Added" } th { "Changed" } th { "Removed" } th { "Complete" } } }
                    tbody {
                        @for r in runs["runs"].as_array().unwrap_or(&empty) {
                            tr {
                                td.num { (r["id"].as_i64().unwrap_or(0)) }
                                td.dim { (r["started"].as_str().unwrap_or("")) }
                                td.num { (r["added"].as_u64().unwrap_or(0)) }
                                td.num { (r["changed"].as_u64().unwrap_or(0)) }
                                td.num { (r["removed"].as_u64().unwrap_or(0)) }
                                td {
                                    @if r["complete"].as_bool().unwrap_or(false) {
                                        span.state.current { "yes" }
                                    } @else {
                                        span.state.partial { "partial" }
                                    }
                                    @if let Some(why) = r["error"].as_str() { div.why { (why) } }
                                    @if let Some(why) = r["refused"].as_str() { div.why { "refused: " (why) } }
                                }
                            }
                        }
                    }
                }
            },
        ),
    )
}

fn run(p: &Platform, name: &str, at: &str) -> (u16, String) {
    let answer = p
        .driver(name)
        .and_then(|d| d.ask("POST", &format!("/dataset/{at}/run"), b""));
    match answer {
        Ok(j) => (
            200,
            page(
                "Filled",
                html! {
                    p { a href={"/d/" (name) "/dataset/" (at)} { "← " (at) } }
                    h1 { "Run " (j["run"].as_i64().unwrap_or(0)) }
                    p.state {
                        "+" (j["added"].as_u64().unwrap_or(0))
                        " ~" (j["changed"].as_u64().unwrap_or(0))
                        " −" (j["removed"].as_u64().unwrap_or(0))
                        " =" (j["unchanged"].as_u64().unwrap_or(0))
                    }
                    @if let Some(why) = j["error"].as_str() { div.note { (why) } }
                },
            ),
        ),
        Err(e) => (
            502,
            page(
                "Not filled",
                html! {
                    p { a href={"/d/" (name) "/dataset/" (at)} { "← " (at) } }
                    h1 { "It did not run" }
                    div.note { (e) }
                },
            ),
        ),
    }
}

// ---------------------------------------------------------------------------------------------
// Changing what a dataset is.

/// What the platform may ask to draft a declaration, where an operator names one.
///
/// Zetlyn ships no model and holds no key for one, the same way it ships no mail client. An
/// operator names the thing that already knows how to reach one, the run's own complaints go in
/// on its standard input, and a declaration comes out.
///
/// It drafts and it does not decide. What comes back is held against the same three checks an
/// `apply` is held against, and then a person reads it and presses a button. That is the whole of
/// the difference between this and the thing `CONCEPT.md` refuses: a source's shape is not
/// guessable from one response, so what is guessed here is a proposal and never a fact.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Drafting {
    #[serde(default)]
    pub run: Vec<String>,
}

pub fn drafter(root: &Path) -> Drafting {
    #[derive(Deserialize)]
    struct Config {
        #[serde(default)]
        draft: Drafting,
    }
    std::fs::read_to_string(root.join("platform.toml"))
        .ok()
        .and_then(|raw| toml::from_str::<Config>(&raw).ok())
        .map(|c| c.draft)
        .unwrap_or_default()
}

/// The complaints, the declaration and the fields, handed over as one document.
fn brief(described: &J, runs: &J, declaration: &str) -> String {
    let empty = Vec::new();
    let recent: Vec<&J> = runs["runs"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .take(3)
        .collect();
    let mut out = String::from(
        "A dataset declaration, and what its last runs could not make sense of.\n\
         Answer with a declaration and nothing else.\n\n",
    );
    out.push_str("## What the runs said\n\n");
    for r in recent {
        out.push_str(&format!(
            "run {} {}: +{} ~{} -{} ={}, unparsed {}, no_text {}, no_known {}, duplicates {}\n",
            r["id"].as_i64().unwrap_or(0),
            r["started"].as_str().unwrap_or(""),
            r["added"].as_u64().unwrap_or(0),
            r["changed"].as_u64().unwrap_or(0),
            r["removed"].as_u64().unwrap_or(0),
            r["unchanged"].as_u64().unwrap_or(0),
            r["unparsed"].as_u64().unwrap_or(0),
            r["no_text"].as_u64().unwrap_or(0),
            r["no_known"].as_u64().unwrap_or(0),
            r["duplicates"].as_u64().unwrap_or(0),
        ));
        for key in ["note", "error", "refused"] {
            if let Some(said) = r[key].as_str() {
                out.push_str(&format!("  {key}: {said}\n"));
            }
        }
    }
    out.push_str("\n## What it holds\n\n");
    for f in described["fields"].as_array().unwrap_or(&empty) {
        out.push_str(&format!(
            "{} ({}) in {} records\n",
            f["name"].as_str().unwrap_or(""),
            f["type"].as_str().unwrap_or(""),
            f["records"].as_u64().unwrap_or(0)
        ));
    }
    out.push_str("\n## The declaration\n\n");
    out.push_str(declaration);
    out
}

/// Run what the operator named, with the brief on its standard input.
fn draft(root: &Path, brief: &str) -> Result<String, String> {
    let named = drafter(root).run;
    let (first, rest) = named
        .split_first()
        .ok_or("no drafter named. `[draft] run = [...]` in platform.toml names one")?;
    let mut child = std::process::Command::new(first)
        .args(rest)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("{first}: {e}"))?;
    if let Some(mut input) = child.stdin.take() {
        use std::io::Write;
        input
            .write_all(brief.as_bytes())
            .map_err(|e| format!("{first}: {e}"))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("{first}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{first}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    if text.trim().is_empty() {
        return Err(format!("{first} answered with nothing"));
    }
    Ok(text)
}

/// The declaration, what a drafter proposes, and the button a person presses.
fn declaration(
    p: &Platform,
    name: &str,
    at: &str,
    proposed: Option<&str>,
    said: Option<&str>,
) -> (u16, String) {
    let driver = match p.driver(name) {
        Ok(d) => d,
        Err(e) => return (404, page(name, html! { h1 { (e) } })),
    };
    let held = match driver.ask("GET", &format!("/dataset/{at}/declaration"), b"") {
        Ok(j) => j["declaration"].as_str().unwrap_or("").to_string(),
        Err(e) => return (502, page(at, html! { h1 { (at) } div.note { (e) } })),
    };
    let drafts = !drafter(&p.root).run.is_empty();
    (
        200,
        page(
            at,
            html! {
                p { a href={"/d/" (name) "/dataset/" (at)} { "← " (at) } }
                h1 { "What " (at) " is" }
                @if let Some(m) = said { div.note { (m) } }

                @if let Some(text) = proposed {
                    h2 { "Proposed" }
                    p.about {
                        "Drafted from what the runs could not make sense of. Nothing has changed "
                        "yet: read it, and press the button if it is right. Whoever presses it is "
                        "who answers for the declaration afterwards."
                    }
                    form method="post" action={"/d/" (name) "/dataset/" (at) "/declaration"} {
                        textarea name="declaration" rows="28" style="width:100%" { (text) }
                        p.bar { button type="submit" { "Apply it" } }
                    }
                    h2 { "What is there now" }
                    pre { (held) }
                } @else {
                    form method="post" action={"/d/" (name) "/dataset/" (at) "/declaration"} {
                        textarea name="declaration" rows="28" style="width:100%" { (held) }
                        p.bar { button type="submit" { "Apply it" } }
                    }
                    @if drafts {
                        form.bar method="post" action={"/d/" (name) "/dataset/" (at) "/draft"} {
                            button type="submit" { "Ask for a draft" }
                        }
                        p.dim {
                            "The drafter is handed the last three runs and what the dataset holds, "
                            "and answers with a declaration. It proposes; you apply."
                        }
                    } @else {
                        p.dim { "No drafter named. `[draft] run = [...]` in platform.toml names one." }
                    }
                }
            },
        ),
    )
}

fn ask_for_draft(p: &Platform, name: &str, at: &str) -> (u16, String) {
    let driver = match p.driver(name) {
        Ok(d) => d,
        Err(e) => return (404, page(name, html! { h1 { (e) } })),
    };
    let described = driver.ask("GET", &format!("/dataset/{at}"), b"");
    let runs = driver.ask("GET", &format!("/dataset/{at}/runs"), b"");
    let held = driver.ask("GET", &format!("/dataset/{at}/declaration"), b"");
    let (described, runs, held) = match (described, runs, held) {
        (Ok(a), Ok(b), Ok(c)) => (a, b, c),
        (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
            return (502, page(at, html! { h1 { (at) } div.note { (e) } }))
        }
    };
    let text = held["declaration"].as_str().unwrap_or("");
    match draft(&p.root, &brief(&described, &runs, text)) {
        Ok(proposed) => declaration(p, name, at, Some(&proposed), None),
        Err(e) => declaration(p, name, at, None, Some(&format!("No draft: {e}"))),
    }
}

fn apply(p: &Platform, name: &str, at: &str, body: &str) -> (u16, String) {
    let text = crate::servescope::form_field(body, "declaration");
    if text.trim().is_empty() {
        return declaration(p, name, at, None, Some("Nothing was sent."));
    }
    let driver = match p.driver(name) {
        Ok(d) => d,
        Err(e) => return (404, page(name, html! { h1 { (e) } })),
    };
    match driver.ask(
        "PUT",
        &format!("/dataset/{at}/declaration"),
        text.as_bytes(),
    ) {
        Ok(j) if j["applied"].as_bool().unwrap_or(false) => (
            200,
            page(
                "Applied",
                html! {
                    p { a href={"/d/" (name) "/dataset/" (at)} { "← " (at) } }
                    h1 { (at) " is now what you sent" }
                    p.dim { "The one that was there is kept beside it as " (j["kept"].as_str().unwrap_or("")) "." }
                },
            ),
        ),
        Ok(j) => declaration(
            p,
            name,
            at,
            Some(&text),
            Some(&format!(
                "Not applied: {}",
                j["why"].as_str().unwrap_or("it is what is there")
            )),
        ),
        Err(e) => declaration(p, name, at, Some(&text), Some(&format!("Not applied: {e}"))),
    }
}
