//! The admin pages, `/admin`: the whole instance seen and managed, by the accounts `ZETLYN_ADMINS`
//! names (on a local instance by whoever uses it). Every change made here goes into the admin log.

use std::collections::HashMap;

use super::*;
use crate::billing::{BASE_CENTS, EXTRA_CENTS};
use crate::plan::INTERVALS;

pub fn route(app: &App, store: &Store, a: &Asked, rest: &[&str]) -> Result<Reply, String> {
    let Some(admin) = admin_of(app, a) else {
        return Ok(if a.account.is_none() && app.hosted.is_some() { redirect("/signin") } else { not_found(app, a) });
    };
    let page = |title: &str, body: String| page_reply(app, a, title, &framed(title, a, &body), 200);
    Ok(match (&a.method, rest) {
        (Method::Get, [] | [""]) => page("Admin", overview(app, store)?),
        (Method::Get, ["accounts"]) => page("Accounts", accounts(store, &a.q("q").unwrap_or_default())?),
        (Method::Get, ["a", id]) => match account_of(store, id)? {
            Some(acc) => page("Account", account(app, store, &acc)?),
            None => not_found(app, a),
        },
        // Shown once, here, not carried in an address: the link signs in to the account.
        (Method::Post, ["a", id, "review"]) if app.hosted.is_some() => match account_of(store, id)? {
            Some(acc) => {
                let days = a.f("days").trim().parse::<i64>().unwrap_or(30).clamp(1, 90);
                let link = review_link(store, &public_base(app, a), acc.id, days)?;
                store.log_admin(&admin.email, "review", &format!("account {} {} for {days} days", acc.id, acc.email))?;
                page("Reviewer link", format!("<h1>Reviewer link</h1><p>For <b>{}</b>, for {days} days. Opening it signs in to this account; it can be opened again until then. It is shown only now.</p><p><code class=tg-command>{}</code></p><p class=dim>For the reviewers: open this link in the browser first, then add the connector https://zetlyn.com/mcp and allow it. End it on the <a href=\"/admin/a/{}\">account's page</a>.</p>", esc(&acc.email), esc(&link), acc.id))
            }
            None => not_found(app, a),
        },
        (Method::Post, ["accounts", "new"]) => {
            let email = a.f("email").trim().to_lowercase();
            if !mail::plausible(&email) {
                return Ok(redirect(&format!("/admin/accounts?note={}", enc("Not an address."))));
            }
            let id = store.account(&email)?;
            store.confirm_account(id)?;
            store.log_admin(&admin.email, "new account", &email)?;
            redirect(&format!("/admin/a/{id}?note={}", enc("Made, confirmed. Make it free and give it a reviewer link below.")))
        }
        (Method::Post, ["a", id, action]) => match account_of(store, id)? {
            Some(acc) => {
                let (to, note, done) = act_on_account(app, store, &acc, action, a)?;
                if done {
                    store.log_admin(&admin.email, action, &format!("account {} {}", acc.id, acc.email))?;
                }
                redirect(&format!("{to}?note={}", enc(&note)))
            }
            None => not_found(app, a),
        },
        (Method::Get, ["watches"]) => page("Watches", watches(store, a)?),
        (Method::Get, ["w", id]) => match watch_of(store, id)? {
            Some(w) => page("Watch", watch(store, &w)?),
            None => not_found(app, a),
        },
        (Method::Post, ["w", id, action]) => match watch_of(store, id)? {
            Some(w) => {
                let (to, note, done) = act_on_watch(app, store, &w, action, a)?;
                if done {
                    store.log_admin(&admin.email, action, &format!("watch {} {}", w.id, w.url))?;
                }
                redirect(&format!("{to}?note={}", enc(&note)))
            }
            None => not_found(app, a),
        },
        (Method::Get, ["funnel"]) => page("Funnel", funnel(store, a.q("weeks").and_then(|w| w.parse().ok()).unwrap_or(12))?),
        (Method::Get, ["quality"]) => page("Read quality", quality(store, a.q("days").and_then(|d| d.parse().ok()).unwrap_or(7))?),
        (Method::Get, ["mail"]) => page("Mail", mails(app, &a.q("q").unwrap_or_default())),
        (Method::Get, ["billing"]) => page("Billing", billing(app, store, a.q("compare").is_some())?),
        (Method::Get, ["system"]) => page("System", system(app, store)?),
        (Method::Post, ["banner"]) => {
            let text: String = a.f("banner").trim().chars().take(300).collect();
            store.set_meta("banner", &text)?;
            *app.banner.lock().unwrap_or_else(|e| e.into_inner()) = text.clone();
            store.log_admin(&admin.email, "banner", &text)?;
            redirect(&format!("/admin/system?note={}", enc(if text.is_empty() { "The notice is gone." } else { "The notice is shown on every page." })))
        }
        (Method::Get, ["log"]) => page("Admin log", log(store)?),
        _ => not_found(app, a),
    })
}

const TABS: &[(&str, &str)] = &[
    ("Admin", "/admin"),
    ("Accounts", "/admin/accounts"),
    ("Watches", "/admin/watches"),
    ("Funnel", "/admin/funnel"),
    ("Read quality", "/admin/quality"),
    ("Mail", "/admin/mail"),
    ("Billing", "/admin/billing"),
    ("System", "/admin/system"),
    ("Admin log", "/admin/log"),
];

/// The admin's tabs over a page, and the note an action left.
fn framed(title: &str, a: &Asked, body: &str) -> String {
    let tabs: String = TABS.iter().map(|(name, href)| format!("<a href=\"{href}\"{}>{name}</a>", if *name == title || (title == "Account" && *name == "Accounts") || (title == "Watch" && *name == "Watches") { " class=on" } else { "" })).collect();
    let note = a.q("note").filter(|n| !n.is_empty()).map(|n| format!("<p class=ok>{}</p>", esc(&n))).unwrap_or_default();
    format!("<section class=admin><nav class=tabs>{tabs}</nav>{note}{body}</section>")
}

