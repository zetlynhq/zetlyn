//! zetlyn — a dataset is served, browsed and searched on its own. A scope puts several of them on
//! one page; it is not what makes them usable.

mod account;
mod artifact;
mod build;
mod console;
mod dataset;
mod decl;
mod expr;
mod fetch;
mod grant;
pub mod guess;
mod hub;
mod identity;
mod key;
mod place;
mod record;
mod remote;
mod scope;
mod scopedecl;
mod serve;
mod servescope;
mod source;
mod store;
mod watch;

use std::path::{Path, PathBuf};

use dataset::Dataset;

/// Days to a civil date. No calendar crate: a run stamp and a file date are the whole of what this
/// program does with time.
pub fn iso_date(unix_seconds: i64) -> String {
    let (y, m, d) = civil(unix_seconds.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn iso_stamp(unix_seconds: i64) -> String {
    let (y, m, d) = civil(unix_seconds.div_euclid(86_400));
    let s = unix_seconds.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        s / 3600,
        (s % 3600) / 60,
        s % 60
    )
}

fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

const USAGE: &str = "\
zetlyn

  zetlyn dataset new --from <path or URL> [--at <dir>] [--name owner/name] [--kind <word>]
      Reads a folder, a .csv or an .xlsx, guesses the identifier, the title, the text and the
      field types, writes <dir>/dataset.toml, and prints the first three records it would produce.

  zetlyn dataset run <dir>
      Fills the store. Says what was added, changed, removed and unchanged.

  zetlyn dataset describe <dir>
      What this dataset is, what it holds, what it can be asked. JSON.

  zetlyn search <dir> <query> [--view <name>] [--limit <n>]
      Free text and comparisons mixed: `log4j severity=high known>2026-01-01`.

  zetlyn scope describe <dir> [--datasets <dir>]
  zetlyn scope search <dir> <query> [--view <name>] [--kind <word>] [--limit <n>]
      A scope holds no index. It rewrites the query per member, fans out, merges ranked lists,
      and gathers the hits into one entry per subject.

  zetlyn account [list] <deployment>
  zetlyn account grant --email <a> [--days 31] [--scopes a,b] <deployment>
  zetlyn account cancel --email <a> <deployment>
  zetlyn account key --email <a> [--name <what for>] <deployment>
  zetlyn account curator --email <a> [--revoke] <deployment>
  zetlyn account dunning [--within 7] [--send] <deployment>
      The first customers arrive before a payment provider does, and are set by hand.

  zetlyn serve <dir> [--port 8080] [--addr 0.0.0.0:8080]
      Overview, views, browse with facets and columns, search, a record page.

  zetlyn dataset publish <dir> --to <hub> [--tag latest]
  zetlyn dataset subscribe <reference> --from <hub> [--at <dir>] [--key ed25519:…]
  zetlyn dataset update <dir>
      A hub is a folder, a mount, s3://bucket/prefix or an address. What travels is the
      records, so a subscriber needs none of the publisher's credentials.

  zetlyn id [new --name <n> --contact <c>]
      One key, everywhere you act. Readers are not this and hold none.

  zetlyn console serve <deployment> | grant | call
  zetlyn hub register | owners | serve
      Letting somebody else run it, and letting somebody else fetch from you.
";

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn positional(args: &[String], from: usize) -> Vec<&String> {
    let mut out = Vec::new();
    let mut i = from;
    while i < args.len() {
        if args[i].starts_with("--") {
            i += 2;
            continue;
        }
        out.push(&args[i]);
        i += 1;
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&args) {
        eprintln!("zetlyn: {e}");
        std::process::exit(1);
    }
}

fn run(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("dataset") => match args.get(1).map(String::as_str) {
            Some("new") => dataset_new(args),
            Some("run") => dataset_run(args),
            Some("publish") => dataset_publish(args),
            Some("subscribe") => dataset_subscribe(args),
            Some("update") => dataset_update(args),
            Some("check") => {
                let ds = Dataset::open(&dir_at(args, 2)?)?;
                let wrong = ds.check();
                for w in &wrong {
                    println!("{}: {w}", ds.decl.name);
                }
                if wrong.is_empty() {
                    println!("{}: nothing it claims is untrue", ds.decl.name);
                }
                Ok(())
            }
            Some("describe") => {
                let dir = dir_at(args, 2)?;
                let ds = Dataset::open(&dir)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&ds.describe()).unwrap_or_default()
                );
                Ok(())
            }
            _ => {
                print!("{USAGE}");
                Ok(())
            }
        },
        Some("search") => search(args),
        Some("changes") => changes(args),
        Some("run") => schedule(args),
        Some("watch") => watch_cmd(args),
        Some("account") => account_cmd(args),
        Some("scope") => match args.get(1).map(String::as_str) {
            Some("publish") => scope_publish(args),
            Some("subscribe") => scope_subscribe(args),
            Some("describe") => {
                let (dir, datasets) = scope_at(args, 2)?;
                let scope = scope::Scope::open(&dir, &datasets)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&scope.describe()).unwrap_or_default()
                );
                Ok(())
            }
            Some("search") => scope_search(args),
            Some("check") => {
                let (dir, datasets) = scope_at(args, 2)?;
                let scope = scope::Scope::open(&dir, &datasets)?;
                let wrong = scope.check();
                for w in &wrong {
                    println!("{}: {w}", scope.decl.name);
                }
                if wrong.is_empty() {
                    println!("{}: nothing it claims is untrue", scope.decl.name);
                }
                Ok(())
            }
            Some("measure") => {
                let (dir, datasets) = scope_at(args, 2)?;
                let scope = scope::Scope::open(&dir, &datasets)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&scope.measure()).unwrap_or_default()
                );
                Ok(())
            }
            _ => {
                print!("{USAGE}");
                Ok(())
            }
        },
        // One command, and the directory says which it is.
        Some("console") => console_command(args),
        Some("hub") => hub_command(args),
        Some("id") => id_command(args),
        Some("serve") => {
            let port = flag(args, "--port").unwrap_or("8080");
            // Loopback unless asked otherwise: a scope reachable from the network is a decision
            // an operator makes, not a default they discover.
            let addr = flag(args, "--addr")
                .map(str::to_string)
                .unwrap_or_else(|| format!("127.0.0.1:{port}"));
            let named = positional(args, 1)
                .first()
                .map(|s| PathBuf::from(s.as_str()))
                .ok_or("which directory?")?;
            if named.join("scope.toml").exists() {
                let (dir, datasets) = scope_at(args, 1)?;
                let scope = scope::Scope::open(&dir, &datasets)?;
                for name in &scope.missing {
                    eprintln!(
                        "zetlyn: {name} is not installed here, and the scope opens without it"
                    );
                }
                return servescope::serve(scope, &addr);
            }
            let dir = dir_at(args, 1)?;
            let ds = Dataset::open(&dir)?;
            if ds.store.count() == 0 {
                eprintln!(
                    "zetlyn: the store is empty. `zetlyn dataset run {}` first.",
                    dir.display()
                );
            }
            serve::serve(ds, &addr)
        }
        // A published artifact names the build that made it, so the build has to name itself.
        Some("--version") | Some("-V") | Some("version") => {
            println!("zetlyn {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => {
            print!("{USAGE}");
            Ok(())
        }
    }
}

