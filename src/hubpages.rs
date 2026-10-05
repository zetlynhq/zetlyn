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
    /// Every version it has on this hub, the one the tag names among them.
    pub versions: Vec<String>,
}

impl Row {
    pub fn reference(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
    /// Where its page is.
    /// Where its page is kept in the hub, from the hub's root.
    pub fn page(&self) -> String {
        format!("/{}/{}/{}/", self.tree, self.owner, self.name)
    }
    /// Where its page is read: a hub is at `/hub/` on the name it shares with a site or a world.
    pub fn href(&self) -> String {
        format!("/hub{}", self.page())
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
        // Every version each thing has, from the names alone: a version is a directory with a
        // manifest in it, and a delta inside one is not another.
        let mut versions: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
        for path in &listed {
            let parts: Vec<&str> = path.split('/').collect();
            if let [_, owner, name, "versions", v, "manifest.json"] = parts[..] {
                versions.entry((owner.to_string(), name.to_string())).or_default().push(v.to_string());
            }
        }
        for path in &listed {
            let parts: Vec<&str> = path.split('/').collect();
            let [_, owner, name, "tags", tag] = parts[..] else { continue };
            let Ok(v) = place.get(path) else { continue };
            let version = String::from_utf8_lossy(&v).trim().to_string();
            let Ok(raw) = place.get(&format!("{tree}/{owner}/{name}/versions/{version}/manifest.json")) else { continue };
            let Ok(manifest) = serde_json::from_slice::<J>(&raw) else { continue };
            let all = versions.get(&(owner.to_string(), name.to_string())).cloned().unwrap_or_default();
            rows.push(Row { tree, owner: owner.into(), name: name.into(), tag: tag.into(), version, manifest, versions: all });
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
                "sources": r.includes().len(), "sealed": r.sealed(), "page": r.href(),
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

/// Where a thing runs, as a hub says it beside it: the world it was published from, as its
/// manifest names it. One published before manifests named it is placed by where its owner's
/// trackers open (`…/<org>/t/<name>/` runs at `…/<org>`). The address, and the words shown for it.
fn instance(r: &Row, rows: &[Row], opens: Opens) -> Option<(String, String)> {
    let from_open = |row: &Row| opens(row).and_then(|o| o.trim_end_matches('/').rsplit_once("/trackers/").map(|(world, _)| world.to_string()));
    let world = r.manifest["world"]
        .as_str()
        .filter(|w| w.starts_with("https://") || w.starts_with("http://"))
        .map(|w| w.trim_end_matches('/').to_string())
        .or_else(|| from_open(r))
        .or_else(|| rows.iter().filter(|o| o.owner == r.owner).find_map(from_open))?;
    let shown = world.split_once("://").map(|(_, rest)| rest.to_string()).unwrap_or_else(|| world.clone());
    Some((world, shown))
}

/// The page around a body: the website's header with the hub where the reader is, and its footer.
fn frame(place: &dyn Place, title: &str, description: &str, body: Markup) -> String {
    let has = |name: &str| place.exists(name);
    // Named by what they hold, so a browser that kept the operator's old design asks for the new.
    // What the hub holds of a file, and this release: on zetlyn.com the sheet is the website's, which
    // the hub's own copy says nothing about, and a release that changes the pages changes the name.
    let stamp = |name: &str| format!("{}-{}", place.get(name).map(|b| crate::place::sha256(&b)[..8].to_string()).unwrap_or_default(), env!("CARGO_PKG_VERSION"));
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
            body.area-hub {
                // The header and the footer every page of Zetlyn has (serve::site_header).
                (crate::serve::site_header("hub", crate::serve::Reader::Anyone))
                main { (body) }
                (crate::serve::site_footer(&[]))
                @if has("app.js") { script src={"/app.js?v=" (stamp("app.js"))} {} }
                script { (PreEscaped(SEARCH)) }
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

/// What a row is, in one word: a tracker, a source, or a package.
fn kind_of(r: &Row) -> &'static str {
    match r.tree {
        "sources" => "source",
        "packages" => "package",
        _ => "tracker",
    }
}

/// A row's claims, a tracker's being its sources' together.
fn claims_of(r: &Row, rows: &[Row]) -> u64 {
    if r.tree != "trackers" {
        return r.claims();
    }
    r.includes()
        .iter()
        .filter_map(|n| rows.iter().find(|s| s.tree == "sources" && s.reference() == *n))
        .map(|s| s.claims())
        .sum()
}

/// The front page: a registry. Search first, the facets beside it, every tracker, package and
/// source as one list, and one line beneath on how the hub works, which the docs say.
pub fn catalog(place: &dyn Place, rows: &[Row], opens: Opens) -> String {
    let trackers: Vec<&Row> = rows.iter().filter(|r| r.is_tracker()).collect();
    let sources: Vec<&Row> = rows.iter().filter(|r| !r.is_tracker()).collect();
    let packages = rows.iter().filter(|r| r.tree == "packages").count();
    let total: u64 = sources.iter().map(|r| r.claims()).sum();
    let mut items: Vec<&Row> = trackers.iter().chain(sources.iter()).copied().collect();
    items.dedup_by(|a, b| a.page() == b.page());
    let shown_kinds: [(&str, &str, usize); 3] = [
        ("tracker", "Trackers", rows.iter().filter(|r| r.tree == "trackers").count()),
        ("package", "Packages", packages),
        ("source", "Sources", sources.len()),
    ];
    let body = html! {
        (trail(None))
        section.hub-top.shell {
            p.overline { "PUBLIC" }
            h1.hub-title { "Find a tracker or a source" }
            p.hub-sub { "Every one here is public and free to use: open it in the browser, or subscribe and keep a copy on your own machine that stays current." }
            input #hub-filter type="search" autocomplete="off" aria-label="Search the hub"
                placeholder=(format!("Search {} and {}", plural(trackers.len(), "tracker", "trackers"), plural(sources.len(), "source", "sources")));
        }
        section.hub-registry.shell #trackers {
            aside.hub-facets {
                fieldset {
                    legend { "Type" }
                    @for (key, label, n) in &shown_kinds {
                        @if *n > 0 { label { input type="checkbox" data-facet="kind" value=(key) checked; " " (label) span.n { (n) } } }
                    }
                }
                fieldset {
                    legend { "Sources may be shown" }
                    @for (key, label) in [("yes", "in full"), ("summary", "titles, values, a link"), ("other", "has not said")] {
                        label { input type="checkbox" data-facet="shown" value=(key) checked; " " (label) }
                    }
                }
                fieldset {
                    legend { "Sort" }
                    select #hub-sort aria-label="Sort" {
                        option value="kind" { "Trackers first" }
                        option value="built" { "Recently published" }
                        option value="claims" { "Most claims" }
                        option value="name" { "Name" }
                    }
                }
                p.caption { (thousands(total)) " claims in all" }
            }
            div.hub-list #hub-list {
                p.hub-count #hub-count { (plural(items.len(), "result", "results")) }
                @for r in &items {
                    @let claims = claims_of(r, rows);
                    @let shown_key = match r.republish().as_str() { "yes" => "yes", "summary" => "summary", _ if r.is_tracker() => "", _ => "other" };
                    article.hub-row id=[(r.tree == "sources" && Some(r.page()) == sources.first().map(|s| s.page())).then_some("sources")]
                        data-kind=(kind_of(r)) data-shown=(shown_key) data-claims=(claims) data-built=(r.built_at())
                        data-name=(r.title().to_lowercase())
                        data-text=(format!("{} {} {}", r.title(), r.reference(), r.about()).to_lowercase()) {
                        div.hub-row-main {
                            h3 { a href=(r.href()) { (r.title()) } span.badge { (kind_of(r)) } @if r.is_tracker() && badge(r) != "Public" { span.badge { (badge(r)) } } }
                            @if !r.about().is_empty() { p { (r.about()) } }
                            p.hub-meta {
                                (r.reference())
                                @if r.is_tracker() { " · " (plural(r.includes().len(), "source", "sources")) }
                                " · " (thousands(claims)) " claims"
                                @if !r.is_tracker() { " · " (shown(&r.republish())) }
                                @if r.built_at() > 0 { " · published " (ago(r.built_at())) }
                                @if let Some((world, shown)) = instance(r, rows, opens) { " · from " a.hub-from href=(world) { (shown) } }
                            }
                        }
                        div.hub-row-act {
                            @if let Some(open) = opens(r) { a.primary href=(open) { "Live" } }
                            a.secondary href=(r.href()) { "Entry" }
                        }
                    }
                }
                p.hub-none #hub-none hidden { "Nothing here matches. " a href="/hub/" { "Show everything" } }
            }
        }
        // What taking one and publishing one are is the docs' to say, not the catalog's.
        p.hub-how.shell { "Keep a copy that stays current, or publish your own: " a href="https://zetlyn.com/docs/hub" { "how the hub works" } "." }
        script { (PreEscaped(FILTER)) }
    };
    frame(place, "Zetlyn Hub", "Public trackers and sources: search them, open one, or subscribe and keep a copy that stays current.", body)
}

fn take(command: &str, note: &str) -> Markup {
    html! {
        div.terminal {
            div.term-head { span {} span {} span {} b { "on your own machine" } }
            pre { span.prompt { "$ " } (command) "\n" span.cmt { "# " (note) } }
        }
    }
}

/// Where a page is, beneath the header: Zetlyn, the hub, and on a page about one thing, its kind
/// and its name. The last is where the reader is and leads nowhere.
fn trail(r: Option<&Row>) -> Markup {
    // As the address says it: /hub/trackers/<owner>/<name>/ is Hub / Trackers / owner / name.
    html! {
        nav.crumbs.shell aria-label="Breadcrumb" {
            ol {
                @match r {
                    None => { li { span aria-current="page" { "Hub" } } }
                    Some(r) => {
                        li { a href="/hub/" { "Hub" } }
                        li { a href={"/hub/#" (if r.is_tracker() { "trackers" } else { "sources" })} { (if r.is_tracker() { "Trackers" } else if r.tree == "packages" { "Packages" } else { "Sources" }) } }
                        li { span { (r.owner) } }
                        li { span aria-current="page" { (r.title()) } }
                    }
                }
            }
        }
    }
}

/// What a page about one thing is, above its name: its kind, that this is its entry in the hub,
/// and whether it is public.
fn crumbs(r: &Row) -> Markup {
    html! {
        p.overline.hub-crumbs {
            (kind_of(r).to_uppercase()) " · IN THE HUB"
            @if badge(r) != "Public" { " · " (badge(r).to_uppercase()) }
        }
    }
}

/// Beneath its name, what this page is and where the thing itself is: an entry is not the
/// tracker, and a reader is told which of the two they are looking at.
fn entry_note(r: &Row, rows: &[Row], opens: Opens) -> Markup {
    let runs = opens(r).is_some();
    html! {
        @match instance(r, rows, opens) {
            Some((world, shown)) => {
                p.hub-entry {
                    "This is its entry in the hub: what it holds and how to take a copy. "
                    @if runs { "It runs at " } @else { "It is published from " }
                    a href=(world) { (shown) } "."
                }
            }
            None => { p.hub-entry { "This is its entry in the hub: what it holds and how to take a copy." } }
        }
    }
}

/// The tabs of a page about one thing. Each is a section of the page; without the script that
/// shows one at a time they are all there, one after the other.
fn tabs(names: &[(&str, &str)]) -> Markup {
    html! {
        nav.hub-tabs.shell aria-label="Sections" {
            @for (i, (id, label)) in names.iter().enumerate() {
                a.on[i == 0] href={"#" (id)} data-tab=(id) { (label) }
            }
        }
    }
}

/// Every version a thing has on the hub, newest first, with when it was published and how much it held.
fn versions(place: &dyn Place, r: &Row) -> Markup {
    let mut held: Vec<(i64, String, u64)> = r
        .versions
        .iter()
        .filter_map(|v| {
            let raw = place.get(&format!("{}/{}/{}/versions/{v}/manifest.json", r.tree, r.owner, r.name)).ok()?;
            let m: J = serde_json::from_slice(&raw).ok()?;
            let claims = m["claims"].as_u64().unwrap_or_else(|| {
                m["sources"].as_array().map(|a| a.iter().filter_map(|s| s["claims"].as_u64()).sum()).unwrap_or(0)
            });
            Some((m["built_at"].as_i64().unwrap_or(0), v.clone(), claims))
        })
        .collect();
    held.sort_by(|a, b| b.cmp(a));
    html! {
        div.hub-scroll { table.hub-table {
            thead { tr { th { "Version" } th { "Published" } th.n { "Claims" } th {} } }
            tbody {
                @for (at, v, claims) in &held {
                    tr {
                        td { code { (v.get(..12).unwrap_or(v)) } }
                        td { (crate::iso_date(*at)) " · " (ago(*at)) }
                        td.n { @if *claims > 0 { (thousands(*claims)) } }
                        td { @if *v == r.version { span.badge { (r.tag) } } }
                    }
                }
            }
        } }
        p.caption { "A subscriber holds one of these and is told of the next; an update fetches only what changed between them." }
    }
}

/// A source: what it is, where it comes from, what one claim carries, its versions, and how to take it.
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
        (trail(Some(r)))
        section.hub-detail-top.shell {
            (crumbs(r))
            h1 { (r.title()) }
            @if !r.about().is_empty() { p.intro { (r.about()) } }
            (entry_note(r, rows, opens))
            p.hub-stats {
                span { (thousands(r.claims())) " claims" }
                @for (scheme, n) in &identifiers { span { (thousands(*n)) " by " (scheme) } }
                @if let (Some(a), Some(b)) = (m["known"]["first"].as_str(), m["known"]["last"].as_str()) { span { (a) " – " (b) } }
                @if let Some(e) = m["every"].as_str() { span { "read every " (e) } }
                span { "published " (ago(r.built_at())) }
                @if let Some((world, shown)) = instance(r, rows, opens) { span { "from " a.hub-from href=(world) { (shown) } } }
                @if m["complete"].as_bool() == Some(false) { span { "partial: not every claim was reached" } }
            }
        }
        (tabs(&[("overview", "Overview"), ("properties", "Properties"), ("versions", "Versions"), ("use", "Use it")]))
        section.hub-tab.shell #overview {
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
            h2 { "In trackers" }
            @if within.is_empty() { p.caption { "None on this hub: it is taken on its own, or into a tracker of yours." } }
            @else {
                ul.hub-links { @for t in &within { li { a href=(t.href()) { (t.title()) } @if let Some(o) = opens(t) { " · " a href=(o) target="_blank" rel="noopener" { "open it" } } } } }
            }
            @if !examples.is_empty() {
                h2 { "Questions it answers" }
                p.caption { "As its publisher wrote them: " @for (i, e) in examples.iter().enumerate() { @if i > 0 { ", " } code { (e) } } }
            }
        }
        section.hub-tab.shell #properties {
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
        section.hub-tab.shell #versions { h2 { "Versions" } (versions(place, r)) }
        section.hub-tab.shell #use {
            h2 { "Use it" }
            (take(&format!("zetlyn source subscribe {reference}"), "every claim and its history, kept current"))
            p.caption { "Version " code { (r.version) } " · " a href={"/hub/sources/" (reference) "/versions/" (r.version) "/manifest.json"} { "manifest.json" } }
        }
        script { (PreEscaped(TABS)) }
    };
    frame(place, &format!("{} · Zetlyn Hub", r.title()), &r.about(), body)
}