/// `/admin/funnel`: where accounts come from and where they stop, week by week of their making.
fn funnel(store: &Store, weeks: i64) -> Result<String, String> {
    const STEPS: &[(&str, &str)] = &[("made", "Made"), ("confirmed", "Confirmed"), ("first_watch", "First watch"), ("trial_ended", "Trial ended"), ("subscribed", "Subscribed"), ("cancelled", "Cancelled")];
    let weeks = weeks.clamp(1, 104);
    let since = now() - weeks * 7 * 86400;
    let mut accounts: HashMap<i64, HashMap<String, (i64, String)>> = HashMap::new();
    for (account, step, at, via) in store.steps_since(since)? {
        accounts.entry(account).or_default().insert(step, (at, via));
    }
    // Weeks from Monday: the epoch began on a Thursday.
    let week_of = |at: i64| (at + 3 * 86400).div_euclid(7 * 86400);
    let mut cohorts: std::collections::BTreeMap<i64, [i64; 6]> = std::collections::BTreeMap::new();
    let mut total = [0i64; 6];
    for steps in accounts.values() {
        let Some((made, _)) = steps.get("made") else { continue };
        let row = cohorts.entry(week_of(*made)).or_default();
        for (i, (step, _)) in STEPS.iter().enumerate() {
            if steps.contains_key(*step) {
                row[i] += 1;
                total[i] += 1;
            }
        }
    }
    let cells = |r: &[i64; 6]| -> String {
        r.iter().enumerate().map(|(i, n)| if i == 0 || r[0] == 0 { format!("<td>{n}</td>") } else { format!("<td>{n} <span class=dim>{:.0} %</span></td>", *n as f64 * 100.0 / r[0] as f64) }).collect()
    };
    let rows: String = cohorts.iter().rev().map(|(w, r)| format!("<tr><td class=dim>{}</td>{}</tr>", &crate::serve::time(w * 7 * 86400 - 3 * 86400)[..10], cells(r))).collect();
    let head: String = STEPS.iter().map(|(_, h)| format!("<td>{h}</td>")).collect();
    // How the first watch was made, and how many of those went on to subscribe.
    let mut ways: HashMap<String, (i64, i64)> = HashMap::new();
    let mut signins: HashMap<String, i64> = HashMap::new();
    for steps in accounts.values() {
        if let Some((_, via)) = steps.get("first_watch") {
            let e = ways.entry(if via.is_empty() { "unknown".into() } else { via.clone() }).or_default();
            e.0 += 1;
            if steps.contains_key("subscribed") {
                e.1 += 1;
            }
        }
        if let Some((_, via)) = steps.get("confirmed") {
            *signins.entry(if via.is_empty() { "unknown".into() } else { via.clone() }).or_default() += 1;
        }
    }
    let mut ways: Vec<_> = ways.into_iter().collect();
    ways.sort_by(|x, y| y.1 .0.cmp(&x.1 .0));
    let way_rows: String = ways.iter().map(|(w, (n, s))| format!("<tr><td>{}</td><td>{n}</td><td>{s} <span class=dim>{:.0} %</span></td></tr>", esc(w), *s as f64 * 100.0 / (*n).max(1) as f64)).collect();
    let mut signins: Vec<_> = signins.into_iter().collect();
    signins.sort_by(|x, y| y.1.cmp(&x.1));
    let signin_rows: String = signins.iter().map(|(w, n)| format!("<tr><td>{}</td><td>{n}</td></tr>", esc(w))).collect();
    let span: String = [4, 12, 52].iter().map(|w| format!("<a href=\"/admin/funnel?weeks={w}\"{}>{w} weeks</a>", if *w == weeks { " class=on" } else { "" })).collect::<Vec<_>>().join(" · ");
    Ok(format!(
        "<h1>Funnel</h1><p class=dim>Accounts made over the last {span}, by the week they were made, and how far each got; the shares are of those made. Accounts made before steps were kept show \"before\" as their way. Admin-made free accounts count as made, not as subscribed.</p>\
         <div class=scroll><table><tr><td>Week</td>{head}</tr><tr><td><b>All</b></td>{}</tr>{rows}</table></div>\
         <h2>How the first watch was made</h2><p class=dim>On the site (web), with an API key (api), or by the app that connected by OAuth, by its name; and how many of them subscribed.</p>{}\
         <h2>How accounts signed in first</h2>{}",
        cells(&total),
        if way_rows.is_empty() { "<p class=dim>None yet.</p>".to_string() } else { format!("<table><tr><td>Way</td><td>First watches</td><td>Subscribed</td></tr>{way_rows}</table>") },
        if signin_rows.is_empty() { "<p class=dim>None yet.</p>".to_string() } else { format!("<table>{signin_rows}</table>") },
    ))
}

fn account_of(store: &Store, id: &str) -> Result<Option<Account>, String> {
    match id.parse() {
        Ok(id) => store.account_by_id(id),
        Err(_) => Ok(None),
    }
}

fn watch_of(store: &Store, id: &str) -> Result<Option<Watch>, String> {
    match id.parse() {
        Ok(id) => store.watch(id),
        Err(_) => Ok(None),
    }
}

fn euros(cents: i64) -> String {
    format!("{}.{:02} €", cents / 100, cents % 100)
}

