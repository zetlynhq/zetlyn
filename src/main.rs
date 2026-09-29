//! zetlyn — a source is served, browsed and searched on its own. A tracker puts several of them on
//! one page; it is not what makes them usable.

mod account;
mod app;
mod assist;
mod artifact;
mod billing;
mod build;
mod console;
mod source;
mod sql;
mod sourcedecl;
mod expr;
mod fetch;
mod grant;
pub mod guess;
mod hub;
mod hook;
mod identity;
mod key;
mod place;
mod platform;
mod mail;
mod matches;
mod migrate;
mod claim;
mod remote;
mod tracker;
mod trackerdecl;
mod serve;
mod servetracker;
mod rows;
mod schemes;
mod store;
mod teach;
mod thingquery;
mod thingstore;
mod watch;
mod yaml;

use std::path::{Path, PathBuf};

use source::Source;

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

  zetlyn [<workspace>] [--port 4747] [--no-open]
      The workspace in a browser, for the person at the machine: a tracker from a first source
      and a second, with what they share shown before they are connected.

  zetlyn source new --from <path or URL> [--at <dir>] [--name owner/name] [--kind <word>]
      Reads a folder, a .csv or an .xlsx, guesses the identifier, the title, the text and the
      property types, writes <dir>/source.yaml, and prints the first three claims it would make.

  zetlyn source new --from github:advisories | github:<owner>/<repo>[/releases|/advisories]
      GitHub, written once: the Advisory Database, a repository's releases or advisories, or the
      files of its checkout. ${GITHUB_TOKEN?} lifts the limit of sixty calls an hour.

  zetlyn assist [key anthropic|openai]
      Who the assist asks here; `key` keeps a key from standard input. workspace.yaml's
      `assist: { provider, model, url, off }` or ZETLYN_ASSIST_URL / ZETLYN_ASSIST_MODEL name an
      OpenAI-compatible endpoint instead of Claude.

  zetlyn assist teach <URL> --at <dir> [--name owner/name] [--send]
      A JSON API: its answer outlined, a declaration proposed, tried against the API, mended
      once if the try complains. Nothing is sent before --send; what was sent is kept in
      <dir>/assist.yaml.

  zetlyn assist align <tracker> <property> [--send] [--apply]
      Which words of the tracker's sources mean the same thing, and how many disagreements that
      takes away. --apply writes the entry into tracker.yaml and keeps its comments.

  zetlyn assist why <tracker> <source dir> [--send]
  zetlyn assist mend <source dir> [--send] [--apply]
      One sentence on why a source is in a tracker; a declaration mended from what its updates
      complained about, tried before it is shown.

  zetlyn assist ask <tracker> <question in words> [--send]
      The question as the filters it becomes, and how many things they hold for. It never
      answers in words: a question that is not a filter is refused, and so is a filter that
      names a source, a property or a kind the tracker does not have.

  zetlyn assist score <proposed dir> <reference dir>
      A proposal against a declaration a person wrote: identifier, title, date, typed properties.

  zetlyn source update <dir> [--reread | --from-start]
      Fills the store. Says what was added, changed, removed and unchanged. `--reread` reads the
      source even where it says nothing changed, once, to take the receipts of claims held before.
      `--from-start` also reads from the beginning of what it covers, as the first update did.

  zetlyn source describe <dir>
      What this source is, what it holds, what it can be asked. JSON.

  zetlyn claim <dir> <identifier or claim id>
      One claim, the words its source used for it, and every version it was at.

  zetlyn search <dir> <query> [--view <name>] [--limit <n>]
      Free text and comparisons mixed: `log4j severity=high known>2026-01-01`.

  zetlyn tracker describe <dir> [--sources <dir>]
  zetlyn tracker search <dir> <query> [--view <name>] [--kind <word>] [--limit <n>]
  zetlyn tracker measure <dir>
  zetlyn tracker match <tracker> <thing> <relation> <target> --by <who> [--why …] [--withdraw]
      A match a person makes where no claim states it: kept, signed, in matches.jsonl beside the
      tracker, and withdrawn by a later line rather than deleted.

  zetlyn tracker refresh <dir> [--rebuild]
  zetlyn tracker things <dir> <question>
      conflict:severity and has:kev, nvd.severity=critical, only:nvd, appeared:exploit<7d
  zetlyn tracker conflicts <dir>
  zetlyn tracker signals <dir> [--since <id>]
      A tracker holds no index. It rewrites the query per source, fans out, merges ranked
      lists, and gathers the claims into one thing per identifier.

  zetlyn tracker publish <dir> [--to <hub>] [--tag latest]
  zetlyn tracker subscribe <reference> [--from <hub>] [--at <workspace>]
      A tracker travels as its statement. Subscribing takes the statement and every source it
      names, and rebuilds the stores here.

  zetlyn account [list] <workspace>
  zetlyn account grant --email <a> [--days 31] [--trackers a,b] <workspace>
  zetlyn account cancel --email <a> <workspace>
  zetlyn account key --email <a> [--name <what for>] <workspace>
  zetlyn account curator --email <a> [--revoke] <workspace>
  zetlyn account dunning [--within 7] [--send] <workspace>
      The first customers arrive before a payment provider does, and are set by hand.

  zetlyn serve <dir> [--port 8080] [--addr 0.0.0.0:8080] [--base /owner/name]
      Overview, views, browse with facets and columns, search, a thing, a claim.

  zetlyn source publish <dir> [--to <hub>] [--tag latest]
  zetlyn source subscribe <reference> [--from <hub>] [--at <dir>] [--key ed25519:…]
  zetlyn source pull <dir>
      A hub is a folder, a mount, s3://bucket/prefix or an address. Named nowhere, it is
      hub.zetlyn.com; a reference that carries a host means that host. What travels is the
      claims, so a subscriber needs none of the publisher's credentials.

  zetlyn run <workspace>
  zetlyn watch [list | check [--deliver]] <workspace>
      Every source that is due, updated, and every watch replayed.

  zetlyn migrate [<workspace>]
      A workspace written before 0.2, in the words and the format of 0.2.

  zetlyn id [new --name <n> --contact <c>]
      One key, everywhere you act. Readers are not this and hold none.

  zetlyn console serve <workspace> | grant | call
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
        Some("source") => match args.get(1).map(String::as_str) {
            Some("new") => dataset_new(args),
            Some("update") => dataset_run(args),
            Some("publish") => dataset_publish(args),
            Some("subscribe") => dataset_subscribe(args),
            Some("pull") => dataset_update(args),
            Some("check") => {
                let ds = Source::open(&dir_at(args, 2)?)?;
                let wrong = ds.check();
                for w in &wrong {
                    println!("{}: {w}", ds.decl.name);
                }
                if wrong.is_empty() {
                    println!("{}: nothing it claims is untrue", ds.decl.name);
                }
                // Non-zero, so a check in CI stops the build that would publish it.
                if wrong.is_empty() { Ok(()) } else { Err(format!("{} untrue", wrong.len())) }
            }
            Some("describe") => {
                let dir = dir_at(args, 2)?;
                let ds = Source::open(&dir)?;
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
        Some("dataset") => Err("`zetlyn dataset` is `zetlyn source` since 0.2, and `run` is `update`".into()),
        Some("scope") => Err("`zetlyn scope` is `zetlyn tracker` since 0.2".into()),
        // One claim with its receipt and every version it was at. By identifier, or by claim id.
        Some("claim") => {
            let rest = positional(args, 1);
            let dir = PathBuf::from(rest.first().ok_or("which source directory?")?.as_str());
            let wanted = rest.get(1).ok_or("which claim? an identifier or a claim id")?;
            let ds = Source::open(&dir)?;
            let mut ids = vec![wanted.to_string()];
            if ds.store.get(wanted).is_none() {
                let q = source::Query {
                    text: String::new(),
                    pred: None,
                    ids: vec![wanted.to_string()],
                    seen_before: None,
                    view: None,
                    sort: None,
                    limit: 50,
                    offset: 0,
                };
                ids = ds.search(&q)?.1.into_iter().map(|h| h.record_id).collect();
            }
            let claims = ds.fetch(&ids, true);
            if claims.is_empty() {
                return Err(format!("{wanted}: no claim here says that"));
            }
            for c in claims {
                println!("{}", serde_json::to_string_pretty(&c.to_json()).unwrap_or_default());
            }
            Ok(())
        }
        Some("search") => search(args),
        Some("changes") => changes(args),
        Some("run") => schedule(args),
        Some("watch") => watch_cmd(args),
        Some("account") => account_cmd(args),
        Some("tracker") => match args.get(1).map(String::as_str) {
            Some("publish") => scope_publish(args),
            Some("subscribe") => scope_subscribe(args),
            Some("describe") => {
                let (dir, datasets) = scope_at(args, 2)?;
                let scope = tracker::Tracker::open(&dir, &datasets)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&scope.describe()).unwrap_or_default()
                );
                Ok(())
            }
            Some("search") => scope_search(args),
            Some("check") => {
                let (dir, datasets) = scope_at(args, 2)?;
                let scope = tracker::Tracker::open(&dir, &datasets)?;
                let wrong = scope.check();
                for w in &wrong {
                    println!("{}: {w}", scope.decl.name);
                }
                if wrong.is_empty() {
                    println!("{}: nothing it claims is untrue", scope.decl.name);
                }
                // Non-zero, so a check in CI stops the build that would publish it.
                if wrong.is_empty() { Ok(()) } else { Err(format!("{} untrue", wrong.len())) }
            }
            // A person's match: this thing is `relation` that, said by them.
            Some("match") => {
                let (dir, datasets) = scope_at(args, 2)?;
                let p = positional(args, 2);
                let (key, relation, target) = match (p.get(1), p.get(2), p.get(3)) {
                    (Some(k), Some(r), Some(t)) => (k.to_string(), r.to_string(), t.to_lowercase()),
                    _ => return Err("tracker match <tracker> <thing> <relation> <target> --by <who> [--why …] [--withdraw]".into()),
                };
                let scope = tracker::Tracker::open(&dir, &datasets)?;
                if !scope.decl.relations.iter().any(|r| r.name == relation) {
                    let names: Vec<&str> = scope.decl.relations.iter().map(|r| r.name.as_str()).collect();
                    return Err(format!("{relation}: this tracker's relations are {}", if names.is_empty() { "none".to_string() } else { names.join(", ") }));
                }
                let scheme = scope.decl.join.first().cloned().unwrap_or_default();
                // `cve:CVE-2026-1` names its scheme; `CVE-2026-1` is the tracker's own.
                let key = match key.split_once(':') {
                    Some((s, v)) => crate::schemes::key(s, v),
                    None => crate::schemes::key(&scheme, &key),
                };
                let m = matches::Match {
                    at: crate::iso_stamp(crate::now()),
                    by: flag(args, "--by").unwrap_or("").to_string(),
                    key,
                    relation,
                    target,
                    why: flag(args, "--why").unwrap_or("").to_string(),
                    withdrawn: args.iter().any(|a| a == "--withdraw"),
                };
                matches::record(&dir, &m)?;
                scope.refresh(false)?;
                println!("{} {} {} {}, by {}", m.key, m.relation, m.target, if m.withdrawn { "withdrawn" } else { "kept" }, m.by);
                Ok(())
            }
            // The tracker's own store: things, conflicts and what changed. `--rebuild` makes it
            // again from the sources and writes no signals.
            Some("refresh") => {
                let (dir, sources) = scope_at(args, 2)?;
                let t = tracker::Tracker::open(&dir, &sources)?;
                let started = std::time::Instant::now();
                let r = t.refresh(args.iter().any(|a| a == "--rebuild"))?;
                println!(
                    "{} things, {} conflicts, {} that differ only in wording, {} signals{} in {:.1}s",
                    r.things,
                    r.conflicts,
                    r.wording,
                    r.signals,
                    if r.first { " (the first look, so none)" } else { "" },
                    started.elapsed().as_secs_f64()
                );
                Ok(())
            }
            // Which things a question holds for, asked of the tracker's store.
            Some("things") => {
                let (dir, sources) = scope_at(args, 2)?;
                let t = tracker::Tracker::open(&dir, &sources)?;
                let question = positional(args, 2).iter().skip(1).map(|s| s.as_str()).collect::<Vec<_>>().join(" ");
                let cx = t.context();
                let q = thingquery::parse(&question, &cx)?;
                let store = thingstore::ThingStore::open(&dir)?;
                // A reader that stops reading (`| head`) is an ending, not a failure.
                use std::io::Write;
                let mut out = std::io::stdout().lock();
                for key in store.matching(&q, &cx)? {
                    if writeln!(out, "{key}").is_err() {
                        break;
                    }
                }
                Ok(())
            }
            Some("conflicts") => {
                let (dir, _) = scope_at(args, 2)?;
                let store = thingstore::ThingStore::open(&dir)?;
                let limit = flag(args, "--limit").and_then(|s| s.parse().ok()).unwrap_or(1000);
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!(store.conflicts(None, limit)))
                        .unwrap_or_default()
                );
                Ok(())
            }
            Some("signals") => {
                let (dir, _) = scope_at(args, 2)?;
                let store = thingstore::ThingStore::open(&dir)?;
                let since = flag(args, "--since").and_then(|s| s.parse().ok());
                let limit = flag(args, "--limit").and_then(|s| s.parse().ok()).unwrap_or(1000);
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!(store.signals(since, limit)))
                        .unwrap_or_default()
                );
                Ok(())
            }
            Some("measure") => {
                let (dir, datasets) = scope_at(args, 2)?;
                let scope = tracker::Tracker::open(&dir, &datasets)?;
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
        // Everything 0.1 wrote, in the words and the format of 0.2.
        Some("migrate") => {
            let root = PathBuf::from(positional(args, 1).first().map(|s| s.as_str()).unwrap_or("."));
            let done = migrate::workspace(&root)?;
            for line in &done.0 {
                println!("{line}");
            }
            if let Some(line) = migrate::identity(&identity::home())? {
                println!("{line}");
            }
            Ok(())
        }
        // One command, and the directory says which it is.
        Some("console") => console_command(args),
        Some("hub") => hub_command(args),
        Some("assist") => assist_command(args),
        Some("billing") => billing::command(args),
        Some("host") => app::host(args),
        Some("id") => id_command(args),
        Some("platform") => platform_command(args),
        Some("serve") => {
            let port = flag(args, "--port").unwrap_or("8080");
            // Several trackers can sit on one host, one process each, so a surface is told where
            // it hangs and writes every address it gives out under that.
            serve::mount(flag(args, "--base").unwrap_or(""));
            // Loopback unless asked otherwise: a tracker reachable from the network is a decision
            // an operator makes, not a default they discover.
            let addr = flag(args, "--addr")
                .map(str::to_string)
                .unwrap_or_else(|| format!("127.0.0.1:{port}"));
            let named = positional(args, 1)
                .first()
                .map(|s| PathBuf::from(s.as_str()))
                .ok_or("which directory?")?;
            if named.join(crate::trackerdecl::FILE).exists() {
                let (dir, datasets) = scope_at(args, 1)?;
                let scope = tracker::Tracker::open(&dir, &datasets)?;
                for name in &scope.missing {
                    eprintln!(
                        "zetlyn: {name} is not installed here, and the tracker opens without it"
                    );
                }
                return servetracker::serve(scope, &dir, &datasets, &addr);
            }
            let dir = dir_at(args, 1)?;
            let ds = Source::open(&dir)?;
            if ds.store.count() == 0 {
                eprintln!(
                    "zetlyn: the store is empty. `zetlyn source update {}` first.",
                    dir.display()
                );
            }
            serve::serve(ds, &addr)
        }
        // A published artifact names the build that made it, so the build has to name itself.
        // Nothing asked for: the workspace, in a browser.
        None | Some("app") => app::run(args),
        // `zetlyn ~/Zetlyn`: a directory on its own is a workspace to open.
        Some(p) if !p.starts_with('-') && std::path::Path::new(p).is_dir() => {
            let with: Vec<String> = std::iter::once("app".to_string()).chain(args.iter().cloned()).collect();
            app::run(&with)
        }
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
        .ok_or("which source directory?")?;
    if !p.join(crate::sourcedecl::FILE).exists() {
        return Err(missing(&p.join(crate::sourcedecl::FILE)));
    }
    Ok(p)
}

