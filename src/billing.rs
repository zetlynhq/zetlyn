//! Paying: one subscription per account, a base price with up to `quota` watches in it and an
//! extra price for each watch beyond. (After the old Zetlyn's `stripe.rs` and `billing.rs`.)
//!
//! Configured by `STRIPE_API_KEY` (a restricted key), `STRIPE_PRICE_BASE`, `STRIPE_PRICE_EXTRA`
//! and `STRIPE_WEBHOOK_SECRET`. Without them an instance runs without payment: the trial ends and
//! nothing can be bought, which is what a local instance wants.

use serde_json::Value as J;

use crate::store::Store;

/// The prices, VAT included, in cents a month: the subscription, and each watch beyond its quota.
pub const BASE_CENTS: i64 = 999;
pub const EXTRA_CENTS: i64 = 50;

/// Stripe's tax code for the subscription: "Software as a service (SaaS) - personal use".
pub const TAX_CODE: &str = "txcd_10103000";

pub struct Stripe {
    key: String,
    pub base_price: String,
    pub extra_price: String,
    pub webhook_secret: String,
}

impl Stripe {
    pub fn from_env() -> Option<Stripe> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        Some(Stripe { key: var("STRIPE_API_KEY")?, base_price: var("STRIPE_PRICE_BASE")?, extra_price: var("STRIPE_PRICE_EXTRA")?, webhook_secret: var("STRIPE_WEBHOOK_SECRET").unwrap_or_default() })
    }

    fn agent() -> ureq::Agent {
        ureq::Agent::config_builder().user_agent(concat!("zetlyn/", env!("CARGO_PKG_VERSION"))).timeout_global(Some(std::time::Duration::from_secs(30))).http_status_as_error(false).build().into()
    }

    fn call(&self, method: &str, path: &str, params: &[(&str, String)]) -> Result<J, String> {
        let body: String = form_urlencoded::Serializer::new(String::new()).extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str()))).finish();
        let url = format!("https://api.stripe.com/v1/{path}");
        let auth = format!("Bearer {}", self.key);
        let response = match method {
            "GET" => Self::agent().get(&format!("{url}?{body}")).header("Authorization", &auth).call(),
            "DELETE" => Self::agent().delete(&url).header("Authorization", &auth).call(),
            _ => Self::agent().post(&url).header("Authorization", &auth).header("Content-Type", "application/x-www-form-urlencoded").send(body),
        };
        let mut r = response.map_err(|e| format!("Stripe {path}: {e}"))?;
        let status = r.status().as_u16();
        let text = r.body_mut().read_to_string().unwrap_or_default();
        let json: J = serde_json::from_str(&text).unwrap_or(J::Null);
        if status >= 400 {
            return Err(format!("Stripe {path}: {}", json["error"]["message"].as_str().unwrap_or("refused")));
        }
        Ok(json)
    }

    /// A key alone, for setting the account up: `zetlyn stripe-setup`.
    pub fn with_key(key: &str) -> Stripe {
        Stripe { key: key.to_string(), base_price: String::new(), extra_price: String::new(), webhook_secret: String::new() }
    }

    /// The product and its two prices, made where they are not yet and found where they are (by
    /// the product's metadata and the prices' lookup keys), so running it twice makes nothing
    /// twice. Prices include VAT. With `archive_others`, every other active product is archived:
    /// the old product's plans and meters. Returns the two price ids.
    pub fn setup(&self, archive_others: bool) -> Result<(String, String, Vec<String>), String> {
        let products = self.call("GET", "products", &[("active", "true".into()), ("limit", "100".into())])?;
        let list = products["data"].as_array().cloned().unwrap_or_default();
        let ours = list.iter().find(|p| p["metadata"]["zetlyn"].as_str() == Some("subscription")).and_then(|p| p["id"].as_str().map(str::to_string));
        let product = match ours {
            Some(id) => id,
            None => self
                .call("POST", "products", &[("name", "Zetlyn".into()), ("description", "Watches web pages and emails you when they change.".into()), ("metadata[zetlyn]", "subscription".into())])?["id"]
                .as_str()
                .ok_or("Stripe made no product")?
                .to_string(),
        };
        // Software as a service for personal use: Stripe Tax then taxes it as an electronically
        // supplied service, by where the buyer lives. Set again each time, so a product made
        // before has it too.
        self.call("POST", &format!("products/{product}"), &[("tax_code", TAX_CODE.into())])?;
        let mut archived = Vec::new();
        if archive_others {
            for p in list.iter().filter(|p| p["id"].as_str() != Some(product.as_str())) {
                if let Some(id) = p["id"].as_str() {
                    self.call("POST", &format!("products/{id}"), &[("active", "false".into())])?;
                    archived.push(format!("{id} ({})", p["name"].as_str().unwrap_or("")));
                }
            }
        }
        let price = |key: &str, cents: i64, nickname: &str| -> Result<String, String> {
            let found = self.call("GET", "prices", &[("lookup_keys[]", key.into()), ("active", "true".into())])?;
            if let Some(id) = found["data"][0]["id"].as_str() {
                return Ok(id.to_string());
            }
            let made = self.call(
                "POST",
                "prices",
                &[
                    ("product", product.clone()),
                    ("currency", "eur".into()),
                    ("unit_amount", cents.to_string()),
                    ("recurring[interval]", "month".into()),
                    ("tax_behavior", "inclusive".into()),
                    ("lookup_key", key.into()),
                    ("nickname", nickname.into()),
                ],
            )?;
            made["id"].as_str().map(str::to_string).ok_or_else(|| "Stripe made no price".into())
        };
        let base = price("zetlyn_monthly", BASE_CENTS, "Zetlyn, up to 10 watches")?;
        let extra = price("zetlyn_extra_watch_monthly", EXTRA_CENTS, "Zetlyn, one more watch")?;
        Ok((base, extra, archived))
    }

    /// Where to pay: a Checkout page for the subscription, for this account.
    /// A checkout for the subscription: where to send the person, and the session's id, kept so
    /// that the page they come back to can ask Stripe how it went (`session`).
    pub fn checkout(&self, account: i64, email: &str, base: &str, back: &str) -> Result<(String, String), String> {
        let session = self.call(
            "POST",
            "checkout/sessions",
            &[
                ("mode", "subscription".into()),
                ("line_items[0][price]", self.base_price.clone()),
                ("line_items[0][quantity]", "1".into()),
                ("client_reference_id", format!("account-{account}")),
                ("customer_email", email.into()),
                ("billing_address_collection", "required".into()),
                ("automatic_tax[enabled]", "true".into()),
                ("subscription_data[metadata][account]", account.to_string()),
                ("success_url", format!("{base}/account/billing")),
                ("cancel_url", format!("{base}{back}")),
            ],
        )?;
        let url = session["url"].as_str().ok_or("Stripe gave no checkout address")?;
        Ok((url.to_string(), session["id"].as_str().unwrap_or("").to_string()))
    }


    /// A checkout session as it stands: whether it was paid, and the customer and subscription it
    /// made. For the page a person comes back to, which may be there before Stripe's event is.
    pub fn session(&self, id: &str) -> Result<(bool, String, String), String> {
        let s = self.call("GET", &format!("checkout/sessions/{id}"), &[])?;
        let paid = s["status"] == "complete" && matches!(s["payment_status"].as_str(), Some("paid" | "no_payment_required"));
        Ok((paid, s["customer"].as_str().unwrap_or("").to_string(), s["subscription"].as_str().unwrap_or("").to_string()))
    }

    /// What a subscription is now, for the account's billing page.
    pub fn overview(&self, subscription: &str) -> Result<Overview, String> {
        let s = self.call("GET", &format!("subscriptions/{subscription}"), &[("expand[]", "default_payment_method".into())])?;
        let items = s["items"]["data"].as_array().cloned().unwrap_or_default();
        let monthly = items.iter().map(|i| i["price"]["unit_amount"].as_i64().unwrap_or(0) * i["quantity"].as_i64().unwrap_or(1)).sum();
        let card = &s["default_payment_method"]["card"];
        // Newer API versions keep the period on the items.
        let period_end = s["current_period_end"].as_i64().or_else(|| items.first().and_then(|i| i["current_period_end"].as_i64())).unwrap_or(0);
        Ok(Overview {
            status: s["status"].as_str().unwrap_or("").to_string(),
            created: s["created"].as_i64().unwrap_or(0),
            period_end,
            ending: s["cancel_at_period_end"].as_bool().unwrap_or(false),
            monthly,
            card: card["last4"].as_str().map(|l| format!("{} ···· {l}", capital(card["brand"].as_str().unwrap_or("card")))),
        })
    }

    /// The customer's invoices, newest first.
    pub fn invoices(&self, customer: &str) -> Result<Vec<Invoice>, String> {
        let list = self.call("GET", "invoices", &[("customer", customer.into()), ("limit", "24".into())])?;
        Ok(list["data"]
            .as_array()
            .map(|all| {
                all.iter()
                    .filter(|i| i["status"] != "draft")
                    .map(|i| Invoice {
                        number: i["number"].as_str().unwrap_or("").to_string(),
                        created: i["created"].as_i64().unwrap_or(0),
                        total: i["total"].as_i64().unwrap_or(0),
                        tax: i["total_taxes"].as_array().map(|t| t.iter().filter_map(|x| x["amount"].as_i64()).sum()).or_else(|| i["tax"].as_i64()).unwrap_or(0),
                        status: i["status"].as_str().unwrap_or("").to_string(),
                        pdf: i["invoice_pdf"].as_str().unwrap_or("").to_string(),
                        page: i["hosted_invoice_url"].as_str().unwrap_or("").to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }


    /// A subscription ended now, not at the end of its month: for an account its owner deletes.
    pub fn cancel_now(&self, subscription: &str) -> Result<(), String> {
        self.call("DELETE", &format!("subscriptions/{subscription}"), &[]).map(|_| ())
    }

    /// Where to change the card, see invoices or cancel.
    pub fn portal(&self, customer: &str, back: &str) -> Result<String, String> {
        let s = self.call("POST", "billing_portal/sessions", &[("customer", customer.into()), ("return_url", back.into())])?;
        s["url"].as_str().map(str::to_string).ok_or_else(|| "Stripe gave no portal address".into())
    }

    /// The extra price's quantity set to the watches beyond the quota: added, changed or removed.
    pub fn sync_extra(&self, subscription: &str, extra: i64) -> Result<(), String> {
        let sub = self.call("GET", &format!("subscriptions/{subscription}"), &[])?;
        let item = sub["items"]["data"].as_array().and_then(|items| items.iter().find(|i| i["price"]["id"].as_str() == Some(self.extra_price.as_str())).and_then(|i| i["id"].as_str().map(str::to_string)));
        match (item, extra) {
            (None, 0) => Ok(()),
            (None, n) => self.call("POST", "subscription_items", &[("subscription", subscription.into()), ("price", self.extra_price.clone()), ("quantity", n.to_string())]).map(|_| ()),
            (Some(id), 0) => self.call("DELETE", &format!("subscription_items/{id}"), &[]).map(|_| ()),
            (Some(id), n) => self.call("POST", &format!("subscription_items/{id}"), &[("quantity", n.to_string())]).map(|_| ()),
        }
    }

    /// What Stripe says of a subscription: its status (`active`, `canceled`, …) and the quantity of
    /// the extra price on it.
    pub fn subscription(&self, subscription: &str) -> Result<(String, i64), String> {
        let sub = self.call("GET", &format!("subscriptions/{subscription}"), &[])?;
        let extra = sub["items"]["data"].as_array().and_then(|items| items.iter().find(|i| i["price"]["id"].as_str() == Some(self.extra_price.as_str()))).and_then(|i| i["quantity"].as_i64()).unwrap_or(0);
        Ok((sub["status"].as_str().unwrap_or("?").to_string(), extra))
    }

    /// Where Stripe's dashboard shows what this key sees: the test data for a test key.
    pub fn dashboard(&self) -> &'static str {
        if self.key.contains("_test_") { "https://dashboard.stripe.com/test/" } else { "https://dashboard.stripe.com/" }
    }
}


/// A subscription as the billing page shows it.
pub struct Overview {
    pub status: String,
    pub created: i64,
    /// When the month paid for ends: the next payment, or the end where it was cancelled.
    pub period_end: i64,
    pub ending: bool,
    /// Cents a month, VAT included.
    pub monthly: i64,
    pub card: Option<String>,
}

pub struct Invoice {
    pub number: String,
    pub created: i64,
    pub total: i64,
    pub tax: i64,
    pub status: String,
    pub pdf: String,
    pub page: String,
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// The event, where its signature holds and it is fresh: `Stripe-Signature: t=<unix>,v1=<hex>`,
/// signed over `<t>.<body>`; older than five minutes is refused, so it cannot be replayed.
pub fn verified(body: &[u8], signature: &str, secret: &str, at: i64) -> Result<J, String> {
    let mut t = "";
    let mut v1: Vec<&str> = Vec::new();
    for part in signature.split(',') {
        match part.trim().split_once('=') {
            Some(("t", v)) => t = v,
            Some(("v1", v)) => v1.push(v),
            _ => {}
        }
    }
    let when: i64 = t.parse().map_err(|_| "no timestamp in the signature")?;
    if (at - when).abs() > 300 {
        return Err("the event is more than five minutes old".into());
    }
    let mut signed = t.as_bytes().to_vec();
    signed.push(b'.');
    signed.extend_from_slice(body);
    let want = crate::crypto::hex(&crate::crypto::hmac_sha256(secret.as_bytes(), &signed));
    if !v1.iter().any(|g| crate::crypto::same(g, &want)) {
        return Err("the signature does not verify".into());
    }
    serde_json::from_slice(body).map_err(|e| format!("not JSON: {e}"))
}

/// What happened, for the lifecycle to act on.
#[derive(Debug, PartialEq)]
pub enum Paid {
    Started(i64),
    Stopped(i64),
    Nothing,
}

/// What an event says about an account: a checkout completed starts a subscription; a
/// subscription deleted, or no longer active, stops it.
pub fn apply(store: &Store, event: &J) -> Result<Paid, String> {
    let id = event["id"].as_str().unwrap_or("");
    if !id.is_empty() && !store.first_event(id, event["type"].as_str().unwrap_or(""))? {
        return Ok(Paid::Nothing);
    }
    let o = &event["data"]["object"];
    match event["type"].as_str().unwrap_or("") {
        "checkout.session.completed" => {
            let account: i64 = o["client_reference_id"].as_str().and_then(|r| r.strip_prefix("account-")).and_then(|n| n.parse().ok()).ok_or("the checkout names no account")?;
            store.set_paid(account, true, o["customer"].as_str().unwrap_or(""), o["subscription"].as_str().unwrap_or(""))?;
            Ok(Paid::Started(account))
        }
        "customer.subscription.deleted" | "customer.subscription.updated" => {
            let Some(a) = store.account_by_customer(o["customer"].as_str().unwrap_or(""))? else { return Ok(Paid::Nothing) };
            let active = event["type"] == "customer.subscription.updated" && matches!(o["status"].as_str(), Some("active" | "trialing" | "past_due"));
            if active == a.paid || a.free {
                return Ok(Paid::Nothing);
            }
            store.set_paid(a.id, active, "", "")?;
            Ok(if active { Paid::Started(a.id) } else { Paid::Stopped(a.id) })
        }
        _ => Ok(Paid::Nothing),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signature_is_checked_and_a_checkout_starts_a_subscription() {
        let body = br#"{"id":"evt_1","type":"checkout.session.completed","data":{"object":{"client_reference_id":"account-1","customer":"cus_1","subscription":"sub_1"}}}"#;
        let t = 1_800_000_000;
        let mut signed = t.to_string().into_bytes();
        signed.push(b'.');
        signed.extend_from_slice(body);
        let sig = format!("t={t},v1={}", crate::crypto::hex(&crate::crypto::hmac_sha256(b"whsec_x", &signed)));
        assert!(verified(body, &sig, "whsec_y", t).is_err(), "another secret");
        assert!(verified(body, &sig, "whsec_x", t + 301).is_err(), "too old");
        let event = verified(body, &sig, "whsec_x", t + 10).unwrap();
        let store = Store::open(&std::env::temp_dir().join(format!("zetlyn-billing-{}", crate::crypto::token()))).unwrap();
        let acc = store.account("a@example.com").unwrap();
        assert_eq!(acc, 1);
        assert_eq!(apply(&store, &event).unwrap(), Paid::Started(1));
        assert_eq!(apply(&store, &event).unwrap(), Paid::Nothing, "the same event twice");
        let a = store.account_by_id(1).unwrap().unwrap();
        assert!(a.paid && a.stripe_customer == "cus_1" && a.stripe_subscription == "sub_1");
        let gone = serde_json::json!({"id":"evt_2","type":"customer.subscription.deleted","data":{"object":{"customer":"cus_1"}}});
        assert_eq!(apply(&store, &gone).unwrap(), Paid::Stopped(1));
        assert!(!store.account_by_id(1).unwrap().unwrap().paid);
    }
}
