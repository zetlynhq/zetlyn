//! A tracker, opened. The same three surfaces a source has, over sources of unlike shape.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use maud::{html, Markup};
use serde_json::{json, Value as J};

use crate::account::{self, Accounts, Site, Viewer};
use crate::expr;
use crate::tracker::{Thing, Tracker, TrackerQuery};
use crate::serve::{at, flatten, mounted, params, shell, unmount, urlencode};

/// A query as one address: each word once, in the order first said, and no more than
/// `MAX_TERMS` of them. Every filter click used to add its term again, and crawlers walked the
/// endless addresses that made, each one worked out as it was asked for.
const MAX_TERMS: usize = 8;

fn canonical(q: &str) -> String {
    let mut seen = BTreeSet::new();
    q.split_whitespace().filter(|t| seen.insert(t.to_string())).take(MAX_TERMS).collect::<Vec<_>>().join(" ")
}

/// A filter added to a query, unless it is there already.
fn with_term(q: &str, term: &str) -> String {
    canonical(&format!("{q} {term}"))
}

fn link(q: &str, view: &str, kind: &str, page: usize) -> String {
    let mut url = format!("{}/?q={}", mounted(), urlencode(q));
    if !view.is_empty() {
        url.push_str(&format!("&view={}", urlencode(view)));
    }
    if !kind.is_empty() {
        url.push_str(&format!("&kind={}", urlencode(kind)));
    }
    if page > 1 {
        url.push_str(&format!("&page={page}"));
    }
    url
}

fn entry_link(e: &Thing) -> Option<String> {
    let k = e.key.as_ref()?;
    Some(format!(
        "{}/thing/{}/{}",
        mounted(),
        urlencode(&k.scheme),
        urlencode(&k.value)
    ))
}

fn cell(e: &Thing, name: &str) -> Markup {
    match name {
        "kind" => html! {
            @for (kind, parts) in e.by_kind() {
                span.chip { (kind) " " (parts.len()) } " "
            }
        },
        "known" => {
            let earliest = e.parts.iter().map(|p| p.known.as_str()).min().unwrap_or("");
            html! { (earliest) }
        }
        "publishers" | "sources" => html! { (e.members().len()) },
        other => match e.fields.get(other) {
            Some(f) => {
                let distinct: Vec<&String> = {
                    let mut v: Vec<&String> = f.means.values().flatten().collect();
                    v.sort();
                    v.dedup();
                    v
                };
                html! {
                    @if f.divergent { span.chip.on { (distinct.iter().map(|s| yes_no(s.as_str()))
                        .collect::<Vec<_>>().join(" / ")) } }
                    @else { (distinct.first().map(|s| yes_no(s.as_str())).unwrap_or("")) }
                }
            }
            None => html! { span.dim { "—" } },
        },
    }
}

/// Where this tracker's entry in the hub is, where it has one: published by its world, public,
/// and at the hub its world says it publishes to (zetlyn.com's for a world hosted there, a
/// world's own at `<url>/hub` otherwise).
fn hub_entry(scope: &Tracker, world: &Site) -> Option<String> {
    let hub = hub_of(scope, world)?;
    // Its page there is read owner first: /hub/<owner>/trackers/<name>/.
    let (owner, name) = scope.decl.name.split_once('/')?;
    Some(format!("{hub}/{owner}/trackers/{name}/"))
}

/// The hub this tracker is published to, where it is: the address it is read at, or the world's
/// own hub. None for a tracker that is private or was never published.
fn hub_of(scope: &Tracker, world: &Site) -> Option<String> {
    let publish = world.publish.as_ref()?;
    if scope.decl.visibility == "private" || !scope.dir.join(".published").exists() {
        return None;
    }
    if !publish.read_at.trim().is_empty() {
        Some(publish.read_at.trim().trim_end_matches('/').to_string())
    } else if !world.url.trim().is_empty() {
        Some(format!("{}/hub", world.url.trim().trim_end_matches('/')))
    } else {
        None
    }
}

/// Its versions on the hub and how to take a copy: what the hub's page about it showed, on the
/// tracker's own, so a tracker is one page wherever it runs. The hub is read over HTTP, and
/// what it says changes only when the tracker is published, so the answer is kept a while.
fn versions_page(scope: &Tracker, world: &Site) -> String {
    static KEPT: std::sync::Mutex<Option<BTreeMap<String, (std::time::Instant, String)>>> = std::sync::Mutex::new(None);
    let name = scope.decl.name.clone();
    let Some(hub) = hub_of(scope, world) else {
        return shell("Versions", html! {
            h1 { "Versions" }
            p.dim { "This tracker is not published to a hub, so it has no versions anybody can take. Its owner publishes it with "
                code { "zetlyn tracker publish" } "." }
        });
    };
    let key = format!("{hub} {name}");
    if let Ok(kept) = KEPT.lock() {
        if let Some((at, page)) = kept.as_ref().and_then(|k| k.get(&key)) {
            if at.elapsed().as_secs() < 300 {
                return page.clone();
            }
        }
    }
    let row = crate::place::at(&hub).ok().and_then(|place| crate::hubpages::read_entry(place.as_ref(), "trackers", &name));
    let page = match row {
        None => shell("Versions", html! {
            h1 { "Versions" }
            p.dim { "The hub at " a href={(hub) "/"} { (hub) } " did not answer with this tracker just now. Try again in a minute." }
        }),
        Some((row, held)) => {
            let m = &row.manifest;
            let reference = row.reference();
            let manifest = format!("{hub}/{}/{reference}/versions/{}/manifest.json", row.tree, row.version);
            shell("Versions", html! {
                h1 { "Versions" }
                p.lede { "Every version of " (scope.decl.title) " on the hub, and how to keep a copy of it on your own machine, current." }
                h2 { "Take a copy" }
                pre { code { "zetlyn tracker subscribe " (reference) } }
                p.dim {
                    @if row.sealed() { "One signed file: every claim, its history, the conflicts and what changed, in this tracker's words. " }
                    @else { "Its statement, and every source it names, each with its claims and their receipts. " }
                    "A subscriber is told of the next version and fetches only what changed."
                }
                h2 { "Versions" }
                table { thead { tr { th { "Version" } th { "Published" } th { "Claims" } th {} } }
                    tbody { @for (at, v, claims) in &held { tr {
                        td { code { (v.get(..12).unwrap_or(v)) } }
                        td { (crate::iso_date(*at)) }
                        td { @if *claims > 0 { (thousands(*claims as i64)) } }
                        td { @if *v == row.version { span.chip.on { (row.tag) } } }
                    } } }
                }
                p.dim {
                    "Version " code { (row.version.get(..12).unwrap_or(&row.version)) }
                    @if let Some(k) = m["signed_by"].as_str() { " · signed " code { (k) } }
                    " · " a href=(manifest) { "manifest.json" }
                }
                @if let Some(text) = m["declaration"].as_str() {
                    details { summary { "Its statement" } pre { (text) } }
                }
            })
        }
    };
    if let Ok(mut kept) = KEPT.lock() {
        kept.get_or_insert_with(BTreeMap::new).insert(key, (std::time::Instant::now(), page.clone()));
    }
    page
}

fn overview(scope: &Tracker, url: &str, v: &Viewer, site: &Site) -> String {
    let bound = account::bound(v, &scope.decl.name);
    let p = params(url);
    let q = canonical(p.get("q").map(String::as_str).unwrap_or(""));
    let view = p.get("view").cloned().unwrap_or_default();
    let kind = p.get("kind").cloned().unwrap_or_default();
    let page: usize = p
        .get("page")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
        .max(1);
    let limit = 25;

    let (text, pred) = expr::parse_query(&q);
    let sq = TrackerQuery {
        text,
        pred: pred.clone(),
        named: (!view.is_empty()).then(|| view.clone()),
        kind: (!kind.is_empty()).then(|| kind.clone()),
        sort: None,
        limit,
        offset: (page - 1) * limit,
        seen_before: bound.clone(),
    };
    // Each part timed, and said in the log where the page took longer than half a second: which
    // part of a slow page is slow is otherwise a guess.
    let started = std::time::Instant::now();
    let answer = scope.search(&sq);
    let searched = started.elapsed();
    let columns = scope.columns(&sq);
    let facets = scope.facets(&sq);
    let listed = started.elapsed();
    let facet_counts: Vec<(String, (Vec<(String, u64)>, u64))> = facets.iter().map(|name| (name.clone(), scope.facet(&sq, name, 8))).collect();
    let counted = started.elapsed();
    if counted.as_millis() > 500 {
        eprintln!(
            "{}: slow page ({} ms: search {} ms, columns and facets {} ms, facet counts {} ms) for {url}",
            scope.decl.name,
            counted.as_millis(),
            searched.as_millis(),
            (listed - searched).as_millis(),
            (counted - listed).as_millis()
        );
    }
    let d = &scope.decl;

    let mut active = Vec::new();
    if let Some(pr) = &pred {
        flatten(pr, &mut active);
    }

    let examples: Vec<String> = scope
        .members
        .iter()
        .filter_map(|m| m.described["search"]["examples"].as_array().cloned())
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .take(4)
        .collect();

    let (holds, late) = scope.promise();
    // What a free reader cannot reach, said out loud on the page they are reading.
    let hidden = match &bound {
        Some(edge) => scope.records().saturating_sub(scope.reachable(Some(edge))),
        None => 0,
    };
    let stale = !holds || scope.members.iter().any(|m| m.state() != "current");
    // What the tracker store counted: how many things, how many only one source names, and
    // the conflicts. Current for everybody, as every count on this page is.
    let coverage = crate::thingstore::ThingStore::open(&scope.dir)
        .ok()
        .filter(|s| s.meta("refreshed").is_some())
        .map(|s| s.coverage());

    let one_kind = scope.kinds().len() <= 1;
    let columns: Vec<String> = columns.into_iter().filter(|c| !(one_kind && c == "kind")).collect();
    let several_sources = scope.members.len() > 1;
    let things_n = coverage.as_ref().and_then(|c| c["things"].as_i64()).unwrap_or(scope.records() as i64);
    let first_shown = (page - 1) * limit + 1;
    let last_shown = (page - 1) * limit + answer.entries.len();
    let pages = (answer.total as usize).div_ceil(limit).max(1);

    // This is the tracker itself, running, and the page says so above its name; its entry in the
    // hub, where somebody reads what it holds and takes a copy, is a link away.
    let world = Site::for_workspace(&scope.root);
    let entry = hub_entry(scope, &world);
    let body = html! {
        p.overline { span.live-dot {} "Live" @if !world.title.is_empty() { " · run by " (world.title) } }
        h1 { (d.title) }
        @if !d.about.is_empty() { p.lede { (d.about) } }
        @if let Some(entry) = &entry {
            p.entry-link { a href=(entry) { "Its entry in the hub · take a copy →" } }
        }
        div.meta {
            span.(if stale { "partial" } else { "current" }) { @if stale { "Behind" } @else { "Current" } }
            span { (plural(scope.members.len(), "source")) }
            @if let Some(p) = &d.package {
                span title=(if p.sealed { "What it answers with, every claim and its history; how it was made stays with whoever published it." } else { "" }) {
                    @if p.sealed { "sealed package " } @else { "package " } (p.version.get(..8).unwrap_or(&p.version))
                }
            }
            @if let Some(age) = scope.oldest_finish() { span { "updated " (crate::tracker::human(age)) " ago" } }
            @if let Some(f) = &d.promise.fresh_within {
                @if holds { span { "fresh within " (f) } } @else { span { "the promise of " (f) " does not hold" } }
            }
            @if is_operator(v) {
                @let settings = format!("{}settings", crate::serve::frame().home.1);
                @match crate::autoupdate::every(&scope.root) {
                    Some(s) => { a.dim href=(settings) { "checks " (crate::autoupdate::words(s)) @if let Some(at) = next_check(scope, s) { " · " @if at <= crate::now() { "due now" } @else { "next in " (crate::web::duration((at - crate::now()) as f64)) } } } }
                    None => { span { "updated by hand · " a href=(settings) { "turn on automatic updates" } } }
                }
            }
            @if is_operator(v) {
                form.update method="post" action=(at("/update")) { button type="submit" { "Update now" } }
            }
        }
        (banner(v, scope, hidden))
        @if is_operator(v) {
            @for (title, why) in paused(scope) {
                div.note { strong { (title) } " is " (why) ". " a href={(crate::serve::frame().home.1) "settings"} { "Try again" } }
            }
        }
        // Public, and a source that never said it may be: said on the page rather than hidden.
        // Not to the person at the machine: for them it is said where they publish.
        @let undeclared = if scope.private() || is_operator(v) { Vec::new() } else { scope.not_public() };
        @if !undeclared.is_empty() { div.note { "Not every source here has said it may be shown in public: " (with_titles(scope, &undeclared.join("; "))) "." } }
        @if let Some(c) = &coverage {
            @let by = c["by_sources"].as_object().cloned().unwrap_or_default();
            @let several: i64 = by.iter().filter(|(n, _)| n.parse::<i64>().unwrap_or(0) > 1).map(|(_, v)| v.as_i64().unwrap_or(0)).sum();
            @let conflicts = c["conflicts"].as_i64().unwrap_or(0);
            div.stats {
                a href=(at("/things")) { b { (thousands(things_n)) } span { "things" } }
                div { b { (thousands(several)) } span { "named by two sources or more" } }
                a.(if conflicts > 0 { "hot" } else { "" }) href=(at("/conflicts")) { b { (thousands(conflicts)) } span { "conflicts" } }
                div { b { (scope.members.len()) } span { (if scope.members.len() == 1 { "source" } else { "sources" }) } }
            }
            @if several_sources {
                @if let Some(only) = c["only"].as_object() {
                    p.dim { "Only one source knows: "
                        @for (i, (source, n)) in only.iter().enumerate() {
                            @if i > 0 { " · " } (title_of(scope, source)) " " (n)
                        }
                    }
                }
            }
        }
        @if !scope.missing.is_empty() {
            div.note {
                "Not installed here, and left out rather than refusing to open: "
                (scope.missing.join(", "))
            }
        }
        @if !late.is_empty() {
            div.note { "Behind: " (late.join("; ")) "." }
        }
        @if !d.promise.covers.is_empty() {
            div.note { strong { "Covers. " } (d.promise.covers)
                @if !d.promise.excludes.is_empty() { " " strong { "Excludes. " } (d.promise.excludes) } }
        }

        @let compared = compared(scope);
        @if !compared.is_empty() {
            details.compared open {
                summary { "Compared" span.dim { " · what is held against what, and from which column of each source" } }
                div.scroll {
                    table.compared-table {
                        thead { tr { th { "Property" } @for (title, _) in &compared[0].1 { th { (title) } } } }
                        tbody {
                            @for (name, per) in &compared {
                                tr {
                                    td { strong { (label(name)) } }
                                    @for (_, column) in per {
                                        td { @match column { Some(c) => code { (c) }, None => span.dim { "not said" } } }
                                    }
                                }
                            }
                        }
                    }
                }
                p.dim { "Everything else the sources say is shown side by side, and not compared." }
            }
        }
        form.bar method="get" action=(at("/")) {
            input type="search" name="q" value=(q) placeholder="Search, or filter with name=value";
            @if !view.is_empty() { input type="hidden" name="view" value=(view); }
            @if !kind.is_empty() { input type="hidden" name="kind" value=(kind); }
            button type="submit" { "Search" }
        }
        @if q.is_empty() && !examples.is_empty() {
            p.bar {
                span.dim { "Try:" }
                @for ex in &examples { a.chip rel="nofollow" href=(link(ex, &view, &kind, 1)) { (ex) } }
            }
        }
        @if !active.is_empty() {
            p.bar {
                @for (name, op, lit) in &active {
                    @let one = format!("{name}{}{}", op.sql(), lit.display());
                    @let without = q.replace(&one, "").split_whitespace()
                        .collect::<Vec<_>>().join(" ");
                    a.chip.on rel="nofollow" href=(link(&without, &view, &kind, 1)) {
                        (name) (op.sql()) (lit.display()) " ✕"
                    }
                }
            }
        }
        @if !answer.unanswered.is_empty() {
            div.note {
                (answer.answered.len()) " of " (scope.members.len()) " members answered. "
                @for (member, why) in &answer.unanswered {
                    (member) ": " (why.join("; ")) ". "
                }
            }
        }

        @if !one_kind || !d.view.named.is_empty() {
            p.bar {
                @if view.is_empty() && kind.is_empty() { span.chip.on { "Everything" } }
                @else { a.chip rel="nofollow" href=(link(&q, "", "", 1)) { "Everything" } }
                @for v in &d.view.named {
                    @let t = if v.title.is_empty() { v.name.clone() } else { v.title.clone() };
                    @if v.name == view { span.chip.on { (t) } }
                    @else { a.chip rel="nofollow" href=(link(&q, &v.name, "", 1)) { (t) } }
                }
                @if !one_kind {
                    @for (k, n) in scope.kinds() {
                        @if k == kind { span.chip.on { (k) " " (n) } }
                        @else { a.chip rel="nofollow" href=(link(&q, &view, &k, 1)) { (k) " " (n) } }
                    }
                }
            }
        }

        div.list-head {
            h2 { @if q.is_empty() { "Things" } @else { "Found" } }
            span.dim {
                @if answer.entries.is_empty() { "none" }
                @else if answer.subjects { (thousands(first_shown as i64)) "–" (thousands(last_shown as i64)) " of " @if answer.truncated { "at least " } (thousands(answer.total as i64)) }
                @else { (plural(answer.entries.len(), "thing")) ", from " @if answer.truncated { "at least " } (thousands(answer.total as i64)) " claims" }
            }
        }
        @if answer.truncated {
            div.note {
                "A source had more candidates than were read. The filter is applied over the "
                "assembled thing, so what is not read is not counted, and this number is a "
                "floor. Narrow the query to get an exact one."
            }
        }
        div.scroll {
            table.things {
                thead { tr {
                    th { "Thing" }
                    @for c in &columns { th { (label(c)) } }
                } }
                tbody {
                    @for e in &answer.entries {
                        tr {
                            td.thing {
                                @match entry_link(e) {
                                    Some(href) => a href=(href) { (e.title) },
                                    None => span { (e.title) },
                                }
                                div.why {
                                    @if let Some(k) = &e.key { span.mono { (k.value) } }
                                    @if several_sources { " · " (e.members().iter().map(|m| title_of(scope, m)).collect::<Vec<_>>().join(", ")) }
                                    @if !e.why.is_empty() { " · matched " (e.why.join(", ")) }
                                }
                            }
                            @for c in &columns { td { (cell(e, c)) } }
                        }
                    }
                }
            }
        }
        @if answer.entries.is_empty() { p.dim { "Nothing here." } }
        @if pages > 1 {
            div.pager {
                @if page > 1 { a rel="nofollow" href=(link(&q, &view, &kind, page - 1)) { "← Previous" } } @else { span.off { "← Previous" } }
                span.dim { "Page " (page) " of " (pages) }
                @if page < pages { a rel="nofollow" href=(link(&q, &view, &kind, page + 1)) { "Next →" } } @else { span.off { "Next →" } }
            }
        }

        h2 { "Facets" }
        div.grid {
            @for (name, (counts, coverage)) in &facet_counts {
                @if !counts.is_empty() {
                    div.card {
                        h4 {
                            (label(name)) " "
                            span.cover { (coverage) " of " (scope.records()) " claims" }
                        }
                        @for (v, n) in counts {
                            div.facet {
                                @let term = format!("{name}={v}");
                                @if q.split_whitespace().any(|t| t == term) { span.on { (v) } }
                                @else { a rel="nofollow" href=(link(&with_term(&q, &term), &view, &kind, 1)) { (v) } }
                                span.n { (n) }
                            }
                        }
                    }
                }
            }
        }

        @let open_to = proposable(scope, is_operator(v));
        h2 { "Sources" }
        div.grid {
            @for m in &scope.members {
                div.card {
                    h4 { (m.title()) " " span.cover { (m.decl.priority.name()) } }
                    p.dim { (m.decl.why) }
                    @if open_to.iter().any(|(n, _)| n == m.name()) {
                        p { a href=(at(&format!("/propose/{}", urlencode(m.name())))) { "Propose a row" } }
                    }
                    div.facet {
                        span { (m.kind()) " · " span.state.(m.state()) { (m.state()) } }
                        span.n { (m.records()) }
                    }
                }
            }
        }

        footer {
            (d.name) " · joined on " (d.keys().join(", "))
            " · " a href=(at("/changes")) { "changes" }
            " · " a href=(at("/catalogue")) { "catalogue" }
            @if !v.free { " · " a href=(at("/pricing")) { "pricing" } }
            " · " a href=(at("/terms")) { "terms" }
            " · " a href=(at("/api/describe")) { "describe" }
            @if !site.title.is_empty() { " · " (site.title) }
        }
    };
    shell(&d.title, body)
}

