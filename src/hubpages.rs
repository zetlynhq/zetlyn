//! A hub's pages: its catalog, and a page for each source, tracker and package on it.
//!
//! A hub serves files, and these are files too. `zetlyn hub render` writes them into the hub
//! itself, a folder or a bucket, next to what they describe, so a hub that is nothing but storage
//! behind a web server still has pages a person can read. `zetlyn hub serve` renders the same
//! pages as they are asked for. Either way they are made from the manifests and nothing else: the
//! hub holds no index and answers no query, and neither do its pages.
//!
//! The design is not in this binary. The pages name `/style.css`, `/app.js`, `/mark.png` and
//! `/favicon.png`, which the hub keeps like anything else; whoever runs a hub puts theirs there.

use std::collections::BTreeMap;

use maud::{html, Markup, PreEscaped, DOCTYPE};
use serde_json::Value as J;

use crate::place::Place;

/// One thing a hub carries under one tag: a source, a tracker, or a package.
pub struct Row {
    pub tree: &'static str,
    pub owner: String,
    pub name: String,
    pub tag: String,
    pub version: String,
    pub manifest: J,
}

impl Row {
    pub fn reference(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
    /// Where its page is.
    pub fn page(&self) -> String {
        format!("/{}/{}/{}/", self.tree, self.owner, self.name)
    }
    fn s(&self, k: &str) -> String {
        self.manifest[k].as_str().unwrap_or_default().to_string()
    }
    pub fn title(&self) -> String {
        let t = self.s("title");
        if t.is_empty() { self.reference() } else { t }
    }
    pub fn about(&self) -> String {
        self.s("about")
    }
    pub fn built_at(&self) -> i64 {
        self.manifest["built_at"].as_i64().unwrap_or(0)
    }
    /// A tracker names its sources; a package lists them with what each holds.
    pub fn includes(&self) -> Vec<String> {
        self.manifest["sources"]
            .as_array()
            .map(|a| a.iter().filter_map(|m| m.as_str().or_else(|| m["name"].as_str()).map(str::to_string)).collect())
            .unwrap_or_default()
    }
    pub fn claims(&self) -> u64 {
        self.manifest["claims"].as_u64().unwrap_or_else(|| {
            self.manifest["sources"].as_array().map(|a| a.iter().filter_map(|m| m["claims"].as_u64()).sum()).unwrap_or(0)
        })
    }
    pub fn sealed(&self) -> bool {
        self.manifest["sealed"].as_bool().unwrap_or(false)
    }
    fn statement(&self) -> Option<crate::trackerdecl::TrackerDecl> {
        crate::yaml::parse(self.manifest["declaration"].as_str()?).ok()
    }
    pub fn private(&self) -> bool {
        self.statement().is_some_and(|d| d.visibility == "private")
    }
    pub fn republish(&self) -> String {
        self.manifest["licence"]["republish"].as_str().unwrap_or_default().to_string()
    }
    pub fn is_tracker(&self) -> bool {
        self.tree != "sources"
    }
}

/// Everything a place holds under `sources/`, `trackers/` and `packages/`, one row per tag.
pub fn shelf(place: &dyn Place) -> Result<Vec<Row>, String> {
    let mut rows = Vec::new();
    for tree in ["sources", "trackers", "packages"] {
        let listed = match place.list(&format!("{tree}/")) {
            Ok(l) => l,
            Err(e) if tree == "sources" => return Err(e),
            Err(_) => continue,
        };
        for path in listed {
            let parts: Vec<&str> = path.split('/').collect();
            let [_, owner, name, "tags", tag] = parts[..] else { continue };
            let Ok(v) = place.get(&path) else { continue };
            let version = String::from_utf8_lossy(&v).trim().to_string();
            let Ok(raw) = place.get(&format!("{tree}/{owner}/{name}/versions/{version}/manifest.json")) else { continue };
            let Ok(manifest) = serde_json::from_slice::<J>(&raw) else { continue };
            rows.push(Row { tree, owner: owner.into(), name: name.into(), tag: tag.into(), version, manifest });
        }
    }
    Ok(rows)
}

/// What a hub lists of itself, for a program to read.
pub fn index_json(rows: &[Row]) -> Vec<u8> {
    let carries: Vec<J> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "tree": r.tree, "reference": r.reference(), "tag": r.tag, "version": r.version,
                "title": r.title(), "about": r.about(), "built_at": r.built_at(), "claims": r.claims(),
                "sources": r.includes().len(), "sealed": r.sealed(), "page": r.page(),
            })
        })
        .collect();
    serde_json::to_vec_pretty(&serde_json::json!({ "spec_version": crate::artifact::SPEC_VERSION, "carries": carries }))
        .unwrap_or_default()
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn ago(at: i64) -> String {
    if at <= 0 {
        return String::new();
    }
    format!("{} ago", crate::tracker::human(crate::now() - at))
}

