//! Telling a change somewhere besides the account's mail: a Telegram chat, a Slack or Discord
//! channel by its incoming webhook, or a webhook of the account's own.
//!
//! A webhook of one's own gets the change as JSON, signed so its receiver knows it came from
//! Zetlyn: `Zetlyn-Signature: t=<unix time>,v1=<hex HMAC-SHA256 of "<t>.<body>" with the
//! channel's secret>`. Slack, Discord and Telegram get a short text, with the page's words kept
//! from pinging anyone or being read as markup.
//!
//! Deliveries leave the round at once, each in a thread of its own, tried twice a minute apart;
//! how each went is kept for the account's page (`record`).

use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value as J};

use crate::store::{now, Channel, Store, Watch};

/// What happened, as every channel is told it.
pub struct Event {
    /// `change`, `unclear` (the place cannot be found any more) or `test`.
    pub kind: &'static str,
    pub watch: String,
    pub url: String,
    pub what: String,
    pub before: String,
    pub after: String,
    /// The condition that came true, said: "Now below 400".
    pub reason: Option<String>,
    pub notify_when: Option<J>,
    pub at: i64,
    /// The watch's page, or where to mark it again.
    pub link: String,
}

impl Event {
    pub fn change(w: &Watch, before: &str, after: &str, base: &str) -> Event {
        Event {
            kind: "change",
            watch: w.key.clone(),
            url: w.url.clone(),
            what: crate::life::what(w),
            before: before.into(),
            after: after.into(),
            reason: w.condition.as_ref().map(|k| k.now_said()),
            notify_when: w.condition.as_ref().map(|k| k.json()),
            at: now(),
            link: format!("{base}/watches/{}", w.key),
        }
    }

    pub fn unclear(w: &Watch, base: &str) -> Event {
        Event { kind: "unclear", before: String::new(), after: String::new(), reason: None, link: format!("{base}/mark?watch={}", w.key), ..Event::change(w, "", "", base) }
    }

    pub fn test(base: &str) -> Event {
        Event {
            kind: "test",
            watch: "test".into(),
            url: "https://example.com/pricing".into(),
            what: "The place beside “Pro”".into(),
            before: "499 €".into(),
            after: "399 €".into(),
            reason: None,
            notify_when: None,
            at: now(),
            link: format!("{base}/account/notifications"),
        }
    }

    /// For a webhook of one's own.
    pub fn json(&self) -> J {
        json!({
            "event": self.kind,
            "at": crate::api::iso(self.at),
            "watch": { "id": self.watch, "url": self.url, "what": self.what, "notify_when": self.notify_when },
            "before": self.before,
            "after": self.after,
            "condition": self.reason,
            "link": self.link,
        })
    }

    /// For a person reading a chat: a head line, what went and came, the link.
    pub fn text(&self) -> String {
        let host = crate::mail::short(&self.url);
        match self.kind {
            "unclear" => format!("Zetlyn cannot find the place you marked on {host} any more: the page was rebuilt. Mark it again: {}", self.link),
            _ => {
                let head = format!("{}{}: {host} · {}", if self.kind == "test" { "Test · " } else { "" }, self.reason.as_deref().unwrap_or("Changed"), crate::mail::clip(&self.what, 50));
                let body = if self.before.lines().count() <= 1 && self.after.lines().count() <= 1 {
                    format!("before: {}\nnow: {}", crate::mail::clip(&self.before, 200), crate::mail::clip(&self.after, 200))
                } else {
                    let (gone, came) = crate::text::diff(&self.before, &self.after);
                    gone.iter().take(5).map(|l| format!("− {}", crate::mail::clip(l, 120))).chain(came.iter().take(5).map(|l| format!("+ {}", crate::mail::clip(l, 120)))).collect::<Vec<_>>().join("\n")
                };
                format!("{head}\n{body}\n{}", self.link)
            }
        }
    }
}

/// The address a person gave for a kind of channel, as kept, or why it will not do.
pub fn target_for(kind: &str, given: &str) -> Result<String, String> {
    let t = shaped(kind, given)?;
    crate::outbound::public_target(&t)?;
    Ok(t)
}