fn entry_page(scope: &Tracker, scheme: &str, value: &str, operator: bool, said: Option<&str>) -> Option<String> {
    let entry = scope.entry(scheme, value)?;
    // What the tracker store judges, which is what the lists and the signals judge too. A thing
    // page that decided for itself could call a conflict what the conflicts page calls wording.
    let key = crate::schemes::key(scheme, value);
    // Where the store has not looked yet, the page judges as it always did, rather than calling
    // everything agreed.
    let judged = crate::thingstore::ThingStore::open(&scope.dir)
        .ok()
        .filter(|s| s.meta("refreshed").is_some())
        .map(|s| s.conflicts_of(&key));
    let d = &scope.decl;
    // Each claim once, with its versions and its receipt. A thing has a handful of claims and
    // this page is one thing, so it is a handful of fetches and never a scan.
    let mut held: BTreeMap<(String, String), crate::claim::Claim> = BTreeMap::new();
    for p in &entry.parts {
        for c in scope.records_of(&p.member, std::slice::from_ref(&p.record_id), true) {
            held.insert((p.member.clone(), p.record_id.clone()), c);
        }
    }
    // The summary is a source's own text, the highest-priority source that has any, and says
    // whose it is. Nothing here is written by this program: a generated sentence would be a fact
    // with no receipt.
    // Never from a source whose terms allow a summary only: its text is not this page's to show.
    let summary = scope.members.iter().filter(|m| operator || scope.text_shown(m.name())).find_map(|m| {
        entry
            .parts
            .iter()
            .filter(|p| p.member == m.name())
            .filter_map(|p| held.get(&(p.member.clone(), p.record_id.clone())))
            .find(|c| !c.text.trim().is_empty())
            .map(|c| (said_by(scope, m.name()).0, c))
    });
    // The claim that says this value, of the ones this source holds about the thing.
    let saying = |member: &str, name: &str, raw: &str| {
        let parts: Vec<_> = entry.parts.iter().filter(|p| p.member == member).collect();
        parts
            .iter()
            .find(|p| p.fields.get(name).map(String::as_str) == Some(raw))
            .or_else(|| parts.iter().find(|p| p.fields.contains_key(name)))
            .and_then(|p| held.get(&(p.member.clone(), p.record_id.clone())))
    };
    // What the tracker says a page of one of its things reads first. Every value in it is one a
    // source said, named; the ladder and the timeline only order what the sources said.
    let view = &d.thing;
    let shaped = !view.is_empty();
    let conflicted = |name: &str| judged.as_ref().map(|j| j.iter().any(|c| c == name)).unwrap_or(false);
    let summary_rows: Vec<(&String, &crate::tracker::PropertyView)> = view.summary.iter().filter_map(|p| entry.fields.get(p).map(|f| (p, f))).collect();
    let ladder: Vec<(&crate::trackerdecl::Step, Option<Vec<String>>)> = view
        .ladder
        .as_ref()
        .map(|l| l.steps.iter().map(|s| (s, step_holds(&entry, &s.when))).collect())
        .unwrap_or_default();
    // The rung with no condition is where a thing stands while nothing above it holds, and only then:
    // "no public code" is not true of a thing with code.
    let above = |i: usize| ladder.iter().skip(i + 1).any(|(s, held)| held.is_some() && !s.when.trim().is_empty());
    let ladder: Vec<(&crate::trackerdecl::Step, Option<Vec<String>>)> = ladder
        .iter()
        .enumerate()
        .map(|(i, (s, held))| (*s, if s.when.trim().is_empty() && above(i) { None } else { held.clone() }))
        .collect();
    let top = ladder.iter().rposition(|(_, held)| held.is_some());
    let timeline = if shaped { timeline_of(scope, &entry, &view.timeline) } else { Vec::new() };
    // Where two sources score a thing differently and each wrote its vector, the metrics that differ.
    let explained: Vec<(String, Vec<(String, Vec<(String, String)>)>)> = entry
        .fields
        .keys()
        .filter(|name| shaped && conflicted(name))
        .filter_map(|name| {
            let vectors = entry.fields.get(&format!("{name}_vector"))?;
            let per: Vec<(String, Vec<(String, String)>)> = vectors.by.iter().filter_map(|(m, v)| Some((m.clone(), metrics_of(v.first()?)))).collect();
            (per.len() > 1).then(|| (name.clone(), per))
        })
        .collect();
    // A source that has no title of its own gives its description as one: shown whole further down.
    let long = entry.title.chars().count() > 140;
    let heading: String = if long { format!("{}…", entry.title.chars().take(140).collect::<String>().trim_end()) } else { entry.title.clone() };
    let values_table = html! {
    table {
        thead { tr { th { "Property" } th { "Source" } th { "Said" } th { "Means here" } } }
        tbody {
            @for (name, f) in &entry.fields {
                @for (member, said) in &f.by {
                    @let mapped = f.means.get(member).cloned().unwrap_or_default();
                    @for (i, raw) in said.iter().enumerate() {
                        tr {
                            td { (label(name)) div.why.mono { (name) }
                                @if judged.as_ref().map(|j| j.contains(name)).unwrap_or(f.divergent && f.by.len() > 1 && d.normalise_for(name).is_some()) { " " span.chip.on { "conflict" } }
                                @else if f.divergent && f.by.len() > 1 && d.normalise_for(name).is_none() { " " span.chip { "not compared" } }
                                @else if f.divergent && f.by.len() > 1 {
                                    @if f.means.values().flatten().all(|v| v.parse::<f64>().is_ok()) { " " span.chip { "within tolerance" } }
                                    @else { " " span.chip { "different words" } }
                                } }
                            td.dim { @if i == 0 { (title_of(scope, member)) } }
                            td { (raw)
                                @if let Some(means) = definition(scope, member, raw) {
                                    div.why { (means) }
                                }
                                @if let Some(c) = saying(member, name, raw) {
                                    @let (source, answered) = said_by(scope, member);
                                    (crate::serve::receipt(c, name, &source, answered.as_deref()))
                                }
                            }
                            td {
                                @let m = mapped.get(i).cloned().unwrap_or_default();
                                @if f.mapped && m != *raw { (m) } @else { span.dim { "—" } }
                            }
                        }
                    }
                }
            }
        }
    }
    };
    let claims_by_kind = html! {
@for (kind, parts) in entry.by_kind() {
    h2 { (kind) }
    table { tbody {
        @for p in parts {
            tr {
                td {
                    a href={(at("/claim/")) (urlencode(&p.member)) "/" (p.record_id)} { (p.title) }
                    div.why { (p.member) " · " (p.known) }
                }
                td {
                    @for (k, v) in &p.fields { span.chip { (k) " " (v) } " " }
                }
                td { @if let Some(u) = &p.url { a href=(u) { "source" } } }
            }
        }
    } }
}
    };
    let body = html! {
        h1 { (heading) }
        p.state { span.chip { (scheme) " " (value) } " "
            span.dim { (plural(entry.members().len(), "source")) ", " (plural(entry.parts.len(), "claim")) " · " }
            a href={(at("/thing/")) (urlencode(scheme)) "/" (urlencode(value)) ".atom"} { "Watch" } }
        @if let Some((source, c)) = &summary {
            div.note {
                span.dim { (source) " writes:" } br;
                @let text = c.text.trim();
                @if text.chars().count() > 600 { (text.chars().take(600).collect::<String>()) "…" } @else { (text) }
                " " a href={(at("/claim/")) (urlencode(&c.dataset)) "/" (c.record_id)} { "the claim" }
            }
        }

        @if shaped {
            @if !summary_rows.is_empty() {
                dl.thing-summary {
                    @for (name, f) in &summary_rows {
                        div.(if conflicted(name) { "hot" } else { "" }) {
                            dt { (label(name)) @if conflicted(name) { " " span.chip.on { "they disagree" } } }
                            dd { @for (member, said) in &f.by {
                                div { strong { (said.join(", ")) } " " span.src { (title_of(scope, member)) } }
                            } }
                        }
                    }
                }
            }
            @if let Some(l) = &view.ladder {
                h2 { (l.title) }
                ol.ladder {
                    @for (i, (step, held)) in ladder.iter().enumerate() {
                        li.(if Some(i) == top { "here" } else if held.is_some() { "reached" } else { "" }) {
                            span.rung { (step.name) }
                            @if let Some(who) = held {
                                @for m in who {
                                    span.dim { " · " (title_of(scope, m)) @if let Some((day, _)) = first_said(&entry, m) { " " (day) } }
                                }
                            }
                        }
                    }
                }
            }
            @for (name, per) in &explained {
                h2 { "Why the " (label(name)) " differs" }
                @let keys: Vec<String> = per.first().map(|(_, ms)| ms.iter().map(|(k, _)| k.clone()).collect()).unwrap_or_default();
                table.vector-diff {
                    thead { tr { th { "Metric" } @for (m, _) in per { th { (title_of(scope, m)) } } } }
                    tbody { @for k in &keys {
                        @let vals: Vec<String> = per.iter().map(|(_, ms)| ms.iter().find(|(mk, _)| mk == k).map(|(_, v)| v.clone()).unwrap_or_default()).collect();
                        @let differs = vals.windows(2).any(|w| w[0] != w[1]);
                        tr.(if differs { "hot" } else { "" }) {
                            td { (metric_words(k, "").0) span.dim { " " (k) } }
                            @for v in &vals { td { (metric_words(k, v).1) span.dim { " " (v) } } }
                        }
                    } }
                }
                p.dim { "Each source scores the same vulnerability from what it judges the attack to need. The rows marked are where they judge it differently." }
            }
            @if !timeline.is_empty() {
                h2 { "Timeline" }
                table.timeline { tbody { @for (day, who, what) in &timeline { tr { td.when { (day) } td { (what) } td.dim { (who) } } } } }
            }
            @if long { p.dim { (entry.title) } }
        }

        (relations_section(scope, &key, scheme, value, operator, said))
        @if shaped && !entry.fields.is_empty() {
            details.claims-all {
                summary { "Every value, with what each source said and its receipt" }
                (values_table)
            }
        }
        @if !shaped && !entry.fields.is_empty() {
            h2 { "What each source says" }
            (values_table)
        }

        @if shaped { details.claims-all { summary { "Every claim, by kind" } (claims_by_kind) } }
        @if !shaped { (claims_by_kind) }
    };
    Some(shell(&entry.title, body))
}

/// The source's own definition of its own word, which arrived with its `describe`.
/// A source's own title and when it last answered, from what it said about itself. For a
/// receipt, which names the source a value came from and how recently it was asked.
fn said_by(scope: &Tracker, member: &str) -> (String, Option<String>) {
    let Some(m) = scope.members.iter().find(|m| m.name() == member) else {
        return (member.to_string(), None);
    };
    let title = m.described["title"].as_str().unwrap_or(member).to_string();
    let answered = m.described["last_update"]["finished"].as_str().map(str::to_string);
    (title, answered)
}

fn definition(scope: &Tracker, member: &str, code: &str) -> Option<String> {
    let m = scope.members.iter().find(|m| m.name() == member)?;
    let vocab = m.described["vocabulary"].as_object()?;
    for (_, words) in vocab {
        if let Some(text) = words.get(code).and_then(J::as_str) {
            return Some(text.to_string());
        }
        if let Some(text) = words.get(code.to_lowercase()).and_then(J::as_str) {
            return Some(text.to_string());
        }
    }
    None
}

fn record_page(scope: &Tracker, member: &str, id: &str, operator: bool) -> Option<String> {
    let rec = scope
        .records_of(member, &[id.to_string()], true)
        .into_iter()
        .next()?;
    let (source, answered) = said_by(scope, member);
    let correctable = proposable(scope, operator).iter().any(|(n, _)| n == member);
    let body = html! {
        h1 { (rec.title) }
        @if correctable {
            p { a href={(at(&format!("/propose/{}", urlencode(member)))) "?claim=" (urlencode(&rec.record_id))} { "Propose a correction" } }
        }
        p.state { span.chip { (member) } " " span.chip { (rec.kind) } " "
            @for i in &rec.ids { span.chip { (i.scheme) " " (i.value) } " " }
            span.dim { "known " (rec.known) } }
        @if let Some(u) = &rec.url { p { a href=(u) { (u) } } }
        @if !rec.fields.is_empty() {
            h2 { "Properties" }
            table { tbody {
                @for (name, value) in &rec.fields {
                    tr {
                        th style="width: 12rem" { (name) }
                        td { (value.display())
                            @if let Some(t) = definition(scope, member, &value.display()) {
                                div.why { (t) }
                            }
                            (crate::serve::receipt(&rec, name, &source, answered.as_deref()))
                        }
                    }
                }
            } }
        }
        @if !rec.text.trim().is_empty() {
            h2 { "Text" }
            @if operator || scope.text_shown(member) {
                div.text { (rec.text) }
            } @else {
                p.dim { "This source's terms allow its title, its values and a link here, not its text."
                    @if let Some(u) = &rec.url { " It is at " a href=(u) { (u) } "." } }
            }
        }
        footer { "from " (rec.from.address()) " · " (rec.hash) }
    };
    Some(shell(&rec.title, body))
}

/// The second value is true when there is nothing at the address: no such call, or no such
/// thing. A call that does not exist, answered as a search of everything, is a caller who thinks
/// they asked something and gets the answer to another question.
fn api(scope: &Tracker, path: &str, url: &str, v: &Viewer) -> (J, bool) {
    let bound = account::bound(v, &scope.decl.name);
    // No API without a subscription. The overview and its counts stay current for everyone; the
    // claims behind them do not.
    if bound.is_some() {
        return (
            json!({ "error": "this needs a subscription", "see": at("/pricing") }),
            false,
        );
    }
    let p = params(url);
    let (text, pred) = expr::parse_query(p.get("q").map(String::as_str).unwrap_or(""));
    let sq = TrackerQuery {
        text,
        pred,
        named: p.get("view").cloned(),
        kind: p.get("kind").cloned(),
        sort: p.get("sort").cloned(),
        limit: p.get("limit").and_then(|s| s.parse().ok()).unwrap_or(25),
        offset: p.get("offset").and_then(|s| s.parse().ok()).unwrap_or(0),
        seen_before: bound.clone(),
    };
    let answer = match path {
        "/api/describe" => scope.describe(),
        "/api/mark" => json!({ "mark": scope.mark() }),
        "/api/changes" => {
            let since = p
                .get("since")
                .cloned()
                .unwrap_or_else(|| scope.mark_before());
            scope.changes(
                &since,
                p.get("limit").and_then(|s| s.parse().ok()).unwrap_or(200),
            )
        }
        "/api/facet" => {
            let field = p.get("property").cloned().unwrap_or_default();
            let (counts, coverage) = scope.facet(&sq, &field, 50);
            json!({ "property": field, "coverage": coverage, "claims": scope.records(),
                    "values": J::Array(counts.iter()
                        .map(|(v, n)| json!({ "value": v, "claims": n })).collect()) })
        }
        // One thing, which is the page a reader opens and had no call of its own.
        _ if path.starts_with("/api/thing/") => {
            let rest: Vec<&str> = path["/api/thing/".len()..].splitn(2, '/').collect();
            match rest.as_slice() {
                [scheme, value] => {
                    let value = crate::serve::urldecode(value);
                    match scope.entry(scheme, &value) {
                        Some(e) => entry_json(&e),
                        None => {
                            return (
                                json!({ "error": "no such thing",
                                        "scheme": scheme, "value": value }),
                                true,
                            )
                        }
                    }
                }
                _ => {
                    return (
                        json!({ "error": "a thing is named by a scheme and a value",
                                "example": at("/api/thing/cve/CVE-2021-44228") }),
                        true,
                    )
                }
            }
        }
        "/api/search" => {
            let answer = scope.search(&sq);
            json!({
                "total": answer.total,
                "counts": if answer.subjects { "things" } else { "claims" },
                "at_least": answer.truncated,
                "answered": answer.answered,
                "unanswered": J::Array(answer.unanswered.iter()
                    .map(|(m, w)| json!({ "source": m, "why": w })).collect()),
                "things": J::Array(answer.entries.iter().map(entry_json).collect()),
            })
        }
        _ => {
            return (
                json!({ "error": "no such call", "calls": [
                    "/api/describe", "/api/search", "/api/thing/{scheme}/{value}",
                    "/api/facet", "/api/changes", "/api/mark",
                ] }),
                true,
            )
        }
    };
    (answer, false)
}

/// One assembled thing, the same shape whether it arrives alone or inside a search.
fn entry_json(e: &crate::tracker::Thing) -> J {
    json!({
        "rank": e.rank,
        "identifier": e.key.as_ref().map(|k| json!({ "scheme": k.scheme, "value": k.value })),
        "title": e.title,
        "why": e.why,
        "claims": J::Array(e.parts.iter().map(|p| json!({
            "source": p.member, "kind": p.kind, "claim_id": p.record_id,
            "title": p.title, "url": p.url, "known": p.known,
        })).collect()),
        "properties": J::Object(e.fields.iter().map(|(name, f)| (name.clone(), json!({
            "by": f.by, "means": f.means, "conflict": f.divergent,
        }))).collect()),
    })
}

// ---------------------------------------------------------------------------------------------
// Who is asking, and what they are paying for.

fn banner(v: &Viewer, scope: &Tracker, hidden: u64) -> Markup {
    // The person at the machine owns all of it: nothing to sign in to, nothing to buy. Where
    // nothing costs anything there is nothing to say either.
    if (is_operator(v) && hidden == 0) || v.free {
        return html! {};
    }
    html! {
        p.bar {
            @match v.email() {
                Some(mail) => {
                    @if v.entitled(&scope.decl.name) {
                        span.chip.on { "subscribed" }
                    } @else {
                        span.chip { "free" }
                    }
                    @if v.by_key { span.chip { "by key" } }
                    @else { a.chip href=(at("/account")) { (mail) } }
                }
                None => {
                    span.chip { "free" }
                    a.chip href=(at("/signin")) { "Sign in" }
                }
            }
            @if hidden > 0 {
                a.chip href=(at("/pricing")) {
                    (hidden) " claims are newer than " (account::FREE_DELAY_DAYS)
                    " days and need a subscription"
                }
            }
        }
    }
}

fn signin_page(site: &Site, message: Option<&str>) -> String {
    signin_page_to(site, message, None)
}

/// The sign-in form, and where the link in the mail leads back to once it is followed.
pub(crate) fn signin_page_to(site: &Site, message: Option<&str>, next: Option<&str>) -> String {
    let body = html! {
        p { a href=(at("/")) { "←" } }
        h1 { "Sign in" }
        p.about {
            "Type your address and a link arrives. There is no password, so there is nothing to "
            "forget and nothing anybody can take."
        }
        @if let Some(m) = message { div.note { (m) } }
        form.bar method="post" action=(at("/signin")) {
            input type="search" name="email" placeholder="you@example.com";
            @if let Some(n) = next { input type="hidden" name="next" value=(n); }
            button type="submit" { "Send the link" }
        }
        // Or as somebody another world, or GitHub, Google or Apple, already knows.
        @let ways = crate::oidc::options(site);
        @if !ways.is_empty() {
            p { @for (id, label) in &ways { a.chip href=(signin_elsewhere(id, next)) { (label) } " " } }
        }
        @if !site.contact.is_empty() { p.dim { "Trouble: " (site.contact) } }
    };
    shell("Sign in", body)
}

fn account_page(scope: &Tracker, accounts: &Accounts, site: &Site, v: &Viewer) -> String {
    let Some(a) = v.account.clone() else {
        return signin_page(site, None);
    };
    let keys = accounts.keys(a.id);
    let entitled = a.entitled(&scope.decl.name);
    let body = html! {
        h1 { (a.email) }
        p.state.(if entitled { "current" } else { "empty" }) {
            (a.state)
            @if let Some(until) = &a.paid_until { " until " (until) }
            @if !a.scopes.is_empty() { " · " (a.scopes.join(", ")) }
        }
        @if !entitled && !v.free {
            div.note {
                "Reading is " (account::FREE_DELAY_DAYS) " days behind, and there are no change "
                "feeds, no API and no export. " a href=(at("/pricing")) { "What a subscription costs" } "."
            }
        }

        (proposals_section(scope, accounts, a.id))

        h2 { "API keys" }
        @if !entitled {
            p.dim { "A key answers only for a subscription." }
        } @else {
            table { tbody {
                @for (name, created, used) in &keys {
                    tr {
                        td { (name) }
                        td.dim { "made " (created) }
                        td.dim { @match used { Some(u) => (u), None => "never used" } }
                        td {
                            form method="post" action=(at("/account/key/drop")) {
                                input type="hidden" name="name" value=(name);
                                button type="submit" { "Revoke" }
                            }
                        }
                    }
                }
            } }
            @if keys.is_empty() { p.dim { "None yet." } }
            form.bar method="post" action=(at("/account/key")) {
                input type="search" name="name" placeholder="what this key is for";
                button type="submit" { "Make a key" }
            }
            p.dim { "Send it as " code { "Authorization: Bearer zk_…" } "." }
        }

        h2 { "Subscription" }
        @if entitled {
            form method="post" action=(at("/account/cancel")) {
                button type="submit" { "Cancel" }
            }
            p.dim {
                "Cancelling stops the next payment. What you already hold you keep until "
                @match &a.paid_until { Some(u) => (u), None => "the end of the period" } "."
            }
        } @else if !v.free {
            p { a href=(at("/pricing")) { "Subscribe" } }
        }

        p.bar { a href=(at("/signout")) { "Sign out" } }

        details {
            summary { "Close this account" }
            p.dim { "Your address, your name, your sessions, keys and sign-ins elsewhere are removed from this workspace. What you proposed stays in its sources under " code { "reader:…" } " and the name you gave, as a source's history is not rewritten, but nothing leads from it to you any more. Signing in again makes a new account." }
            form.bar method="post" action=(at("/account/close")) {
                label { input type="checkbox" name="sure" value="yes" required; " Close it" }
                button type="submit" { "Close the account" }
            }
        }
    };
    shell(&a.email, body)
}

fn key_made(key: &str) -> String {
    let body = html! {
        p { a href=(at("/account")) { "← account" } }
        h1 { "Your key" }
        div.note { "Shown once. Zetlyn keeps its hash and cannot show it again." }
        p { code { (key) } }
        p.dim { "Send it as " code { "Authorization: Bearer " (key) } "." }
    };
    shell("Your key", body)
}

