//! The instance as a web app: the field, the page to mark, the account, and what the watches found.
//!
//! Hosted (`--hosted`), a watch needs an address: the first time it is confirmed by a link, which
//! makes the account and signs its owner in; after that a session cookie does. Local, everything
//! belongs to the local account and nobody signs in.

use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tiny_http::{Header, Method, Request, Response, Server};

use crate::anchor::Anchor;
use crate::check;
use crate::life::{self, Hosted};
use crate::mail;
use crate::page;
use crate::plan::{Out, Plan, Req, Start};
use crate::store::{now, Account, Store, Watch, LOCAL};

mod admin;
mod authz;

type Reply = Response<std::io::Cursor<Vec<u8>>>;

pub struct App {
    pub data: PathBuf,
    pub every_min: i64,
    pub hosted: Option<Hosted>,
    /// Who runs it, and on what terms (`plan.rs`).
    pub plan: Plan,
    /// When each address last asked for what is limited, per kind: reading a page for the frame,
    /// a new watch, a sign-in link.
    pub limits: std::sync::Mutex<std::collections::HashMap<String, Vec<i64>>>,
    /// The addresses whose accounts may use `/admin` (`ZETLYN_ADMINS`); on a local instance,
    /// whoever uses it.
    pub admins: Vec<String>,
    /// When it started, for the admin's system page.
    pub started: i64,
    /// A notice over every page, set on the admin's system page (`meta.banner`).
    pub banner: std::sync::Mutex<String>,
    /// Signing in with Google or Apple, where set up (`oauth.rs`).
    pub oauth: crate::oauth::OAuth,
    /// Watches marked by someone gone to sign in with Google or Apple, by the cookie `zw`.
    pub intents: std::sync::Mutex<std::collections::HashMap<String, Intent>>,
}

impl App {
    /// Whether `ip` may ask for `kind` again: at most `max` times in `window` seconds. Somebody
    /// else's page is read on our server for whoever asks, and a mail goes to whatever address is
    /// typed; without a limit either is a tool for anybody.
    fn allow(&self, kind: &str, ip: &str, max: usize, window: i64) -> bool {
        let at = now();
        let mut limits = self.limits.lock().unwrap_or_else(|e| e.into_inner());
        if limits.len() > 50_000 {
            limits.retain(|_, times| times.last().is_some_and(|t| at - t < 3600));
        }
        let times = limits.entry(format!("{kind} {ip}")).or_default();
        times.retain(|t| at - t < window);
        if times.len() >= max {
            return false;
        }
        times.push(at);
        true
    }
}

/// Whether this request is still within its address's limit. Only what reads somebody else's
/// page or sends a mail is limited.
fn limited(app: &App, a: &Asked) -> bool {
    match (&a.method, a.path.trim_matches('/')) {
        (Method::Get, "frame") => app.allow("frame", &a.ip, 60, 600),
        (Method::Get, "r") => app.allow("r", &a.ip, 3000, 600),
        (Method::Get, "mark") => app.allow("mark", &a.ip, 60, 600),
        (Method::Post, "watch") => app.allow("watch", &a.ip, 10, 3600),
        (Method::Post, "signin") => app.allow("signin", &a.ip, 5, 3600),
        _ => true,
    }
}

pub fn run(app: App, bind: &str) -> Result<(), String> {
    let store = Store::open(&app.data)?;
    *app.banner.lock().unwrap() = store.meta("banner");
    let server = Server::http(bind).map_err(|e| format!("{bind}: {e}"))?;
    let mode = match &app.hosted {
        Some(h) => format!("hosted at {}, mail {}, {}", h.base, if h.mailer.smtp.is_some() { "by SMTP" } else { "to outbox/" }, app.plan.describe()),
        None => "local".into(),
    };
    let mode = format!("{mode}, {}", crate::render::describe());
    println!("zetlyn {} ({}, {mode}) on http://{bind}", env!("CARGO_PKG_VERSION"), store.instance());
    let app = Arc::new(app);
    let ticking = Arc::clone(&app);
    std::thread::spawn(move || {
        // Chrome is started here, by the thread that lives as long as the app: on Linux it ends
        // with the thread that started it.
        crate::render::warm();
        loop {
            std::thread::sleep(Duration::from_secs(60));
            match Store::open(&ticking.data).and_then(|s| life::tick(&s, ticking.hosted.as_ref(), &ticking.plan)) {
                Ok(said) => said.iter().for_each(|l| println!("{l}")),
                Err(e) => eprintln!("tick: {e}"),
            }
        }
    });
    // Several requests at once, each worker with its own connection to the file: reading a page
    // for the frame takes seconds (rendered, more), and nobody else should wait for it.
    let server = Arc::new(server);
    let workers: Vec<_> = (0..WORKERS)
        .map(|_| {
            let (app, server) = (Arc::clone(&app), Arc::clone(&server));
            let store = Store::open(&app.data);
            std::thread::spawn(move || -> Result<(), String> {
                let store = store?;
                loop {
                    let request = server.recv().map_err(|e| e.to_string())?;
                    // A request that panics loses its answer, not the worker.
                    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| answer(&app, &store, request))) {
                        Ok(Err(e)) => eprintln!("{e}"),
                        Err(_) => eprintln!("a request panicked"),
                        Ok(Ok(())) => {}
                    }
                }
            })
        })
        .collect();
    drop(store);
    for w in workers {
        match w.join() {
            Ok(r) => r?,
            Err(_) => return Err("a worker panicked".into()),
        }
    }
    Ok(())
}

/// How many requests are answered at once.
const WORKERS: usize = 8;

/// A request, read: where it goes, what it asks, who is signed in.
struct Asked {
    method: Method,
    path: String,
    query: String,
    form: String,
    body: Vec<u8>,
    headers: Vec<(String, String)>,
    session: Option<String>,
    /// The address asking: the first one Caddy puts in X-Forwarded-For, or the peer.
    ip: String,
    /// The API key in `Authorization: Bearer …`, for the API and MCP.
    bearer: Option<String>,
    account: Option<Account>,
}

impl Asked {
    fn q(&self, name: &str) -> Option<String> {
        form_urlencoded::parse(self.query.as_bytes()).find(|(k, _)| k == name).map(|(_, v)| v.into_owned())
    }
    fn f(&self, name: &str) -> String {
        form_urlencoded::parse(self.form.as_bytes()).find(|(k, _)| k == name).map(|(_, v)| v.into_owned()).unwrap_or_default()
    }
}

fn answer(app: &App, store: &Store, mut req: Request) -> Result<(), String> {
    let url = req.url().to_string();
    let (path, query) = url.split_once('?').map(|(p, q)| (p.to_string(), q.to_string())).unwrap_or((url.clone(), String::new()));
    let header = |name: &str| req.headers().iter().find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name)).map(|h| h.value.as_str().to_string()).unwrap_or_default();
    let headers: Vec<(String, String)> = req.headers().iter().map(|h| (h.field.as_str().as_str().to_string(), h.value.as_str().to_string())).collect();
    let bearer = header("Authorization").strip_prefix("Bearer ").map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
    let forwarded = header("X-Forwarded-For");
    let ip = forwarded.split(',').next().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| req.remote_addr().map(|a| a.ip().to_string()).unwrap_or_default());
    let session = header("Cookie").split(';').filter_map(|c| c.trim().split_once('=')).find(|(k, _)| *k == "z").map(|(_, v)| v.to_string());
    // HEAD is answered as GET would be, without the body (tiny_http leaves it out): link checkers
    // and previews ask that way, and a 404 to them is a page that looks broken.
    let method = if *req.method() == Method::Head { Method::Get } else { req.method().clone() };
    let mut body = Vec::new();
    if method == Method::Post || method == Method::Patch {
        req.as_reader().take(1024 * 1024).read_to_end(&mut body).map_err(|e| e.to_string())?;
    }
    let account = if app.hosted.is_some() {
        session.as_deref().and_then(|t| store.session(t).ok().flatten()).and_then(|id| store.account_by_id(id).ok().flatten())
    } else {
        store.local_account().ok().and_then(|id| store.account_by_id(id).ok().flatten())
    };
    let asked = Asked { method, path, query, form: String::from_utf8_lossy(&body).into_owned(), body, headers, session, ip, bearer, account };
    // What went wrong goes to the log; the person is told only that it did, and what to do.
    let reply = route(app, store, &asked).unwrap_or_else(|e| {
        eprintln!("{} {}: {e}", asked.method, asked.path);
        page_reply(app, &asked, "Something went wrong", "<section class=narrow><h1>Something went wrong</h1><p>Zetlyn could not finish this just now. Please try again in a moment; if it keeps happening, write to hello@zetlyn.com.</p><p><a href=\"/\">Back to the front page</a></p></section>", 500)
    });
    req.respond(reply).map_err(|e| e.to_string())
}

fn route(app: &App, store: &Store, a: &Asked) -> Result<Reply, String> {
    let segments: Vec<&str> = a.path.trim_matches('/').split('/').collect();
    // OAuth for the MCP server: its discovery addresses, registration, consent and tokens.
    if let Some(reply) = authz::route(app, store, a, &segments) {
        return reply;
    }
    let hosted = app.hosted.as_ref();
    Ok(match (&a.method, segments.as_slice()) {
        (Method::Get, ["frame"] | ["mark"] | ["r"]) | (Method::Post, ["watch"] | ["signin"]) if !limited(app, a) => {
            page_reply(app, a, "Slow down", "<section class=narrow><h1>Slow down</h1><p>Too many requests from your address. Please try again in a few minutes.</p></section>", 429)
        }
        (Method::Get, [""]) => html(home(app, a)?),
        (Method::Get, ["health"]) => Response::from_string("ok"),
        (_, ["admin", rest @ ..]) => admin::route(app, store, a, rest)?,
        (Method::Get, ["mark.png"]) => image(MARK),
        (Method::Get, ["favicon.png"]) => image(FAVICON),
        (Method::Get, ["og.png"]) => image(OG),
        (Method::Get, ["mark"]) => mark_page(app, store, a)?,
        (Method::Get, ["frame"]) => frame(&a.q("url").unwrap_or_default(), &origin(app, a), a.q("browser").is_some()),
        (Method::Get, ["r"]) => through(&a.q("u").unwrap_or_default(), &a.q("s").unwrap_or_default(), &origin(app, a)),
        (Method::Post, ["watch"]) => watch(app, store, a)?,
        (Method::Get, ["sent"]) => page_reply(app, a, "Check your inbox", &sent(app, a), 200),
        (Method::Get, ["c", token]) if hosted.is_some() => confirm(app, store, token)?,
        (Method::Get, ["signin"]) if hosted.is_some() => page_reply(app, a, "Sign in", &signin_page(app, a), 200),
        (Method::Get, ["oauth", provider]) if hosted.is_some() => oauth_begin(app, a, provider),
        (Method::Get, ["oauth", provider, "back"]) if hosted.is_some() => oauth_back(app, store, a, provider, &a.q("state").unwrap_or_default(), &a.q("code").unwrap_or_default())?,
        // Apple answers by a form posted back.
        (Method::Post, ["oauth", provider, "back"]) if hosted.is_some() => oauth_back(app, store, a, provider, &a.f("state"), &a.f("code"))?,
        (Method::Post, ["signin"]) if hosted.is_some() => signin(app, store, a)?,
        (Method::Get, ["s", token]) if hosted.is_some() => {
            let next = a.q("next").filter(|n| !n.is_empty()).or_else(|| authz::next_after_signin(a)).unwrap_or_default();
            enter(app, store, token, &next)?.with_header(authz::next_cookie(app, ""))
        }
        // A reviewer's way in (`review_link`): signs in to the one account it was made for, as
        // often as needed until it ends, then on to where they were going (an app's request to
        // connect, waiting).
        (Method::Get, ["review", token]) if hosted.is_some() => match store.token_holder(token, "review")?.map(|id| store.account_by_id(id)).transpose()?.flatten() {
            Some(acc) if !acc.blocked => {
                let session = store.new_session(acc.id)?;
                let next = authz::next_after_signin(a).filter(|n| n.starts_with('/') && !n.starts_with("//")).unwrap_or_else(|| "/watches".into());
                redirect(&next).with_header(cookie(app, &session)).with_header(authz::next_cookie(app, ""))
            }
            _ => redirect("/signin?expired=1"),
        },
        (Method::Post, ["signout"]) => {
            if let Some(t) = &a.session {
                store.end_session(t)?;
            }
            redirect("/").with_header(cookie(app, ""))
        }
        (Method::Get, ["account"]) => match &a.account {
            Some(acc) => page_reply(app, a, "Your watches", &account_page(app, store, acc, a)?, 200),
            None => redirect("/signin"),
        },
        (_, ["api", "v1", rest @ ..]) => programmatic(app, store, a, |c| {
            let mut body: serde_json::Value = serde_json::from_slice(&a.body).unwrap_or(serde_json::Value::Null);
            if let (Method::Get, Some(since)) = (&a.method, a.q("since")) {
                body = serde_json::json!({ "since": since });
            }
            // A change of a watch reads its page only when it marks another place, or the whole page.
            let rereads = body.get("selector").is_some_and(|s| !s.is_null()) || body["whole_page"].as_bool() == Some(true);
            let fetches = matches!((&a.method, rest), (Method::Post, ["preview"] | ["watches"] | ["read"])) || (matches!((&a.method, rest), (Method::Patch, ["watches", _])) && rereads);
            if fetches && !app.allow("api-fetch", &c.account.id.to_string(), 60, 600) {
                return (429, serde_json::json!({ "error": "Too many pages read for this account; try again in a few minutes." }));
            }
            crate::api::rest(c, a.method.as_str(), rest, &body)
        }),
        (Method::Get, ["mcp"]) => Response::from_string("This MCP server takes JSON-RPC by POST; it offers no event stream.").with_status_code(405).with_header(header("Allow", "POST")),
        (Method::Post, ["mcp"]) => authz::cors(mcp_reply(app, store, a)),
        // Keys live under the account; /keys was their address before.
        (Method::Get, ["account", "notifications"]) => match &a.account {
            Some(acc) if hosted.is_some() => page_reply(app, a, "Notifications", &notifications_page(app, store, acc, a)?, 200),
            _ => redirect("/signin"),
        },
        (Method::Post, ["account", "notifications", rest @ ..]) if hosted.is_some() => match &a.account {
            Some(acc) => notifications_post(app, store, acc, a, rest)?,
            None => redirect("/signin"),
        },
        // What Zetlyn's Telegram bot was told, posted by Telegram with the secret it was given.
        (Method::Post, ["telegram", secret]) => match crate::tell::bot() {
            Some(bot) if *secret == bot.secret() && a.headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("X-Telegram-Bot-Api-Secret-Token") && *v == bot.secret()) => {
                if let Err(e) = crate::tell::update(store, bot, &public_base(app, a), &a.body) {
                    eprintln!("telegram: {e}");
                }
                Response::from_string("").with_status_code(200)
            }
            _ => not_found(app, a),
        },
        (Method::Post, ["account", "delete"]) => delete_own_account(app, store, a)?,
        (Method::Post, ["account", "email"]) if hosted.is_some() => ask_new_address(app, store, a)?,
        (Method::Get, ["account", "email", token]) if hosted.is_some() => take_new_address(app, store, a, token)?,
        (Method::Post, ["account", "keys"] | ["keys"]) => match &a.account {
            Some(acc) => {
                let key = store.new_api_key(acc.id, &a.f("name"))?;
                page_reply(app, a, "Your API key", &key_page(&key, &public_base(app, a)), 200)
            }
            None => redirect("/signin"),
        },
        (Method::Get, ["account", "keys"]) => match &a.account {
            Some(acc) => page_reply(app, a, "API keys", &keys_page(app, store, acc, a)?, 200),
            None => redirect("/signin"),
        },
        (Method::Get, ["account", "billing"]) => match &a.account {
            Some(acc) if hosted.is_some() => page_reply(app, a, "Billing", &account_shell(app, a, "Billing", &billing_page(app, store, acc)?), 200),
            Some(_) => redirect("/account"),
            None => redirect("/signin"),
        },
        (Method::Post, ["account", "apps", client, "disconnect"]) => {
            if let Some(acc) = &a.account {
                store.disconnect(acc.id, client)?;
            }
            redirect("/account/keys")
        }
        (Method::Post, ["account", "keys", id, "delete"] | ["keys", id, "delete"]) => {
            if let (Some(acc), Ok(id)) = (&a.account, id.parse()) {
                store.delete_api_key(acc.id, id)?;
            }
            redirect("/account/keys")
        }
        (Method::Post, ["check"]) if hosted.is_none() => {
            let s = Store::open(&app.data)?;
            check::round(&s, true)?.iter().for_each(|c| println!("{}", c.line()));
            redirect("/watches")
        }
        (Method::Get, ["watches"]) => match &a.account {
            Some(acc) => page_reply(app, a, "Your watches", &watches_page(app, store, acc)?, 200),
            None => redirect("/signin"),
        },
        (Method::Get, ["watches", id]) => match owned(store, a, id)? {
            // Named by its number, as before: the address it has now.
            Some(w) if *id != w.key => Response::from_string("").with_status_code(301).with_header(header("Location", &format!("/watches/{}", w.key))),
            Some(w) => {
                let faster = a.account.as_ref().is_some_and(|acc| crate::plan::interval_allowed(acc, app.hosted.is_some(), 1));
                let to = a.account.as_ref().map(|acc| acc.email.clone()).filter(|e| e != LOCAL);
                page_reply(app, a, &host_of(&w.url), &detail(app, store, &w, faster, to.as_deref(), a.q("when").filter(|v| v == "bad").map(|_| a.q("why").unwrap_or_default()))?, 200)
            }
            // A link from a mail opened where nobody is signed in: signing in comes first.
            None if a.account.is_none() => redirect("/signin"),
            None => not_found(app, a),
        },
        // The addresses watches had before: kept, so links in mails already sent still go somewhere.
        (Method::Get, ["w", id]) => Response::from_string("").with_status_code(301).with_header(header("Location", &format!("/watches/{id}"))),
        (Method::Post, ["watches" | "w", id, "every"]) => {
            let minutes: i64 = a.f("minutes").parse().unwrap_or(60);
            match (owned(store, a, id)?, &a.account) {
                (Some(w), Some(acc)) if crate::plan::interval_allowed(acc, app.hosted.is_some(), minutes) => {
                    store.set_interval(w.id, minutes)?;
                    redirect(&format!("/watches/{}", w.key))
                }
                (Some(w), Some(_)) => redirect(&format!("/watches/{}", w.key)),
                _ => not_found(app, a),
            }
        }
        (Method::Post, ["watches", id, "when"]) => match owned(store, a, id)? {
            Some(w) => match crate::when::Condition::from_form(&a.f("kind"), &a.f("operand")) {
                Ok(k) if k.as_ref().is_some_and(|k| k.needs_number()) && !matches!(w.mode, crate::store::Mode::Place(_)) => redirect(&format!("/watches/{}?when=bad", w.key)),
                // Weighed on what the place says now: two amounts there leave the number unclear.
                Ok(Some(k)) if k.fits(w.last_value.as_deref().unwrap_or("")).is_err() => {
                    let why = k.fits(w.last_value.as_deref().unwrap_or("")).err().unwrap_or_default();
                    redirect(&format!("/watches/{}?when=bad&why={}", w.key, enc(&why)))
                }
                Ok(k) => {
                    store.set_condition(w.id, k.as_ref())?;
                    redirect(&format!("/watches/{}", w.key))
                }
                Err(_) => redirect(&format!("/watches/{}?when=bad", w.key)),
            },
            None => not_found(app, a),
        },
        (Method::Post, ["watches" | "w", id, "end" | "delete"]) => {
            if let Some(w) = owned(store, a, id)? {
                if hosted.is_some() {
                    app.plan.ended_early(store, &w)?;
                }
                store.delete(w.id)?;
                if hosted.is_some() {
                    app.plan.sync(store, w.account);
                }
            }
            redirect("/watches")
        }
        _ => fallback(app, store, a)?,
    })
}