/// The address as given, if it has the shape of its kind of channel; where it leads is not asked yet.
fn shaped(kind: &str, given: &str) -> Result<String, String> {
    let t = given.trim();
    let ok = match kind {
        "slack" => t.starts_with("https://hooks.slack.com/services/"),
        "discord" => t.starts_with("https://discord.com/api/webhooks/") || t.starts_with("https://discordapp.com/api/webhooks/"),
        "webhook" => t.starts_with("https://"),
        _ => return Err("No such kind of channel.".into()),
    };
    if !ok || t.len() > 500 || t.contains(char::is_whitespace) {
        return Err(match kind {
            "slack" => "A Slack webhook address starts with https://hooks.slack.com/services/ (Slack: Apps, Incoming Webhooks).",
            "discord" => "A Discord webhook address starts with https://discord.com/api/webhooks/ (channel settings, Integrations, Webhooks).",
            _ => "A webhook address starts with https:// and has no spaces.",
        }
        .into());
    }
    Ok(t.to_string())
}

/// One delivery to one channel: `ok`, or what went wrong.
pub fn send(c: &Channel, ev: &Event) -> Result<(), String> {
    let (url, body, headers) = match c.kind.as_str() {
        "slack" => (c.target.clone(), json!({ "text": slack_safe(&ev.text()), "unfurl_links": false }).to_string(), Vec::new()),
        "discord" => (c.target.clone(), json!({ "content": crate::mail::clip(&ev.text(), 1900), "allowed_mentions": { "parse": [] }, "flags": 4 }).to_string(), Vec::new()),
        "webhook" => {
            let body = ev.json().to_string();
            let t = now();
            let mac = crate::crypto::hex(&crate::crypto::hmac_sha256(c.secret.as_bytes(), format!("{t}.{body}").as_bytes()));
            (c.target.clone(), body, vec![("Zetlyn-Signature", format!("t={t},v1={mac}")), ("Zetlyn-Event", ev.kind.to_string())])
        }
        "telegram" => {
            let bot = bot().ok_or("Telegram is not set up on this instance")?;
            return bot.say(&c.target, &ev.text());
        }
        other => return Err(format!("no such kind of channel: {other}")),
    };
    match crate::fetch::post_json(&url, &body, &headers)? {
        (s, _) if (200..300).contains(&s) => Ok(()),
        (s, _) => Err(format!("answered HTTP {s}")),
    }
}

/// Slack reads `&`, `<` and `>` as its own markup: a page's words must not make links or pings.
fn slack_safe(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// How deliveries went, waiting to be written down by the round (a thread has no store).
static OUTCOMES: Mutex<Vec<(i64, String)>> = Mutex::new(Vec::new());

/// Every channel of the account told, each in a thread of its own; tried again once a minute
/// later.
pub fn all(store: &Store, account: i64, ev: Event) -> Result<(), String> {
    let channels = store.channels_of(account)?;
    if channels.is_empty() {
        return Ok(());
    }
    let ev = std::sync::Arc::new(ev);
    for c in channels {
        let ev = ev.clone();
        std::thread::spawn(move || {
            let mut went = send(&c, &ev);
            if went.is_err() {
                std::thread::sleep(std::time::Duration::from_secs(60));
                went = send(&c, &ev);
            }
            if let Err(e) = &went {
                eprintln!("channel {} ({}): {e}", c.id, c.kind);
            }
            let said = went.err().unwrap_or_else(|| "ok".into());
            OUTCOMES.lock().unwrap_or_else(|e| e.into_inner()).push((c.id, said));
        });
    }
    Ok(())
}

/// How the deliveries since the last round went, written down.
pub fn record(store: &Store) {
    let done: Vec<(i64, String)> = std::mem::take(&mut *OUTCOMES.lock().unwrap_or_else(|e| e.into_inner()));
    for (id, said) in done {
        let _ = store.set_channel_status(id, &said);
    }
}

// --- Telegram ------------------------------------------------------------------------------

/// The instance's Telegram bot, from `TELEGRAM_BOT_TOKEN` (or the file `TELEGRAM_BOT_TOKEN_FILE`
/// names) and `TELEGRAM_BOT_NAME`; none without them.
pub struct Bot {
    token: String,
    pub name: String,
}

pub fn bot() -> Option<&'static Bot> {
    static BOT: OnceLock<Option<Bot>> = OnceLock::new();
    BOT.get_or_init(|| {
        let var = |k: &str| std::env::var(k).unwrap_or_default().trim().to_string();
        let token = match var("TELEGRAM_BOT_TOKEN") {
            t if !t.is_empty() => t,
            _ => std::fs::read_to_string(var("TELEGRAM_BOT_TOKEN_FILE")).map(|t| t.trim().to_string()).unwrap_or_default(),
        };
        let name = var("TELEGRAM_BOT_NAME").trim_start_matches('@').to_string();
        (!token.is_empty() && !name.is_empty()).then_some(Bot { token, name })
    })
    .as_ref()
}