fn dir_at(args: &[String], from: usize) -> Result<PathBuf, String> {
    let p = positional(args, from)
        .first()
        .map(|s| PathBuf::from(s.as_str()))
        .ok_or("which dataset directory?")?;
    if !p.join("dataset.toml").exists() {
        return Err(format!("{}: no dataset.toml here", p.display()));
    }
    Ok(p)
}

fn dataset_new(args: &[String]) -> Result<(), String> {
    let from = flag(args, "--from").ok_or("--from <path> or a URL is required")?;
    if from.starts_with("http://") || from.starts_with("https://") {
        let stem = from
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .map(|s| guess::slug(s.split('?').next().unwrap_or(s)))
            .unwrap_or_else(|| "dataset".into());
        let dir = PathBuf::from(
            flag(args, "--at")
                .map(str::to_string)
                .unwrap_or(format!("./{stem}")),
        );
        let toml = guess::propose_url(from, &dir, flag(args, "--name"), flag(args, "--kind"))?;
        println!("{}\n", dir.join("dataset.toml").display());
        print!("{toml}");
        return Ok(());
    }
    let from = Path::new(from);
    let stem = from
        .file_stem()
        .map(|s| guess::slug(&s.to_string_lossy()))
        .unwrap_or_else(|| "dataset".into());
    let dir = PathBuf::from(
        flag(args, "--at")
            .map(str::to_string)
            .unwrap_or(format!("./{stem}")),
    );
    let toml = guess::propose(from, &dir, flag(args, "--name"), flag(args, "--kind"))?;

    println!("{}", dir.join("dataset.toml").display());
    println!();
    print!("{toml}");
    println!();

    // Three records, because a creator who agrees changes nothing and a creator who does not needs
    // to see why before a run writes anything.
    let ds = Dataset::open(&dir)?;
    let root = ds.decl.source.root(&dir);
    let mut notes = build::Notes::default();
    let mut shown = 0usize;
    let _ = source::each_row(&ds.decl, &dir, &root, None, |produced: source::Produced| {
        for (sub, origin) in
            build::expand(&ds.decl, produced.row, produced.origin, produced.expanded)
        {
            if shown >= 3 {
                return Err("enough".into());
            }
            if let Some(rec) = build::build(&ds.decl, sub, origin, &mut notes) {
                shown += 1;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rec.to_json()).unwrap_or_default()
                );
            }
        }
        Ok(())
    });
    if shown == 0 {
        println!("(no records — the source handed over nothing this declaration could read)");
    }
    Ok(())
}