/// A tracker or a package: what it is about, its sources, what it compares, its versions, and how to take it.
fn tracker_page(place: &dyn Place, r: &Row, rows: &[Row], opens: Opens) -> String {
    let m = &r.manifest;
    let reference = r.reference();
    let statement = r.statement();
    let why = |name: &str| statement.as_ref().and_then(|d| d.members.iter().find(|s| s.dataset == name).map(|s| s.why.clone())).unwrap_or_default();
    let listed = m["sources"].as_array().cloned().unwrap_or_default();
    let withheld: Vec<String> = m["withheld"].as_array().map(|a| a.iter().filter_map(|w| w.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let compared: Vec<(String, String)> = statement
        .as_ref()
        .map(|d| {
            d.normalise
                .iter()
                .map(|(name, a)| {
                    let how = if !a.scale.is_empty() {
                        format!("on the scale {}", a.scale.join(" > "))
                    } else if let Some(t) = &a.tolerance {
                        format!("within {t}")
                    } else {
                        "exactly".to_string()
                    };
                    (name.clone(), how)
                })
                .collect()
        })
        .unwrap_or_default();
    let body = html! {
        (trail(Some(r)))
        section.hub-detail-top.shell {
            (crumbs(r))
            h1 { (r.title()) }
            @if !r.about().is_empty() { p.intro { (r.about()) } }
            (entry_note(r, rows, opens))
            p.hub-stats {
                span { (plural(listed.len(), "source", "sources")) }
                span { (thousands(claims_of(r, rows))) " claims" }
                @if let Some(keys) = m["identified_by"].as_array() {
                    span { "joined on " (keys.iter().filter_map(|k| k.as_str()).collect::<Vec<_>>().join(", ")) }
                }
                span { "published " (ago(r.built_at())) }
                @if let Some((world, shown)) = instance(r, rows, opens) { span { "from " a.hub-from href=(world) { (shown) } } }
            }
            @if let Some(open) = opens(r) {
                div.hero-actions { a.primary href=(open) { "Open the live tracker →" } }
            }
        }
        (tabs(&[("overview", "Overview"), ("properties", "Properties"), ("versions", "Versions"), ("use", "Use it")]))
        section.hub-tab.shell #overview {
            h2 { "Its sources" }
            div.hub-scroll { table.hub-table {
                thead { tr { th { "Source" } th { "What it adds" } th.n { "Claims" } } }
                tbody {
                    @for s in &listed {
                        @let name = s.as_str().or(s["name"].as_str()).unwrap_or_default();
                        @let held = rows.iter().find(|x| x.tree == "sources" && x.reference() == name);
                        tr {
                            td {
                                @match held { Some(h) => { a href=(h.href()) { (h.title()) } }, None => { (s["title"].as_str().filter(|t| !t.is_empty()).unwrap_or(name)) } }
                                small { (name) }
                            }
                            td { (why(name)) }
                            td.n { @match held { Some(h) => { (thousands(h.claims())) }, None => { (s["claims"].as_u64().map(thousands).unwrap_or_default()) } } }
                        }
                    }
                }
            } }
            @if let Some(covers) = m["promise"]["covers"].as_str().filter(|c| !c.is_empty()) {
                h2 { "What it promises" }
                div.hub-scroll { table.hub-table.kv { tbody {
                    @if let Some(f) = m["promise"]["fresh_within"].as_str() { tr { td { "fresh within" } td { (f) } } }
                    tr { td { "covers" } td { (covers) } }
                    @if let Some(x) = m["promise"]["excludes"].as_str().filter(|x| !x.is_empty()) { tr { td { "excludes" } td { (x) } } }
                } } }
            }
            @if r.sealed() {
                h2 { "Sealed" }
                p.caption { "Every claim, its history, the conflicts and what changed, in the tracker's own words. What stays with its publisher:" }
                ul.hub-links { @for w in &withheld { li { (w) } } }
            }
        }
        section.hub-tab.shell #properties {
            h2 { "What it compares" }
            @if compared.is_empty() { p.caption { "Nothing is held against anything: its sources are shown side by side." } }
            @else {
                p.caption { "Each of these is held against every source that says it; a difference beyond what is allowed is a conflict. Everything else is shown side by side." }
                div.hub-scroll { table.hub-table {
                    thead { tr { th { "Property" } th { "Compared" } } }
                    tbody { @for (name, how) in &compared { tr { td { code { (name) } } td { (how) } } } }
                } }
            }
        }
        section.hub-tab.shell #versions { h2 { "Versions" } (versions(place, r)) }
        section.hub-tab.shell #use {
            h2 { "Use it" }
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
        script { (PreEscaped(TABS)) }
    };
    frame(place, &format!("{} · Zetlyn Hub", r.title()), &r.about(), body)
}