fn dataset_new(args: &[String]) -> Result<(), String> {
    let from = flag(args, "--from").ok_or("--from <path> or a URL is required")?;
    if from.starts_with("github:") {
        let stem = guess::slug(from.trim_start_matches("github:").rsplit('/').next().unwrap_or("github"));
        let dir = PathBuf::from(flag(args, "--at").map(str::to_string).unwrap_or(format!("./{stem}")));
        let proposed = guess::propose_github(from, &dir, flag(args, "--name"))?;
        println!("{}\n", dir.join(crate::sourcedecl::FILE).display());
        print!("{proposed}");
        return Ok(());
    }
    if from.starts_with("http://") || from.starts_with("https://") {
        let stem = from
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .map(|s| guess::slug(s.split('?').next().unwrap_or(s)))
            .unwrap_or_else(|| "source".into());
        let dir = PathBuf::from(
            flag(args, "--at")
                .map(str::to_string)
                .unwrap_or(format!("./{stem}")),
        );
        let proposed = guess::propose_url(from, &dir, flag(args, "--name"), flag(args, "--kind"))?;
        println!("{}\n", dir.join(crate::sourcedecl::FILE).display());
        print!("{proposed}");
        return Ok(());
    }
    let from = Path::new(from);
    let stem = from
        .file_stem()
        .map(|s| guess::slug(&s.to_string_lossy()))
        .unwrap_or_else(|| "source".into());
    let dir = PathBuf::from(
        flag(args, "--at")
            .map(str::to_string)
            .unwrap_or(format!("./{stem}")),
    );
    let proposed = guess::propose(from, &dir, flag(args, "--name"), flag(args, "--kind"))?;

    println!("{}", dir.join(crate::sourcedecl::FILE).display());
    println!();
    print!("{proposed}");
    println!();

    // Three claims, because a creator who agrees changes nothing and a creator who does not needs
    // to see why before a run writes anything.
    let ds = Source::open(&dir)?;
    let root = ds.decl.source.root(&dir);
    let mut notes = build::Notes::default();
    let mut shown = 0usize;
    let _ = rows::each_row(&ds.decl, &dir, &root, None, |produced: rows::Produced| {
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
    let ds = Source::open(&dir)?;
    let started = std::time::Instant::now();
    let r = ds.run_with(
        args.iter().any(|a| a == "--reread"),
        args.iter().any(|a| a == "--from-start"),
    )?;
    println!(
        "update {} {} in {:.1}s: +{} ~{} −{} ={}",
        r.id,
        if r.complete { "complete" } else { "partial" },
        started.elapsed().as_secs_f64(),
        r.added,
        r.changed,
        r.removed,
        r.unchanged
    );
    if r.no_text > 0 {
        println!("  {} claims with no text", r.no_text);
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
            "  {} claims were read and the store was not replaced: {why}",
            r.unchanged
        );
        println!("  nothing was written, and the last complete update still stands");
    }
    if let Some(e) = &r.error {
        println!("  the update did not finish: {e}");
        println!("  nothing was removed, because a partial update has not seen the source");
    }
    Ok(())
}

fn changes(args: &[String]) -> Result<(), String> {
    let rest = positional(args, 1);
    let dir = PathBuf::from(rest.first().ok_or("which directory?")?.as_str());
    let limit = flag(args, "--limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);
    let report = if dir.join(crate::trackerdecl::FILE).exists() {
        let (dir, datasets) = scope_at(args, 1)?;
        let scope = tracker::Tracker::open(&dir, &datasets)?;
        let since = flag(args, "--since")
            .map(str::to_string)
            .unwrap_or_else(|| scope.mark_before().to_string());
        scope.changes(&since, limit)
    } else {
        let ds = Source::open(&dir_at(args, 1)?)?;
        let since = flag(args, "--since")
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| (ds.mark() - 1).max(0));
        source::Interface::changes(&ds, since, limit)
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&report).unwrap_or_default()
    );
    Ok(())
}

