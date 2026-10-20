//! The command line of `zetlyn`.

use std::path::PathBuf;

use crate::plan::Plan;

const HELP: &str = "zetlyn: a URL in, its changes out

  serve [--bind 127.0.0.1:4848] [--every 60]   the app, and the checks as they fall due: as on
        [--base https://zetlyn.com]           zetlyn.com, accounts by mail link, mails on change,
                                              the free day, the subscription
        [--local]                             instead one account nobody signs in to, no mail, no
                                              payment (--hosted is the default and may be given)

  Mail goes by SMTP_HOST, SMTP_PORT, SMTP_USER, SMTP_PASSWORD_FILE and MAIL_FROM, or to
  <data>/outbox without them; payment by STRIPE_API_KEY, STRIPE_PRICE_BASE, STRIPE_PRICE_EXTRA and
  STRIPE_WEBHOOK_SECRET, or none without them. ZETLYN_CHROME: the Chrome that renders pages whose
  content their scripts make (off: none; without it the usual places). ZETLYN_ADMINS: the
  addresses (by commas) whose accounts may use /admin.

  check                                         check every watch now, one line each
  add <url> [--select <css>] [--every 60]       watch a page: the place the selector names, or all of it
  add-list <file> [--every 60]                  the same for each line: <url> [<css>]; # starts a comment
  tally                                         how the checks came out, by outcome
  backup <file>                                 a consistent copy of the data, taken while it runs
  alert [--to alarm@zetlyn.com]                 mail when the app does not answer or its checks stopped
  name <instance>                               name this instance (one per server)
  review <email> [--days 30] [--base <url>]    an account for a directory's reviewers: made, free, and a link that signs in to it
  telegram-setup [--base <url>]                 tell Telegram where the bot's messages go (TELEGRAM_BOT_TOKEN, _NAME)
  stripe-setup [--archive-others]               make the product and its prices at Stripe (again: finds
                                                them) and print STRIPE_PRICE_BASE and STRIPE_PRICE_EXTRA

  --data <dir>   where the instance keeps its file (default: ./data)";

/// Runs the command given on the command line.
pub fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let data = PathBuf::from(flag("--data").unwrap_or_else(|| "data".into()));
    let every = flag("--every").and_then(|m| m.parse().ok()).unwrap_or(60);
    crate::render::configure(&data);
    let result = match args.first().map(String::as_str) {
        Some("serve") => {
            let bind = flag("--bind").unwrap_or_else(|| "127.0.0.1:4848".into());
            let hosted = (!args.iter().any(|a| a == "--local")).then(|| crate::life::Hosted {
                base: flag("--base").unwrap_or_else(|| format!("http://{bind}")).trim_end_matches('/').to_string(),
                mailer: crate::mail::Mailer::from_env(&data),
            });
            crate::serve::run(crate::serve::App { data, every_min: every, hosted, plan: Plan::from_env(), limits: Default::default(), admins: admins(), started: crate::store::now(), banner: Default::default(), oauth: crate::oauth::OAuth::from_env(), intents: Default::default() }, &bind)
        }
        Some("check") => crate::store::Store::open(&data).and_then(|s| crate::check::round(&s, true)).map(|done| done.iter().for_each(|c| println!("{}", c.line()))),
        Some("add") => match args.get(1) {
            Some(url) if !url.starts_with("--") => crate::store::Store::open(&data).and_then(|s| crate::check::add(&s, url, flag("--select").as_deref(), every)).map(|(id, value)| println!("{id:>5} {}", first(&value))),
            _ => Err("add: which address?".into()),
        },
        Some("add-list") => match args.get(1) {
            Some(file) if !file.starts_with("--") => add_list(&data, file, every),
            _ => Err("add-list: which file?".into()),
        },
        Some("tally") => crate::store::Store::open(&data).and_then(|s| s.tally()).map(|t| t.iter().for_each(|(o, n, moved)| println!("{o:<8} {n:>6}  found again {moved}"))),
        Some("backup") => match args.get(1) {
            Some(to) if !to.starts_with("--") => crate::store::Store::open(&data).and_then(|s| s.backup(std::path::Path::new(to))),
            _ => Err("backup: to which file?".into()),
        },
        Some("alert") => crate::alert::run(&data, &flag("--to").unwrap_or_else(|| "alarm@zetlyn.com".into()), &flag("--url").unwrap_or_else(|| "http://127.0.0.1:4848".into())),
        Some("name") => match args.get(1) {
            Some(n) if !n.starts_with("--") => crate::store::Store::open(&data).and_then(|s| s.name_instance(n)),
            _ => Err("name: which name?".into()),
        },
        Some("review") => match args.get(1) {
            Some(email) if !email.starts_with("--") => crate::store::Store::open(&data).and_then(|s| {
                let id = s.account(&email.trim().to_lowercase())?;
                s.confirm_account(id)?;
                s.set_free(id, true)?;
                Plan::from_env().paid(&s, id)?;
                let days = flag("--days").and_then(|d| d.parse().ok()).unwrap_or(30);
                let link = crate::serve::review_link(&s, flag("--base").as_deref().unwrap_or("https://zetlyn.com").trim_end_matches('/'), id, days)?;
                println!("{link}");
                Ok(())
            }),
            _ => Err("review: for which address?".into()),
        },
        Some("telegram-setup") => match crate::tell::bot() {
            Some(bot) => bot.set_up(&flag("--base").unwrap_or_else(|| "https://zetlyn.com".into())).map(|_| println!("@{} posts to /telegram/…", bot.name)),
            None => Err("telegram-setup: TELEGRAM_BOT_TOKEN and TELEGRAM_BOT_NAME are not set".into()),
        },
        Some("stripe-setup") => match std::env::var("STRIPE_API_KEY") {
            Ok(key) => crate::billing::Stripe::with_key(&key).setup(args.iter().any(|a| a == "--archive-others")).map(|(base, extra, archived)| {
                for a in archived {
                    eprintln!("archived {a}");
                }
                println!("STRIPE_PRICE_BASE={base}\nSTRIPE_PRICE_EXTRA={extra}");
            }),
            Err(_) => Err("stripe-setup: STRIPE_API_KEY is not set".into()),
        },
        _ => {
            eprintln!("{HELP}");
            std::process::exit(2);
        }
    };
    crate::render::shutdown();
    if let Err(e) = result {
        eprintln!("zetlyn: {e}");
        std::process::exit(1);
    }
}