fn pricing_page(_scope: &Tracker, site: &Site, v: &Viewer) -> String {
    // Only asked for where a price is named: the route answers 404 otherwise.
    let named = site.price.clone().unwrap_or_default();
    let p = &named;
    let body = html! {
        h1 { "What it costs" }
        p.about {
            "Free is the whole of this tracker, " (account::FREE_DELAY_DAYS) " days behind. "
            "A subscription is the same thing now, plus the part that is worth paying for: "
            "what changed since you last looked, delivered."
        }
        div.grid {
            div.card {
                h4 { "Free" }
                p.dim { "Nothing to sign." }
                div.facet { span { "Browse and search" } span.n { (account::FREE_DELAY_DAYS) " days behind" } }
                div.facet { span { "The overview and its counts" } span.n { "current" } }
                div.facet { span { "Change feeds" } span.n { "no" } }
                div.facet { span { "API and export" } span.n { "no" } }
            }
            div.card {
                h4 { (p.currency) (p.one) " a month" }
                p.dim { "One person, this tracker." }
                div.facet { span { "Browse and search" } span.n { "current" } }
                div.facet { span { "Change feeds" } span.n { "Atom, webhook, command" } }
                div.facet { span { "API and export" } span.n { "yes" } }
            }
            div.card {
                h4 { (p.currency) (p.team) " a month" }
                p.dim { "A team, every tracker this workspace serves." }
                div.facet { span { "Everything above" } span.n { "yes" } }
                div.facet { span { "Keys" } span.n { "as many as you need" } }
            }
        }
        @if p.buy.is_empty() {
            div.note {
                "Card payment is not connected on this workspace. Write to "
                @if site.contact.is_empty() { "the operator" } @else { (site.contact) }
                " and a subscription is set by hand, which is what the first ones are anyway."
            }
        } @else {
            p.bar { a.chip.on href=(p.buy) { "Subscribe" } }
        }
        p.dim {
            "What is sold is currency and coverage. "
            a href=(at("/terms")) { "Terms" } " · "
            @if v.email().is_some() { a href=(at("/account")) { "Your account" } }
            @else { a href=(at("/signin")) { "Sign in" } }
        }
    };
    shell("Pricing", body)
}

fn terms_page(scope: &Tracker, site: &Site) -> String {
    let who = if site.contact.is_empty() {
        "the operator of this workspace"
    } else {
        &site.contact
    };
    let body = html! {
        h1 { "Terms" }
        div.note {
            "A draft. It says what this workspace actually does, and it has not been read by a "
            "lawyer. Anybody selling from it should have one read it first."
        }
        h2 { "What is sold" }
        p { "A subscription to " (scope.decl.title) ": the records as they stand rather than "
            (account::FREE_DELAY_DAYS) " days behind, the change feeds, the API and export." }
        p { (scope.decl.promise.covers) }
        @if !scope.decl.promise.excludes.is_empty() { p { "Not included: " (scope.decl.promise.excludes) } }
        h2 { "What is not promised" }
        p { "No availability guarantee, and no undertaking about how fast a change at a publisher "
            "reaches this workspace beyond what the tracker states and shows. Every source is a third "
            "party and may change or stop without notice; where one does, the overview says so." }
        p { "The data is what publishers said. It is not advice, and nothing here decides which of "
            "two publishers is right." }
        h2 { "Money" }
        p { "Monthly, in advance, cancellable at any time and effective at the end of the paid "
            "period. No refund for a part-used month. Prices include VAT where it applies." }
        h2 { "What happens when you stop" }
        p { "Access to current claims, feeds, the API and export ends. Anything already exported "
            "stays yours; there is no recall." }
        h2 { "Your data" }
        p { "An address and, if you make them, API keys. No password is stored because none is "
            "asked for. The address is used to sign you in and to tell you about your subscription, "
            "and is deleted with the account on request." }
        h2 { "Third-party licences" }
        p { "Each source carries its own terms and is named on the overview. Nothing here relicenses "
            "what a publisher wrote." }
        p.dim { "Operated by " (who) "." }
    };
    shell("Terms", body)
}

/// Paid, because an export is the whole of what a subscriber holds.
fn export(scope: &Tracker, url: &str, v: &Viewer, as_csv: bool) -> Option<(String, &'static str)> {
    if !v.reads(&scope.decl.name) {
        return None;
    }
    let p = params(url);
    let (text, pred) = expr::parse_query(p.get("q").map(String::as_str).unwrap_or(""));
    let sq = TrackerQuery {
        text,
        pred,
        named: p.get("view").cloned(),
        kind: p.get("kind").cloned(),
        sort: None,
        limit: p.get("limit").and_then(|s| s.parse().ok()).unwrap_or(5000),
        offset: 0,
        seen_before: None,
    };
    let answer = scope.search(&sq);
    if !as_csv {
        let rows: Vec<J> = answer
            .entries
            .iter()
            .map(|e| {
                json!({
                    "identifier": e.key.as_ref().map(|k| format!("{}:{}", k.scheme, k.value)),
                    "title": e.title,
                    "sources": e.members(),
                    "properties": J::Object(e.fields.iter()
                        .map(|(n, f)| (n.clone(), json!({ "by": f.by, "means": f.means,
                                                          "conflict": f.divergent }))).collect()),
                })
            })
            .collect();
        return Some((J::Array(rows).to_string(), "application/json"));
    }
    let mut names: BTreeSet<String> = BTreeSet::new();
    for e in &answer.entries {
        names.extend(e.fields.keys().cloned());
    }
    let quote = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let mut out = String::from("identifier,title,sources");
    for n in &names {
        out.push(',');
        out.push_str(n);
    }
    out.push('\n');
    for e in &answer.entries {
        out.push_str(&quote(
            &e.key.as_ref().map(|k| k.value.clone()).unwrap_or_default(),
        ));
        out.push(',');
        out.push_str(&quote(&e.title));
        out.push(',');
        out.push_str(&quote(&e.members().join(" ")));
        for n in &names {
            out.push(',');
            let shown = e
                .fields
                .get(n)
                .map(|f| {
                    let mut v: Vec<&String> = f.means.values().flatten().collect();
                    v.sort();
                    v.dedup();
                    v.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" / ")
                })
                .unwrap_or_default();
            out.push_str(&quote(&shown));
        }
        out.push('\n');
    }
    Some((out, "text/csv; charset=utf-8"))
}