/// What an account pays a month, VAT included: nothing unless it has a subscription of its own.
fn monthly(acc: &Account, running: i64) -> i64 {
    if acc.paid && !acc.free && !acc.stripe_subscription.is_empty() { BASE_CENTS + EXTRA_CENTS * (running - acc.quota).max(0) } else { 0 }
}

fn stat(label: &str, value: impl std::fmt::Display, note: &str) -> String {
    format!("<div><b>{value}</b><span>{label}</span>{}</div>", if note.is_empty() { String::new() } else { format!("<small>{}</small>", esc(note)) })
}

fn overview(app: &App, store: &Store) -> Result<String, String> {
    let at = now();
    let accounts = store.find_accounts("", i64::MAX)?;
    let people: Vec<&Account> = accounts.iter().filter(|a| a.email != LOCAL).collect();
    let counts = store.watch_counts()?;
    let running_of = |id: i64| counts.get(&id).map_or(0, |c| c.0);
    let paying = people.iter().filter(|a| a.paid && !a.free && !a.stripe_subscription.is_empty()).count();
    let revenue: i64 = people.iter().map(|a| monthly(a, running_of(a.id))).sum();
    let states = store.counts("SELECT state, COUNT(*) FROM watches GROUP BY state", [])?;
    let state = |s: &str| states.iter().find(|(k, _)| k == s).map_or(0, |(_, n)| *n);
    let outcomes = store.counts("SELECT outcome, COUNT(*) FROM checks WHERE at >= ?1 AND outcome != 'new' GROUP BY outcome", [at - 86400])?;
    let checks: i64 = outcomes.iter().map(|(_, n)| n).sum();
    let outcome = |s: &str| outcomes.iter().find(|(k, _)| k == s).map_or(0, |(_, n)| *n);
    let last_check = store.count("SELECT COALESCE(MAX(at), 0) FROM checks", [])?;
    let overdue = store.overdue(at, 600)?.len();
    let mails = mail_lines(app).into_iter().filter(|m| m.0 >= at - 86400).collect::<Vec<_>>();
    let failed_mails = mails.iter().filter(|m| m.1 != "sent").count();
    let percent = |n: i64| if checks == 0 { "–".to_string() } else { format!("{:.1} %", n as f64 * 100.0 / checks as f64) };
    let health = if overdue > 0 {
        format!("<p class=error>{overdue} running watches are more than ten minutes overdue. The checks may have stopped.</p>")
    } else {
        String::new()
    };
    Ok(format!(
        "<h1>Overview</h1>{health}<h2>Accounts</h2><div class=stats>{}{}{}{}{}{}</div>\
         <h2>Watches</h2><div class=stats>{}{}{}{}{}</div>\
         <h2>Checks, last 24 hours</h2><div class=stats>{}{}{}{}{}{}</div>\
         <h2>Running</h2><div class=stats>{}{}{}{}</div>",
        stat("accounts", people.len(), ""),
        stat("confirmed", people.iter().filter(|a| a.confirmed).count(), ""),
        stat("subscribed", paying, ""),
        stat("free (by admin)", people.iter().filter(|a| a.free).count(), ""),
        stat("new in 7 days", people.iter().filter(|a| a.created >= at - 7 * 86400).count(), ""),
        stat("a month, VAT incl.", euros(revenue), "subscriptions and extra watches"),
        stat("running", state("running"), ""),
        stat("waiting to subscribe", state("waiting"), ""),
        stat("waiting for confirmation", state("pending"), ""),
        stat("ended", state("ended"), ""),
        stat("changes in 24 hours", outcome("changed"), ""),
        stat("checks", checks, ""),
        stat("same", outcome("same"), &percent(outcome("same"))),
        stat("changed", outcome("changed"), &percent(outcome("changed"))),
        stat("unclear", outcome("unclear"), &percent(outcome("unclear"))),
        stat("gone", outcome("gone"), &percent(outcome("gone"))),
        stat("failed", outcome("failed"), &percent(outcome("failed"))),
        stat("last check", if last_check == 0 { "never".to_string() } else { ago(last_check) }, ""),
        stat("overdue watches", overdue, ""),
        stat("mails in 24 hours", mails.len(), ""),
        stat("mails failed", failed_mails, ""),
    ))
}

fn accounts(store: &Store, q: &str) -> Result<String, String> {
    let counts = store.watch_counts()?;
    let list = store.find_accounts(q, 500)?;
    let mut rows = String::new();
    for acc in &list {
        let (running, all) = counts.get(&acc.id).copied().unwrap_or((0, 0));
        rows.push_str(&format!(
            "<tr><td><a href=\"/admin/a/{}\">{}</a></td><td>{}</td><td>{running} of {all}</td><td class=dim>{}</td></tr>",
            acc.id,
            esc(&acc.email),
            account_state(acc),
            time(acc.created)
        ));
    }
    Ok(format!(
        "<h1>Accounts</h1><form class=field action=/admin/accounts><input name=q value=\"{}\" placeholder=\"Part of an address\"><button>Find</button></form>\
         <p class=dim>{} shown, newest first.</p><div class=scroll><table><tr><td>Address</td><td>State</td><td>Watches running</td><td>Made</td></tr>{rows}</table></div>\
         <h2>Make an account</h2><p class=dim>For a directory's reviewers, say: made confirmed, without a mail; on its page, make it free and give it a reviewer link.</p><form method=post action=/admin/accounts/new class=row><input name=email type=email required placeholder=\"review@zetlyn.com\"><button>Make</button></form>",
        esc(q),
        list.len()
    ))
}

fn account_state(acc: &Account) -> String {
    let mut s = Vec::new();
    if acc.blocked {
        s.push("<b class=error>blocked</b>");
    }
    if acc.free {
        s.push("free");
    } else if acc.paid {
        s.push("subscribed");
    } else if acc.trial_used {
        s.push("trial used");
    } else {
        s.push("trial open");
    }
    if !acc.confirmed {
        s.push("unconfirmed");
    }
    s.join(", ")
}

