//! What Zetlyn charges and how a watch lives by it: an account's first watch runs free for three
//! days; with a subscription at Stripe (9.99 € a month, up to 10 watches, 0.50 € for each one
//! more, VAT included) its watches run without an end. A day before a free trial ends the
//! account is told, and when it has. An account that never paid goes 30 days after its last watch
//! ended. Without Stripe's settings an instance runs the same, and nothing can be bought.

use serde_json::{json, Value as J};

use crate::billing::{self, Paid, Stripe};
use crate::life::Hosted;
use crate::mail;
use crate::store::{now, Account, Store};

pub const TRIAL: i64 = 3 * 86400;
pub const REMIND: i64 = 86400;

/// How a confirmed watch starts.
pub enum Start {
    /// Watching, until `until` if it has an end; `note` says so to the person.
    Running { until: Option<i64>, note: String },
    /// Not yet: `note` says why, and `go` is the page that makes it start.
    Waiting { note: String, go: String },
}

/// A request the plan's own pages answer, read already.
pub struct Req<'a> {
    pub method: &'a str,
    pub path: &'a [&'a str],
    pub body: &'a [u8],
    /// A request header by name, empty when there is none.
    pub header: &'a dyn Fn(&str) -> String,
    pub account: Option<&'a Account>,
}

/// An answer of the plan's own pages.
pub enum Out {
    /// A page in the site's layout.
    Page { status: u16, title: String, body: String },
    Redirect(String),
    Text { status: u16, body: String },
}

pub struct Plan {
    pub stripe: Option<Stripe>,
}

pub const PRICING: &str = r#"<section class="block pricing"><p class=overline>Pricing</p><h2>Three days free.<br><span>Then 9.99 € a month.</span></h2><div class=plans>
<div class=plan><h3>Try it</h3><p class=price>Free</p><p>One watch for 3 days, checked every hour. No card.</p></div>
<div class="plan main"><h3>Subscription</h3><p class=price>9.99 € <span>a month</span></p><p>Up to 10 watches, as long as you like, checked as often as every minute. 0.50 € a month for each watch more. VAT included. Cancel any time.</p></div>
</div></section>"#;

pub const HERO_NOTE: &str = "The first 3 days are free.";
pub const MARK_NOTE: &str = "Checked every hour. The first 3 days are free.";
pub const CONFIRM_NOTE: &str = "The first 3 days are free. ";

/// How often a watch can be checked, in minutes: every hour for everyone, more often with a
/// subscription.
pub const INTERVALS: &[i64] = &[60, 15, 5, 1];

/// Whether an account may have its watches checked every `minutes`: every hour always; more often
/// with a subscription, or on an instance nobody signs in to.
pub fn interval_allowed(account: &Account, hosted: bool, minutes: i64) -> bool {
    INTERVALS.contains(&minutes) && (minutes == 60 || account.paid || !hosted)
}

impl Plan {
    pub fn from_env() -> Plan {
        Plan { stripe: Stripe::from_env() }
    }

    /// A watch whose address was confirmed starts: with no end while its account pays, for three days
    /// on the account's one free trial, and otherwise it waits for a subscription.
    pub fn start(&self, store: &Store, account: &Account, watch: i64) -> Result<Start, String> {
        if account.paid {
            store.set_state(watch, "running", None)?;
            self.sync(store, account.id);
            return Ok(Start::Running { until: None, note: "Watching. A change is emailed to the account's address.".into() });
        }
        if store.take_trial(account.id)? {
            let until = now() + TRIAL;
            store.set_state(watch, "running", Some(until))?;
            return Ok(Start::Running { until: Some(until), note: "Watching. This is the account's free trial: it runs for 3 days.".into() });
        }
        store.set_state(watch, "waiting", None)?;
        Ok(Start::Waiting { note: "Made, but not watching yet: the free trial is used. It starts once the account subscribes at".into(), go: "/subscribe".into() })
    }

    /// The account pays now: every watch it confirmed runs, with no end.
    pub fn paid(&self, store: &Store, account: i64) -> Result<(), String> {
        for w in store.watches_of(account)? {
            if w.state != "pending" {
                store.set_state(w.id, "running", None)?;
            }
        }
        self.sync(store, account);
        Ok(())
    }