/// What no route of Zetlyn's own answers: the plan's pages (subscribing, payment events), then
/// the pages beside the app, then "not here".
fn fallback(app: &App, store: &Store, a: &Asked) -> Result<Reply, String> {
    if let Some(h) = &app.hosted {
        let segments: Vec<&str> = a.path.trim_matches('/').split('/').collect();
        let header = |name: &str| a.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone()).unwrap_or_default();
        let req = Req { method: a.method.as_str(), path: &segments, body: &a.body, header: &header, account: a.account.as_ref() };
        if let Some(out) = app.plan.route(store, h, &req) {
            return Ok(match out? {
                Out::Page { status, title, body } => page_reply(app, a, &title, &body, status),
                Out::Redirect(to) => redirect(&to),
                Out::Text { status, body } => Response::from_string(body).with_status_code(status),
            });
        }
    }
    Ok(match (&a.method, a.path.trim_matches('/')) {
        // Who runs an instance, and on what terms, is its operator's to say (`operator_page`).
        (Method::Get, "legal") => page_reply(app, a, "Legal notice — Zetlyn", &operator_page(app, "legal.html", SITE_LEGAL), 200),
        (Method::Get, "privacy") => page_reply(app, a, "Privacy — Zetlyn", &operator_page(app, "privacy.html", SITE_PRIVACY), 200),
        (Method::Get, "terms") => page_reply(app, a, "Terms — Zetlyn", &operator_page(app, "terms.html", SITE_TERMS), 200),
        (Method::Get, "withdrawal") => page_reply(app, a, "Cancellation policy — Zetlyn", &operator_page(app, "withdrawal.html", SITE_WITHDRAWAL), 200),
        (Method::Get, "llms.txt") => Response::from_string(crate::docs::guide(&public_base(app, a))).with_header(header("Content-Type", "text/markdown; charset=utf-8")).with_header(header("Cache-Control", "public, max-age=3600")),
        // Proof for the MCP registry that zetlyn.com publishes com.zetlyn/* (`server.json`).
        (Method::Get, ".well-known/mcp-registry-auth") => match own_file(app, "mcp-registry-auth", include_str!("../site/mcp-registry-auth")) {
            Some(proof) => Response::from_string(proof.trim().to_string()).with_header(header("Content-Type", "text/plain; charset=utf-8")),
            None => not_found(app, a),
        },
        // Proof for OpenAI that zetlyn.com is ours: the token its dashboard gives, from the environment.
        (Method::Get, ".well-known/openai-apps-challenge") => match std::env::var("OPENAI_APPS_CHALLENGE").ok().filter(|t| !t.trim().is_empty()) {
            Some(t) => Response::from_string(t.trim().to_string()).with_header(header("Content-Type", "text/plain; charset=utf-8")),
            None => not_found(app, a),
        },
        (Method::Get, "llms-full.txt") => Response::from_string(crate::docs::llms_full(&public_base(app, a))).with_header(header("Content-Type", "text/markdown; charset=utf-8")).with_header(header("Cache-Control", "public, max-age=3600")),
        (Method::Get, "openapi.json") => Response::from_string(crate::docs::openapi(&public_base(app, a))).with_header(header("Content-Type", "application/json")).with_header(header("Access-Control-Allow-Origin", "*")).with_header(header("Cache-Control", "public, max-age=3600")),
        (Method::Get, "docs") => page_reply(app, a, "Docs — Zetlyn", &crate::docs::page(&public_base(app, a)), 200),
        (Method::Get, "robots.txt") => Response::from_string(format!("{ROBOTS}Sitemap: {}/sitemap.xml\n", public_base(app, a))).with_header(header("Content-Type", "text/plain; charset=utf-8")).with_header(header("Cache-Control", "public, max-age=3600")),
        (Method::Get, "sitemap.xml") => Response::from_string(crate::seo::sitemap(&public_base(app, a), &[])).with_header(header("Content-Type", "application/xml; charset=utf-8")).with_header(header("Cache-Control", "public, max-age=3600")),
        _ => not_found(app, a),
    })
}

/// The pages beside the app, carried in the binary: what a provider has to say, and how an LLM
/// uses Zetlyn.
const SITE_LEGAL: &str = include_str!("../site/legal.html");
const SITE_PRIVACY: &str = include_str!("../site/privacy.html");
const SITE_TERMS: &str = include_str!("../site/terms.html");
const SITE_WITHDRAWAL: &str = include_str!("../site/withdrawal.html");
/// Whether this instance is zetlyn.com itself, whose operator's pages are carried in the binary.
fn is_zetlyn_com(app: &App) -> bool {
    app.hosted.as_ref().is_some_and(|h| h.base == "https://zetlyn.com")
}

/// A file of the instance's operator, `<data>/site/<name>`; on zetlyn.com, zetlyn.com's own.
/// Another instance never says it is zetlyn.com or speaks for its operator.
fn own_file(app: &App, name: &str, zetlyn_com: &str) -> Option<String> {
    std::fs::read_to_string(app.data.join("site").join(name)).ok().or_else(|| is_zetlyn_com(app).then(|| zetlyn_com.to_string()))
}

/// A page only the operator can write (the legal notice, privacy, terms), or that it is missing.
fn operator_page(app: &App, name: &str, zetlyn_com: &str) -> String {
    own_file(app, name, zetlyn_com).unwrap_or_else(|| {
        format!("<section class=prose><h1>Not written yet</h1><p>This Zetlyn is run by someone other than zetlyn.com, and its operator has not put this page in place yet. Operator: write it to <code>{}</code> and it is shown here.</p></section>", esc(&app.data.join("site").join(name).display().to_string()))
    })
}
/// The pages for one person (marking, the account, links from mails) are nobody's to index.
const ROBOTS: &str = "User-agent: *\nAllow: /\nDisallow: /mark\nDisallow: /frame\nDisallow: /r\nDisallow: /account\nDisallow: /w/\nDisallow: /s/\nDisallow: /c/\nDisallow: /sent\nDisallow: /api/\nDisallow: /mcp\nDisallow: /keys\nDisallow: /admin\nDisallow: /oauth\nDisallow: /review/\nDisallow: /watches\nDisallow: /subscribe\n";
const FOOTER: &str = r#"<a href="/legal">Legal notice</a><a href="/privacy">Privacy</a><a href="/terms">Terms</a><a href="/withdrawal">Withdrawal</a><a href="/docs">Docs</a><a href="/llms.txt">For LLMs</a><span>Servers in the EU</span>"#;

/// Where a started watch sends its owner: their watches, or the page that makes it start.
fn started(start: Start, key: &str) -> String {
    match start {
        Start::Waiting { go, .. } => go,
        Start::Running { .. } => format!("/watches/{key}"),
    }
}

/// A watch the one asking may see: their own, or any on a local instance. Named by its key, or by
/// its number as addresses had it before.
fn owned(store: &Store, a: &Asked, id: &str) -> Result<Option<Watch>, String> {
    let Some(acc) = &a.account else { return Ok(None) };
    let w = match id.parse::<i64>() {
        Ok(n) => store.watch(n)?,
        Err(_) if !id.is_empty() => store.watch_by_key(id)?,
        Err(_) => None,
    };
    Ok(w.filter(|w| w.account == acc.id))
}

/// What somebody typed, as an address Zetlyn may ask: `https://` when no scheme was given.
fn address(typed: &str) -> Result<String, String> {
    let t = typed.trim();
    let url = crate::fetch::clean(&if t.contains("://") { t.to_string() } else { format!("https://{t}") });
    crate::outbound::public_target(&url).map(|_| url)
}

fn watch(app: &App, store: &Store, a: &Asked) -> Result<Reply, String> {
    // Back to the page to mark, with what was chosen beside the mark still chosen.
    let back = |e: &str| redirect(&format!("/mark?error={}{}&url={}&kind={}&operand={}", enc(e), if a.f("watch").is_empty() { String::new() } else { format!("&watch={}", a.f("watch")) }, enc(&a.f("url")), enc(&a.f("kind")), enc(&a.f("operand"))));
    let url = match address(&a.f("url")) {
        Ok(u) => u,
        Err(e) => return Ok(redirect(&format!("/?error={}", enc(&e)))),
    };
    let anchor: Option<Anchor> = match a.f("anchor").trim() {
        "" => None,
        json => match serde_json::from_str(json) {
            Ok(x) => Some(x),
            Err(_) => return Ok(back("The marked place could not be read. Please mark it again.")),
        },
    };
    // When to tell, chosen beside the mark (`when.rs`); a number needs a place to read it in.
    let condition = match crate::when::Condition::from_form(&a.f("kind"), &a.f("operand")) {
        Ok(k) => k,
        Err(e) => return Ok(back(&e)),
    };
    if anchor.is_none() && condition.as_ref().is_some_and(|k| k.needs_number()) {
        return Ok(back("Below and above read a number in one place: mark the place with the number, or choose “When it says” for the whole page."));
    }
    let (mode, value, render) = match check::first(&url, anchor) {
        Ok(x) => x,
        Err(e) => {
            let _ = store.refused(&url, "mark", &e);
            return Ok(back(&e));
        }
    };
    if let Some(Err(e)) = condition.as_ref().map(|k| k.fits(&value)) {
        return Ok(back(&e));
    }
    // Marked again: the watch keeps its history and its state.
    if let Some(w) = owned(store, a, &a.f("watch"))? {
        store.remark(w.id, &mode, &value)?;
        store.set_condition(w.id, condition.as_ref())?;
        store.set_render(w.id, render)?;
        return Ok(redirect(&format!("/watches/{}", w.key)));
    }
    let Some(h) = &app.hosted else {
        let id = store.add_watch(store.local_account()?, &url, &mode, app.every_min, None, &value)?;
        store.set_condition(id, condition.as_ref())?;
        store.set_render(id, render)?;
        return Ok(redirect(&format!("/watches/{}", store.watch(id)?.map(|w| w.key).unwrap_or_default())));
    };
    if let Some(acc) = &a.account {
        let id = store.add_watch_as(acc.id, &url, &mode, app.every_min, None, &value, "pending")?;
        store.set_condition(id, condition.as_ref())?;
        store.step(acc.id, "first_watch", "web")?;
        store.set_render(id, render)?;
        let key = store.watch(id)?.map(|w| w.key).unwrap_or_default();
        return Ok(redirect(&started(life::start(store, h, &app.plan, acc.id, id)?, &key)));
    }
    // With Google or Apple instead of an address: the watch waits here while the person signs in
    // there (`oauth_back` makes it), for fifteen minutes at most.
    let via = a.f("via");
    if app.oauth.providers().contains(&via.as_str()) {
        let Some((to, kept)) = app.oauth.begin(&via, &public_base(app, a)) else { return Ok(back("That way of signing in is not set up.")) };
        let id = crate::crypto::token();
        let mut intents = app.intents.lock().unwrap_or_else(|e| e.into_inner());
        intents.retain(|_, i| now() - i.at < INTENT_TTL);
        intents.insert(id.clone(), Intent { url: url.clone(), mode, value, render, condition, at: now() });
        return Ok(redirect(&to).with_header(oauth_cookie(app, "zo", &kept)).with_header(oauth_cookie(app, "zw", &id)));
    }
    let email = a.f("email").trim().to_lowercase();
    if !mail::plausible(&email) {
        return Ok(back("Please enter the email address the changes should go to."));
    }
    let account = store.account(&email)?;
    if store.account_by_id(account)?.is_some_and(|acc| acc.blocked) {
        return Ok(back("This address cannot be used for watches."));
    }
    if store.watches_of(account)?.iter().filter(|w| w.state == "pending").count() >= 5 {
        return Ok(back("There are five watches for this address waiting for confirmation already. Please confirm them from your inbox first."));
    }
    let id = store.add_watch_as(account, &url, &mode, app.every_min, None, &value, "pending")?;
    store.set_condition(id, condition.as_ref())?;
    store.step(account, "first_watch", "web")?;
    store.set_render(id, render)?;
    let token = store.new_token(account, "confirm", Some(id), 2 * 86400)?;
    let w = store.watch(id)?.ok_or("the watch just made")?;
    h.tell_soon(&email, mail::confirm(&url, &life::what(&w), &format!("{}/c/{token}", h.base), crate::plan::CONFIRM_NOTE));
    Ok(redirect(&format!("/sent?to={}&what={}&page={}", enc(&email), enc(&life::what(&w)), enc(&url))))
}

