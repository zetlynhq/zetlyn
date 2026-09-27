//! A scope, opened. The same three surfaces a dataset has, over members of unlike shape.

use std::collections::BTreeSet;

use maud::{html, Markup};
use serde_json::{json, Value as J};

use crate::account::{self, Accounts, Site, Viewer};
use crate::expr;
use crate::scope::{Entry, Scope, ScopeQuery};
use crate::serve::{flatten, params, shell, urlencode};

fn link(q: &str, view: &str, kind: &str, page: usize) -> String {
    let mut url = format!("/?q={}", urlencode(q));
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

fn entry_link(e: &Entry) -> Option<String> {
    let k = e.key.as_ref()?;
    Some(format!(
        "/entry/{}/{}",
        urlencode(&k.scheme),
        urlencode(&k.value)
    ))
}

fn cell(e: &Entry, name: &str) -> Markup {
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
        "publishers" | "members" => html! { (e.members().len()) },
        other => match e.fields.get(other) {
            Some(f) => {
                let distinct: Vec<&String> = {
                    let mut v: Vec<&String> = f.means.values().collect();
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

fn overview(scope: &Scope, url: &str, v: &Viewer, site: &Site) -> String {
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
    let sq = ScopeQuery {
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

    let body = html! {
        h1 { (d.title) }
        (banner(v, scope, hidden))
        @if !d.about.is_empty() { p.about { (d.about) } }
        p.state.(if stale { "partial" } else { "current" }) {
            (scope.records()) " records · " (scope.members.len()) " members · "
            @for (k, n) in scope.kinds() { (k) " " (n) " · " }
            @if let Some(f) = &d.promise.fresh_within {
                @if holds { "fresh within " (f) ", checked just now" }
                @else { "the promise of " (f) " does not hold" }
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

        form.bar method="get" action="/" {
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
            @if answer.subjects { " subjects" } @else { " records" }
        }
        @if answer.truncated {
            div.note {
                "A member had more candidates than were read. The filter is applied over the "
                "assembled subject, so what is not read is not counted, and this number is a "
                "floor. Narrow the query to get an exact one."
            }
        }
        table {
            thead { tr {
                th { "Subject" }
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
                            span.cover { (coverage) " of " (scope.records()) " records" }
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

        h2 { "Members" }
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
            " · " a href="/changes" { "changes" }
            " · " a href="/catalogue" { "catalogue" }
            " · " a href="/pricing" { "pricing" }
            " · " a href="/terms" { "terms" }
            " · " a href="/api/describe" { "describe" }
            @if !site.title.is_empty() { " · " (site.title) }
        }
    };
    shell(&d.title, body)
}

fn entry_page(scope: &Scope, scheme: &str, value: &str) -> Option<String> {
    let entry = scope.entry(scheme, value)?;
    let d = &scope.decl;
    let body = html! {
        p { a href="/" { "← " (d.title) } }
        h1 { (entry.title) }
        p.state { span.chip { (scheme) " " (value) } " "
            span.dim { (entry.members().len()) " members, " (entry.parts.len()) " records" } }

        @if !entry.fields.is_empty() {
            h2 { "What each member says" }
            table {
                thead { tr { th { "Field" } th { "Member" } th { "Said" } th { "Means here" } } }
                tbody {
                    @for (name, f) in &entry.fields {
                        @for (member, raw) in &f.by {
                            tr {
                                td { (name)
                                    @if f.divergent { " " span.chip.on { "divergent" } } }
                                td.dim { (member) }
                                td { (raw)
                                    @if let Some(means) = definition(scope, member, raw) {
                                        div.why { (means) }
                                    }
                                }
                                td {
                                    @let m = f.means.get(member).cloned().unwrap_or_default();
                                    @if f.mapped && &m != raw { (m) } @else { span.dim { "—" } }
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
                            a href={"/record/" (urlencode(&p.member)) "/" (p.record_id)} { (p.title) }
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

/// The member's own definition of its own word, which arrived with its `describe`.
fn definition(scope: &Scope, member: &str, code: &str) -> Option<String> {
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

fn record_page(scope: &Scope, member: &str, id: &str) -> Option<String> {
    let rec = scope
        .records_of(member, &[id.to_string()])
        .into_iter()
        .next()?;
    let body = html! {
        p { a href="/" { "← " (scope.decl.title) } }
        h1 { (rec.title) }
        p.state { span.chip { (member) } " " span.chip { (rec.kind) } " "
            @for i in &rec.ids { span.chip { (i.scheme) " " (i.value) } " " }
            span.dim { "known " (rec.known) } }
        @if let Some(u) = &rec.url { p { a href=(u) { (u) } } }
        @if !rec.fields.is_empty() {
            h2 { "Fields" }
            table { tbody {
                @for (name, value) in &rec.fields {
                    tr {
                        th style="width: 12rem" { (name) }
                        td { (value.display())
                            @if let Some(t) = definition(scope, member, &value.display()) {
                                div.why { (t) }
                            }
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
/// subject. A call that does not exist, answered as a search of everything, is a caller who thinks
/// they asked something and gets the answer to another question.
fn api(scope: &Scope, path: &str, url: &str, v: &Viewer) -> (J, bool) {
    let bound = account::bound(v, &scope.decl.name);
    // No API without a subscription. The overview and its counts stay current for everyone; the
    // records behind them do not.
    if bound.is_some() {
        return (
            json!({ "error": "this needs a subscription", "see": "/pricing" }),
            false,
        );
    }
    let p = params(url);
    let (text, pred) = expr::parse_query(p.get("q").map(String::as_str).unwrap_or(""));
    let sq = ScopeQuery {
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
            let field = p.get("field").cloned().unwrap_or_default();
            let (counts, coverage) = scope.facet(&sq, &field, 50);
            json!({ "field": field, "coverage": coverage, "records": scope.records(),
                    "values": J::Array(counts.iter()
                        .map(|(v, n)| json!({ "value": v, "records": n })).collect()) })
        }
        // One subject, which is the page a reader opens and had no call of its own.
        _ if path.starts_with("/api/entry/") => {
            let rest: Vec<&str> = path["/api/entry/".len()..].splitn(2, '/').collect();
            match rest.as_slice() {
                [scheme, value] => {
                    let value = crate::serve::urldecode(value);
                    match scope.entry(scheme, &value) {
                        Some(e) => entry_json(&e),
                        None => {
                            return (
                                json!({ "error": "no such subject",
                                        "scheme": scheme, "value": value }),
                                true,
                            )
                        }
                    }
                }
                _ => {
                    return (
                        json!({ "error": "an entry is named by a scheme and a value",
                                "example": "/api/entry/cve/CVE-2021-44228" }),
                        true,
                    )
                }
            }
        }
        "/api/search" => {
            let answer = scope.search(&sq);
            json!({
                "total": answer.total,
                "counts": if answer.subjects { "subjects" } else { "records" },
                "at_least": answer.truncated,
                "answered": answer.answered,
                "unanswered": J::Array(answer.unanswered.iter()
                    .map(|(m, w)| json!({ "member": m, "why": w })).collect()),
                "entries": J::Array(answer.entries.iter().map(entry_json).collect()),
            })
        }
        _ => {
            return (
                json!({ "error": "no such call", "calls": [
                    "/api/describe", "/api/search", "/api/entry/{scheme}/{value}",
                    "/api/facet", "/api/changes", "/api/mark",
                ] }),
                true,
            )
        }
    };
    (answer, false)
}

/// One assembled subject, the same shape whether it arrives alone or inside a search.
fn entry_json(e: &crate::scope::Entry) -> J {
    json!({
        "rank": e.rank,
        "key": e.key.as_ref().map(|k| json!({ "scheme": k.scheme, "value": k.value })),
        "title": e.title,
        "why": e.why,
        "records": J::Array(e.parts.iter().map(|p| json!({
            "member": p.member, "kind": p.kind, "record_id": p.record_id,
            "title": p.title, "url": p.url, "known": p.known,
        })).collect()),
        "fields": J::Object(e.fields.iter().map(|(name, f)| (name.clone(), json!({
            "by": f.by, "means": f.means, "divergent": f.divergent,
        }))).collect()),
    })
}

// ---------------------------------------------------------------------------------------------
// Who is asking, and what they are paying for.

fn banner(v: &Viewer, scope: &Scope, hidden: u64) -> Markup {
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
                    @else { a.chip href="/account" { (mail) } }
                }
                None => {
                    span.chip { "free" }
                    a.chip href="/signin" { "Sign in" }
                }
            }
            @if hidden > 0 {
                a.chip href="/pricing" {
                    (hidden) " records are newer than " (account::FREE_DELAY_DAYS)
                    " days and need a subscription"
                }
            }
        }
    }
}

fn signin_page(site: &Site, message: Option<&str>) -> String {
    let body = html! {
        p { a href="/" { "←" } }
        h1 { "Sign in" }
        p.about {
            "Type your address and a link arrives. There is no password, so there is nothing to "
            "forget and nothing anybody can take."
        }
        @if let Some(m) = message { div.note { (m) } }
        form.bar method="post" action="/signin" {
            input type="search" name="email" placeholder="you@example.com";
            button type="submit" { "Send the link" }
        }
        @if !site.contact.is_empty() { p.dim { "Trouble: " (site.contact) } }
    };
    shell("Sign in", body)
}

fn account_page(scope: &Scope, accounts: &Accounts, site: &Site, v: &Viewer) -> String {
    let Some(a) = v.account.clone() else {
        return signin_page(site, None);
    };
    let keys = accounts.keys(a.id);
    let entitled = a.entitled(&scope.decl.name);
    let body = html! {
        p { a href="/" { "← " (scope.decl.title) } }
        h1 { (a.email) }
        p.state.(if entitled { "current" } else { "empty" }) {
            (a.state)
            @if let Some(until) = &a.paid_until { " until " (until) }
            @if !a.scopes.is_empty() { " · " (a.scopes.join(", ")) }
        }
        @if !entitled {
            div.note {
                "Reading is " (account::FREE_DELAY_DAYS) " days behind, and there are no change "
                "feeds, no API and no export. " a href="/pricing" { "What a subscription costs" } "."
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
                            form method="post" action="/account/key/drop" {
                                input type="hidden" name="name" value=(name);
                                button type="submit" { "Revoke" }
                            }
                        }
                    }
                }
            } }
            @if keys.is_empty() { p.dim { "None yet." } }
            form.bar method="post" action="/account/key" {
                input type="search" name="name" placeholder="what this key is for";
                button type="submit" { "Make a key" }
            }
            p.dim { "Send it as " code { "Authorization: Bearer zk_…" } "." }
        }

        h2 { "Subscription" }
        @if entitled {
            form method="post" action="/account/cancel" {
                button type="submit" { "Cancel" }
            }
            p.dim {
                "Cancelling stops the next payment. What you already hold you keep until "
                @match &a.paid_until { Some(u) => (u), None => "the end of the period" } "."
            }
        } @else {
            p { a href="/pricing" { "Subscribe" } }
        }

        p.bar { a href="/signout" { "Sign out" } }
    };
    shell(&a.email, body)
}

fn key_made(key: &str) -> String {
    let body = html! {
        p { a href="/account" { "← account" } }
        h1 { "Your key" }
        div.note { "Shown once. Zetlyn keeps its hash and cannot show it again." }
        p { code { (key) } }
        p.dim { "Send it as " code { "Authorization: Bearer " (key) } "." }
    };
    shell("Your key", body)
}

fn pricing_page(scope: &Scope, site: &Site, v: &Viewer) -> String {
    let p = &site.price;
    let body = html! {
        p { a href="/" { "← " (scope.decl.title) } }
        h1 { "What it costs" }
        p.about {
            "Free is the whole of this scope, " (account::FREE_DELAY_DAYS) " days behind. "
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
                p.dim { "One person, this scope." }
                div.facet { span { "Browse and search" } span.n { "current" } }
                div.facet { span { "Change feeds" } span.n { "Atom, webhook, command" } }
                div.facet { span { "API and export" } span.n { "yes" } }
            }
            div.card {
                h4 { (p.currency) (p.team) " a month" }
                p.dim { "A team, every scope this deployment serves." }
                div.facet { span { "Everything above" } span.n { "yes" } }
                div.facet { span { "Keys" } span.n { "as many as you need" } }
            }
        }
        @if p.buy.is_empty() {
            div.note {
                "Card payment is not connected on this deployment. Write to "
                @if site.contact.is_empty() { "the operator" } @else { (site.contact) }
                " and a subscription is set by hand, which is what the first ones are anyway."
            }
        } @else {
            p.bar { a.chip.on href=(p.buy) { "Subscribe" } }
        }
        p.dim {
            "What is sold is currency and coverage. "
            a href="/terms" { "Terms" } " · "
            @if v.email().is_some() { a href="/account" { "Your account" } }
            @else { a href="/signin" { "Sign in" } }
        }
    };
    shell("Pricing", body)
}

fn terms_page(scope: &Scope, site: &Site) -> String {
    let who = if site.contact.is_empty() {
        "the operator of this deployment"
    } else {
        &site.contact
    };
    let body = html! {
        p { a href="/" { "← " (scope.decl.title) } }
        h1 { "Terms" }
        div.note {
            "A draft. It says what this deployment actually does, and it has not been read by a "
            "lawyer. Anybody selling from it should have one read it first."
        }
        h2 { "What is sold" }
        p { "A subscription to " (scope.decl.title) ": the records as they stand rather than "
            (account::FREE_DELAY_DAYS) " days behind, the change feeds, the API and export." }
        p { (scope.decl.promise.covers) }
        @if !scope.decl.promise.excludes.is_empty() { p { "Not included: " (scope.decl.promise.excludes) } }
        h2 { "What is not promised" }
        p { "No availability guarantee, and no undertaking about how fast a change at a publisher "
            "reaches this deployment beyond what the scope states and shows. Every source is a third "
            "party and may change or stop without notice; where one does, the overview says so." }
        p { "The data is what publishers said. It is not advice, and nothing here decides which of "
            "two publishers is right." }
        h2 { "Money" }
        p { "Monthly, in advance, cancellable at any time and effective at the end of the paid "
            "period. No refund for a part-used month. Prices include VAT where it applies." }
        h2 { "What happens when you stop" }
        p { "Access to current records, feeds, the API and export ends. Anything already exported "
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
fn export(scope: &Scope, url: &str, v: &Viewer, as_csv: bool) -> Option<(String, &'static str)> {
    if !v.entitled(&scope.decl.name) {
        return None;
    }
    let p = params(url);
    let (text, pred) = expr::parse_query(p.get("q").map(String::as_str).unwrap_or(""));
    let sq = ScopeQuery {
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
                    "key": e.key.as_ref().map(|k| format!("{}:{}", k.scheme, k.value)),
                    "title": e.title,
                    "members": e.members(),
                    "fields": J::Object(e.fields.iter()
                        .map(|(n, f)| (n.clone(), json!({ "by": f.by, "means": f.means,
                                                          "divergent": f.divergent }))).collect()),
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
    let mut out = String::from("key,title,members");
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
                    let mut v: Vec<&String> = f.means.values().collect();
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

/// Atom, for a reader. The same entries a webhook receives, in the shape a feed reader expects.
fn atom(title: &str, self_url: &str, entries: &[J], updated: &str) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    out.push_str("<feed xmlns=\"http://www.w3.org/2005/Atom\">\n");
    out.push_str(&format!("  <title>{}</title>\n", escape(title)));
    out.push_str(&format!("  <id>urn:zetlyn:{}</id>\n", escape(self_url)));
    out.push_str(&format!("  <updated>{updated}</updated>\n"));
    out.push_str(&format!(
        "  <link rel=\"self\" href=\"{}\"/>\n",
        escape(self_url)
    ));
    for e in entries.iter().rev() {
        let title = e["title"].as_str().unwrap_or("");
        let key = e["key"]["value"].as_str().unwrap_or("");
        let empty = Vec::new();
        let mut body = String::new();
        for c in e["changes"].as_array().unwrap_or(&empty) {
            let member = c["member"].as_str().unwrap_or("");
            let how = c["how"].as_str().unwrap_or("");
            body.push_str(&format!("{member}: {how}\n"));
            for f in c["fields"].as_array().unwrap_or(&empty) {
                body.push_str(&format!(
                    "  {}: {} → {}\n",
                    f["field"].as_str().unwrap_or(""),
                    f["was"].as_str().unwrap_or("—"),
                    f["is"].as_str().unwrap_or("—"),
                ));
            }
        }
        if body.is_empty() {
            body.push_str(e["how"].as_str().unwrap_or("changed"));
        }
        out.push_str("  <entry>\n");
        out.push_str(&format!("    <title>{}</title>\n", escape(title)));
        out.push_str(&format!("    <id>urn:zetlyn:{}</id>\n", escape(key)));
        out.push_str(&format!("    <updated>{updated}</updated>\n"));
        out.push_str(&format!(
            "    <content type=\"text\">{}</content>\n",
            escape(&body)
        ));
        out.push_str("  </entry>\n");
    }
    out.push_str("</feed>\n");
    out
}

fn changes_page(scope: &Scope, url: &str) -> String {
    let p = params(url);
    let since = p
        .get("since")
        .cloned()
        .unwrap_or_else(|| scope.mark_before());
    let report = scope.changes(&since, 200);
    let empty = Vec::new();
    let entries = report["entries"].as_array().unwrap_or(&empty);
    let without: Vec<&str> = report["without_history"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(J::as_str)
        .collect();
    let body = html! {
        p { a href="/" { "← " (scope.decl.title) } }
        h1 { "Changes" }
        p.dim { "Since " (since) ". The mark now is " (scope.mark()) "." }
        p.bar {
            a.chip href={"/changes.atom?since=" (urlencode(&since))} { "Atom" }
            a.chip href={"/api/changes?since=" (urlencode(&since))} { "JSON" }
        }
        @if !without.is_empty() {
            div.note {
                "These members keep no history, so a change from them says that a record moved and "
                "not what in it did: " (without.join(", ")) "."
            }
        }
        @if entries.is_empty() { p.dim { "Nothing since then." } }
        @for e in entries {
            @let key = e["key"]["value"].as_str().unwrap_or("");
            @let scheme = e["key"]["scheme"].as_str().unwrap_or("");
            h3 {
                @if key.is_empty() { (e["title"].as_str().unwrap_or("")) }
                @else {
                    a href={"/entry/" (urlencode(scheme)) "/" (urlencode(key))} {
                        (e["title"].as_str().unwrap_or(""))
                    }
                }
            }
            table { tbody {
                @for c in e["changes"].as_array().unwrap_or(&empty) {
                    tr {
                        td style="width: 10rem" {
                            span.chip { (c["how"].as_str().unwrap_or("")) } " "
                            span.dim { (c["member"].as_str().unwrap_or("")) }
                        }
                        td {
                            @let moved = c["fields"].as_array();
                            @match moved {
                                Some(fields) if !fields.is_empty() => {
                                    @for f in fields {
                                        div {
                                            (f["field"].as_str().unwrap_or("")) ": "
                                            span.dim { (f["was"].as_str().unwrap_or("—")) }
                                            " → " strong { (f["is"].as_str().unwrap_or("—")) }
                                        }
                                    }
                                }
                                _ => span.dim { "the record moved; this member keeps no history" },
                            }
                        }
                    }
                }
            } }
        }
    };
    shell("Changes", body)
}

fn watch_feed(scope: &Scope, name: &str) -> Option<String> {
    let w = crate::watch::all(&scope.root)
        .into_iter()
        .find(|w| w.decl.name == name)?;
    let state = w.state();
    let title = if w.decl.title.is_empty() {
        w.decl.name.clone()
    } else {
        w.decl.title.clone()
    };
    Some(atom(
        &title,
        &format!("/watch/{name}.atom"),
        &state.delivered,
        &crate::iso_stamp(crate::now()),
    ))
}
pub fn serve(scope: Scope, addr: &str) -> Result<(), String> {
    let accounts = Accounts::open(&scope.root)?;
    let site = Site::load(&scope.root);
    let server = tiny_http::Server::http(addr).map_err(|e| e.to_string())?;
    println!("{} on http://{addr}", scope.decl.name);
    if site.mail.run.is_empty() {
        println!("no mailer named in zetlyn.toml, so sign-in links are printed here");
    }

    for mut request in server.incoming_requests() {
        let url = request.url().to_string();
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
        let v = account::viewer_of(&accounts, cookie.as_deref(), authorization.as_deref());
        let post = request.method() == &tiny_http::Method::Post;
        let mut form = String::new();
        if post {
            let _ = std::io::Read::read_to_string(request.as_reader(), &mut form);
        }

        // (body, content type, extra header)
        // A page that says "no such record" under a 200 is telling a person one thing and
        // every machine another. 402 is the paywall, 404 is nothing there.
        let mut status = 200u16;
        let mut missing = false;
        let (body, kind, extra): (String, &str, Option<(String, String)>) = match path.as_str() {
            "/style.css" => (
                crate::serve::STYLE.to_string(),
                "text/css; charset=utf-8",
                None,
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
            "/catalogue/dataset" | "/catalogue/scope"
                if post && !v.account.as_ref().is_some_and(|a| a.curator) =>
            {
                (
                    catalogue(&scope, &v, Some("That needs a curator.")),
                    "text/html; charset=utf-8",
                    None,
                )
            }
            "/catalogue/dataset" if post => {
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
            "/catalogue/scope" if post => {
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
                match accounts.ensure(&email) {
                    Ok(a) => {
                        let raw = accounts.new_link(a.id)?;
                        let base = if site.url.is_empty() {
                            format!("http://{addr}")
                        } else {
                            site.url.clone()
                        };
                        let link = format!("{base}/signin/{raw}");
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
                        html! { p { a href="/" { "← back" } } h1 { "Signed out" } },
                    ),
                    "text/html; charset=utf-8",
                    Some((
                        "Set-Cookie".into(),
                        "zs=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax".into(),
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

            "/changes" | "/changes.atom" if !v.entitled(&scope.decl.name) => (
                pricing_page(&scope, &site, &v),
                "text/html; charset=utf-8",
                None,
            ),
            "/changes" => (changes_page(&scope, &url), "text/html; charset=utf-8", None),
            "/changes.atom" => {
                let p = params(&url);
                let since = p
                    .get("since")
                    .cloned()
                    .unwrap_or_else(|| scope.mark_before());
                let report = scope.changes(&since, 200);
                let empty = Vec::new();
                let entries = report["entries"].as_array().unwrap_or(&empty);
                (
                    atom(
                        &scope.decl.title,
                        "/changes.atom",
                        entries,
                        &crate::iso_stamp(crate::now()),
                    ),
                    "application/atom+xml; charset=utf-8",
                    None,
                )
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
                        html! { p { a href="/account" { "→ your account" } } h1 { "Signed in" } },
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
            _ if parts.len() == 3 && parts[0] == "entry" => {
                match entry_page(&scope, &parts[1], &parts[2]) {
                    Some(html) => (html, "text/html; charset=utf-8", None),
                    None => {
                        missing = true;
                        (
                            shell("Not here", html! { h1 { "No such subject" } }),
                            "text/html; charset=utf-8",
                            None,
                        )
                    }
                }
            }
            _ if parts.len() == 3 && parts[0] == "record" => {
                match record_page(&scope, &parts[1], &parts[2]) {
                    Some(html) => (html, "text/html; charset=utf-8", None),
                    None => {
                        missing = true;
                        (
                            shell("Not here", html! { h1 { "No such record" } }),
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
                            p { a href="/" { "← " (scope.decl.title) } }
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
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The catalogue: what this deployment holds, and what somebody may add to it.

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

fn catalogue(scope: &Scope, v: &Viewer, message: Option<&str>) -> String {
    let root = &scope.root;
    let datasets = crate::scope::registry(&root.join("datasets"));
    let scopes = crate::scope::scope_registry(&root.join("scopes"));
    let may = v.account.as_ref().is_some_and(|a| a.curator);

    let held: Vec<(String, String, u64, String)> = datasets
        .iter()
        .filter_map(|(name, dir)| {
            // Four values, not a description. A full describe() walks every field of every
            // dataset, and this page shows none of that.
            let ds = crate::dataset::Dataset::open(dir).ok()?;
            Some((
                name.clone(),
                ds.decl.kind.clone(),
                ds.store.count(),
                ds.state().to_string(),
            ))
        })
        .collect();

    let body = html! {
        p { a href="/" { "← " (scope.decl.title) } }
        h1 { "The catalogue" }
        p.about {
            "Every dataset and scope this deployment holds. A dataset belongs to no scope: several "
            "may name it, and the cost of a source is paid by whoever fetches it rather than by "
            "each scope again."
        }
        @if let Some(m) = message { div.note { (m) } }

        h2 { "Datasets" }
        table {
            thead { tr { th { "Name" } th { "Kind" } th { "Records" } th { "State" } } }
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

        h2 { "Scopes" }
        ul {
            @for (name, _) in &scopes { li { (name) } }
        }
        @if scopes.is_empty() { p.dim { "None." } }

        @if !may {
            div.note {
                "Adding a dataset or composing a scope needs a curator. "
                @match v.email() {
                    Some(mail) => { (mail) " is not one yet." }
                    None => { a href="/signin" { "Sign in" } " and ask the operator." }
                }
            }
        } @else {
            h2 { "Add a dataset" }
            p.dim {
                "A link to a CSV or to a feed. The declaration is proposed from what is behind it, "
                "and the scheduler fills the store on its next tick."
            }
            form.bar method="post" action="/catalogue/dataset" {
                input type="search" name="url" placeholder="https://…/something.csv";
                input type="search" name="name" placeholder="owner/name";
                input type="search" name="kind" placeholder="what one record is";
                button type="submit" { "Propose it" }
            }

            h2 { "Compose a scope" }
            p.dim {
                "Pick the members and say what makes two of their records the same thing. Every "
                "member needs a sentence saying what it contributes that the others do not: a "
                "member nobody can justify in a sentence is one somebody added and nobody removed."
            }
            form method="post" action="/catalogue/scope" {
                p.bar {
                    input type="search" name="name" placeholder="owner/name";
                    input type="search" name="title" placeholder="Title";
                    input type="search" name="key" placeholder="the identifier scheme to join on";
                }
                p.bar { input type="search" name="about" placeholder="What this subject is, in one sentence"; }
                table { tbody {
                    @for (name, kind, records, _) in &held {
                        tr {
                            td style="width: 2rem" {
                                input type="checkbox" name="member" value=(name);
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
fn add_dataset(scope: &Scope, form: &str) -> Result<String, String> {
    let url = form_field(form, "url");
    if url.trim().is_empty() {
        return Err("a link is needed".into());
    }
    let name = form_field(form, "name");
    let kind = form_field(form, "kind");
    let dir = scope
        .root
        .join("datasets")
        .join(slug(if name.trim().is_empty() { &url } else { &name }));
    if dir.join("dataset.toml").exists() {
        return Err(format!("{} already holds a dataset", dir.display()));
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

/// Members, a key, and a sentence each. The sentence is required here because it is required in
/// the format, and a form that let somebody skip it would be a way around the rule.
fn add_scope(scope: &Scope, form: &str) -> Result<String, String> {
    let name = form_field(form, "name");
    if !name.contains('/') {
        return Err("a scope is named owner/name".into());
    }
    let members = form_fields(form, "member");
    if members.is_empty() {
        return Err("a scope with no members is a scope about nothing".into());
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
            "these members have no sentence saying why they belong: {}",
            missing.join(", ")
        ));
    }

    let title = form_field(form, "title");
    let about = form_field(form, "about");
    let key = form_field(form, "key");
    let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));

    let mut toml = format!(
        "name  = {}\ntitle = {}\nabout = {}\n",
        quote(name.trim()),
        quote(if title.trim().is_empty() {
            name.trim()
        } else {
            title.trim()
        }),
        quote(about.trim())
    );
    for (m, why) in members.iter().zip(&whys) {
        toml.push_str(&format!(
            "\n[[members]]\ndataset  = {}\npriority = \"normal\"\nwhy      = {}\n",
            quote(m),
            quote(why.trim())
        ));
    }
    if !key.trim().is_empty() {
        toml.push_str(&format!("\n[[join]]\nkey = {}\n", quote(key.trim())));
    }
    toml.push_str("\n[view]\ncolumns = [\"kind\", \"known\"]\nfacets  = [\"kind\", \"dataset\"]\n");
    toml.push_str("\n[promise]\nfresh_within = \"24h\"\ncovers       = \"\"\n");

    let dir = scope.root.join("scopes").join(slug(name.trim()));
    if dir.join("scope.toml").exists() {
        return Err(format!("{} already holds a scope", dir.display()));
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join("scope.toml");
    std::fs::write(&path, toml).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(format!(
        "{} is composed. Its promise says nothing yet, which is the one thing a curator has to \
         write themselves.",
        path.display()
    ))
}