/// When this source is next due, from its declared cadence and when it last finished.
fn due_at(ds: &Source) -> Option<i64> {
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
    if !root.join("sources").is_dir() {
        return Err(format!("{}: no sources here", root.display()));
    }
    Ok(root)
}

/// The scheduler. Runs what is due, then replays every watch.
fn schedule(args: &[String]) -> Result<(), String> {
    let root = deployment(args, 1)?;
    let once = args.iter().any(|a| a == "--once");
    let deliver = !args.iter().any(|a| a == "--no-deliver");
    loop {
        let soonest = schedule_pass(&root, deliver, &Limits::default());
        if once {
            return Ok(());
        }
        // A minute at least, an hour at most: the promise is checked on the same tick.
        let wait = soonest.map(|s| (s - now()).clamp(60, 3600)).unwrap_or(3600);
        println!("next in {wait}s");
        std::thread::sleep(std::time::Duration::from_secs(wait as u64));
    }
}

/// What a hosted workspace's plan allows: how many sources it updates, and how often at most.
/// Nothing is limited for a workspace somebody runs themselves.
#[derive(Debug, Default, Clone)]
pub struct Limits {
    pub sources: Option<usize>,
    pub every: i64,
}

/// One pass: every source that is due runs, the trackers whose sources moved look again, and every
/// watch is asked. When the next source is due, if any is.
pub fn schedule_pass(root: &Path, deliver: bool, limits: &Limits) -> Option<i64> {
    let root = root.to_path_buf();
    {
        let tick = now();
        let mut soonest: Option<i64> = None;
        let mut moved: Vec<String> = Vec::new();
        // The runs happen one after another in one process, so the order matters: a source
        // that is being throttled can take twenty minutes, and an hourly source behind it
        // would wait that out. Shortest cadence first, so what is asked for most often is
        // asked for first.
        let mut due: Vec<(i64, String, PathBuf)> = tracker::registry(&root.join("sources"))
            .into_iter()
            .filter_map(|(name, dir)| {
                let every = Source::open(&dir)
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
        // A plan's sources, in the order they are asked for; the rest are said, not run.
        if let Some(n) = limits.sources {
            for (_, name, _) in due.iter().skip(n) {
                eprintln!("{name}: not updated, the plan updates {n} sources");
            }
            due.truncate(n);
        }
        for (_, name, dir) in due {
            let ds = match Source::open(&dir) {
                Ok(ds) => ds,
                Err(e) => {
                    eprintln!("{name}: {e}");
                    continue;
                }
            };
            let Some(due) = due_at(&ds) else { continue };
            // And no more often than the plan allows, whatever the source asks for.
            let due = match last_finished(&ds) {
                Some(at) if limits.every > 0 => due.max(at + limits.every),
                _ => due,
            };
            if due > tick {
                soonest = Some(soonest.map_or(due, |s: i64| s.min(due)));
                continue;
            }
            match ds.run() {
                Ok(r) => {
                    println!(
                        "{} update {} {}: +{} ~{} −{} ={}{}",
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
                    );
                    // What follows this one has nothing to do until it has found something.
                    if r.added + r.changed + r.removed > 0 {
                        moved.push(name.clone());
                    }
                }
                Err(e) => eprintln!("{name}: {e}"),
            }
            if let Ok(ds) = Source::open(&dir) {
                if let Some(next) = due_at(&ds) {
                    soonest = Some(soonest.map_or(next, |s: i64| s.min(next)));
                }
            }
        }

        // A source that takes its things from another has nothing to ask about until that one
        // has found something new. `models/hf` asks Hugging Face about the models `models/gguf`
        // names: 2,684 calls to be told what it already holds, or none.
        for (name, dir) in follows(&root, &moved) {
            let ds = match Source::open(&dir) {
                Ok(ds) => ds,
                Err(e) => {
                    eprintln!("{name}: {e}");
                    continue;
                }
            };
            match ds.run() {
                Ok(r) => println!(
                    "{name} update {} {} after {}: +{} ~{} −{} ={}",
                    r.id,
                    if r.complete { "complete" } else { "partial" },
                    ds.decl.source.after().unwrap_or(""),
                    r.added,
                    r.changed,
                    r.removed,
                    r.unchanged
                ),
                Err(e) => eprintln!("{name}: {e}"),
            }
        }

        // Every tracker whose sources moved looks again, so what changed is a signal before a
        // watch is asked about it.
        for dir in tracker::scope_registry(&root.join("trackers")).values() {
            match tracker::Tracker::open(dir, &root.join("sources")).and_then(|t| t.refresh_if_moved()) {
                Ok(Some(r)) => println!("{}: {} signals, {} conflicts", dir.display(), r.signals, r.conflicts),
                Ok(None) => {}
                Err(e) => eprintln!("{}: {e}", dir.display()),
            }
        }

        for w in watch::all(&root) {
            match w.check(&root) {
                Ok((report, mark)) => {
                    let n = report["signals"].as_array().or_else(|| report["things"].as_array()).map(Vec::len).unwrap_or(0);
                    if !deliver {
                        if n > 0 {
                            println!("{}: {n} to tell, not delivered", w.decl.name);
                        }
                        continue;
                    }
                    match w.deliver(&report, &mark) {
                        Ok(to) if n > 0 => println!("{}: {n} told → {}", w.decl.name, to.join(", ")),
                        Ok(_) => {}
                        Err(e) => eprintln!("{}: {e}", w.decl.name),
                    }
                }
                Err(e) => eprintln!("{}: {e}", w.decl.name),
            }
        }

        soonest
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
                let n = report["signals"].as_array().or_else(|| report["things"].as_array()).map(Vec::len).unwrap_or(0);
                println!("{}: {n} to tell since {}", w.decl.name, report["since"]);
                // Delivered, or at least remembered: a look that told nothing still moves the mark
                // and keeps what a view held.
                if deliver {
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
            let scopes: Vec<String> = flag(args, "--trackers")
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
        .ok_or("which tracker directory?")?;
    if !p.join(crate::trackerdecl::FILE).exists() {
        return Err(missing(&p.join(crate::trackerdecl::FILE)));
    }
    // A workspace holds `sources/` beside `trackers/`, and a source is found there by its name.
    let datasets = match flag(args, "--sources") {
        Some(d) => PathBuf::from(d),
        None => p.join("..").join("..").join("sources"),
    };
    if !datasets.is_dir() {
        return Err(format!(
            "{}: no sources here. Name one with --sources <dir>",
            datasets.display()
        ));
    }
    Ok((p, datasets))
}

fn scope_search(args: &[String]) -> Result<(), String> {
    let (dir, datasets) = scope_at(args, 2)?;
    let scope = tracker::Tracker::open(&dir, &datasets)?;
    let rest = positional(args, 2);
    let terms: Vec<String> = rest.iter().skip(1).map(|s| s.to_string()).collect();
    let (text, pred) = expr::parse_query(&terms.join(" "));
    let q = tracker::TrackerQuery {
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
        println!("missing source: {name}");
    }
    for (member, reasons) in &answer.unanswered {
        println!("{member} did not answer: {}", reasons.join("; "));
    }
    println!(
        "{} things over {} claims, {} of {} sources answered",
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
                    let v = v.join(", ");
                    let raw = view.by.get(m).cloned().unwrap_or_default().join(", ");
                    if raw == v {
                        format!("{m}={v}")
                    } else {
                        format!("{m}={raw}→{v}")
                    }
                })
                .collect();
            println!(
                "     {name}{}: {}",
                if view.divergent { " (conflict)" } else { "" },
                shown.join(", ")
            );
        }
    }
    Ok(())
}

fn search(args: &[String]) -> Result<(), String> {
    let rest = positional(args, 1);
    let dir = PathBuf::from(rest.first().ok_or("which source directory?")?.as_str());
    let terms: Vec<String> = rest.iter().skip(1).map(|s| s.to_string()).collect();
    let ds = Source::open(&dir)?;
    let (text, pred) = expr::parse_query(&terms.join(" "));
    let q = source::Query {
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
    println!("{total} claims");
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

/// `zetlyn source publish <dir> [--to <hub>] [--tag latest] [--expect <version>|-]`
fn dataset_publish(args: &[String]) -> Result<(), String> {
    let dir = dir_at(args, 2)?;
    let to = flag(args, "--to").unwrap_or(artifact::DEFAULT_HUB);
    let tag = flag(args, "--tag").unwrap_or("latest");
    let ds = Source::open(&dir)?;
    // Nothing is published that the source itself says is untrue.
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
        "{}@{tag} is {version}, {} claims, at {}",
        ds.decl.name,
        ds.store.count(),
        place.describe()
    );
    Ok(())
}

/// `zetlyn source subscribe <reference> --from <hub> [--at <dir>]`
fn dataset_subscribe(args: &[String]) -> Result<(), String> {
    let raw = positional(args, 2)
        .first()
        .map(|s| s.to_string())
        .ok_or("which source? owner/name, with an optional @tag")?;
    let reference = artifact::Reference::parse(&raw)?;
    let from = flag(args, "--from")
        .map(str::to_string)
        .or_else(|| reference.host.as_ref().map(|h| format!("https://{h}")))
        .unwrap_or_else(|| artifact::DEFAULT_HUB.to_string());
    let into = match flag(args, "--at") {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from("sources").join(&reference.name),
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
        "{reference} is {version}, {held} claims, in {}",
        into.display()
    );
    Ok(())
}

/// `zetlyn source update <dir>`: ask the hub this one came from whether there is a newer version.
fn dataset_update(args: &[String]) -> Result<(), String> {
    let dir = dir_at(args, 2)?;
    let decl = sourcedecl::SourceDecl::load(&dir)?;
    let sourcedecl::Fetch::Hub {
        at, reference, key, ..
    } = &decl.source
    else {
        return Err(format!(
            "{} is not subscribed. `zetlyn source update` fills it from where it fetches",
            decl.name
        ));
    };
    let pinned = Some(key.as_str()).filter(|k| !k.trim().is_empty());
    let reference = artifact::Reference::parse(reference)?;
    let place = place::at(at)?;
    let manifest = artifact::manifest_signed_by(place.as_ref(), &reference, "sources", pinned)?;
    let offered = manifest["version"].as_str().unwrap_or_default();
    let held = artifact::held_version(&dir).unwrap_or_default();
    if artifact::take_statement(&dir, &manifest)? {
        println!("{reference}: the publisher's licence and terms, taken");
    }
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
    println!("{reference} {held} → {version}, {n} claims, whole");
    Ok(())
}

/// `zetlyn tracker publish <dir> [--to <hub>] [--tag latest] [--expect <version>]`
fn scope_publish(args: &[String]) -> Result<(), String> {
    let (dir, datasets) = scope_at(args, 2)?;
    let to = flag(args, "--to").unwrap_or(artifact::DEFAULT_HUB);
    let tag = flag(args, "--tag").unwrap_or("latest");
    // A tracker that does not hold together is not published, for the same reason a source is not.
    let scope = tracker::Tracker::open(&dir, &datasets)?;
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
        "{}@{tag} is {version}, {} sources, at {}",
        scope.decl.name,
        scope.members.len(),
        place.describe()
    );
    Ok(())
}

/// `zetlyn tracker subscribe <reference> [--from <hub>] [--at <workspace>]`
fn scope_subscribe(args: &[String]) -> Result<(), String> {
    let raw = positional(args, 2)
        .first()
        .map(|s| s.to_string())
        .ok_or("which tracker? owner/name, with an optional @tag")?;
    let reference = artifact::Reference::parse(&raw)?;
    let from = flag(args, "--from")
        .map(str::to_string)
        .or_else(|| reference.host.as_ref().map(|h| format!("https://{h}")))
        .unwrap_or_else(|| artifact::DEFAULT_HUB.to_string());
    let root = PathBuf::from(flag(args, "--at").unwrap_or("."));
    let into = root.join("trackers").join(&reference.name);
    let datasets = root.join("sources");
    let place = place::at(&from)?;
    let (version, taken) =
        artifact::subscribe_scope(place.as_ref(), &reference, &into, &datasets, &from)?;
    println!("{reference} is {version}, in {}", into.display());
    for name in &taken {
        println!("  took {name}");
    }
    if taken.is_empty() {
        println!("  every source was held already");
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
            println!("under sources/{name}/ and trackers/{name}/ and nowhere else.");
            Ok(())
        }
        // The key a publisher signs with. It lives in the source directory and is read by
        // `source publish`; there is nothing to turn on.
        Some("key") => {
            let dir = PathBuf::from(flag(args, "--at").unwrap_or("."));
            if let Some(held) = identity::or_local(&dir, artifact::KEY_FILE) {
                println!("{held}");
                println!("\nThat is the public half. Give it to subscribers; they pin it with");
                println!("`zetlyn source subscribe … --key {held}`.");
                return Ok(());
            }
            let public = artifact::new_key(&dir)?;
            println!("{public}");
            println!(
                "\nThe private half is in {}/{}, readable by you and nobody else.",
                dir.display(),
                artifact::KEY_FILE
            );
            println!("Do not publish this source from a second machine with a second key: every");
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
        Some("platform") => platform_command(args),
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
            // Which of the trackers this hub carries are also served on this host, so the front
            // page can link them. The hub itself still answers nothing about them.
            let serving: Vec<String> = flag(args, "--serving")
                .unwrap_or("")
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            hub::serve(&dir, &addr, &serving)
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

  zetlyn hub serve <dir> [--port 8090] [--addr 127.0.0.1:8090] [--serving owner/name,…]
      GET for anybody. PUT signed by a key that holds the owner named in the path.
      A folder, a mount and a private bucket need none of this.
";

// ---------------------------------------------------------------------------------------------
// A workspace answering for itself, and whoever is allowed to ask.

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
            let out = PathBuf::from(flag(args, "--out").unwrap_or("grant.yaml"));
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

/// A workspace is its directory, so it is called what the directory is called.
fn deployment_name(root: &Path) -> String {
    root.canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "workspace".into())
}

/// The smallest thing that can drive a console, which is what a platform will be a larger one of.
fn console_call(args: &[String]) -> Result<(), String> {
    let url = positional(args, 2)
        .first()
        .map(|s| s.to_string())
        .ok_or("which address? http://host:port/source/kev")?;
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

  zetlyn console grant --to <key> --can read,update,apply --until <date> [--at <workspace>]
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
                "workspace, driving a console. The private half is in {}/{},",
                identity::home().display(),
                identity::KEY_FILE
            );
            println!("readable by you and nobody else, and it is the one file worth backing up.");
            println!("\nReaders are not this. A person who subscribes to a tracker is an email");
            println!("address in that workspace, and has no key.");
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

// ---------------------------------------------------------------------------------------------
// Zetlyn run for somebody instead of by them.

fn platform_command(args: &[String]) -> Result<(), String> {
    match args.get(1).map(String::as_str) {
        Some("hold") => {
            let root = PathBuf::from(flag(args, "--at").unwrap_or("."));
            let name = flag(args, "--name").ok_or("--name what shall it be called here?")?;
            let address = flag(args, "--console").ok_or("--console where does it answer?")?;
            let from = PathBuf::from(flag(args, "--grant").ok_or("--grant which file?")?);
            if name.contains('/') || name.contains("..") {
                return Err(format!("{name}: a name, not a path"));
            }
            let dir = root.join("workspaces");
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let grants = root.join("grants");
            std::fs::create_dir_all(&grants).map_err(|e| format!("{}: {e}", grants.display()))?;
            let grant_file = format!("{name}.yaml");
            std::fs::copy(&from, grants.join(&grant_file))
                .map_err(|e| format!("{}: {e}", from.display()))?;
            let held = platform::Held {
                at: address.trim_end_matches('/').to_string(),
                grant: grant_file,
                title: flag(args, "--title").unwrap_or("").to_string(),
            };
            crate::yaml::write(&dir.join(format!("{name}.yaml")), &held)?;

            // Held against the thing itself rather than against the file that was handed over.
            let platform = platform::Platform::open(&root)?;
            match platform.driver(name).and_then(|d| d.ask("GET", "/", b"")) {
                Ok(j) => {
                    println!(
                        "{name} answers: {} sources, {} trackers",
                        j["sources"].as_array().map(Vec::len).unwrap_or(0),
                        j["trackers"].as_array().map(Vec::len).unwrap_or(0)
                    );
                    Ok(())
                }
                Err(e) => Err(format!("held, and it did not answer: {e}")),
            }
        }
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
                _ => "127.0.0.1:8110".to_string(),
            };
            platform::serve(&root, &addr)
        }
        _ => {
            print!("{PLATFORM_USAGE}");
            Ok(())
        }
    }
}