fn confirm(app: &App, store: &Store, token: &str) -> Result<Reply, String> {
    let h = app.hosted.as_ref().ok_or("not hosted")?;
    let Some((account, Some(watch))) = store.take_token(token, "confirm")? else {
        return Ok(redirect("/signin?expired=1"));
    };
    if store.account_by_id(account)?.is_none_or(|acc| acc.blocked) {
        return Ok(redirect("/signin?expired=1"));
    }
    store.confirm_account(account)?;
    store.step(account, "confirmed", "email")?;
    let key = store.watch(watch)?.map(|w| w.key).unwrap_or_default();
    let to = started(life::start(store, h, &app.plan, account, watch)?, &key);
    let session = store.new_session(account)?;
    Ok(redirect(&to).with_header(cookie(app, &session)))
}

fn signin(app: &App, store: &Store, a: &Asked) -> Result<Reply, String> {
    let h = app.hosted.as_ref().ok_or("not hosted")?;
    let email = a.f("email").trim().to_lowercase();
    if !mail::plausible(&email) {
        return Ok(redirect("/signin?bad=1"));
    }
    // An address without an account gets one: the link confirms it when it is clicked, and an
    // account nobody confirms goes with the unpaid ones after 30 days. A blocked one gets nothing,
    // and the page says the same either way.
    let acc = store.account_by_id(store.account(&email)?)?.ok_or("the account just made")?;
    if !acc.blocked {
        let token = store.new_token(acc.id, "signin", None, 1800)?;
        // An app's request to connect waiting: the link leads back to it, in whatever browser it is opened.
        let next = authz::next_after_signin(a).map(|n| format!("?next={}", enc(&n))).unwrap_or_default();
        h.tell_soon(&email, mail::signin(&format!("{}/s/{token}{next}", h.base)));
    }
    Ok(redirect(&format!("/sent?to={}&signin=1", enc(&email))))
}

/// A link that signs a directory's reviewers in to one account for `days` days, without a mail
/// they could not read: `/review/<token>`, opened as often as needed. Only its digest is kept;
/// `Store::end_review` takes it back.
pub fn review_link(store: &Store, base: &str, account: i64, days: i64) -> Result<String, String> {
    let token = store.new_token(account, "review", None, days.clamp(1, 90) * 86400)?;
    Ok(format!("{base}/review/{token}"))
}

fn enter(app: &App, store: &Store, token: &str, next: &str) -> Result<Reply, String> {
    let Some((account, _)) = store.take_token(token, "signin")? else {
        return Ok(redirect("/signin?expired=1"));
    };
    if store.account_by_id(account)?.is_none_or(|acc| acc.blocked) {
        return Ok(redirect("/signin?expired=1"));
    }
    store.confirm_account(account)?;
    store.step(account, "confirmed", "email")?;
    let session = store.new_session(account)?;
    // Only a path on this site: a link must not send its reader elsewhere.
    let to = if next.starts_with('/') && !next.starts_with("//") && !next.contains('\\') { next } else { "/watches" };
    Ok(redirect(to).with_header(cookie(app, &session)))
}

fn frame(url: &str, origin: &str, browser: bool) -> Reply {
    match address(url).and_then(|u| crate::fetch::get(&u)) {
        // A wall instead of the page (a captcha): said, not shown as if it were the page.
        Ok(f) if f.is_html() && check::walled(&f.body) => message(&check::refused(url)),
        Ok(f) if f.is_html() && check::frames(&f.body, &f.url).is_some() => message(&check::framed_said(url, &check::frames(&f.body, &f.url).unwrap_or_default())),
        Ok(f) if f.is_html() => {
            let nonce = crate::crypto::token();
            // As served, which takes a moment, when that shows the page; in Chrome (seconds) when
            // its scripts make it, or when asked (`browser`). Its images, fonts and styles come
            // through this app either way (`through`), into the reader's browser.
            let rendered = crate::render::available() && (browser || check::thin(&f.body));
            let (at, body) = if rendered { crate::render::html(&f.url).unwrap_or((f.url, f.body)) } else { (f.url, f.body) };
            if check::walled(&body) {
                return message(&check::refused(url));
            }
            Response::from_string(page::framed(&body, &at, &nonce, origin))
                .with_header(header("Content-Type", "text/html; charset=utf-8"))
                .with_header(header("Content-Security-Policy", &page::frame_policy(&nonce, origin)))
        }
        // JSON, a feed, XML or text: its page (`doc.rs`), marked like any other.
        Ok(f) if crate::doc::kind(&f.content_type, &f.body).is_some() => {
            let kind = crate::doc::kind(&f.content_type, &f.body).expect("just seen");
            match crate::doc::page(kind, &f.body) {
                Ok(body) => {
                    let nonce = crate::crypto::token();
                    Response::from_string(page::framed(&body, &f.url, &nonce, origin))
                        .with_header(header("Content-Type", "text/html; charset=utf-8"))
                        .with_header(header("Content-Security-Policy", &page::frame_policy(&nonce, origin)))
                }
                Err(e) => message(&e),
            }
        }
        Ok(f) => message(&format!("There is no text here to watch ({}). Zetlyn reads web pages, JSON, RSS and Atom feeds, XML and text.", f.content_type)),
        Err(e) => message(&format!("Zetlyn could not read this page: {}", check::said(url, &e))),
    }
}

/// What a framed page loads, loaded here (`page::through`): images, fonts and stylesheets only, at
/// most 5 MB, and only addresses this app signed. An SVG opened by itself runs nothing (sandbox).
fn through(target: &str, s: &str, origin: &str) -> Reply {
    let nothing = || Response::from_data(Vec::new()).with_status_code(404);
    if !page::signed(target, s) {
        return nothing();
    }
    let Ok(got) = crate::fetch::get_bytes(target, 5 * 1024 * 1024) else { return nothing() };
    let kind = got.content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let path = got.url.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
    let font_file = [".woff2", ".woff", ".ttf", ".otf"].iter().any(|x| path.ends_with(x));
    let body = if kind == "text/css" {
        page::css_through(&String::from_utf8_lossy(&got.bytes), &got.url, origin).into_bytes()
    } else if kind.starts_with("image/") || kind.starts_with("font/") || kind.contains("font") || ((kind.is_empty() || kind == "application/octet-stream") && font_file) {
        got.bytes
    } else {
        return nothing();
    };
    Response::from_data(body)
        .with_header(header("Content-Type", if kind.is_empty() { "application/octet-stream" } else { &kind }))
        .with_header(header("Cache-Control", "private, max-age=3600"))
        .with_header(header("Content-Security-Policy", "sandbox; default-src 'none'; style-src 'unsafe-inline'"))
        .with_header(header("X-Content-Type-Options", "nosniff"))
        // The frame has no origin of its own; a font from here is a font from elsewhere to it.
        .with_header(header("Access-Control-Allow-Origin", "*"))
}

/// This app's own origin, which a framed page's `<base>` would otherwise take for the page's site.
fn origin(app: &App, a: &Asked) -> String {
    match &app.hosted {
        Some(h) => h.base.trim_end_matches('/').to_string(),
        None => format!("http://{}", a.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("host")).map_or("127.0.0.1:4848", |(_, v)| v.as_str())),
    }
}

fn message(text: &str) -> Reply {
    Response::from_string(format!("<!doctype html><meta charset=utf-8><body style=\"font:15px system-ui;padding:2rem;color:#333\">{}</body>", esc(text))).with_header(header("Content-Type", "text/html; charset=utf-8"))
}

fn home(app: &App, a: &Asked) -> Result<String, String> {
    let error = a.q("error").map(|e| format!("<p class=error>{}</p>", esc(&e))).unwrap_or_default();
    let field = format!(
        "<form class=field action=/mark><input name=url placeholder=\"https://example.com/pricing\" aria-label=\"The address of the page to watch\" autofocus required><button class=primary>Watch it</button></form>{error}"
    );
    Ok(layout(app, a, "Zetlyn: website change monitor, get told when a page changes", &format!("{}{LANDING}{}{FAQ}", hero(&field, crate::plan::HERO_NOTE), crate::plan::PRICING)))
}

fn hero(field: &str, note: &str) -> String {
    format!(
        "<section class=hero><div><p class=overline>A price · stock · a version · or the whole page</p><h1>Get told when a<br><span>web page changes.</span></h1><p class=lead>Zetlyn watches a web page, or one place on it, and tells you what it said before and what it says now. No password, no dashboard to learn.</p>{field}<p class=dim>{note}</p></div>{HERO_CARD}</section>"
    )
}

/// What a watch is, beside the field: a shop's page with the place that is watched, and the change
/// Zetlyn reports for it. The console is drawn here, plainly; no maker's picture or mark.
const HERO_CARD: &str = r#"<div class=demo aria-label="An example: a console's availability on a shop's page, watched, and the change Zetlyn reports">
<div class=browser>
<div class=browser-bar><i></i><i></i><i></i><span>shop.example/ps5-pro</span></div>
<div class=product>
<div class=product-img><svg viewBox="0 0 120 150" width="96" height="120" aria-hidden="true"><path d="M30 12 C22 40 20 110 26 140 L48 140 C44 104 44 46 52 12 Z" fill="var(--white)" stroke="var(--ink)" stroke-width="2"/><path d="M90 12 C98 40 100 110 94 140 L72 140 C76 104 76 46 68 12 Z" fill="var(--white)" stroke="var(--ink)" stroke-width="2"/><path d="M52 12 L68 12 C76 46 76 104 72 140 L48 140 C44 104 44 46 52 12 Z" fill="var(--ink)"/><line x1="60" y1="30" x2="60" y2="70" stroke="var(--line-2)" stroke-width="2"/></svg></div>
<div class=product-info>
<p class=product-kicker>Consoles</p>
<p class=product-name>PlayStation 5 Pro</p>
<p class=product-spec>2 TB · Disc drive sold separately</p>
<p class=product-avail><span>Availability</span><b class=marked>In stock</b></p>
<span class=fake-button>Add to cart</span>
</div>
</div>
</div>
<div class="card demo-card">
<div class=card-head><span>Changed <b>shop.example/ps5-pro</b></span><span class=signal>notified</span></div>
<div class=card-row><span>Before</span><s>Out of stock</s></div>
<div class="card-row hot"><span>Now</span><b>In stock</b></div>
<div class=card-row><span>Noticed</span>18 Oct 2026, 09:00 UTC</div>
<p class=card-foot>One value, before and now. The clock, the visitor count and the advert on the page are never reported.</p>
</div>
</div>"#;

const LANDING: &str = r#"<section class=block><p class=overline>How it works</p><h2>Three steps.<br><span>Then it runs on its own.</span></h2>
<div class=steps>
<div><h3>Paste the address</h3><p>Any public web page: a pricing page, a release list, terms, a status page.</p></div>
<div><h3>Click what matters</h3><p>Zetlyn shows the page. Click the price, the version, the date, or watch the whole page.</p></div>
<div><h3>Get notified</h3><p>When it changes you get the value before and now, with a link to the page.</p></div>
</div></section>
<section class="block split">
<div><p class=overline>Not a diff</p><h2>Watch one place, or the whole page.<br><span>Get the value, not forty lines of HTML.</span></h2></div>
<div><p>Other tools email you every changed line. Zetlyn tells you what moved: <b>Out of stock → In stock</b>. Clocks, counters and adverts that change on every visit are recognised when you start and never reported.</p><p>When a page is rebuilt, Zetlyn finds your place again by what stands beside it. If it cannot be sure, it asks you to mark it again rather than send a change that is not one.</p></div>
</section>
<section class="block uses"><p class=overline>What people watch</p><h2>Price drops, back in stock, new versions.<br><span>Anything a page says.</span></h2><ul>
<li><b>Prices</b> of a competitor, a product, a plan</li>
<li><b>Releases</b> of software they depend on</li>
<li><b>Terms</b> and privacy policies of services they use</li>
<li><b>Rates</b> and figures published by banks and offices</li>
<li><b>Status pages</b> and notices</li>
<li><b>Job pages</b>, calls and tenders</li>
</ul></section>
<section class=block><p class=overline>For agents and code</p><h2>From Claude, ChatGPT and your own code.<br><span>One address: zetlyn.com/mcp.</span></h2>
<p class=lead>Add Zetlyn to Claude, ChatGPT, Cursor or any MCP client with that address, sign in once, and ask it to watch a page for you. Programs use the HTTP API and signed webhooks. <a href="/docs">How to connect</a>.</p>
</section>
"#;

const FAQ: &str = r#"<section class="block faq"><p class=overline>Questions</p><h2>Before you paste.</h2>
<details><summary>Do I need an account?</summary><p>No, not one you make. When you watch your first page, Zetlyn makes an account for your email address by itself and sends you a link to confirm; the watch starts when you click it. After that you are signed in. There is no password.</p></details>
<details><summary>Which pages can it watch?</summary><p>Public web pages that show their content without signing in. Pages behind a login, a paywall or a captcha are not read, and Zetlyn does not try to get around a site that refuses it. Coming next: PDF documents, CSV and Excel files, JSON APIs, and RSS and Atom feeds.</p></details>
<details><summary>How often does it check?</summary><p>Every hour. With a subscription you choose for each watch: every hour, every 15 or 5 minutes, or every minute.</p></details>
</section>"#;

/// On an instance nobody signs in to: how the checks came out so far, and checking all now rather
/// than waiting for the hourly round.
fn local_box(store: &Store) -> Result<String, String> {
    let tally = store.tally()?;
    let tally: Vec<String> = tally.iter().map(|(o, n, moved)| if *moved > 0 { format!("{o} {n} ({moved} found again)") } else { format!("{o} {n}") }).collect();
    Ok(format!(
        "<div class=box><h3>This instance</h3><p class=dim>Checks so far: {}</p><form method=post action=/check><button>Check all now</button></form></div>",
        esc(&if tally.is_empty() { "none yet".to_string() } else { tally.join(" · ") })
    ))
}


/// The address a link to `/mark` names. `url=` comes last there, and all after it is the
/// address: typed by hand it may hold `?` and `&` of its own and is taken as it is; encoded, as a
/// form sends it, it is decoded.
fn named_url(query: &str) -> Option<String> {
    let at = if query.starts_with("url=") { 0 } else { query.find("&url=")? + 1 };
    let raw = &query[at + 4..];
    Some(if raw.contains("://") { raw.to_string() } else { percent_decoded(raw) })
}