/// The front page's facets and its search, in the browser, over the rows the page already holds.
const FILTER: &str = r##"(function () {
  var list = document.getElementById("hub-list"); if (!list) return;
  var rows = Array.prototype.slice.call(list.querySelectorAll(".hub-row"));
  var q = document.getElementById("hub-filter"), sort = document.getElementById("hub-sort");
  var count = document.getElementById("hub-count"), none = document.getElementById("hub-none");
  function on(facet) { return Array.prototype.slice.call(document.querySelectorAll('input[data-facet="' + facet + '"]:checked')).map(function (i) { return i.value; }); }
  function apply() {
    var kinds = on("kind"), shown = on("shown");
    var words = (q.value || "").trim().toLowerCase().split(/\s+/).filter(Boolean);
    var n = 0;
    rows.forEach(function (r) {
      var s = r.dataset.shown, ok = kinds.indexOf(r.dataset.kind) >= 0 && (!s || shown.indexOf(s) >= 0)
        && words.every(function (w) { return r.dataset.text.indexOf(w) >= 0; });
      r.hidden = !ok; if (ok) n++;
    });
    var by = sort.value;
    rows.slice().sort(function (a, b) {
      if (by === "name") return a.dataset.name < b.dataset.name ? -1 : 1;
      if (by === "kind") {
        var rank = { tracker: 0, package: 1, source: 2 }, d = rank[a.dataset.kind] - rank[b.dataset.kind];
        return d || Number(b.dataset.claims) - Number(a.dataset.claims);
      }
      return Number(b.dataset[by]) - Number(a.dataset[by]);
    }).forEach(function (r) { list.insertBefore(r, none); });
    count.textContent = n + (n === 1 ? " result" : " results");
    none.hidden = n > 0;
  }
  [q, sort].forEach(function (el) { el.addEventListener("input", apply); });
  document.querySelectorAll("input[data-facet]").forEach(function (el) { el.addEventListener("change", apply); });
  apply();
})();"##;