    /// Back from Stripe's checkout, perhaps before Stripe's event: what the checkout says is taken
    /// at once, the same as the event will say it (which then changes nothing). Whether the account
    /// pays now.
    pub fn settle(&self, store: &Store, account: &Account) -> Result<bool, String> {
        let (Some(stripe), false) = (&self.stripe, account.paid) else { return Ok(account.paid) };
        let session = store.checkout_session(account.id);
        if session.is_empty() {
            return Ok(false);
        }
        let (paid, customer, subscription) = stripe.session(&session)?;
        if paid {
            store.set_paid(account.id, true, &customer, &subscription)?;
            store.set_checkout_session(account.id, "")?;
            self.paid(store, account.id)?;
        }
        Ok(paid)
    }

    /// A free trial's watch ended in its first quarter hour, before anything changed: the free trial
    /// comes back. Trying a page that turns out not to work should not cost it. Before the watch
    /// is deleted.
    pub fn ended_early(&self, store: &Store, w: &crate::store::Watch) -> Result<bool, String> {
        let Some(acc) = store.account_by_id(w.account)? else { return Ok(false) };
        if acc.paid || w.until.is_none() || now() - w.created > 15 * 60 || !store.changes(w.id, 1)?.is_empty() {
            return Ok(false);
        }
        store.set_trial_used(acc.id, false)?;
        Ok(true)
    }

    /// The account no longer pays: its watches end. Stripe tells the customer itself.
    pub fn stopped(&self, store: &Store, account: i64) -> Result<(), String> {
        for w in store.watches_of(account)? {
            if w.state == "running" || w.state == "waiting" {
                store.set_state(w.id, "ended", None)?;
            }
        }
        Ok(())
    }

    /// The subscription's extra quantity matched to the running watches beyond the quota. Said and
    /// otherwise let be when Stripe cannot be reached; the next change tries again.
    pub fn sync(&self, store: &Store, account: i64) {
        let (Some(stripe), Ok(Some(a))) = (&self.stripe, store.account_by_id(account)) else { return };
        if !a.paid || a.stripe_subscription.is_empty() {
            return;
        }
        let running = store.watches_of(account).map(|ws| ws.iter().filter(|w| w.state == "running").count() as i64).unwrap_or(0);
        if let Err(e) = stripe.sync_extra(&a.stripe_subscription, (running - a.quota).max(0)) {
            eprintln!("account {account}: {e}");
            billing_trouble(store, &format!("account {account}: {e}"));
        }
    }

    /// Once a minute, before the checks: free trials ending soon are said, those that ended end, and
    /// what was never paid for is cleaned away.
    pub fn tick(&self, store: &Store, h: &Hosted, at: i64) -> Result<Vec<String>, String> {
        let mut said = Vec::new();
        for w in store.ending(at, REMIND)? {
            store.set_reminded(w.id)?;
            if let Some(a) = store.account_by_id(w.account)?.filter(|a| !a.paid) {
                let link = h.door(store, a.id, "/subscribe")?;
                let checks = store.check_count(w.id)?;
                h.tell(&a.email, &mail::trial_ending(&w.url, checks, w.last_value.as_deref().unwrap_or(""), &link));
                said.push(format!("{:>5} reminded {}", w.id, w.url));
            }
        }
        for w in store.expired(at)? {
            store.set_state(w.id, "ended", w.until)?;
            said.push(format!("{:>5} ended    {}", w.id, w.url));
            if let Some(a) = store.account_by_id(w.account)? {
                let link = h.door(store, a.id, "/subscribe")?;
                h.tell(&a.email, &mail::ended(&w.url, &link));
            }
        }
        let (watches, accounts) = store.clean_unpaid(at)?;
        if watches + accounts > 0 {
            said.push(format!("cleaned {watches} waiting watches and {accounts} unpaid accounts"));
        }
        Ok(said)
    }