fn stripe_link(app: &App, path: &str, label: &str) -> String {
    match &app.plan.stripe {
        Some(stripe) if !label.is_empty() => format!("<a href=\"{}{}\" target=_blank rel=noopener>{}</a>", stripe.dashboard(), esc(path), esc(label)),
        _ => esc(label),
    }
}

fn account(app: &App, store: &Store, acc: &Account) -> Result<String, String> {
    let watches = store.watches_of(acc.id)?;
    let running = watches.iter().filter(|w| w.state == "running").count() as i64;
    let keys = store.api_keys_of(acc.id)?;
    let key_list = if keys.is_empty() {
        "none".to_string()
    } else {
        keys.iter().map(|(_, name, hint, _, used)| format!("{} (…{}, {})", esc(name), esc(hint), used.map(ago).unwrap_or_else(|| "never used".into()))).collect::<Vec<_>>().join("; ")
    };
    let info = [
        ("Address", esc(&acc.email)),
        ("State", account_state(acc)),
        ("Made", time(acc.created)),
        ("Watches in the plan", format!("{} ({running} running)", acc.quota)),
        ("A month", euros(monthly(acc, running))),
        ("Stripe customer", stripe_link(app, &format!("customers/{}", acc.stripe_customer), &acc.stripe_customer)),
        ("Stripe subscription", stripe_link(app, &format!("subscriptions/{}", acc.stripe_subscription), &acc.stripe_subscription)),
        ("Sessions open", store.sessions_of(acc.id)?.to_string()),
        ("API keys", key_list),
        ("Reviewer link", match store.review_until(acc.id)? { Some(t) => format!("works until {}", time(t)), None => "none".into() }),
    ];
    let info: String = info.iter().map(|(k, v)| format!("<tr><td class=dim>{k}</td><td>{v}</td></tr>")).collect();
    let id = acc.id;
    let button = |action: &str, label: &str| format!("<form method=post action=\"/admin/a/{id}/{action}\"><button>{label}</button></form>");
    let mut actions = String::new();
    if app.hosted.is_some() {
        actions.push_str(&button("link", "Mail a sign-in link"));
    }
    actions.push_str(&if acc.trial_used { button("trial", "Give the free trial again") } else { button("trial-used", "Mark the free trial used") });
    actions.push_str(&if acc.free { button("unfree", "No longer free") } else { button("free", "Make free (no Stripe)") });
    actions.push_str(&if acc.blocked { button("unblock", "Unblock") } else { button("block", "Block") });
    actions.push_str(&button("access", "End sessions and API keys"));
    if app.hosted.is_some() {
        actions.push_str(&format!("<form method=post action=\"/admin/a/{id}/review\" class=row><input name=days type=number min=1 max=90 value=30 class=short aria-label=\"Days\"><button>Make a reviewer link</button></form>"));
        if store.review_until(acc.id)?.is_some() {
            actions.push_str(&button("unreview", "End the reviewer links"));
        }
    }
    let quota = format!("<form method=post action=\"/admin/a/{id}/quota\" class=row><label class=dim for=n>Watches in the plan</label><input id=n name=n type=number min=0 max=10000 value={} class=short><button>Set</button></form>", acc.quota);
    let delete = format!(
        "<form method=post action=\"/admin/a/{id}/delete\" class=row><input name=confirm placeholder=\"Type {} to delete\" autocomplete=off><button>Delete the account and everything in it</button></form>",
        esc(&acc.email)
    );
    Ok(format!(
        "<h1 class=url>{}</h1><table>{info}</table><h2>Do</h2><div class=actions>{actions}</div>{quota}<h2>Watches</h2>{}<h2>Delete</h2><p class=dim>For a request to be forgotten: the watches with their checks and changes, the links, sessions and keys. A subscription at Stripe is not cancelled by this; cancel it there first.</p>{delete}",
        esc(&acc.email),
        watch_rows(store, &watches, false)?
    ))
}

fn act_on_account(app: &App, store: &Store, acc: &Account, action: &str, a: &Asked) -> Result<(String, String, bool), String> {
    let here = format!("/admin/a/{}", acc.id);
    let said = |s: &str| Ok((here.clone(), s.to_string(), true));
    match action {
        "link" => {
            let Some(h) = &app.hosted else { return Ok((here, "A local instance sends no mail.".into(), false)) };
            let token = store.new_token(acc.id, "signin", None, 1800)?;
            h.tell_soon(&acc.email, mail::signin(&format!("{}/s/{token}", h.base)));
            said("A sign-in link was mailed; it works once, for 30 minutes.")
        }
        "trial" => {
            store.set_trial_used(acc.id, false)?;
            said("The free trial can be used again.")
        }
        "trial-used" => {
            store.set_trial_used(acc.id, true)?;
            said("The free trial is marked used.")
        }
        "free" => {
            store.set_free(acc.id, true)?;
            app.plan.paid(store, acc.id)?;
            said("The account is free: its watches run without an end, nothing is charged.")
        }
        "unfree" => {
            store.set_free(acc.id, false)?;
            let now_paid = store.account_by_id(acc.id)?.is_some_and(|x| x.paid);
            if !now_paid {
                app.plan.stopped(store, acc.id)?;
            }
            said(if now_paid { "No longer free; its own subscription goes on." } else { "No longer free; its watches ended." })
        }
        "block" => {
            store.set_blocked(acc.id, true)?;
            store.end_access(acc.id)?;
            for w in store.watches_of(acc.id)? {
                if w.state != "ended" {
                    store.set_state(w.id, "ended", Some(now()))?;
                }
            }
            said("Blocked: signed out, keys gone, watches ended. A subscription at Stripe goes on until cancelled there.")
        }
        "unblock" => {
            store.set_blocked(acc.id, false)?;
            said("Unblocked. Its watches stay ended; it can start new ones.")
        }
        "unreview" => {
            let n = store.end_review(acc.id)?;
            said(&format!("{n} reviewer links ended, and the account's sessions with them."))
        }
        "access" => {
            let n = store.end_access(acc.id)?;
            said(&format!("{n} sessions and keys ended."))
        }
        "quota" => match a.f("n").trim().parse::<i64>() {
            Ok(n) if (0..=10_000).contains(&n) => {
                store.set_quota(acc.id, n)?;
                app.plan.sync(store, acc.id);
                said(&format!("{n} watches are in the plan now."))
            }
            _ => Ok((here, "A number from 0 to 10000, please.".into(), false)),
        },
        "delete" => {
            if a.f("confirm").trim().to_lowercase() != acc.email || acc.email == LOCAL {
                return Ok((here, "Not deleted: type the address exactly to delete.".into(), false));
            }
            store.delete_account(acc.id)?;
            Ok(("/admin/accounts".into(), format!("{} is deleted with everything in it.", acc.email), true))
        }
        _ => Ok((here, "No such action.".into(), false)),
    }
}