/// Each line of a file added as a watch, one line said for each, the ones that failed among them.
fn add_list(data: &std::path::Path, file: &str, every: i64) -> Result<(), String> {
    let store = crate::store::Store::open(data)?;
    let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
    let (mut good, mut bad) = (0, 0);
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let (url, css) = match line.split_once(char::is_whitespace) {
            Some((u, c)) => (u, Some(c.trim())),
            None => (line, None),
        };
        match crate::check::add(&store, url, css, every) {
            Ok((id, value)) => {
                good += 1;
                println!("{id:>5} ok   {url}  {}", first(&value));
            }
            Err(e) => {
                bad += 1;
                println!("    - FAIL {url}  {e}");
            }
        }
    }
    println!("{good} added, {bad} failed");
    Ok(())
}

fn first(value: &str) -> String {
    let line = value.lines().next().unwrap_or("");
    let n = value.lines().count();
    let line: String = line.chars().take(70).collect();
    if n > 1 { format!("{line} (+{} lines)", n - 1) } else { line }
}


/// `ZETLYN_ADMINS`: the addresses whose accounts may use `/admin`, separated by commas.
fn admins() -> Vec<String> {
    std::env::var("ZETLYN_ADMINS").unwrap_or_default().split(',').map(|a| a.trim().to_lowercase()).filter(|a| !a.is_empty()).collect()
}