fn shown(republish: &str) -> &'static str {
    match republish {
        "yes" => "in full",
        "summary" => "titles, values and a link",
        "no" => "not in public",
        _ => "has not said",
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Where a tracker can be opened and asked, where somewhere answers for it.
pub type Opens<'a> = &'a dyn Fn(&Row) -> Option<String>;

/// The page around a body: the website's header with the hub where the reader is, and its footer.
fn frame(place: &dyn Place, title: &str, description: &str, body: Markup) -> String {
    let has = |name: &str| place.exists(name);
    // Named by what they hold, so a browser that kept the operator's old design asks for the new.
    let stamp = |name: &str| place.get(name).map(|b| crate::place::sha256(&b)[..8].to_string()).unwrap_or_default();
    let page = html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="theme-color" content="#f2efe7";
                // The same one the website carries: a choice already made is honoured before the
                // first paint, and a reader who made none gets their system's.
                script { (PreEscaped("document.documentElement.className+=\" js\";try{var t=localStorage.getItem(\"theme\");if(t)document.documentElement.dataset.theme=t}catch(e){}")) }
                title { (title) }
                meta name="description" content=(description);
                @if has("favicon.png") { link rel="icon" href="/favicon.png" type="image/png"; }
                link rel="stylesheet" href={"/style.css?v=" (stamp("style.css"))};
            }
            body {
                header.site-header.shell {
                    a.brand href="https://zetlyn.com" {
                        @if has("mark.png") { img src="/mark.png" alt="" class="brand-mark"; }
                        span { "Zetlyn" }
                    }
                    // The website's header, link for link, with the hub where the reader is.
                    nav {
                        @for (label, href) in crate::serve::SITE_NAV {
                            @if *label == "Hub" { a href=(href) aria-current="page" { (label) } } @else { a href=(href) { (label) } }
                        }
                    }
                }
                main { (body) }
                footer.site-footer.shell {
                    span { "© Zetlyn" }
                    div {
                        @for (label, href) in crate::serve::SITE_FOOTER { a href=(href) { (label) } }
                        @if has("app.js") { button.theme-toggle type="button" id="theme-toggle" { "Theme" } }
                    }
                }
                @if has("app.js") { script src={"/app.js?v=" (stamp("app.js"))} {} }
            }
        }
    };
    page.into_string()
}

fn badge(r: &Row) -> &'static str {
    if r.private() {
        "Private"
    } else if r.sealed() {
        "Sealed"
    } else {
        "Public"
    }
}

