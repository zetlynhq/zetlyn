//! Stripe's API, for the one product the main server sells (CELLS.md, billing.rs): its meters and
//! its usage prices made once, a checkout made for each order, and each cell's usage reported.
//! The key is `STRIPE_API_KEY`, a restricted one, in /etc/zetlyn/stripe.env.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value as J;

/// What usage is billed as: the meter's event name, the product it is shown as, and its price
/// beyond what the plan includes, in euro cents per unit (decimals allowed).
pub struct Metered {
    pub key: &'static str,
    pub event: &'static str,
    pub product: &'static str,
    pub included: u64,
    pub cents: &'static str,
}

/// Storage in megabyte-days: 2 GB for a 30-day month included, €0.50 a GB-month after.
/// Source reads: 25,000 included, €1 per 10,000 after. Mails: 1,000 included, €1 per 1,000 after.
pub const METERED: [Metered; 3] = [
    Metered { key: "storage", event: "zetlyn_storage_mb_days", product: "Zetlyn Storage", included: 2 * 1024 * 30, cents: "0.001627604167" },
    Metered { key: "reads", event: "zetlyn_source_reads", product: "Zetlyn Source Reads", included: 25_000, cents: "0.01" },
    Metered { key: "mails", event: "zetlyn_mails", product: "Zetlyn Mails", included: 1_000, cents: "0.1" },
];

/// What the setup made, kept beside plans.yaml: the base price and, per metered kind, its meter
/// and its price.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct Ids {
    pub base_price: String,
    #[serde(default)]
    pub meters: BTreeMap<String, String>,
    #[serde(default)]
    pub prices: BTreeMap<String, String>,
}

