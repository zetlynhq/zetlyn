//! Watching the watcher: `zetlyn alert`, run every few minutes beside the instance (its own
//! systemd timer, so it runs when the instance does not), mails the operator when something is
//! wrong and once more when it is right again. Never twice for the same state.
//!
//! What counts as wrong: the instance does not answer; running watches are checked late (the
//! checks have stopped); or most of the last two hours' checks could not read their page (the
//! machine has lost its way out, or is being refused everywhere); or, within the last hour, a
//! change could not be passed to Stripe or an event from it not taken (`plan.rs`).
//!
//! What it cannot see is the machine itself being down: that needs a look from elsewhere.

use std::path::Path;
use std::time::Duration;

use crate::mail::{Letter, Mailer};
use crate::store::{now, Store};

pub fn run(data: &Path, to: &str, url: &str) -> Result<(), String> {
    let wrong = wrong(data, url);
    let state = data.join("alert.state");
    let before = std::fs::read_to_string(&state).unwrap_or_default();
    let now_said = wrong.join("\n");
    if now_said == before.trim_end() {
        println!("{}", if wrong.is_empty() { "right, as before".to_string() } else { format!("wrong, as before: {}", wrong.join("; ")) });
        return Ok(());
    }
    let letter = if wrong.is_empty() {
        Letter { subject: "Zetlyn: right again".into(), body: format!("What was wrong is right again:\n\n{}\n", before.trim_end()) }
    } else {
        Letter { subject: "Zetlyn: something is wrong".into(), body: format!("{}\n\nSaid once; you hear again when it is right.\n", wrong.join("\n")) }
    };
    Mailer::from_env(data).send(to, &letter)?;
    std::fs::write(&state, &now_said).map_err(|e| format!("{}: {e}", state.display()))?;
    println!("told {to}: {}", letter.subject);
    Ok(())
}

fn wrong(data: &Path, url: &str) -> Vec<String> {
    let mut wrong = Vec::new();
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).http_status_as_error(false).build().into();
    match agent.get(&format!("{url}/health")).call() {
        Ok(r) if r.status().as_u16() == 200 => {}
        Ok(r) => wrong.push(format!("The app answers {} at {url}/health.", r.status().as_u16())),
        Err(e) => wrong.push(format!("The app does not answer at {url}/health: {e}")),
    }
    match Store::open(data) {
        Ok(store) => {
            let at = now();
            if let Ok(late) = store.overdue(at, 15 * 60) {
                if !late.is_empty() {
                    wrong.push(format!("{} running watches are checked late (the first: {}): the checks have stopped.", late.len(), late[0].url));
                }
            }
            if let Ok((all, failed)) = store.outcomes_since(at - 7200) {
                if all >= 10 && failed * 2 >= all {
                    wrong.push(format!("{failed} of the last two hours' {all} checks could not read their page."));
                }
            }
            // Zetlyn and Stripe out of step (plan.rs notes it): wrong for an hour after the last time.
            let trouble = store.meta("billing_error");
            if let Some((when, what)) = trouble.split_once('\t') {
                if when.parse::<i64>().is_ok_and(|t| at - t < 3600) {
                    wrong.push(format!("Stripe: {what}"));
                }
            }
        }
        Err(e) => wrong.push(format!("The data cannot be opened: {e}")),
    }
    wrong
}