    /// Its pages: subscribing, the customer portal, and Stripe's events. None elsewhere.
    pub fn route(&self, store: &Store, h: &Hosted, req: &Req) -> Option<Result<Out, String>> {
        let page = |title: &str, body: String| Out::Page { status: 200, title: title.into(), body };
        Some(Ok(match (req.method, req.path) {
            ("GET", ["subscribe"]) => match req.account {
                Some(a) if a.paid => Out::Redirect("/account/billing".into()),
                Some(a) => page("Subscribe", self.subscribe_page(a, false, None)),
                None => Out::Redirect("/signin".into()),
            },
            ("POST", ["subscribe"]) => match (req.account, &self.stripe) {
                // Starting at once is asked for in so many words, or nothing starts.
                (Some(a), Some(_)) if !form_urlencoded::parse(req.body).any(|(k, v)| k == "start_now" && v == "1") => page("Subscribe", self.subscribe_page(a, true, None)),
                (Some(a), Some(stripe)) => match stripe.checkout(a.id, &a.email, &h.base, "/subscribe").and_then(|(url, session)| store.set_checkout_session(a.id, &session).map(|_| url)) {
                    Ok(url) => Out::Redirect(url),
                    Err(e) => return Some(Err(e)),
                },
                (Some(a), None) => page("Subscribe", self.subscribe_page(a, false, None)),
                (None, _) => Out::Redirect("/signin".into()),
            },
            // A link an LLM or program gave the person (`subscribe_link`): it names the account, so
            // subscribing needs no signing in on this browser. It can only start a checkout the
            // person pays for themselves, so it signs nobody in and stays usable for its hour.
            ("GET" | "POST", ["subscribe", token]) => return Some(self.by_link(store, h, req, token)),
            ("POST", ["billing"]) => match (req.account, &self.stripe) {
                (Some(a), Some(stripe)) if !a.stripe_customer.is_empty() => match stripe.portal(&a.stripe_customer, &format!("{}/account/billing", h.base)) {
                    Ok(url) => Out::Redirect(url),
                    Err(e) => return Some(Err(e)),
                },
                _ => Out::Redirect("/account".into()),
            },
            ("POST", ["stripe"] | ["billing", "stripe"]) => self.event(store, req),
            _ => return None,
        }))
    }

    /// `/subscribe/<token>`: subscribing by a link `subscribe_link` made for an account.
    fn by_link(&self, store: &Store, h: &Hosted, req: &Req, token: &str) -> Result<Out, String> {
        let page = |body: String| Out::Page { status: 200, title: "Subscribe".into(), body };
        let held = match store.token_holder(token, "subscribe")? {
            Some(id) => store.account_by_id(id)?,
            None => None,
        };
        Ok(match (held, &self.stripe) {
            (None, _) => page("<section class=subscribe><h1>This link has expired.</h1><p class=lead>Ask for a new one, or <a href=\"/signin\">sign in</a> and subscribe under Account.</p></section>".into()),
            (Some(a), _) if a.paid => page("<section class=subscribe><h1>You subscribe already.</h1><p class=lead>Your watches run without an end. Invoices and cancelling are under <a href=\"/account/billing\">Account, Billing</a>.</p></section>".into()),
            (Some(a), Some(stripe)) if req.method == "POST" => {
                if !form_urlencoded::parse(req.body).any(|(k, v)| k == "start_now" && v == "1") {
                    page(self.subscribe_page(&a, true, Some(token)))
                } else {
                    let (url, session) = stripe.checkout(a.id, &a.email, &h.base, &format!("/subscribe/{token}"))?;
                    store.set_checkout_session(a.id, &session)?;
                    Out::Redirect(url)
                }
            }
            (Some(a), _) => page(self.subscribe_page(&a, false, Some(token))),
        })
    }

    /// What the API says of the account's subscription.
    pub fn api_account(&self, account: &Account, base: &str) -> J {
        json!({
            "subscribed": account.paid,
            "watches_in_plan": account.quota,
            "free_trial_used": account.trial_used,
            "subscribe_url": format!("{base}/subscribe"),
            "price": "9.99 € a month for up to 10 watches, 0.50 € a month for each one more, VAT included",
        })
    }

    pub fn describe(&self) -> String {
        if self.stripe.is_some() { "payment by Stripe".into() } else { "payment not set up".into() }
    }