impl Bot {
    /// The secret Telegram sends with each update (and the path it posts to), so that nobody else
    /// can pose as Telegram. Made from the token: nothing more to keep.
    pub fn secret(&self) -> String {
        crate::crypto::digest(&format!("zetlyn-telegram:{}", self.token))[..40].to_string()
    }

    fn call(&self, method: &str, body: J) -> Result<J, String> {
        let (status, text) = crate::fetch::post_json(&format!("https://api.telegram.org/bot{}/{method}", self.token), &body.to_string(), &[]).map_err(|e| e.replace(&self.token, "…"))?;
        let answer: J = serde_json::from_str(&text).unwrap_or(J::Null);
        if status == 200 && answer["ok"] == true {
            Ok(answer["result"].clone())
        } else {
            Err(format!("Telegram answered HTTP {status}: {}", answer["description"].as_str().unwrap_or("")))
        }
    }

    pub fn say(&self, chat: &str, text: &str) -> Result<(), String> {
        self.call("sendMessage", json!({ "chat_id": chat, "text": crate::mail::clip(text, 4000), "link_preview_options": { "is_disabled": true } })).map(|_| ())
    }

    /// Where Telegram is to post what the bot is told: `<base>/telegram/<secret>`.
    pub fn set_up(&self, base: &str) -> Result<(), String> {
        self.call("setWebhook", json!({ "url": format!("{base}/telegram/{}", self.secret()), "secret_token": self.secret(), "allowed_updates": ["message"] }))?;
        self.call("setMyCommands", json!({ "commands": [{ "command": "stop", "description": "Stop telling changes here" }] })).map(|_| ())
    }

    /// The link that connects a chat to an account: `/start <token>` in the bot's chat.
    pub fn link(&self, token: &str) -> String {
        format!("https://t.me/{}?start={token}", self.name)
    }
}