fn dataset_run(args: &[String]) -> Result<(), String> {
    let dir = dir_at(args, 2)?;
    let ds = Dataset::open(&dir)?;
    let started = std::time::Instant::now();
    let r = ds.run()?;
    println!(
        "run {} {} in {:.1}s: +{} ~{} −{} ={}",
        r.id,
        if r.complete { "complete" } else { "partial" },
        started.elapsed().as_secs_f64(),
        r.added,
        r.changed,
        r.removed,
        r.unchanged
    );
    if r.no_text > 0 {
        println!("  {} records with no text", r.no_text);
    }
    if r.no_known > 0 {
        println!("  {} took their date from the file", r.no_known);
    }
    if r.duplicates > 0 {
        println!(
            "  {} rows the source gave twice under one key, and the first of each was kept",
            r.duplicates
        );
    }
    if r.unparsed > 0 {
        println!(
            "  {} values did not parse as their type{}",
            r.unparsed,
            r.note
                .as_deref()
                .filter(|n| !n.is_empty())
                .map(|n| format!(": {n}"))
                .unwrap_or_default()
        );
    }
    if let Some(why) = &r.refused {
        println!(
            "  {} records were read and the store was not replaced: {why}",
            r.unchanged
        );
        println!("  nothing was written, and the last complete run still stands");
    }
    if let Some(e) = &r.error {
        println!("  the run did not finish: {e}");
        println!("  nothing was removed, because a partial run has not seen the source");
    }
    Ok(())
}

fn changes(args: &[String]) -> Result<(), String> {
    let rest = positional(args, 1);
    let dir = PathBuf::from(rest.first().ok_or("which directory?")?.as_str());
    let limit = flag(args, "--limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);
    let report = if dir.join("scope.toml").exists() {
        let (dir, datasets) = scope_at(args, 1)?;
        let scope = scope::Scope::open(&dir, &datasets)?;
        let since = flag(args, "--since")
            .map(str::to_string)
            .unwrap_or_else(|| scope.mark_before().to_string());
        scope.changes(&since, limit)
    } else {
        let ds = Dataset::open(&dir_at(args, 1)?)?;
        let since = flag(args, "--since")
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| (ds.mark() - 1).max(0));
        dataset::Member::changes(&ds, since, limit)
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&report).unwrap_or_default()
    );
    Ok(())
}

/// When this dataset is next due, from its declared cadence and when it last finished.
fn due_at(ds: &Dataset) -> Option<i64> {
    let every = ds
        .decl
        .schedule
        .every
        .as_deref()
        .and_then(fetch::duration)?;
    let last = ds
        .store
        .run_report(ds.store.last_run())
        .and_then(|r| r.finished)
        .map(|f| fetch::seconds_of(&f));
    Some(match last {
        Some(at) => at + every,
        None => 0,
    })
}

fn deployment(args: &[String], from: usize) -> Result<PathBuf, String> {
    let root = positional(args, from)
        .first()
        .map(|s| PathBuf::from(s.as_str()))
        .unwrap_or_else(|| PathBuf::from("."));
    if !root.join("datasets").is_dir() {
        return Err(format!("{}: no datasets here", root.display()));
    }
    Ok(root)
}

/// The scheduler. Runs what is due, then replays every watch.
fn schedule(args: &[String]) -> Result<(), String> {
    let root = deployment(args, 1)?;
    let once = args.iter().any(|a| a == "--once");
    let deliver = !args.iter().any(|a| a == "--no-deliver");
    loop {
        let tick = now();
        let mut soonest: Option<i64> = None;
        // The runs happen one after another in one process, so the order matters: a source
        // that is being throttled can take twenty minutes, and an hourly dataset behind it
        // would wait that out. Shortest cadence first, so what is asked for most often is
        // asked for first.
        let mut due: Vec<(i64, String, PathBuf)> = scope::registry(&root.join("datasets"))
            .into_iter()
            .filter_map(|(name, dir)| {
                let every = Dataset::open(&dir)
                    .ok()?
                    .decl
                    .schedule
                    .every
                    .as_deref()
                    .and_then(fetch::duration)?;
                Some((every, name, dir))
            })
            .collect();
        due.sort();
        for (_, name, dir) in due {
            let ds = match Dataset::open(&dir) {
                Ok(ds) => ds,
                Err(e) => {
                    eprintln!("{name}: {e}");
                    continue;
                }
            };
            let Some(due) = due_at(&ds) else { continue };
            if due > tick {
                soonest = Some(soonest.map_or(due, |s: i64| s.min(due)));
                continue;
            }
            match ds.run() {
                Ok(r) => println!(
                    "{} run {} {}: +{} ~{} −{} ={}{}",
                    name,
                    r.id,
                    if r.complete { "complete" } else { "partial" },
                    r.added,
                    r.changed,
                    r.removed,
                    r.unchanged,
                    r.refused
                        .map(|w| format!("  refused: {w}"))
                        .unwrap_or_default()
                ),
                Err(e) => eprintln!("{name}: {e}"),
            }
            if let Ok(ds) = Dataset::open(&dir) {
                if let Some(next) = due_at(&ds) {
                    soonest = Some(soonest.map_or(next, |s: i64| s.min(next)));
                }
            }
        }

        for w in watch::all(&root) {
            match w.check(&root) {
                Ok((report, mark)) => {
                    let n = report["entries"].as_array().map(Vec::len).unwrap_or(0);
                    if n == 0 {
                        continue;
                    }
                    if !deliver {
                        println!("{}: {n} entries, not delivered", w.decl.name);
                        continue;
                    }
                    match w.deliver(&report, &mark) {
                        Ok(to) => println!("{}: {n} entries → {}", w.decl.name, to.join(", ")),
                        Err(e) => eprintln!("{}: {e}", w.decl.name),
                    }
                }
                Err(e) => eprintln!("{}: {e}", w.decl.name),
            }
        }

        if once {
            return Ok(());
        }
        // A minute at least, an hour at most: the promise is checked on the same tick.
        let wait = soonest.map(|s| (s - now()).clamp(60, 3600)).unwrap_or(3600);
        println!("next in {wait}s");
        std::thread::sleep(std::time::Duration::from_secs(wait as u64));
    }
}