    /// `/subscribe`: the free trial and the subscription side by side, and subscribing, with the
    /// request that the service start at once asked for in so many words (`/withdrawal`).
    fn subscribe_page(&self, account: &Account, error: bool, link: Option<&str>) -> String {
        let trial = if account.trial_used { "You have used it." } else { "Yours to try: watch a page." };
        let ask = if self.stripe.is_some() {
            format!(
                "<form method=post action=/subscribe{} class=subscribe-form>{}{}<label class=consent><input type=checkbox name=start_now value=1 required><span>I want my subscription to start right away. If I withdraw within 14 days, I pay for the days until then (<a href=\"/withdrawal\">cancellation policy</a>).</span></label><button class=primary>Subscribe · 9.99 € a month</button><p class=dim>VAT included. You pay at Stripe, by card or another way it offers, and agree to the <a href=\"/terms\">terms</a>.</p></form>",
                link.map(|t| format!("/{t}")).unwrap_or_default(),
                link.map(|_| format!("<p class=dim>For the account {}.</p>", crate::serve::esc(&account.email))).unwrap_or_default(),
                if error { "<p class=error>Please tick the box first: without it the subscription cannot start right away.</p>" } else { "" }
            )
        } else {
            "<p class=notice>Payment is not set up on this instance yet, so nothing can be bought here.</p>".to_string()
        };
        format!(
            r#"<section class=subscribe>
<p class=crumb><a href="/watches">Your watches</a> <span class=dim>/</span> Subscription</p>
<h1>Keep watching, without an end.</h1>
<p class=lead>One subscription for up to 10 pages, checked as often as you like. Cancel any time.</p>
<div class=tiers>
  <div class=tier><h2>Free trial</h2><p class=price>0 €</p><ul class="ticks plain"><li>One watch</li><li>For 3 days</li><li>Checked every hour</li></ul><p class=dim>{trial}</p></div>
  <div class="tier main"><h2>Subscription</h2><p class=price>9.99 € <span>a month</span></p><ul class=ticks><li>Up to 10 watches, for as long as you like</li><li>0.50 € a month for each watch more</li><li>Checked as often as every minute</li><li>For programs and LLMs: the API and MCP</li><li>Cancel any time, to the end of the month</li></ul>{ask}</div>
</div>
<div class=qa>
  <div><h3>What happens to my watches?</h3><p>They run without an end while you subscribe. A watch that waits for a subscription starts the moment the payment is through.</p></div>
  <div><h3>How do I cancel?</h3><p>Under Account, “Invoices, payment and cancelling”, at any time; the subscription then ends with the month already paid for.</p></div>
  <div><h3>Invoices?</h3><p>Stripe sends one each month by email, and keeps all of them under Account.</p></div>
</div>
</section>"#
        )
    }

    /// An event from Stripe, checked and applied.
    fn event(&self, store: &Store, req: &Req) -> Out {
        let text = |status: u16, body: String| Out::Text { status, body };
        let Some(stripe) = &self.stripe else { return text(404, "not set up".into()) };
        let event = match billing::verified(req.body, &(req.header)("Stripe-Signature"), &stripe.webhook_secret, now()) {
            Ok(e) => e,
            Err(e) => return text(400, e),
        };
        let done = billing::apply(store, &event).and_then(|p| match p {
            Paid::Started(acc) => self.paid(store, acc),
            Paid::Stopped(acc) => self.stopped(store, acc),
            Paid::Nothing => Ok(()),
        });
        match done {
            Ok(()) => text(200, "ok".into()),
            Err(e) => {
                eprintln!("stripe: {e}");
                billing_trouble(store, &format!("an event from Stripe: {e}"));
                text(500, e)
            }
        }
    }
}

