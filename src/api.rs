//! Zetlyn for programs and LLMs: the same few things a person does on the site, as JSON at
//! `/api/v1/…` and as tools of an MCP server at `/mcp`. Both are opened by an API key
//! (`Authorization: Bearer zk_…`), which a person makes under "Your watches"; both act as that
//! person's account and nothing else.
//!
//! The operations are written once, here, and the two doors only translate: what the API answers
//! is what an MCP tool returns.

use serde_json::{json, Value as J};

use crate::anchor;
use crate::check;
use crate::life;
use crate::store::{Account, Mode, Store, Watch};

/// An answer: a status and a JSON body (an `error` field when it went wrong).
pub type Answer = (u16, J);

fn error(status: u16, message: &str) -> Answer {
    (status, json!({ "error": message }))
}

/// What the caller is, for the operations: the account, and the instance's hosted part if any.
pub struct Caller<'a> {
    pub store: &'a Store,
    pub account: Account,
    pub hosted: Option<&'a life::Hosted>,
    pub plan: &'a crate::plan::Plan,
    pub every_min: i64,
    pub base: String,
}

/// `/api/v1/<path>`: GET account | GET watches | POST watches | GET watches/<id> | PATCH watches/<id> |
/// DELETE watches/<id> | POST preview | POST read | GET changes.
pub fn rest(c: &Caller, method: &str, path: &[&str], body: &J) -> Answer {
    match (method, path) {
        ("GET", ["account"]) => account(c),
        ("POST", ["preview"]) => preview(c, body["url"].as_str().unwrap_or(""), body["find"].as_str()),
        ("GET", ["watches"]) => list(c),
        ("POST", ["watches"]) => watch(c, body["url"].as_str().unwrap_or(""), body["selector"].as_str(), body["every_minutes"].as_i64(), body["confirm_cost"].as_bool().unwrap_or(false), body.get("notify_when").unwrap_or(&J::Null)),
        ("POST", ["read"]) => read_place(c, body["url"].as_str().unwrap_or(""), &selectors_of(body)),
        ("GET", ["changes"]) => changes(c, body["since"].as_str()),
        ("POST", ["subscribe-link"]) => subscribe_link(c),
        ("GET", ["watches", id]) => get(c, id),
        ("PATCH", ["watches", id]) => update(c, id, body["selector"].as_str(), body["whole_page"].as_bool().unwrap_or(false), body["every_minutes"].as_i64(), body.get("notify_when")),
        ("DELETE", ["watches", id]) => end(c, id),
        _ => error(404, "No such call. See https://zetlyn.com/llms.txt"),
    }
}

fn account(c: &Caller) -> Answer {
    let watches = c.store.watches_of(c.account.id).unwrap_or_default();
    let running = watches.iter().filter(|w| w.state == "running").count();
    let mut out = json!({ "email": c.account.email, "watches": watches.len(), "watches_running": running });
    if let (Some(o), J::Object(more)) = (out.as_object_mut(), c.plan.api_account(&c.account, &c.base)) {
        o.extend(more);
    }
    (200, out)
}

fn address(typed: &str) -> Result<String, String> {
    let t = typed.trim();
    if t.is_empty() {
        return Err("Give the page's address as `url`.".into());
    }
    let url = crate::fetch::clean(&if t.contains("://") { t.to_string() } else { format!("https://{t}") });
    crate::outbound::public_target(&url).map(|_| url)
}

fn preview(_c: &Caller, url: &str, find: Option<&str>) -> Answer {
    let url = match address(url) {
        Ok(u) => u,
        Err(e) => return error(400, &e),
    };
    let (html, rendered) = match check::seen(&url) {
        Ok(h) => h,
        Err(e) => return error(422, &format!("Zetlyn could not read this page: {e}")),
    };
    let find = find.map(str::trim).filter(|f| !f.is_empty());
    let places: Vec<J> = anchor::places(&html, 150, find).into_iter().map(|a| json!({ "selector": a.path, "text": a.text, "beside": a.label })).collect();
    // A page with nothing on it to mark is said so, as a watch of it would be.
    if places.is_empty() && find.is_none() {
        return error(422, check::NO_TEXT);
    }
    let page_text = crate::text::lines(&html).join("\n");
    let lines = page_text.lines().count();
    (
        200,
        json!({
            "url": url,
            "places": places,
            "whole_page_lines": lines,
            "whole_page_watchable": check::watchable_whole(&page_text),
            "rendered": rendered,
            "find": find,
            "how": match (find, places.is_empty()) {
                (Some(_), true) => "No place on the page says that. Try another word (a currency sign, a word beside the value), or leave find out to see the page's places.",
                _ => "To watch one place, pass its `selector` to create a watch. To watch the whole page, pass no selector. The page's own content comes first, its menus, header and footer last; with find, only the places that say it.",
            },
        }),
    )
}

/// The selectors a call names: `selectors` (a list) or `selector` (one).
fn selectors_of(args: &J) -> Vec<String> {
    let mut out: Vec<String> = args["selectors"].as_array().map(|a| a.iter().filter_map(|s| s.as_str()).map(str::to_string).collect()).unwrap_or_default();
    if let Some(s) = args["selector"].as_str() {
        out.push(s.to_string());
    }
    out.retain(|s| !s.trim().is_empty());
    out
}