/// The front page: what anyone can take, first, then what the hub says about itself.
pub fn catalog(place: &dyn Place, rows: &[Row], opens: Opens) -> String {
    let (trackers, sources): (Vec<&Row>, Vec<&Row>) = rows.iter().partition(|r| r.is_tracker());
    let about = place.get("hub-about.html").ok().map(|b| String::from_utf8_lossy(&b).into_owned());
    let total: u64 = sources.iter().map(|r| r.claims()).sum();
    let body = html! {
        section.hero.shell {
            p.overline { "THE HUB · PUBLIC" }
            h1 { "Trackers anyone " span { "can use." } }
            p.intro {
                "Every tracker and source here is public and free to use. Open one in the "
                "browser, or subscribe and keep a copy on your own machine that stays current."
            }
            p.hub-stats {
                span { (plural(trackers.len(), "tracker", "trackers")) }
                span { (plural(sources.len(), "source", "sources")) }
                span { (thousands(total)) " claims" }
            }
        }
        section.hub-list.shell #trackers {
            h2 { "Trackers" }
            p.caption { "A topic, and the sources it is made of, joined on what they share." }
            @if trackers.is_empty() { p { "None yet." } }
            div.hub-cards {
                @for r in &trackers {
                    @let claims: u64 = if r.tree == "packages" { r.claims() } else {
                        r.includes().iter().filter_map(|n| sources.iter().find(|s| s.reference() == *n)).map(|s| s.claims()).sum()
                    };
                    article.hub-card {
                        div.hub-card-head {
                            h3 { a href=(r.page()) { (r.title()) } }
                            span.badge { (badge(r)) }
                        }
                        @if !r.about().is_empty() { p { (r.about()) } }
                        p.hub-meta {
                            (plural(r.includes().len(), "source", "sources")) " · " (thousands(claims)) " claims"
                            @if r.built_at() > 0 { " · published " (ago(r.built_at())) }
                        }
                        div.hub-actions {
                            @if let Some(open) = opens(r) { a.primary href=(open) target="_blank" rel="noopener" { "Open" } }
                            a.secondary href=(r.page()) { "Details" }
                        }
                    }
                }
            }
        }
        section.hub-list.shell #sources {
            h2 { "Sources" }
            p.caption { "Each one can be taken on its own: " code { "zetlyn source subscribe owner/name" } }
            @if sources.is_empty() { p { "None yet." } } @else {
                div.hub-scroll {
                    table.hub-table {
                        thead { tr { th { "Source" } th { "In" } th.n { "Claims" } th { "May be shown" } th { "Published" } } }
                        tbody {
                            @for r in &sources {
                                @let reference = r.reference();
                                @let within: Vec<String> = trackers.iter().filter(|t| t.includes().iter().any(|n| *n == reference)).map(|t| t.title()).collect();
                                tr {
                                    td { a href=(r.page()) { (r.title()) } small { (reference) " · " (r.version.get(..8).unwrap_or(&r.version)) } }
                                    td { @if within.is_empty() { span.dim { "on its own" } } @else { (within.join(", ")) } }
                                    td.n { (thousands(r.claims())) }
                                    td { (shown(&r.republish())) }
                                    td { (ago(r.built_at())) }
                                }
                            }
                        }
                    }
                }
            }
        }
        @if let Some(about) = about { (PreEscaped(about)) }
    };
    frame(place, "The hub — Zetlyn", "Trackers and sources anyone can use: open one, or subscribe and keep a copy that stays current.", body)
}

fn take(command: &str, note: &str) -> Markup {
    html! {
        div.terminal {
            div.term-head { span {} span {} span {} b { "on your own machine" } }
            pre { span.prompt { "$ " } (command) "\n" span.cmt { "# " (note) } }
        }
    }
}

fn crumbs(r: &Row) -> Markup {
    html! {
        p.overline.hub-crumbs {
            a href="/" { "THE HUB" } " · "
            (match r.tree { "sources" => "SOURCE", "packages" => "PACKAGE", _ => "TRACKER" })
            " · " (badge(r).to_uppercase())
        }
    }
}