/// What went wrong between Zetlyn and Stripe, noted for `zetlyn alert`, which mails it: money
/// that is not charged, or charged for what does not run, must not wait for someone to read logs.
fn billing_trouble(store: &Store, what: &str) {
    let _ = store.set_meta("billing_error", &format!("{}\t{}", now(), what.replace(['\t', '\n'], " ")));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::life::{start, tick};
    use crate::mail::Mailer;
    use crate::store::Mode;

    fn hosted(dir: &std::path::Path) -> Hosted {
        Hosted { base: "https://zetlyn.test".into(), mailer: Mailer { from: "Zetlyn <hello@zetlyn.test>".into(), smtp: None, outbox: dir.join("outbox") } }
    }

    #[test]
    fn the_first_watch_is_a_free_trial_the_second_waits_and_paying_starts_both() {
        let dir = std::env::temp_dir().join(format!("zetlyn-plan-{}", crate::crypto::token()));
        let (h, s, plan) = (hosted(&dir), Store::in_memory(), Plan { stripe: None });
        let a = s.account("a@example.com").unwrap();
        let page = Mode::Page { volatile: vec![] };
        let one = s.add_watch_as(a, "https://example.com/1", &page, 60, None, "x", "pending").unwrap();
        let two = s.add_watch_as(a, "https://example.com/2", &page, 60, None, "x", "pending").unwrap();
        assert!(matches!(start(&s, &h, &plan, a, one).unwrap(), Start::Running { until: Some(_), .. }));
        assert!(matches!(start(&s, &h, &plan, a, two).unwrap(), Start::Waiting { .. }));
        s.set_paid(a, true, "cus_1", "").unwrap();
        plan.paid(&s, a).unwrap();
        for id in [one, two] {
            let w = s.watch(id).unwrap().unwrap();
            assert_eq!((w.state.as_str(), w.until), ("running", None));
        }
        plan.stopped(&s, a).unwrap();
        assert_eq!(s.watch(one).unwrap().unwrap().state, "ended");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_free_trial_is_said_a_day_before_it_ends_and_then_ends() {
        let dir = std::env::temp_dir().join(format!("zetlyn-plan-{}", crate::crypto::token()));
        let (h, s, plan) = (hosted(&dir), Store::in_memory(), Plan { stripe: None });
        let a = s.account("a@example.com").unwrap();
        let id = s.add_watch_as(a, "https://example.com/", &Mode::Page { volatile: vec![] }, 60, None, "x", "pending").unwrap();
        start(&s, &h, &plan, a, id).unwrap();
        s.set_state(id, "running", Some(now() + 3600)).unwrap();
        // Not due for a check (just started), so the tick reads no page.
        let said = tick(&s, Some(&h), &plan).unwrap();
        assert!(said.iter().any(|l| l.contains("reminded")), "{said:?}");
        assert!(tick(&s, Some(&h), &plan).unwrap().is_empty(), "said once");
        s.set_state(id, "running", Some(now() - 1)).unwrap();
        s.set_reminded(id).unwrap();
        let said = tick(&s, Some(&h), &plan).unwrap();
        assert!(said.iter().any(|l| l.contains("ended")), "{said:?}");
        let mails: Vec<_> = std::fs::read_dir(dir.join("outbox")).unwrap().collect();
        assert_eq!(mails.len(), 2, "the reminder and the end");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_api_says_the_subscription() {
        let plan = Plan { stripe: None };
        let s = Store::in_memory();
        let id = s.account("a@example.com").unwrap();
        let a = s.account_by_id(id).unwrap().unwrap();
        let api = plan.api_account(&a, "https://zetlyn.test");
        assert_eq!((api["subscribed"].as_bool(), api["subscribe_url"].as_str()), (Some(false), Some("https://zetlyn.test/subscribe")));
    }

    #[test]
    fn a_subscription_starts_only_when_starting_at_once_was_asked_for() {
        let dir = std::env::temp_dir().join(format!("zetlyn-plan-{}", crate::crypto::token()));
        let store = Store::open(&dir).unwrap();
        let h = hosted(&dir);
        let plan = Plan { stripe: Some(Stripe::with_key("rk_test_x")) };
        let a = store.account_by_id(store.account("a@example.com").unwrap()).unwrap().unwrap();
        let no_header = |_: &str| String::new();
        let req = |body: &'static [u8]| Req { method: "POST", path: &["subscribe"], body, header: &no_header, account: Some(&a) };
        match plan.route(&store, &h, &req(b"")).unwrap().unwrap() {
            Out::Page { body, .. } => assert!(body.contains("Please tick the box"), "{body}"),
            _ => panic!("without the box ticked, no payment starts"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_subscribe_link_names_its_account_signs_nobody_in_and_still_asks_for_the_box() {
        let dir = std::env::temp_dir().join(format!("zetlyn-plan-{}", crate::crypto::token()));
        let (h, store) = (hosted(&dir), Store::in_memory());
        let plan = Plan { stripe: Some(Stripe::with_key("rk_test_x")) };
        let id = store.account("a@example.com").unwrap();
        let token = store.new_token_for(id, "subscribe", "", 3600).unwrap();
        let no_header = |_: &str| String::new();
        let path = ["subscribe", token.as_str()];
        let ask = |method: &'static str, body: &'static [u8]| match plan.route(&store, &h, &Req { method, path: &path, body, header: &no_header, account: None }).unwrap().unwrap() {
            Out::Page { body, .. } => body,
            _ => panic!("a page"),
        };
        let page = ask("GET", b"");
        assert!(page.contains("For the account a@example.com") && page.contains(&format!("action=/subscribe/{token}")), "{page}");
        assert!(ask("GET", b"").contains("For the account"), "opened again, it still works");
        assert!(ask("POST", b"").contains("Please tick the box"), "the consent is asked for as on /subscribe");
        let wrong = ["subscribe", "nope"];
        match plan.route(&store, &h, &Req { method: "GET", path: &wrong, body: b"", header: &no_header, account: None }).unwrap().unwrap() {
            Out::Page { body, .. } => assert!(body.contains("expired")),
            _ => panic!("a page"),
        }
        store.set_paid(id, true, "cus_1", "sub_1").unwrap();
        assert!(ask("GET", b"").contains("You subscribe already"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