fn watches(store: &Store, a: &Asked) -> Result<String, String> {
    let state = a.q("state").unwrap_or_default();
    let outcome = a.q("outcome").unwrap_or_default();
    let q = a.q("q").unwrap_or_default().to_lowercase();
    let emails: HashMap<i64, String> = store.find_accounts("", i64::MAX)?.into_iter().map(|a| (a.id, a.email)).collect();
    let mut list: Vec<Watch> = store
        .watches()?
        .into_iter()
        .filter(|w| state.is_empty() || w.state == state)
        .filter(|w| outcome.is_empty() || w.status == outcome)
        .filter(|w| q.is_empty() || w.url.to_lowercase().contains(&q) || emails.get(&w.account).is_some_and(|e| e.contains(&q)))
        .collect();
    list.sort_by(|x, y| y.id.cmp(&x.id));
    let total = list.len();
    list.truncate(500);
    let select = |name: &str, options: &[&str], current: &str| {
        let o: String = std::iter::once("").chain(options.iter().copied()).map(|v| format!("<option value=\"{v}\"{}>{}</option>", if v == current { " selected" } else { "" }, if v.is_empty() { "any" } else { v })).collect();
        format!("<label class=dim>{name} <select name={name}>{o}</select></label>")
    };
    Ok(format!(
        "<h1>Watches</h1><form action=/admin/watches class=row>{}{}<input name=q value=\"{}\" placeholder=\"Part of an address or URL\"><button>Show</button></form><p class=dim>{total} found{}.</p>{}",
        select("state", &["running", "waiting", "pending", "ended"], &state),
        select("outcome", &["new", "same", "changed", "unclear", "gone", "failed"], &outcome),
        esc(&q),
        if total > 500 { ", the newest 500 shown" } else { "" },
        watch_rows(store, &list, true)?
    ))
}

fn watch_rows(store: &Store, list: &[Watch], with_account: bool) -> Result<String, String> {
    if list.is_empty() {
        return Ok("<p class=dim>None.</p>".into());
    }
    let emails: HashMap<i64, String> = if with_account { store.find_accounts("", i64::MAX)?.into_iter().map(|a| (a.id, a.email)).collect() } else { HashMap::new() };
    let rows: String = list
        .iter()
        .map(|w| {
            let account = if with_account { format!("<td><a href=\"/admin/a/{}\">{}</a></td>", w.account, esc(emails.get(&w.account).map_or("?", String::as_str))) } else { String::new() };
            format!(
                "<tr><td><a href=\"/admin/w/{}\">{}</a></td><td class=url>{}</td>{account}<td>{}</td><td><span class=\"dot {}\"></span>{}</td><td class=dim>{}</td><td class=dim>{}</td></tr>",
                w.id,
                w.id,
                esc(&w.url),
                esc(&w.state),
                esc(&w.status),
                esc(&w.status),
                every(w.interval_min),
                w.last_checked.map(ago).unwrap_or_else(|| "never".into())
            )
        })
        .collect();
    let account = if with_account { "<td>Account</td>" } else { "" };
    Ok(format!("<div class=scroll><table><tr><td>#</td><td>Page</td>{account}<td>State</td><td>Last check</td><td>Checked</td><td>Last</td></tr>{rows}</table></div>"))
}