/// A browser encodes the name as well as the value, so `why_cve/kev` arrives as
/// `why_cve%2Fkev`. Decoding only the value finds nothing and says the field was empty.
pub fn form_field(body: &str, name: &str) -> String {
    body.split('&')
        .filter_map(|p| p.split_once('='))
        .find(|(k, _)| crate::serve::urldecode(k) == name)
        .map(|(_, v)| crate::serve::urldecode(v))
        .unwrap_or_default()
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn changes_page(scope: &Tracker, url: &str, v: &Viewer) -> String {
    let p = params(url);
    let before: Option<i64> = p.get("before").and_then(|s| s.parse().ok());
    let store = crate::thingstore::ThingStore::open(&scope.dir).ok();
    let all = store.as_ref().map(|s| s.signals(None, 5000)).unwrap_or_default();
    let signals: Vec<&J> = all
        .iter()
        .filter(|s| before.map(|b| s["id"].as_i64().unwrap_or(0) < b).unwrap_or(true))
        .take(300)
        .collect();
    // A signed-in reader sees what is new since they last opened this page. Only the first page
    // moves the mark: paging back through older signals is not reading the new ones.
    let latest = all.first().and_then(|s| s["id"].as_i64()).unwrap_or(0);
    let last_visit = match (v.email(), &store, before) {
        (Some(reader), Some(s), None) => s.visited(reader, latest),
        _ => None,
    };
    let fresh = |s: &J| last_visit.map(|w| s["id"].as_i64().unwrap_or(0) > w).unwrap_or(false);
    let new_count = signals.iter().filter(|s| fresh(s)).count();
    let today = crate::iso_date(crate::now());
    let yesterday = crate::iso_date(crate::now() - 86_400);
    // By day, newest first, as an inbox is read.
    let mut days: Vec<(String, Vec<&J>)> = Vec::new();
    for s in &signals {
        let day = s["at"].as_str().unwrap_or("").get(..10).unwrap_or("").to_string();
        match days.last_mut() {
            Some((d, list)) if *d == day => list.push(s),
            _ => days.push((day, vec![s])),
        }
    }
    let day_name = |day: &str| -> String {
        if day == today {
            "Today".into()
        } else if day == yesterday {
            "Yesterday".into()
        } else {
            day.to_string()
        }
    };
    let words = |kind: &str, n: usize| -> String {
        let (one, many) = match kind {
            "new_thing" => ("new thing", "new things"),
            "new_perspective" => ("new perspective", "new perspectives"),
            "changed" => ("change", "changes"),
            "conflict" => ("new conflict", "new conflicts"),
            "resolved" => ("conflict resolved", "conflicts resolved"),
            "withdrawn" => ("withdrawn", "withdrawn"),
            _ => ("source health", "source health"),
        };
        format!("{n} {}", if n == 1 { one } else { many })
    };
    let thing = |s: &J| -> Markup {
        let title = s["title"].as_str().filter(|t| !t.is_empty()).or(s["value"].as_str()).unwrap_or("");
        match (s["scheme"].as_str(), s["value"].as_str()) {
            (Some(scheme), Some(value)) => html! {
                a href={(at("/thing/")) (urlencode(scheme)) "/" (urlencode(value))} { (title) }
            },
            _ => html! { (s["key"].as_str().unwrap_or("")) },
        }
    };
    let list = |v: &J| -> String {
        match v {
            J::Array(a) => a.iter().filter_map(J::as_str).map(yes_no).collect::<Vec<_>>().join(", "),
            J::Object(o) => o
                .iter()
                .map(|(k, v)| format!("{} {}", title_of(scope, k), v.as_array().map(|a| a.iter().filter_map(J::as_str).map(yes_no).collect::<Vec<_>>().join(", ")).unwrap_or_default()))
                .collect::<Vec<_>>()
                .join(" · "),
            J::String(s) => yes_no(s).to_string(),
            _ => "—".into(),
        }
    };
    let body = html! {
        h1 { "Changes" }
        @if let Some(u) = p.get("updated") { div.note { strong { "Read again just now. " } (u) } }
        @if is_operator(v) {
            form.bar method="post" action=(at("/update")) {
                button.primary type="submit" { "Update now" }
                span.dim { "Reads every source of this tracker again, and shows here what changed." }
            }
        }
        p.bar {
            a.chip href=(at("/conflicts")) { "Conflicts" }
            a.chip href=(at("/changes.atom")) { "Atom" }
            a.chip href=(at("/things")) { "Ask about things" }
        }
        @if last_visit.is_some() {
            p.state { (new_count) " new since your last visit" }
        }
        @if store.is_none() || all.is_empty() {
            p.dim { "Nothing has changed since this tracker first looked at its sources. What changes from now on is kept here." }
        }
        @for (day, list_of) in &days {
            h2 { (day_name(day)) }
            @let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
            @for s in list_of { @let _ = { *kinds.entry(s["kind"].as_str().unwrap_or("")).or_default() += 1; }; }
            p.bar { @for (k, n) in &kinds { span.chip { (words(k, *n)) } " " } }
            table { tbody {
                @for s in list_of {
                    @let kind = s["kind"].as_str().unwrap_or("");
                    @let source = title_of(scope, s["source"].as_str().unwrap_or(""));
                    @let property = label(s["property"].as_str().unwrap_or(""));
                    tr {
                        td.dim style="width: 5rem" {
                            (s["at"].as_str().unwrap_or("").get(11..16).unwrap_or(""))
                            @if fresh(s) { " " span.chip.on { "new" } }
                        }
                        td {
                            @match kind {
                                "new_thing" => { "New: " (thing(s)) span.dim { " · first said by " (source) } }
                                "new_perspective" => { (source) " now speaks about " (thing(s)) }
                                "changed" => {
                                    (thing(s)) ": " strong { (property) } " at " (source) " "
                                    span.dim { (list(&s["was"])) } " → " strong { (list(&s["is"])) }
                                }
                                "conflict" => {
                                    span.chip.on { "conflict" } " " (thing(s)) ": sources now disagree about "
                                    strong { (property) } " " span.dim { (list(&s["is"])) }
                                }
                                "resolved" => { (thing(s)) ": sources agree again about " strong { (property) } }
                                "withdrawn" => {
                                    @if source.is_empty() { (list(&s["was"])) " is no longer said by any source" }
                                    @else { (source) " no longer says anything about " (thing(s)) }
                                }
                                "health" => { (source) ": " span.dim { (list(&s["was"])) } " → " strong { (list(&s["is"])) } }
                                _ => { (kind) }
                            }
                        }
                    }
                }
            } }
        }
        @if signals.len() == 300 {
            @if let Some(last) = signals.last().and_then(|s| s["id"].as_i64()) {
                p { a href={(at("/changes?before=")) (last)} { "Older" } }
            }
        }
    };
    shell("Changes", body)
}

fn watch_feed(scope: &Tracker, name: &str) -> Option<String> {
    let w = crate::watch::all(&scope.root)
        .into_iter()
        .find(|w| w.decl.name == name)?;
    let state = w.state();
    let title = if w.decl.title.is_empty() {
        w.decl.name.clone()
    } else {
        w.decl.title.clone()
    };
    let newest_first: Vec<J> = state.delivered.iter().rev().cloned().collect();
    Some(signal_atom(&title, &at(&format!("/watch/{name}.atom")), &newest_first))
}

/// How long a reading of the sources stands before it is taken again. The scheduler runs in
/// another process, so a surface that read its sources once would serve the counts it started
/// with for ever and say they were current.
const REREAD: i64 = 60;

pub fn serve(scope: Tracker, dir: &Path, datasets: &Path, addr: &str) -> Result<(), String> {
    let server = tiny_http::Server::http(addr).map_err(|e| e.to_string())?;
    println!("{} on http://{addr}", scope.decl.name);
    let mut site = TrackerSite::open(scope, dir, datasets, addr, false)?;
    if site.site.mail.run.is_empty() {
        println!("no mailer named in workspace.yaml, so sign-in links are printed here");
    }
    for request in server.incoming_requests() {
        site.answer(request);
    }
    Ok(())
}

/// One tracker answering requests: what `zetlyn serve` runs, and what the local app runs for each
/// tracker in a workspace under its own prefix.
pub struct TrackerSite {
    pub scope: Tracker,
    dir: std::path::PathBuf,
    datasets: std::path::PathBuf,
    accounts: Accounts,
    site: Site,
    read_at: i64,
    /// Where this is served, for a link written when the workspace names no address of its own.
    addr: String,
    /// The person at the machine. Locally there are no readers and nothing is paid for: the
    /// operator sees every page, and the paywall is for what is published.
    operator: bool,
    /// The pages drawn for somebody who carries no cookie and no key, by address, while the
    /// store and its statement stand as they were when the first was drawn.
    pages: Pages,
}

/// What a page drawn for nobody in particular is kept as: its body, its content type and the
/// one header beside it, and when it was drawn.
type Page = (String, String, Option<(String, String)>, i64);

/// The pages kept for anonymous readers. A reading that finds the store or the statement moved
/// empties it; so does every form sent, and an hour, for the "five minutes ago" a page says.
#[derive(Default)]
struct Pages {
    stamp: String,
    bytes: usize,
    kept: BTreeMap<String, Page>,
}

/// At most this much is kept per tracker, and no page larger than an eighth of it.
const PAGES_BYTES: usize = 32 << 20;
const PAGE_AGE: i64 = 3600;

impl Pages {
    fn get(&self, url: &str) -> Option<&Page> {
        self.kept.get(url).filter(|p| crate::now() - p.3 < PAGE_AGE)
    }

    fn keep(&mut self, url: &str, page: Page) {
        let size = url.len() + page.0.len();
        if size > PAGES_BYTES / 8 {
            return;
        }
        if self.bytes + size > PAGES_BYTES {
            self.forget();
        }
        self.bytes += size;
        if let Some(old) = self.kept.insert(url.to_string(), page) {
            self.bytes -= url.len() + old.0.len();
        }
    }

    fn forget(&mut self) {
        self.kept.clear();
        self.bytes = 0;
    }

    /// Empties itself when what the pages are drawn from has moved: a source run (the mark), or
    /// anything written in the tracker's directory, its statement and its store among them.
    fn still(&mut self, scope: &Tracker) {
        let written = std::fs::read_dir(&scope.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.metadata().ok()?.modified().ok())
            .max();
        let stamp = format!("{}|{:?}", scope.mark(), written);
        if stamp != self.stamp {
            self.forget();
            self.stamp = stamp;
        }
    }
}

impl TrackerSite {
    pub fn open(scope: Tracker, dir: &Path, datasets: &Path, addr: &str, operator: bool) -> Result<TrackerSite, String> {
        let accounts = Accounts::open(&scope.root)?;
        let site = Site::for_workspace(&scope.root);
        let scope = scope;
        // A tracker whose store is behind its sources, or has none, refreshes before it answers.
        if let Err(e) = scope.refresh_if_moved() {
            eprintln!("{}: the tracker store was not refreshed: {e}", dir.display());
        }
        let mut pages = Pages::default();
        pages.still(&scope);
        Ok(TrackerSite {
            scope,
            dir: dir.to_path_buf(),
            datasets: datasets.to_path_buf(),
            accounts,
            site,
            read_at: crate::now(),
            addr: addr.to_string(),
            operator,
            pages,
        })
    }

    /// One request. A failure is that request's, answered 500 as it is dropped, and never the
    /// end of the server.
    pub fn answer(&mut self, request: tiny_http::Request) {
        // Every page of it is in this tracker, with its tabs. Standing alone, the tracker is home.
        let tabs = vec![
            ("Overview".to_string(), at("/")),
            ("Things".to_string(), at("/things")),
            // What the person at the machine has not seen yet, on the tab that shows it; not on
            // that page itself, which is where it is being seen.
            (match (self.operator, request.url().contains("/changes")) {
                (true, false) => match crate::thingstore::ThingStore::open(&self.scope.dir).map(|s| s.unseen("you")) {
                    Ok(n) if n > 0 => format!("Changes · {n}"),
                    _ => "Changes".to_string(),
                },
                _ => "Changes".to_string(),
            }, at("/changes")),
            ("Conflicts".to_string(), at("/conflicts")),
        ];
        // Published, it has versions anybody can take, and that is a page of the tracker too.
        let mut tabs = tabs;
        // What its things are related to, each a tab: Vendors, Products.
        for r in &self.scope.decl.relations {
            if let Some(part) = r.part.as_deref().filter(|p| !p.is_empty()) {
                let plural = format!("{part}s");
                tabs.push((format!("{}{}", plural.get(..1).unwrap_or("").to_uppercase(), plural.get(1..).unwrap_or("")), at(&format!("/{plural}/"))));
            }
        }
        if self.scope.decl.inventory.is_some() {
            tabs.push(("Check yours".to_string(), at("/inventory")));
        }
        if hub_of(&self.scope, &Site::for_workspace(&self.scope.root)).is_some() {
            tabs.push(("Versions".to_string(), at("/versions")));
        }
        crate::serve::frame_section(Some((self.scope.decl.title.clone(), at("/"))), tabs);
        if let Err(e) = self.answer_or_fail(request) {
            eprintln!("{}: {e}", self.dir.display());
        }
        crate::serve::frame_section(None, Vec::new());
    }

    fn answer_or_fail(&mut self, mut request: tiny_http::Request) -> Result<(), String> {
        let TrackerSite { scope, dir, datasets, accounts, site, read_at, addr: site_addr, operator, pages } = self;
        let (dir, datasets) = (dir.as_path(), datasets.as_path());
        // Before anything is read off it, and only between requests, so no page is drawn from
        // two readings.
        if crate::now() - *read_at >= REREAD {
            match Tracker::open(dir, datasets) {
                Ok(fresh) => {
                    *scope = fresh;
                    // What changed since the last look becomes signals, once per update.
                    if let Err(e) = scope.refresh_if_moved() {
                        eprintln!("{}: the tracker store was not refreshed: {e}", dir.display());
                    }
                }
                Err(e) => eprintln!("{}: read again failed, serving the last one: {e}", dir.display()),
            }
            *read_at = crate::now();
            pages.still(scope);
        }
        let url = unmount(request.url());
        let path = url.split('?').next().unwrap_or("/").to_string();
        let parts: Vec<String> = path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(crate::serve::urldecode)
            .collect();
        let header = |name: &'static str| -> Option<String> {
            request
                .headers()
                .iter()
                .find(|h| h.field.equiv(name))
                .map(|h| h.value.as_str().to_string())
        };
        let cookie = header("Cookie");
        let authorization = header("Authorization");
        let mut v = if *operator {
            Viewer::operator()
        } else {
            account::viewer_of(&accounts, cookie.as_deref(), authorization.as_deref())
        };
        // A workspace that names no price charges nothing.
        v.free = site.price.is_none();
        // A private tracker, or one a source forbids showing in public, is its accounts' alone:
        // everything but signing in and what it costs is a page saying so.
        let closed = !*operator && (scope.private() || scope.licences().iter().any(|(_, r)| r == "no"));
        let open_anyway = matches!(path.as_str(), "/style.css" | "/zetlyn.css" | "/signin" | "/signout" | "/pricing" | "/terms" | "/account")
            || path.starts_with("/signin/");
        let path = if closed && !open_anyway && !v.entitled(&scope.decl.name) { "/private".to_string() } else { path };
        let post = request.method() == &tiny_http::Method::Post;
        let mut form = String::new();
        if post {
            // At most 16 MB: an SBOM of a large image, sent as a form, and not a disk's worth.
            let _ = std::io::Read::read_to_string(&mut std::io::Read::take(request.as_reader(), 16 << 20), &mut form);
            // A form sent may change what every page shows; none drawn before it is kept. A list
            // checked against the tracker changes nothing.
            if path != "/inventory" {
                pages.forget();
            }
        }
        // Somebody who carries no cookie and no key sees what anybody would, so the page drawn
        // for the last of them is theirs as well. Signing in, out and the account are not.
        let anonymous = !*operator
            && request.method() == &tiny_http::Method::Get
            && cookie.is_none()
            && authorization.is_none()
            && !["/signin", "/signout", "/account", "/watch"].iter().any(|p| path.starts_with(p));
        if anonymous {
            if let Some((body, kind, extra, _)) = pages.get(&url).cloned() {
                respond(request, 200, body, &kind, extra, "kept");
                return Ok(());
            }
        }

        // (body, content type, extra header)
        // A page that says "no such claim" under a 200 is telling a person one thing and
        // every machine another. 402 is the paywall, 404 is nothing there.
        let mut status = 200u16;
        let mut missing = false;
        let (body, kind, extra): (String, &str, Option<(String, String)>) = match path.as_str() {
            "/style.css" | "/zetlyn.css" => (
                crate::serve::STYLE.to_string(),
                "text/css; charset=utf-8",
                None,
            ),
            "/private" => {
                status = 401;
                (private_page(&scope, v.free), "text/html; charset=utf-8", None)
            }
            // For the website's front page: what the overview and a thing page already show to
            // anyone, as one answer another site may fetch. Nothing gated is in it, so it is open
            // to every origin and carries no cookie.
            "/demo.json" => (
                demo(&scope).to_string(),
                "application/json",
                Some(("Access-Control-Allow-Origin".to_string(), "*".to_string())),
            ),
            "/pricing" if site.price.is_some() => (
                pricing_page(&scope, &site, &v),
                "text/html; charset=utf-8",
                None,
            ),
            "/terms" => (terms_page(&scope, &site), "text/html; charset=utf-8", None),
            "/catalogue" => (
                catalogue(&scope, &v, None),
                "text/html; charset=utf-8",
                None,
            ),
            // Curation is a permission, checked here rather than only hidden in the form.
            "/catalogue/source" | "/catalogue/tracker"
                if post && !v.account.as_ref().is_some_and(|a| a.curator) =>
            {
                (
                    catalogue(&scope, &v, Some("That needs a curator.")),
                    "text/html; charset=utf-8",
                    None,
                )
            }
            "/catalogue/source" if post => {
                let said = match add_dataset(&scope, &form) {
                    Ok(m) => m,
                    Err(e) => e,
                };
                (
                    catalogue(&scope, &v, Some(&said)),
                    "text/html; charset=utf-8",
                    None,
                )
            }
            "/catalogue/tracker" if post => {
                let said = match add_scope(&scope, &form) {
                    Ok(m) => m,
                    Err(e) => e,
                };
                (
                    catalogue(&scope, &v, Some(&said)),
                    "text/html; charset=utf-8",
                    None,
                )
            }

            "/signin" if post => {
                let email = form_field(&form, "email");
                match accounts.ensure(&email).and_then(|a| accounts.new_link(a.id).map(|raw| (a, raw))) {
                    Ok((a, raw)) => {
                        let next = next_of(&form_field(&form, "next")).map(|n| format!("?next={}", urlencode(&n))).unwrap_or_default();
                        let here = at(&format!("/signin/{raw}{next}"));
                        let link = if site.url.is_empty() { format!("http://{site_addr}{here}") } else { site.link(&here) };
                        let sent = site
                            .send(
                                &a.email,
                                &crate::mail::signin_letter(&link).0,
                                &crate::mail::signin_letter(&link).1,
                            )
                            .unwrap_or(false);
                        let said = if sent {
                            "A link is on its way. It is good for a quarter of an hour, and once."
                        } else {
                            "This deployment names no mailer, so the link was printed on the \
                             server's own terminal."
                        };
                        (
                            signin_page(&site, Some(said)),
                            "text/html; charset=utf-8",
                            None,
                        )
                    }
                    Err(e) => (
                        signin_page(&site, Some(&e)),
                        "text/html; charset=utf-8",
                        None,
                    ),
                }
            }
            "/signin" => (signin_page_to(&site, None, params(&url).get("next").and_then(|n| next_of(n)).as_deref()), "text/html; charset=utf-8", None),

            "/signout" => {
                if let Some(c) = &cookie {
                    if let Some(s) = c
                        .split(';')
                        .filter_map(|p| p.trim().split_once('='))
                        .find(|(k, _)| *k == account::READER_COOKIE)
                    {
                        accounts.end_session(s.1);
                    }
                }
                (
                    shell(
                        "Signed out",
                        html! { p { a href=(at("/")) { "← back" } } h1 { "Signed out" } },
                    ),
                    "text/html; charset=utf-8",
                    Some((
                        "Set-Cookie".into(),
                        reader_cookie(site, "", 0),
                    )),
                )
            }

            "/account" => (
                account_page(&scope, &accounts, &site, &v),
                "text/html; charset=utf-8",
                None,
            ),

            "/account/key" if post => match &v.account {
                Some(a) if a.entitled(&scope.decl.name) => {
                    let name = form_field(&form, "name");
                    let name = if name.trim().is_empty() {
                        "a key".into()
                    } else {
                        name
                    };
                    let raw = accounts.new_key(a.id, &name)?;
                    (key_made(&raw), "text/html; charset=utf-8", None)
                }
                _ => (
                    pricing_page(&scope, &site, &v),
                    "text/html; charset=utf-8",
                    None,
                ),
            },
            // By the person, signed in as themselves, who said they are sure: never by a key.
            "/account/close" if post && !v.by_key && form_field(&form, "sure") == "yes" => match &v.account {
                Some(a) => match accounts.delete(a.id) {
                    Ok(()) => (
                        shell("Closed", html! { h1 { "The account is closed" } p { "Nothing of it is kept here any more." } p { a href=(at("/")) { "← back" } } }),
                        "text/html; charset=utf-8",
                        Some(("Set-Cookie".into(), reader_cookie(site, "", 0))),
                    ),
                    Err(e) => (shell("Not closed", html! { h1 { "The account was not closed" } p { (e) } }), "text/html; charset=utf-8", None),
                },
                None => (signin_page(&site, None), "text/html; charset=utf-8", None),
            },
            "/account/key/drop" if post => {
                if let Some(a) = &v.account {
                    accounts.drop_key(a.id, &form_field(&form, "name"));
                }
                (
                    account_page(&scope, &accounts, &site, &v),
                    "text/html; charset=utf-8",
                    None,
                )
            }
            "/account/cancel" if post => {
                if let Some(a) = &v.account {
                    // Cancelled, and what was paid for stays paid for. There is no recall.
                    accounts.set_subscription(
                        &a.email,
                        "cancelled",
                        a.paid_until.as_deref(),
                        &a.scopes,
                        None,
                    )?;
                }
                let mut again = account::viewer_of(&accounts, cookie.as_deref(), None);
                again.free = v.free;
                (
                    account_page(&scope, &accounts, &site, &again),
                    "text/html; charset=utf-8",
                    None,
                )
            }

            "/export.csv" | "/export.json" => {
                match export(&scope, &url, &v, path.ends_with(".csv")) {
                    Some((body, kind)) => (body, kind, None),
                    None => (
                        pricing_page(&scope, &site, &v),
                        "text/html; charset=utf-8",
                        None,
                    ),
                }
            }

            "/conflicts" | "/conflicts/mark" if !v.reads(&scope.decl.name) => (
                pricing_page(&scope, &site, &v),
                "text/html; charset=utf-8",
                None,
            ),
            "/conflicts" => (conflicts_page(&scope, &url, &v, None), "text/html; charset=utf-8", None),
            "/versions" => (versions_page(&scope, &Site::for_workspace(&scope.root)), "text/html; charset=utf-8", None),
            "/conflicts/mark" if post => {
                let said = match (v.email(), crate::thingstore::ThingStore::open(&scope.dir)) {
                    (Some(reader), Ok(store)) => store
                        .mark(reader, &form_field(&form, "key"), &form_field(&form, "property"), &form_field(&form, "state"))
                        .map(|_| "Marked. The mark is yours alone.".to_string())
                        .unwrap_or_else(|e| e),
                    (None, _) => "Sign in to mark a conflict.".to_string(),
                    (_, Err(e)) => e,
                };
                (conflicts_page(&scope, &url, &v, Some(&said)), "text/html; charset=utf-8", None)
            }
            "/changes" | "/changes.atom" if !v.reads(&scope.decl.name) => (
                pricing_page(&scope, &site, &v),
                "text/html; charset=utf-8",
                None,
            ),
            // Every source read again now, for the person at the machine: what changed since the
            // last look is then on the Changes page, without waiting for a cadence.
            "/update" if post && *operator => {
                let registry = crate::tracker::registry(datasets);
                let mut said: Vec<String> = Vec::new();
                // A packaged tracker is updated by taking the next package, not by reading.
                let packaged = scope.decl.package.is_some();
                if packaged {
                    let root = datasets.parent().unwrap_or(datasets).to_path_buf();
                    said.push(crate::package::pull(dir, &root).unwrap_or_else(|e| e));
                }
                for m in scope.decl.members.iter().filter(|_| !packaged) {
                    let Some(path) = registry.get(&m.dataset) else { continue };
                    let title = scope.members.iter().find(|r| r.name() == m.dataset).map(|r| r.title()).unwrap_or_else(|| m.dataset.clone());
                    // Beside an update in the background it would read the same files twice at once.
                    if crate::autoupdate::running() {
                        said.push("an automatic update is running right now; what it finds appears here".into());
                        break;
                    }
                    let Ok(ds) = crate::source::Source::open(path) else { continue };
                    let outcome = ds.run();
                    // Update now is also try again: a source paused after failures is asked afresh.
                    crate::autoupdate::record(&ds, &outcome);
                    match outcome {
                        Ok(r) if r.added + r.changed + r.removed == 0 => said.push(format!("{title}: nothing changed")),
                        Ok(r) => said.push(format!("{title}: {} new, {} changed, {} gone", r.added, r.changed, r.removed)),
                        Err(e) => said.push(format!("{title}: {e}")),
                    }
                }
                if let Ok(fresh) = Tracker::open(dir, datasets) {
                    *scope = fresh;
                    if let Err(e) = scope.refresh_if_moved() {
                        said.push(format!("the tracker was not refreshed: {e}"));
                    }
                    *read_at = crate::now();
                }
                let url = format!("/changes?updated={}", urlencode(&said.join(" · ")));
                (changes_page(scope, &url, &v), "text/html; charset=utf-8", None)
            }
            "/changes" => (changes_page(&scope, &url, &v), "text/html; charset=utf-8", None),
            "/changes.atom" => {
                let signals = crate::thingstore::ThingStore::open(&scope.dir)
                    .map(|s| s.signals(None, 200))
                    .unwrap_or_default();
                (
                    signal_atom(&scope.decl.title, &at("/changes.atom"), &signals),
                    "application/atom+xml; charset=utf-8",
                    None,
                )
            }
            "/things" | "/things.atom" if !v.reads(&scope.decl.name) => (
                pricing_page(&scope, &site, &v),
                "text/html; charset=utf-8",
                None,
            ),
            // In words, for the person at the machine: the assist is theirs, and so is what it is
            // sent. A reader of a published tracker types the filters.
            "/things/ask" if post && *operator => {
                let words = form_field(&form, "words");
                let assist = crate::assist::Assist::configured(&scope.root);
                let page = match crate::teach::translate(&assist, scope, &words, form_field(&form, "send") == "1") {
                    Ok(crate::teach::Outcome::NeedsConsent(d)) => consent_page(&words, &d),
                    Ok(crate::teach::Outcome::Done(t)) => match t.query {
                        Some(q) => things_page(scope, &format!("/things?q={}&asked={}", urlencode(&q), urlencode(&words)), true),
                        None => things_page(scope, &format!("/things?asked={}&refused={}", urlencode(&words), urlencode(&t.refused.unwrap_or_default())), true),
                    },
                    Err(e) => things_page(scope, &format!("/things?asked={}&refused={}", urlencode(&words), urlencode(&e)), true),
                };
                (page, "text/html; charset=utf-8", None)
            }
            // A question kept: a watch on it, in the workspace, with a feed.
            "/things/watch" if post && *operator => {
                let q = form_field(&form, "q");
                let title = form_field(&form, "title");
                let said = watch_question(scope, &q, &title);
                (things_page(scope, &format!("/things?q={}&watched={}", urlencode(&q), urlencode(&said)), true), "text/html; charset=utf-8", None)
            }
            "/things" => (things_page(&scope, &url, *operator), "text/html; charset=utf-8", None),
            "/things.atom" => {
                let question = params(&url).get("q").cloned().unwrap_or_default();
                match view_of(&scope, &question) {
                    Ok((_, signals)) => (
                        signal_atom(
                            &format!("{} — {question}", scope.decl.title),
                            &at(&format!("/things.atom?q={}", urlencode(&question))),
                            &signals,
                        ),
                        "application/atom+xml; charset=utf-8",
                        None,
                    ),
                    Err(e) => {
                        status = 400;
                        (e, "text/plain; charset=utf-8", None)
                    }
                }
            }

            _ if path.starts_with("/api/") => {
                let (answer, nothing_there) = api(&scope, &path, &url, &v);
                missing = nothing_there;
                (answer.to_string(), "application/json", None)
            }
            _ if parts.len() == 2 && parts[0] == "signin" => match accounts.spend_link(&parts[1], crate::account::Kind::Reader) {
                Some(session) => (
                    shell(
                        "Signed in",
                        html! {
                            p { a href=(at("/account")) { "→ your account" } } h1 { "Signed in" }
                            @if let Some(next) = params(&url).get("next").and_then(|n| next_of(n)) { p { a href=(at(&next)) { "Go on where you were" } } }
                        },
                    ),
                    "text/html; charset=utf-8",
                    Some((
                        "Set-Cookie".into(),
                        reader_cookie(site, &session, 2_592_000),
                    )),
                ),
                None => (
                    signin_page(
                        &site,
                        Some("That link was used, or it is older than a quarter of an hour."),
                    ),
                    "text/html; charset=utf-8",
                    None,
                ),
            },
            _ if parts.len() == 2 && parts[0] == "watch" => {
                if !v.reads(&scope.decl.name) {
                    (
                        pricing_page(&scope, &site, &v),
                        "text/html; charset=utf-8",
                        None,
                    )
                } else {
                    match watch_feed(&scope, parts[1].trim_end_matches(".atom")) {
                        Some(xml) => (xml, "application/atom+xml; charset=utf-8", None),
                        None => {
                            missing = true;
                            (
                                shell("Not here", html! { h1 { "No such watch" } }),
                                "text/html; charset=utf-8",
                                None,
                            )
                        }
                    }
                }
            }
            // A thing's own feed: every signal about it, for a reader who watches one thing.
            _ if parts.len() == 3 && parts[0] == "thing" && parts[2].ends_with(".atom") => {
                let value = parts[2].trim_end_matches(".atom");
                let key = crate::schemes::key(&parts[1], &value);
                let signals: Vec<J> = crate::thingstore::ThingStore::open(&scope.dir)
                    .map(|s| s.signals(None, 20_000))
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|s| s["key"].as_str() == Some(key.as_str()))
                    .take(200)
                    .collect();
                (
                    signal_atom(
                        &format!("{} — {} {value}", scope.decl.title, parts[1]),
                        &at(&format!("/thing/{}/{}.atom", parts[1], value)),
                        &signals,
                    ),
                    "application/atom+xml; charset=utf-8",
                    None,
                )
            }
            // A person's match, signed and kept, and the store made to hold it.
            _ if post && *operator && parts.len() == 4 && parts[0] == "thing" && parts[3] == "match" => {
                let key = crate::schemes::key(&parts[1], &parts[2]);
                let m = crate::matches::Match {
                    at: crate::iso_stamp(crate::now()),
                    by: form_field(&form, "by").trim().to_string(),
                    key,
                    relation: form_field(&form, "relation"),
                    target: form_field(&form, "target").trim().to_lowercase(),
                    why: form_field(&form, "why").trim().to_string(),
                    withdrawn: form_field(&form, "withdraw") == "1",
                };
                let known = scope.decl.relations.iter().any(|r| r.name == m.relation);
                let said = if !known {
                    format!("{}: this tracker has no such relation", m.relation)
                } else {
                    match crate::matches::record(&scope.dir, &m).and_then(|_| scope.refresh(false).map(|_| ())) {
                        Ok(()) if m.withdrawn => format!("Withdrawn. The match and its withdrawal both stay in {}.", crate::matches::FILE),
                        Ok(()) => format!("Kept: {} {} {}, confirmed by {}.", parts[2], m.relation, m.target, m.by),
                        Err(e) => e,
                    }
                };
                match entry_page(&scope, &parts[1], &parts[2], true, Some(&said)) {
                    Some(html) => (html, "text/html; charset=utf-8", None),
                    None => (shell("Not here", html! { h1 { "No such thing" } }), "text/html; charset=utf-8", None),
                }
            }
            _ if parts.len() == 3 && parts[0] == "thing" => {
                match entry_page(&scope, &parts[1], &parts[2], *operator, None) {
                    Some(html) => (html, "text/html; charset=utf-8", None),
                    None => {
                        missing = true;
                        (
                            shell("Not here", html! { h1 { "No such thing" } }),
                            "text/html; charset=utf-8",
                            None,
                        )
                    }
                }
            }
            // A row proposed from the browser, by a reader the workspace signs for.
            _ if parts.len() == 2 && parts[0] == "propose" => {
                let member = parts[1].clone();
                match proposable(scope, *operator).into_iter().find(|(m, _)| *m == member) {
                    None => {
                        missing = true;
                        (shell("Not here", html! { h1 { "Nothing here takes proposals" } }), "text/html; charset=utf-8", None)
                    }
                    Some((_, source)) => {
                        let mut reader = reader_of(scope, accounts, &v, *operator);
                        let may = reader.as_ref().map(|r| crate::propose::may(&source, r)).unwrap_or(Ok(()));
                        let page = match reader.as_mut() {
                            Some(r) if post && may.is_ok() => {
                                let account = if *operator { None } else { v.account.as_ref().map(|a| a.id) };
                                match take_from_form(scope, accounts, &source, r, account, &form) {
                                    Ok(_) => shell("Proposed", html! {
                                        p { a href=(at("/")) { "← " (scope.decl.title) } }
                                        h1 { "Proposed" }
                                        p.about { "It waits for the owner, and is part of this tracker once they accept it. "
                                            @if !*operator { "You get a mail when they decide." } }
                                        p.bar {
                                            a href=(at(&format!("/propose/{}", urlencode(&member)))) { "Propose another" }
                                            @if !*operator { " · " a href=(at("/account")) { "Your proposals" } }
                                        }
                                    }),
                                    Err(e) => {
                                        status = 400;
                                        let given: BTreeMap<String, String> = form
                                            .split('&')
                                            .filter_map(|p| p.split_once('='))
                                            .map(|(k, v)| (crate::serve::urldecode(k), crate::serve::urldecode(v)))
                                            .collect();
                                        propose_page(scope, &member, &source, Some(r), Ok(()), &url, Some(&e), &given)
                                    }
                                }
                            }
                            _ => {
                                if post {
                                    status = if reader.is_none() { 401 } else { 403 };
                                }
                                propose_page(scope, &member, &source, reader.as_ref(), may, &url, None, &BTreeMap::new())
                            }
                        };
                        (page, "text/html; charset=utf-8", None)
                    }
                }
            }
            "/inventory" if scope.decl.inventory.is_some() => {
                let decl = scope.decl.inventory.clone().unwrap_or_default();
                let sent = post.then(|| form_field(&form, "list"));
                (inventory_page(&scope, &decl, sent.as_deref()), "text/html; charset=utf-8", None)
            }
            _ if !parts.is_empty() && relation_at(&scope, &parts[0]).is_some() => {
                let r = relation_at(&scope, &parts[0]).expect("matched above");
                let noindex = url.contains('?').then(|| ("X-Robots-Tag".to_string(), "noindex, nofollow".to_string()));
                if parts.len() == 1 {
                    (related_index(&scope, r, &url), "text/html; charset=utf-8", noindex)
                } else {
                    match related_page(&scope, r, &parts[1..].join("/"), &url, *operator) {
                        Some(html) => (html, "text/html; charset=utf-8", noindex),
                        None => {
                            missing = true;
                            (shell("Not here", html! { h1 { "Nothing is related to that here" } }), "text/html; charset=utf-8", None)
                        }
                    }
                }
            }
            _ if parts.len() == 3 && parts[0] == "claim" => {
                match record_page(&scope, &parts[1], &parts[2], *operator) {
                    Some(html) => (html, "text/html; charset=utf-8", None),
                    None => {
                        missing = true;
                        (
                            shell("Not here", html! { h1 { "No such claim" } }),
                            "text/html; charset=utf-8",
                            None,
                        )
                    }
                }
            }
            // Only the front page is the front page. Every other address that matches nothing is
            // nothing, and says so with the number to match.
            _ if !parts.is_empty() => {
                missing = true;
                (
                    shell(
                        "Nothing here",
                        html! {
                            h1 { "Nothing here at that address" }
                        },
                    ),
                    "text/html; charset=utf-8",
                    None,
                )
            }
            // Its filtered views are not for search engines: endless, and each worked out as asked.
            _ => (
                overview(&scope, &url, &v, &site),
                "text/html; charset=utf-8",
                url.contains('?').then(|| ("X-Robots-Tag".to_string(), "noindex, nofollow".to_string())),
            ),
        };

        // A refusal and a miss have their own words in the body already; this gives them
        // the number as well. The four gated paths refuse on one predicate, so the number
        // comes from that predicate and not from reading the words back out of the body.
        let gated = path.starts_with("/api/")
            || path.starts_with("/export")
            || path.starts_with("/changes")
            || path.starts_with("/conflicts")
            || path.starts_with("/things")
            || path.starts_with("/watch/");
        if gated && !v.reads(&scope.decl.name) {
            status = 402;
        }
        if missing {
            status = 404;
        }
        // Kept only as anybody would see it: answered in full, and setting nobody's cookie.
        let kind = kind.to_string();
        if anonymous && status == 200 && !extra.as_ref().is_some_and(|(n, _)| n == "Set-Cookie") {
            pages.keep(&url, (body.clone(), kind.clone(), extra.clone(), crate::now()));
        }
        respond(request, status, body, &kind, extra, "drawn");
        Ok(())
    }
}