/// What places on a page say now, read twice a few seconds apart, without watching anything: so
/// a place can be tried before a watch is made, and a place that flips between two texts on its
/// own (`steady: false`) is seen before it is watched.
fn read_place(_c: &Caller, url: &str, selectors: &[String]) -> Answer {
    if selectors.is_empty() {
        return error(400, "Give the places to read as `selectors` (a list), from preview_page.");
    }
    if selectors.len() > 10 {
        return error(400, "At most 10 selectors at a time.");
    }
    let url = match address(url) {
        Ok(u) => u,
        Err(e) => return error(400, &e),
    };
    let (html, rendered) = match check::seen(&url) {
        Ok(h) => h,
        Err(e) => return error(422, &format!("Zetlyn could not read this page: {e}")),
    };
    std::thread::sleep(std::time::Duration::from_secs(3));
    let again = check::read_as(&url, rendered).ok();
    let places: Vec<J> = selectors
        .iter()
        .map(|css| match anchor::at_counted(&html, css.trim()) {
            Err(e) => json!({ "selector": css, "error": e }),
            Ok((a, n)) => {
                let mode = Mode::Place(a.clone());
                let now = check::value(&mode, &html).map(|(v, _)| v).unwrap_or_default();
                let then = again.as_deref().and_then(|h| check::value(&mode, h).ok()).map(|(v, _)| v);
                let mut out = json!({ "selector": css, "text": now.chars().take(2000).collect::<String>(), "beside": a.label, "empty": now.trim().is_empty(), "matches": n });
                if n > 1 {
                    out["note"] = J::String(anchor::many_said(css.trim(), n));
                }
                match then {
                    Some(t) if t == now => out["steady"] = J::Bool(true),
                    Some(t) => {
                        out["steady"] = J::Bool(false);
                        out["second_read"] = J::String(t.chars().take(2000).collect());
                    }
                    None => out["steady"] = J::Null,
                }
                out
            }
        })
        .collect();
    (
        200,
        json!({
            "url": url,
            "rendered": rendered,
            "places": places,
            "how": "Nothing is watched by this. A place with steady: false said something else on a second read seconds later: the site changes it on its own, so watching it would report flips; prefer a narrower place inside it.",
        }),
    )
}

/// A link for the person to subscribe by, in their own browser without signing in there: the
/// account is in the link. Paying stays theirs to do, at Stripe, with the consent the law asks for
/// given on the page; the link can do nothing else, and lasts an hour.
fn subscribe_link(c: &Caller) -> Answer {
    if c.hosted.is_none() || c.plan.stripe.is_none() {
        return error(404, "This instance takes no payment; there is nothing to subscribe to.");
    }
    if c.account.paid {
        return (200, json!({ "subscribed": true, "billing_url": format!("{}/account/billing", c.base), "note": "The account subscribes already. Invoices, payment and cancelling are at billing_url, for the person to open signed in." }));
    }
    match c.store.new_token_for(c.account.id, "subscribe", "", 3600) {
        Ok(token) => (
            200,
            json!({
                "subscribed": false,
                "url": format!("{}/subscribe/{token}", c.base),
                "price": "9.99 € a month for up to 10 watches, 0.50 € a month for each one more, VAT included; cancel any time to the end of the month",
                "expires_in_minutes": 60,
                "note": "Give the person this link to open themselves; do not open it for them. They agree to the terms and pay at Stripe. Watches waiting for the subscription start the moment the payment is through; get_account then says subscribed: true.",
            }),
        ),
        Err(e) => error(500, &e),
    }
}

/// The interval asked for, or why it cannot be had.
fn interval(c: &Caller, every: i64) -> Result<i64, Answer> {
    if every != c.every_min && !crate::plan::INTERVALS.contains(&every) {
        return Err(error(400, "every_minutes is 60, 15, 5 or 1."));
    }
    if every != c.every_min && !crate::plan::interval_allowed(&c.account, c.hosted.is_some(), every) {
        return Err(error(403, "Checking more often than every hour comes with the subscription. Leave every_minutes out to check every hour."));
    }
    Ok(every)
}

/// A watch changed in place: another place on its page (or the whole page), how often it is
/// checked, or both. Its id and history stay; the free trial is not spent again.
fn update(c: &Caller, id: &str, selector: Option<&str>, whole_page: bool, every: Option<i64>, notify_when: Option<&J>) -> Answer {
    let Some(w) = own(c, id) else { return error(404, "No such watch on this account.") };
    let selector = selector.map(str::trim).filter(|s| !s.is_empty());
    if selector.is_none() && !whole_page && every.is_none() && notify_when.is_none() {
        return error(400, "Give a new `selector`, `whole_page: true`, `every_minutes` or `notify_when`.");
    }
    if selector.is_some() && whole_page {
        return error(400, "Give either a selector or whole_page, not both.");
    }
    let every = match every.map(|e| interval(c, e)).transpose() {
        Ok(e) => e,
        Err(a) => return a,
    };
    let condition = match notify_when.map(crate::when::Condition::from_json).transpose() {
        Ok(k) => k,
        Err(e) => return error(400, &e),
    };
    // A number is read in one place; the whole page has many.
    let page_after = whole_page || (selector.is_none() && matches!(w.mode, Mode::Page { .. }));
    if page_after && condition.as_ref().is_some_and(|k| k.as_ref().is_some_and(|k| k.needs_number())) {
        return error(400, "below and above need one place with a number (a selector); a whole page can be watched with contains or not_contains.");
    }
    // The condition the watch will have, weighed on what its place will say: refused before anything changes.
    let wanted = condition.clone().unwrap_or_else(|| w.condition.clone());
    let mut note = Vec::new();
    let mut many: Option<String> = None;
    if selector.is_none() && !whole_page {
        if let Some(Err(e)) = condition.as_ref().and_then(|k| k.as_ref()).map(|k| k.fits(w.last_value.as_deref().unwrap_or(""))) {
            return error(422, &e);
        }
    }
    if selector.is_some() || whole_page {
        let anchor = match selector {
            Some(css) => match check::seen(&w.url).and_then(|(html, _)| anchor::at_counted(&html, css)) {
                Ok((a, n)) => {
                    if n > 1 {
                        many = Some(anchor::many_said(css, n));
                    }
                    Some(a)
                }
                Err(e) => return error(422, &format!("The selector marks nothing on the page: {e}. Call preview_page or read_place for the places there are.")),
            },
            None => None,
        };
        let (mode, value, render) = match check::first(&w.url, anchor) {
            Ok(x) => x,
            Err(e) => return error(422, &e),
        };
        if let Some(Err(e)) = wanted.as_ref().filter(|_| !whole_page).map(|k| k.fits(&value)) {
            return error(422, &e);
        }
        if let Err(e) = c.store.remark(w.id, &mode, &value).and_then(|_| c.store.set_render(w.id, render)) {
            return error(500, &e);
        }
        note.push(if whole_page { "Now watching the whole page." } else { "Now watching the new place." });
    }
    if let Some(e) = every {
        if let Err(x) = c.store.set_interval(w.id, e) {
            return error(500, &x);
        }
        note.push("Checked at the new interval from the next check.");
    }
    let keep = condition.clone().unwrap_or_else(|| w.condition.clone());
    let keep = if page_after && keep.as_ref().is_some_and(|k| k.needs_number()) {
        note.push("Its number condition no longer applies to the whole page and was removed.");
        None
    } else {
        keep
    };
    if keep != w.condition {
        if let Err(x) = c.store.set_condition(w.id, keep.as_ref()) {
            return error(500, &x);
        }
        if condition.is_some() {
            note.push(match &keep { Some(_) => "Changes are told when the condition comes true.", None => "Every change is told." });
        }
    }
    let Some(w) = c.store.watch(w.id).ok().flatten() else { return error(500, "the watch is not there") };
    (200, json!({ "watch": watch_json(&w), "note": with_many(note.join(" "), &many) }))
}