fn watch(store: &Store, w: &Watch) -> Result<String, String> {
    let owner = store.account_by_id(w.account)?;
    let checks: String = store
        .checks(w.id, 30)?
        .iter()
        .map(|c| format!("<tr><td class=dim>{}</td><td><span class=\"dot {}\"></span>{}{}</td><td class=dim>{}</td></tr>", time(c.at), esc(&c.outcome), esc(&c.outcome), if c.moved { " (moved)" } else { "" }, esc(&clip(&c.detail, 160))))
        .collect();
    let changes: String = store.changes(w.id, 10)?.iter().map(|c| format!("<li><span class=dim>{}</span><br><span class=gone>{}</span><br><span class=came>{}</span></li>", time(c.at), esc(&clip(&c.before, 300)), esc(&clip(&c.after, 300)))).collect();
    let options: String = INTERVALS.iter().map(|m| format!("<option value={m}{}>{}</option>", if *m == w.interval_min { " selected" } else { "" }, every(*m))).collect();
    let id = w.id;
    Ok(format!(
        "<h1 class=url><a href=\"{u}\" target=_blank rel=noopener>{u}</a></h1><p>{what} · {state} · checked {every}{rendered} · last check {last} · made {made}{until}</p><p>Account: {owner}</p>\
         <div class=actions><form method=post action=\"/admin/w/{id}/check\"><button>Check now</button></form>\
         <form method=post action=\"/admin/w/{id}/every\" class=row><select name=minutes>{options}</select><button>Set</button></form>\
         <form method=post action=\"/admin/w/{id}/end\"><button>End</button></form>\
         <form method=post action=\"/admin/w/{id}/delete\" class=row><label class=dim><input type=checkbox name=sure value=1> sure</label><button>Delete</button></form></div>\
         <h2>Now</h2><pre>{value}</pre><h2>Changes</h2>{changes}<h2>Checks</h2><div class=scroll><table>{checks}</table></div>",
        u = esc(&w.url),
        what = esc(&life::what(w)),
        state = esc(&w.state),
        rendered = if w.render { " · rendered in Chrome" } else { "" },
        every = every(w.interval_min),
        last = w.last_checked.map(ago).unwrap_or_else(|| "never".into()),
        made = time(w.created),
        until = w.until.map(|u| format!(" · until {}", time(u))).unwrap_or_default(),
        owner = owner.map(|o| format!("<a href=\"/admin/a/{}\">{}</a>", o.id, esc(&o.email))).unwrap_or_else(|| "?".into()),
        value = esc(w.last_value.as_deref().unwrap_or("")),
        changes = if changes.is_empty() { "<p class=dim>None yet.</p>".to_string() } else { format!("<ul class=changes>{changes}</ul>") },
    ))
}

fn act_on_watch(app: &App, store: &Store, w: &Watch, action: &str, a: &Asked) -> Result<(String, String, bool), String> {
    let here = format!("/admin/w/{}", w.id);
    match action {
        "check" => {
            let done = check::round_of(store, vec![w.clone()])?;
            if let (Some(h), Some(c)) = (&app.hosted, done.first()) {
                if w.state == "running" {
                    life::notify(store, h, c)?;
                }
            }
            let outcome = done.first().map_or("nothing", |c| c.outcome.as_str());
            Ok((here, format!("Checked: {outcome}."), true))
        }
        "every" => match a.f("minutes").parse::<i64>() {
            Ok(m) if INTERVALS.contains(&m) => {
                store.set_interval(w.id, m)?;
                Ok((here, format!("Checked {} from now on.", every(m)), true))
            }
            _ => Ok((here, "No such interval.".into(), false)),
        },
        "end" => {
            store.set_state(w.id, "ended", Some(now()))?;
            app.plan.sync(store, w.account);
            Ok((here, "Ended.".into(), true))
        }
        "delete" if a.f("sure") == "1" => {
            store.delete(w.id)?;
            app.plan.sync(store, w.account);
            Ok((format!("/admin/a/{}", w.account), format!("Watch {} is deleted.", w.id), true))
        }
        "delete" => Ok((here, "Not deleted: tick “sure” first.".into(), false)),
        _ => Ok((here, "No such action.".into(), false)),
    }
}

/// The host of an address: what a site is, for counting.
fn host(url: &str) -> String {
    url.split_once("://").map_or(url, |(_, r)| r).split(['/', '?', '#']).next().unwrap_or("").trim_start_matches("www.").to_lowercase()
}

/// What went wrong, without the address it went wrong at: `https://x/y: HTTP 403` is `HTTP 403`.
fn reason(detail: &str) -> String {
    let d = if detail.starts_with("http") { detail.split_once(": ").map_or(detail, |(_, r)| r) } else { detail };
    clip(d.trim(), 90)
}