/// The answer, with its content type, the one header beside it, and whether it was drawn for
/// this request or kept from an earlier one.
fn respond(request: tiny_http::Request, status: u16, body: String, kind: &str, extra: Option<(String, String)>, page: &str) {
    let mut response = tiny_http::Response::from_string(body).with_status_code(status);
    let headers = [Some(("Content-Type".to_string(), kind.to_string())), extra, Some(("X-Zetlyn-Page".to_string(), page.to_string()))];
    for (name, value) in headers.into_iter().flatten() {
        if let Ok(h) = tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes()) {
            response = response.with_header(h);
        }
    }
    let _ = request.respond(response);
}

// ---------------------------------------------------------------------------------------------
// The catalogue: what this workspace holds, and what somebody may add to it.

fn form_fields(body: &str, name: &str) -> Vec<String> {
    body.split('&')
        .filter_map(|p| p.split_once('='))
        .filter(|(k, _)| crate::serve::urldecode(k) == name)
        .map(|(_, v)| crate::serve::urldecode(v))
        .filter(|v| !v.trim().is_empty())
        .collect()
}
fn slug(s: &str) -> String {
    crate::guess::slug(s.rsplit('/').next().unwrap_or(s))
}

fn catalogue(scope: &Tracker, v: &Viewer, message: Option<&str>) -> String {
    let root = &scope.root;
    let datasets = crate::tracker::registry(&root.join("sources"));
    let scopes = crate::tracker::scope_registry(&root.join("trackers"));
    let may = v.account.as_ref().is_some_and(|a| a.curator);

    let held: Vec<(String, String, u64, String)> = datasets
        .iter()
        .filter_map(|(name, dir)| {
            // Four values, not a description. A full describe() walks every field of every
            // source, and this page shows none of that.
            let ds = crate::source::Source::open(dir).ok()?;
            Some((
                name.clone(),
                ds.decl.kind.clone(),
                ds.store.count(),
                ds.state().to_string(),
            ))
        })
        .collect();

    let body = html! {
        h1 { "The catalogue" }
        p.about {
            "Every source and tracker this workspace holds. A source belongs to no tracker: several "
            "may name it, and the cost of a source is paid by whoever fetches it rather than by "
            "each tracker again."
        }
        @if let Some(m) = message { div.note { (m) } }

        h2 { "Sources" }
        table {
            thead { tr { th { "Name" } th { "Kind" } th { "Claims" } th { "State" } } }
            tbody {
                @for (name, kind, records, state) in &held {
                    tr {
                        td { (name) }
                        td.dim { (kind) }
                        td.num { (records) }
                        td { span.state.(state) { (state) } }
                    }
                }
            }
        }
        @if held.is_empty() { p.dim { "None." } }

        h2 { "Trackers" }
        ul {
            @for (name, _) in &scopes { li { (name) } }
        }
        @if scopes.is_empty() { p.dim { "None." } }

        @if !may {
            div.note {
                "Adding a source or composing a tracker needs a curator. "
                @match v.email() {
                    Some(mail) => { (mail) " is not one yet." }
                    None => { a href=(at("/signin")) { "Sign in" } " and ask the operator." }
                }
            }
        } @else {
            h2 { "Add a source" }
            p.dim {
                "A link to a CSV or to a feed. The declaration is proposed from what is behind it, "
                "and the scheduler fills the store on its next tick."
            }
            form.bar method="post" action=(at("/catalogue/source")) {
                input type="search" name="url" placeholder="https://…/something.csv";
                input type="search" name="name" placeholder="owner/name";
                input type="search" name="kind" placeholder="what one claim is";
                button type="submit" { "Propose it" }
            }

            h2 { "Compose a tracker" }
            p.dim {
                "Pick the sources and say what identifies the thing they talk about. Every "
                "source needs a sentence saying what it contributes that the others do not: a "
                "source nobody can justify in a sentence is one somebody added and nobody removed."
            }
            form method="post" action=(at("/catalogue/tracker")) {
                p.bar {
                    input type="search" name="name" placeholder="owner/name";
                    input type="search" name="title" placeholder="Title";
                    input type="search" name="identifier" placeholder="the identifier scheme, such as cve";
                }
                p.bar { input type="search" name="about" placeholder="What this tracker is about, in one sentence"; }
                table { tbody {
                    @for (name, kind, records, _) in &held {
                        tr {
                            td style="width: 2rem" {
                                input type="checkbox" name="source" value=(name);
                            }
                            td { (name) " " span.dim { (kind) " · " (records) } }
                            td { input type="search" name={"why_" (name)}
                                 placeholder="what it contributes that the others do not"; }
                        }
                    }
                } }
                p.bar { button type="submit" { "Compose it" } }
            }
        }
    };
    shell("The catalogue", body)
}

/// A link, a name, and the declaration is proposed from what is behind it.
fn add_dataset(scope: &Tracker, form: &str) -> Result<String, String> {
    let url = form_field(form, "url");
    if url.trim().is_empty() {
        return Err("a link is needed".into());
    }
    let name = form_field(form, "name");
    let kind = form_field(form, "kind");
    let dir = scope
        .root
        .join("sources")
        .join(slug(if name.trim().is_empty() { &url } else { &name }));
    if dir.join(crate::sourcedecl::FILE).exists() {
        return Err(format!("{} already holds a source", dir.display()));
    }
    crate::guess::propose_url(
        url.trim(),
        &dir,
        (!name.trim().is_empty()).then_some(name.trim()),
        (!kind.trim().is_empty()).then_some(kind.trim()),
    )?;
    Ok(format!(
        "{} is proposed. It holds nothing until a run fills it, and the scheduler does that on its \
         next tick.",
        dir.display()
    ))
}

/// Sources, a key, and a sentence each. The sentence is required here because it is required in
/// the format, and a form that let somebody skip it would be a way around the rule.
fn add_scope(scope: &Tracker, form: &str) -> Result<String, String> {
    let name = form_field(form, "name");
    if !name.contains('/') {
        return Err("a tracker is named owner/name".into());
    }
    let members = form_fields(form, "source");
    if members.is_empty() {
        return Err("a tracker with no sources is a tracker about nothing".into());
    }
    let mut missing = Vec::new();
    let mut whys = Vec::new();
    for m in &members {
        let why = form_field(form, &format!("why_{m}"));
        if why.trim().is_empty() {
            missing.push(m.clone());
        }
        whys.push(why);
    }
    if !missing.is_empty() {
        return Err(format!(
            "these sources have no sentence saying why they belong: {}",
            missing.join(", ")
        ));
    }

    let title = form_field(form, "title");
    let about = form_field(form, "about");
    let key = form_field(form, "identifier");
    let built = serde_json::json!({
        "name": name.trim(),
        "title": if title.trim().is_empty() { name.trim() } else { title.trim() },
        "about": about.trim(),
        "sources": members.iter().zip(&whys)
            .map(|(m, why)| serde_json::json!({ "source": m, "why": why.trim() }))
            .collect::<Vec<_>>(),
        "identified_by": if key.trim().is_empty() { vec![] } else { vec![key.trim()] },
        "view": { "columns": ["kind", "known"], "facets": ["kind", "source"] },
        "promise": { "fresh_within": "24h" },
    });
    // Read back as a declaration before it is written, as every declaration this program writes.
    let decl: crate::trackerdecl::TrackerDecl = serde_json::from_value(built)
        .map_err(|e| format!("that does not make a tracker: {e}"))?;

    let dir = scope.root.join("trackers").join(slug(name.trim()));
    if dir.join(crate::trackerdecl::FILE).exists() {
        return Err(format!("{} already holds a tracker", dir.display()));
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(crate::trackerdecl::FILE);
    crate::yaml::write(&path, &decl)?;
    Ok(format!(
        "{} is composed. Its promise says nothing yet, which is the one thing a curator has to \
         write themselves.",
        path.display()
    ))
}

/// Every open conflict, by property. A reader marks one seen or mutes it, for themselves; nobody
/// resolves one, because it is the sources' and resolves when they agree.
fn conflicts_page(scope: &Tracker, url: &str, v: &Viewer, said: Option<&str>) -> String {
    let p = params(url);
    let showing = p.get("state").cloned().unwrap_or_else(|| "open".into());
    let property = p.get("property").cloned();
    let store = crate::thingstore::ThingStore::open(&scope.dir).ok();
    let reader = v.email();
    let all = store.as_ref().map(|s| s.conflicts(reader, 100_000)).unwrap_or_default();
    let counts = store.as_ref().map(|s| s.conflict_counts()).unwrap_or_default();
    let shown: Vec<&J> = all
        .iter()
        .filter(|c| property.as_deref().map(|p| c["property"] == p).unwrap_or(true))
        .filter(|c| {
            let state = c["state"].as_str().unwrap_or("new");
            match showing.as_str() {
                "open" => state != "muted",
                "all" => true,
                s => state == s,
            }
        })
        .take(500)
        .collect();
    let new = all.iter().filter(|c| c["state"] == "new").count();
    let body = html! {
        h1 { "Conflicts" }
        @if let Some(m) = said { div.note { (m) } }
        p.state {
            (all.len()) " open" @if reader.is_some() { ", " (new) " new to you" } ". "
            span.dim { "Two or more sources say different things after the tracker's map. "
                "Where only the words differ and no map says what they mean together, it is "
                "counted as wording and not listed here." }
        }
        p.bar {
            a.chip.on[property.is_none()] href=(at("/conflicts")) { "every property" }
            @for (name, n) in &counts {
                " " a.chip.on[property.as_deref() == Some(name.as_str())]
                    href={(at("/conflicts?property=")) (urlencode(name))} { (label(name)) " " (n) }
            }
            " · "
            @for s in ["open", "new", "seen", "muted", "all"] {
                " " a.chip.on[showing == s] href={(at("/conflicts?state=")) (s)
                    @if let Some(p) = &property { "&property=" (urlencode(p)) }} { (s) }
            }
        }
        @if shown.is_empty() { p.dim { "None." } }
        table {
            @if !shown.is_empty() {
                thead { tr { th { "Thing" } th { "Property" } th { "What each source says" } th { "Since" } th {} } }
            }
            tbody {
                @for c in &shown {
                    @let scheme = c["scheme"].as_str().unwrap_or("");
                    @let value = c["value"].as_str().unwrap_or("");
                    @let state = c["state"].as_str().unwrap_or("new");
                    tr {
                        td {
                            a href={(at("/thing/")) (urlencode(scheme)) "/" (urlencode(value))} {
                                (c["title"].as_str().unwrap_or(value))
                            }
                            div.why.mono { (value) }
                        }
                        td { (label(c["property"].as_str().unwrap_or(""))) }
                        td {
                            @if let Some(o) = c["sources"].as_object() {
                                @for (source, words) in o {
                                    div { span.dim { (title_of(scope, source)) } " "
                                        strong { (words.as_array().map(|a| a.iter().filter_map(J::as_str).map(yes_no).collect::<Vec<_>>().join(", ")).unwrap_or_default()) } }
                                }
                            }
                        }
                        td.dim { (c["since"].as_str().unwrap_or("").get(..10).unwrap_or("")) }
                        td {
                            @if reader.is_some() {
                                form method="post" action=(at("/conflicts/mark")) {
                                    input type="hidden" name="key" value=(c["key"].as_str().unwrap_or(""));
                                    input type="hidden" name="property" value=(c["property"].as_str().unwrap_or(""));
                                    @if state == "new" {
                                        button name="state" value="seen" { "seen" } " "
                                    }
                                    @if state == "muted" {
                                        button name="state" value="new" { "unmute" }
                                    } @else {
                                        button name="state" value="muted" { "mute" }
                                    }
                                }
                            } @else if state != "new" { span.chip { (state) } }
                        }
                    }
                }
            }
        }
        @if reader.is_none() {
            p.dim { "Sign in to mark a conflict seen or mute it. The marks are yours and nobody else sees them." }
        }
    };
    shell("Conflicts", body)
}
/// Signals as Atom, newest first, for a feed reader. One entry per signal, named by its id, so
/// a reader that has seen it does not show it twice.
fn signal_atom(title: &str, self_url: &str, signals: &[J]) -> String {
    let updated = signals
        .first()
        .and_then(|s| s["at"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| crate::iso_stamp(crate::now()));
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    out.push_str("<feed xmlns=\"http://www.w3.org/2005/Atom\">\n");
    out.push_str(&format!("  <title>{}</title>\n", escape(title)));
    out.push_str(&format!("  <id>urn:zetlyn:{}</id>\n", escape(self_url)));
    out.push_str(&format!("  <updated>{updated}</updated>\n"));
    out.push_str(&format!("  <link rel=\"self\" href=\"{}\"/>\n", escape(self_url)));
    for s in signals {
        let said = crate::thingstore::say(s);
        let id = s["id"].as_i64().map(|i| i.to_string()).unwrap_or_else(|| {
            format!("{}:{}", s["kind"].as_str().unwrap_or(""), s["key"].as_str().unwrap_or(""))
        });
        out.push_str("  <entry>\n");
        out.push_str(&format!("    <title>{}</title>\n", escape(&said)));
        out.push_str(&format!("    <id>urn:zetlyn:{}:signal:{}</id>\n", escape(self_url), escape(&id)));
        out.push_str(&format!("    <updated>{}</updated>\n", s["at"].as_str().unwrap_or(&updated)));
        if let (Some(scheme), Some(value)) = (s["scheme"].as_str(), s["value"].as_str()) {
            out.push_str(&format!(
                "    <link href=\"{}\"/>\n",
                escape(&at(&format!("/thing/{}/{}", urlencode(scheme), urlencode(value))))
            ));
        }
        out.push_str(&format!("    <content type=\"text\">{}</content>\n", escape(&said)));
        out.push_str("  </entry>\n");
    }
    out.push_str("</feed>\n");
    out
}

/// The things a question holds for, from the tracker's store, and the signals about them.
/// The keys a question holds for, and nothing else: a page that lists things has no use for the
/// signals `view_of` reads beside them, twenty thousand of them for every page a crawler asks.
fn keys_where(scope: &Tracker, store: &crate::thingstore::ThingStore, question: &str) -> Result<Vec<String>, String> {
    let cx = scope.context();
    let q = crate::thingquery::parse(question, &cx)?;
    store.matching(&q, &cx)
}

fn view_of(scope: &Tracker, question: &str) -> Result<(Vec<String>, Vec<J>), String> {
    let cx = scope.context();
    let q = crate::thingquery::parse(question, &cx)?;
    let store = crate::thingstore::ThingStore::open(&scope.dir)?;
    let keys = store.matching(&q, &cx)?;
    let set: BTreeSet<&String> = keys.iter().collect();
    let signals = store
        .signals(None, 20_000)
        .into_iter()
        .filter(|s| s["key"].as_str().is_some_and(|k| set.contains(&k.to_string())))
        .take(200)
        .collect();
    Ok((keys, signals))
}


/// The things a question holds for, from the tracker's store: what a view is before it is saved,
/// and what a watch on it would hear about.
fn things_page(scope: &Tracker, url: &str, operator: bool) -> String {
    let p = params(url);
    let question = p.get("q").cloned().unwrap_or_default();
    let (asked, refused, watched) = (p.get("asked").cloned(), p.get("refused").cloned(), p.get("watched").cloned());
    let store = crate::thingstore::ThingStore::open(&scope.dir).ok();
    let answer = if question.trim().is_empty() { None } else { Some(view_of(scope, &question)) };
    let assist = operator.then(|| crate::assist::Assist::configured(&scope.root)).filter(|a| a.available());
    // Examples in this tracker's own words: its sources, a property they carry, its kind.
    let short = |name: &str| name.rsplit('/').next().unwrap_or(name).to_string();
    let sources: Vec<String> = scope.members.iter().map(|m| short(m.name())).collect();
    let property = scope
        .columns(&TrackerQuery::default())
        .into_iter()
        .find(|c| c != "kind" && c != "known");
    let kind = scope.kinds().first().map(|(k, _)| k.clone()).unwrap_or_else(|| "claim".into());
    let mut examples: Vec<String> = Vec::new();
    if let Some(p) = &property {
        if sources.len() > 1 {
            examples.push(format!("conflict:{p}"));
        }
        examples.push(format!("changed:{p}<24h"));
    }
    examples.push(format!("appeared:{kind}<7d"));
    if let Some(s) = sources.first() {
        examples.push(if sources.len() > 1 { format!("only:{s}") } else { format!("has:{s}") });
    }
    if let (Some(a), Some(b)) = (sources.first(), sources.get(1)) {
        examples.push(format!("has:{a} and not has:{b}"));
    }
    let placeholder = examples.first().cloned().unwrap_or_default();
    let terms = top_terms(&question);
    let body = html! {
        h1 { "Things" }
        @if let Some(a) = &assist {
            form.bar method="post" action=(at("/things/ask")) {
                input type="search" name="words" value=(asked.clone().unwrap_or_default())
                    placeholder="Ask in words: exploited, with a Metasploit module";
                button type="submit" { "Ask" }
            }
            p.dim { "Translated by " (a.who()) " into the filters below, which are what runs. It never answers in words." }
        }
        @if let Some(w) = &asked {
            @if let Some(r) = &refused {
                div.note { "“" (w) "” did not become a filter: " (r) }
            } @else {
                p.dim { "“" (w) "” became:" }
            }
        }
        form.bar method="get" action=(at("/things")) {
            input type="search" name="q" value=(question)
                placeholder=(placeholder);
            button type="submit" { "Filter" }
        }
        // Each part of the filter a chip, and the × takes that part away.
        @if terms.len() > 1 || (terms.len() == 1 && asked.is_some()) {
            p.bar {
                @for (i, t) in terms.iter().enumerate() {
                    @let rest = terms.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, t)| t.as_str()).collect::<Vec<_>>().join(" and ");
                    span.chip.on { (t) " " a href={(at("/things?q=")) (urlencode(&rest))} style="color:inherit" { "×" } } " "
                }
            }
        }
        @if question.trim().is_empty() {
            p.bar {
                span.dim { "Try:" }
                @for ex in examples {
                    " " a.chip href={(at("/things?q=")) (urlencode(&ex))} { (ex) }
                }
            }
        }
        @match &answer {
            None => {
                p.dim { "A question about things, asked of what this tracker holds: "
                    code { "conflict:" } " a property, " code { "has:" } " and " code { "only:" }
                    " a source, " code { "source.property=value" } ", " code { "appeared:kind<7d" }
                    ", " code { "changed:property<24h" } ", joined with and, or, not and parentheses." }
            }
            Some(Err(e)) => { div.note { (e) } }
            Some(Ok((keys, _))) => {
                p.state {
                    (keys.len()) @if keys.len() == 1 { " thing. " } @else { " things. " }
                    a href={(at("/things.atom?q=")) (urlencode(&question))} { "Its feed" }
                }
                @if operator {
                    @if let Some(w) = &watched {
                        div.note { (w) }
                    } @else {
                        form.bar method="post" action=(at("/things/watch")) {
                            input type="hidden" name="q" value=(question);
                            input type="text" name="title" value=(asked.clone().unwrap_or_default()) placeholder="What to call it";
                            button type="submit" { "Watch it" }
                        }
                    }
                }
                table { tbody {
                    @for key in keys.iter().take(500) {
                        @if let Some((title, scheme, value)) = store.as_ref().and_then(|s| s.named(key)) {
                            tr {
                                td {
                                    a href={(at("/thing/")) (urlencode(&scheme)) "/" (urlencode(&value))} { (clipped(&title, 140)) }
                                    div.why { (scheme) " " (value) }
                                }
                                td {
                                    @for p in store.as_ref().map(|s| s.conflicts_of(key)).unwrap_or_default() {
                                        span.chip.on { (p) } " "
                                    }
                                }
                            }
                        }
                    }
                } }
                @if keys.len() > 500 { p.dim { "The first 500 of " (keys.len()) "." } }
            }
        }
    };
    shell("Things", body)
}

/// A relation with `as:` has pages of its own, at the plural of what it is about: `/vendors/`
/// for one `as: vendor`. One without names a whole identifier, and is asked on the things page.
fn relation_at<'a>(scope: &'a Tracker, segment: &str) -> Option<&'a crate::trackerdecl::Relation> {
    scope.decl.relations.iter().find(|r| r.part.as_deref().is_some_and(|p| !p.is_empty() && format!("{p}s") == segment))
}

/// Where the page of one other side of a relation is, or None where it has none.
fn related_href(scope: &Tracker, name: &str, target: &str) -> Option<String> {
    let r = scope.decl.relations.iter().find(|r| r.name == name)?;
    let part = r.part.as_deref().filter(|p| !p.is_empty())?;
    let path: Vec<String> = target.split('/').map(urlencode).collect();
    Some(at(&format!("/{part}s/{}", path.join("/"))))
}

/// A title in a list, at most so many characters, cut at a word: NVD's titles are its whole
/// description where nobody gave a shorter name.
fn clipped(title: &str, at_most: usize) -> String {
    if title.chars().count() <= at_most {
        return title.to_string();
    }
    let cut: String = title.chars().take(at_most).collect();
    let cut = cut.rsplit_once(' ').map(|(head, _)| head.to_string()).unwrap_or(cut);
    format!("{}…", cut.trim_end_matches([',', ';', ':', '.', ' ']))
}

/// How a name spelled as an identifier reads: `palo_alto_networks` as palo alto networks.
fn spelled_out(target: &str) -> String {
    target.replace('_', " ")
}

/// The properties a list of things shows beside each, and counts over all of them: those of the
/// thing page's summary (or the facets) that have a scale, so a value is one of a few words.
fn scaled(scope: &Tracker) -> Vec<(String, Vec<String>)> {
    let named = if scope.decl.thing.summary.is_empty() { scope.decl.view.facets.clone() } else { scope.decl.thing.summary.clone() };
    named
        .into_iter()
        .filter_map(|p| {
            let scale = scope.decl.normalise.get(&p)?.scale.clone();
            (!scale.is_empty()).then_some((p, scale))
        })
        .collect()
}