/// What the bot was told, by Telegram: `/start <token>` connects the chat to the account the
/// token was made for, `/stop` lets it go. The answer is said in the chat.
pub fn update(store: &Store, bot: &Bot, base: &str, body: &[u8]) -> Result<(), String> {
    let u: J = serde_json::from_slice(body).unwrap_or(J::Null);
    let msg = &u["message"];
    let Some(chat) = msg["chat"]["id"].as_i64().map(|c| c.to_string()) else { return Ok(()) };
    let text = msg["text"].as_str().unwrap_or("").trim();
    let reply = if let Some(token) = text.strip_prefix("/start").map(str::trim).filter(|t| !t.is_empty()) {
        match store.take_token(token, "telegram")? {
            Some((account, _)) => {
                store.add_channel(account, "telegram", &chat, "")?;
                format!("Connected. Zetlyn tells the changes of your watches here. Send /stop to stop; your channels are at {base}/account/notifications")
            }
            None => format!("This link has expired. Connect again at {base}/account/notifications"),
        }
    } else if text == "/stop" {
        match store.delete_telegram_chat(&chat)? {
            0 => "Nothing is told here.".into(),
            _ => format!("Stopped: Zetlyn tells nothing more here. Connect again at {base}/account/notifications"),
        }
    } else {
        format!("This is Zetlyn's bot: it tells the changes of your watches. Connect it at {base}/account/notifications")
    };
    bot.say(&chat, &reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_addresses_of_their_kind_are_taken() {
        assert!(shaped("slack", " https://hooks.slack.com/services/T0/B0/xyz ").is_ok());
        assert!(target_for("slack", "https://example.com/services/x").is_err());
        assert!(shaped("discord", "https://discord.com/api/webhooks/1/abc").is_ok());
        assert!(target_for("webhook", "http://example.com/hook").is_err(), "only https");
        assert!(target_for("webhook", "https://127.0.0.1/hook").is_err(), "nothing private");
        assert_eq!(shaped("webhook", "https://hooks.example.com/zetlyn").unwrap(), "https://hooks.example.com/zetlyn");
        assert!(shaped("webhook", "https://hooks.example.com/a b").is_err());
        assert!(target_for("fax", "x").is_err());
    }

    #[test]
    fn a_change_reads_short_in_a_chat_and_the_page_cannot_make_markup_there() {
        let ev = Event { reason: Some("Now below 400".into()), before: "429 €".into(), after: "<!channel> 389 €".into(), ..Event::test("https://zetlyn.test") };
        let t = ev.text();
        assert!(t.starts_with("Test · Now below 400: example.com"), "{t}");
        assert!(t.contains("before: 429 €\nnow: <!channel> 389 €"));
        assert_eq!(slack_safe("<!channel> & <x>"), "&lt;!channel&gt; &amp; &lt;x&gt;");
        let j = ev.json();
        assert_eq!((j["event"].as_str(), j["after"].as_str()), (Some("test"), Some("<!channel> 389 €")));
    }

    #[test]
    fn channels_belong_to_their_account_and_telegram_chats_can_leave() {
        let s = Store::in_memory();
        let (a, b) = (s.account("a@example.com").unwrap(), s.account("b@example.com").unwrap());
        assert!(s.mails_changes(a).unwrap(), "mail is on from the start");
        let hook = s.add_channel(a, "webhook", "https://hooks.example.com/z", "zws_x").unwrap();
        s.add_channel(a, "telegram", "4711", "").unwrap();
        s.add_channel(a, "telegram", "4711", "").unwrap();
        assert_eq!(s.channels_of(a).unwrap().len(), 2, "the same chat once");
        s.delete_channel(b, hook).unwrap();
        assert_eq!(s.channels_of(a).unwrap().len(), 2, "not b's to remove");
        assert_eq!(s.delete_telegram_chat("4711").unwrap(), 1);
        s.set_channel_status(hook, "ok").unwrap();
        let left = s.channels_of(a).unwrap();
        assert_eq!((left.len(), left[0].last_status.as_str()), (1, "ok"));
        s.set_mails_changes(a, false).unwrap();
        assert!(!s.mails_changes(a).unwrap());
    }

    #[test]
    fn a_reviewer_link_opens_its_account_again_until_it_is_ended() {
        let s = Store::in_memory();
        let a = s.account("review@example.com").unwrap();
        assert_eq!(s.review_until(a).unwrap(), None);
        let link = crate::serve::review_link(&s, "https://zetlyn.test", a, 500).unwrap();
        let token = link.rsplit('/').next().unwrap();
        let until = s.review_until(a).unwrap().unwrap();
        assert!(until <= now() + 90 * 86400 + 5, "at most 90 days");
        assert_eq!(s.token_holder(token, "review").unwrap(), Some(a));
        assert_eq!(s.token_holder(token, "review").unwrap(), Some(a), "again");
        assert_eq!(s.token_holder(token, "signin").unwrap(), None, "a reviewer link is no sign-in link");
        s.end_review(a).unwrap();
        assert_eq!(s.token_holder(token, "review").unwrap(), None);
    }
}