fn quality(store: &Store, days: i64) -> Result<String, String> {
    let days = days.clamp(1, 90);
    let since = now() - days * 86400;
    let mut per_host: HashMap<String, (i64, i64, i64, i64, String)> = HashMap::new();
    for (url, n) in store.counts("SELECT w.url, COUNT(*) FROM checks c JOIN watches w ON w.id = c.watch WHERE c.at >= ?1 AND c.outcome != 'new' GROUP BY w.url", [since])? {
        per_host.entry(host(&url)).or_default().0 += n;
    }
    let mut reasons: HashMap<String, i64> = HashMap::new();
    for (url, outcome, detail, _) in store.misses_since(since)? {
        let e = per_host.entry(host(&url)).or_default();
        match outcome.as_str() {
            "failed" => e.1 += 1,
            "unclear" => e.2 += 1,
            _ => e.3 += 1,
        }
        if e.4.is_empty() && outcome == "failed" {
            e.4 = reason(&detail);
        }
        if outcome == "failed" {
            *reasons.entry(reason(&detail)).or_default() += 1;
        }
    }
    let mut hosts: Vec<_> = per_host.into_iter().filter(|(_, v)| v.1 + v.2 + v.3 > 0).collect();
    hosts.sort_by(|x, y| (y.1 .1 + y.1 .2 + y.1 .3).cmp(&(x.1 .1 + x.1 .2 + x.1 .3)));
    let rows: String = hosts
        .iter()
        .take(100)
        .map(|(h, (all, failed, unclear, gone, why))| {
            format!(
                "<tr><td><a href=\"/admin/watches?q={}\">{}</a></td><td>{all}</td><td>{failed}</td><td>{unclear}</td><td>{gone}</td><td>{}</td><td class=dim>{}</td></tr>",
                enc(h),
                esc(h),
                if *all > 0 { format!("{:.0} %", (failed + unclear + gone) as f64 * 100.0 / *all as f64) } else { "–".into() },
                esc(why)
            )
        })
        .collect();
    let mut reasons: Vec<_> = reasons.into_iter().collect();
    reasons.sort_by(|x, y| y.1.cmp(&x.1));
    let reason_rows: String = reasons.iter().take(20).map(|(r, n)| format!("<tr><td>{n}</td><td>{}</td></tr>", esc(r))).collect();
    // Pages that could not be read when somebody wanted to watch them: no watch keeps these.
    let mut refused: HashMap<String, (i64, Vec<String>, String)> = HashMap::new();
    for (host, what, why, _) in store.refusals_since(since)? {
        let e = refused.entry(host).or_default();
        e.0 += 1;
        if !e.1.contains(&what) {
            e.1.push(what);
        }
        if e.2.is_empty() {
            e.2 = reason(&why);
        }
    }
    let mut refused: Vec<_> = refused.into_iter().collect();
    refused.sort_by(|x, y| y.1 .0.cmp(&x.1 .0));
    let refused_rows: String = refused.iter().take(100).map(|(h, (n, whats, why))| format!("<tr><td>{}</td><td>{n}</td><td class=dim>{}</td><td class=dim>{}</td></tr>", esc(h), esc(&whats.join(", ")), esc(why))).collect();
    let span: String = [1, 7, 30].iter().map(|d| format!("<a href=\"/admin/quality?days={d}\"{}>{d} days</a>", if *d == days { " class=on" } else { "" })).collect::<Vec<_>>().join(" · ");
    Ok(format!(
        "<h1>Read quality</h1><p class=dim>Where reading goes wrong, by site, over {span}. Failed: the page could not be read (blocked, an error, too large, not a web page). Unclear: the marked place was not found with certainty. Gone: it is no longer there.</p>\
         <h2>Sites</h2>{}<h2>Why reading failed</h2>{}<h2>Not watchable when tried</h2><p class=dim>Pages a person or an agent wanted to watch and Zetlyn could not read: in a preview, read_place, a new watch, or marking on the site.</p>{}",
        if rows.is_empty() { "<p class=dim>Nothing went wrong.</p>".to_string() } else { format!("<div class=scroll><table><tr><td>Site</td><td>Checks</td><td>Failed</td><td>Unclear</td><td>Gone</td><td>Share</td><td>Last reason</td></tr>{rows}</table></div>") },
        if reason_rows.is_empty() { "<p class=dim>None.</p>".to_string() } else { format!("<table>{reason_rows}</table>") },
        if refused_rows.is_empty() { "<p class=dim>None.</p>".to_string() } else { format!("<div class=scroll><table><tr><td>Site</td><td>Times</td><td>In</td><td>Last reason</td></tr>{refused_rows}</table></div>") }
    ))
}

/// The mail log (`mail.rs`), newest first: (at, `sent` or what went wrong, to, subject).
fn mail_lines(app: &App) -> Vec<(i64, String, String, String)> {
    let text = std::fs::read_to_string(app.data.join("mail.log")).unwrap_or_default();
    let mut lines: Vec<_> = text
        .lines()
        .filter_map(|l| {
            let mut p = l.splitn(4, '\t');
            Some((p.next()?.parse().ok()?, p.next()?.to_string(), p.next()?.to_string(), p.next().unwrap_or("").to_string()))
        })
        .collect();
    lines.reverse();
    lines
}

fn mails(app: &App, q: &str) -> String {
    let q = q.to_lowercase();
    let all = mail_lines(app);
    let shown: Vec<_> = all.iter().filter(|m| q.is_empty() || m.2.to_lowercase().contains(&q) || m.3.to_lowercase().contains(&q)).take(300).collect();
    let rows: String = shown
        .iter()
        .map(|(at, outcome, to, subject)| format!("<tr><td class=dim>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>", time(*at), if outcome == "sent" { "sent".to_string() } else { format!("<b class=error>{}</b>", esc(outcome)) }, esc(to), esc(subject)))
        .collect();
    let how = match &app.hosted {
        Some(h) if h.mailer.smtp.is_some() => "Mail goes by SMTP.",
        Some(_) => "No SMTP is set: mail is written to outbox/ in the data directory.",
        None => "A local instance sends no mail.",
    };
    format!(
        "<h1>Mail</h1><p class=dim>{how} What was sent, newest first; the texts are not kept.</p><form class=field action=/admin/mail><input name=q value=\"{}\" placeholder=\"Address or subject\"><button>Find</button></form>{}",
        esc(&q),
        if rows.is_empty() { "<p class=dim>No mail yet.</p>".to_string() } else { format!("<div class=scroll><table><tr><td>When</td><td></td><td>To</td><td>Subject</td></tr>{rows}</table></div>") }
    )
}