/// The changes on all of the account's watches since a moment (the last 7 days without one),
/// newest first.
fn changes(c: &Caller, since: Option<&str>) -> Answer {
    let since = match since.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => match moment(s) {
            Some(t) => t,
            None => return error(400, "`since` is a moment in RFC 3339, e.g. 2026-10-20T08:00:00Z."),
        },
        None => crate::store::now() - 7 * 86400,
    };
    match c.store.changes_of(c.account.id, since, 200) {
        Ok(all) => (
            200,
            json!({
                "since": iso(since),
                "changes": all.iter().map(|(key, url, ch)| { let mut j = change_json(ch); j["watch"] = J::from(key.as_str()); j["url"] = J::from(url.as_str()); j }).collect::<Vec<_>>(),
            }),
        ),
        Err(e) => error(500, &e),
    }
}

/// A moment written in RFC 3339 (`2026-10-20T08:00:00Z`, an offset, or only a date), as seconds.
fn moment(s: &str) -> Option<i64> {
    let num = |t: &str| t.parse::<i64>().ok();
    let (date, rest) = s.split_at(s.len().min(10));
    let mut d = date.split('-');
    let (y, m, day) = (num(d.next()?)?, num(d.next()?)?, num(d.next()?)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }
    // Howard Hinnant's days-from-civil.
    let yy = if m <= 2 { y - 1 } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468;
    let rest = rest.trim_start_matches(['T', 't', ' ']);
    if rest.is_empty() {
        return Some(days * 86400);
    }
    let (clock, offset) = match rest.find(['Z', 'z', '+', '-']) {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let mut c = clock.split('.').next()?.split(':');
    let (h, mi, sec) = (num(c.next()?)?, num(c.next()?)?, c.next().map_or(Some(0), num)?);
    let off = match offset {
        "" | "Z" | "z" => 0,
        o => {
            let sign = if o.starts_with('-') { -1 } else { 1 };
            let mut p = o[1..].split(':');
            sign * (num(p.next()?)? * 3600 + p.next().map_or(Some(0), num)? * 60)
        }
    };
    Some(days * 86400 + h * 3600 + mi * 60 + sec - off)
}

fn watch(c: &Caller, url: &str, selector: Option<&str>, every: Option<i64>, confirm_cost: bool, notify_when: &J) -> Answer {
    let every = match interval(c, every.unwrap_or(c.every_min)) {
        Ok(e) => e,
        Err(a) => return a,
    };
    let condition = match crate::when::Condition::from_json(notify_when) {
        Ok(k) => k,
        Err(e) => return error(400, &e),
    };
    if condition.as_ref().is_some_and(|k| k.needs_number()) && selector.map_or(true, |s| s.trim().is_empty()) {
        return error(400, "below and above need one place with a number (a selector); a whole page can be watched with contains or not_contains.");
    }
    // A watch beyond the plan costs money every month: said first, and made only when the person
    // agreed to it, so that no program spends for them unasked.
    if c.hosted.is_some() && c.account.paid && !confirm_cost {
        let running = c.store.watches_of(c.account.id).map(|ws| ws.iter().filter(|w| w.state == "running").count() as i64).unwrap_or(0);
        if running >= c.account.quota {
            return error(402, &format!("The subscription holds {} watches and {running} are running, so this one costs 0.50 € a month more (VAT included), billed with the subscription until it is ended. Ask the person whether they want that; if they do, call again with confirm_cost: true. Or end a watch they no longer need first.", c.account.quota));
        }
    }
    let url = match address(url) {
        Ok(u) => u,
        Err(e) => return error(400, &e),
    };
    // A selector that matches more than one place is said so with the answer.
    let mut many: Option<String> = None;
    let anchor = match selector.map(str::trim).filter(|s| !s.is_empty()) {
        Some(css) => match check::seen(&url).and_then(|(html, _)| anchor::at_counted(&html, css)) {
            Ok((a, n)) => {
                if n > 1 {
                    many = Some(anchor::many_said(css, n));
                }
                Some(a)
            }
            Err(e) => return error(422, &format!("The selector marks nothing on the page: {e}. Call preview for the places there are.")),
        },
        None => None,
    };
    let (mode, value, render) = match check::first(&url, anchor) {
        Ok(x) => x,
        Err(e) => return error(422, &e),
    };
    if let Some(Err(e)) = condition.as_ref().map(|k| k.fits(&value)) {
        return error(422, &e);
    }
    let store = c.store;
    let Some(h) = c.hosted else {
        return match store.add_watch(c.account.id, &url, &mode, every, None, &value).and_then(|id| store.set_render(id, render).and_then(|_| store.set_condition(id, condition.as_ref())).map(|_| id)) {
            Ok(id) => created(c, id, with_many("Watching.".into(), &many)),
            Err(e) => error(500, &e),
        };
    };
    let id = match store.add_watch_as(c.account.id, &url, &mode, every, None, &value, "pending").and_then(|id| store.set_render(id, render).and_then(|_| store.set_condition(id, condition.as_ref())).map(|_| id)) {
        Ok(id) => id,
        Err(e) => return error(500, &e),
    };
    match life::start(store, h, c.plan, c.account.id, id) {
        Ok(crate::plan::Start::Running { note, .. }) => created(c, id, with_many(note, &many)),
        Ok(crate::plan::Start::Waiting { note, go }) => created(c, id, with_many(format!("{note} {}{go}", c.base), &many)),
        Err(e) => error(500, &e),
    }
}

/// A note, and what there is to say of a selector that matched more than one place.
fn with_many(note: String, many: &Option<String>) -> String {
    match many {
        Some(m) => format!("{note} {m}"),
        None => note,
    }
}

fn created(c: &Caller, id: i64, note: String) -> Answer {
    let Some(w) = c.store.watch(id).ok().flatten() else { return error(500, "the watch just made is not there") };
    (201, json!({ "watch": watch_json(&w), "note": note }))
}

fn list(c: &Caller) -> Answer {
    match c.store.watches_of(c.account.id) {
        Ok(ws) => (200, json!({ "watches": ws.iter().map(watch_json).collect::<Vec<_>>() })),
        Err(e) => error(500, &e),
    }
}

fn own(c: &Caller, id: &str) -> Option<Watch> {
    let id = id.trim();
    let w = match id.parse::<i64>() {
        Ok(n) => c.store.watch(n).ok().flatten(),
        Err(_) => c.store.watch_by_key(id).ok().flatten(),
    };
    w.filter(|w| w.account == c.account.id)
}

fn get(c: &Caller, id: &str) -> Answer {
    let Some(w) = own(c, id) else { return error(404, "No such watch on this account.") };
    let changes: Vec<J> = c.store.changes(w.id, 20).unwrap_or_default().iter().map(|ch| change_json(ch)).collect();
    let checks: Vec<J> = c.store.checks(w.id, 10).unwrap_or_default().iter().map(|ch| json!({ "at": iso(ch.at), "outcome": ch.outcome, "found_again": ch.moved })).collect();
    (200, json!({ "watch": watch_json_up_to(&w, VALUE_IN_ONE), "changes": changes, "recent_checks": checks }))
}

fn end(c: &Caller, id: &str) -> Answer {
    let Some(w) = own(c, id) else { return error(404, "No such watch on this account.") };
    let back = c.hosted.is_some() && c.plan.ended_early(c.store, &w).unwrap_or(false);
    if let Err(e) = c.store.delete(w.id) {
        return error(500, &e);
    }
    if let Some(h) = c.hosted {
        let _ = h;
        c.plan.sync(c.store, c.account.id);
    }
    let note = if back { "Ended within its first quarter hour, before anything changed: the free trial is back for another page." } else { "Ended; it is deleted with its history." };
    (200, json!({ "ended": w.key, "url": w.url, "note": note }))
}

/// A change as an answer carries it: before and after (each at most `VALUE_IN_LISTS`, and said
/// whether whole), and for a value of many lines the lines that went and came.
fn change_json(ch: &crate::store::Change) -> J {
    let cut = |s: &str| s.chars().take(VALUE_IN_LISTS).collect::<String>();
    let whole = ch.before.chars().count() <= VALUE_IN_LISTS && ch.after.chars().count() <= VALUE_IN_LISTS;
    let mut j = json!({ "at": iso(ch.at), "before": cut(&ch.before), "after": cut(&ch.after), "complete": whole });
    if ch.before.contains('\n') || ch.after.contains('\n') {
        let (gone, came) = crate::text::diff(&ch.before, &ch.after);
        j["gone"] = json!(gone.into_iter().take(100).collect::<Vec<_>>());
        j["came"] = json!(came.into_iter().take(100).collect::<Vec<_>>());
    }
    j
}

/// How much of a watch's value an answer carries: a list of watches stays small, one watch says
/// more; either says how long the value is and whether all of it is there.
const VALUE_IN_LISTS: usize = 2_000;
const VALUE_IN_ONE: usize = 20_000;

fn watch_json(w: &Watch) -> J {
    watch_json_up_to(w, VALUE_IN_LISTS)
}

fn watch_json_up_to(w: &Watch, most: usize) -> J {
    let length = w.last_value.as_deref().map_or(0, |v| v.chars().count());
    let (selector, beside) = match &w.mode {
        Mode::Place(a) => (J::String(a.path.clone()), J::String(a.label.clone())),
        Mode::Page { .. } => (J::Null, J::Null),
    };
    json!({
        "id": w.key,
        "url": w.url,
        "what": life::what(w),
        "selector": selector,
        "beside": beside,
        "state": w.state,
        "last_outcome": w.status,
        "value": w.last_value.as_deref().map(|v| v.chars().take(most).collect::<String>()),
        "value_length": length,
        "value_complete": length <= most,
        "checked_every_minutes": w.interval_min,
        "notify_when": w.condition.as_ref().map(|k| k.json()),
        "condition_met_now": w.condition.as_ref().map(|k| k.met(w.last_value.as_deref().unwrap_or(""))),
        "rendered": w.render,
        "learned_flips": w.variants,
        "free_until": w.until.map(iso),
        "last_checked": w.last_checked.map(iso),
        "created": iso(w.created),
    })
}

/// A moment as RFC 3339 in UTC.
pub fn iso(t: i64) -> String {
    let s = crate::serve::time(t);
    format!("{}T{}:{:02}Z", &s[0..10], &s[11..16], t.rem_euclid(60))
}

// --- MCP ---------------------------------------------------------------------------------

const PROTOCOLS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Zetlyn watches public web pages for a person and tells them by email when something changes. To watch a page: call preview_page with the address, choose the place that matters from the places it lists (a price, an availability, a version, a date) and call watch_page with that place's selector; without a selector the whole page is watched. Before watching, look at what preview_page found: if it says the site does not let programs read the page, or the places are only menus and navigation with nothing of what the person asked about, the page cannot be watched; tell the person so and suggest another site with the same information instead of making a watch. To try places before watching, call read_place with their selectors; it watches nothing and says whether each place is steady or flips on its own between reads. Never make and end watches to try selectors. To change what a watch watches or how often, call update_watch; it keeps the watch and its history. A watch that reads nothing is refused. When the person cares about a threshold or a word (a price under a mark, back in stock, a sold-out notice gone), set notify_when instead of reporting every change; condition_met_now on the watch says whether it holds already, so tell them if it does. Changes go by email to the account's address; list_watches and get_watch show what was read and what changed, list_changes the changes across all watches since a moment. The account's first watch is free for three days; ending it within a quarter hour, before anything changed, gives the free trial back. When a watch cannot start yet (after the free trial, without a subscription), call subscribe_link and give the person the link and the price; they pay themselves at Stripe, never you. A watch beyond the subscription's 10 costs 0.50 € a month more: watch_page refuses it with the price until the person has agreed and you pass confirm_cost: true.";

/// The tools, each with a title and with hints a client weighs before calling it: whether it only
/// reads, whether it can undo something (`end_watch` deletes), whether calling it twice is the
/// same as once, and that it reaches pages outside Zetlyn.
pub fn tools() -> J {
    let hints = |name: &str| -> (&str, bool, bool, bool, bool) {
        // (title, read only, destructive, idempotent, open world)
        match name {
            "preview_page" => ("Preview a page", true, false, true, true),
            "read_place" => ("Read places on a page", true, false, true, true),
            "watch_page" => ("Watch a page", false, false, false, true),
            "update_watch" => ("Change a watch", false, false, true, true),
            "list_changes" => ("List recent changes", true, false, true, false),
            "subscribe_link" => ("Get a link to subscribe", true, false, false, false),
            "list_watches" => ("List watches", true, false, true, false),
            "get_watch" => ("Get a watch", true, false, true, false),
            "end_watch" => ("End a watch", false, true, true, false),
            _ => ("Get the account", true, false, true, false),
        }
    };
    let mut all = tool_list();
    for t in all.as_array_mut().into_iter().flatten() {
        let (title, read_only, destructive, idempotent, open) = hints(t["name"].as_str().unwrap_or(""));
        t["title"] = J::from(title);
        t["annotations"] = json!({ "title": title, "readOnlyHint": read_only, "destructiveHint": destructive, "idempotentHint": idempotent, "openWorldHint": open });
    }
    all
}

fn tool_list() -> J {
    json!([
        {
            "name": "preview_page",
            "description": "Read a public web page (or JSON, an RSS or Atom feed, XML or plain text, each read as a page of its own: one line per value, the field's path beside it) and list the places on it that can be watched, each with the selector that marks it, what it says now, and the text beside it: the page's own content first, its menus, header and footer last. Call this before watch_page to choose a place. If the place the person means is not among them, call it again with find (a word or sign the place says, such as \"€\", \"In stock\" or \"Version\"), which searches the whole page. If it fails saying the site does not let programs read the page, that page cannot be watched: tell the person, and do not call watch_page for it.",
            "inputSchema": { "type": "object", "properties": { "url": { "type": "string", "description": "The page's address, e.g. https://example.com/pricing" }, "find": { "type": "string", "description": "Only the places whose text, or the text beside them, says this (case aside), from the whole page." } }, "required": ["url"] }
        },
        {
            "name": "watch_page",
            "description": "Start watching a page: one place on it (give the selector from preview_page) or the whole page (no selector). A JSON field is a place like any other; a feed watched whole is told by its new entries. Zetlyn checks it every hour (with a subscription as often as every minute, see every_minutes) and notifies the account when it changes.",
            "inputSchema": { "type": "object", "properties": {
                "url": { "type": "string", "description": "The page's address." },
                "selector": { "type": "string", "description": "The selector of the place to watch, from preview_page. Leave out to watch the whole page." },
                "every_minutes": { "type": "integer", "enum": [60, 15, 5, 1], "description": "How often to check, in minutes. 60 if left out; 15, 5 and 1 need a subscription." },
                "confirm_cost": { "type": "boolean", "description": "Only when the person agreed: a watch beyond the 10 in the subscription costs 0.50 € a month more. Without it such a watch is refused with the price, to ask them first." },
                "notify_when": { "type": "object", "description": "Tell the person only when the value comes to meet this, not on every change: {\"below\": 400} or {\"above\": 1000} (the amount of money in the place, a number beside €, $, EUR and the like, else its first number; needs a selector; a place showing two different amounts is refused, mark a narrower one), {\"contains\": \"In stock\"} or {\"not_contains\": \"Sold out\"} (case does not matter). Told once when it comes true, again only after it stopped being true. Leave out to be told of every change." }
            }, "required": ["url"] }
        },
        {
            "name": "read_place",
            "description": "Read what one or more places on a page say now, without watching anything. Use it to try selectors before watch_page or update_watch instead of making and ending watches. Each place is read twice a few seconds apart; steady: false means the site shows something else on its own from one read to the next (an A/B test, a box that guesses where the reader is), so prefer a narrower place inside it.",
            "inputSchema": { "type": "object", "properties": {
                "url": { "type": "string", "description": "The page's address." },
                "selectors": { "type": "array", "items": { "type": "string" }, "maxItems": 10, "description": "Selectors from preview_page." }
            }, "required": ["url", "selectors"] }
        },
        {
            "name": "update_watch",
            "description": "Change a watch in place: watch another place on the same page (selector), the whole page (whole_page: true), or check it at another interval (every_minutes). The watch keeps its id and history and does not start a new free trial. To watch a different page, make a new watch.",
            "inputSchema": { "type": "object", "properties": {
                "id": { "type": "string", "description": "The watch's id, as list_watches gives it." },
                "selector": { "type": "string", "description": "The new place's selector, from preview_page or read_place." },
                "whole_page": { "type": "boolean", "description": "Watch the whole page instead of one place." },
                "every_minutes": { "type": "integer", "enum": [60, 15, 5, 1], "description": "How often to check; 15, 5 and 1 need a subscription." },
                "notify_when": { "type": ["object", "null"], "description": "As in watch_page: {\"below\": n}, {\"above\": n}, {\"contains\": \"text\"}, {\"not_contains\": \"text\"}; null to be told of every change again." }
            }, "required": ["id"] }
        },
        {
            "name": "list_changes",
            "description": "The changes on all of the account's watches since a moment, newest first: which watch, when, before and after. Use it to answer what changed lately.",
            "inputSchema": { "type": "object", "properties": {
                "since": { "type": "string", "description": "A moment in RFC 3339, e.g. 2026-10-20T08:00:00Z. The last 7 days if left out." }
            } }
        },
        {
            "name": "subscribe_link",
            "description": "A link for the person to subscribe to Zetlyn by (9.99 € a month for up to 10 watches, 0.50 € for each one more). Call it when a watch is waiting for a subscription or the person wants one. Give the person the link and the price; they open it themselves and pay at Stripe. You cannot pay or agree for them. Afterwards get_account says subscribed: true and waiting watches run.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "list_watches",
            "description": "List the account's watches: what each watches, what it read last, its state and when it was checked.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "get_watch",
            "description": "One watch with its last 20 changes (before and after) and its recent checks.",
            "inputSchema": { "type": "object", "properties": { "id": { "type": "string", "description": "The watch's id, as list_watches gives it." } }, "required": ["id"] }
        },
        {
            "name": "end_watch",
            "description": "End a watch. It is deleted with its history.",
            "inputSchema": { "type": "object", "properties": { "id": { "type": "string", "description": "The watch's id, as list_watches gives it." } }, "required": ["id"] }
        },
        {
            "name": "get_account",
            "description": "The account: its address, how many watches it has and runs, and on zetlyn.com its subscription and how many watches the plan holds.",
            "inputSchema": { "type": "object", "properties": {} }
        }
    ])
}

/// Whether an MCP message is a tool call that reads a page, and so counts against the account's
/// limit of pages read, as the same calls through the API do.
pub fn reads_a_page(msg: &J) -> bool {
    if msg["method"] != "tools/call" {
        return false;
    }
    let args = &msg["params"]["arguments"];
    match msg["params"]["name"].as_str().unwrap_or("") {
        "preview_page" | "read_place" | "watch_page" => true,
        "update_watch" => args.get("selector").is_some_and(|s| !s.is_null()) || args["whole_page"].as_bool() == Some(true),
        _ => false,
    }
}

/// The answer to a tool call refused because the account read too many pages lately.
pub fn too_many_reads(msg: &J) -> Option<J> {
    let id = msg.get("id").cloned()?;
    let text = "Too many pages read for this account in the last ten minutes (60 at most); try again in a few minutes. Reading the account's watches and changes still works.";
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": { "content": [{ "type": "text", "text": text }], "structuredContent": { "error": text }, "isError": true } }))
}

/// One JSON-RPC message to the MCP server. None for a notification, which gets no answer.
pub fn mcp(c: &Caller, msg: &J) -> Option<J> {
    let id = msg.get("id").cloned()?;
    let method = msg["method"].as_str().unwrap_or("");
    let ok = |result: J| Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    match method {
        "initialize" => {
            let asked = msg["params"]["protocolVersion"].as_str().unwrap_or("");
            let version = if PROTOCOLS.contains(&asked) { asked } else { PROTOCOLS[0] };
            ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "zetlyn", "title": "Zetlyn", "version": env!("CARGO_PKG_VERSION"), "websiteUrl": c.base, "icons": [{ "src": format!("{}/mark.png", c.base), "mimeType": "image/png", "sizes": ["512x512"] }] },
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => ok(json!({})),
        "tools/list" => ok(json!({ "tools": tools() })),
        "tools/call" => {
            let args = &msg["params"]["arguments"];
            let id_of = |a: &J| a["id"].as_i64().map(|n| n.to_string()).or_else(|| a["id"].as_str().map(str::to_string)).unwrap_or_default();
            let (status, body) = match msg["params"]["name"].as_str().unwrap_or("") {
                "preview_page" => preview(c, args["url"].as_str().unwrap_or(""), args["find"].as_str()),
                "watch_page" => watch(c, args["url"].as_str().unwrap_or(""), args["selector"].as_str(), args["every_minutes"].as_i64(), args["confirm_cost"].as_bool().unwrap_or(false), args.get("notify_when").unwrap_or(&J::Null)),
                "list_watches" => list(c),
                "get_watch" => get(c, &id_of(args)),
                "end_watch" => end(c, &id_of(args)),
                "get_account" => account(c),
                "read_place" => read_place(c, args["url"].as_str().unwrap_or(""), &selectors_of(args)),
                "update_watch" => update(c, &id_of(args), args["selector"].as_str(), args["whole_page"].as_bool().unwrap_or(false), args["every_minutes"].as_i64(), args.get("notify_when")),
                "list_changes" => changes(c, args["since"].as_str()),
                "subscribe_link" => subscribe_link(c),
                other => error(404, &format!("No tool called {other}.")),
            };
            let text = serde_json::to_string_pretty(&body).unwrap_or_default();
            ok(json!({ "content": [{ "type": "text", "text": text }], "structuredContent": body, "isError": status >= 400 }))
        }
        _ => Some(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("No method {method}.") } })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller(store: &Store) -> Caller<'_> {
        let id = store.account("a@example.com").unwrap();
        Caller { store, account: store.account_by_id(id).unwrap().unwrap(), hosted: None, plan: &crate::plan::Plan { stripe: None }, every_min: 60, base: "https://zetlyn.test".into() }
    }

    #[test]
    fn mcp_says_what_it_is_and_lists_its_tools() {
        let store = Store::in_memory();
        let c = caller(&store);
        let init = mcp(&c, &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26" } })).unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(init["result"]["serverInfo"]["name"], "zetlyn");
        assert!(mcp(&c, &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).is_none(), "a notification gets no answer");
        let list = mcp(&c, &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })).unwrap();
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["preview_page", "watch_page", "read_place", "update_watch", "list_changes", "subscribe_link", "list_watches", "get_watch", "end_watch", "get_account"]);
        let unknown = mcp(&c, &json!({ "jsonrpc": "2.0", "id": 3, "method": "resources/list" })).unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
    }

    #[test]
    fn a_tool_acts_on_its_own_account_only() {
        let store = Store::in_memory();
        let c = caller(&store);
        let other = store.account("b@example.com").unwrap();
        let theirs = store.add_watch(other, "https://example.com/", &Mode::Page { volatile: vec![] }, 60, None, "x").unwrap();
        let mine = store.add_watch(c.account.id, "https://example.org/", &Mode::Page { volatile: vec![] }, 60, None, "y").unwrap();
        let listed = mcp(&c, &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "list_watches", "arguments": {} } })).unwrap();
        let watches = listed["result"]["structuredContent"]["watches"].as_array().unwrap().clone();
        assert_eq!(watches.len(), 1);
        assert_eq!(watches[0]["id"], store.watch(mine).unwrap().unwrap().key, "a watch is named by its key");
        let (status, _) = rest(&c, "DELETE", &["watches", &theirs.to_string()], &J::Null);
        assert_eq!(status, 404, "not theirs to end");
        let their_key = store.watch(theirs).unwrap().unwrap().key;
        assert_eq!(rest(&c, "GET", &["watches", &their_key], &J::Null).0, 404, "nor theirs to see by key");
        let got = mcp(&c, &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "get_watch", "arguments": { "id": theirs } } })).unwrap();
        assert_eq!(got["result"]["isError"], true);
        assert_eq!(rest(&c, "GET", &["watches", &mine.to_string()], &J::Null).0, 200);
        assert_eq!(iso(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn a_private_address_is_refused_before_anything_is_read() {
        let store = Store::in_memory();
        let c = caller(&store);
        assert_eq!(rest(&c, "POST", &["preview"], &json!({ "url": "http://127.0.0.1:4848/" })).0, 400);
        assert_eq!(rest(&c, "POST", &["watches"], &json!({ "url": "" })).0, 400);
    }

    #[test]
    fn checking_more_often_than_hourly_needs_a_subscription_where_people_sign_in() {
        let store = Store::in_memory();
        let mut c = caller(&store);
        let h = crate::life::Hosted { base: "https://zetlyn.test".into(), mailer: crate::mail::Mailer { from: "Zetlyn <hello@zetlyn.test>".into(), smtp: None, outbox: std::env::temp_dir().join("zetlyn-api-outbox") } };
        c.hosted = Some(&h);
        let every = |m: i64| json!({ "url": "https://example.com/", "every_minutes": m });
        assert_eq!(rest(&c, "POST", &["watches"], &every(7)).0, 400, "not one of the intervals");
        assert_eq!(rest(&c, "POST", &["watches"], &every(5)).0, 403, "a free account checks hourly");
        assert!(crate::plan::interval_allowed(&c.account, true, 60));
        c.account.paid = true;
        assert!(crate::plan::interval_allowed(&c.account, true, 1));
        assert!(!crate::plan::interval_allowed(&c.account, true, 2));
        c.account.paid = false;
        assert!(crate::plan::interval_allowed(&c.account, false, 1), "a local instance has no subscription");
    }

    #[test]
    fn a_moment_is_read_as_rfc_3339() {
        assert_eq!(moment("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(moment("2026-10-18T08:00:00Z"), Some(1_792_310_400));
        assert_eq!(moment("2026-10-18T10:00:00+02:00"), Some(1_792_310_400));
        assert_eq!(moment("2026-10-18T08:00:00.250Z"), Some(1_792_310_400));
        assert_eq!(moment("2026-10-18"), Some(1_792_281_600));
        assert_eq!(moment("yesterday"), None);
        assert_eq!(moment("2026-13-01"), None);
    }

    #[test]
    fn a_long_value_is_cut_and_said_to_be() {
        let store = Store::in_memory();
        let c = caller(&store);
        let long = "a line of the page\n".repeat(500);
        let w = store.add_watch(c.account.id, "https://example.org/", &Mode::Page { volatile: vec![] }, 60, None, &long).unwrap();
        store.record(w, crate::store::now(), "changed", false, "x", Some((&long, &format!("{long}a new line")))).unwrap();
        let listed = &rest(&c, "GET", &["watches"], &J::Null).1["watches"][0];
        assert_eq!((listed["value"].as_str().unwrap().chars().count(), listed["value_complete"].as_bool()), (2000, Some(false)));
        let one = rest(&c, "GET", &["watches", &w.to_string()], &J::Null).1;
        assert_eq!(one["watch"]["value_complete"], true, "one watch says up to 20,000 characters");
        assert_eq!(one["changes"][0]["came"], json!(["a new line"]));
        assert_eq!(one["changes"][0]["complete"], false);
    }

    #[test]
    fn changes_are_listed_across_the_accounts_watches_only() {
        let store = Store::in_memory();
        let c = caller(&store);
        let other = store.account("b@example.com").unwrap();
        let mine = store.add_watch(c.account.id, "https://example.org/", &Mode::Page { volatile: vec![] }, 60, None, "a").unwrap();
        let theirs = store.add_watch(other, "https://example.com/", &Mode::Page { volatile: vec![] }, 60, None, "x").unwrap();
        store.record(mine, 1_792_310_400, "changed", false, "b", Some(("a", "b"))).unwrap();
        store.record(theirs, 1_792_310_400, "changed", false, "y", Some(("x", "y"))).unwrap();
        let (status, body) = rest(&c, "GET", &["changes"], &json!({ "since": "2026-10-18T00:00:00Z" }));
        assert_eq!(status, 200);
        let all = body["changes"].as_array().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0]["watch"], store.watch(mine).unwrap().unwrap().key);
        assert_eq!(all[0]["after"], "b");
        assert_eq!(rest(&c, "GET", &["changes"], &json!({ "since": "2026-10-19T00:00:00Z" })).1["changes"].as_array().unwrap().len(), 0);
        assert_eq!(rest(&c, "GET", &["changes"], &json!({ "since": "soon" })).0, 400);
    }

    #[test]
    fn a_watch_is_changed_only_by_its_account_and_only_as_asked() {
        let store = Store::in_memory();
        let c = caller(&store);
        let other = store.account("b@example.com").unwrap();
        let theirs = store.add_watch(other, "https://example.com/", &Mode::Page { volatile: vec![] }, 60, None, "x").unwrap();
        let mine = store.add_watch(c.account.id, "https://example.org/", &Mode::Page { volatile: vec![] }, 60, None, "y").unwrap();
        assert_eq!(rest(&c, "PATCH", &["watches", &theirs.to_string()], &json!({ "every_minutes": 60 })).0, 404);
        assert_eq!(rest(&c, "PATCH", &["watches", &mine.to_string()], &json!({})).0, 400, "nothing asked");
        assert_eq!(rest(&c, "PATCH", &["watches", &mine.to_string()], &json!({ "selector": "h1", "whole_page": true })).0, 400);
        assert_eq!(rest(&c, "PATCH", &["watches", &mine.to_string()], &json!({ "every_minutes": 7 })).0, 400);
        let (status, body) = rest(&c, "PATCH", &["watches", &mine.to_string()], &json!({ "every_minutes": 5 }));
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["watch"]["checked_every_minutes"], 5);
        assert_eq!(rest(&c, "POST", &["read"], &json!({ "url": "https://example.com/" })).0, 400, "no selectors");
        assert_eq!(rest(&c, "POST", &["read"], &json!({ "url": "http://127.0.0.1/", "selectors": ["h1"] })).0, 400);
    }

    #[test]
    fn a_watch_beyond_the_plan_is_made_only_when_its_cost_was_agreed() {
        let store = Store::in_memory();
        let mut c = caller(&store);
        let h = crate::life::Hosted { base: "https://zetlyn.test".into(), mailer: crate::mail::Mailer { from: "Zetlyn <hello@zetlyn.test>".into(), smtp: None, outbox: std::env::temp_dir().join("zetlyn-api-outbox") } };
        c.hosted = Some(&h);
        c.account.paid = true;
        c.account.quota = 1;
        let w = store.add_watch(c.account.id, "https://example.org/", &Mode::Page { volatile: vec![] }, 60, None, "y").unwrap();
        store.set_state(w, "running", None).unwrap();
        let (status, body) = rest(&c, "POST", &["watches"], &json!({ "url": "http://127.0.0.1/" }));
        assert_eq!(status, 402, "{body}");
        assert!(body["error"].as_str().unwrap().contains("0.50 €"));
        assert_eq!(rest(&c, "POST", &["watches"], &json!({ "url": "http://127.0.0.1/", "confirm_cost": true })).0, 400, "agreed: on to reading the page, which is refused here as private");
        assert_eq!(rest(&c, "POST", &["subscribe-link"], &J::Null).0, 404, "no payment on this instance");
    }

    #[test]
    fn a_condition_is_set_shown_and_kept_to_what_the_watch_can_read() {
        let store = Store::in_memory();
        let c = caller(&store);
        let page = store.add_watch(c.account.id, "https://example.org/", &Mode::Page { volatile: vec![] }, 60, None, "Sold out\nShoes").unwrap();
        let key = store.watch(page).unwrap().unwrap().key;
        assert_eq!(rest(&c, "POST", &["watches"], &json!({ "url": "https://example.com/", "notify_when": { "below": 400 } })).0, 400, "a number needs a place");
        assert_eq!(rest(&c, "POST", &["watches"], &json!({ "url": "https://example.com/", "notify_when": { "cheaper": 1 } })).0, 400);
        assert_eq!(rest(&c, "PATCH", &["watches", &key], &json!({ "notify_when": { "above": 3 } })).0, 400, "nor on a whole page by update");
        let (status, body) = rest(&c, "PATCH", &["watches", &key], &json!({ "notify_when": { "not_contains": "sold out" } }));
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["watch"]["notify_when"], json!({ "not_contains": "sold out" }));
        assert_eq!(body["watch"]["condition_met_now"], false);
        let (_, body) = rest(&c, "PATCH", &["watches", &key], &json!({ "notify_when": null }));
        assert_eq!(body["watch"]["notify_when"], J::Null);
        assert_eq!(body["note"], "Every change is told.");
    }

    #[test]
    fn the_tools_that_read_a_page_are_the_ones_counted() {
        let call = |name: &str, args: J| json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": args } });
        assert!(reads_a_page(&call("preview_page", json!({ "url": "x" }))));
        assert!(reads_a_page(&call("read_place", json!({}))));
        assert!(reads_a_page(&call("watch_page", json!({}))));
        assert!(reads_a_page(&call("update_watch", json!({ "id": "a", "selector": "h1" }))));
        assert!(reads_a_page(&call("update_watch", json!({ "id": "a", "whole_page": true }))));
        assert!(!reads_a_page(&call("update_watch", json!({ "id": "a", "every_minutes": 15 }))), "an interval reads nothing");
        assert!(!reads_a_page(&call("list_watches", json!({}))));
        assert!(!reads_a_page(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })));
        let refused = too_many_reads(&call("preview_page", json!({}))).unwrap();
        assert_eq!((refused["id"].as_i64(), refused["result"]["isError"].as_bool()), (Some(1), Some(true)));
        assert!(too_many_reads(&json!({ "method": "tools/call" })).is_none(), "a notification gets no answer");
    }
}