/// One section of a page about one thing at a time, the one its tab names, and the address says which.
const TABS: &str = r##"(function () {
  var tabs = Array.prototype.slice.call(document.querySelectorAll(".hub-tabs a[data-tab]"));
  if (!tabs.length) return;
  function show() {
    var want = (location.hash || "").slice(1);
    if (!tabs.some(function (t) { return t.dataset.tab === want; })) want = tabs[0].dataset.tab;
    tabs.forEach(function (t) {
      var on = t.dataset.tab === want, s = document.getElementById(t.dataset.tab);
      t.classList.toggle("on", on); if (s) s.hidden = !on;
    });
  }
  // A tab is chosen, not scrolled to: the address says which, and the page stays where it is.
  tabs.forEach(function (t) {
    t.addEventListener("click", function (e) {
      e.preventDefault();
      history.replaceState(null, "", "#" + t.dataset.tab);
      show();
    });
  });
  window.addEventListener("hashchange", show);
  show();
  if (location.hash) window.scrollTo(0, 0);
})();"##;

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

/// The search in the hub's header. It reads `/index.json` once, the first time somebody types, and
/// looks through the titles, names and sentences there: what a hub lists about itself, and no more.
const SEARCH: &str = r##"(function () {
  var q = document.getElementById("hub-q"), box = document.getElementById("hub-results");
  if (!q || !box) return;
  var rows = null, asked = false;
  function kind(t) { return t === "sources" ? "source" : t === "packages" ? "package" : "tracker"; }
  function show() {
    var words = q.value.trim().toLowerCase().split(/\s+/).filter(Boolean);
    box.innerHTML = "";
    if (!words.length || !rows) { box.hidden = true; return; }
    var hits = rows.filter(function (r) {
      var hay = (r.title + " " + r.reference + " " + (r.about || "")).toLowerCase();
      return words.every(function (w) { return hay.indexOf(w) >= 0; });
    }).slice(0, 8);
    if (!hits.length) {
      var none = document.createElement("p"); none.textContent = "Nothing here by that name."; box.appendChild(none);
    }
    hits.forEach(function (r) {
      var a = document.createElement("a"); a.href = r.page;
      var t = document.createElement("b"); t.textContent = r.title;
      var k = document.createElement("small"); k.textContent = kind(r.tree) + " · " + r.reference;
      a.appendChild(t); a.appendChild(k); box.appendChild(a);
    });
    box.hidden = false;
  }
  q.addEventListener("input", function () {
    if (!asked) {
      asked = true;
      fetch("/hub/index.json").then(function (r) { return r.json(); })
        .then(function (j) { rows = (j.carries || []).filter(function (r, i, all) {
          return all.findIndex(function (x) { return x.page === r.page; }) === i; }); show(); })
        .catch(function () { rows = []; });
    }
    show();
  });
  q.addEventListener("keydown", function (e) {
    if (e.key === "Enter") { var first = box.querySelector("a"); if (first) location.href = first.href; }
    if (e.key === "Escape") { q.value = ""; show(); }
  });
  document.addEventListener("click", function (e) { if (!box.contains(e.target) && e.target !== q) box.hidden = true; });
})();"##;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(tree: &'static str, owner: &str, name: &str, manifest: J) -> Row {
        Row { tree, owner: owner.into(), name: name.into(), tag: "latest".into(), version: "v1".into(), manifest, versions: vec!["v1".into()] }
    }

    #[test]
    fn a_thing_says_the_world_it_runs_in() {
        let tracker = row("trackers", "zetlyn", "cve", json!({}));
        let source = row("sources", "zetlyn", "cve-kev", json!({}));
        let named = row("sources", "ann", "prices", json!({ "world": "https://prices.example/" }));
        let stranger = row("sources", "bob", "x", json!({ "world": "javascript:alert(1)" }));
        let rows = vec![row("trackers", "zetlyn", "cve", json!({})), row("sources", "zetlyn", "cve-kev", json!({}))];
        let opens = |r: &Row| (r.reference() == "zetlyn/cve").then(|| "https://zetlyn.com/worlds/zetlyn/trackers/cve/".to_string());
        // Its manifest says it; or where its own tracker opens; or where its owner's does.
        assert_eq!(instance(&named, &rows, &opens), Some(("https://prices.example".into(), "prices.example".into())));
        assert_eq!(instance(&tracker, &rows, &opens), Some(("https://zetlyn.com/worlds/zetlyn".into(), "zetlyn.com/worlds/zetlyn".into())));
        assert_eq!(instance(&source, &rows, &opens), Some(("https://zetlyn.com/worlds/zetlyn".into(), "zetlyn.com/worlds/zetlyn".into())));
        // Nothing that is not an address, and nothing where nothing says.
        assert_eq!(instance(&stranger, &rows, &opens), None);
    }
}