fn billing(app: &App, store: &Store, compare: bool) -> Result<String, String> {
    let counts = store.watch_counts()?;
    let accounts: Vec<Account> = store.find_accounts("", i64::MAX)?.into_iter().filter(|a| a.paid || a.free || !a.stripe_subscription.is_empty()).collect();
    let mut total = 0;
    let mut rows = String::new();
    for acc in &accounts {
        let running = counts.get(&acc.id).map_or(0, |c| c.0);
        let extra = (running - acc.quota).max(0);
        let month = monthly(acc, running);
        total += month;
        let stripe = match (&app.plan.stripe, compare && !acc.stripe_subscription.is_empty() && !acc.free) {
            (Some(s), true) => match s.subscription(&acc.stripe_subscription) {
                Ok((status, quantity)) => {
                    let active = matches!(status.as_str(), "active" | "trialing" | "past_due");
                    let wrong = active != acc.paid || (active && quantity != extra);
                    let said = format!("{status}, {quantity} extra");
                    if wrong { format!("<b class=error>{} — Zetlyn: {}, {extra} extra</b>", esc(&said), if acc.paid { "paid" } else { "not paid" }) } else { esc(&said) }
                }
                Err(e) => format!("<span class=error>{}</span>", esc(&e)),
            },
            _ => String::new(),
        };
        rows.push_str(&format!(
            "<tr><td><a href=\"/admin/a/{}\">{}</a></td><td>{}</td><td>{running} / {}</td><td>{extra}</td><td>{}</td><td>{}</td><td>{stripe}</td></tr>",
            acc.id,
            esc(&acc.email),
            account_state(acc),
            acc.quota,
            euros(month),
            stripe_link(app, &format!("subscriptions/{}", acc.stripe_subscription), &acc.stripe_subscription),
        ));
    }
    let events: String = store.events(50)?.iter().map(|(id, kind, at)| format!("<tr><td class=dim>{}</td><td>{}</td><td class=dim>{}</td></tr>", time(*at), esc(kind), esc(id))).collect();
    let setup = match &app.plan.stripe {
        Some(_) => format!("Stripe is set up. <a href=\"/admin/billing?compare=1\">Compare with Stripe</a> asks Stripe for each subscription and marks where it and Zetlyn differ."),
        None => "Stripe is not set up on this instance: nothing can be bought; accounts made free here work all the same.".to_string(),
    };
    Ok(format!(
        "<h1>Billing</h1><p class=dim>{setup}</p><div class=stats>{}{}</div><h2>Accounts that pay or are free</h2>{}<h2>Stripe's events</h2>{}",
        stat("a month, VAT incl.", euros(total), ""),
        stat("accounts", accounts.len(), ""),
        if rows.is_empty() { "<p class=dim>None yet.</p>".to_string() } else { format!("<div class=scroll><table><tr><td>Account</td><td>State</td><td>Running / plan</td><td>Extra</td><td>A month</td><td>Subscription</td><td>{}</td></tr>{rows}</table></div>", if compare { "At Stripe" } else { "" }) },
        if events.is_empty() { "<p class=dim>None yet.</p>".to_string() } else { format!("<div class=scroll><table>{events}</table></div>") }
    ))
}

fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b => format!("{} KB", b / 1024),
    }
}

fn system(app: &App, store: &Store) -> Result<String, String> {
    let file = |name: &str| std::fs::metadata(app.data.join(name)).map(|m| m.len()).unwrap_or(0);
    let up = now() - app.started;
    let mode = match &app.hosted {
        Some(h) => format!("hosted at {}", esc(&h.base)),
        None => "local".into(),
    };
    let mut limited: Vec<(String, usize)> = {
        let limits = app.limits.lock().unwrap_or_else(|e| e.into_inner());
        let at = now();
        limits.iter().map(|(k, t)| (k.clone(), t.iter().filter(|t| at - **t < 3600).count())).filter(|(_, n)| *n > 0).collect()
    };
    limited.sort_by(|x, y| y.1.cmp(&x.1));
    let limit_rows: String = limited.iter().take(20).map(|(k, n)| format!("<tr><td>{}</td><td>{n}</td></tr>", esc(k))).collect();
    let info = [
        ("Version", env!("CARGO_PKG_VERSION").to_string()),
        ("Instance", esc(&store.instance())),
        ("Mode", mode),
        ("Running since", format!("{} ({} days, {} hours)", time(app.started), up / 86400, up % 86400 / 3600)),
        ("Payment", app.plan.describe()),
        ("Rendering", esc(&crate::render::describe())),
        ("Data", esc(&app.data.display().to_string())),
        ("Database", format!("{} (+ {} not yet written back)", size(file("zetlyn.db")), size(file("zetlyn.db-wal")))),
        ("Checks kept", store.count("SELECT COUNT(*) FROM checks", [])?.to_string()),
        ("Admins", if app.hosted.is_some() { esc(&app.admins.join(", ")) } else { "everybody (local)".into() }),
    ];
    let info: String = info.iter().map(|(k, v)| format!("<tr><td class=dim>{k}</td><td>{v}</td></tr>")).collect();
    let banner = app.banner.lock().map(|b| b.clone()).unwrap_or_default();
    Ok(format!(
        "<h1>System</h1><table>{info}</table><h2>Notice</h2><p class=dim>Shown over every page, e.g. before maintenance. Empty: none.</p><form method=post action=/admin/banner class=field><input name=banner value=\"{}\" maxlength=300 placeholder=\"Zetlyn is updated tonight at 22:00 CET for a few minutes.\"><button>Save</button></form>\
         <h2>Busiest addresses, last hour</h2><p class=dim>Requests counted against the limits (reading a page to mark, new watches, sign-in links, the API), by kind and address.</p>{}\
         <p class=dim>The alarm runs on the server by its own timer (<code>zetlyn-alert.timer</code>) and mails when something is wrong. Backups are Hetzner's daily backups of the whole server, the last seven.</p>",
        esc(&banner),
        if limit_rows.is_empty() { "<p class=dim>None.</p>".to_string() } else { format!("<table>{limit_rows}</table>") }
    ))
}

fn log(store: &Store) -> Result<String, String> {
    let rows: String = store.admin_log(300)?.iter().map(|(at, who, action, target)| format!("<tr><td class=dim>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>", time(*at), esc(who), esc(action), esc(target))).collect();
    Ok(format!("<h1>Admin log</h1><p class=dim>Every change made on these pages, newest first.</p>{}", if rows.is_empty() { "<p class=dim>Nothing yet.</p>".to_string() } else { format!("<div class=scroll><table>{rows}</table></div>") }))
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() > n { format!("{}…", s.chars().take(n - 1).collect::<String>()) } else { s.to_string() }
}
