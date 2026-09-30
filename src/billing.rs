//! Who has a hosted workspace, on which plan, paid until when.
//!
//! One directory, on the machine that hosts: `plans.yaml`, which the operator writes, and
//! `customers.db`, which Stripe's webhook and the operator move. A hosted workspace reads its own
//! row and nothing else: whether it is paid for, and what its plan allows. It holds no card and
//! sees no payment; Stripe does, and says what happened in a signed event.
//!
//! A plan is per workspace (D16), in tiers that differ in what costs: how many sources are
//! updated, how often at most, how much mail is sent.

use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    #[serde(default)]
    pub title: String,
    /// Sources updated. Beyond it, a source is held but not asked again.
    pub sources: usize,
    /// The shortest cadence, `1h`, `15m`.
    pub every: String,
    /// Mail sent by the workspace in a month.
    pub mails: u64,
    /// What it costs, as the website shows it: `19`.
    #[serde(default)]
    pub price: String,
    /// Stripe's Payment Link for it. The workspace's name goes with it as `client_reference_id`.
    #[serde(default)]
    pub link: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Plans {
    #[serde(default)]
    currency: String,
    plans: BTreeMap<String, Plan>,
}

pub fn plans(dir: &Path) -> Result<(String, BTreeMap<String, Plan>), String> {
    let p: Plans = crate::yaml::read(&dir.join("plans.yaml"))?;
    Ok((p.currency, p.plans))
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Customer {
    pub name: String,
    pub email: String,
    pub plan: String,
    /// `active`, `past_due`, `cancelled`.
    pub state: String,
    pub paid_until: Option<String>,
    pub stripe_customer: Option<String>,
}

impl Customer {
    /// Paid for today: active, or cancelled and not yet past what was paid for.
    pub fn in_good_standing(&self) -> bool {
        let today = crate::iso_date(crate::now());
        let paid = self.paid_until.as_deref().map_or(true, |u| u >= today.as_str());
        matches!(self.state.as_str(), "active" | "cancelled") && paid
    }
}

const SCHEMA: &str = "
create table if not exists customer(
  name text primary key, email text not null, plan text not null, state text not null,
  paid_until text, stripe_customer text, stripe_subscription text, updated text not null);
create table if not exists event(id text primary key, kind text not null, at text not null);
";

pub struct Book {
    db: Connection,
}

impl Book {
    pub fn open(dir: &Path) -> Result<Book, String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let db = Connection::open(dir.join("customers.db")).map_err(|e| e.to_string())?;
        db.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        Ok(Book { db })
    }

    /// For a hosted workspace, which reads its plan and may not write it.
    pub fn read(dir: &Path) -> Result<Book, String> {
        let db = Connection::open_with_flags(dir.join("customers.db"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| format!("{}: {e}", dir.join("customers.db").display()))?;
        Ok(Book { db })
    }

    pub fn get(&self, name: &str) -> Option<Customer> {
        self.db
            .query_row(
                "select name, email, plan, state, paid_until, stripe_customer from customer where name = ?1",
                [name],
                |r| Ok(Customer { name: r.get(0)?, email: r.get(1)?, plan: r.get(2)?, state: r.get(3)?, paid_until: r.get(4)?, stripe_customer: r.get(5)? }),
            )
            .ok()
    }

    pub fn all(&self) -> Vec<Customer> {
        let Ok(mut stmt) = self.db.prepare("select name, email, plan, state, paid_until, stripe_customer from customer order by name") else {
            return Vec::new();
        };
        stmt.query_map([], |r| Ok(Customer { name: r.get(0)?, email: r.get(1)?, plan: r.get(2)?, state: r.get(3)?, paid_until: r.get(4)?, stripe_customer: r.get(5)? }))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    pub fn set(&self, c: &Customer, subscription: Option<&str>) -> Result<(), String> {
        self.db
            .execute(
                "insert into customer(name, email, plan, state, paid_until, stripe_customer, stripe_subscription, updated)
                 values(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 on conflict(name) do update set email = excluded.email, plan = excluded.plan, state = excluded.state,
                   paid_until = excluded.paid_until,
                   stripe_customer = coalesce(excluded.stripe_customer, customer.stripe_customer),
                   stripe_subscription = coalesce(excluded.stripe_subscription, customer.stripe_subscription),
                   updated = excluded.updated",
                rusqlite::params![c.name, c.email, c.plan, c.state, c.paid_until, c.stripe_customer, subscription, crate::iso_stamp(crate::now())],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn by_stripe(&self, customer: &str) -> Option<Customer> {
        let name: String = self.db.query_row("select name from customer where stripe_customer = ?1", [customer], |r| r.get(0)).ok()?;
        self.get(&name)
    }

    /// Whether this event was taken already. Stripe sends an event again until it is answered
    /// 2xx, and a second delivery of the same one is not a second payment.
    fn first_time(&self, id: &str, kind: &str) -> Result<bool, String> {
        let n = self
            .db
            .execute("insert or ignore into event(id, kind, at) values(?1, ?2, ?3)", rusqlite::params![id, kind, crate::iso_stamp(crate::now())])
            .map_err(|e| e.to_string())?;
        Ok(n == 1)
    }
}

/// What a hosted workspace may do today, and whether it may do anything.
pub fn limits(dir: &Path, name: &str) -> (bool, crate::Limits, u64) {
    let Ok(book) = Book::read(dir) else { return (false, crate::Limits::default(), 0) };
    let Some(c) = book.get(name) else { return (false, crate::Limits::default(), 0) };
    let plan = plans(dir).ok().and_then(|(_, p)| p.get(&c.plan).cloned()).unwrap_or_default();
    let limits = crate::Limits { sources: Some(plan.sources), every: crate::fetch::duration(&plan.every).unwrap_or(0) };
    (c.in_good_standing(), limits, plan.mails)
}

/// A Stripe event, checked and applied. `Stripe-Signature: t=<unix>,v1=<hex>`, signed over
/// `<t>.<body>` with the endpoint's secret; older than five minutes is refused, as Stripe's own
/// libraries refuse it, so a captured event cannot be replayed later.
pub fn stripe(dir: &Path, body: &[u8], signature: &str, secret: &str) -> Result<String, String> {
    let mut t = "";
    let mut v1: Vec<&str> = Vec::new();
    for part in signature.split(',') {
        match part.split_once('=') {
            Some(("t", v)) => t = v,
            Some(("v1", v)) => v1.push(v),
            _ => {}
        }
    }
    let at: i64 = t.parse().map_err(|_| "no timestamp in the signature")?;
    if (crate::now() - at).abs() > 300 {
        return Err("the event is more than five minutes old".into());
    }
    let mut signed = t.as_bytes().to_vec();
    signed.push(b'.');
    signed.extend_from_slice(body);
    let want: String = crate::place::hmac_sha256(secret.as_bytes(), &signed).iter().map(|b| format!("{b:02x}")).collect();
    if !v1.iter().any(|g| crate::place::same(g, &want)) {
        return Err("the signature does not verify".into());
    }
    let event: J = serde_json::from_slice(body).map_err(|e| format!("not JSON: {e}"))?;
    apply(dir, &event)
}

/// What an event moves. Checkout completed makes or renews a workspace's row; a subscription's
/// changes and invoices move its state and the date it is paid to.
pub fn apply(dir: &Path, event: &J) -> Result<String, String> {
    let book = Book::open(dir)?;
    let id = event["id"].as_str().unwrap_or("");
    let kind = event["type"].as_str().unwrap_or("");
    if !id.is_empty() && !book.first_time(id, kind)? {
        return Ok(format!("{id}: taken already"));
    }
    let o = &event["data"]["object"];
    let date = |secs: &J| secs.as_i64().map(crate::iso_date);
    match kind {
        "checkout.session.completed" => {
            let name = o["client_reference_id"].as_str().filter(|s| !s.is_empty()).ok_or("the checkout names no workspace (client_reference_id)")?;
            let email = o["customer_details"]["email"].as_str().or(o["customer_email"].as_str()).unwrap_or("").to_string();
            let plan = o["metadata"]["plan"].as_str().unwrap_or("").to_string();
            let known = plans(dir).map(|(_, p)| p.contains_key(&plan)).unwrap_or(false);
            if !known {
                return Err(format!("{plan}: no such plan in plans.yaml"));
            }
            let c = Customer {
                name: name.to_string(),
                email,
                plan,
                state: "active".into(),
                // Until the first invoice says the period; a month is what was bought.
                paid_until: Some(crate::iso_date(crate::now() + 31 * 86_400)),
                stripe_customer: o["customer"].as_str().map(str::to_string),
            };
            book.set(&c, o["subscription"].as_str())?;
            Ok(format!("{name}: active on {}", c.plan))
        }
        "customer.subscription.updated" | "customer.subscription.deleted" => {
            let customer = o["customer"].as_str().unwrap_or("");
            let mut c = book.by_stripe(customer).ok_or_else(|| format!("{customer}: no workspace for this customer"))?;
            c.state = match (kind, o["status"].as_str().unwrap_or("")) {
                ("customer.subscription.deleted", _) | (_, "canceled") => "cancelled",
                (_, "active" | "trialing") => "active",
                (_, "past_due" | "unpaid" | "incomplete") => "past_due",
                (_, other) => other,
            }
            .to_string();
            if let Some(end) = date(&o["current_period_end"]) {
                c.paid_until = Some(end);
            }
            book.set(&c, o["id"].as_str())?;
            Ok(format!("{}: {}", c.name, c.state))
        }
        "invoice.paid" => {
            let customer = o["customer"].as_str().unwrap_or("");
            let mut c = book.by_stripe(customer).ok_or_else(|| format!("{customer}: no workspace for this customer"))?;
            if let Some(end) = date(&o["lines"]["data"][0]["period"]["end"]) {
                c.paid_until = Some(end);
            }
            c.state = "active".into();
            book.set(&c, None)?;
            Ok(format!("{}: paid until {}", c.name, c.paid_until.clone().unwrap_or_default()))
        }
        other => Ok(format!("{other}: nothing to do")),
    }
}

/// `zetlyn billing`.
pub fn command(args: &[String]) -> Result<(), String> {
    let dir = crate::positional(args, 2).first().map(|s| std::path::PathBuf::from(s.as_str())).ok_or("which billing directory?")?;
    match args.get(1).map(String::as_str) {
        Some("list") => {
            let book = Book::open(&dir)?;
            for c in book.all() {
                println!("{}  {}  {}  {}  paid until {}", c.name, c.email, c.plan, c.state, c.paid_until.unwrap_or_else(|| "—".into()));
            }
            Ok(())
        }
        // The first customers arrive before any payment does: granted by hand, for so many days.
        Some("grant") => {
            let name = crate::positional(args, 2).get(1).map(|s| s.to_string()).ok_or("which workspace?")?;
            let email = crate::flag(args, "--email").ok_or("--email of whoever owns it")?.to_string();
            let plan = crate::flag(args, "--plan").ok_or("--plan, from plans.yaml")?.to_string();
            let days: i64 = crate::flag(args, "--days").and_then(|d| d.parse().ok()).unwrap_or(31);
            if !plans(&dir)?.1.contains_key(&plan) {
                return Err(format!("{plan}: no such plan in plans.yaml"));
            }
            let c = Customer { name, email, plan, state: "active".into(), paid_until: Some(crate::iso_date(crate::now() + days * 86_400)), stripe_customer: None };
            Book::open(&dir)?.set(&c, None)?;
            println!("{}: {} until {}", c.name, c.plan, c.paid_until.unwrap_or_default());
            Ok(())
        }
        // Stripe's webhook, and nothing else, on its own port behind the web server.
        Some("serve") => {
            let addr = crate::flag(args, "--addr").unwrap_or("127.0.0.1:2195").to_string();
            let secret = std::env::var("STRIPE_WEBHOOK_SECRET").map_err(|_| "STRIPE_WEBHOOK_SECRET is not set")?;
            let server = tiny_http::Server::http(&addr).map_err(|e| e.to_string())?;
            println!("billing for {} on http://{addr}", dir.display());
            for mut request in server.incoming_requests() {
                let signature = request
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Stripe-Signature"))
                    .map(|h| h.value.as_str().to_string())
                    .unwrap_or_default();
                let mut body = Vec::new();
                let _ = std::io::Read::read_to_end(request.as_reader(), &mut body);
                let post = request.method() == &tiny_http::Method::Post;
                let (status, said) = if !post {
                    (405, json!({ "error": "POST a Stripe event" }))
                } else {
                    match stripe(&dir, &body, &signature, &secret) {
                        Ok(s) => (200, json!({ "ok": s })),
                        // A refused signature is 400 and Stripe gives up; anything else is 500
                        // and Stripe sends it again, which is what a transient failure wants.
                        Err(e) if e.contains("signature") || e.contains("five minutes") || e.contains("timestamp") => (400, json!({ "error": e })),
                        Err(e) => (500, json!({ "error": e })),
                    }
                };
                eprintln!("{status} {said}");
                let _ = request.respond(tiny_http::Response::from_string(said.to_string()).with_status_code(status));
            }
            Ok(())
        }
        _ => Err("billing list | grant <dir> <name> --email --plan [--days] | serve <dir> [--addr]".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("zetlyn-billing-{}-{}", std::process::id(), crate::now()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("plans.yaml"), "currency: €\nplans:\n  solo:\n    sources: 10\n    every: 1h\n    mails: 500\n    price: \"19\"\n").unwrap();
        d
    }

    fn signed(body: &str, secret: &str, at: i64) -> String {
        let mac = crate::place::hmac_sha256(secret.as_bytes(), format!("{at}.{body}").as_bytes());
        format!("t={at},v1={}", mac.iter().map(|b| format!("{b:02x}")).collect::<String>())
    }

    #[test]
    fn a_checkout_makes_a_workspace_paid_and_its_changes_move_it() {
        let d = dir();
        let checkout = json!({ "id": "evt_1", "type": "checkout.session.completed", "data": { "object": {
            "client_reference_id": "acme", "customer": "cus_1", "subscription": "sub_1",
            "customer_details": { "email": "owner@acme.test" }, "metadata": { "plan": "solo" } } } }).to_string();
        // Refused unsigned, wrongly signed, and signed too long ago.
        assert!(stripe(&d, checkout.as_bytes(), "t=1,v1=00", "whsec").is_err());
        assert!(stripe(&d, checkout.as_bytes(), &signed(&checkout, "other", crate::now()), "whsec").unwrap_err().contains("does not verify"));
        assert!(stripe(&d, checkout.as_bytes(), &signed(&checkout, "whsec", crate::now() - 600), "whsec").unwrap_err().contains("five minutes"));
        assert!(stripe(&d, checkout.as_bytes(), &signed(&checkout, "whsec", crate::now()), "whsec").unwrap().contains("acme: active on solo"));
        // The same event again is not a second payment.
        assert!(stripe(&d, checkout.as_bytes(), &signed(&checkout, "whsec", crate::now()), "whsec").unwrap().contains("taken already"));
        let (ok, limits, mails) = limits(&d, "acme");
        assert!(ok && limits.sources == Some(10) && limits.every == 3600 && mails == 500);

        let gone = json!({ "id": "evt_2", "type": "customer.subscription.deleted", "data": { "object": {
            "id": "sub_1", "customer": "cus_1", "status": "canceled", "current_period_end": crate::now() - 86_400 } } });
        apply(&d, &gone).unwrap();
        let c = Book::open(&d).unwrap().get("acme").unwrap();
        assert_eq!(c.state, "cancelled");
        assert!(!c.in_good_standing(), "cancelled and past what was paid for");
        let _ = std::fs::remove_dir_all(&d);
    }
}
