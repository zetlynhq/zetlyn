//! A tracker, opened. The same three surfaces a source has, over sources of unlike shape.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use maud::{html, Markup};
use serde_json::{json, Value as J};

use crate::account::{self, Accounts, Site, Viewer};
use crate::expr;
use crate::tracker::{Thing, Tracker, TrackerQuery};
use crate::serve::{at, flatten, mounted, params, shell, unmount, urlencode};

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
                    @if f.divergent { span.chip.on { (distinct.iter().map(|s| s.as_str())
                        .collect::<Vec<_>>().join(" / ")) } }
                    @else { (distinct.first().map(|s| s.as_str()).unwrap_or("")) }
                }
            }
            None => html! { span.dim { "—" } },
        },
    }
}

fn overview(scope: &Tracker, url: &str, v: &Viewer, site: &Site) -> String {
    let bound = account::bound(v, &scope.decl.name);
    let p = params(url);
    let q = p.get("q").cloned().unwrap_or_default();
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
    let answer = scope.search(&sq);
    let columns = scope.columns(&sq);
    let facets = scope.facets(&sq);
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

    let body = html! {
        h1 { (d.title) }
        (banner(v, scope, hidden))
        @if !d.about.is_empty() { p.about { (d.about) } }
        p.state.(if stale { "partial" } else { "current" }) {
            (scope.records()) " claims · " (scope.members.len()) " sources · "
            @for (k, n) in scope.kinds() { (k) " " (n) " · " }
            @if let Some(f) = &d.promise.fresh_within {
                @if holds {
                    "fresh within " (f)
                    @if let Some(age) = scope.oldest_finish() {
                        ", the source updated longest ago " (crate::tracker::human(age)) " ago"
                    }
                }
                @else { "the promise of " (f) " does not hold" }
            }
        }
        @if let Some(c) = &coverage {
            @let by = c["by_sources"].as_object().cloned().unwrap_or_default();
            @let several: i64 = by.iter().filter(|(n, _)| n.parse::<i64>().unwrap_or(0) > 1).map(|(_, v)| v.as_i64().unwrap_or(0)).sum();
            p.bar {
                span.chip { (c["things"].as_i64().unwrap_or(0)) " things" } " "
                span.chip { (several) " named by two sources or more" } " "
                a.chip href=(at("/conflicts")) { (c["conflicts"].as_i64().unwrap_or(0)) " conflicts" } " "
                a.chip href=(at("/changes")) { "what changed" }
            }
            @if let Some(only) = c["only"].as_object() {
                p.dim { "Only one source knows: "
                    @for (i, (source, n)) in only.iter().enumerate() {
                        @if i > 0 { " · " } (source) " " (n)
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

        form.bar method="get" action=(at("/")) {
            input type="search" name="q" value=(q) placeholder="Search, or filter with name=value";
            @if !view.is_empty() { input type="hidden" name="view" value=(view); }
            @if !kind.is_empty() { input type="hidden" name="kind" value=(kind); }
            button type="submit" { "Search" }
        }
        @if q.is_empty() && !examples.is_empty() {
            p.bar {
                span.dim { "Try:" }
                @for ex in &examples { a.chip href=(link(ex, &view, &kind, 1)) { (ex) } }
            }
        }
        @if !active.is_empty() {
            p.bar {
                @for (name, op, lit) in &active {
                    @let one = format!("{name}{}{}", op.sql(), lit.display());
                    @let without = q.replace(&one, "").split_whitespace()
                        .collect::<Vec<_>>().join(" ");
                    a.chip.on href=(link(&without, &view, &kind, 1)) {
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

        p.bar {
            @if view.is_empty() && kind.is_empty() { span.chip.on { "Everything" } }
            @else { a.chip href=(link(&q, "", "", 1)) { "Everything" } }
            @for v in &d.view.named {
                @let t = if v.title.is_empty() { v.name.clone() } else { v.title.clone() };
                @if v.name == view { span.chip.on { (t) } }
                @else { a.chip href=(link(&q, &v.name, "", 1)) { (t) } }
            }
            @for (k, n) in scope.kinds() {
                @if k == kind { span.chip.on { (k) " " (n) } }
                @else { a.chip href=(link(&q, &view, &k, 1)) { (k) " " (n) } }
            }
        }

        h2 {
            (answer.entries.len()) " shown of "
            @if answer.truncated { "at least " }
            (answer.total)
            @if answer.subjects { " things" } @else { " claims" }
        }
        @if answer.truncated {
            div.note {
                "A source had more candidates than were read. The filter is applied over the "
                "assembled thing, so what is not read is not counted, and this number is a "
                "floor. Narrow the query to get an exact one."
            }
        }
        table {
            thead { tr {
                th { "Thing" }
                @for c in &columns { th { (c) } }
            } }
            tbody {
                @for e in &answer.entries {
                    tr {
                        td {
                            @match entry_link(e) {
                                Some(href) => a href=(href) { (e.title) },
                                None => span { (e.title) },
                            }
                            div.why {
                                @if let Some(k) = &e.key { (k.scheme) " " (k.value) " · " }
                                (e.members().join(", "))
                                @if !e.why.is_empty() { " · matched " (e.why.join(", ")) }
                            }
                        }
                        @for c in &columns { td { (cell(e, c)) } }
                    }
                }
            }
        }
        @if answer.entries.is_empty() { p.dim { "Nothing here." } }
        @if answer.total as usize > limit * page {
            p.bar {
                @if page > 1 { a href=(link(&q, &view, &kind, page - 1)) { "← previous" } }
                a href=(link(&q, &view, &kind, page + 1)) { "next →" }
            }
        }

        h2 { "Facets" }
        div.grid {
            @for name in &facets {
                @let (counts, coverage) = scope.facet(&sq, name, 8);
                @if !counts.is_empty() {
                    div.card {
                        h4 {
                            (name) " "
                            span.cover { (coverage) " of " (scope.records()) " claims" }
                        }
                        @for (v, n) in &counts {
                            div.facet {
                                a href=(link(format!("{q} {name}={v}").trim(), &view, &kind, 1)) { (v) }
                                span.n { (n) }
                            }
                        }
                    }
                }
            }
        }

        h2 { "Sources" }
        div.grid {
            @for m in &scope.members {
                div.card {
                    h4 { (m.name()) " " span.cover { (m.decl.priority.name()) } }
                    p.dim { (m.decl.why) }
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
            " · " a href=(at("/pricing")) { "pricing" }
            " · " a href=(at("/terms")) { "terms" }
            " · " a href=(at("/api/describe")) { "describe" }
            @if !site.title.is_empty() { " · " (site.title) }
        }
    };
    shell(&d.title, body)
}

fn entry_page(scope: &Tracker, scheme: &str, value: &str) -> Option<String> {
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
    let summary = scope.members.iter().find_map(|m| {
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
    let body = html! {
        p { a href=(at("/")) { "← " (d.title) } }
        h1 { (entry.title) }
        p.state { span.chip { (scheme) " " (value) } " "
            span.dim { (entry.members().len()) " sources, " (entry.parts.len()) " claims · " }
            a href={(at("/thing/")) (urlencode(scheme)) "/" (urlencode(value)) ".atom"} { "Watch" } }
        @if let Some((source, c)) = &summary {
            div.note {
                span.dim { (source) " writes:" } br;
                @let text = c.text.trim();
                @if text.chars().count() > 600 { (text.chars().take(600).collect::<String>()) "…" } @else { (text) }
                " " a href={(at("/claim/")) (urlencode(&c.dataset)) "/" (c.record_id)} { "the claim" }
            }
        }

        @if !entry.fields.is_empty() {
            h2 { "What each source says" }
            table {
                thead { tr { th { "Property" } th { "Source" } th { "Said" } th { "Means here" } } }
                tbody {
                    @for (name, f) in &entry.fields {
                        @for (member, said) in &f.by {
                            @let mapped = f.means.get(member).cloned().unwrap_or_default();
                            @for (i, raw) in said.iter().enumerate() {
                                tr {
                                    td { (name)
                                        @if judged.as_ref().map(|j| j.contains(name)).unwrap_or(f.divergent && f.by.len() > 1 && d.normalise_for(name).is_some()) { " " span.chip.on { "conflict" } }
                                        @else if f.divergent && f.by.len() > 1 && d.normalise_for(name).is_none() { " " span.chip { "not compared" } }
                                        @else if f.divergent && f.by.len() > 1 {
                                            @if f.means.values().flatten().all(|v| v.parse::<f64>().is_ok()) { " " span.chip { "within tolerance" } }
                                            @else { " " span.chip { "different words" } }
                                        } }
                                    td.dim { @if i == 0 { (member) } }
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
        }

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

fn record_page(scope: &Tracker, member: &str, id: &str) -> Option<String> {
    let rec = scope
        .records_of(member, &[id.to_string()], true)
        .into_iter()
        .next()?;
    let (source, answered) = said_by(scope, member);
    let body = html! {
        p { a href=(at("/")) { "← " (scope.decl.title) } }
        h1 { (rec.title) }
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
        @if !rec.text.trim().is_empty() { h2 { "Text" } div.text { (rec.text) } }
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
            button type="submit" { "Send the link" }
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
        p { a href=(at("/")) { "← " (scope.decl.title) } }
        h1 { (a.email) }
        p.state.(if entitled { "current" } else { "empty" }) {
            (a.state)
            @if let Some(until) = &a.paid_until { " until " (until) }
            @if !a.scopes.is_empty() { " · " (a.scopes.join(", ")) }
        }
        @if !entitled {
            div.note {
                "Reading is " (account::FREE_DELAY_DAYS) " days behind, and there are no change "
                "feeds, no API and no export. " a href=(at("/pricing")) { "What a subscription costs" } "."
            }
        }

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
        } @else {
            p { a href=(at("/pricing")) { "Subscribe" } }
        }

        p.bar { a href=(at("/signout")) { "Sign out" } }
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

fn pricing_page(scope: &Tracker, site: &Site, v: &Viewer) -> String {
    let p = &site.price;
    let body = html! {
        p { a href=(at("/")) { "← " (scope.decl.title) } }
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
        p { a href=(at("/")) { "← " (scope.decl.title) } }
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
    if !v.entitled(&scope.decl.name) {
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
    let label = |day: &str| -> String {
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
            J::Array(a) => a.iter().filter_map(J::as_str).collect::<Vec<_>>().join(", "),
            J::Object(o) => o
                .iter()
                .map(|(k, v)| format!("{k} {}", v.as_array().map(|a| a.iter().filter_map(J::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default()))
                .collect::<Vec<_>>()
                .join(" · "),
            J::String(s) => s.clone(),
            _ => "—".into(),
        }
    };
    let body = html! {
        p { a href=(at("/")) { "← " (scope.decl.title) } }
        h1 { "Changes" }
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
            h2 { (label(day)) }
            @let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
            @for s in list_of { @let _ = { *kinds.entry(s["kind"].as_str().unwrap_or("")).or_default() += 1; }; }
            p.bar { @for (k, n) in &kinds { span.chip { (words(k, *n)) } " " } }
            table { tbody {
                @for s in list_of {
                    @let kind = s["kind"].as_str().unwrap_or("");
                    @let source = s["source"].as_str().unwrap_or("");
                    @let property = s["property"].as_str().unwrap_or("");
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
}

impl TrackerSite {
    pub fn open(scope: Tracker, dir: &Path, datasets: &Path, addr: &str, operator: bool) -> Result<TrackerSite, String> {
        let accounts = Accounts::open(&scope.root)?;
        let site = Site::load(&scope.root);
        let scope = scope;
        // A tracker whose store is behind its sources, or has none, refreshes before it answers.
        if let Err(e) = scope.refresh_if_moved() {
            eprintln!("{}: the tracker store was not refreshed: {e}", dir.display());
        }
        Ok(TrackerSite {
            scope,
            dir: dir.to_path_buf(),
            datasets: datasets.to_path_buf(),
            accounts,
            site,
            read_at: crate::now(),
            addr: addr.to_string(),
            operator,
        })
    }

    /// One request. A failure is that request's, answered 500 as it is dropped, and never the
    /// end of the server.
    pub fn answer(&mut self, request: tiny_http::Request) {
        if let Err(e) = self.answer_or_fail(request) {
            eprintln!("{}: {e}", self.dir.display());
        }
    }

    fn answer_or_fail(&mut self, mut request: tiny_http::Request) -> Result<(), String> {
        let TrackerSite { scope, dir, datasets, accounts, site, read_at, addr: site_addr, operator } = self;
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
        let v = if *operator {
            Viewer::operator()
        } else {
            account::viewer_of(&accounts, cookie.as_deref(), authorization.as_deref())
        };
        let post = request.method() == &tiny_http::Method::Post;
        let mut form = String::new();
        if post {
            let _ = std::io::Read::read_to_string(request.as_reader(), &mut form);
        }

        // (body, content type, extra header)
        // A page that says "no such claim" under a 200 is telling a person one thing and
        // every machine another. 402 is the paywall, 404 is nothing there.
        let mut status = 200u16;
        let mut missing = false;
        let (body, kind, extra): (String, &str, Option<(String, String)>) = match path.as_str() {
            "/style.css" => (
                crate::serve::STYLE.to_string(),
                "text/css; charset=utf-8",
                None,
            ),
            // For the website's front page: what the overview and a thing page already show to
            // anyone, as one answer another site may fetch. Nothing gated is in it, so it is open
            // to every origin and carries no cookie.
            "/demo.json" => (
                demo(&scope).to_string(),
                "application/json",
                Some(("Access-Control-Allow-Origin".to_string(), "*".to_string())),
            ),
            "/pricing" => (
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
                        let base = if site.url.is_empty() {
                            format!("http://{addr}", addr = site_addr)
                        } else {
                            site.url.clone()
                        };
                        let link = format!("{base}{}/signin/{raw}", mounted());
                        let sent = site
                            .send(
                                &a.email,
                                "Your Zetlyn sign-in link",
                                &format!("{link}\n\nGood for a quarter of an hour, and once."),
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
            "/signin" => (signin_page(&site, None), "text/html; charset=utf-8", None),

            "/signout" => {
                if let Some(c) = &cookie {
                    if let Some(s) = c
                        .split(';')
                        .filter_map(|p| p.trim().split_once('='))
                        .find(|(k, _)| *k == "zs")
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
                        "zs=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax".to_string(),
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
                let again = account::viewer_of(&accounts, cookie.as_deref(), None);
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

            "/conflicts" | "/conflicts/mark" if !v.entitled(&scope.decl.name) => (
                pricing_page(&scope, &site, &v),
                "text/html; charset=utf-8",
                None,
            ),
            "/conflicts" => (conflicts_page(&scope, &url, &v, None), "text/html; charset=utf-8", None),
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
            "/changes" | "/changes.atom" if !v.entitled(&scope.decl.name) => (
                pricing_page(&scope, &site, &v),
                "text/html; charset=utf-8",
                None,
            ),
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
            "/things" | "/things.atom" if !v.entitled(&scope.decl.name) => (
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
            _ if parts.len() == 2 && parts[0] == "signin" => match accounts.spend_link(&parts[1]) {
                Some(session) => (
                    shell(
                        "Signed in",
                        html! { p { a href=(at("/account")) { "→ your account" } } h1 { "Signed in" } },
                    ),
                    "text/html; charset=utf-8",
                    Some((
                        "Set-Cookie".into(),
                        format!("zs={session}; Path=/; Max-Age=2592000; HttpOnly; SameSite=Lax"),
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
                if !v.entitled(&scope.decl.name) {
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
            _ if parts.len() == 3 && parts[0] == "thing" => {
                match entry_page(&scope, &parts[1], &parts[2]) {
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
            _ if parts.len() == 3 && parts[0] == "claim" => {
                match record_page(&scope, &parts[1], &parts[2]) {
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
                            p { a href=(at("/")) { "← " (scope.decl.title) } }
                            h1 { "Nothing here at that address" }
                        },
                    ),
                    "text/html; charset=utf-8",
                    None,
                )
            }
            _ => (
                overview(&scope, &url, &v, &site),
                "text/html; charset=utf-8",
                None,
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
        if gated && !v.entitled(&scope.decl.name) {
            status = 402;
        }
        if missing {
            status = 404;
        }
        let mut response = tiny_http::Response::from_string(body).with_status_code(status);
        if let Ok(h) = tiny_http::Header::from_bytes(&b"Content-Type"[..], kind.as_bytes()) {
            response = response.with_header(h);
        }
        if let Some((name, value)) = extra {
            if let Ok(h) = tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes()) {
                response = response.with_header(h);
            }
        }
        let _ = request.respond(response);
        Ok(())
    }
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
        p { a href=(at("/")) { "← " (scope.decl.title) } }
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
        p { a href=(at("/")) { "← " (scope.decl.title) } }
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
                    href={(at("/conflicts?property=")) (urlencode(name))} { (name) " " (n) }
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
                            div.why { (scheme) " " (value) }
                        }
                        td { (c["property"].as_str().unwrap_or("")) }
                        td {
                            @if let Some(o) = c["sources"].as_object() {
                                @for (source, words) in o {
                                    div { span.dim { (source) } " "
                                        strong { (words.as_array().map(|a| a.iter().filter_map(J::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default()) } }
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
    let examples = [
        "conflict:severity",
        "conflict:cvss and has:kev",
        "only:kev",
        "appeared:exploit<7d",
        "changed:severity<24h",
    ];
    let terms = top_terms(&question);
    let body = html! {
        p { a href=(at("/")) { "← " (scope.decl.title) } }
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
                placeholder="conflict:severity and has:kev";
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
                    " " a.chip href={(at("/things?q=")) (urlencode(ex))} { (ex) }
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
                                    a href={(at("/thing/")) (urlencode(&scheme)) "/" (urlencode(&value))} { (title) }
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
        p { a href=(at("/things")) { "← Things" } }
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