/// `%xx` decoded, and nothing else: a `+` in an address is a plus.
fn percent_decoded(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        match (b[i], b.get(i + 1).copied().and_then(hex), b.get(i + 2).copied().and_then(hex)) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (c, _, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Marking: the page over the whole window, and a panel over it that can be folded away or moved
/// to the other side, since most pages are wide already.
fn mark_page(app: &App, store: &Store, a: &Asked) -> Result<Reply, String> {
    let remark = match a.q("watch") {
        Some(id) => owned(store, a, &id)?,
        None => None,
    };
    // No room for one more watch: the way to it, not a page to mark that would only wait.
    if remark.is_none() && a.account.as_ref().is_some_and(|acc| !can_add(app, acc)) {
        return Ok(redirect("/subscribe"));
    }
    let url = match (&remark, named_url(&a.query)) {
        (Some(w), _) => w.url.clone(),
        (None, Some(u)) => match address(&u) {
            Ok(u) => u,
            Err(e) => return Ok(redirect(&format!("/?error={}", enc(&e)))),
        },
        (None, None) => return Ok(redirect("/")),
    };
    let ask_email = app.hosted.is_some() && a.account.is_none();
    // The page as served, or as a browser shows it once its scripts ran (asked for, or `frame` finds it bare).
    let browser = a.q("browser").is_some();
    // Not signed in: Google or Apple instead of the address, where they are set up.
    let others: String = if ask_email {
        app.oauth
            .providers()
            .iter()
            .map(|p| match *p {
                "google" => format!("<button name=via value=google formnovalidate disabled class=\"oauth google needs-mark\">{GOOGLE_G}Continue with Google</button>"),
                _ => format!("<button name=via value=apple formnovalidate disabled class=\"oauth apple needs-mark\">{APPLE_LOGO}Continue with Apple</button>"),
            })
            .collect()
    } else {
        String::new()
    };
    let others = if others.is_empty() { others } else { format!("<p class=or><span>or</span></p><div class=oauth-buttons>{others}</div>") };
    let email = if ask_email { r#"<label class=email>Your email address<input type=email name=email placeholder="you@example.com" required autocomplete=email></label>"# } else { "" };
    let error = a.q("error").map(|e| format!("<p class=error>{}</p>", esc(&e))).unwrap_or_default();
    let (title, button) = if remark.is_some() { ("Mark the place again", "Save") } else { ("What should Zetlyn watch?", "Watch") };
    let host = url.split_once("://").map_or(url.as_str(), |(_, r)| r).split(['/', '?', '#']).next().unwrap_or("");
    // When to tell: on any change, or once a condition comes true; marked again, as it was.
    let held = remark.as_ref().and_then(|w| w.condition.clone());
    // Coming back from an error, what was chosen then; else the watch's own.
    let asked = a.q("kind").filter(|k| ["any", "below", "above", "contains", "not_contains"].contains(&k.as_str()));
    let current = asked.as_deref().unwrap_or_else(|| held.as_ref().map_or("any", |k| k.kind()));
    let kinds = [("any", "On every change"), ("below", "When the number is below"), ("above", "When the number is above"), ("contains", "When it says"), ("not_contains", "When it no longer says")];
    let kinds: String = kinds.iter().map(|(k, label)| format!("<option value={k}{}{}>{label}</option>", if *k == current { " selected" } else { "" }, if k.starts_with("below") || k.starts_with("above") { " class=number" } else { "" })).collect();
    let operand = if asked.is_some() { a.q("operand").unwrap_or_default() } else { held.as_ref().map(|k| k.operand()).unwrap_or_default() };
    let tell = format!("<div class=tell><label for=kind>Tell me</label><div class=tell-row><select name=kind id=kind>{kinds}</select><input name=operand id=operand value=\"{}\" placeholder=\"399 or In stock\" aria-label=\"Number or text\"{}></div></div>", esc(&operand), if current == "any" { " hidden" } else { "" });
    let body = format!(
        r#"<div class=bar><a href="/" class=brand><img src="/mark.png" alt="">Zetlyn</a><span class=url title="{u}">{u}</span><a href="/" class=cancel>Cancel</a></div>
<div class=stage>
  <iframe id=frame src="/frame?{browser}url={q}" sandbox="allow-scripts" referrerpolicy="no-referrer" title="The page"></iframe>
  <div class=loading id=loading><div class=spinner></div><p>Reading <b>{h}</b></p><p class=dim id=slow hidden>Still reading: this page builds itself with scripts, which takes a few seconds.</p></div>
</div>
<aside class=panel id=panel>
  <div class=panel-head><h2>{title}</h2><span><button type=button class=link id=side title="Move to the other side">⇆</button><button type=button class=link id=fold title="Fold away">–</button></span></div>
  <div class=panel-body>
  <p id=hint>Click the place on the page that matters, or watch the whole page.</p>
  <div class=picked><div class=dim id=label>Nothing marked yet</div><div id=text></div></div>
  <p class=row><button type=button id=bigger disabled>Larger</button><button type=button id=whole>Whole page</button></p>
  <p class=dim id=cleared hidden></p>
  <p class=dim>Something over the page in the way? <button type=button class=link id=remove>Remove it</button></p>
  {fuller}
  {error}
  <form method=post action=/watch>
    <input type=hidden name=anchor id=anchor><input type=hidden name=watch value="{w}"><input type=hidden name=url value="{u}">
    {tell}
    {email}
    <button class=primary id=go disabled>{button}</button>
    {others}
  </form>
  <p class=dim>{mark_note}</p>
  </div>
</aside>
<script>
const frame = document.getElementById('frame'), $ = id => document.getElementById(id);
const slow = setTimeout(() => {{ $('slow').hidden = false; }}, 3500);
frame.addEventListener('load', () => {{ clearTimeout(slow); $('loading').hidden = true; }});
$('fold').onclick = () => {{ const f = $('panel').classList.toggle('folded'); $('fold').textContent = f ? '+' : '–'; $('fold').title = f ? 'Open' : 'Fold away'; }};
$('side').onclick = () => $('panel').classList.toggle('left');
window.addEventListener('message', e => {{
  if (e.source !== frame.contentWindow || !e.data) return;
  if (e.data.zetlyn === 'cleared') {{ if (e.data.count > 0) {{ $('cleared').textContent = (e.data.count === 1 ? 'One box' : e.data.count + ' boxes') + ' over the page, such as a cookie notice, hidden. Zetlyn reads the page beneath them.'; $('cleared').hidden = false; }} return; }}
  if (e.data.zetlyn === 'removed') {{ $('remove').textContent = 'Remove it'; return; }}
  if (e.data.zetlyn !== 'picked') return;
  const a = e.data.anchor;
  $('anchor').value = JSON.stringify(a);
  $('text').textContent = a.text || '(empty)';
  $('label').textContent = a.label ? 'Beside “' + a.label + '”' : 'Marked';
  $('go').disabled = false; document.querySelectorAll('.needs-mark').forEach(b => {{ b.disabled = false; }}); $('bigger').disabled = false;
  numbers(true);
  $('panel').classList.remove('folded'); $('fold').textContent = '–';
}});
const numbers = on => {{ document.querySelectorAll('#kind option.number').forEach(o => {{ o.disabled = !on; }}); if (!on && $('kind').selectedOptions[0].disabled) {{ $('kind').value = 'any'; $('operand').hidden = true; }} }};
$('kind').onchange = () => {{ $('operand').hidden = $('kind').value === 'any'; if (!$('operand').hidden) $('operand').focus(); }};
$('bigger').onclick = () => frame.contentWindow.postMessage('bigger', '*');
$('remove').onclick = () => {{ frame.contentWindow.postMessage('remove', '*'); $('remove').textContent = 'Now click it on the page'; }};
$('whole').onclick = () => {{
  frame.contentWindow.postMessage('clear', '*');
  $('anchor').value = ''; $('label').textContent = 'The whole page'; $('text').textContent = 'Any change to the text of the page is reported.';
  $('go').disabled = false; document.querySelectorAll('.needs-mark').forEach(b => {{ b.disabled = false; }}); $('bigger').disabled = true;
  numbers(false);
}};
</script>"#,
        u = esc(&url),
        q = enc(&url),
        browser = if browser { "browser=1&" } else { "" },
        fuller = if browser || !crate::render::available() {
            String::new()
        } else {
            format!("<p class=dim>Something missing? <a href=\"/mark?browser=1&{}url={}\">Show it as a browser would</a>, which takes a few seconds.</p>", remark.as_ref().map(|w| format!("watch={}&", w.key)).unwrap_or_default(), enc(&url))
        },
        h = esc(host),
        w = remark.map(|w| w.key).unwrap_or_default(),
        mark_note = crate::plan::MARK_NOTE,
    );
    Ok(html(format!(
        "<!doctype html><html lang=en><head><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\"><meta name=theme-color content=\"#f2efe7\"><title>{}</title><link rel=icon href=\"/favicon.png\" type=image/png><style>{CSS}</style></head><body class=marking>{body}</body></html>",
        esc(title)
    )))
}

/// After a link was mailed: where to look, what the mail says, and what happens on the click.
fn sent(app: &App, a: &Asked) -> String {
    let to = a.q("to").unwrap_or_default();
    let signin = a.q("signin").is_some();
    let page = a.q("page").filter(|p| p.starts_with("http"));
    let from = app.hosted.as_ref().map(|h| h.mailer.from.clone()).unwrap_or_default();
    let sender = from.rsplit_once('<').map_or(from.as_str(), |(_, r)| r.trim_end_matches('>')).to_string();
    let subject = match (&page, signin) {
        (_, true) => "Your Zetlyn sign-in link".to_string(),
        (Some(p), false) => format!("Confirm: watch {}", mail::short(p)),
        (None, false) => "Confirm: watch …".to_string(),
    };
    let (overline, lead, steps) = if signin {
        (
            "Sign in",
            format!("We sent a link to <b>{}</b>. It signs you in, and makes your account if there is none yet.", esc(&to)),
            ["Open the mail", "Click the link in it. It works once, for 30 minutes.", "You are signed in. There is no password."],
        )
    } else {
        (
            "One step left",
            format!("We sent a link to <b>{}</b>. The watch starts when you confirm it.", esc(&to)),
            ["Open the mail", "Click the link in it. It works for two days.", "Zetlyn starts watching and you are signed in. There is no password."],
        )
    };
    let steps: String = steps
        .iter()
        .enumerate()
        .map(|(i, s)| match i {
            0 => format!("<li><b>{s}</b> from {}, subject “{}”.</li>", esc(if sender.is_empty() { "Zetlyn" } else { &sender }), esc(&subject)),
            _ => format!("<li>{s}</li>"),
        })
        .collect();
    let waiting = match (&page, signin) {
        (Some(p), false) => format!(
            "<div class=sent-watch><span class=overline>Waiting to start</span><a class=url href=\"{u}\" target=_blank rel=noopener>{u}</a><span class=dim>{}</span></div>",
            esc(&a.q("what").unwrap_or_default()),
            u = esc(p)
        ),
        _ => String::new(),
    };
    let open = match webmail(&to, &sender) {
        Some((name, link)) => format!("<p><a class=\"button primary\" href=\"{}\" target=_blank rel=noopener>Open {name}</a></p>", esc(&link)),
        None => String::new(),
    };
    let again = match (&page, signin) {
        (Some(p), false) => format!("<a href=\"/mark?url={}\">Start again</a>", enc(p)),
        (_, true) => "<a href=\"/signin\">Try again</a>".to_string(),
        _ => "<a href=\"/\">Start again</a>".to_string(),
    };
    format!(
        r#"<section class=sent><div class=sent-card>
<svg class=envelope viewBox="0 0 48 48" aria-hidden=true><rect x="5" y="11" width="38" height="27" fill="none" stroke="currentColor" stroke-width="2.5"/><path d="M6 13l18 14 18-14" fill="none" stroke="currentColor" stroke-width="2.5"/><circle cx="40" cy="12" r="7" fill="var(--signal)"/></svg>
<p class=overline>{overline}</p><h1>Check your inbox</h1><p class=lead>{lead}</p>
<ol class=sent-steps>{steps}</ol>{waiting}{open}
<p class="dim small">Nothing there after a few minutes? Look in the spam or promotions folder{}. Wrong address? {again}.</p>
</div></section>"#,
        if sender.is_empty() { String::new() } else { format!(", and add {} to your contacts", esc(&sender)) }
    )
}

/// The webmail of an address, where it is a well-known one: its name, and where to open it
/// (for Gmail, at the mail itself).
fn webmail(to: &str, sender: &str) -> Option<(&'static str, String)> {
    let domain = to.rsplit_once('@')?.1.to_ascii_lowercase();
    let d = domain.as_str();
    Some(match d {
        "gmail.com" | "googlemail.com" => ("Gmail", format!("https://mail.google.com/mail/u/0/#search/{}", enc(&format!("from:{}", if sender.is_empty() { "zetlyn" } else { sender })))),
        "outlook.com" | "hotmail.com" | "hotmail.de" | "live.com" | "live.de" | "msn.com" => ("Outlook", "https://outlook.live.com/mail/".into()),
        "yahoo.com" | "yahoo.de" => ("Yahoo Mail", "https://mail.yahoo.com/".into()),
        "icloud.com" | "me.com" | "mac.com" => ("iCloud Mail", "https://www.icloud.com/mail".into()),
        "gmx.de" | "gmx.net" | "gmx.at" | "gmx.ch" => ("GMX", "https://www.gmx.net/".into()),
        "web.de" => ("WEB.DE", "https://web.de/".into()),
        "t-online.de" => ("Telekom Mail", "https://email.t-online.de/".into()),
        "proton.me" | "protonmail.com" | "pm.me" => ("Proton Mail", "https://mail.proton.me/".into()),
        "posteo.de" | "posteo.net" => ("Posteo", "https://posteo.de/webmail/".into()),
        _ => return None,
    })
}

/// `/signin`: Google or Apple where set up, or an address in and a link out; what went wrong with
/// the last try, if anything.
fn signin_page(app: &App, a: &Asked) -> String {
    let said = if a.q("expired").is_some() {
        "<p class=error>That link has expired or was used already. Links work once, for a short time; ask for a new one.</p>"
    } else if a.q("bad").is_some() {
        "<p class=error>That does not look like an email address. Please check it.</p>"
    } else if a.q("failed").is_some() {
        "<p class=error>Signing in that way did not work. Please try again, or use your email address.</p>"
    } else if a.q("connect").is_some() {
        "<p class=notice>An app asks to use Zetlyn for you. Sign in (or sign up), and Zetlyn asks you to allow it.</p>"
    } else {
        ""
    };
    let buttons: String = app
        .oauth
        .providers()
        .iter()
        .map(|p| match *p {
            "google" => format!("<a class=\"oauth google\" href=\"/oauth/google\">{GOOGLE_G}Continue with Google</a>"),
            _ => format!("<a class=\"oauth apple\" href=\"/oauth/apple\">{APPLE_LOGO}Continue with Apple</a>"),
        })
        .collect();
    let names: Vec<&str> = app.oauth.providers().iter().map(|p| if *p == "google" { "Google" } else { "Apple" }).collect();
    let (lead, or) = if buttons.is_empty() {
        ("Enter your email address. Zetlyn sends you a link that signs you in, and makes your account if you have none yet. There is no password.".to_string(), String::new())
    } else {
        (
            format!("Continue with {}, or with your email address. Either way, your account is made the first time; there is no password.", names.join(" or ")),
            format!("<div class=oauth-buttons>{buttons}</div><p class=or><span>or</span></p>"),
        )
    };
    format!(
        r#"<section class=sent><div class="sent-card signin">
<img src="/mark.png" alt="" class=signin-mark>
<p class=overline>Sign in or sign up</p><h1>Welcome to Zetlyn</h1>
<p class=lead>{lead}</p>
{said}{or}
<form method=post action=/signin class=signin-form>
  <label for=email>Email address</label>
  <input type=email id=email name=email placeholder="you@example.com" required autocomplete=email>
  <button class=primary>Send sign-in link</button>
</form>
</div></section>"#
    )
}

/// The marks the providers ask their buttons to carry.
const GOOGLE_G: &str = r##"<svg viewBox="0 0 48 48" aria-hidden=true><path fill="#EA4335" d="M24 9.5c3.54 0 6.71 1.22 9.21 3.6l6.85-6.85C35.9 2.38 30.47 0 24 0 14.62 0 6.51 5.38 2.56 13.22l7.98 6.19C12.43 13.72 17.74 9.5 24 9.5z"/><path fill="#4285F4" d="M46.98 24.55c0-1.57-.15-3.09-.38-4.55H24v9.02h12.94c-.58 2.96-2.26 5.48-4.78 7.18l7.73 6c4.51-4.18 7.09-10.36 7.09-17.65z"/><path fill="#FBBC05" d="M10.53 28.59c-.48-1.45-.76-2.99-.76-4.59s.27-3.14.76-4.59l-7.98-6.19C.92 16.46 0 20.12 0 24c0 3.88.92 7.54 2.56 10.78l7.97-6.19z"/><path fill="#34A853" d="M24 48c6.48 0 11.93-2.13 15.89-5.81l-7.73-6c-2.15 1.45-4.92 2.3-8.16 2.3-6.26 0-11.57-4.22-13.47-9.91l-7.98 6.19C6.51 42.62 14.62 48 24 48z"/></svg>"##;
const APPLE_LOGO: &str = r#"<svg viewBox="0 0 384 512" aria-hidden=true><path fill="currentColor" d="M318.7 268.7c-.2-36.7 16.4-64.4 50-84.8-18.8-26.9-47.2-41.7-84.7-44.6-35.5-2.8-74.3 20.7-88.5 20.7-15 0-49.4-19.7-76.4-19.7C63.3 141.2 4 184.8 4 273.5q0 39.3 14.4 81.2c12.8 36.7 59 126.7 107.2 125.2 25.2-.6 43-17.9 75.8-17.9 31.8 0 48.3 17.9 76.4 17.9 48.6-.7 90.4-82.5 102.6-119.3-65.2-30.7-61.7-90-61.7-91.9zm-56.6-164.2c27.3-32.4 24.8-61.9 24-72.5-24.1 1.4-52 16.4-67.9 34.9-17.5 19.8-27.8 44.3-25.6 71.9 26.1 2 49.9-11.4 69.5-34.3z"/></svg>"#;

/// A request to the API: its key read, its account found, its answer as JSON.
fn programmatic(app: &App, store: &Store, a: &Asked, call: impl FnOnce(&crate::api::Caller) -> crate::api::Answer) -> Reply {
    // An API key (zk_…), or an access token an app got by OAuth (zat_…).
    let id = a.bearer.as_deref().and_then(|k| if k.starts_with("zat_") { store.access_account(k).ok().flatten() } else { store.api_account(k).ok().flatten() });
    let account = id.and_then(|id| store.account_by_id(id).ok().flatten());
    let Some(account) = account else {
        // Where an MCP client finds out how to sign in (RFC 9728).
        let metadata = format!("{}/.well-known/oauth-protected-resource/mcp", public_base(app, a));
        return json_reply(401, &serde_json::json!({ "error": "Sign in: connect by OAuth (the client finds how at /.well-known/oauth-protected-resource), or send an API key as Authorization: Bearer zk_… (make one under Account, API keys at https://zetlyn.com/account/keys)." }))
            .with_header(header("WWW-Authenticate", &format!("Bearer realm=\"zetlyn\", resource_metadata=\"{metadata}\"")));
    };
    if !app.allow("api", &account.id.to_string(), 600, 600) {
        return json_reply(429, &serde_json::json!({ "error": "Too many requests for this account; try again in a few minutes." }));
    }
    let base = app.hosted.as_ref().map(|h| h.base.clone()).unwrap_or_else(|| "http://127.0.0.1:4848".into());
    // How it came: an app connected by OAuth by its name, else a key of the account.
    let via = match a.bearer.as_deref() {
        Some(t) if t.starts_with("zat_") => store.access_client(t).ok().flatten().unwrap_or_else(|| "oauth".into()),
        _ => "api".into(),
    };
    let caller = crate::api::Caller { store, account, hosted: app.hosted.as_ref(), plan: &app.plan, every_min: app.every_min, base, via };
    let (status, body) = call(&caller);
    json_reply(status, &body)
}

/// MCP over HTTP: one JSON-RPC message, or a batch of them, by POST; a notification gets 202.
fn mcp_reply(app: &App, store: &Store, a: &Asked) -> Reply {
    let message: serde_json::Value = match serde_json::from_slice(&a.body) {
        Ok(m) => m,
        Err(_) => return json_reply(400, &serde_json::json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "Not JSON." } })),
    };
    let mut answers: Option<serde_json::Value> = None;
    let reply = programmatic(app, store, a, |c| {
        // A tool that reads a page counts against the same limit as the API's calls that do.
        let one = |m: &serde_json::Value| {
            if crate::api::reads_a_page(m) && !app.allow("api-fetch", &c.account.id.to_string(), 60, 600) {
                crate::api::too_many_reads(m)
            } else {
                crate::api::mcp(c, m)
            }
        };
        let out = match &message {
            serde_json::Value::Array(batch) => {
                let all: Vec<serde_json::Value> = batch.iter().filter_map(one).collect();
                if all.is_empty() { None } else { Some(serde_json::Value::Array(all)) }
            }
            m => one(m),
        };
        answers = out.clone();
        (200, out.unwrap_or(serde_json::Value::Null))
    });
    if reply.status_code().0 == 200 && answers.is_none() {
        return Response::from_string("").with_status_code(202);
    }
    reply
}