fn watch_cmd(args: &[String]) -> Result<(), String> {
    let root = deployment(args, 2)?;
    let watches = watch::all(&root);
    match args.get(1).map(String::as_str) {
        Some("list") => {
            for w in &watches {
                let over = w
                    .decl
                    .scope
                    .clone()
                    .or(w.decl.dataset.clone())
                    .unwrap_or_default();
                let to: Vec<&str> = w.decl.deliver.iter().map(|d| d.to.as_str()).collect();
                println!(
                    "{}  over {over}  query {:?}  → {}  mark {}",
                    w.decl.name,
                    w.decl.query,
                    to.join(", "),
                    w.state().mark
                );
            }
            Ok(())
        }
        _ => {
            let deliver = args.iter().any(|a| a == "--deliver");
            for w in &watches {
                let (report, mark) = w.check(&root)?;
                let n = report["entries"].as_array().map(Vec::len).unwrap_or(0);
                println!("{}: {n} entries since {}", w.decl.name, report["since"]);
                if deliver && n > 0 {
                    println!("  → {}", w.deliver(&report, &mark)?.join(", "));
                } else if n > 0 {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&report).unwrap_or_default()
                    );
                }
            }
            Ok(())
        }
    }
}

/// Accounts and subscriptions from the terminal. The first customers arrive before any payment
/// provider does, and they are set by hand.
fn account_cmd(args: &[String]) -> Result<(), String> {
    let root = deployment(args, 2)?;
    let accounts = account::Accounts::open(&root)?;
    match args.get(1).map(String::as_str) {
        Some("grant") => {
            let email = flag(args, "--email").ok_or("--email is required")?;
            let days: i64 = flag(args, "--days")
                .and_then(|d| d.parse().ok())
                .unwrap_or(31);
            let scopes: Vec<String> = flag(args, "--scopes")
                .map(|s| s.split(',').map(str::to_string).collect())
                .unwrap_or_default();
            let until = iso_date(now() + days * 86_400);
            let a = accounts.set_subscription(
                email,
                "active",
                Some(&until),
                &scopes,
                flag(args, "--reference"),
            )?;
            println!("{} active until {}", a.email, until);
            Ok(())
        }
        Some("cancel") => {
            let email = flag(args, "--email").ok_or("--email is required")?;
            let held = accounts.by_email(email).ok_or("no such account")?;
            let a = accounts.set_subscription(
                email,
                "cancelled",
                held.paid_until.as_deref(),
                &held.scopes,
                None,
            )?;
            println!(
                "{} cancelled, and holds what was paid for until {}",
                a.email,
                a.paid_until.as_deref().unwrap_or("the end of the period")
            );
            Ok(())
        }
        Some("curator") => {
            let email = flag(args, "--email").ok_or("--email is required")?;
            let yes = !args.iter().any(|a| a == "--revoke");
            let a = accounts.set_curator(email, yes)?;
            println!(
                "{} {}",
                a.email,
                if a.curator {
                    "may curate"
                } else {
                    "may not curate"
                }
            );
            Ok(())
        }
        Some("key") => {
            let email = flag(args, "--email").ok_or("--email is required")?;
            let a = accounts.by_email(email).ok_or("no such account")?;
            let name = flag(args, "--name").unwrap_or("a key");
            println!("{}", accounts.new_key(a.id, name)?);
            Ok(())
        }
        // The dunning run: who lapses soon, and one message each.
        Some("dunning") => {
            let within: i64 = flag(args, "--within")
                .and_then(|d| d.parse().ok())
                .unwrap_or(7);
            let site = account::Site::load(&root);
            let send = args.iter().any(|a| a == "--send");
            for a in accounts.lapsing(within) {
                let until = a.paid_until.clone().unwrap_or_default();
                println!("{} lapses {until}", a.email);
                if send {
                    site.send(
                        &a.email,
                        "Your Zetlyn subscription",
                        &format!(
                            "Your subscription runs to {until}.\n\nNothing stops on that day \
                             except the current records, the feeds, the API and the export. What \
                             you already hold you keep."
                        ),
                    )?;
                }
            }
            Ok(())
        }
        _ => {
            for a in accounts.all() {
                println!(
                    "{:<34} {:<10} {}",
                    a.email,
                    a.state,
                    a.paid_until.as_deref().unwrap_or("")
                );
            }
            Ok(())
        }
    }
}

fn scope_at(args: &[String], from: usize) -> Result<(PathBuf, PathBuf), String> {
    let p = positional(args, from)
        .first()
        .map(|s| PathBuf::from(s.as_str()))
        .ok_or("which scope directory?")?;
    if !p.join("scope.toml").exists() {
        return Err(format!("{}: no scope.toml here", p.display()));
    }
    // A deployment holds `datasets/` beside `scopes/`, and a member is found there by its name.
    let datasets = match flag(args, "--datasets") {
        Some(d) => PathBuf::from(d),
        None => p.join("..").join("..").join("datasets"),
    };
    if !datasets.is_dir() {
        return Err(format!(
            "{}: no datasets here. Name one with --datasets <dir>",
            datasets.display()
        ));
    }
    Ok((p, datasets))
}