const PLATFORM_USAGE: &str = "\
  zetlyn platform hold --name <n> --console <url> --grant <file> [--at <dir>]
      Take a grant somebody signed, and check the deployment answers under it.

  zetlyn platform serve <dir> [--port 8110]
      Every deployment it holds a grant for. It holds no records and no accounts: what a
      page shows was asked for when the page was asked for.
";

/// Which sources take their things from one of these, and are therefore worth running now.
///
/// Read from the declarations rather than from a file somebody keeps in step with them. A
/// source already says what it follows, in the `for_each` that makes it follow.
fn follows(root: &Path, moved: &[String]) -> Vec<(String, PathBuf)> {
    if moved.is_empty() {
        return Vec::new();
    }
    tracker::registry(&root.join("sources"))
        .into_iter()
        .filter(|(_, dir)| {
            sourcedecl::SourceDecl::load(dir)
                .ok()
                .and_then(|d| d.source.after().map(str::to_string))
                .is_some_and(|after| moved.contains(&after))
        })
        .collect()
}

/// A directory without the file that makes it what was asked for. Where it holds the file 0.1
/// wrote instead, that is the thing to say.
fn missing(path: &std::path::Path) -> String {
    match yaml::older(path) {
        Some(old) => format!(
            "{} is from before 0.2. `zetlyn migrate` rewrites it as {}",
            old.display(),
            path.display()
        ),
        None => format!("{}: not here", path.display()),
    }
}

