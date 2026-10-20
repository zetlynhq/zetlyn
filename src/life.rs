//! A watch's life on a hosted instance: confirmed by its address, it starts as the plan says
//! (`plan.rs`); every minute what is due is checked and what changed is mailed; what was never
//! confirmed is cleaned away.
//!
//! A local instance has no mail: its watches simply run, and only the checks of `tick` apply.

use crate::check;
use crate::mail::{self, Mailer};
use crate::plan::{Plan, Start};
use crate::store::{now, Store, Watch, LOCAL};

/// How long a link in a mail about a watch opens the account: a week, once.
pub const DOOR: i64 = 7 * 86400;

/// What a hosted instance needs beyond a local one.
pub struct Hosted {
    /// `https://zetlyn.com`, no slash at the end: what links in mails start with.
    pub base: String,
    pub mailer: Mailer,
}

impl Hosted {
    /// A link that signs its reader in to `account` once and takes them to `next`.
    pub fn door(&self, store: &Store, account: i64, next: &str) -> Result<String, String> {
        let token = store.new_token(account, "signin", None, DOOR)?;
        Ok(format!("{}/s/{token}?next={}", self.base, form_urlencoded::byte_serialize(next.as_bytes()).collect::<String>()))
    }

    /// A mail sent, or said in the log when it could not be: a failed mail must not stop a round.
    pub fn tell(&self, to: &str, letter: &mail::Letter) {
        if let Err(e) = self.mailer.send(to, letter) {
            eprintln!("mail to {to}: {e}");
        }
    }

    /// A mail somebody waits for on a page (a sign-in link, a confirmation): sent beside the
    /// request, so the page answers at once and not after the mail server has had its say, which
    /// can take many seconds; tried once more a minute later if it fails. The mail log has it
    /// either way.
    pub fn tell_soon(&self, to: &str, letter: mail::Letter) {
        let (mailer, to) = (self.mailer.clone(), to.to_string());
        std::thread::spawn(move || {
            if let Err(e) = mailer.send(&to, &letter) {
                eprintln!("mail to {to}: {e}; again in a minute");
                std::thread::sleep(std::time::Duration::from_secs(60));
                if let Err(e) = mailer.send(&to, &letter) {
                    eprintln!("mail to {to}: {e}; given up");
                }
            }
        });
    }
}

/// A watch whose address was confirmed starts, as the plan says.
pub fn start(store: &Store, _h: &Hosted, plan: &Plan, account: i64, watch: i64) -> Result<Start, String> {
    let a = store.account_by_id(account)?.ok_or("no such account")?;
    plan.start(store, &a, watch)
}

/// Once a minute: the plan's own round, watches past their end ended, what was never confirmed
/// cleaned away, and what is due checked, its changes mailed. A line for each thing done.
pub fn tick(store: &Store, hosted: Option<&Hosted>, plan: &Plan) -> Result<Vec<String>, String> {
    let at = now();
    crate::tell::record(store);
    store.prune_checks(at)?;
    let mut said = Vec::new();
    if let Some(h) = hosted {
        said.extend(plan.tick(store, h, at)?);
        h.mailer.prune_log(at);
    }
    for w in store.expired(at)? {
        store.set_state(w.id, "ended", w.until)?;
        said.push(format!("{:>5} ended    {}", w.id, w.url));
    }
    if hosted.is_some() {
        let gone = store.clean_pending(at)?;
        if gone > 0 {
            said.push(format!("cleaned {gone} unconfirmed watches"));
        }
        store.clean_grants(at)?;
    }
    for c in check::round(store, false)? {
        said.push(c.line());
        if let Some(h) = hosted {
            notify(store, h, &c)?;
        }
    }
    Ok(said)
}

/// What a check means for the account: a change is mailed; a place that became unclear or went
/// is said once, until it is found again.
pub fn notify(store: &Store, h: &Hosted, c: &check::Checked) -> Result<(), String> {
    let Some(a) = store.account_by_id(c.watch.account)? else { return Ok(()) };
    if a.email == LOCAL {
        return Ok(());
    }
    let w = &c.watch;
    // The account's address may be off for changes, with another channel told instead (`tell.rs`).
    let mails = store.mails_changes(a.id)?;
    match c.outcome.as_str() {
        "changed" => {
            // With a condition, only the change that makes it come true is told (`when.rs`).
            let told = c.change.as_ref().filter(|(before, after)| w.condition.as_ref().map_or(true, |k| k.came_true(before, after)));
            if let Some((before, after)) = told {
                let account = format!("{}/watches/{}", h.base, w.key);
                let reason = w.condition.as_ref().map(|k| k.now_said());
                if mails {
                    h.tell(&a.email, &mail::changed(&w.url, &what(w), before, after, &crate::serve::time(now()), &w.url, &account, reason.as_deref()));
                }
                crate::tell::all(store, a.id, crate::tell::Event::change(w, before, after, &h.base))?;
            }
            if !w.told.is_empty() {
                store.set_told(w.id, "")?;
            }
        }
        "unclear" | "gone" if w.told != "unclear" && w.told != "gone" => {
            if mails {
                let link = h.door(store, a.id, &format!("/mark?watch={}", w.key))?;
                h.tell(&a.email, &mail::mark_again(&w.url, &link));
            }
            crate::tell::all(store, a.id, crate::tell::Event::unclear(w, &h.base))?;
            store.set_told(w.id, &c.outcome)?;
        }
        "same" if !w.told.is_empty() => store.set_told(w.id, "")?,
        _ => {}
    }
    Ok(())
}

pub fn what(w: &Watch) -> String {
    match &w.mode {
        crate::store::Mode::Place(a) if !a.label.is_empty() => format!("The place beside “{}”", clip(&a.label, 40)),
        crate::store::Mode::Place(_) => "The place you marked".into(),
        crate::store::Mode::Page { .. } => "The page".into(),
    }
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() > n { format!("{}…", s.chars().take(n - 1).collect::<String>()) } else { s.to_string() }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Mode;
    use crate::when::Condition;

    #[test]
    fn with_a_condition_only_the_change_that_makes_it_true_is_mailed() {
        let dir = std::env::temp_dir().join(format!("zetlyn-life-{}", crate::crypto::token()));
        let h = Hosted { base: "https://zetlyn.test".into(), mailer: mail::Mailer { from: "Zetlyn <hello@zetlyn.test>".into(), smtp: None, outbox: dir.join("outbox") } };
        let s = Store::in_memory();
        let a = s.account("a@example.com").unwrap();
        let id = s.add_watch(a, "https://shop.example/p", &Mode::Place(Default::default()), 60, None, "429,00 €").unwrap();
        s.set_condition(id, Some(&Condition::Below(400.0))).unwrap();
        let w = s.watch(id).unwrap().unwrap();
        let mails = || std::fs::read_dir(dir.join("outbox")).map(|d| d.count()).unwrap_or(0);
        let change = |before: &str, after: &str| check::Checked { watch: w.clone(), outcome: "changed".into(), moved: false, change: Some((before.into(), after.into())) };
        notify(&s, &h, &change("429,00 €", "419,00 €")).unwrap();
        assert_eq!(mails(), 0, "still above the mark");
        notify(&s, &h, &change("419,00 €", "389,00 €")).unwrap();
        assert_eq!(mails(), 1, "under the mark: told");
        notify(&s, &h, &change("389,00 €", "379,00 €")).unwrap();
        assert_eq!(mails(), 1, "further under it: not told again");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