fn json_reply(status: u16, body: &serde_json::Value) -> Reply {
    Response::from_string(serde_json::to_string_pretty(body).unwrap_or_default()).with_status_code(status).with_header(header("Content-Type", "application/json"))
}





fn layout(app: &App, a: &Asked, title: &str, main: &str) -> String {
    let nav = match (&app.hosted, &a.account) {
        (Some(_), Some(_)) => r#"<a href="/watches">Watches</a><a href="/account">Account</a><form method=post action=/signout><button class=link>Sign out</button></form>"#,
        (Some(_), None) => r#"<a href="/signin" class=boxed>Sign in</a>"#,
        (None, _) => r#"<a href="/watches">Watches</a><a href="/account">Account</a>"#,
    };
    let nav = match admin_of(app, a) {
        Some(_) => format!("<a href=\"/admin\">Admin</a>{nav}"),
        None => nav.to_string(),
    };
    let banner = app.banner.lock().map(|b| b.clone()).unwrap_or_default();
    let banner = if banner.trim().is_empty() { String::new() } else { format!("<p class=banner>{}</p>", esc(&banner)) };
    let links = FOOTER;
    let footer = format!("<footer><span>© Zetlyn</span><div>{links}</div></footer>");
    // In the tab and in a search result the page says it is Zetlyn's.
    let title = if title.contains("Zetlyn") { title.to_string() } else { format!("{title} — Zetlyn") };
    let seo = crate::seo::head(&public_base(app, a), &a.path, &title, FAQ);
    format!(
        "<!doctype html><html lang=en><head><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\"><meta name=theme-color content=\"#f2efe7\"><title>{}</title>{seo}<link rel=icon href=\"/favicon.png\" type=image/png><style>{CSS}</style></head><body><header><a href=\"/\" class=brand><img src=\"/mark.png\" alt=\"\">Zetlyn</a><nav>{nav}</nav></header>{banner}<main>{main}</main>{footer}<script>{LOCAL_TIME}</script></body></html>",
        esc(&title)
    )
}

fn page_reply(app: &App, a: &Asked, title: &str, main: &str, code: u16) -> Reply {
    html(layout(app, a, title, main)).with_status_code(code)
}

fn not_found(app: &App, a: &Asked) -> Reply {
    page_reply(app, a, "Not here", "<section class=narrow><h1>Not here</h1><p>There is nothing at this address. <a href=\"/\">Watch a page</a>.</p></section>", 404)
}

const CSS: &str = include_str!("../assets/app.css");

fn html(s: String) -> Reply {
    Response::from_string(s).with_header(header("Content-Type", "text/html; charset=utf-8"))
}

fn redirect(to: &str) -> Reply {
    Response::from_string("").with_status_code(303).with_header(header("Location", to))
}

/// The session cookie; empty ends it.
fn cookie(app: &App, token: &str) -> Header {
    let secure = app.hosted.as_ref().is_some_and(|h| h.base.starts_with("https://"));
    let age = if token.is_empty() { 0 } else { 90 * 86400 };
    header("Set-Cookie", &format!("z={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={age}{}", if secure { "; Secure" } else { "" }))
}

fn header(k: &str, v: &str) -> Header {
    Header::from_bytes(k.as_bytes(), v.as_bytes()).expect("a header Zetlyn writes")
}

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn enc(s: &str) -> String {
    form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() > n { format!("{}…", s.chars().take(n - 1).collect::<String>()) } else { s.to_string() }
}

fn first_line(s: &str) -> String {
    let mut lines = s.lines();
    let first = lines.next().unwrap_or("").to_string();
    let more = lines.count();
    if more > 0 { format!("{first} (+{more} lines)") } else { first }
}

fn ago(at: i64) -> String {
    match now() - at {
        s if s < 90 => "just now".into(),
        s if s < 5400 => format!("{} min ago", s / 60),
        s if s < 172800 => format!("{} h ago", s / 3600),
        s => format!("{} days ago", s / 86400),
    }
}

/// A moment as a date and time in UTC.
pub fn time(at: i64) -> String {
    let days = at.div_euclid(86400);
    let secs = at.rem_euclid(86400);
    // Howard Hinnant's days-to-civil.
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y}-{m:02}-{d:02} {:02}:{:02} UTC", secs / 3600, secs % 3600 / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moment_reads_as_its_date() {
        assert_eq!(time(0), "1970-01-01 00:00 UTC");
        assert_eq!(time(1_792_310_400), "2026-10-18 08:00 UTC");
    }

    #[test]
    fn a_private_address_is_refused() {
        assert!(address("127.0.0.1/x").is_err());
        assert!(address("http://10.0.0.1/").is_err());
    }

    #[test]
    fn the_address_to_mark_is_all_after_url_typed_or_encoded() {
        assert_eq!(named_url("url=cataproof.com").as_deref(), Some("cataproof.com"));
        assert_eq!(named_url("url=https://shop.example/p?id=4&size=XL").as_deref(), Some("https://shop.example/p?id=4&size=XL"));
        assert_eq!(named_url("url=https%3A%2F%2Fshop.example%2Fp%3Fid%3D4%26size%3DXL").as_deref(), Some("https://shop.example/p?id=4&size=XL"));
        assert_eq!(named_url("error=Try%20again&watch=3&url=https%3A%2F%2Fa.example%2F").as_deref(), Some("https://a.example/"));
        assert_eq!(named_url("url=https://a.example/c++").as_deref(), Some("https://a.example/c++"));
        assert_eq!(named_url("watch=3"), None);
    }
}

/// The mark: a river through a round, drawn for the first Zetlyn and kept.
const MARK: &[u8] = include_bytes!("../assets/mark.png");
const FAVICON: &[u8] = include_bytes!("../assets/favicon.png");
/// The card a shared link shows (Open Graph), 1200 × 630; made from `assets/src/og.html`.
const OG: &[u8] = include_bytes!("../assets/og.png");

fn image(bytes: &'static [u8]) -> Reply {
    Response::from_data(bytes).with_header(header("Content-Type", "image/png")).with_header(header("Cache-Control", "public, max-age=86400"))
}

/// The account asking, if it may use `/admin`: one named in `ZETLYN_ADMINS`, or anybody on a
/// local instance, which is its operator's own.
fn admin_of<'a>(app: &App, a: &'a Asked) -> Option<&'a Account> {
    let acc = a.account.as_ref()?;
    (app.hosted.is_none() || app.admins.iter().any(|x| *x == acc.email)).then_some(acc)
}
/// The address's host, without `www.`: what a page is called on Zetlyn's own pages.
fn host_of(url: &str) -> String {
    url.split_once("://").map_or(url, |(_, r)| r).split(['/', '?', '#']).next().unwrap_or(url).trim_start_matches("www.").to_string()
}

/// A moment as the reader's clock has it: UTC here, put in their own time by the page's script.
fn when(at: i64) -> String {
    let t = time(at);
    format!("<time datetime=\"{}T{}:00Z\">{t}</time>", &t[..10], &t[11..16])
}

/// Where a watch stands, for a person: a class for its colour and a few words.
fn standing(w: &Watch) -> (&'static str, String) {
    match (w.state.as_str(), w.until) {
        ("pending", _) => ("pending", "Waiting for confirmation".into()),
        ("waiting", _) => ("waiting", "Waiting for a subscription".into()),
        ("ended", _) => ("ended", "Ended".into()),
        (_, Some(u)) => ("running", format!("Free until {}", when(u))),
        _ => ("running", "Watching".into()),
    }
}

/// How a check came out, for a person.
fn outcome(o: &str) -> &'static str {
    match o {
        "new" => "First read",
        "same" => "No change",
        "changed" => "Changed",
        "unclear" => "Place unclear",
        "gone" => "Place gone",
        "failed" => "Could not read the page",
        _ => "",
    }
}


fn watch_table(watches: &[Watch]) -> String {
    let mut rows = String::new();
    for w in watches {
        let (class, standing) = standing(w);
        let last = if w.state == "running" { format!("{} · {}", outcome(&w.status), w.last_checked.map(ago).unwrap_or_default()) } else { String::new() };
        rows.push_str(&format!(
            "<tr><td><a href=\"/watches/{key}\" class=host>{host}</a><div class=dim>{what}</div></td><td class=value>{value}</td><td><span class=\"state {class}\">{standing}</span><div class=dim>{last}</div></td></tr>",
            key = w.key,
            host = esc(&host_of(&w.url)),
            what = esc(&life::what(w)),
            value = esc(&clip(&first_line(w.last_value.as_deref().unwrap_or("")), 70)),
            last = esc(&last),
        ));
    }
    format!("<div class=scroll><table class=list><tr class=head><td>Page</td><td>Now</td><td>State</td></tr>{rows}</table></div>")
}


/// How often a watch is checked, said in words.
fn every(minutes: i64) -> String {
    match minutes {
        60 => "every hour".into(),
        1 => "every minute".into(),
        m => format!("every {m} minutes"),
    }
}

/// The choice of how often a watch is checked: a select where the account may choose, the way to
/// more where it may not.
fn interval_form(w: &Watch, faster: bool) -> String {
    let word = every(w.interval_min);
    let word = format!("{}{}", word[..1].to_uppercase(), &word[1..]);
    if !faster {
        return format!("{word} <span class=dim>· <a href=\"/subscribe\">more often with a subscription</a></span>");
    }
    let options: String = crate::plan::INTERVALS.iter().map(|m| format!("<option value={m}{}>{}</option>", if *m == w.interval_min { " selected" } else { "" }, every(*m))).collect();
    format!("<form method=post action=\"/watches/{}/every\" class=inline><select name=minutes aria-label=\"How often\" onchange=\"this.form.submit()\">{options}</select><noscript><button>Save</button></noscript></form>", w.key)
}