/// Identifiers in the order a person counts them: CVE-2026-9999 before CVE-2026-10000.
fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    fn runs(s: &str) -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        for c in s.chars() {
            let digit = c.is_ascii_digit();
            match out.last_mut() {
                Some((d, run)) if *d == digit => run.push(c),
                _ => out.push((digit, c.to_string())),
            }
        }
        out
    }
    let (x, y) = (runs(a), runs(b));
    for ((dx, rx), (dy, ry)) in x.iter().zip(y.iter()) {
        let o = if *dx && *dy {
            let (tx, ty) = (rx.trim_start_matches('0'), ry.trim_start_matches('0'));
            tx.len().cmp(&ty.len()).then_with(|| tx.cmp(ty))
        } else {
            rx.to_lowercase().cmp(&ry.to_lowercase())
        };
        if o != std::cmp::Ordering::Equal {
            return o;
        }
    }
    x.len().cmp(&y.len())
}

/// Every other side of a relation: every vendor, with how many things each is related to.
fn related_index(scope: &Tracker, r: &crate::trackerdecl::Relation, url: &str) -> String {
    let part = r.part.clone().unwrap_or_default();
    let plural = format!("{part}s");
    let heading = format!("{}{}", plural.get(..1).unwrap_or("").to_uppercase(), plural.get(1..).unwrap_or(""));
    let filter = params(url).get("q").cloned().unwrap_or_default();
    let wanted = crate::trackerdecl::Relation::spell(&[filter.as_str()]);
    let all = crate::thingstore::ThingStore::open(&scope.dir).map(|s| s.targets(&r.name)).unwrap_or_default();
    let shown: Vec<&(String, u64)> = all.iter().filter(|(t, _)| wanted.is_empty() || t.contains(&wanted)).collect();
    const AT_MOST: usize = 300;
    let body = html! {
        h1 { (heading) }
        @if !r.about.is_empty() { p.lede { (r.about) } }
        form.bar method="get" action=(at(&format!("/{plural}/"))) {
            input type="search" name="q" value=(filter) placeholder={"Find a " (part)};
            button type="submit" { "Find" }
        }
        @if shown.is_empty() {
            p.dim { @if all.is_empty() { "No source relates anything here yet." } @else { "None has that in its name." } }
        } @else {
            table {
                thead { tr { th { (part) } th { "Things" } } }
                tbody { @for (target, n) in shown.iter().take(AT_MOST) { tr {
                    td { a href=(related_href(scope, &r.name, target).unwrap_or_default()) { (spelled_out(target)) } }
                    td { (thousands(*n as i64)) }
                } } }
            }
            @if shown.len() > AT_MOST { p.dim { "The " (AT_MOST) " with the most things, of " (thousands(shown.len() as i64)) ". Find the others by name." } }
        }
    };
    shell(&heading, body)
}

/// One other side of a relation and every thing related to it: a vendor and its vulnerabilities.
/// Counts first, each a filter; then its products, where it is a vendor; then the things, newest
/// first; and a way to be told, which is the things page's question for it.
fn related_page(scope: &Tracker, r: &crate::trackerdecl::Relation, target: &str, url: &str, operator: bool) -> Option<String> {
    let target = target.trim_matches('/').to_lowercase();
    let part = r.part.clone().unwrap_or_default();
    let store = crate::thingstore::ThingStore::open(&scope.dir).ok()?;
    let base = format!("{}:{target}", r.name);
    let all = keys_where(scope, &store, &base).ok()?;
    if all.is_empty() {
        return None;
    }
    let p = params(url);
    let filter = p.get("where").cloned().unwrap_or_default();
    let question = if filter.trim().is_empty() { base.clone() } else { format!("{base} and ({filter})") };
    let mut keys = if filter.trim().is_empty() { all.clone() } else { keys_where(scope, &store, &question).unwrap_or_default() };
    let values: BTreeMap<String, String> = keys.iter().filter_map(|k| Some((k.clone(), store.named(k)?.2))).collect();
    keys.sort_by(|a, b| natural(values.get(b).map(String::as_str).unwrap_or(b), values.get(a).map(String::as_str).unwrap_or(a)));
    // Over every thing of it, whatever the filter: what each source says, counted once per word.
    let props = scaled(scope);
    let counts: Vec<(String, Vec<(String, usize)>)> = props
        .iter()
        .map(|(prop, scale)| {
            let words = store.words_of(&all, prop);
            let n: Vec<(String, usize)> = scale
                .iter()
                .map(|w| (w.clone(), words.values().filter(|ws| ws.contains(w)).count()))
                .filter(|(_, n)| *n > 0)
                .collect();
            (prop.clone(), n)
        })
        .filter(|(_, n)| !n.is_empty())
        .collect();
    // A vendor's products, where the tracker relates things to products of the same identifier.
    let sibling = |as_: &str| scope.decl.relations.iter().find(|o| o.to == r.to && o.part.as_deref() == Some(as_)).map(|o| o.name.clone());
    let products_of = if part == "vendor" { sibling("product") } else { None };
    let products: Vec<(String, u64)> = products_of
        .as_ref()
        .map(|o| store.targets(o).into_iter().filter(|(t, _)| t.starts_with(&format!("{target}/"))).collect())
        .unwrap_or_default();
    // A product's vendor, the part before the slash, and the product by its own name.
    let vendor = match (part.as_str(), target.split_once('/')) {
        ("product", Some((v, _))) => sibling("vendor").map(|n| (n, v.to_string())),
        _ => None,
    };
    let name = match (part.as_str(), target.split_once('/')) {
        ("product", Some((_, p))) => spelled_out(p),
        _ => spelled_out(&target),
    };
    let said_by: Vec<String> = store.relating(&r.name, &target).iter().map(|s| title_of(scope, s)).collect();
    const PAGE: usize = 100;
    let page: usize = p.get("page").and_then(|n| n.parse().ok()).filter(|n| *n >= 1).unwrap_or(1);
    let pages = keys.len().div_ceil(PAGE).max(1);
    let here = related_href(scope, &r.name, &target).unwrap_or_default();
    let link = |filter: &str, page: usize| -> String {
        let mut q: Vec<String> = Vec::new();
        if !filter.is_empty() { q.push(format!("where={}", urlencode(filter))); }
        if page > 1 { q.push(format!("page={page}")); }
        if q.is_empty() { here.clone() } else { format!("{here}?{}", q.join("&")) }
    };
    let shown: Vec<String> = keys.iter().skip((page - 1) * PAGE).take(PAGE).cloned().collect();
    let beside: Vec<(String, Vec<String>, BTreeMap<String, BTreeSet<String>>)> =
        props.iter().map(|(prop, scale)| (prop.clone(), scale.clone(), store.words_of(&shown, prop))).collect();
    let body = html! {
        p.overline {
            (part)
            @if let Some((vname, v)) = &vendor {
                " of " a href=(related_href(scope, vname, v).unwrap_or_default()) { (spelled_out(v)) }
            }
        }
        h1 { (name) }
        div.meta {
            span { (thousands(all.len() as i64)) " things" }
            @if !said_by.is_empty() { span { "related by " (said_by.join(", ")) } }
        }
        @if !counts.is_empty() {
            div.thing-summary {
                @for (prop, n) in &counts {
                    div {
                        div.dim { (label(prop)) }
                        div.bar {
                            @for (w, c) in n {
                                @let term = format!("{prop}={w}");
                                @if filter == term {
                                    a.chip.on href=(link("", 1)) rel="nofollow" { (w) " " (c) " ×" }
                                } @else {
                                    a.chip href=(link(&term, 1)) rel="nofollow" { (w) " " (c) }
                                }
                                " "
                            }
                        }
                    }
                }
            }
        }
        @if !products.is_empty() {
            h2 { "Its products" }
            p.bar {
                @for (t, n) in products.iter().take(40) {
                    a.chip href=(products_of.as_deref().and_then(|pn| related_href(scope, pn, t)).unwrap_or_default()) {
                        (spelled_out(t.split_once('/').map(|(_, p)| p).unwrap_or(t))) " " (n)
                    }
                    " "
                }
            }
            @if products.len() > 40 { p.dim { "The 40 with the most things, of " (products.len()) "." } }
        }
        h2 { "Being told" }
        p.bar {
            a.chip href={(at("/things.atom?q=")) (urlencode(&question))} { "Watch: its feed" }
            " "
            a.chip href={(at("/things?q=")) (urlencode(&question))} rel="nofollow" { "Ask more of it" }
        }
        @if operator {
            form.bar method="post" action=(at("/things/watch")) {
                input type="hidden" name="q" value=(question);
                input type="text" name="title" value=(name) placeholder="What to call it";
                button type="submit" { "Watch it" }
            }
        }
        p.dim { "The feed says each thing that enters, leaves or changes; a reader adds its address, "
            code { (at("/things.atom?q=")) "…" } ", to theirs." }
        h2 {
            @if filter.is_empty() { "Every thing" } @else { (thousands(keys.len() as i64)) " where " code { (filter) } }
        }
        table { tbody {
            @for key in &shown {
                @if let Some((title, scheme, value)) = store.named(key) {
                    tr {
                        td {
                            a href={(at("/thing/")) (urlencode(&scheme)) "/" (urlencode(&value))} { (clipped(&title, 140)) }
                            div.why { (value) }
                        }
                        td {
                            @for (prop, scale, words) in &beside {
                                // The worst any source says, as the scale orders it.
                                @if let Some(w) = scale.iter().find(|w| words.get(key).is_some_and(|ws| ws.contains(*w))) {
                                    span.chip.on[Some(w) == scale.first()] { (label(prop)) " " (w) } " "
                                }
                            }
                        }
                    }
                }
            }
        } }
        @if pages > 1 {
            p.bar {
                @if page > 1 { a rel="nofollow" href=(link(&filter, page - 1)) { "← Previous" } } @else { span.off { "← Previous" } }
                " " (page) " of " (pages) " "
                @if page < pages { a rel="nofollow" href=(link(&filter, page + 1)) { "Next →" } } @else { span.off { "Next →" } }
            }
        }
    };
    Some(shell(&name, body))
}

/// The tracker's kind of thing, counted: one vulnerability, two vulnerabilities.
fn kind_word(scope: &Tracker, n: usize) -> String {
    // The kind its first source, the primary one, says: what the tracker is about, not what is written about it.
    let kind = scope.members.first().map(|m| m.kind().to_string()).filter(|k| !k.is_empty()).unwrap_or_else(|| "thing".into());
    match (n, kind.strip_suffix('y')) {
        (1, _) => kind,
        (_, Some(stem)) if !stem.ends_with(['a', 'e', 'o', 'u']) => format!("{stem}ies"),
        _ => format!("{kind}s"),
    }
}