fn scope_search(args: &[String]) -> Result<(), String> {
    let (dir, datasets) = scope_at(args, 2)?;
    let scope = scope::Scope::open(&dir, &datasets)?;
    let rest = positional(args, 2);
    let terms: Vec<String> = rest.iter().skip(1).map(|s| s.to_string()).collect();
    let (text, pred) = expr::parse_query(&terms.join(" "));
    let q = scope::ScopeQuery {
        text,
        pred,
        named: flag(args, "--view").map(str::to_string),
        kind: flag(args, "--kind").map(str::to_string),
        sort: flag(args, "--sort").map(str::to_string),
        limit: flag(args, "--limit")
            .and_then(|s| s.parse().ok())
            .unwrap_or(10),
        offset: 0,
        seen_before: None,
    };
    let answer = scope.search(&q);
    for name in &scope.missing {
        println!("missing member: {name}");
    }
    for (member, reasons) in &answer.unanswered {
        println!("{member} did not answer: {}", reasons.join("; "));
    }
    println!(
        "{} entries over {} records, {} of {} members answered",
        answer.entries.len(),
        answer.total,
        answer.answered.len(),
        scope.members.len()
    );
    for e in &answer.entries {
        let key = e
            .key
            .as_ref()
            .map(|k| k.value.clone())
            .unwrap_or_else(|| "—".into());
        println!("{:>3}. {}  [{}]", e.rank, e.title, key);
        for (kind, parts) in e.by_kind() {
            let names: Vec<String> = parts
                .iter()
                .map(|p| format!("{} ({})", p.title, p.member))
                .collect();
            println!("     {kind}: {}", names.join(", "));
        }
        for (name, view) in &e.fields {
            if view.by.len() < 2 && !view.divergent {
                continue;
            }
            let shown: Vec<String> = view
                .means
                .iter()
                .map(|(m, v)| {
                    let raw = view.by.get(m).cloned().unwrap_or_default();
                    if &raw == v {
                        format!("{m}={v}")
                    } else {
                        format!("{m}={raw}→{v}")
                    }
                })
                .collect();
            println!(
                "     {name}{}: {}",
                if view.divergent { " (divergent)" } else { "" },
                shown.join(", ")
            );
        }
    }
    Ok(())
}