/// When the watch tells: on every change, or once a condition comes true (`when.rs`), and the
/// form that sets it. A number is read in one place, so the whole page offers only the texts.
fn when_form(w: &Watch, bad: Option<&str>) -> String {
    let current = w.condition.as_ref().map_or("any", |k| k.kind());
    let operand = w.condition.as_ref().map(|k| k.operand()).unwrap_or_default();
    let mut kinds = vec![("any", "On every change")];
    if matches!(w.mode, crate::store::Mode::Place(_)) {
        kinds.extend([("below", "When the number is below"), ("above", "When the number is above")]);
    }
    kinds.extend([("contains", "When it says"), ("not_contains", "When it no longer says")]);
    let options: String = kinds.iter().map(|(k, label)| format!("<option value={k}{}>{label}</option>", if *k == current { " selected" } else { "" })).collect();
    let met = match &w.condition {
        Some(k) if k.met(w.last_value.as_deref().unwrap_or("")) => "<span class=\"state running\">true now</span>",
        _ => "",
    };
    format!(
        "<form method=post action=\"/watches/{key}/when\" class=when-form><select name=kind aria-label=\"When to tell\" onchange=\"this.form.operand.hidden = this.value == 'any'\">{options}</select><input name=operand value=\"{v}\" aria-label=\"Number or text\" placeholder=\"399 or In stock\"{hide}><button>Save</button>{met}</form>{err}",
        key = w.key,
        v = esc(&operand),
        hide = if current == "any" { " hidden" } else { "" },
        err = match bad {
            None => String::new(),
            Some("") => "<p class=error>That did not work: give a number for below and above, a text for the others.</p>".to_string(),
            Some(why) => format!("<p class=error>{}</p>", esc(why)),
        },
    )
}

/// `/watches/{id}`: one watch, from what it reads now to every check.
fn detail(app: &App, store: &Store, w: &Watch, faster: bool, to: Option<&str>, bad: Option<String>) -> Result<String, String> {
    let (class, standing) = standing(w);
    let next = w.last_checked.unwrap_or(w.created) + w.interval_min * 60;
    let tell = match to {
        Some(t) => format!("an email to <b>{}</b>", esc(t)),
        None => "this page".into(),
    };
    // Said from what the watch is, not from where the reader came from.
    let notice = match (w.state.as_str(), w.status.as_str()) {
        ("running", "new") => format!("<div class=notice><b>Zetlyn is watching this page.</b> The first check is {}; when it changes, you get {tell}. Until then there is nothing to do.</div>", when(next)),
        ("pending", _) => "<div class=notice>Waiting for confirmation: click the link in the mail Zetlyn sent you, and the watch starts.</div>".into(),
        ("waiting", _) => "<div class=\"notice warn\">Your free watch is used. <a href=\"/subscribe\">Subscribe</a>, and this watch starts at once.</div>".into(),
        ("running", "unclear" | "gone") => format!("<div class=\"notice warn\">Zetlyn cannot be sure where the place you marked is now: the page was rebuilt. <a href=\"/mark?watch={}\">Mark it again</a>.</div>", w.key),
        ("running", "failed") => "<div class=\"notice warn\">The last check could not read the page. Zetlyn tries again at the next one.</div>".into(),
        _ => String::new(),
    };
    let value = w.last_value.as_deref().unwrap_or("");
    let lines: Vec<&str> = value.lines().collect();
    let now = if lines.len() <= 1 {
        format!("<div class=now-value>{}</div>", esc(&clip(value, 300)))
    } else {
        let head: String = lines.iter().take(8).map(|l| format!("{}\n", esc(&clip(l, 200)))).collect();
        let rest: String = lines.iter().skip(8).take(400).map(|l| format!("{}\n", esc(&clip(l, 200)))).collect();
        let more = if lines.len() > 8 { format!("<details><summary>{} lines more</summary><pre>{rest}</pre></details>", lines.len() - 8) } else { String::new() };
        format!("<pre class=now-text>{head}</pre>{more}")
    };
    let mut changes = String::new();
    for c in store.changes(w.id, 20)? {
        let (gone, came) = crate::text::diff(&c.before, &c.after);
        let lines: String = gone.iter().take(10).map(|l| format!("<div class=gone>− {}</div>", esc(&clip(l, 200)))).chain(came.iter().take(10).map(|l| format!("<div class=came>+ {}</div>", esc(&clip(l, 200))))).collect();
        changes.push_str(&format!("<li><span class=dim>{}</span>{lines}</li>", when(c.at)));
    }
    let changes = if changes.is_empty() { "<p class=dim>No change yet. Zetlyn writes each one here, and tells you.</p>".into() } else { format!("<ul class=changes>{changes}</ul>") };
    let mut checks = String::new();
    for c in store.checks(w.id, 50)? {
        checks.push_str(&format!(
            "<tr><td class=dim>{}</td><td><span class=\"dot {o}\"></span> {}{}</td></tr>",
            when(c.at),
            outcome(&c.outcome),
            if c.moved { " · found again" } else { "" },
            o = esc(&c.outcome),
        ));
    }
    let mut facts = vec![("Checked", interval_form(w, faster)), ("Tell me", when_form(w, bad.as_deref()))];
    if w.state == "running" {
        facts.push(("Next check", when(next)));
        facts.push(("Last check", format!("{} · {}", w.last_checked.map(ago).unwrap_or_else(|| "not yet".into()), outcome(&w.status))));
    }
    if app.hosted.is_some() {
        facts.push(("Changes go to", match to { Some(t) => esc(t), None => "–".into() }));
    }
    facts.push((if w.state == "running" { "Watching since" } else { "Made" }, when(w.created)));
    if w.render {
        facts.push(("Read", "in a browser, after the page's scripts ran".into()));
    }
    let facts: String = facts.iter().map(|(k, v)| format!("<dt>{k}</dt><dd>{v}</dd>")).collect();
    Ok(format!(
        r#"<section class=watch>
<p class=crumb><a href="/watches">Your watches</a> <span class=dim>/</span> {host}</p>
<span class="state {class}">{standing}</span>
<h1>{host}</h1>
<a class=address href="{u}" target=_blank rel=noopener>{u}</a>
{notice}
<div class=watch-grid>
  <div class=now-card><p class=overline>{what}</p>{now}<p class=row><a class=button href="/mark?watch={id}">Mark again</a></p></div>
  <dl class=facts>{facts}</dl>
</div>
<h2>Changes</h2>{changes}
<h2>Checks</h2><div class=scroll><table>{checks}</table></div>
<details class=end><summary>End this watch</summary><p class=dim>Zetlyn stops watching and deletes the watch with its changes and checks.</p><form method=post action="/watches/{id}/end"><button>End and delete</button></form></details>
</section>"#,
        host = esc(&host_of(&w.url)),
        u = esc(&w.url),
        what = esc(&life::what(w)),
        id = w.key,
    ))
}

/// Every `<time>` on a page in the reader's own time and way of writing it (`when`).
const LOCAL_TIME: &str = "for (const t of document.querySelectorAll('time[datetime]')) { const d = new Date(t.dateTime); if (!isNaN(d)) t.textContent = d.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' }); }";
/// Whether the account may start one more watch: always on a local instance or with a
/// subscription, and on a free trial only while its one watch is not yet taken.
fn can_add(app: &App, acc: &Account) -> bool {
    app.hosted.is_none() || acc.paid || !acc.trial_used
}

/// The plan an account is on: its name, and what it holds now.
fn plan_state(acc: &Account, watches: &[Watch]) -> (&'static str, String) {
    let running = watches.iter().filter(|w| w.state == "running").count() as i64;
    let extra = if running > acc.quota { format!(", {} more at 0.50 € each", running - acc.quota) } else { String::new() };
    if acc.free {
        ("Free account", format!("{running} of {} watches, given by Zetlyn", acc.quota))
    } else if acc.paid {
        ("Subscription", format!("{running} of {} watches{extra}", acc.quota))
    } else {
        let until = watches.iter().filter(|w| w.state == "running").filter_map(|w| w.until).max();
        (
            "Free trial",
            match until {
                Some(u) => format!("One watch, free until {}", when(u)),
                None if acc.trial_used => "Your free trial is used".into(),
                None => "One watch, free for 3 days".into(),
            },
        )
    }
}

fn price() -> String {
    format!("{}.{:02} €", crate::billing::BASE_CENTS / 100, crate::billing::BASE_CENTS % 100)
}

/// The plan the account is on, beside its watches: what it has, and the way to more, said once
/// and quietly.
fn plan_card(acc: &Account, watches: &[Watch]) -> String {
    let (name, said) = plan_state(acc, watches);
    let more = if acc.free {
        String::new()
    } else if acc.paid {
        "<a href=\"/account/billing\" class=upgrade>Billing and invoices →</a>".into()
    } else {
        format!("<a href=\"/subscribe\" class=upgrade>Upgrade · {} a month →</a>", price())
    };
    format!("<aside class=plan-card><p class=overline>Your plan</p><p class=plan-name>{name}</p><p class=dim>{}</p>{more}</aside>", said)
}

/// Shown instead of the page to mark when the account has no room for one more watch.
const UPGRADE_DIALOG: &str = r#"<dialog id=upgrade class=upgrade-dialog aria-labelledby=upgrade-title>
<form method=dialog class=close><button aria-label="Close">×</button></form>
<p class=overline>Upgrade</p>
<h2 id=upgrade-title>Watch more than one page</h2>
<p>Your free trial covers one watch. With a subscription Zetlyn watches up to 10 pages for you, for as long as you like.</p>
<ul class=ticks><li>Up to 10 watches; 0.50 € a month for each one more</li><li>Checked as often as every minute</li><li>Cancel any time</li></ul>
<p class=price-line><b>PRICE</b> a month, VAT included</p>
<div class=row><a class="button primary" href="/subscribe">Subscribe</a><form method=dialog><button>Not now</button></form></div>
</dialog>
<script>
document.getElementById('add').addEventListener('submit', e => { e.preventDefault(); document.getElementById('upgrade').showModal(); });
</script>"#;

/// `/watches`: the account's watches, and a field for one more.
fn watches_page(app: &App, store: &Store, acc: &Account) -> Result<String, String> {
    let watches: Vec<Watch> = store.watches_of(acc.id)?.into_iter().filter(|w| w.state != "ended" || w.until.is_some_and(|u| now() - u < 30 * 86400)).collect();
    let running = watches.iter().filter(|w| w.state == "running").count();
    // Without room for one more, the field stays and the button tells how to get it.
    let room = can_add(app, acc);
    let field = format!(
        "<form class=field action=/mark id=add><input name=url placeholder=\"Paste the address of a page\" required><button class=primary>Watch a page</button></form>{}",
        if room { String::new() } else { UPGRADE_DIALOG.replace("PRICE", &price()) }
    );
    let list = if watches.is_empty() {
        "<div class=empty><p><b>No watches yet.</b></p><p class=dim>Paste the address of a page above, mark the place that matters, and Zetlyn tells you when it changes.</p></div>".to_string()
    } else {
        watch_table(&watches)
    };
    let local = if acc.email == LOCAL { local_box(store)? } else { String::new() };
    let plan = if acc.email == LOCAL { String::new() } else { plan_card(acc, &watches) };
    Ok(format!(
        "<section class=watches><div class=watches-head><div><h1>Your watches</h1><p class=dim>{running} running{}</p></div>{plan}</div>{field}{list}{local}</section>",
        if acc.email == LOCAL { String::new() } else { format!(" · changes go to {}", esc(&acc.email)) }
    ))
}

/// The account's pages share a side bar: General, Billing (where people pay), API keys.
fn account_shell(app: &App, a: &Asked, active: &str, body: &str) -> String {
    let items = [("General", "/account", true), ("Notifications", "/account/notifications", app.hosted.is_some()), ("Billing", "/account/billing", app.hosted.is_some()), ("API keys", "/account/keys", true)];
    let nav: String = items.iter().filter(|(_, _, shown)| *shown).map(|(name, href, _)| format!("<a href=\"{href}\"{}>{name}</a>", if *name == active { " class=on aria-current=page" } else { "" })).collect();
    // Who runs the instance finds its admin pages here too, not only in the header.
    let admin = if admin_of(app, a).is_some() { "<a href=\"/admin\" class=admin-link>Admin →</a>" } else { "" };
    let out = if app.hosted.is_some() { "<form method=post action=/signout><button class=link>Sign out</button></form>" } else { "" };
    format!("<div class=settings-page><aside class=settings-nav><p class=overline>Account</p><nav>{nav}</nav>{admin}{out}</aside><div class=settings-main>{body}</div></div>")
}

/// `/account`: who, the plan in short, where changes go, the account's data.
fn account_page(app: &App, store: &Store, acc: &Account, a: &Asked) -> Result<String, String> {
    let local = acc.email == LOCAL;
    let watches = store.watches_of(acc.id)?;
    let running = watches.iter().filter(|w| w.state == "running").count() as i64;
    let section = |id: &str, title: &str, about: &str, body: String| format!("<section class=setting id={id}><div class=setting-label><h2>{title}</h2><p class=dim>{about}</p></div><div class=setting-body>{body}</div></section>");
    let mut out = String::new();
    let head = if local {
        "<div class=profile><div><h1>Account</h1><p class=dim>This instance's own account: nobody signs in, no mail is sent.</p></div></div>".to_string()
    } else {
        let initial = acc.email.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
        format!(
            "<div class=profile><span class=avatar aria-hidden=true>{}</span><div><h1>{}</h1><p class=dim>Member since {} · {running} of your watches running</p>{}</div></div>",
            esc(&initial),
            esc(&acc.email),
            when(acc.created),
            if app.hosted.is_some() {
                "<details class=change-address><summary>Change address</summary><form method=post action=/account/email class=row><input type=email name=email required placeholder=\"new@example.com\" autocomplete=email aria-label=\"New email address\"><button>Send a link</button></form><p class=dim>Zetlyn mails a link to the new address; following it moves the account there.</p></details>"
            } else {
                ""
            }
        )
    };
    if app.hosted.is_some() && !local {
        let (name, said) = plan_state(acc, &watches);
        let chip = if acc.free {
            "<span class=\"state running\">Given</span>"
        } else if acc.paid {
            "<span class=\"state running\">Active</span>"
        } else {
            "<span class=\"state waiting\">Trial</span>"
        };
        let usage = if acc.paid {
            let pct = (running * 100 / acc.quota.max(1)).min(100);
            format!("<div class=usage><span style=\"width:{pct}%\"></span></div>")
        } else {
            String::new()
        };
        let action = if acc.free {
            String::new()
        } else if acc.paid {
            format!("<p class=dim>{} a month, VAT included.</p><p><a class=button href=\"/account/billing\">Billing and invoices</a></p>", price())
        } else {
            format!(
                "<p class=with>With a subscription:</p><ul class=ticks><li>Up to 10 watches; 0.50 € a month for each one more</li><li>Checked as often as every minute</li><li>Cancel any time</li></ul><div class=row><a class=\"button primary\" href=\"/subscribe\">Subscribe · {} a month</a></div><p class=dim>VAT included.</p>",
                price()
            )
        };
        out.push_str(&section("plan", "Plan", "What your account includes.", format!("<div class=plan-line><p class=plan-name>{name}</p>{chip}</div><p class=dim>{said}</p>{usage}{action}")));
        let mails = store.mails_changes(acc.id)?;
        let mut summary = format!("<li><span><b>Email</b><span class=dim>{}</span></span><span class=\"state {}\">{}</span></li>", esc(&acc.email), if mails { "running" } else { "waiting" }, if mails { "On" } else { "Off" });
        for c in store.channels_of(acc.id)? {
            let name = match c.kind.as_str() { "telegram" => "Telegram", "slack" => "Slack", "discord" => "Discord", _ => "Webhook" };
            summary.push_str(&format!("<li><span><b>{name}</b></span><span class=\"state running\">On</span></li>"));
        }
        out.push_str(&section(
            "notifications",
            "Notifications",
            "Where Zetlyn tells you about a change.",
            format!(
                "<ul class=channels>{}</ul><p><a class=button href=\"/account/notifications\">Manage notifications</a></p>",
                summary
            ),
        ));
        out.push_str(&section(
            "data",
            "Your data",
            "What Zetlyn keeps, and how it goes.",
            "<p>Your address, your watches with their checks and changes, and nothing else. A watch you end is deleted with all it holds; an account without a subscription goes 30 days after its last watch. <a href=\"/privacy\">Privacy</a></p><details class=delete-account><summary>Delete account</summary><p>Your watches with their checks and changes, and your API keys, are deleted at once; that cannot be undone. A subscription is cancelled at Stripe right away, with nothing more charged; the invoices stay there, as the law requires.</p><form method=post action=/account/delete class=row><input name=confirm placeholder=\"Type your email address\" autocomplete=off aria-label=\"Your email address, to confirm\"><button class=danger>Delete my account</button></form></details>".into(),
        ));
    }
    Ok(account_shell(app, a, "General", &format!("{head}<div class=settings>{out}</div>")))
}