pub fn ids(billing: &Path) -> Option<Ids> {
    std::fs::read(billing.join("stripe.json")).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

fn key() -> Result<String, String> {
    if std::env::var("STRIPE_API_KEY").is_err() {
        let text = std::fs::read_to_string("/etc/zetlyn/stripe.env").unwrap_or_default();
        for (k, v) in text.lines().filter_map(|l| l.trim().split_once('=')) {
            if k.trim() == "STRIPE_API_KEY" {
                std::env::set_var("STRIPE_API_KEY", v.trim().trim_matches('"'));
            }
        }
    }
    std::env::var("STRIPE_API_KEY").map_err(|_| "STRIPE_API_KEY is not set".to_string())
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(concat!("zetlyn/", env!("CARGO_PKG_VERSION")))
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .into()
}

fn form(params: &[(String, String)]) -> String {
    params.iter().map(|(k, v)| format!("{}={}", crate::serve::urlencode(k), crate::serve::urlencode(v))).collect::<Vec<_>>().join("&")
}

fn answer(mut r: ureq::http::Response<ureq::Body>, what: &str) -> Result<J, String> {
    let status = r.status().as_u16();
    let body: J = r.body_mut().read_json().unwrap_or(J::Null);
    if status >= 400 {
        return Err(format!("Stripe {what}: {}", body["error"]["message"].as_str().unwrap_or("refused")));
    }
    Ok(body)
}

pub fn post(path: &str, params: &[(String, String)], idempotency: Option<&str>) -> Result<J, String> {
    let mut req = agent()
        .post(&format!("https://api.stripe.com/v1/{path}"))
        .header("Authorization", &format!("Bearer {}", key()?))
        .header("Content-Type", "application/x-www-form-urlencoded");
    if let Some(i) = idempotency {
        req = req.header("Idempotency-Key", i);
    }
    answer(req.send(form(params)).map_err(|e| format!("Stripe {path}: {e}"))?, path)
}

pub fn get(path: &str, params: &[(String, String)]) -> Result<J, String> {
    let q = form(params);
    let url = if q.is_empty() { format!("https://api.stripe.com/v1/{path}") } else { format!("https://api.stripe.com/v1/{path}?{q}") };
    answer(agent().get(&url).header("Authorization", &format!("Bearer {}", key()?)).call().map_err(|e| format!("Stripe {path}: {e}"))?, path)
}

fn p(k: &str, v: &str) -> (String, String) {
    (k.to_string(), v.to_string())
}

/// The meters and the usage prices, made where they are not yet, beside the product's own
/// monthly price, and written to billing/stripe.json. Run again, it finds what it made.
pub fn setup(billing: &Path, product: &str) -> Result<Ids, String> {
    let mut ids = ids(billing).unwrap_or_default();
    let prices = get("prices", &[p("product", product), p("active", "true"), p("type", "recurring")])?;
    let base = prices["data"].as_array().into_iter().flatten().find(|x| x["recurring"]["usage_type"] == "licensed" && x["recurring"]["interval"] == "month");
    ids.base_price = base.and_then(|b| b["id"].as_str()).ok_or("the product has no monthly price")?.to_string();
    let meters = get("billing/meters", &[p("status", "active"), p("limit", "100")])?;
    let products = get("products", &[p("active", "true"), p("limit", "100")])?;
    for m in &METERED {
        let meter = match meters["data"].as_array().into_iter().flatten().find(|x| x["event_name"] == m.event).and_then(|x| x["id"].as_str()) {
            Some(id) => id.to_string(),
            None => post(
                "billing/meters",
                &[
                    p("display_name", m.product),
                    p("event_name", m.event),
                    p("default_aggregation[formula]", "sum"),
                    p("customer_mapping[type]", "by_id"),
                    p("customer_mapping[event_payload_key]", "stripe_customer_id"),
                    p("value_settings[event_payload_key]", "value"),
                ],
                Some(&format!("zetlyn-meter-{}", m.event)),
            )?["id"]
                .as_str()
                .ok_or("no meter id")?
                .to_string(),
        };
        ids.meters.insert(m.key.to_string(), meter.clone());
        if ids.prices.get(m.key).is_some_and(|x| !x.is_empty()) {
            continue;
        }
        // The product it is shown as: found by the mark it was made with, or by its name in any
        // case, and given the name it is meant to have; made where there is none.
        let pid = match products["data"].as_array().into_iter().flatten().find(|x| x["metadata"]["zetlyn"] == m.event || x["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(m.product))).and_then(|x| x["id"].as_str()) {
            Some(id) => {
                post(&format!("products/{id}"), &[p("name", m.product), p("metadata[zetlyn]", m.event)], None)?;
                id.to_string()
            }
            None => post("products", &[p("name", m.product), p("tax_code", "txcd_10103001"), p("metadata[zetlyn]", m.event)], Some(&format!("zetlyn-product-v2-{}", m.event)))?["id"]
                .as_str()
                .ok_or("no product id")?
                .to_string(),
        };
        let included = m.included.to_string();
        let price = post(
            "prices",
            &[
                p("product", &pid),
                p("currency", "eur"),
                p("recurring[interval]", "month"),
                p("recurring[usage_type]", "metered"),
                p("recurring[meter]", &meter),
                p("billing_scheme", "tiered"),
                p("tiers_mode", "graduated"),
                p("tiers[0][up_to]", &included),
                p("tiers[0][unit_amount]", "0"),
                p("tiers[1][up_to]", "inf"),
                p("tiers[1][unit_amount_decimal]", m.cents),
                p("tax_behavior", "exclusive"),
                p("nickname", m.product),
            ],
            Some(&format!("zetlyn-price-{}-{}", m.event, pid)),
        )?;
        ids.prices.insert(m.key.to_string(), price["id"].as_str().ok_or("no price id")?.to_string());
    }
    std::fs::write(billing.join("stripe.json"), serde_json::to_vec_pretty(&ids).unwrap_or_default()).map_err(|e| e.to_string())?;
    Ok(ids)
}

/// The first of next month at midnight UTC, in seconds: where every subscription's months begin,
/// so that a month here is the month on the invoice.
fn next_month_start() -> i64 {
    let today = crate::iso_date(crate::now());
    let (y, m): (i64, i64) = (today[..4].parse().unwrap_or(2026), today[5..7].parse().unwrap_or(1));
    let (y, m) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    crate::thingstore::days(&format!("{y:04}-{m:02}-01")).unwrap_or(0) * 86_400
}

/// A checkout for a world: the monthly price and the three usage prices, a business's tax ID and
/// address, tax worked out by Stripe. Where to send the buyer.
pub fn checkout(billing: &Path, world: &str, email: &str, base: &str) -> Result<String, String> {
    let ids = ids(billing).ok_or("Stripe is not set up here: `zetlyn billing stripe-setup`")?;
    let mut params = vec![
        p("mode", "subscription"),
        p("line_items[0][price]", &ids.base_price),
        p("line_items[0][quantity]", "1"),
        p("client_reference_id", world),
        p("customer_email", email),
        p("tax_id_collection[enabled]", "true"),
        p("billing_address_collection", "required"),
        p("automatic_tax[enabled]", "true"),
        p("allow_promotion_codes", "false"),
        p("metadata[plan]", "managed"),
        p("subscription_data[metadata][world]", world),
        p("subscription_data[billing_cycle_anchor]", &next_month_start().to_string()),
        p("subscription_data[proration_behavior]", "create_prorations"),
        p("success_url", &format!("{base}/account/welcome?session={{CHECKOUT_SESSION_ID}}")),
        p("cancel_url", &format!("{base}/account/new")),
    ];
    for (i, m) in METERED.iter().enumerate() {
        let price = ids.prices.get(m.key).ok_or_else(|| format!("no price for {}", m.key))?;
        params.push(p(&format!("line_items[{}][price]", i + 1), price));
    }
    let session = post("checkout/sessions", &params, None)?;
    session["url"].as_str().map(str::to_string).ok_or_else(|| "Stripe gave no checkout address".to_string())
}

/// So much usage of a kind for a customer, said once whatever is retried: `identifier` is the
/// same for the same hour's report.
pub fn report(event: &str, customer: &str, value: u64, identifier: &str, at: i64) -> Result<(), String> {
    if value == 0 {
        return Ok(());
    }
    post(
        "billing/meter_events",
        &[
            p("event_name", event),
            p("payload[stripe_customer_id]", customer),
            p("payload[value]", &value.to_string()),
            p("identifier", identifier),
            p("timestamp", &at.to_string()),
        ],
        Some(identifier),
    )
    .map(|_| ())
}

/// A checkout session as Stripe has it now, made into the event a webhook would have carried, so
/// that a world is made whether or not the webhook arrived.
pub fn session_event(id: &str) -> Result<J, String> {
    let s = get(&format!("checkout/sessions/{id}"), &[])?;
    Ok(serde_json::json!({ "id": format!("pull-{id}"), "type": "checkout.session.completed", "data": { "object": s } }))
}

/// Every checkout completed in the last two days, as such events.
pub fn recent_sessions() -> Result<Vec<J>, String> {
    let since = (crate::now() - 2 * 86_400).to_string();
    let list = get("checkout/sessions", &[p("status", "complete"), p("limit", "100"), p("created[gte]", &since)])?;
    Ok(list["data"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| serde_json::json!({ "id": format!("pull-{}", s["id"].as_str().unwrap_or("")), "type": "checkout.session.completed", "data": { "object": s } }))
        .collect())
}


/// A customer's newest subscription, as the event its state would have been.
pub fn customer_subscription_event(customer: &str) -> Result<Option<J>, String> {
    let list = get("subscriptions", &[p("customer", customer), p("status", "all"), p("limit", "1")])?;
    let Some(s) = list["data"].as_array().and_then(|a| a.first()).cloned() else { return Ok(None) };
    let kind = if s["status"] == "canceled" { "customer.subscription.deleted" } else { "customer.subscription.updated" };
    let hour = crate::iso_stamp(crate::now()).get(..13).unwrap_or("").to_string();
    Ok(Some(serde_json::json!({ "id": format!("pull-{}-{hour}", s["id"].as_str().unwrap_or("")), "type": kind, "data": { "object": s } })))
}