fn search(args: &[String]) -> Result<(), String> {
    let rest = positional(args, 1);
    let dir = PathBuf::from(rest.first().ok_or("which dataset directory?")?.as_str());
    let terms: Vec<String> = rest.iter().skip(1).map(|s| s.to_string()).collect();
    let ds = Dataset::open(&dir)?;
    let (text, pred) = expr::parse_query(&terms.join(" "));
    let q = dataset::Query {
        text,
        pred,
        ids: Vec::new(),
        seen_before: None,
        view: flag(args, "--view").map(str::to_string),
        sort: flag(args, "--sort").map(str::to_string),
        limit: flag(args, "--limit")
            .and_then(|s| s.parse().ok())
            .unwrap_or(20),
        offset: 0,
    };
    let (total, hits, unanswered) = ds.search(&q)?;
    for u in &unanswered.0 {
        println!("not answered here: {u}");
    }
    println!("{total} records");
    for h in &hits {
        let why = if let Some(id) = &h.why_id {
            format!("identifier {}", id.value)
        } else if !h.why_text.is_empty() {
            format!("matched {}", h.why_text.join(", "))
        } else {
            String::new()
        };
        println!(
            "{:>3}. {}  {}",
            h.rank,
            h.title,
            if why.is_empty() {
                String::new()
            } else {
                format!("({why})")
            }
        );
        println!("     {} · {}", h.known, h.record_id);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Publishing, and taking what somebody else published.

/// `zetlyn dataset publish <dir> --to <hub> [--tag latest] [--expect <version>|-]`
fn dataset_publish(args: &[String]) -> Result<(), String> {
    let dir = dir_at(args, 2)?;
    let to = flag(args, "--to").ok_or("--to where? a folder, a mount, s3://bucket/prefix")?;
    let tag = flag(args, "--tag").unwrap_or("latest");
    let ds = Dataset::open(&dir)?;
    // Nothing is published that the dataset itself says is untrue.
    let wrong = ds.check();
    if !wrong.is_empty() {
        for w in &wrong {
            eprintln!("{}: {w}", ds.decl.name);
        }
        return Err("publishing would put that on somebody else's machine".into());
    }
    let place = place::at(to)?;
    let version = artifact::publish(&ds, place.as_ref(), tag, flag(args, "--expect"))?;
    println!(
        "{}@{tag} is {version}, {} records, at {}",
        ds.decl.name,
        ds.store.count(),
        place.describe()
    );
    Ok(())
}

/// `zetlyn dataset subscribe <reference> --from <hub> [--at <dir>]`
fn dataset_subscribe(args: &[String]) -> Result<(), String> {
    let raw = positional(args, 2)
        .first()
        .map(|s| s.to_string())
        .ok_or("which dataset? owner/name, with an optional @tag")?;
    let reference = artifact::Reference::parse(&raw)?;
    let from = flag(args, "--from")
        .map(str::to_string)
        .or_else(|| reference.host.as_ref().map(|h| format!("https://{h}")))
        .ok_or("--from where? a folder, a mount, s3://bucket/prefix, or an address")?;
    let into = match flag(args, "--at") {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from("datasets").join(&reference.name),
    };
    let place = place::at(&from)?;
    let (held, version) = artifact::subscribe(
        place.as_ref(),
        &reference,
        &into,
        &from,
        flag(args, "--key"),
    )?;
    println!(
        "{reference} is {version}, {held} records, in {}",
        into.display()
    );
    Ok(())
}

/// `zetlyn dataset update <dir>`: ask the hub this one came from whether there is a newer version.
fn dataset_update(args: &[String]) -> Result<(), String> {
    let dir = dir_at(args, 2)?;
    let decl = decl::Declaration::load(&dir)?;
    let decl::Source::Hub {
        at, reference, key, ..
    } = &decl.source
    else {
        return Err(format!(
            "{} is not subscribed. `zetlyn dataset run` fills it from its source",
            decl.name
        ));
    };
    let pinned = Some(key.as_str()).filter(|k| !k.trim().is_empty());
    let reference = artifact::Reference::parse(reference)?;
    let place = place::at(at)?;
    let manifest = artifact::manifest_signed_by(place.as_ref(), &reference, "datasets", pinned)?;
    let offered = manifest["version"].as_str().unwrap_or_default();
    let held = artifact::held_version(&dir).unwrap_or_default();
    if offered == held {
        println!("{reference} is at {held}, which is what you hold");
        return Ok(());
    }
    // The delta first, and the whole where there is none or it did not hold together.
    if !held.is_empty() {
        match artifact::apply_delta(place.as_ref(), &reference, &dir, &held, offered, &manifest) {
            Ok(Some((added, changed, removed))) => {
                println!("{reference} {held} → {offered} by delta: +{added} ~{changed} −{removed}");
                return Ok(());
            }
            Ok(None) => {}
            Err(e) => eprintln!("the delta did not apply, taking the whole: {e}"),
        }
    }
    let (n, version) = artifact::subscribe(place.as_ref(), &reference, &dir, at, pinned)?;
    println!("{reference} {held} → {version}, {n} records, whole");
    Ok(())
}

/// `zetlyn scope publish <dir> --to <hub> [--tag latest] [--expect <version>]`
fn scope_publish(args: &[String]) -> Result<(), String> {
    let (dir, datasets) = scope_at(args, 2)?;
    let to = flag(args, "--to").ok_or("--to where? a folder, a mount, s3://bucket/prefix")?;
    let tag = flag(args, "--tag").unwrap_or("latest");
    // A scope that does not hold together is not published, for the same reason a dataset is not.
    let scope = scope::Scope::open(&dir, &datasets)?;
    let wrong = scope.check();
    if !wrong.is_empty() {
        for w in &wrong {
            eprintln!("{}: {w}", scope.decl.name);
        }
        return Err("publishing would put that on somebody else's machine".into());
    }
    let place = place::at(to)?;
    let version =
        artifact::publish_scope(&dir, &datasets, place.as_ref(), tag, flag(args, "--expect"))?;
    println!(
        "{}@{tag} is {version}, {} members, at {}",
        scope.decl.name,
        scope.members.len(),
        place.describe()
    );
    Ok(())
}

/// `zetlyn scope subscribe <reference> --from <hub> [--at <deployment>]`
fn scope_subscribe(args: &[String]) -> Result<(), String> {
    let raw = positional(args, 2)
        .first()
        .map(|s| s.to_string())
        .ok_or("which scope? owner/name, with an optional @tag")?;
    let reference = artifact::Reference::parse(&raw)?;
    let from = flag(args, "--from")
        .map(str::to_string)
        .or_else(|| reference.host.as_ref().map(|h| format!("https://{h}")))
        .ok_or("--from where? a folder, a mount, s3://bucket/prefix, or an address")?;
    let root = PathBuf::from(flag(args, "--at").unwrap_or("."));
    let into = root.join("scopes").join(&reference.name);
    let datasets = root.join("datasets");
    let place = place::at(&from)?;
    let (version, taken) =
        artifact::subscribe_scope(place.as_ref(), &reference, &into, &datasets, &from)?;
    println!("{reference} is {version}, in {}", into.display());
    for name in &taken {
        println!("  took {name}");
    }
    if taken.is_empty() {
        println!("  every member was held already");
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// A hub several people publish to.

fn hub_command(args: &[String]) -> Result<(), String> {
    match args.get(1).map(String::as_str) {
        Some("register") => {
            let dir = PathBuf::from(flag(args, "--at").unwrap_or("."));
            let name = flag(args, "--owner").ok_or("--owner which name?")?;
            let email = flag(args, "--email").ok_or("--email for whom?")?;
            // Your own key by default, because a publisher and an operator should be one person.
            let who = match flag(args, "--key") {
                Some(k) => k.to_string(),
                None => identity::key().ok_or(
                    "no identity. `zetlyn id new --name … --contact …` makes one, and a name on \
                     a hub belongs to a key rather than to a token",
                )?,
            };
            let mut owners = hub::Owners::load(&dir);
            owners.register(&dir, name, email, &who)?;
            println!("{name} is yours, first come, and it belongs to");
            println!("  {who}");
            println!("\nNothing was handed out: you already hold the half that signs. It writes");
            println!("under datasets/{name}/ and scopes/{name}/ and nowhere else.");
            Ok(())
        }
        // The key a publisher signs with. It lives in the dataset directory and is read by
        // `dataset publish`; there is nothing to turn on.
        Some("key") => {
            let dir = PathBuf::from(flag(args, "--at").unwrap_or("."));
            if let Some(held) = identity::or_local(&dir, artifact::KEY_FILE) {
                println!("{held}");
                println!("\nThat is the public half. Give it to subscribers; they pin it with");
                println!("`zetlyn dataset subscribe … --key {held}`.");
                return Ok(());
            }
            let public = artifact::new_key(&dir)?;
            println!("{public}");
            println!(
                "\nThe private half is in {}/{}, readable by you and nobody else.",
                dir.display(),
                artifact::KEY_FILE
            );
            println!("Do not publish this dataset from a second machine with a second key: every");
            println!("subscriber who pinned the first would stop trusting you.");
            Ok(())
        }
        Some("owners") => {
            let dir = PathBuf::from(flag(args, "--at").unwrap_or("."));
            let owners = hub::Owners::load(&dir);
            for (name, o) in &owners.owner {
                println!("{name:<20} {:<32} {}", o.email, o.registered);
            }
            if owners.owner.is_empty() {
                println!("nobody yet");
            }
            Ok(())
        }
        Some("console") => console_command(args),
        Some("hub") => hub_command(args),
        Some("id") => id_command(args),
        Some("serve") => {
            let dir = PathBuf::from(
                positional(args, 2)
                    .first()
                    .map(|s| s.as_str())
                    .unwrap_or("."),
            );
            let addr = match (flag(args, "--addr"), flag(args, "--port")) {
                (Some(a), _) => a.to_string(),
                (None, Some(p)) => format!("127.0.0.1:{p}"),
                _ => "127.0.0.1:8090".to_string(),
            };
            hub::serve(&dir, &addr)
        }
        _ => {
            print!("{HUB_USAGE}");
            Ok(())
        }
    }
}

const HUB_USAGE: &str = "\
  zetlyn hub register --owner <name> --email <a> [--at <dir>]
      Takes an owner name, first come and for good, for your identity or a key you name.

  zetlyn hub owners [--at <dir>]
      Who holds what.

  zetlyn hub key [--at <dataset dir>]
      The key this dataset is published under. Your identity where it has none.

  zetlyn hub serve <dir> [--port 8090] [--addr 127.0.0.1:8090]
      GET for anybody. PUT signed by a key that holds the owner named in the path.
      A folder, a mount and a private bucket need none of this.
";

// ---------------------------------------------------------------------------------------------
// A deployment answering for itself, and whoever is allowed to ask.

fn console_command(args: &[String]) -> Result<(), String> {
    match args.get(1).map(String::as_str) {
        Some("key") => {
            let dir = PathBuf::from(flag(args, "--at").unwrap_or("."));
            let file = flag(args, "--name").unwrap_or(grant::OPERATOR_KEY);
            if let Some(held) = key::public(&dir, file) {
                println!("{held}");
                return Ok(());
            }
            println!("{}", key::new(&dir, file)?);
            println!(
                "\nThe private half is in {}/{file}, readable by you and nobody else.",
                dir.display()
            );
            Ok(())
        }
        Some("grant") => {
            let root = PathBuf::from(flag(args, "--at").unwrap_or("."));
            let to = flag(args, "--to").ok_or("--to which key? the one the other side made")?;
            let can: Vec<String> = flag(args, "--can")
                .unwrap_or("read")
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            let until = flag(args, "--until").ok_or("--until when? a date")?;
            let name = deployment_name(&root);
            let signed = grant::issue(
                &root,
                &name,
                to,
                &can,
                until,
                flag(args, "--why").unwrap_or(""),
            )?;
            let out = PathBuf::from(flag(args, "--out").unwrap_or("grant.toml"));
            signed.write(&out)?;
            println!("{} may {} on {name} until {until}", to, can.join(", "));
            println!(
                "written to {}. It is not a secret and it is not a way in on its own:",
                out.display()
            );
            println!("whoever uses it still has to hold the private half of that key.");
            Ok(())
        }
        Some("call") => console_call(args),
        Some("serve") | None => {
            let root = PathBuf::from(
                positional(args, 2)
                    .first()
                    .map(|s| s.as_str())
                    .unwrap_or("."),
            );
            let addr = match (flag(args, "--addr"), flag(args, "--port")) {
                (Some(a), _) => a.to_string(),
                (None, Some(p)) => format!("127.0.0.1:{p}"),
                _ => "127.0.0.1:8100".to_string(),
            };
            console::serve(&root, &addr)
        }
        _ => {
            print!("{CONSOLE_USAGE}");
            Ok(())
        }
    }
}

/// A deployment is its directory, so it is called what the directory is called.
fn deployment_name(root: &Path) -> String {
    root.canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "deployment".into())
}

/// The smallest thing that can drive a console, which is what a platform will be a larger one of.
fn console_call(args: &[String]) -> Result<(), String> {
    let url = positional(args, 2)
        .first()
        .map(|s| s.to_string())
        .ok_or("which address? http://host:port/dataset/kev")?;
    let method = flag(args, "--method").unwrap_or("GET").to_uppercase();
    let grant_path = PathBuf::from(flag(args, "--grant").ok_or("--grant which file?")?);
    // Your own identity by default. A caller with a key of its own is a caller somebody has to
    // remember is also them.
    let (key_dir, key_file) = match flag(args, "--key") {
        Some(dir) => (
            PathBuf::from(dir),
            flag(args, "--key-name").unwrap_or("caller.key"),
        ),
        None => (identity::home(), identity::KEY_FILE),
    };
    let body = match flag(args, "--body") {
        Some(p) => std::fs::read(p).map_err(|e| format!("{p}: {e}"))?,
        None => Vec::new(),
    };

    let raw = std::fs::read(&grant_path).map_err(|e| format!("{}: {e}", grant_path.display()))?;
    let (base, path) = split(&url)?;
    let at = crate::iso_stamp(crate::now());
    let statement = grant::request_statement(&method, &path, &body, &at);
    let signature = key::sign(&key_dir, key_file, statement.as_bytes())?
        .ok_or_else(|| format!("{}/{key_file}: no key to sign with", key_dir.display()))?;

    // A refusal carries its reason in the body, and a client that turns the status into an error
    // throws the reason away. The status is printed beside the answer instead.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(concat!("zetlyn/", env!("CARGO_PKG_VERSION")))
        .timeout_global(Some(std::time::Duration::from_secs(600)))
        .http_status_as_error(false)
        .build()
        .into();
    // ureq types a request by whether it carries a body, so the two shapes are built apart and
    // only the answer is shared.
    let address = format!("{base}{path}");
    let carried = console::encode(&raw);
    let mut response = match method.as_str() {
        "GET" => agent
            .get(&address)
            .header("Zetlyn-Grant", &carried)
            .header("Zetlyn-Date", &at)
            .header("Zetlyn-Signature", &signature)
            .call(),
        "POST" | "PUT" => {
            let builder = if method == "POST" {
                agent.post(&address)
            } else {
                agent.put(&address)
            };
            builder
                .header("Zetlyn-Grant", &carried)
                .header("Zetlyn-Date", &at)
                .header("Zetlyn-Signature", &signature)
                .header("Content-Type", "text/plain")
                .send(&body[..])
        }
        other => return Err(format!("{other}: a console answers GET, POST and PUT")),
    }
    .map_err(|e| format!("{address}: {e}"))?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("{address}: {e}"))?;
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(j) => println!("{}", serde_json::to_string_pretty(&j).unwrap_or(text)),
        Err(_) => println!("{text}"),
    }
    if status >= 400 {
        return Err(format!("{status} from {address}"));
    }
    Ok(())
}

/// `http://host:port/a/b` into the two halves a signature is over separately.
fn split(url: &str) -> Result<(String, String), String> {
    let after = url
        .find("://")
        .map(|i| i + 3)
        .ok_or_else(|| format!("{url}: an address with a scheme"))?;
    match url[after..].find('/') {
        Some(i) => Ok((url[..after + i].to_string(), url[after + i..].to_string())),
        None => Ok((url.to_string(), "/".to_string())),
    }
}

const CONSOLE_USAGE: &str = "\
  zetlyn console serve <deployment> [--port 8100]
      The deployment answering for itself. It holds no secret: every call has to trace back
      to a grant this operator signed.

  zetlyn console key [--at <dir>] [--name operator.key]
      The key in a directory. Makes one where there is none, and prints the public half.

  zetlyn console grant --to <key> --can read,run,apply --until <date> [--at <deployment>]
      What the operator signs. Not a secret, and not a way in on its own.

  zetlyn console call <url> [--method GET] [--body <file>] --grant <file> [--key <dir>]
      One signed call. This is the whole of what a platform does, in one command.
";

// ---------------------------------------------------------------------------------------------
// Who you are, where that has to be the same person twice.

fn id_command(args: &[String]) -> Result<(), String> {
    match args.get(1).map(String::as_str) {
        Some("new") => {
            let name = flag(args, "--name").unwrap_or("");
            let contact = flag(args, "--contact").unwrap_or("");
            let public = identity::new(name, contact)?;
            println!("{public}");
            println!("\nThat is you, everywhere you act: publishing to a hub, operating a");
            println!(
                "deployment, driving a console. The private half is in {}/{},",
                identity::home().display(),
                identity::KEY_FILE
            );
            println!("readable by you and nobody else, and it is the one file worth backing up.");
            println!("\nReaders are not this. A person who subscribes to a scope is an email");
            println!("address in that deployment, and has no key.");
            Ok(())
        }
        _ => {
            let Some(public) = identity::key() else {
                return Err(
                    "no identity yet. `zetlyn id new --name … --contact …` makes one".into(),
                );
            };
            let who = identity::read();
            println!("{public}");
            if !who.name.is_empty() {
                println!("{}", who.name);
            }
            if !who.contact.is_empty() {
                println!("{}", who.contact);
            }
            println!("\nin {}", identity::home().display());
            Ok(())
        }
    }
}