/// How many channels an account may have besides its mail.
const CHANNELS: usize = 5;

/// `/account/notifications`: where changes are told: the account's mail, Telegram, Slack,
/// Discord, a webhook of its own (`tell.rs`).
fn notifications_page(app: &App, store: &Store, acc: &Account, a: &Asked) -> Result<String, String> {
    let channels = store.channels_of(acc.id)?;
    let mails = store.mails_changes(acc.id)?;
    let note = match (a.q("error"), a.q("tested").or_else(|| a.q("added").map(|_| String::new()))) {
        (Some(e), _) => format!("<p class=error>{}</p>", esc(&e)),
        (None, Some(id)) if id.is_empty() => "<div class=notice>Added. Send a test to see that it arrives.</div>".to_string(),
        (None, Some(id)) => match channels.iter().find(|c| c.id.to_string() == id) {
            Some(c) if c.last_status == "ok" => "<div class=notice>Sent: a test change should be there now.</div>".to_string(),
            Some(c) => format!("<div class=\"notice warn\">The test did not arrive: {}</div>", esc(&c.last_status)),
            None => String::new(),
        },
        _ => String::new(),
    };
    let name = |k: &str| match k {
        "telegram" => "Telegram",
        "slack" => "Slack",
        "discord" => "Discord",
        _ => "Webhook",
    };
    let shown = |c: &crate::store::Channel| match c.kind.as_str() {
        "telegram" => "A chat with the Zetlyn bot".to_string(),
        "webhook" => esc(&c.target),
        // The secret part of a Slack or Discord address stays off the page.
        _ => esc(&c.target.splitn(6, '/').take(5).collect::<Vec<_>>().join("/")) + "/…",
    };
    let mut rows = String::new();
    for c in &channels {
        let status = match (c.last_at, c.last_status.as_str()) {
            (None, _) => "<span class=dim>Nothing sent yet</span>".to_string(),
            (Some(t), "ok") => format!("<span class=dim>Last sent {}</span>", ago(t)),
            (Some(t), e) => format!("<span class=error-text>Failed {}: {}</span>", ago(t), esc(e)),
        };
        let secret = if c.kind == "webhook" {
            format!("<details class=secret><summary>Signing secret</summary><code>{}</code><p class=dim>Each delivery carries <code>Zetlyn-Signature: t=…,v1=…</code>: the HMAC-SHA256 of <code>t.body</code> with this secret, in hex. <a href=\"/llms.txt\">Format</a></p></details>", esc(&c.secret))
        } else {
            String::new()
        };
        rows.push_str(&format!(
            "<li><span><b>{}</b><span class=dim>{}</span>{status}{secret}</span><span class=channel-actions><form method=post action=\"/account/notifications/{id}/test\" class=inline-form><button class=link>Send a test</button></form><form method=post action=\"/account/notifications/{id}/delete\" class=inline-form><button class=link>Remove</button></form></span></li>",
            name(&c.kind),
            shown(c),
            id = c.id
        ));
    }
    let others = !channels.is_empty();
    let mail_switch = format!(
        "<form method=post action=/account/notifications/mail class=inline-form><input type=hidden name=on value={}><button class=link{}>{}</button></form>",
        if mails { 0 } else { 1 },
        if mails && !others { " disabled title=\"Add another channel first\"" } else { "" },
        if mails { "Turn off" } else { "Turn on" }
    );
    let mail_row = format!(
        "<li><span><b>Email</b><span class=dim>{}</span></span><span class=channel-actions><span class=\"state {}\">{}</span>{mail_switch}</span></li>",
        esc(&acc.email),
        if mails { "running" } else { "waiting" },
        if mails { "On" } else { "Off" }
    );
    let telegram = match crate::tell::bot() {
        Some(_) => "<form method=post action=/account/notifications/telegram class=add-channel><div><b>Telegram</b><p class=dim>Opens a chat with the Zetlyn bot; press Start there, and changes come to that chat.</p></div><button>Connect Telegram</button></form>".to_string(),
        None => String::new(),
    };
    let add = |kind: &str, title: &str, about: &str, placeholder: &str| {
        format!("<form method=post action=/account/notifications/add class=add-channel><input type=hidden name=kind value={kind}><div><b>{title}</b><p class=dim>{about}</p></div><div class=row><input name=target type=url required placeholder=\"{placeholder}\" aria-label=\"{title} address\"><button>Add</button></div></form>")
    };
    let full = channels.len() >= CHANNELS;
    let adding = if full {
        format!("<p class=dim>{CHANNELS} channels is the most; remove one to add another.</p>")
    } else {
        format!(
            "{telegram}{}{}{}",
            add("slack", "Slack", "An incoming webhook of your workspace: Slack, Apps, Incoming Webhooks, Add to a channel.", "https://hooks.slack.com/services/…"),
            add("discord", "Discord", "A channel's webhook: the channel's settings, Integrations, Webhooks, New Webhook, Copy URL.", "https://discord.com/api/webhooks/…"),
            add("webhook", "Webhook", "Your own address: each change is posted to it as JSON, signed, for a program or an automation (n8n, Zapier, Make).", "https://…"),
        )
    };
    Ok(account_shell(
        app,
        a,
        "Notifications",
        &format!("<h1>Notifications</h1><p class=lead>Where Zetlyn tells you that a page changed. Every channel here is told each change; reminders about your plan go by email only.</p>{note}<ul class=channels>{mail_row}{rows}</ul><h2>Add a channel</h2><div class=add-channels>{adding}</div>"),
    ))
}

/// Connecting Telegram: the ways to start the chat with the bot. A t.me link opens the app only
/// where Telegram is installed; elsewhere the browser cannot follow it, so the phone (by a QR code),
/// Telegram Web and the command by hand are offered beside it.
fn telegram_page(app: &App, a: &Asked, bot: &crate::tell::Bot, token: &str) -> String {
    let link = bot.link(token);
    let web = format!("https://web.telegram.org/k/#?tgaddr={}", enc(&format!("tg://resolve?domain={}&start={token}", bot.name)));
    let command = format!("/start {token}");
    account_shell(
        app,
        a,
        "Notifications",
        &format!(
            r#"<p class=crumb><a href="/account/notifications">Notifications</a> <span class=dim>/</span> Telegram</p>
<h1>Connect Telegram</h1>
<p class=lead>Zetlyn tells changes in a chat with its bot, <b>@{name}</b>. Start that chat in one of these ways; this page works for an hour.</p>
<div class=tg-ways>
  <div class=tg-way><h2>On your phone</h2><p class=dim>Scan this with the phone's camera, open it in Telegram and press Start.</p><div class=qr>{qr}</div></div>
  <div class=tg-way><h2>On this computer</h2><p class=dim>Needs the Telegram app here. Press Start in the chat it opens.</p><p><a class="button primary" href="{link}" target=_blank rel=noopener>Open Telegram</a></p><p class=dim>No app? <a href="{web}" target=_blank rel=noopener>Telegram in the browser</a>, signed in with your phone number.</p></div>
  <div class=tg-way><h2>By hand</h2><p class=dim>Find <b>@{name}</b> in Telegram and send it this message:</p><code class=tg-command id=tgcmd>{command}</code><p><button type=button onclick="navigator.clipboard.writeText(document.getElementById('tgcmd').textContent).then(() => {{ this.textContent = 'Copied' }})">Copy</button></p></div>
</div>
<p>When the bot answers “Connected”, <a href="/account/notifications">go back to your notifications</a> and send a test.</p>"#,
            name = esc(&bot.name),
            qr = qr_svg(&link),
            link = esc(&link),
            web = esc(&web),
            command = esc(&command),
        ),
    )
}

/// A QR code as SVG, black on white, with the quiet zone around it that readers need.
fn qr_svg(text: &str) -> String {
    let Ok(qr) = qrcodegen::QrCode::encode_text(text, qrcodegen::QrCodeEcc::Medium) else { return String::new() };
    let n = qr.size();
    let mut path = String::new();
    for y in 0..n {
        for x in 0..n {
            if qr.get_module(x, y) {
                path.push_str(&format!("M{x},{y}h1v1h-1z"));
            }
        }
    }
    format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"-4 -4 {s} {s}\" shape-rendering=crispEdges role=img aria-label=\"QR code for the Telegram bot\"><rect x=-4 y=-4 width={s} height={s} fill=#fff /><path d=\"{path}\" fill=#000 /></svg>", s = n + 8)
}

/// The changes `/account/notifications` asks for.
fn notifications_post(app: &App, store: &Store, acc: &Account, a: &Asked, rest: &[&str]) -> Result<Reply, String> {
    let back = |e: &str| redirect(&format!("/account/notifications?error={}", enc(e)));
    let channels = store.channels_of(acc.id)?;
    let own = |id: &str| channels.iter().find(|c| c.id.to_string() == id);
    Ok(match rest {
        ["mail"] => {
            let on = a.f("on") == "1";
            if !on && channels.is_empty() {
                return Ok(back("Add another channel before turning email off, or you would hear of no change at all."));
            }
            store.set_mails_changes(acc.id, on)?;
            redirect("/account/notifications")
        }
        ["telegram"] => match crate::tell::bot() {
            Some(_) if channels.len() >= CHANNELS => back("Remove a channel first."),
            Some(bot) => page_reply(app, a, "Connect Telegram", &telegram_page(app, a, bot, &store.new_token(acc.id, "telegram", None, 3600)?), 200),
            None => back("Telegram is not set up here."),
        },
        ["add"] => {
            if channels.len() >= CHANNELS {
                return Ok(back("Remove a channel first."));
            }
            let kind = a.f("kind");
            match crate::tell::target_for(&kind, &a.f("target")) {
                Ok(target) => {
                    let secret = if kind == "webhook" { format!("zws_{}", &crate::crypto::token()[..40]) } else { String::new() };
                    let id = store.add_channel(acc.id, &kind, &target, &secret)?;
                    redirect(&format!("/account/notifications?added={id}"))
                }
                Err(e) => back(&e),
            }
        }
        [id, "delete"] => {
            if let Some(c) = own(id) {
                store.delete_channel(acc.id, c.id)?;
                // The last other channel gone: the mail is on again, so that nothing goes unsaid.
                if channels.len() == 1 {
                    store.set_mails_changes(acc.id, true)?;
                }
            }
            redirect("/account/notifications")
        }
        [id, "test"] => match own(id) {
            Some(c) => {
                let said = crate::tell::send(c, &crate::tell::Event::test(&public_base(app, a))).err().unwrap_or_else(|| "ok".into());
                store.set_channel_status(c.id, &said)?;
                redirect(&format!("/account/notifications?tested={}", c.id))
            }
            None => redirect("/account/notifications"),
        },
        _ => not_found(app, a),
    })
}

/// `/account/keys`: keys for the API and the MCP server, and how to connect them.
fn keys_page(app: &App, store: &Store, acc: &Account, a: &Asked) -> Result<String, String> {
    let base = public_base(app, a);
    let mut rows = String::new();
    for (id, name, hint, created, used) in store.api_keys_of(acc.id)? {
        rows.push_str(&format!(
            "<tr><td><b>{}</b><div class=dim>zk_…{}</div></td><td class=dim>{}</td><td class=dim>{}</td><td><form method=post action=\"/account/keys/{id}/delete\"><button>Delete</button></form></td></tr>",
            esc(&name),
            esc(&hint),
            when(created),
            used.map(ago).unwrap_or_else(|| "Never".into())
        ));
    }
    let list = if rows.is_empty() {
        "<div class=empty><p><b>No keys yet.</b></p><p class=dim>Make one below; Zetlyn shows it once, with how to connect it.</p></div>".to_string()
    } else {
        format!("<div class=scroll><table class=keys><tr class=head><td>Key</td><td>Made</td><td>Last used</td><td></td></tr>{rows}</table></div>")
    };
    // Apps let in by signing in (OAuth): no key to copy, and each can be let out here.
    let apps: String = store
        .clients_of(acc.id)?
        .iter()
        .map(|(id, name, since)| format!("<tr><td><b>{}</b></td><td class=dim>Since {}</td><td><form method=post action=\"/account/apps/{}/disconnect\"><button>Disconnect</button></form></td></tr>", esc(name), when(*since), esc(id)))
        .collect();
    let apps = if apps.is_empty() {
        "<p class=dim>None yet. An app that connects by signing in, such as Claude with <code>/mcp</code> as a connector, shows here.</p>".to_string()
    } else {
        format!("<div class=scroll><table class=keys>{apps}</table></div>")
    };
    Ok(account_shell(
        app,
        a,
        "API keys",
        &format!(
            "<h1>API keys</h1><p class=lead>A key lets a program, or an LLM such as Claude, watch pages for you through the API or the MCP server at <code>{}/mcp</code>.</p>{list}<form method=post action=/account/keys class=\"row make-key\"><input name=name placeholder=\"What it is for, e.g. Claude\" class=keyname aria-label=\"What the key is for\"><button class=primary>Make a key</button></form><h2>Connected apps</h2>{apps}<h2>How to connect</h2><p class=dim>Most apps need only the address <code>{}/mcp</code> and ask you to sign in. Where an app wants a key instead, put yours in place of <code>zk_…</code>; the page that shows a new key fills it in for you.</p>{}",
            esc(&base),
            esc(&base),
            connect_help(&base, "zk_…")
        ),
    ))
}

/// Where this app is reached from outside: what goes into instructions to copy.
fn public_base(app: &App, a: &Asked) -> String {
    match &app.hosted {
        Some(h) => h.base.clone(),
        None => origin(app, a),
    }
}

/// How to connect an MCP client or a program, with `KEY` and `BASE` put in.
const CONNECT: &str = r#"<div class=connect>
<h3>Claude (web, desktop, mobile)</h3>
<p class=dim>Settings → Connectors → Add custom connector. Name: Zetlyn; URL as below. Then Connect: you sign in to Zetlyn and allow Claude. No key needed.</p>
<div class=snippet><pre id=c-url>BASE/mcp</pre><button type=button class=copy data-copy=c-url>Copy</button></div>
<h3>Claude Code</h3>
<p class=dim>In a terminal; then <code>/mcp</code> in Claude Code, choose zetlyn, and sign in in the browser it opens.</p>
<div class=snippet><pre id=c-code>claude mcp add --transport http --scope user zetlyn BASE/mcp</pre><button type=button class=copy data-copy=c-code>Copy</button></div>
<h3>ChatGPT, Cursor, VS Code and other MCP clients</h3>
<p class=dim>Add an MCP server by its URL; the client asks you to sign in to Zetlyn. Where a client wants a configuration file:</p>
<div class=snippet><pre id=c-other>{
  "mcpServers": {
    "zetlyn": { "type": "http", "url": "BASE/mcp" }
  }
}</pre><button type=button class=copy data-copy=c-other>Copy</button></div>
<h3>With a key instead</h3>
<p class=dim>For a program, or a client that cannot sign in: the key goes in a header.</p>
<div class=snippet><pre id=c-key-code>claude mcp add --transport http --scope user zetlyn BASE/mcp --header "Authorization: Bearer KEY"</pre><button type=button class=copy data-copy=c-key-code>Copy</button></div>
<div class=snippet><pre id=c-api>curl -H "Authorization: Bearer KEY" BASE/api/v1/watches</pre><button type=button class=copy data-copy=c-api>Copy</button></div>
<p class=dim>All calls and tools are in <a href="/llms.txt">llms.txt</a>.</p>
</div>
<script>
for (const b of document.querySelectorAll('button.copy')) b.addEventListener('click', () => {
  navigator.clipboard.writeText(document.getElementById(b.dataset.copy).textContent).then(() => { b.textContent = 'Copied'; setTimeout(() => { b.textContent = 'Copy'; }, 1500); });
});
</script>"#;