/// What somebody runs, checked against the tracker: a form, and once sent, every thing of the
/// tracker a package or product of it is affected by, the version with the fix, and who says so.
/// What was sent is read, answered and dropped; nothing of it is written anywhere.
fn inventory_page(scope: &Tracker, decl: &crate::trackerdecl::Inventory, sent: Option<&str>) -> String {
    let list = sent.unwrap_or("");
    let form = html! {
        form method="post" action=(at("/inventory")) {
            p { textarea.wide #inv-list name="list" rows="12" spellcheck="false"
                placeholder="openssl-libs-3.0.7-27.el9.x86_64\npkg:npm/lodash@4.17.20\ncpe:2.3:a:apache:http_server:2.4.57:*:*:*:*:*:*:*" { (list) } }
            p.bar {
                input #inv-file type="file" accept=".json,.txt,.csv,.spdx,.cdx,text/plain,application/json";
                button type="submit" { "Check" }
            }
        }
        script { (maud::PreEscaped(r#"document.getElementById("inv-file").addEventListener("change",function(e){var f=e.target.files[0];if(f){f.text().then(function(t){document.getElementById("inv-list").value=t;});}});"#)) }
        p.dim {
            "A CycloneDX or SPDX SBOM as JSON, the output of " code { "rpm -qa" } ", package URLs ("
            code { "pkg:npm/lodash@4.17.20" } "), CPEs, " code { "name==version" } " from requirements.txt, or "
            code { "name version" } " a line. What you send is checked and dropped: nothing of it is kept, here or anywhere."
        }
    };
    let Some(text) = sent.filter(|t| !t.trim().is_empty()) else {
        let body = html! {
            h1 { "Check what you run" }
            @if !decl.about.is_empty() { p.lede { (decl.about) } }
            (form)
        };
        return shell("Check what you run", body);
    };
    let read = crate::inventory::read(text);
    let store = crate::thingstore::ThingStore::open(&scope.dir).ok();
    let findings = store
        .as_ref()
        .map(|s| crate::inventory::Known::of(s, decl).check(s, &read.items))
        .unwrap_or_default();
    let keys: Vec<String> = findings.iter().map(|f| f.key.clone()).collect::<BTreeSet<_>>().into_iter().collect();
    // What each thing is, by the tracker's scaled properties: the worst any source says.
    let props = scaled(scope);
    let words: Vec<(String, Vec<String>, BTreeMap<String, BTreeSet<String>>)> = props
        .iter()
        .map(|(p, scale)| (p.clone(), scale.clone(), store.as_ref().map(|s| s.words_of(&keys, p)).unwrap_or_default()))
        .collect();
    let worst = |key: &str| -> Vec<(String, usize, String, bool)> {
        words
            .iter()
            .filter_map(|(p, scale, w)| {
                let at = scale.iter().position(|v| w.get(key).is_some_and(|ws| ws.contains(v)))?;
                Some((p.clone(), at, scale[at].clone(), at == 0))
            })
            .collect()
    };
    // A yes-or-no property (exploited) orders first, then the others in the summary's order.
    let urgency = |key: &str| -> Vec<usize> {
        let w = worst(key);
        let rank = |p: &str, scale: &[String]| w.iter().find(|(q, ..)| q == p).map(|(_, at, ..)| *at).unwrap_or(scale.len());
        let mut order: Vec<usize> = props.iter().filter(|(_, s)| s.first().map(String::as_str) == Some("yes")).map(|(p, s)| rank(p, s)).collect();
        order.extend(props.iter().filter(|(_, s)| s.first().map(String::as_str) != Some("yes")).map(|(p, s)| rank(p, s)));
        order
    };
    let mut sure: Vec<&crate::inventory::Finding> = findings.iter().filter(|f| f.below_fix).collect();
    let mut unsure: Vec<&crate::inventory::Finding> = findings.iter().filter(|f| !f.below_fix).collect();
    sure.sort_by_cached_key(|f| (urgency(&f.key), f.item, f.key.clone()));
    unsure.sort_by_cached_key(|f| (urgency(&f.key), f.item, f.key.clone()));
    let affected_items = sure.iter().map(|f| f.item).collect::<BTreeSet<_>>().len();
    let sure_keys: Vec<String> = sure.iter().map(|f| f.key.clone()).collect::<BTreeSet<_>>().into_iter().collect();
    let counts: Vec<(String, Vec<(String, usize)>)> = words
        .iter()
        .map(|(p, scale, _)| {
            let n = scale
                .iter()
                .map(|v| (v.clone(), sure_keys.iter().filter(|k| worst(k).iter().any(|(q, _, w, _)| q == p && w == v)).count()))
                .filter(|(_, n)| *n > 0)
                .collect();
            (p.clone(), n)
        })
        .filter(|(_, n): &(String, Vec<(String, usize)>)| !n.is_empty())
        .collect();
    let row = |f: &crate::inventory::Finding| -> Markup {
        let item = &read.items[f.item];
        let named = store.as_ref().and_then(|s| s.named(&f.key));
        html! {
            tr {
                td { b { (item.label()) } div.why { (item.version()) } }
                td {
                    @if let Some((title, scheme, value)) = &named {
                        a href={(at("/thing/")) (urlencode(scheme)) "/" (urlencode(value))} { (value) }
                        div.why { (clipped(title, 100)) }
                    } @else { (f.key) }
                }
                td { @for (p, _, w, first) in worst(&f.key) { span.chip.on[first] { (label(&p)) " " (w) } " " } }
                td { @if f.fixed.is_empty() { span.dim { "—" } } @else { code { (f.fixed) } } }
                td.dim { (title_of(scope, &f.source)) }
            }
        }
    };
    let body = html! {
        h1 { "What you run" }
        div.meta {
            span { (plural(read.items.len(), "package")) " read" @if !read.format.is_empty() { " (" (read.format) ")" } }
            span { (affected_items) " affected" }
            span { (sure_keys.len()) " " (kind_word(scope, sure_keys.len())) }
            @if !read.unread.is_empty() { span { (read.unread.len()) " lines not understood" } }
        }
        @if !counts.is_empty() {
            div.thing-summary {
                @for (p, n) in &counts {
                    div { div.dim { (label(p)) } div { @for (w, c) in n { span.chip { (w) " " (c) } " " } } }
                }
            }
        }
        @if sure.is_empty() {
            p { "Nothing you run is below a fix any source here names." }
        } @else {
            h2 { "Below the fix" }
            p.dim { "The version you run is older than the one each source says fixes it. Most urgent first." }
            table {
                thead { tr { th { "Package" } th { "Vulnerability" } th {} th { "Fixed in" } th { "Said by" } } }
                tbody { @for f in &sure { (row(f)) } }
            }
        }
        @if !unsure.is_empty() {
            h2 { "Named, not compared" }
            p.dim { "A source names the product, or you gave no version, so which of its versions are affected is for you to look up on the vulnerability's page." }
            details {
                summary { "Show " (unsure.len()) }
                table {
                    thead { tr { th { "Package" } th { "Vulnerability" } th {} th { "Fixed in" } th { "Said by" } } }
                    tbody { @for f in unsure.iter().take(500) { (row(f)) } }
                }
            }
        }
        @if !read.unread.is_empty() {
            details {
                summary { "Lines not understood" }
                pre { @for l in read.unread.iter().take(50) { (l) "\n" } }
            }
        }
        h2 { "Check again" }
        (form)
    };
    shell("What you run", body)
}

/// A filter's parts joined by `and` at its top level, which is what a chip can take away without
/// changing what the rest means. Inside parentheses, or under `or`, it is one part.
fn top_terms(q: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    let words: Vec<&str> = q.split_whitespace().collect();
    if words.iter().any(|w| w.eq_ignore_ascii_case("or")) && !q.contains('(') {
        return if q.trim().is_empty() { Vec::new() } else { vec![q.trim().to_string()] };
    }
    for w in words {
        depth += w.matches('(').count() as i32 - w.matches(')').count() as i32;
        if depth == 0 && w.eq_ignore_ascii_case("and") {
            if !cur.trim().is_empty() {
                out.push(cur.trim().to_string());
            }
            cur.clear();
            continue;
        }
        cur.push_str(w);
        cur.push(' ');
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// What the assist would be sent to translate a question, and the button that sends it (D10).
fn consent_page(words: &str, d: &crate::assist::Disclosure) -> String {
    shell("Ask in words", html! {
        h1 { "Before it is asked" }
        div.note {
            "Translating “" (words) "” sends " strong { (d.to) } ":"
            ul { @for s in &d.sends { li { (s) } } }
            "Asked once for this tracker; what is sent is written in its " code { "assist.yaml" } "."
        }
        form method="post" action=(at("/things/ask")) {
            input type="hidden" name="words" value=(words);
            input type="hidden" name="send" value="1";
            button type="submit" { "Send it" }
        }
    })
}

/// A watch file for a question, in the workspace's `watches/`, delivered as a feed.
fn watch_question(scope: &Tracker, q: &str, title: &str) -> String {
    let cx = scope.context();
    if let Err(e) = crate::thingquery::parse(q, &cx) {
        return e;
    }
    let dir = scope.root.join("watches");
    let base = crate::guess::slug(if title.trim().is_empty() { q } else { title }).replace('_', "-");
    let base: String = base.chars().take(48).collect();
    let mut name = if base.is_empty() { "question".to_string() } else { base };
    let mut n = 2;
    while dir.join(format!("{name}.yaml")).exists() {
        name = format!("{}-{n}", name.trim_end_matches(char::is_numeric).trim_end_matches('-'));
        n += 1;
    }
    let decl = serde_json::json!({
        "name": name,
        "title": if title.trim().is_empty() { q.to_string() } else { title.trim().to_string() },
        "tracker": scope.decl.name,
        "query": q,
        "deliver": [{ "to": "feed" }],
    });
    let written = serde_json::from_value::<crate::watch::WatchDecl>(decl)
        .map_err(|e| e.to_string())
        .and_then(|w| {
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            crate::yaml::write(&dir.join(format!("{name}.yaml")), &w)
        });
    match written {
        Ok(()) => format!("Watched as {name}: its first check remembers what it holds, and each one after says what entered, what left and what changed. The feed is {}.", at(&format!("/watch/{name}.atom"))),
        Err(e) => e,
    }
}

/// The tracker as the website shows it: its counts, its sources, how much it noticed today, and
/// one thing its sources disagree about, with what each of them says. Every figure is one a
/// public page of this tracker shows, and each carries the address that shows it.
fn demo(scope: &Tracker) -> J {
    let Ok(store) = crate::thingstore::ThingStore::open(&scope.dir) else {
        return json!({ "error": "this tracker has no store yet" });
    };
    let title_of = |name: &str| -> String {
        scope.members.iter().find(|m| m.name() == name).map(|m| said_by(scope, m.name()).0).unwrap_or_else(|| name.to_string())
    };
    let per_source = store.things_per_source();
    let day = crate::iso_date(crate::now());
    let mut today: BTreeMap<String, u64> = BTreeMap::new();
    for s in store.signals(None, 20_000) {
        if s["at"].as_str().is_some_and(|a| a.starts_with(&day)) {
            *today.entry(s["kind"].as_str().unwrap_or("").to_string()).or_default() += 1;
        }
    }
    let prefer = scope.members.iter().find(|m| m.name().ends_with("-kev")).map(|m| m.name().to_string()).unwrap_or_default();
    let thing = store.showcase(&prefer).and_then(|key| {
        let (title, scheme, value) = store.named(&key)?;
        let conflicts = store.conflicts_of(&key);
        let mut by: BTreeMap<String, Vec<J>> = BTreeMap::new();
        for (source, property, raw, means) in store.said_of(&key) {
            by.entry(property).or_default().push(json!({ "source": title_of(&source), "said": raw, "means": means }));
        }
        let values: Vec<J> = by
            .into_iter()
            .map(|(p, said)| json!({ "property": p, "conflict": conflicts.contains(&p), "by": said }))
            .collect();
        Some(json!({
            "title": title, "scheme": scheme, "value": value,
            "page": at(&format!("/thing/{}/{}", urlencode(&scheme), urlencode(&value))),
            "sources": store.speakers(&key).iter().map(|s| title_of(s)).collect::<Vec<_>>(),
            "conflicts": conflicts,
            "values": values,
        }))
    });
    json!({
        "tracker": { "name": scope.decl.name, "title": scope.decl.title, "page": at("/") },
        "taken": crate::iso_stamp(crate::now()),
        "counts": store.coverage(),
        "sources": scope.members.iter().map(|m| json!({
            "name": m.name(), "title": said_by(scope, m.name()).0, "why": m.decl.why,
            "claims": m.described["claims"], "things": per_source.get(m.name()).copied().unwrap_or(0),
        })).collect::<Vec<_>>(),
        "today": today,
        "thing": thing,
    })
}

/// What a thing is to other things: every relation a source states, every match a person signed,
/// and what a source says only in words, offered for a person to confirm and never counted.
fn relations_section(scope: &Tracker, key: &str, scheme: &str, value: &str, operator: bool, said: Option<&str>) -> Markup {
    if scope.decl.relations.is_empty() {
        return html! {};
    }
    let Ok(store) = crate::thingstore::ThingStore::open(&scope.dir) else { return html! {} };
    let related = store.related_of(key);
    let signed: Vec<crate::matches::Match> = crate::matches::standing(&scope.dir).into_iter().filter(|m| m.key == key).collect();
    let title_of = |source: &str| -> String {
        match source.strip_prefix("person:") {
            Some(p) => format!("confirmed by {p}"),
            None => said_by(scope, source).0,
        }
    };
    // Suggestions, per source and per relation: the words it uses, spelled as the relation spells
    // an identifier, where no source or person already relates it so.
    let words = store.said_of(key);
    let mut suggested: Vec<(String, String, String)> = Vec::new();
    for r in scope.decl.relations.iter().filter(|r| !r.suggest_from.is_empty()) {
        let sources: BTreeSet<&String> = words.iter().map(|(s, ..)| s).collect();
        for source in sources {
            let lists: Vec<Vec<String>> = r
                .suggest_from
                .iter()
                .map(|p| words.iter().filter(|(s, prop, ..)| s == source && prop == p).flat_map(|(_, _, raw, _)| raw.clone()).collect())
                .collect();
            if lists.iter().any(|l| l.is_empty()) {
                continue;
            }
            // Every combination, a handful at most: which vendor goes with which product is the
            // person's to say, not this page's.
            let mut combos: Vec<Vec<String>> = vec![Vec::new()];
            for l in &lists {
                combos = combos.into_iter().flat_map(|c| l.iter().map(move |w| { let mut c = c.clone(); c.push(w.clone()); c })).take(6).collect();
            }
            for c in combos {
                let refs: Vec<&str> = c.iter().map(String::as_str).collect();
                let target = crate::trackerdecl::Relation::spell(&refs);
                let held = related.get(&r.name).is_some_and(|t| t.contains_key(&target));
                if !target.is_empty() && !held && !suggested.iter().any(|(n, t, _)| *n == r.name && *t == target) {
                    suggested.push((r.name.clone(), target, format!("{} says “{}”", said_by(scope, source).0, c.join(" · "))));
                }
            }
        }
    }
    if related.is_empty() && suggested.is_empty() {
        return html! {};
    }
    let action = format!("{}/{}/match", urlencode(scheme), urlencode(value));
    html! {
        h2 { "What it is to other things" }
        @if let Some(s) = said { div.note { (s) } }
        table { tbody {
            @for (name, targets) in &related {
                @for (target, who) in targets {
                    tr {
                        td { (name) }
                        td {
                            a href=(related_href(scope, name, target).unwrap_or_else(|| format!("{}{}", at("/things?q="), urlencode(&format!("{name}:{target}"))))) { (target) }
                            div.why { (who.iter().map(|w| title_of(w)).collect::<Vec<_>>().join(", ")) }
                            @for m in signed.iter().filter(|m| m.relation == *name && m.target == *target) {
                                div.why { (m.at.get(..10).unwrap_or("")) @if !m.why.is_empty() { ": " (m.why) } }
                                @if operator {
                                    form method="post" action={(at("/thing/")) (action)} {
                                        input type="hidden" name="relation" value=(name);
                                        input type="hidden" name="target" value=(target);
                                        input type="hidden" name="by" value=(m.by);
                                        input type="hidden" name="withdraw" value="1";
                                        button type="submit" { "Withdraw" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        } }
        @if !suggested.is_empty() {
            p.dim { "In words only, so not counted until a person confirms one:" }
            table { tbody {
                @for (name, target, whence) in &suggested {
                    tr {
                        td { (name) }
                        td {
                            code { (target) } div.why { (whence) }
                            @if operator {
                                form.bar method="post" action={(at("/thing/")) (action)} {
                                    input type="hidden" name="relation" value=(name);
                                    input type="hidden" name="target" value=(target);
                                    input type="text" name="by" placeholder="Your name" required;
                                    input type="text" name="why" placeholder="Why, in a few words";
                                    button type="submit" { "Confirm" }
                                }
                            }
                        }
                    }
                }
            } }
        }
    }
}

/// What a private tracker says to somebody without an account on it: that it exists, whose it is,
/// and how to sign in. Nothing it holds.
fn private_page(scope: &Tracker, free: bool) -> String {
    let forbidden: Vec<String> = scope.licences().into_iter().filter(|(_, r)| r == "no").map(|(s, _)| s).collect();
    shell(&scope.decl.title, html! {
        h1 { (scope.decl.title) }
        @if forbidden.is_empty() {
            p.about { "A private tracker. Its pages are for the accounts it has given access to." }
        } @else {
            p.about { "Not public: " (forbidden.join(", ")) " may not be republished, so its pages are for the accounts it has given access to." }
        }
        p.bar { a.chip.on href=(at("/signin")) { "Sign in" } @if !free { a.chip href=(at("/pricing")) { "What it costs" } } }
    })
}

impl TrackerSite {
    /// Hosted, the same tracker answers its owner as their operator and anybody else as a reader.
    pub fn set_operator(&mut self, yes: bool) {
        self.operator = yes;
    }
}

/// A field's name as a person reads it: `data_ds_appid` is `Appid`, `search_released` is
/// `Released`. The prefixes are a page's markup, not what the value is.
fn label(name: &str) -> String {
    if name == "known" {
        return "Date".into();
    }
    let mut s = name;
    for prefix in ["data_ds_", "data_", "tab_item_", "search_", "item_", "field_"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            if !rest.is_empty() {
                s = rest;
            }
        }
    }
    // The initials a field is named by are written as the initials they are: CVSS, not Cvss.
    let words = s
        .replace(['_', '-'], " ")
        .split(' ')
        .map(|w| if ["cvss", "epss", "cwe", "cwes", "cpe", "cve", "kev", "ssvc", "url", "id", "nvd", "ghsa"].contains(&w) { w.to_uppercase() } else { w.to_string() })
        .collect::<Vec<_>>()
        .join(" ");
    let mut c = words.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => name.to_string(),
    }
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

fn thousands(n: i64) -> String {
    crate::web::thousands(n.max(0) as usize)
}

/// A source by the name a person gave it rather than its handle.
fn title_of(scope: &Tracker, name: &str) -> String {
    scope.members.iter().find(|m| m.name() == name).map(|m| m.title()).unwrap_or_else(|| name.to_string())
}

/// Handles in a sentence, each put as its title.
fn with_titles(scope: &Tracker, text: &str) -> String {
    let mut out = text.to_string();
    for m in &scope.members {
        out = out.replace(m.name(), &m.title());
    }
    out
}

/// A yes or no as a person says it.
fn yes_no(word: &str) -> &str {
    match word {
        "true" => "yes",
        "false" => "no",
        w => w,
    }
}

/// The person at the machine, in the local app: they own every source and may read them again.
fn is_operator(v: &Viewer) -> bool {
    v.account.as_ref().is_some_and(|a| a.id == 0)
}

/// This tracker's sources that automatic updates stopped asking, and why.
fn paused(scope: &Tracker) -> Vec<(String, String)> {
    let registry = crate::tracker::registry(&scope.root.join("sources"));
    scope
        .decl
        .members
        .iter()
        .filter_map(|m| {
            let dir = registry.get(&m.dataset)?;
            let ds = crate::source::Source::open(dir).ok()?;
            (crate::autoupdate::failures(&ds) >= crate::autoupdate::PATIENCE).then(|| (ds.decl.title.clone(), crate::autoupdate::held(&ds, dir).unwrap_or_default()))
        })
        .collect()
}

/// When the next of this tracker's sources is due an automatic update.
fn next_check(scope: &Tracker, every: i64) -> Option<i64> {
    let registry = crate::tracker::registry(&scope.root.join("sources"));
    scope
        .decl
        .members
        .iter()
        .filter_map(|m| {
            let dir = registry.get(&m.dataset)?;
            let ds = crate::source::Source::open(dir).ok()?;
            if crate::autoupdate::held(&ds, dir).is_some() {
                return None;
            }
            crate::autoupdate::next_at(&ds, Some(every))
        })
        .min()
}

/// What this tracker compares, and from which column of each source: a property, and for every
/// source its column as the source writes it, or nothing where the source does not say it.
fn compared(scope: &Tracker) -> Vec<(String, Vec<(String, Option<String>)>)> {
    let registry = crate::tracker::registry(&scope.root.join("sources"));
    let decls: Vec<(String, String, Option<crate::sourcedecl::SourceDecl>)> = scope
        .decl
        .members
        .iter()
        .map(|m| {
            let title = title_of(scope, &m.dataset);
            let decl = registry.get(&m.dataset).and_then(|p| crate::sourcedecl::SourceDecl::load(p).ok());
            (m.dataset.clone(), title, decl)
        })
        .collect();
    scope
        .decl
        .normalise
        .iter()
        .map(|(name, align)| {
            let per = decls
                .iter()
                .map(|(member, title, decl)| {
                    let field = align.field_in(member, name);
                    // A source whose claims arrived built (subscribed, or in a package) declares its
                    // properties without the column they were read from: the name it holds is the one.
                    let column = decl.as_ref().and_then(|d| d.records.fields.get(&field)).map(|p| {
                        let from = p.from.trim_start_matches("field:").trim();
                        if from.is_empty() { field.clone() } else { from.to_string() }
                    });
                    (title.clone(), column)
                })
                .collect();
            (name.clone(), per)
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Proposing from the browser: a reader signed in by a link, a form made from the source's own
// declaration, and the workspace signing for them. See propose.rs.

/// The sources of this tracker a viewer may propose to from here, by name, with where they are:
/// every one that takes proposals for the person at the machine, the ones that name readers for
/// everybody else.
fn proposable(scope: &Tracker, operator: bool) -> Vec<(String, std::path::PathBuf)> {
    let registry = crate::tracker::registry(&scope.root.join("sources"));
    // A world that names proposers or editors of its own takes proposals where a source names nobody.
    let access = crate::account::Site::load(&scope.root).access;
    let world_takes = !access.proposers.is_empty() || !access.editors.is_empty();
    scope
        .members
        .iter()
        .filter_map(|m| {
            let dir = registry.get(m.name())?.clone();
            let readers = crate::propose::readers(&dir).ok()?;
            (operator || world_takes || !readers.is_empty()).then(|| (m.name().to_string(), dir))
        })
        .collect()
}

/// Who is proposing, where anybody is: the person at the machine as the owner, a signed-in
/// reader as their pseudonym.
fn reader_of(scope: &Tracker, accounts: &Accounts, v: &Viewer, operator: bool) -> Option<crate::propose::Reader> {
    // The owner proposes as the world, by its own key: not as whoever runs the program, which on a
    // machine of many worlds is the machine.
    if operator {
        let title = Site::load(&scope.root).title;
        return Some(crate::propose::Reader {
            id: crate::propose::operator_key(&scope.root).ok()?,
            name: if title.is_empty() { "the owner".into() } else { format!("the owner of {title}") },
            email: String::new(),
            owner: true,
            issuers: Vec::new(),
        });
    }
    let a = v.account.as_ref().filter(|_| !v.by_key)?;
    Some(crate::propose::Reader {
        id: crate::propose::pseudonym(&scope.root, a.id).ok()?,
        name: accounts.name_of(a.id),
        email: a.email.clone(),
        owner: false,
        issuers: accounts.issuers_of(a.id),
    })
}

/// Where a reader's session holds: the workspace this tracker is in, which is the mount with its
/// `/trackers/<tracker>` taken off. A reader signed in to one tracker is signed in to its workspace's
/// others, because they are one `accounts.db`, and to no other workspace's on the same host.
fn workspace_path() -> String {
    let m = mounted();
    let base = match m.rfind("/trackers/") {
        Some(i) if !m[i + 10..].contains('/') => m[..i].to_string(),
        _ => m,
    };
    if base.is_empty() { "/".into() } else { base }
}

/// The reader's cookie, set or (with no session and no age) cleared. Secure where the workspace
/// says it is served over https.
pub(crate) fn reader_cookie(site: &Site, session: &str, max_age: u32) -> String {
    format!(
        "{}={session}; Path={}; Max-Age={max_age}; HttpOnly; SameSite=Lax{}",
        account::READER_COOKIE,
        workspace_path(),
        if site.url.starts_with("https://") { "; Secure" } else { "" }
    )
}

/// Signing in through `id`, at the workspace's own `/oauth/login`, and back to where the reader was.
fn signin_elsewhere(id: &str, next: Option<&str>) -> String {
    let wp = workspace_path();
    let prefix = if wp == "/" { "" } else { wp.as_str() };
    format!("{prefix}/oauth/login?with={}&next={}", urlencode(id), urlencode(&at(next.unwrap_or("/"))))
}

/// Where a signed-in reader comes back to: a path on this site, never another site.
pub(crate) fn next_of(raw: &str) -> Option<String> {
    let raw = raw.trim();
    // `/\evil.example` is read by browsers as `//evil.example`: no backslash, no control character.
    (raw.starts_with('/') && !raw.starts_with("//") && !raw.contains("://") && !raw.contains('\\') && !raw.chars().any(char::is_control)).then(|| raw.to_string())
}

/// The form for one row: a field per field of the source, filled from a claim when it corrects
/// one, then where it was read and how.
#[allow(clippy::too_many_arguments)]
fn propose_page(
    scope: &Tracker,
    member: &str,
    dir: &Path,
    reader: Option<&crate::propose::Reader>,
    may: Result<(), String>,
    url: &str,
    said: Option<&str>,
    given: &BTreeMap<String, String>,
) -> String {
    let Ok(decl) = crate::sourcedecl::SourceDecl::load(dir) else {
        return shell("Not here", html! { h1 { "No such source" } });
    };
    let fields = crate::propose::fields(&decl);
    let title = if decl.title.is_empty() { decl.name.clone() } else { decl.title.clone() };
    let p = params(url);
    // A correction starts from what the claim says now.
    let correcting = p.get("claim").and_then(|id| scope.records_of(member, std::slice::from_ref(id), false).into_iter().next());
    let mut value: BTreeMap<String, String> = BTreeMap::new();
    if let Some(rec) = &correcting {
        // The row as it was proposed, where the claim came from one; what the claim says
        // otherwise, which has no field that only went into its identifier.
        let behind = rec.from.url.as_deref().and_then(|u| crate::propose::row_behind(dir, u)).unwrap_or_default();
        for f in &fields {
            let property = f.property.clone().unwrap_or_else(|| f.name.clone());
            let said = match behind.get(&f.name) {
                Some(J::String(s)) => Some(s.clone()),
                Some(J::Null) | None => rec.fields.get(&property).map(|v| v.display()),
                Some(other) => Some(other.to_string()),
            };
            if let Some(v) = said {
                value.insert(format!("f.{}", f.name), v);
            }
        }
    }
    value.extend(given.iter().map(|(k, v)| (k.clone(), v.clone())));
    let get = |k: &str| value.get(k).cloned().unwrap_or_default();
    let today = crate::iso_date(crate::now());
    let here = format!("/propose/{}{}", urlencode(member), correcting.as_ref().map(|r| format!("?claim={}", urlencode(&r.record_id))).unwrap_or_default());
    let body = html! {
        p { a href=(at("/")) { "← " (scope.decl.title) } }
        h1 { @if let Some(rec) = &correcting { "A correction to " (rec.title) } @else { "A row for " (title) } }
        p.about {
            "What you propose waits for the owner of " (title) ", and is part of it only once they accept it. "
            "Say where you read it: the receipt for every value names that place, and you."
        }
        @if let Some(s) = said { div.note { (s) } }
        @match (reader, &may) {
            (None, _) => {
                div.note {
                    "Proposing needs you signed in, so the owner can tell you what they decided. "
                    a href={(at("/signin")) "?next=" (urlencode(&here))} { "Sign in with your address" } "."
                }
            }
            (Some(_), Err(e)) => { div.note { (e) "." } }
            (Some(r), Ok(())) => {
                form.settings method="post" action=(at(&here)) {
                    @for f in &fields {
                        p { label {
                            (f.name) @if f.identifies { " *" }
                            @if let Some(p) = &f.property { span.dim { " → " (p) } }
                            br;
                            @if f.kind == crate::sourcedecl::PropertyType::Number {
                                input.wide type="text" inputmode="decimal" name={"f." (f.name)} value=(get(&format!("f.{}", f.name))) required[f.identifies];
                            } @else if f.kind == crate::sourcedecl::PropertyType::Date {
                                input.wide type="date" name={"f." (f.name)} value=(get(&format!("f.{}", f.name))) required[f.identifies];
                            } @else if f.kind == crate::sourcedecl::PropertyType::Bool {
                                @let said = get(&format!("f.{}", f.name)).to_lowercase();
                                select name={"f." (f.name)} required[f.identifies] {
                                    option value="" { "" }
                                    option value="yes" selected[matches!(said.as_str(), "yes" | "true" | "ja")] { "yes" }
                                    option value="no" selected[matches!(said.as_str(), "no" | "false" | "nein")] { "no" }
                                }
                            } @else {
                                input.wide type="text" name={"f." (f.name)} value=(get(&format!("f.{}", f.name))) required[f.identifies];
                            }
                        } }
                    }
                    h2 { "Where you read it" }
                    p { label { "The page it was read on *" br; input.wide type="url" name="read_from" placeholder="https://…" value=(get("read_from")) required; } }
                    p { label { "On *" br; input type="date" name="read_at" value=(if get("read_at").is_empty() { today.clone() } else { get("read_at") }) required; } }
                    p { label { input type="radio" name="attest" value="read" checked[get("attest") != "relayed"]; " I read it there myself" } }
                    p { label { input type="radio" name="attest" value="relayed" checked[get("attest") == "relayed"]; " Somebody who read it passed it on, and allows it (name them below)" } }
                    p { label { "Note" br; input.wide type="text" name="note" value=(if get("note").is_empty() { correcting.as_ref().map(|c| format!("Corrects {}", c.title)).unwrap_or_default() } else { get("note") }); } }
                    @if !r.owner {
                        p { label { "Shown as" br; input.wide type="text" name="name" placeholder="your name, or leave it empty" value=(if given.contains_key("name") { get("name") } else { r.name.clone() }); } }
                        p.dim { "Beside your proposal the owner and the receipts show this name and " code { (r.id) } ". Your address is never shown." }
                    }
                    p { button.primary type="submit" { "Propose" } }
                }
            }
        }
    };
    shell(&format!("Propose · {title}"), body)
}

/// A proposal from the form: made into a body as a key's proposer would write it, and kept,
/// signed by this workspace for the reader. The file it was kept under.
fn take_from_form(scope: &Tracker, accounts: &Accounts, dir: &Path, reader: &mut crate::propose::Reader, account: Option<i64>, form: &str) -> Result<String, String> {
    let decl = crate::sourcedecl::SourceDecl::load(dir)?;
    let fields = crate::propose::fields(&decl);
    let said: BTreeMap<String, String> = fields.iter().map(|f| (f.name.clone(), form_field(form, &format!("f.{}", f.name)))).collect();
    let row = crate::propose::row_from(&fields, &said)?;
    let body = json!({
        "row": row,
        "read_at": form_field(form, "read_at").trim(),
        "read_from": form_field(form, "read_from").trim(),
        "attest": form_field(form, "attest"),
        "note": form_field(form, "note").trim(),
    });
    // The name it is shown as, where the form asked for one (the owner's has no such field, and an
    // absent field is not an empty name). Kept only once the proposal is.
    let named = account.filter(|_| !reader.owner && form.split('&').any(|p| p == "name" || p.starts_with("name=")));
    let before = reader.name.clone();
    if named.is_some() {
        reader.name = crate::account::clean_name(&form_field(form, "name"));
    }
    let file = crate::propose::receive_from_reader(dir, &scope.root, body.to_string().as_bytes(), reader)?;
    if let Some(id) = named.filter(|_| reader.name != before) {
        accounts.set_name(id, &reader.name)?;
    }
    if let Some(id) = account {
        accounts.record_proposal(id, &decl.name, &file)?;
    }
    crate::propose::tell_owner(&scope.root, dir, &file);
    Ok(file)
}

/// What a reader proposed here, and where each stands.
fn proposals_section(scope: &Tracker, accounts: &Accounts, account: i64) -> Markup {
    let mine = accounts.proposals_of(account);
    let registry = crate::tracker::registry(&scope.root.join("sources"));
    html! {
        h2 { "Your proposals" }
        @if mine.is_empty() { p.dim { "None yet." } }
        table { tbody {
            @for p in &mine {
                @let entry = registry.get(&p.source).and_then(|d| crate::propose::list(d).into_iter().find(|e| e.file == p.file));
                tr {
                    td { span.chip { (p.source) } " " @if let Some(e) = &entry { code { (e.row) } } }
                    td.dim { (p.at) }
                    td.num { @match &entry { Some(e) => span.state.(if e.status == "accepted" { "current" } else { "empty" }) { (e.status) }, None => span.dim { "gone" } } }
                }
            }
        } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_filter_is_said_once_and_a_query_is_one_address() {
        assert_eq!(canonical("kind=a  kind=a source=x kind=a"), "kind=a source=x");
        assert_eq!(with_term("kind=a source=x", "kind=a"), "kind=a source=x", "clicked again, nothing new");
        assert_eq!(with_term("kind=a", "source=x"), "kind=a source=x");
        let many: Vec<String> = (0..20).map(|i| format!("f{i}=v")).collect();
        assert_eq!(canonical(&many.join(" ")).split_whitespace().count(), MAX_TERMS);
    }

    /// A workspace with one tracker over one proposals source, served on a free port in this
    /// process. The address, and where the workspace is.
    fn served(tag: &str, readers: &str) -> (String, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("zetlyn-serve-propose-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let source = root.join("sources/prices");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join("source.yaml"),
            format!("name: t/prices\ntitle: Car prices\nkind: price\nfetch:\n  type: proposals\n  from: []\n  readers: [{readers}]\nlicence:\n  republish: yes\nclaims:\n  id:\n    scheme: price\n    from: \"const:{{country}}-{{week}}\"\n  title: \"const:{{country}} {{week}}\"\n  known: field:read_at\n  properties:\n    price:\n      type: number\n      from: field:price\n"),
        )
        .unwrap();
        let tracker = root.join("trackers/prices");
        std::fs::create_dir_all(&tracker).unwrap();
        std::fs::write(tracker.join("tracker.yaml"), "name: t/prices-tracker\ntitle: Prices\nsources:\n- source: t/prices\n  why: Read by people, since nobody publishes it.\nidentified_by: [price]\n").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let at = root.clone();
        // Ends with the test process; nothing outlives it.
        std::thread::spawn(move || {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let addr = server.server_addr().to_ip().unwrap().to_string();
            let scope = Tracker::open(&at.join("trackers/prices"), &at.join("sources")).unwrap();
            let mut site = TrackerSite::open(scope, &at.join("trackers/prices"), &at.join("sources"), &addr, false).unwrap();
            tx.send(addr).unwrap();
            for request in server.incoming_requests() {
                site.answer(request);
            }
        });
        (format!("http://{}", rx.recv().unwrap()), root)
    }

    fn session(root: &Path, email: &str) -> String {
        let accounts = Accounts::open(root).unwrap();
        let a = accounts.ensure(email).unwrap();
        format!("zr={}", accounts.spend_link(&accounts.new_link(a.id).unwrap(), crate::account::Kind::Reader).unwrap())
    }

    fn ask(method: &str, url: &str, cookie: Option<&str>, form: &str) -> (u16, String) {
        let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
        let mut response = match method {
            "POST" => {
                let mut r = agent.post(url).header("Content-Type", "application/x-www-form-urlencoded");
                if let Some(c) = cookie {
                    r = r.header("Cookie", c);
                }
                r.send(form).unwrap()
            }
            _ => {
                let mut r = agent.get(url);
                if let Some(c) = cookie {
                    r = r.header("Cookie", c);
                }
                r.call().unwrap()
            }
        };
        (response.status().as_u16(), response.body_mut().read_to_string().unwrap_or_default())
    }

    #[test]
    fn a_page_drawn_for_nobody_in_particular_is_kept_for_the_next_until_something_is_sent() {
        let (base, root) = served("kept", "anyone");
        let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
        let page = |path: &str, cookie: Option<&str>| {
            let mut r = agent.get(&format!("{base}{path}"));
            if let Some(c) = cookie {
                r = r.header("Cookie", c);
            }
            let mut response = r.call().unwrap();
            let how = response.headers().get("X-Zetlyn-Page").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            (how, response.body_mut().read_to_string().unwrap())
        };
        let (first, drawn) = page("/", None);
        let (second, kept) = page("/", None);
        assert_eq!((first.as_str(), second.as_str()), ("drawn", "kept"));
        assert_eq!(drawn, kept, "the page kept is the page drawn");
        assert_eq!(page("/things?q=x", None).0, "drawn", "another address is another page");
        // Whoever carries a cookie is drawn a page of their own, and it is not kept for others.
        let cookie = session(&root, "a@example.com");
        assert_eq!(page("/", Some(&cookie)).0, "drawn");
        assert_eq!(page("/", None).0, "kept");
        // A form sent may change any page: none drawn before it is kept.
        ask("POST", &format!("{base}/signout"), None, "");
        assert_eq!(page("/", None).0, "drawn");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_reader_signs_in_proposes_from_the_form_and_sees_it_wait() {
        let (base, root) = served("flow", "signed-in");
        let form_url = format!("{base}/propose/t%2Fprices");

        let (status, page) = ask("GET", &format!("{base}/"), None, "");
        assert_eq!(status, 200);
        assert!(page.contains("Propose a row"), "the overview offers it");

        // Not signed in: the form says why, and sign-in comes back here.
        let (status, page) = ask("GET", &form_url, None, "");
        assert_eq!(status, 200);
        assert!(page.contains("Sign in with your address") && page.contains("next=%2Fpropose%2Ft%252Fprices"), "{page}");
        assert_eq!(ask("POST", &form_url, None, "f.country=DEU").0, 401);
        let (_, page) = ask("GET", &format!("{base}/signin?next=%2Fpropose%2Ft%252Fprices"), None, "");
        assert!(page.contains(r#"name="next" value="/propose/t%2Fprices""#), "{page}");
        assert!(!ask("GET", &format!("{base}/signin?next=%2F%2Fevil.example"), None, "").1.contains(r#"name="next""#));

        let ann = session(&root, "ann@example.org");
        let (status, page) = ask("GET", &form_url, Some(&ann), "");
        assert_eq!(status, 200);
        for field in ["f.country", "f.week", "f.price", "read_from", "read_at", "attest"] {
            assert!(page.contains(&format!(r#"name="{field}""#)), "{field} in {page}");
        }

        // What was typed stays when something is missing.
        let (status, page) = ask("POST", &form_url, Some(&ann), "f.country=DEU&f.price=44990&read_from=https%3A%2F%2Fexample.com&read_at=2026-10-02&attest=read&name=Ann");
        assert_eq!(status, 400);
        assert!(page.contains("`week`") && page.contains(r#"value="44990""#), "{page}");

        let (status, page) = ask("POST", &form_url, Some(&ann), "f.country=DEU&f.week=2026-W40&f.price=44990&read_from=https%3A%2F%2Fexample.com&read_at=2026-10-02&attest=read&name=Ann");
        assert_eq!(status, 200, "{page}");
        assert!(page.contains("Proposed"));
        let queue = crate::propose::list(&root.join("sources/prices"));
        assert_eq!(queue.len(), 1);
        assert_eq!((queue[0].name.as_str(), queue[0].via.as_str(), queue[0].verified), ("Ann", "browser", true));
        assert_eq!(queue[0].row, json!({"country": "DEU", "week": "2026-W40", "price": 44990}));

        let (_, page) = ask("GET", &format!("{base}/account"), Some(&ann), "");
        assert!(page.contains("Your proposals") && page.contains("pending"), "{page}");

        // Accepted, it is a claim, and its page offers a correction filled from what it says.
        crate::propose::decide(&root.join("sources/prices"), &queue[0].file, true, "owner", "").unwrap();
        let ds = crate::source::Source::open(&root.join("sources/prices")).unwrap();
        ds.run().unwrap();
        let q = crate::source::Query { text: String::new(), pred: None, ids: vec!["DEU-2026-W40".into()], seen_before: None, view: None, sort: None, limit: 5, offset: 0 };
        let id = ds.search(&q).unwrap().1.into_iter().next().unwrap().record_id;
        let (status, page) = ask("GET", &format!("{base}/claim/t%2Fprices/{}", urlencode(&id)), None, "");
        assert_eq!(status, 200, "{page}");
        assert!(page.contains("Propose a correction"), "{page}");
        let (_, page) = ask("GET", &format!("{form_url}?claim={}", urlencode(&id)), Some(&ann), "");
        assert!(page.contains("A correction to") && page.contains(r#"value="44990""#) && page.contains(r#"value="DEU""#), "{page}");
        let (_, page) = ask("GET", &format!("{base}/account"), Some(&ann), "");
        assert!(page.contains("accepted"), "{page}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_source_that_names_no_readers_takes_nothing_from_the_browser() {
        let (base, root) = served("closed", "");
        let ann = session(&root, "ann@example.org");
        assert!(!ask("GET", &format!("{base}/"), None, "").1.contains("Propose a row"));
        assert_eq!(ask("GET", &format!("{base}/propose/t%2Fprices"), Some(&ann), "").0, 404);
        assert_eq!(ask("POST", &format!("{base}/propose/t%2Fprices"), Some(&ann), "f.country=DEU&f.week=W&read_from=x&read_at=2026-10-02&attest=read").0, 404);
        assert!(crate::propose::list(&root.join("sources/prices")).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_a_path_here_is_somewhere_to_come_back_to() {
        assert_eq!(next_of("/propose/x").as_deref(), Some("/propose/x"));
        for elsewhere in ["//evil.example", "https://evil.example", "/x?u=https://evil", "propose/x", ""] {
            assert!(next_of(elsewhere).is_none(), "{elsewhere}");
        }
    }

    /// The Set-Cookie a request is answered with, where there is one.
    fn set_cookie(url: &str, cookie: Option<&str>) -> Option<String> {
        let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
        let mut r = agent.get(url);
        if let Some(c) = cookie {
            r = r.header("Cookie", c);
        }
        r.call().unwrap().headers().get("set-cookie").map(|v| v.to_str().unwrap_or("").to_string())
    }

    #[test]
    fn a_reader_session_is_its_own_cookie_and_leaves_the_apps_alone() {
        let (base, root) = served("cookie", "signed-in");
        let accounts = Accounts::open(&root).unwrap();
        let a = accounts.ensure("ann@example.org").unwrap();

        let set = set_cookie(&format!("{base}/signin/{}", accounts.new_link(a.id).unwrap()), None).unwrap();
        assert!(set.starts_with("zr=") && set.contains("Path=/;") && set.contains("Max-Age=2592000"), "{set}");
        assert!(!set.contains("Secure"), "a workspace that names no https address is not told to be: {set}");
        let session = set.split(';').next().unwrap().to_string();
        assert!(ask("GET", &format!("{base}/account"), Some(&session), "").1.contains("ann@example.org"));

        // The app's cookie, even holding a session this very database knows, is not a reader's.
        let app_session = format!("zs={}", accounts.spend_link(&accounts.new_link(a.id).unwrap(), crate::account::Kind::Member).unwrap());
        let (_, page) = ask("GET", &format!("{base}/account"), Some(&app_session), "");
        assert!(!page.contains("ann@example.org") && page.contains("Send the link"), "{page}");

        // Signing out ends the reader's session and clears their cookie, and only theirs.
        let cleared = set_cookie(&format!("{base}/signout"), Some(&format!("{session}; {app_session}"))).unwrap();
        assert!(cleared.starts_with("zr=;") && cleared.contains("Max-Age=0") && !cleared.contains("zs="), "{cleared}");
        assert!(!ask("GET", &format!("{base}/account"), Some(&session), "").1.contains("ann@example.org"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_reader_is_signed_in_to_the_workspace_a_tracker_is_mounted_in() {
        for (mount, path) in [("", "/"), ("/trackers/cve", "/"), ("/worlds/zetlyn/trackers/cve", "/worlds/zetlyn"), ("/worlds/acme/trackers/prices", "/worlds/acme"), ("/worlds/zetlyn", "/worlds/zetlyn")] {
            crate::serve::mount(mount);
            assert_eq!(workspace_path(), path, "mounted at {mount:?}");
        }
        crate::serve::mount("");
        let site = Site { url: "https://zetlyn.com".into(), ..Site::default() };
        crate::serve::mount("/worlds/zetlyn/trackers/cve");
        assert_eq!(reader_cookie(&site, "abc", 60), "zr=abc; Path=/worlds/zetlyn; Max-Age=60; HttpOnly; SameSite=Lax; Secure");
        crate::serve::mount("");
    }

    #[test]
    fn an_organisation_with_no_address_of_its_own_is_at_the_hosting_machines() {
        let (_, made) = served("org", "signed-in");
        let hosting = std::env::temp_dir().join(format!("zetlyn-serve-propose-hosting-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&hosting);
        let org = hosting.join("orgs/acme");
        std::fs::create_dir_all(&org).unwrap();
        for d in ["sources", "trackers"] {
            std::fs::rename(made.join(d), org.join(d)).unwrap();
        }
        let open = |root: &Path| {
            let scope = Tracker::open(&root.join("trackers/prices"), &root.join("sources")).unwrap();
            TrackerSite::open(scope, &root.join("trackers/prices"), &root.join("sources"), "127.0.0.1:0", false).unwrap().site.url
        };
        assert_eq!(open(&org), "", "nothing names an address");
        std::fs::write(hosting.join("workspace.yaml"), "url: https://app.example.org\n").unwrap();
        assert_eq!(open(&org), "https://app.example.org/acme", "the machine's, under its own name");
        std::fs::write(org.join("workspace.yaml"), "url: https://acme.example.org\n").unwrap();
        assert_eq!(open(&org), "https://acme.example.org", "its own, where it names one");
        let _ = std::fs::remove_dir_all(&hosting);
        let _ = std::fs::remove_dir_all(&made);
    }
}

/// Whether a source's name is what a term names: `zetlyn/cve-kev`, or `cve-kev` for short.
fn names_member(member: &str, said: &str) -> bool {
    member == said || member.rsplit('/').next() == Some(said)
}

/// The sources that make a ladder's step true of this thing, or None where it is not. Terms are
/// joined by `and`: `has:<source>`, `<property>=<value>`, `<source>.<property>=<value>`. The
/// empty step is every thing's, and nobody had to say it.
fn step_holds(entry: &crate::tracker::Thing, when: &str) -> Option<Vec<String>> {
    let mut who: Vec<String> = Vec::new();
    for term in when.split(" and ").map(str::trim).filter(|t| !t.is_empty()) {
        if let Some(source) = term.strip_prefix("has:") {
            let found: Vec<String> = entry.members().into_iter().filter(|m| names_member(m, source)).map(str::to_string).collect();
            if found.is_empty() {
                return None;
            }
            who.extend(found);
            continue;
        }
        let (lhs, value) = term.split_once('=')?;
        let (source, property) = match lhs.trim().rsplit_once('.') {
            Some((s, p)) => (Some(s), p),
            None => (None, lhs.trim()),
        };
        let field = entry.fields.get(property)?;
        let value = value.trim().to_lowercase();
        let found: Vec<String> = field
            .by
            .iter()
            .filter(|(m, _)| source.map_or(true, |s| names_member(m, s)))
            .filter(|(m, said)| {
                said.iter().chain(field.means.get(*m).into_iter().flatten()).any(|v| v.to_lowercase() == value)
            })
            .map(|(m, _)| m.clone())
            .collect();
        if found.is_empty() {
            return None;
        }
        who.extend(found);
    }
    who.sort();
    who.dedup();
    Some(who)
}

/// The day a source first spoke of this thing, and of which of its claims.
fn first_said(entry: &crate::tracker::Thing, member: &str) -> Option<(String, String)> {
    entry
        .parts
        .iter()
        .filter(|p| p.member == member && !p.known.is_empty())
        .min_by(|a, b| a.known.cmp(&b.known))
        .map(|p| (p.known.get(..10).unwrap_or(&p.known).to_string(), p.title.clone()))
}

/// What happened to a thing, by day: when each source first spoke of it, and every date a
/// declared property says, each with whose it is. Oldest first.
fn timeline_of(scope: &Tracker, entry: &crate::tracker::Thing, dated: &[String]) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = Vec::new();
    for m in entry.members() {
        if let Some((day, title)) = first_said(entry, m) {
            out.push((day, title_of(scope, m), format!("first spoke of it: {title}")));
        }
    }
    for property in dated {
        let Some(f) = entry.fields.get(property) else { continue };
        for (m, said) in &f.by {
            for v in said {
                let day = v.get(..10).unwrap_or(v);
                if day.len() == 10 && day.as_bytes()[4] == b'-' && day.as_bytes()[7] == b'-' {
                    out.push((day.to_string(), title_of(scope, m), label(property)));
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// A CVSS vector as its metrics, `AV` to `N`, in the order the vector writes them.
fn metrics_of(vector: &str) -> Vec<(String, String)> {
    let body = vector.split_once('/').filter(|(v, _)| v.starts_with("CVSS:")).map(|(_, rest)| rest).unwrap_or(vector);
    body.split('/').filter_map(|m| m.split_once(':')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// A CVSS metric and its value in words, as the specification names them.
fn metric_words(metric: &str, value: &str) -> (String, String) {
    let name = match metric {
        "AV" => "Attack vector",
        "AC" => "Attack complexity",
        "AT" => "Attack requirements",
        "PR" => "Privileges required",
        "UI" => "User interaction",
        "S" => "Scope",
        "C" | "VC" => "Confidentiality",
        "I" | "VI" => "Integrity",
        "A" | "VA" => "Availability",
        "SC" => "Subsequent confidentiality",
        "SI" => "Subsequent integrity",
        "SA" => "Subsequent availability",
        other => other,
    };
    let word = match (metric, value) {
        ("AV", "N") => "network",
        ("AV", "A") => "adjacent",
        ("AV", "L") => "local",
        ("AV", "P") => "physical",
        ("UI", "N") | ("PR", "N") | ("AT", "N") => "none",
        ("UI", "R") => "required",
        ("UI", "P") => "passive",
        ("UI", "A") => "active",
        ("S", "U") => "unchanged",
        ("S", "C") => "changed",
        ("AT", "P") => "present",
        (_, "L") => "low",
        (_, "H") => "high",
        (_, "N") => "none",
        (_, v) => v,
    };
    (name.to_string(), word.to_string())
}

#[cfg(test)]
mod ladder_tests {
    use super::*;

    fn thing(members: &[&str], fields: &[(&str, &str, &str)]) -> crate::tracker::Thing {
        let parts = members
            .iter()
            .map(|m| crate::tracker::ClaimRef {
                member: m.to_string(),
                priority: 0,
                kind: "vulnerability".into(),
                record_id: format!("{m}-1"),
                title: format!("{m} says"),
                url: None,
                known: "2021-12-10".into(),
                fields: BTreeMap::new(),
            })
            .collect();
        let mut by_name: BTreeMap<String, crate::tracker::PropertyView> = BTreeMap::new();
        for (member, name, value) in fields {
            let f = by_name.entry(name.to_string()).or_insert_with(|| crate::tracker::PropertyView { by: BTreeMap::new(), means: BTreeMap::new(), divergent: false, mapped: false });
            f.by.entry(member.to_string()).or_default().push(value.to_string());
        }
        crate::tracker::Thing { key: None, rank: 0, title: "t".into(), parts, fields: by_name, why: Vec::new() }
    }

    #[test]
    fn a_step_holds_where_a_source_says_what_it_names_and_names_who_said_it() {
        let t = thing(&["zetlyn/cve-kev", "zetlyn/cve-exploitdb"], &[("zetlyn/cve-kev", "exploited", "yes"), ("zetlyn/cve-exploitdb", "verified", "false")]);
        assert_eq!(step_holds(&t, ""), Some(vec![]), "the bottom rung needs nobody");
        assert_eq!(step_holds(&t, "has:cve-exploitdb"), Some(vec!["zetlyn/cve-exploitdb".to_string()]), "a source by its short name");
        assert_eq!(step_holds(&t, "has:cve-metasploit"), None);
        assert_eq!(step_holds(&t, "exploited=Yes"), Some(vec!["zetlyn/cve-kev".to_string()]), "case aside");
        assert_eq!(step_holds(&t, "cve-exploitdb.verified=true"), None, "what the source said, not what is wished");
        assert_eq!(step_holds(&t, "cve-kev.verified=false"), None, "only that source");
        assert_eq!(step_holds(&t, "has:cve-kev and exploited=yes"), Some(vec!["zetlyn/cve-kev".to_string()]));
        assert_eq!(step_holds(&t, "has:cve-kev and has:cve-metasploit"), None, "every term");
    }

    #[test]
    fn a_vector_is_its_metrics_and_each_metric_has_words() {
        assert_eq!(metrics_of("CVSS:3.1/AV:N/UI:R")[1], ("UI".to_string(), "R".to_string()));
        assert_eq!(metric_words("UI", "R"), ("User interaction".to_string(), "required".to_string()));
        assert_eq!(metric_words("S", "C").1, "changed");
    }
}

#[cfg(test)]
mod related_tests {
    use super::{clipped, natural};

    #[test]
    fn identifiers_sort_as_a_person_counts_them() {
        let mut ids = vec!["CVE-2026-9999", "CVE-2025-10000", "CVE-2026-10000", "CVE-2026-0042"];
        ids.sort_by(|a, b| natural(b, a));
        assert_eq!(ids, ["CVE-2026-10000", "CVE-2026-9999", "CVE-2026-0042", "CVE-2025-10000"]);
    }

    #[test]
    fn a_long_title_is_cut_at_a_word() {
        assert_eq!(clipped("short", 140), "short");
        assert_eq!(clipped("Buffer overflow in PDFium, in Chrome", 22), "Buffer overflow in…");
    }
}