/// The workspace a directory sits in: the nearest parent holding a `workspace.yaml`, else here.
fn workspace_of(dir: &Path) -> PathBuf {
    let start = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    start
        .ancestors()
        .find(|p| p.join("workspace.yaml").exists())
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn assist_command(args: &[String]) -> Result<(), String> {
    match args.get(1).map(String::as_str) {
        Some("key") => {
            let provider = positional(args, 2).first().map(|s| s.to_string()).unwrap_or_else(|| "anthropic".into());
            let path = assist::store_key(&provider)?;
            println!("kept in {}", path.display());
            Ok(())
        }
        Some("teach") => {
            let url = positional(args, 2).first().map(|s| s.to_string()).ok_or("which address?")?;
            let dir = PathBuf::from(flag(args, "--at").ok_or("--at <dir> is where the source goes")?);
            let name = flag(args, "--name").map(str::to_string).unwrap_or_else(|| {
                format!("local/{}", dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "source".into()))
            });
            let a = assist::Assist::configured(&workspace_of(&dir));
            let yes = args.iter().any(|a| a == "--send");
            match teach::api(&a, &url, &dir, &name, yes)? {
                teach::Outcome::NeedsConsent(d) => Err(format!(
                    "this would send to {}:\n  {}\nAsked once per source. Run again with --send to agree, and {} records it.",
                    d.to,
                    d.sends.join("\n  "),
                    dir.join(assist::CONSENT).display()
                )),
                teach::Outcome::Done(t) => {
                    print!("{}", t.declaration);
                    eprintln!("\n{}{}", if t.mended { "mended once; " } else { "" }, t.trial);
                    Ok(())
                }
            }
        }
        Some("align") => {
            let p = positional(args, 2);
            let dir = PathBuf::from(p.first().ok_or("which tracker?")?.as_str());
            let property = p.get(1).ok_or("which property?")?.to_string();
            let (dir, sources) = scope_at(&[String::new(), String::new(), dir.display().to_string()], 2)?;
            let a = assist::Assist::configured(&workspace_of(&dir));
            match teach::align(&a, &dir, &sources, &property, args.iter().any(|a| a == "--send"))? {
                teach::Outcome::NeedsConsent(d) => Err(consent_needed(&d, &dir)),
                teach::Outcome::Done(al) => {
                    print!("{}", al.entry);
                    eprintln!("\n{property}: {} disagreements after the map now, {} with this one", al.before, al.after);
                    if args.iter().any(|a| a == "--apply") {
                        let path = dir.join(crate::trackerdecl::FILE);
                        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
                        let spliced = teach::splice_align(&text, &property, &al.entry);
                        // What goes in must still be the tracker the proposal was counted as.
                        let _: crate::trackerdecl::TrackerDecl = crate::yaml::parse(&spliced)?;
                        std::fs::write(&path, spliced).map_err(|e| e.to_string())?;
                        eprintln!("written into {}", dir.join(crate::trackerdecl::FILE).display());
                    }
                    Ok(())
                }
            }
        }
        Some("why") => {
            let p = positional(args, 2);
            let tracker = PathBuf::from(p.first().ok_or("which tracker?")?.as_str());
            let source = PathBuf::from(p.get(1).ok_or("which source directory?")?.as_str());
            let a = assist::Assist::configured(&workspace_of(&source));
            match teach::why(&a, &tracker, &source, args.iter().any(|a| a == "--send"))? {
                teach::Outcome::NeedsConsent(d) => Err(consent_needed(&d, &source)),
                teach::Outcome::Done(why) => {
                    println!("{why}");
                    Ok(())
                }
            }
        }
        Some("mend") => {
            let dir = dir_at(args, 2)?;
            let ds = crate::source::Source::open(&dir)?;
            let text = std::fs::read_to_string(dir.join(crate::sourcedecl::FILE)).map_err(|e| e.to_string())?;
            let brief = platform::brief(&ds.describe(), &console::runs(&ds), &text);
            let a = assist::Assist::configured(&workspace_of(&dir));
            match teach::mend(&a, &brief, &format!("{}#mend", ds.decl.name), &dir, args.iter().any(|a| a == "--send"))? {
                teach::Outcome::NeedsConsent(d) => Err(consent_needed(&d, &dir)),
                teach::Outcome::Done(proposed) => {
                    print!("{proposed}");
                    if args.iter().any(|a| a == "--apply") {
                        let path = dir.join(crate::sourcedecl::FILE);
                        std::fs::write(dir.join("source.yaml.before"), &text).map_err(|e| e.to_string())?;
                        std::fs::write(&path, &proposed).map_err(|e| e.to_string())?;
                        eprintln!("written; the one before is source.yaml.before");
                    }
                    Ok(())
                }
            }
        }
        Some("ask") => {
            let p = positional(args, 2);
            let dir = PathBuf::from(p.first().ok_or("which tracker?")?.as_str());
            let question = p.iter().skip(1).map(|s| s.as_str()).collect::<Vec<_>>().join(" ");
            if question.trim().is_empty() {
                return Err("what is the question?".into());
            }
            let (dir, sources) = scope_at(&[String::new(), String::new(), dir.display().to_string()], 2)?;
            let t = tracker::Tracker::open(&dir, &sources)?;
            let a = assist::Assist::configured(&workspace_of(&dir));
            match teach::translate(&a, &t, &question, args.iter().any(|a| a == "--send"))? {
                teach::Outcome::NeedsConsent(d) => Err(consent_needed(&d, &dir)),
                teach::Outcome::Done(tr) => match tr.query {
                    Some(q) => {
                        let cx = t.context();
                        let parsed = crate::thingquery::parse(&q, &cx)?;
                        let n = crate::thingstore::ThingStore::open(&dir)?.matching(&parsed, &cx)?.len();
                        println!("{q}");
                        eprintln!("{n} {}", if n == 1 { "thing" } else { "things" });
                        Ok(())
                    }
                    None => Err(format!("no filter: {}", tr.refused.unwrap_or_default())),
                },
            }
        }
        Some("score") => {
            let p = positional(args, 2);
            let (a, b) = (p.first().ok_or("which proposal?")?, p.get(1).ok_or("against which declaration?")?);
            println!("{}", serde_json::to_string_pretty(&teach::score(Path::new(a.as_str()), Path::new(b.as_str()))?).unwrap_or_default());
            Ok(())
        }
        _ => {
            let root = std::env::current_dir().map_err(|e| e.to_string())?;
            let a = assist::Assist::configured(&root);
            println!("the assist here asks {}", a.who());
            Ok(())
        }
    }
}

fn consent_needed(d: &assist::Disclosure, dir: &Path) -> String {
    format!(
        "this would send to {}:\n  {}\nAsked once per source. Run again with --send to agree, and {} records it.",
        d.to,
        d.sends.join("\n  "),
        dir.join(assist::CONSENT).display()
    )
}

fn last_finished(ds: &Source) -> Option<i64> {
    ds.store.run_report(ds.store.last_run()).and_then(|r| r.finished).map(|f| fetch::seconds_of(&f))
}