fn connect_help(base: &str, key: &str) -> String {
    CONNECT.replace("BASE", &esc(base)).replace("KEY", &esc(key))
}

/// A key just made: shown this once, with how to put it to use.
fn key_page(key: &str, base: &str) -> String {
    format!(
        "<section class=account><p class=crumb><a href=\"/account/keys\">API keys</a> <span class=dim>/</span> New key</p><h1>Your API key</h1><p>Copy it now: Zetlyn keeps only a fingerprint of it and cannot show it again.</p><div class=snippet><pre class=key id=c-key>{}</pre><button type=button class=copy data-copy=c-key>Copy</button></div><h2>Connect</h2><p>The instructions below already hold your key.</p>{}<p><a href=\"/account/keys\">Back to your keys</a></p></section>",
        esc(key),
        connect_help(base, key)
    )
}

/// Off to Google or Apple, with what their answer must carry kept in a cookie (`oauth.rs`).
fn oauth_begin(app: &App, a: &Asked, provider: &str) -> Reply {
    match app.oauth.begin(provider, &public_base(app, a)) {
        Some((url, kept)) => redirect(&url).with_header(oauth_cookie(app, "zo", &kept)),
        None => not_found(app, a),
    }
}

/// Back from Google or Apple: the person signed in, their account made if it was not there, and
/// the watch they marked before going (`watch`, via) made and started: the provider vouched for
/// the address, so no mail needs to.
fn oauth_back(app: &App, store: &Store, a: &Asked, provider: &str, state: &str, code: &str) -> Result<Reply, String> {
    let (gone, gone_w) = (oauth_cookie(app, "zo", ""), oauth_cookie(app, "zw", ""));
    let intent = {
        let id = cookie_named(a, "zw");
        let mut intents = app.intents.lock().unwrap_or_else(|e| e.into_inner());
        intents.remove(&id).filter(|i| now() - i.at < INTENT_TTL)
    };
    // They said no, or came back with nothing: back where they were, without a word.
    if code.is_empty() {
        let to = intent.map(|i| format!("/mark?url={}", enc(&i.url))).unwrap_or_else(|| "/signin".into());
        return Ok(redirect(&to).with_header(gone).with_header(gone_w));
    }
    let person = match app.oauth.finish(provider, &cookie_named(a, "zo"), state, code, &public_base(app, a)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("sign-in with {provider}: {e}");
            return Ok(redirect("/signin?failed=1").with_header(gone).with_header(gone_w));
        }
    };
    let acc = store.account_by_id(store.account(&person.email)?)?.ok_or("the account just made")?;
    if acc.blocked {
        return Ok(redirect("/signin?failed=1").with_header(gone).with_header(gone_w));
    }
    store.confirm_account(acc.id)?;
    store.step(acc.id, "confirmed", provider)?;
    let session = store.new_session(acc.id)?;
    let to = match (intent, &app.hosted) {
        (Some(i), Some(h)) => {
            let id = store.add_watch_as(acc.id, &i.url, &i.mode, app.every_min, None, &i.value, "pending")?;
            store.set_condition(id, i.condition.as_ref())?;
            store.step(acc.id, "first_watch", "web")?;
            store.set_render(id, i.render)?;
            let key = store.watch(id)?.map(|w| w.key).unwrap_or_default();
            started(life::start(store, h, &app.plan, acc.id, id)?, &key)
        }
        _ => authz::next_after_signin(a).unwrap_or_else(|| "/watches".to_string()),
    };
    Ok(redirect(&to).with_header(cookie(app, &session)).with_header(gone).with_header(gone_w).with_header(authz::next_cookie(app, "")))
}

/// A cookie of ten minutes for a sign-in with Google or Apple: `zo` keeps its state and nonce, `zw`
/// names the watch waiting for it. Apple answers by a form posted from its own site, which a
/// SameSite=Lax cookie would not come back with.
fn oauth_cookie(app: &App, name: &str, value: &str) -> Header {
    let secure = app.hosted.as_ref().is_some_and(|h| h.base.starts_with("https://"));
    let age = if value.is_empty() { 0 } else { 600 };
    header("Set-Cookie", &format!("{name}={value}; Path=/oauth; HttpOnly; Max-Age={age}{}", if secure { "; SameSite=None; Secure" } else { "; SameSite=Lax" }))
}

/// How long a watch marked before signing in with Google or Apple waits for the person to return.
const INTENT_TTL: i64 = 15 * 60;

/// A watch marked and read, waiting for its account (`watch`, via Google or Apple).
pub struct Intent {
    url: String,
    mode: crate::store::Mode,
    value: String,
    render: bool,
    condition: Option<crate::when::Condition>,
    at: i64,
}

fn cookie_named(a: &Asked, name: &str) -> String {
    a.headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("cookie"))
        .flat_map(|(_, v)| v.split(';'))
        .filter_map(|c| c.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
        .unwrap_or_default()
}

/// `/account/billing`: the plan, what it costs and when, the card, the invoices, and the way to
/// change the card or cancel. Stripe's checkout comes back here; that the subscription has just
/// begun is told by the subscription, not by the address.
fn billing_page(app: &App, store: &Store, acc: &Account) -> Result<String, String> {
    // Back from the checkout before Stripe's event: what the checkout says is taken now.
    let paid = match app.plan.settle(store, acc) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("billing page, account {}: {e}", acc.id);
            acc.paid
        }
    };
    let acc = &store.account_by_id(acc.id)?.unwrap_or_else(|| acc.clone());
    let watches = store.watches_of(acc.id)?;
    let running = watches.iter().filter(|w| w.state == "running").count() as i64;
    let euros = |cents: i64| format!("{}.{:02} €", cents / 100, cents % 100);
    let portal = "<form method=post action=/billing target=_blank><button>Change card, or cancel ↗</button></form>";
    let crumb = "<h1>Billing</h1>";
    if acc.free {
        return Ok(format!("<section class=account>{crumb}<div class=bill-card><p class=plan-name>Free account</p><p class=dim>Given by Zetlyn: {running} of {} watches, nothing to pay.</p></div></section>", acc.quota));
    }
    if !paid || acc.stripe_subscription.is_empty() {
        let pending = !store.checkout_session(acc.id).is_empty();
        let said = if pending {
            "<div class=notice>Your payment is being confirmed. This page shows your subscription in a moment; reload it if it does not.</div><script>setTimeout(() => location.reload(), 4000)</script>"
        } else {
            ""
        };
        return Ok(format!(
            "<section class=account>{crumb}{said}<div class=bill-card><p class=plan-name>Free trial</p><p class=dim>No subscription, so no invoices yet.</p><p><a class=\"button primary\" href=\"/subscribe\">Subscribe · {} a month</a></p></div></section>",
            euros(crate::billing::BASE_CENTS)
        ));
    }
    let Some(stripe) = &app.plan.stripe else { return Ok(format!("<section class=account>{crumb}<p class=dim>Payment is not set up on this instance.</p></section>")) };
    let overview = stripe.overview(&acc.stripe_subscription);
    let mut out = String::new();
    match &overview {
        Ok(o) => {
            if now() - o.created < 30 * 60 {
                out.push_str("<div class=notice><b>Thank you.</b> Your subscription is active: your watches run without an end, as often as you choose. The invoice is on its way by email and below.</div>");
            }
            let state = if o.ending {
                format!("Ends on {}; your watches end with it.", when(o.period_end))
            } else if o.status == "past_due" {
                "A payment did not go through; Stripe tries again. Please check your card.".to_string()
            } else {
                format!("Next payment {} on {}.", euros(o.monthly), when(o.period_end))
            };
            let pct = (running * 100 / acc.quota.max(1)).min(100);
            out.push_str(&format!(
                "<div class=bill-card><div class=bill-head><div><p class=plan-name>Subscription</p><p class=dim>{} a month, VAT included</p></div><span class=\"state running\">{}</span></div>\
                 <dl class=facts><dt>Watches</dt><dd>{running} of {}<div class=usage><span style=\"width:{pct}%\"></span></div></dd><dt>Payment</dt><dd>{state}</dd><dt>Card</dt><dd>{}</dd></dl>{portal}</div>",
                euros(o.monthly),
                esc(if o.ending { "ending" } else { &o.status }),
                acc.quota,
                esc(o.card.as_deref().unwrap_or("–")),
            ));
        }
        Err(e) => {
            eprintln!("billing page, account {}: {e}", acc.id);
            out.push_str(&format!("<div class=bill-card><p class=plan-name>Subscription</p><p class=dim>Stripe does not answer just now.</p>{portal}</div>"));
        }
    }
    let invoices = match stripe.invoices(&acc.stripe_customer) {
        Ok(list) if list.is_empty() => "<p class=dim>No invoices yet.</p>".to_string(),
        Ok(list) => {
            let rows: String = list
                .iter()
                .map(|i| {
                    let link = if !i.pdf.is_empty() { format!("<a href=\"{}\" target=_blank rel=noopener>PDF ↗</a>", esc(&i.pdf)) } else if !i.page.is_empty() { format!("<a href=\"{}\" target=_blank rel=noopener>View ↗</a>", esc(&i.page)) } else { String::new() };
                    format!(
                        "<tr><td>{}</td><td class=dim>{}</td><td>{}<div class=dim>incl. {} VAT</div></td><td><span class=\"state {}\">{}</span></td><td>{link}</td></tr>",
                        when(i.created),
                        esc(&i.number),
                        euros(i.total),
                        euros(i.tax),
                        if i.status == "paid" { "running" } else { "waiting" },
                        esc(&i.status)
                    )
                })
                .collect();
            format!("<div class=scroll><table class=invoices>{rows}</table></div>")
        }
        Err(e) => {
            eprintln!("billing page, invoices of account {}: {e}", acc.id);
            format!("<p>Your invoices are kept by Stripe, our payment provider; each one is also sent to you by email.</p><form method=post action=/billing target=_blank><button>See your invoices ↗</button></form>")
        }
    };
    Ok(format!("<section class=account>{crumb}{out}<h2>Invoices</h2>{invoices}</section>"))
}

/// The owner deletes the account: the address typed again, a subscription cancelled at Stripe first
/// (and nothing deleted if Stripe cannot be told, lest it go on charging), then everything gone,
/// signed out, and a mail to say so.
fn delete_own_account(app: &App, store: &Store, a: &Asked) -> Result<Reply, String> {
    let Some(acc) = &a.account else { return Ok(redirect("/signin")) };
    if acc.email == LOCAL || app.hosted.is_none() {
        return Ok(redirect("/account"));
    }
    if a.f("confirm").trim().to_lowercase() != acc.email {
        return Ok(page_reply(app, a, "Account", &account_shell(app, a, "General", "<h1>Not deleted</h1><p>The address you typed is not this account's. Nothing was deleted.</p><p><a class=button href=\"/account#data\">Back to your account</a></p>"), 200));
    }
    let mut cancelled = false;
    if acc.paid && !acc.free && !acc.stripe_subscription.is_empty() {
        let Some(stripe) = &app.plan.stripe else { return Err("payment is not set up, so the subscription cannot be cancelled".into()) };
        if let Err(e) = stripe.cancel_now(&acc.stripe_subscription) {
            eprintln!("deleting account {}: {e}", acc.id);
            return Ok(page_reply(app, a, "Account", &account_shell(app, a, "General", "<h1>Not deleted yet</h1><p>Stripe, our payment provider, could not be reached to cancel your subscription, so nothing was deleted: otherwise it would go on charging. Please try again in a few minutes, or write to privacy@zetlyn.com.</p><p><a class=button href=\"/account#data\">Back to your account</a></p>"), 200));
        }
        cancelled = true;
    }
    store.delete_account(acc.id)?;
    if let Some(h) = &app.hosted {
        h.tell_soon(&acc.email, mail::deleted(cancelled));
    }
    Ok(page_reply(
        app,
        a,
        "Account deleted",
        "<section class=sent><div class=sent-card><p class=overline>Account deleted</p><h1>Goodbye</h1><p class=lead>Your account is deleted, with all its watches, their checks and changes, and its API keys. A mail confirms it.</p><p><a href=\"/\">Zetlyn's front page</a></p></div></section>",
        200,
    )
    .with_header(cookie(app, "")))
}

/// The owner asks to move the account to another address: a link to that address, which moves it
/// when it is followed (`take_new_address`). Until then nothing changes.
fn ask_new_address(app: &App, store: &Store, a: &Asked) -> Result<Reply, String> {
    let (Some(acc), Some(h)) = (&a.account, &app.hosted) else { return Ok(redirect("/signin")) };
    let new = a.f("email").trim().to_lowercase();
    let said = |text: &str| page_reply(app, a, "Account", &account_shell(app, a, "General", &format!("<h1>Your address</h1>{text}<p><a class=button href=\"/account\">Back to your account</a></p>")), 200);
    if !mail::plausible(&new) {
        return Ok(said("<p class=error>That does not look like an email address.</p>"));
    }
    if new == acc.email {
        return Ok(said("<p>That is the address the account has already.</p>"));
    }
    if store.find_accounts(&new, 50)?.iter().any(|x| x.email == new) {
        return Ok(said("<p class=error>Another Zetlyn account has that address. Delete that one first, or use another address.</p>"));
    }
    let token = store.new_token_for(acc.id, "email", &new, 86400)?;
    h.tell_soon(&new, mail::new_address(&acc.email, &format!("{}/account/email/{token}", h.base)));
    Ok(said(&format!(
        "<p>We sent a link to <b>{}</b>. Follow it, and the account uses that address from then on. Until then it stays <b>{}</b>.</p>",
        esc(&new),
        esc(&acc.email)
    )))
}

/// The link to the new address followed: the account moves there, the old address is told, and
/// whoever followed it is signed in (it was mailed to the address, at the owner's asking).
fn take_new_address(app: &App, store: &Store, a: &Asked, token: &str) -> Result<Reply, String> {
    let Some(h) = &app.hosted else { return Ok(redirect("/")) };
    let Some((id, new)) = store.take_token_payload(token, "email")? else { return Ok(redirect("/signin?expired=1")) };
    let Some(acc) = store.account_by_id(id)?.filter(|x| !x.blocked) else { return Ok(redirect("/signin?expired=1")) };
    let old = acc.email.clone();
    if !store.set_email(id, &new)? {
        return Ok(page_reply(app, a, "Account", "<section class=narrow><h1>Not changed</h1><p>Another Zetlyn account took that address meanwhile, so yours stays as it was.</p><p><a href=\"/account\">Your account</a></p></section>", 200));
    }
    h.tell_soon(&old, mail::address_changed(&new));
    let session = store.new_session(id)?;
    Ok(redirect("/account").with_header(cookie(app, &session)))
}