/// A source: what it is, where it comes from, what one claim carries, and how to take it.
fn source_page(place: &dyn Place, r: &Row, rows: &[Row], opens: Opens) -> String {
    let m = &r.manifest;
    let reference = r.reference();
    let within: Vec<&Row> = rows.iter().filter(|t| t.is_tracker() && t.includes().iter().any(|n| *n == reference)).collect();
    let properties = m["properties"].as_array().cloned().unwrap_or_default();
    let claims = r.claims().max(1);
    let identifiers: BTreeMap<String, u64> = m["identifiers"]
        .as_object()
        .map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_u64().unwrap_or(0))).collect())
        .unwrap_or_default();
    let examples: Vec<String> = m["read"]["search"]["examples"].as_array().map(|a| a.iter().filter_map(|e| e.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let body = html! {
        section.hero.shell.hub-detail {
            (crumbs(r))
            h1 { (r.title()) }
            @if !r.about().is_empty() { p.intro { (r.about()) } }
            p.hub-stats {
                span { (thousands(r.claims())) " claims" }
                @for (scheme, n) in &identifiers { span { (thousands(*n)) " by " (scheme) } }
                @if let (Some(a), Some(b)) = (m["known"]["first"].as_str(), m["known"]["last"].as_str()) { span { (a) " – " (b) } }
                @if let Some(e) = m["every"].as_str() { span { "read every " (e) } }
                span { "published " (ago(r.built_at())) }
                @if m["complete"].as_bool() == Some(false) { span { "partial: not every claim was reached" } }
            }
        }
        section.hub-list.shell {
            h2 { "Where it comes from" }
            div.hub-scroll { table.hub-table.kv { tbody {
                @if let Some(f) = m["fetched_from"].as_str().filter(|f| !f.is_empty()) {
                    tr { td { "read from" } td { @if f.starts_with("http") { a href=(f) { (f) } } @else { (f) } } }
                }
                tr { td { "may be shown" } td { (shown(&r.republish())) } }
                @if let Some(n) = m["licence"]["note"].as_str() { tr { td { "licence" } td { (n) } } }
                @if let Some(t) = m["licence"]["terms"].as_str().or(m["terms"].as_str()).filter(|t| !t.is_empty()) {
                    tr { td { "terms" } td { a href=(t) { (t) } } }
                }
                @if let Some(t) = m["text_is"].as_str() { tr { td { "its text" } td { @if t == "whole" { "in full" } @else { (t) } } } }
                tr { td { "published by" } td { (r.owner) @if let Some(k) = m["signed_by"].as_str() { small { "signed " (k) } } } }
            } } }
        }
        section.hub-list.shell {
            h2 { "What one claim carries" }
            p.caption { "Besides its title, its text and the identifiers that say what it is about." }
            div.hub-scroll { table.hub-table {
                thead { tr { th { "Property" } th { "Type" } th.n { "Claims carrying it" } } }
                tbody {
                    @for p in &properties {
                        @let n = p["claims"].as_u64().unwrap_or(0);
                        tr {
                            td { code { (p["name"].as_str().unwrap_or_default()) } @if let Some(v) = p["vocabulary"].as_str() { small { "words: " (v) } } }
                            td { (p["type"].as_str().unwrap_or_default()) }
                            td.n { (thousands(n)) " · " (n * 100 / claims) "%" }
                        }
                    }
                }
            } }
        }
        section.hub-list.shell {
            h2 { "In trackers" }
            @if within.is_empty() { p.caption { "None on this hub: it is taken on its own, or into a tracker of yours." } }
            @else {
                ul.hub-links { @for t in &within { li { a href=(t.page()) { (t.title()) } @if let Some(o) = opens(t) { " · " a href=(o) target="_blank" rel="noopener" { "open it" } } } } }
            }
        }
        section.hub-list.shell {
            h2 { "Take it" }
            (take(&format!("zetlyn source subscribe {reference}"), "every claim and its history, kept current"))
            @if !examples.is_empty() {
                p.caption { "Questions it answers, as its publisher wrote them: " @for (i, e) in examples.iter().enumerate() { @if i > 0 { ", " } code { (e) } } }
            }
            p.caption { "Version " code { (r.version) } " · " a href={"/sources/" (reference) "/versions/" (r.version) "/manifest.json"} { "manifest.json" } }
        }
    };
    frame(place, &format!("{} — the hub — Zetlyn", r.title()), &r.about(), body)
}

/// A tracker or a package: what it is about, its sources, what it promises, and how to take it.
fn tracker_page(place: &dyn Place, r: &Row, rows: &[Row], opens: Opens) -> String {
    let m = &r.manifest;
    let reference = r.reference();
    let statement = r.statement();
    let why = |name: &str| statement.as_ref().and_then(|d| d.members.iter().find(|s| s.dataset == name).map(|s| s.why.clone())).unwrap_or_default();
    let listed = m["sources"].as_array().cloned().unwrap_or_default();
    let withheld: Vec<String> = m["withheld"].as_array().map(|a| a.iter().filter_map(|w| w.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let body = html! {
        section.hero.shell.hub-detail {
            (crumbs(r))
            h1 { (r.title()) }
            @if !r.about().is_empty() { p.intro { (r.about()) } }
            p.hub-stats {
                span { (plural(listed.len(), "source", "sources")) }
                @if let Some(keys) = m["identified_by"].as_array() {
                    span { "joined on " (keys.iter().filter_map(|k| k.as_str()).collect::<Vec<_>>().join(", ")) }
                }
                span { "published " (ago(r.built_at())) }
            }
            @if let Some(open) = opens(r) {
                div.hero-actions { a.primary href=(open) target="_blank" rel="noopener" { "Open it" } }
            }
        }
        section.hub-list.shell {
            h2 { "Its sources" }
            div.hub-scroll { table.hub-table {
                thead { tr { th { "Source" } th { "What it adds" } th.n { "Claims" } } }
                tbody {
                    @for s in &listed {
                        @let name = s.as_str().or(s["name"].as_str()).unwrap_or_default();
                        @let held = rows.iter().find(|x| x.tree == "sources" && x.reference() == name);
                        tr {
                            td {
                                @match held { Some(h) => { a href=(h.page()) { (h.title()) } }, None => { (s["title"].as_str().filter(|t| !t.is_empty()).unwrap_or(name)) } }
                                small { (name) }
                            }
                            td { (why(name)) }
                            td.n { @match held { Some(h) => { (thousands(h.claims())) }, None => { (s["claims"].as_u64().map(thousands).unwrap_or_default()) } } }
                        }
                    }
                }
            } }
        }
        @if let Some(covers) = m["promise"]["covers"].as_str().filter(|c| !c.is_empty()) {
            section.hub-list.shell {
                h2 { "What it promises" }
                div.hub-scroll { table.hub-table.kv { tbody {
                    @if let Some(f) = m["promise"]["fresh_within"].as_str() { tr { td { "fresh within" } td { (f) } } }
                    tr { td { "covers" } td { (covers) } }
                    @if let Some(x) = m["promise"]["excludes"].as_str().filter(|x| !x.is_empty()) { tr { td { "excludes" } td { (x) } } }
                } } }
            }
        }
        @if r.sealed() {
            section.hub-list.shell {
                h2 { "Sealed" }
                p.caption { "Every claim, its history, the conflicts and what changed, in the tracker's own words. What stays with its publisher:" }
                ul.hub-links { @for w in &withheld { li { (w) } } }
            }
        }
        section.hub-list.shell {
            h2 { "Take it" }
            (take(&format!("zetlyn tracker subscribe {reference}"), if r.sealed() { "one signed file, kept current" } else { "the statement, and every source it names" }))
            p.caption {
                "Version " code { (r.version) }
                @if let Some(k) = m["signed_by"].as_str() { " · signed " code { (k) } }
                " · " a href={"/" (r.tree) "/" (reference) "/versions/" (r.version) "/manifest.json"} { "manifest.json" }
            }
            @if let Some(text) = m["declaration"].as_str() {
                details { summary { "Its statement" } pre.hub-statement { (text) } }
            }
        }
    };
    frame(place, &format!("{} — the hub — Zetlyn", r.title()), &r.about(), body)
}

/// The page for one row.
pub fn page(place: &dyn Place, r: &Row, rows: &[Row], opens: Opens) -> String {
    if r.is_tracker() {
        tracker_page(place, r, rows, opens)
    } else {
        source_page(place, r, rows, opens)
    }
}

/// The page a path asks for, rendered now: `/`, or `/<tree>/<owner>/<name>/`.
pub fn page_at(place: &dyn Place, path: &str, opens: Opens) -> Option<String> {
    let rows = shelf(place).ok()?;
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return Some(catalog(place, &rows, opens));
    }
    let r = rows.iter().find(|r| format!("{}/{}/{}", r.tree, r.owner, r.name) == trimmed && r.tag == "latest")
        .or_else(|| rows.iter().find(|r| format!("{}/{}/{}", r.tree, r.owner, r.name) == trimmed))?;
    Some(page(place, r, &rows, opens))
}

/// Every page written into the place, beside what it describes, and `index.json` with them.
pub fn render(place: &dyn Place, opens: Opens) -> Result<usize, String> {
    let rows = shelf(place)?;
    place.put("index.html", catalog(place, &rows, opens).as_bytes())?;
    place.put("index.json", &index_json(&rows))?;
    let mut written = 2;
    let mut seen = std::collections::BTreeSet::new();
    for r in &rows {
        // A thing under several tags has one page, and it is about the one called latest.
        let key = r.page();
        let latest = rows.iter().find(|x| x.page() == key && x.tag == "latest").unwrap_or(r);
        if !seen.insert(key.clone()) {
            continue;
        }
        place.put(&format!("{}index.html", key.trim_start_matches('/')), page(place, latest, &rows, opens).as_bytes())?;
        written += 1;
    }
    Ok(written)
}
