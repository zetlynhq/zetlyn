//! Signing in with one world at another, and with GitHub, Google and Apple (FEDERATION.md, M14).
//!
//! Every world is both sides of OpenID Connect.
//!
//! **A provider for its own people.** Its issuer is its address. Discovery at
//! `<issuer>/.well-known/openid-configuration`, the authorization code flow with PKCE (S256) and
//! nothing else, ID tokens signed EdDSA with `oidc.key` beside `operator.key`. A person proves who
//! they are to it as they always have, with a link sent to their address, or, where it takes them,
//! through GitHub, Google or Apple. `sub` is the pseudonym a world already gives a reader; `name`
//! is what they chose to be shown as; `email` only with the `email` scope and their say-so.
//!
//! **A relying party for everybody else.** `identity:` in workspace.yaml says whom it takes:
//! zetlyn worlds (by address, or whichever a person names) and GitHub, Google, Apple. Between two
//! zetlyn worlds nothing is registered first: the relying party's `client_id` is the address of a
//! document it serves about itself, `<endpoints>/oauth/client.json`, whose `redirect_uris` the
//! provider holds the redirect against. What signing in ends in is the reader's own session here.
//!
//! The addresses, under where the endpoints are mounted (a world's root, or `/app` on the machine):
//!
//! ```text
//! /.well-known/openid-configuration   discovery (at the issuer)
//! /oauth/jwks /oauth/authorize /oauth/token /oauth/userinfo     the provider
//! /oauth/signin[/<link>]              a person signing in to the provider
//! /oauth/client.json /oauth/login /oauth/callback               the relying party
//! ```

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;

use maud::html;
use serde_json::{json, Value as J};

use crate::account::{Accounts, IdentityDecl, Pending, Site};

/// The ID token key, beside the operator's.
pub const KEY_FILE: &str = "oidc.key";

/// Where signing in happens, from one place's point of view.
pub struct Here {
    /// Its workspace: accounts, keys, workspace.yaml.
    pub root: PathBuf,
    /// The issuer: a world's address, or the machine's.
    pub issuer: String,
    /// Where its `/oauth/…` endpoints are, absolute.
    pub endpoints: String,
    /// The path those endpoints are under, as a browser asks for them (`` at a domain's root).
    pub mount: String,
    /// The path a reader's session holds for, here.
    pub reader_path: String,
}

impl Here {
    /// A world: its issuer is its address, its endpoints are at it.
    pub fn world(root: &std::path::Path, mount: &str) -> Here {
        let issuer = Site::for_workspace(root).url.trim_end_matches('/').to_string();
        Here {
            root: root.to_path_buf(),
            endpoints: issuer.clone(),
            issuer,
            mount: mount.trim_end_matches('/').to_string(),
            reader_path: if mount.trim_end_matches('/').is_empty() { "/".into() } else { mount.trim_end_matches('/').to_string() },
        }
    }

    /// The machine a hosting directory is: issuer its address, endpoints under `/app`.
    pub fn machine(dir: &std::path::Path) -> Here {
        let issuer = Site::load(dir).url.trim_end_matches('/').to_string();
        Here { root: dir.to_path_buf(), endpoints: format!("{issuer}/app"), issuer, mount: "/app".into(), reader_path: "/".into() }
    }

    fn at(&self, path: &str) -> String {
        format!("{}{path}", self.mount)
    }

    fn secure(&self) -> &'static str {
        if self.issuer.starts_with("https://") { "; Secure" } else { "" }
    }

    fn oauth_path(&self) -> String {
        self.at("/oauth")
    }

    pub fn discovery(&self) -> J {
        let e = &self.endpoints;
        json!({
            "issuer": self.issuer,
            "authorization_endpoint": format!("{e}/oauth/authorize"),
            "token_endpoint": format!("{e}/oauth/token"),
            "userinfo_endpoint": format!("{e}/oauth/userinfo"),
            "jwks_uri": format!("{e}/oauth/jwks"),
            "response_types_supported": ["code"],
            "response_modes_supported": ["query"],
            "grant_types_supported": ["authorization_code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["EdDSA"],
            "code_challenge_methods_supported": ["S256"],
            "scopes_supported": ["openid", "profile", "email"],
            "claims_supported": ["sub", "name", "email", "email_verified"],
            "token_endpoint_auth_methods_supported": ["none"],
            "client_id_metadata_document_supported": true,
        })
    }

    fn key(&self) -> Result<String, String> {
        match crate::key::public(&self.root, KEY_FILE) {
            Some(k) => Ok(k),
            None => crate::key::new(&self.root, KEY_FILE),
        }
    }

    /// The document this place serves about itself as a relying party.
    pub fn client_document(&self) -> J {
        let site = Site::load(&self.root);
        json!({
            "client_id": format!("{}/oauth/client.json", self.endpoints),
            "client_name": if site.title.is_empty() { self.issuer.clone() } else { site.title },
            "client_uri": self.issuer,
            "redirect_uris": [format!("{}/oauth/callback", self.endpoints)],
            "grant_types": ["authorization_code"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
            "application_type": "web",
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Answering.

struct Asked {
    method: tiny_http::Method,
    query: BTreeMap<String, String>,
    form: BTreeMap<String, String>,
    cookie: Option<String>,
    authorization: Option<String>,
}

fn pairs(text: &str) -> BTreeMap<String, String> {
    text.split('&')
        .filter_map(|p| p.split_once('=').or(Some((p, ""))))
        .filter(|(k, _)| !k.is_empty())
        .map(|(k, v)| (crate::serve::urldecode(&k.replace('+', " ")), crate::serve::urldecode(&v.replace('+', " "))))
        .collect()
}

fn reply(request: tiny_http::Request, status: u16, kind: &str, body: String, headers: &[(&str, String)]) {
    let mut response = tiny_http::Response::from_string(body).with_status_code(status);
    let mut all = vec![("Content-Type", kind.to_string()), ("Cache-Control", "no-store".to_string())];
    all.extend(headers.iter().map(|(k, v)| (*k, v.clone())));
    for (k, v) in all {
        if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            response = response.with_header(h);
        }
    }
    let _ = request.respond(response);
}

fn go(request: tiny_http::Request, to: &str, cookie: Option<String>) {
    let mut headers = vec![("Location", to.to_string())];
    if let Some(c) = cookie {
        headers.push(("Set-Cookie", c));
    }
    reply(request, 303, "text/plain; charset=utf-8", String::new(), &headers);
}

fn page(request: tiny_http::Request, status: u16, title: &str, body: maud::Markup) {
    reply(request, status, "text/html; charset=utf-8", crate::serve::shell(title, body), &[]);
}

fn json_reply(request: tiny_http::Request, status: u16, body: J) {
    reply(request, status, "application/json", body.to_string(), &[]);
}

/// A path on this site to come back to, and nothing else.
fn local(next: &str) -> Option<String> {
    (next.starts_with('/') && !next.starts_with("//") && !next.contains('\\')).then(|| next.to_string())
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(concat!("zetlyn/", env!("CARGO_PKG_VERSION")))
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .into()
}

/// The machine this process serves, where it serves one: `zetlyn hosting serve` answers for the
/// machine and for every organisation on it, one request at a time, so an organisation asking the
/// machine (or the machine asking an organisation) over HTTP would be waiting for itself. Asked of
/// an address this process serves, the answer is made here instead.
/// A list rather than one, because a process that serves two (a test does) must not forget the first.
static SERVED: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

pub fn serving_machine(dir: &std::path::Path) {
    if let Ok(mut s) = SERVED.lock() {
        if !s.iter().any(|d| d == dir) {
            s.push(dir.to_path_buf());
        }
    }
}

/// The workspace an address of this process is the root of, where it is one: a world's document
/// asked for from the same process is made here, not fetched from itself.
pub fn served_world(url: &str) -> Option<PathBuf> {
    let (here, rest) = served(url)?;
    (rest.is_empty() || rest == "/").then_some(here.root)
}

/// The place an address of this process is, and the rest of the address under its mount.
fn served(url: &str) -> Option<(Here, String)> {
    let dirs = SERVED.lock().ok()?.clone();
    dirs.iter().find_map(|dir| served_by(dir, url))
}

fn served_by(dir: &std::path::Path, url: &str) -> Option<(Here, String)> {
    let dir = dir.to_path_buf();
    // An organisation on a domain of its own is this process too, at the root of that domain.
    for org in std::fs::read_dir(dir.join("orgs")).into_iter().flatten().flatten() {
        let root = org.path();
        let domain = Site::load(&root).domain.trim().to_lowercase();
        if domain.is_empty() {
            continue;
        }
        if let Some(rest) = url.strip_prefix(&format!("https://{domain}")) {
            if rest.is_empty() || rest.starts_with('/') || rest.starts_with('?') {
                return Some((Here::world(&root, ""), rest.split('?').next().unwrap_or("").to_string()));
            }
        }
    }
    let issuer = Site::load(&dir).url.trim_end_matches('/').to_string();
    if issuer.is_empty() {
        return None;
    }
    let rest = url.strip_prefix(&issuer)?;
    if !(rest.is_empty() || rest.starts_with('/')) {
        return None;
    }
    let path = rest.split('?').next().unwrap_or("").to_string();
    if path.starts_with("/.well-known/openid-configuration") {
        return Some((Here::machine(&dir), path));
    }
    if let Some(under) = path.strip_prefix("/app") {
        return Some((Here::machine(&dir), under.to_string()));
    }
    let org = path.trim_start_matches('/').split('/').next().unwrap_or("").to_string();
    let root = dir.join("orgs").join(&org);
    if org.is_empty() || !root.is_dir() {
        return None;
    }
    Some((Here::world(&root, &format!("/{org}")), path[org.len() + 1..].to_string()))
}

/// What this process would answer `url` with, where it serves it.
fn answered_here(url: &str, form: Option<&BTreeMap<String, String>>) -> Option<Result<J, String>> {
    let (here, path) = served(url)?;
    Some(match (path.as_str(), form) {
        ("/.well-known/openid-configuration", None) => Ok(here.discovery()),
        ("/oauth/jwks", None) => here.key().and_then(|k| crate::jwt::ed25519_jwk(&k)).map(|jwk| json!({ "keys": [jwk] })),
        ("/oauth/client.json", None) => Ok(here.client_document()),
        ("/oauth/token", Some(form)) => {
            let accounts = match Accounts::open(&here.root) {
                Ok(a) => a,
                Err(e) => return Some(Err(e)),
            };
            match token_answer(&here, &accounts, form) {
                (200, body) => Ok(body),
                (status, body) => Err(format!("{url}: {status} {}", body["error_description"].as_str().unwrap_or(""))),
            }
        }
        _ => return None,
    })
}

fn get_json(url: &str) -> Result<J, String> {
    if let Some(answer) = answered_here(url, None) {
        return answer;
    }
    let mut r = agent().get(url).header("Accept", "application/json").call().map_err(|e| format!("{url}: {e}"))?;
    if !r.status().is_success() {
        return Err(format!("{url}: {}", r.status()));
    }
    r.body_mut().read_json::<J>().map_err(|e| format!("{url}: {e}"))
}

/// What a sign-in endpoint says, to a POST of a form.
fn post_form(url: &str, form: &[(&str, &str)], bearer: Option<&str>) -> Result<J, String> {
    let as_map: BTreeMap<String, String> = form.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    if let Some(answer) = answered_here(url, Some(&as_map)) {
        return answer;
    }
    let mut req = agent().post(url).header("Accept", "application/json");
    if let Some(t) = bearer {
        req = req.header("Authorization", &format!("Bearer {t}"));
    }
    let mut r = req.send_form(form.iter().copied()).map_err(|e| format!("{url}: {e}"))?;
    let status = r.status();
    let body: J = r.body_mut().read_json().unwrap_or(J::Null);
    if !status.is_success() {
        return Err(format!("{url}: {status} {}", body["error_description"].as_str().or(body["error"].as_str()).unwrap_or("")));
    }
    Ok(body)
}

/// Everything under `/oauth/` and discovery, for this place. The request back where it is not one
/// of these.
pub fn answer(here: &Here, mut request: tiny_http::Request, rel: &[String], url: &str) -> Option<tiny_http::Request> {
    let parts: Vec<&str> = rel.iter().map(String::as_str).collect();
    let ours = matches!(parts.as_slice(), [".well-known", "openid-configuration"] | ["oauth", ..]);
    if !ours {
        return Some(request);
    }
    let header = |name: &'static str| request.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str().to_string());
    let (cookie, authorization) = (header("Cookie"), header("Authorization"));
    let method = request.method().clone();
    let mut body = String::new();
    if method == tiny_http::Method::Post {
        let _ = std::io::Read::read_to_string(&mut std::io::Read::take(request.as_reader(), 64 * 1024), &mut body);
    }
    let asked = Asked { method, query: pairs(url.split_once('?').map(|(_, q)| q).unwrap_or("")), form: pairs(&body), cookie, authorization };
    let accounts = match Accounts::open(&here.root) {
        Ok(a) => a,
        Err(e) => {
            json_reply(request, 500, json!({ "error": "server_error", "error_description": e }));
            return None;
        }
    };
    let post = asked.method == tiny_http::Method::Post;
    match parts.as_slice() {
        [".well-known", "openid-configuration"] => json_reply(request, 200, here.discovery()),
        ["oauth", "jwks"] => match here.key().and_then(|k| crate::jwt::ed25519_jwk(&k)) {
            Ok(jwk) => json_reply(request, 200, json!({ "keys": [jwk] })),
            Err(e) => json_reply(request, 500, json!({ "error": e })),
        },
        ["oauth", "client.json"] => json_reply(request, 200, here.client_document()),
        ["oauth", "authorize"] => authorize(here, &accounts, request, &asked, url),
        ["oauth", "signin"] if post => signin_mail(here, &accounts, request, &asked),
        ["oauth", "signin", raw] => match accounts.spend_link(raw) {
            Some(session) => {
                let next = asked.query.get("next").and_then(|n| local(n)).unwrap_or_else(|| here.at("/"));
                go(request, &next, Some(format!("zo={session}; Path={}; Max-Age=3600; HttpOnly; SameSite=Lax{}", here.oauth_path(), here.secure())));
            }
            None => page(request, 410, "Sign in", html! { h1 { "That link was used, or it is older than a quarter of an hour" } }),
        },
        ["oauth", "token"] if post => token(here, &accounts, request, &asked),
        ["oauth", "userinfo"] => userinfo(here, &accounts, request, &asked),
        ["oauth", "login"] => login(here, &accounts, request, &asked),
        ["oauth", "callback"] => callback(here, &accounts, request, &asked),
        _ => json_reply(request, 404, json!({ "error": "not here" })),
    }
    None
}

// ---------------------------------------------------------------------------------------------
// The provider.

/// The client a relying party says it is, from its own document, and the redirect it asked for,
/// held against that document.
fn client_of(client_id: &str, redirect: &str) -> Result<J, String> {
    if !(client_id.starts_with("https://") || client_id.starts_with("http://127.0.0.1") || client_id.starts_with("http://localhost")) {
        return Err("a client here is the address of a document about itself".into());
    }
    let doc = get_json(client_id)?;
    if doc["client_id"].as_str() != Some(client_id) {
        return Err(format!("{client_id} says it is somebody else"));
    }
    let allowed = doc["redirect_uris"].as_array().is_some_and(|all| all.iter().any(|r| r.as_str() == Some(redirect)));
    if !allowed {
        return Err(format!("{redirect} is not where {client_id} says to send people back to"));
    }
    Ok(doc)
}

fn session_of(accounts: &Accounts, asked: &Asked) -> Option<crate::account::Account> {
    let raw = asked.cookie.as_deref()?.split(';').filter_map(|p| p.trim().split_once('=')).find(|(k, _)| *k == "zo").map(|(_, v)| v.to_string())?;
    accounts.by_session(&raw)
}

fn authorize(here: &Here, accounts: &Accounts, request: tiny_http::Request, asked: &Asked, url: &str) {
    let p = if asked.method == tiny_http::Method::Post { &asked.form } else { &asked.query };
    let get = |k: &str| p.get(k).cloned().unwrap_or_default();
    let (client_id, redirect, state, nonce, challenge) = (get("client_id"), get("redirect_uri"), get("state"), get("nonce"), get("code_challenge"));
    let scope: Vec<String> = get("scope").split_whitespace().map(str::to_string).collect();
    let refuse = |request: tiny_http::Request, why: String| page(request, 400, "Not signed in", html! { h1 { "This sign-in cannot go on" } p { (why) } });
    if get("response_type") != "code" {
        return refuse(request, "only `response_type=code` is answered here".into());
    }
    if challenge.is_empty() || get("code_challenge_method") != "S256" {
        return refuse(request, "a client here proves itself with PKCE, S256".into());
    }
    if !scope.iter().any(|s| s == "openid") {
        return refuse(request, "the scope has to include `openid`".into());
    }
    let client = match client_of(&client_id, &redirect) {
        Ok(c) => c,
        Err(e) => return refuse(request, e),
    };
    let client_name = client["client_name"].as_str().unwrap_or(&client_id).to_string();
    let site = Site::load(&here.root);
    let here_name = if site.title.is_empty() { here.issuer.clone() } else { site.title.clone() };
    let Some(who) = session_of(accounts, asked) else {
        // Signed in nowhere here yet: an address, or one of the providers this place takes.
        let next = format!("{}{}", here.at("/oauth/authorize"), url.split_once('?').map(|(_, q)| format!("?{q}")).unwrap_or_default());
        let upstream = providers_for_signin(here);
        return page(request, 200, "Sign in", html! {
            h1 { "Sign in to " (here_name) }
            p.about { (client_name) " asks who you are. Sign in here first; you are asked before anything is told." }
            form.bar method="post" action=(here.at("/oauth/signin")) {
                input type="hidden" name="next" value=(next);
                input type="email" name="email" placeholder="you@example.org" required;
                button type="submit" { "Send me a link" }
            }
            @if !upstream.is_empty() {
                p { @for (id, label) in &upstream {
                    a.chip href={(here.at("/oauth/login")) "?with=" (crate::serve::urlencode(id)) "&purpose=provider&next=" (crate::serve::urlencode(&next))} { (label) } " "
                } }
            }
        });
    };
    if asked.method != tiny_http::Method::Post {
        let wants_email = scope.iter().any(|s| s == "email");
        let name = accounts.name_of(who.id);
        let back_to = redirect.split("://").nth(1).unwrap_or(&redirect).split('/').next().unwrap_or("").to_string();
        return page(request, 200, "Tell them who you are?", html! {
            h1 { (client_name) " asks who you are" }
            p.about { "It is at " code { (back_to) } ". It learns a name for you here, " code { (crate::propose::pseudonym(&here.root, who.id).unwrap_or_default()) } ", that says nothing else about you." }
            form.settings method="post" action=(here.at("/oauth/authorize")) {
                @for k in ["client_id", "redirect_uri", "state", "nonce", "code_challenge", "code_challenge_method", "scope", "response_type"] {
                    input type="hidden" name=(k) value=(get(k));
                }
                p { label { "Shown as" br; input.wide type="text" name="name" value=(name) placeholder="your name, or leave it empty"; } }
                @if wants_email && !who.email.ends_with(".invalid") {
                    p { label { input type="checkbox" name="share_email" value="1"; " Tell it my address too, " (who.email) } }
                }
                p { button.primary type="submit" name="allow" value="1" { "Tell " (client_name) } " " a href=(redirect_with_error(&redirect, &state)) { "Do not" } }
            }
        });
    }
    if p.get("allow").map(String::as_str) != Some("1") {
        return go(request, &redirect_with_error(&redirect, &state), None);
    }
    // The name changes only where the form said one; a client that sends no field keeps the name.
    if let Some(n) = p.get("name") {
        let name: String = n.trim().chars().filter(|c| !c.is_control()).take(80).collect();
        let _ = accounts.set_name(who.id, &name);
    }
    let share_email = p.get("share_email").map(String::as_str) == Some("1");
    let granted: Vec<&str> = scope.iter().map(String::as_str).filter(|s| matches!(*s, "openid" | "profile") || (*s == "email" && share_email)).collect();
    let code = crate::jwt::random();
    if let Err(e) = accounts.put_code(&code, who.id, &client_id, &redirect, &nonce, &challenge, &granted.join(" ")) {
        return refuse(request, e);
    }
    let sep = if redirect.contains('?') { '&' } else { '?' };
    go(request, &format!("{redirect}{sep}code={}&state={}&iss={}", crate::serve::urlencode(&code), crate::serve::urlencode(&state), crate::serve::urlencode(&here.issuer)), None);
}

fn redirect_with_error(redirect: &str, state: &str) -> String {
    let sep = if redirect.contains('?') { '&' } else { '?' };
    format!("{redirect}{sep}error=access_denied&state={}", crate::serve::urlencode(state))
}

fn signin_mail(here: &Here, accounts: &Accounts, request: tiny_http::Request, asked: &Asked) {
    let email = asked.form.get("email").cloned().unwrap_or_default();
    let next = asked.form.get("next").and_then(|n| local(n)).unwrap_or_else(|| here.at("/"));
    match accounts.ensure(&email).and_then(|a| accounts.new_link(a.id).map(|raw| (a, raw))) {
        Ok((a, raw)) => {
            let site = Site::for_workspace(&here.root);
            let link = format!("{}/oauth/signin/{raw}?next={}", here.endpoints, crate::serve::urlencode(&next));
            if let Err(e) = site.send(&a.email, "Your Zetlyn sign-in link", &format!("{link}\n\nGood for a quarter of an hour, and once.")) {
                eprintln!("sign-in mail: {e}");
            }
            page(request, 200, "Check your mail", html! { h1 { "Check your mail" } p { "A link is on its way. It is good for a quarter of an hour, and once." } });
        }
        Err(e) => page(request, 400, "Sign in", html! { h1 { "Sign in" } p { (e) } }),
    }
}

fn token(here: &Here, accounts: &Accounts, request: tiny_http::Request, asked: &Asked) {
    let (status, body) = token_answer(here, accounts, &asked.form);
    json_reply(request, status, body);
}

/// What the token endpoint answers a form with: over HTTP, or in this process where the relying
/// party is this process too.
fn token_answer(here: &Here, accounts: &Accounts, form: &BTreeMap<String, String>) -> (u16, J) {
    let f = |k: &str| form.get(k).cloned().unwrap_or_default();
    let bad = |e: &str, d: &str| (400, json!({ "error": e, "error_description": d }));
    if f("grant_type") != "authorization_code" {
        return bad("unsupported_grant_type", "only authorization_code");
    }
    let Some(code) = accounts.take_code(&f("code")) else {
        return bad("invalid_grant", "that code was used, is older than a minute, or was never given");
    };
    if code.client != f("client_id") || code.redirect != f("redirect_uri") {
        return bad("invalid_grant", "that code was given to somebody else");
    }
    if crate::jwt::sha256_b64(f("code_verifier").as_bytes()) != code.challenge {
        return bad("invalid_grant", "the verifier is not the one the challenge was made from");
    }
    let Some(account) = accounts.by_id(code.account) else { return bad("invalid_grant", "nobody") };
    let now = crate::now();
    let scope: Vec<&str> = code.scope.split_whitespace().collect();
    let mut claims = json!({
        "iss": here.issuer,
        "sub": crate::propose::pseudonym(&here.root, account.id).unwrap_or_default(),
        "aud": code.client,
        "iat": now,
        "exp": now + 300,
    });
    if !code.nonce.is_empty() {
        claims["nonce"] = json!(code.nonce);
    }
    let name = accounts.name_of(account.id);
    if scope.contains(&"profile") && !name.is_empty() {
        claims["name"] = json!(name);
    }
    if scope.contains(&"email") && !account.email.ends_with(".invalid") {
        claims["email"] = json!(account.email);
        claims["email_verified"] = json!(true);
    }
    let _ = here.key();
    let id_token = match crate::jwt::sign_eddsa(&here.root, KEY_FILE, &claims) {
        Ok(t) => t,
        Err(e) => return (500, json!({ "error": "server_error", "error_description": e })),
    };
    let access = crate::jwt::random();
    let _ = accounts.put_token(&access, account.id, &code.client, &code.scope);
    (200, json!({ "access_token": access, "token_type": "Bearer", "expires_in": 600, "id_token": id_token, "scope": code.scope }))
}

fn userinfo(here: &Here, accounts: &Accounts, request: tiny_http::Request, asked: &Asked) {
    let raw = asked.authorization.as_deref().and_then(|a| a.strip_prefix("Bearer ")).unwrap_or("");
    let Some((id, scope)) = accounts.by_token(raw.trim()) else {
        return reply(request, 401, "application/json", json!({ "error": "invalid_token" }).to_string(), &[("WWW-Authenticate", "Bearer".into())]);
    };
    let Some(account) = accounts.by_id(id) else { return json_reply(request, 401, json!({ "error": "invalid_token" })) };
    let mut out = json!({ "sub": crate::propose::pseudonym(&here.root, id).unwrap_or_default() });
    let name = accounts.name_of(id);
    if scope.split_whitespace().any(|s| s == "profile") && !name.is_empty() {
        out["name"] = json!(name);
    }
    if scope.split_whitespace().any(|s| s == "email") && !account.email.ends_with(".invalid") {
        out["email"] = json!(account.email);
        out["email_verified"] = json!(true);
    }
    json_reply(request, 200, out);
}

// ---------------------------------------------------------------------------------------------
// The relying party.

/// The ways a person may sign in here besides a link, as `(id, label)`. `zetlyn:<address>` is a
/// world, `world` is whichever world they name, and the others are themselves.
pub fn options(site: &Site) -> Vec<(String, String)> {
    site.identities()
        .into_iter()
        .map(|d| match d {
            IdentityDecl::Zetlyn(at) if at.trim() == "any" => ("world".to_string(), "Sign in with your own world".to_string()),
            IdentityDecl::Zetlyn(at) => {
                let at = at.trim().trim_end_matches('/').to_string();
                let host = at.split("://").nth(1).unwrap_or(&at).to_string();
                (format!("zetlyn:{at}"), format!("Sign in with {host}"))
            }
            IdentityDecl::Github(_) => ("github".into(), "Sign in with GitHub".into()),
            IdentityDecl::Google(_) => ("google".into(), "Sign in with Google".into()),
            IdentityDecl::Apple(_) => ("apple".into(), "Sign in with Apple".into()),
        })
        .collect()
}

/// What a provider signing a person in to this place may send them through: its options, but not
/// itself.
fn providers_for_signin(here: &Here) -> Vec<(String, String)> {
    options(&Site::load(&here.root)).into_iter().filter(|(id, _)| id != &format!("zetlyn:{}", here.issuer) && id != "world").collect()
}

fn decl(site: &Site, id: &str) -> Option<IdentityDecl> {
    site.identities().into_iter().find(|d| match (d, id) {
        (IdentityDecl::Github(_), "github") | (IdentityDecl::Google(_), "google") | (IdentityDecl::Apple(_), "apple") => true,
        (IdentityDecl::Zetlyn(at), id) if at.trim() == "any" => id.starts_with("zetlyn:") || id == "world",
        (IdentityDecl::Zetlyn(at), id) => id == format!("zetlyn:{}", at.trim().trim_end_matches('/')),
        _ => false,
    })
}

/// A value from workspace.yaml, `${VAR}` read from the environment.
fn secret(v: &str) -> Result<String, String> {
    crate::fetch::resolve(v).map(|r| r.unwrap_or_default())
}

/// Where a provider's endpoints are, from its discovery document, held against the issuer.
fn discover(issuer: &str) -> Result<J, String> {
    let doc = get_json(&format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/')))?;
    if doc["issuer"].as_str().map(|i| i.trim_end_matches('/')) != Some(issuer.trim_end_matches('/')) {
        return Err(format!("{issuer} says its issuer is {}", doc["issuer"].as_str().unwrap_or("nothing")));
    }
    Ok(doc)
}

fn issuer_for(d: &IdentityDecl, id: &str) -> String {
    match d {
        IdentityDecl::Zetlyn(_) => id.trim_start_matches("zetlyn:").trim_end_matches('/').to_string(),
        IdentityDecl::Google(g) => if g.issuer.is_empty() { "https://accounts.google.com".into() } else { g.issuer.trim_end_matches('/').to_string() },
        IdentityDecl::Apple(a) => if a.issuer.is_empty() { "https://appleid.apple.com".into() } else { a.issuer.trim_end_matches('/').to_string() },
        IdentityDecl::Github(g) => if g.web.is_empty() { "https://github.com".into() } else { g.web.trim_end_matches('/').to_string() },
    }
}

fn login(here: &Here, accounts: &Accounts, request: tiny_http::Request, asked: &Asked) {
    let site = Site::load(&here.root);
    let mut id = asked.query.get("with").cloned().unwrap_or_default();
    let purpose = if asked.query.get("purpose").map(String::as_str) == Some("provider") { "provider" } else { "reader" };
    let next = asked.query.get("next").and_then(|n| local(n)).unwrap_or_else(|| here.at("/"));
    let fail = |request: tiny_http::Request, why: String| page(request, 400, "Not signed in", html! { h1 { "Signing in did not work" } p { (why) } });
    if id == "world" {
        let Some(at) = asked.query.get("world").map(|w| w.trim().trim_end_matches('/').to_string()).filter(|w| !w.is_empty()) else {
            return page(request, 200, "Your world", html! {
                h1 { "Sign in with your own world" }
                form.bar method="get" action=(here.at("/oauth/login")) {
                    input type="hidden" name="with" value="world";
                    input type="hidden" name="next" value=(next);
                    input type="url" name="world" placeholder="https://your-world.example" required;
                    button type="submit" { "Go there" }
                }
            });
        };
        let at = if at.starts_with("http://") || at.starts_with("https://") { at } else { format!("https://{at}") };
        id = format!("zetlyn:{at}");
    }
    let Some(d) = decl(&site, &id) else { return fail(request, format!("{id} is not a way to sign in here")) };
    let issuer = issuer_for(&d, &id);
    let (state, verifier, nonce) = (crate::jwt::random(), crate::jwt::random(), crate::jwt::random());
    let challenge = crate::jwt::sha256_b64(verifier.as_bytes());
    let redirect = format!("{}/oauth/callback", here.endpoints);
    let to = match &d {
        IdentityDecl::Github(g) => {
            let client = match secret(&g.client) { Ok(c) => c, Err(e) => return fail(request, e) };
            format!(
                "{issuer}/login/oauth/authorize?client_id={}&redirect_uri={}&scope={}&state={}&code_challenge={challenge}&code_challenge_method=S256",
                crate::serve::urlencode(&client), crate::serve::urlencode(&redirect), crate::serve::urlencode("read:user user:email"), crate::serve::urlencode(&state)
            )
        }
        _ => {
            let doc = match discover(&issuer) { Ok(d) => d, Err(e) => return fail(request, e) };
            let Some(authorize) = doc["authorization_endpoint"].as_str() else { return fail(request, format!("{issuer} names no authorization endpoint")) };
            let (client, scope, extra) = match &d {
                IdentityDecl::Zetlyn(_) => (format!("{}/oauth/client.json", here.endpoints), "openid profile", format!("&code_challenge={challenge}&code_challenge_method=S256")),
                IdentityDecl::Google(g) => (
                    match secret(&g.client) { Ok(c) => c, Err(e) => return fail(request, e) },
                    "openid email profile",
                    format!("&code_challenge={challenge}&code_challenge_method=S256{}", if g.domain.is_empty() { String::new() } else { format!("&hd={}", crate::serve::urlencode(&g.domain)) }),
                ),
                IdentityDecl::Apple(a) => (match secret(&a.client) { Ok(c) => c, Err(e) => return fail(request, e) }, "name email", "&response_mode=form_post".to_string()),
                IdentityDecl::Github(_) => unreachable!(),
            };
            format!(
                "{authorize}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&nonce={}{extra}",
                crate::serve::urlencode(&client), crate::serve::urlencode(&redirect), crate::serve::urlencode(scope), crate::serve::urlencode(&state), crate::serve::urlencode(&nonce)
            )
        }
    };
    let pending = Pending { provider: id, verifier, nonce, next, purpose: purpose.into() };
    if let Err(e) = accounts.put_pending(&state, &pending) {
        return fail(request, e);
    }
    go(request, &to, None);
}

/// Who signed in, as the provider says.
struct Signed {
    issuer: String,
    subject: String,
    name: String,
    email: Option<String>,
}

fn callback(here: &Here, accounts: &Accounts, request: tiny_http::Request, asked: &Asked) {
    // Apple answers with a form; everybody else in the address.
    let p = if asked.method == tiny_http::Method::Post { &asked.form } else { &asked.query };
    let fail = |request: tiny_http::Request, why: String| page(request, 400, "Not signed in", html! { h1 { "Signing in did not work" } p { (why) } });
    if let Some(e) = p.get("error") {
        return fail(request, format!("The provider said: {e}"));
    }
    let Some(pending) = p.get("state").and_then(|s| accounts.take_pending(s)) else {
        return fail(request, "This sign-in was not started here, or was started more than a quarter of an hour ago.".into());
    };
    let code = p.get("code").cloned().unwrap_or_default();
    let site = Site::load(&here.root);
    let Some(d) = decl(&site, &pending.provider) else { return fail(request, format!("{} is not a way to sign in here any more", pending.provider)) };
    let signed = match exchange(here, &d, &pending, &code, p) {
        Ok(s) => s,
        Err(e) => return fail(request, e),
    };
    let account = match accounts.for_identity(&signed.issuer, &signed.subject, signed.email.as_deref()) {
        Ok(a) => a,
        Err(e) => return fail(request, e),
    };
    if accounts.name_of(account.id).is_empty() && !signed.name.is_empty() {
        let _ = accounts.set_name(account.id, &signed.name);
    }
    let session = match accounts.new_session(account.id) {
        Ok(s) => s,
        Err(e) => return fail(request, e),
    };
    let cookie = if pending.purpose == "provider" {
        format!("zo={session}; Path={}; Max-Age=3600; HttpOnly; SameSite=Lax{}", here.oauth_path(), here.secure())
    } else {
        format!("{}={session}; Path={}; Max-Age=2592000; HttpOnly; SameSite=Lax{}", crate::account::READER_COOKIE, here.reader_path, here.secure())
    };
    go(request, &pending.next, Some(cookie));
}

fn exchange(here: &Here, d: &IdentityDecl, pending: &Pending, code: &str, p: &BTreeMap<String, String>) -> Result<Signed, String> {
    let redirect = format!("{}/oauth/callback", here.endpoints);
    let issuer = issuer_for(d, &pending.provider);
    match d {
        IdentityDecl::Github(g) => {
            let api = if g.api.is_empty() { "https://api.github.com".to_string() } else { g.api.trim_end_matches('/').to_string() };
            let (client, secret_v) = (secret(&g.client)?, secret(&g.secret)?);
            let t = post_form(&format!("{issuer}/login/oauth/access_token"), &[("client_id", &client), ("client_secret", &secret_v), ("code", code), ("redirect_uri", &redirect), ("code_verifier", &pending.verifier)], None)?;
            let access = t["access_token"].as_str().ok_or_else(|| format!("GitHub said: {}", t["error_description"].as_str().unwrap_or("no token")))?;
            let user = github_get(&format!("{api}/user"), access)?;
            let id = user["id"].as_i64().ok_or("GitHub named nobody")?;
            let emails = github_get(&format!("{api}/user/emails"), access).unwrap_or(J::Null);
            let email = emails.as_array().and_then(|all| all.iter().find(|e| e["primary"].as_bool() == Some(true) && e["verified"].as_bool() == Some(true))).and_then(|e| e["email"].as_str()).map(str::to_string);
            let name = user["name"].as_str().filter(|n| !n.is_empty()).or(user["login"].as_str()).unwrap_or("").to_string();
            Ok(Signed { issuer, subject: id.to_string(), name, email })
        }
        _ => {
            let doc = discover(&issuer)?;
            let token_at = doc["token_endpoint"].as_str().ok_or("no token endpoint")?.to_string();
            let jwks_at = doc["jwks_uri"].as_str().ok_or("no keys")?.to_string();
            let (client, t) = match d {
                IdentityDecl::Zetlyn(_) => {
                    let client = format!("{}/oauth/client.json", here.endpoints);
                    let t = post_form(&token_at, &[("grant_type", "authorization_code"), ("code", code), ("redirect_uri", &redirect), ("client_id", &client), ("code_verifier", &pending.verifier)], None)?;
                    (client, t)
                }
                IdentityDecl::Google(g) => {
                    let (client, secret_v) = (secret(&g.client)?, secret(&g.secret)?);
                    let t = post_form(&token_at, &[("grant_type", "authorization_code"), ("code", code), ("redirect_uri", &redirect), ("client_id", &client), ("client_secret", &secret_v), ("code_verifier", &pending.verifier)], None)?;
                    (client, t)
                }
                IdentityDecl::Apple(a) => {
                    let client = secret(&a.client)?;
                    let now = crate::now();
                    // Made when it is needed, for ten minutes: Apple takes one for at most six months,
                    // and a secret that is not kept cannot be lost.
                    let client_secret = crate::jwt::sign_es256(&secret(&a.key)?, &secret(&a.key_id)?, &json!({ "iss": secret(&a.team)?, "iat": now, "exp": now + 600, "aud": issuer, "sub": client }))?;
                    let t = post_form(&token_at, &[("grant_type", "authorization_code"), ("code", code), ("redirect_uri", &redirect), ("client_id", &client), ("client_secret", &client_secret)], None)?;
                    (client, t)
                }
                IdentityDecl::Github(_) => unreachable!(),
            };
            let id_token = t["id_token"].as_str().ok_or("the provider gave no ID token")?;
            let keys = get_json(&jwks_at)?["keys"].as_array().cloned().unwrap_or_default();
            let claims = crate::jwt::verify(id_token, &keys)?;
            crate::jwt::check(&claims, &issuer, &client, Some(&pending.nonce), crate::now())?;
            if let IdentityDecl::Google(g) = d {
                if !g.domain.is_empty() && claims["hd"].as_str() != Some(g.domain.as_str()) {
                    return Err(format!("only people of {} sign in here with Google", g.domain));
                }
            }
            let verified = matches!(&claims["email_verified"], J::Bool(true)) || claims["email_verified"].as_str() == Some("true");
            let email = claims["email"].as_str().filter(|_| verified).map(str::to_string);
            let mut name = claims["name"].as_str().unwrap_or("").to_string();
            if let (IdentityDecl::Apple(_), Some(user)) = (d, p.get("user")) {
                // Apple says the name once, on the first sign-in, beside the code and nowhere else.
                if let Ok(u) = serde_json::from_str::<J>(user) {
                    let full = format!("{} {}", u["name"]["firstName"].as_str().unwrap_or(""), u["name"]["lastName"].as_str().unwrap_or(""));
                    if !full.trim().is_empty() {
                        name = full.trim().to_string();
                    }
                }
            }
            let subject = claims["sub"].as_str().ok_or("the token names nobody")?.to_string();
            Ok(Signed { issuer, subject, name, email })
        }
    }
}

fn github_get(url: &str, token: &str) -> Result<J, String> {
    let mut r = agent().get(url).header("Accept", "application/vnd.github+json").header("Authorization", &format!("Bearer {token}")).call().map_err(|e| format!("{url}: {e}"))?;
    if !r.status().is_success() {
        return Err(format!("{url}: {}", r.status()));
    }
    let mut body = String::new();
    r.body_mut().as_reader().read_to_string(&mut body).map_err(|e| e.to_string())?;
    serde_json::from_str(&body).map_err(|e| format!("{url}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use base64::Engine;

    const RSA: &[u8] = include_bytes!("../tests/fixtures/oidc/rsa-test.der");
    const RSA_JWK: &str = include_str!("../tests/fixtures/oidc/rsa-test.jwk.json");
    const EC_P8: &str = include_str!("../tests/fixtures/oidc/ec-test.p8");

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    fn up(port: u16) {
        for _ in 0..50 {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("nothing answered on {port}");
    }

    fn world(dir: &Path, port: u16, extra: &str) -> String {
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir.join("sources")).unwrap();
        std::fs::create_dir_all(dir.join("trackers")).unwrap();
        let url = format!("http://127.0.0.1:{port}");
        std::fs::write(dir.join("workspace.yaml"), format!("title: World {port}\nurl: {url}\nowners: [owner@example.org]\n{extra}")).unwrap();
        let args: Vec<String> = ["world", "serve", dir.to_str().unwrap(), "--addr", &format!("127.0.0.1:{port}")].iter().map(|s| s.to_string()).collect();
        // Ends with the test process; nothing outlives it.
        std::thread::spawn(move || crate::app::world_serve(&args));
        up(port);
        url
    }

    /// A request with no redirect followed: its status, body, Location and Set-Cookie.
    fn ask(method: &str, url: &str, cookie: Option<&str>, form: Option<&str>) -> (u16, String, String, String) {
        let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).max_redirects(0).build().into();
        let mut r = if method == "POST" {
            let mut q = agent.post(url).header("Content-Type", "application/x-www-form-urlencoded");
            if let Some(c) = cookie {
                q = q.header("Cookie", c);
            }
            q.send(form.unwrap_or("")).unwrap()
        } else {
            let mut q = agent.get(url);
            if let Some(c) = cookie {
                q = q.header("Cookie", c);
            }
            q.call().unwrap()
        };
        let h = |k: &str| r.headers().get(k).map(|v| v.to_str().unwrap_or("").to_string()).unwrap_or_default();
        let (location, set) = (h("location"), h("set-cookie"));
        (r.status().as_u16(), r.body_mut().read_to_string().unwrap_or_default(), location, set)
    }

    fn query_of(url: &str) -> BTreeMap<String, String> {
        pairs(url.split_once('?').map(|(_, q)| q).unwrap_or(""))
    }

    /// A small server that answers as `handle` says: (method, path, query, body) → (status, body).
    fn mock(handle: impl Fn(&str, &str, &BTreeMap<String, String>, &str) -> (u16, String) + Send + 'static) -> String {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}", server.server_addr().to_ip().unwrap());
        std::thread::spawn(move || {
            for mut request in server.incoming_requests() {
                let mut body = String::new();
                let _ = request.as_reader().read_to_string(&mut body);
                let full = request.url().to_string();
                let path = full.split('?').next().unwrap_or("/").to_string();
                let (status, out) = handle(request.method().as_str(), &path, &query_of(&full), &body);
                let response = tiny_http::Response::from_string(out).with_status_code(status).with_header(tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
                let _ = request.respond(response);
            }
        });
        url
    }

    fn rs256(claims: &J) -> String {
        let pair = ring::signature::RsaKeyPair::from_der(RSA).unwrap();
        let input = format!("{}.{}", crate::jwt::b64(br#"{"alg":"RS256","kid":"test-rsa"}"#), crate::jwt::b64(claims.to_string().as_bytes()));
        let mut sig = vec![0u8; pair.public().modulus_len()];
        pair.sign(&ring::signature::RSA_PKCS1_SHA256, &ring::rand::SystemRandom::new(), input.as_bytes(), &mut sig).unwrap();
        format!("{input}.{}", crate::jwt::b64(&sig))
    }

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("zetlyn-oidc-{tag}-{}", std::process::id()))
    }

    #[test]
    fn somebody_signed_in_to_one_world_is_known_to_another_that_registered_nothing() {
        let (pa, pb) = (free_port(), free_port());
        let (a, b) = (tmp("a"), tmp("b"));
        let url_a = world(&a, pa, "");
        let url_b = world(&b, pb, &format!("identity:\n- zetlyn: {url_a}\n"));

        // A says what it is, and with which key it signs.
        let (_, disc, _, _) = ask("GET", &format!("{url_a}/.well-known/openid-configuration"), None, None);
        let disc: J = serde_json::from_str(&disc).unwrap();
        assert_eq!(disc["issuer"], url_a);
        assert_eq!(disc["code_challenge_methods_supported"], json!(["S256"]));
        let (_, jwks, _, _) = ask("GET", &format!("{url_a}/oauth/jwks"), None, None);
        assert_eq!(serde_json::from_str::<J>(&jwks).unwrap()["keys"][0]["kty"], "OKP");
        // B is a client by an address of its own.
        let (_, client, _, _) = ask("GET", &format!("{url_b}/oauth/client.json"), None, None);
        assert_eq!(serde_json::from_str::<J>(&client).unwrap()["redirect_uris"], json!([format!("{url_b}/oauth/callback")]));

        // Signing in at B with A: off to A, which asks who it is first.
        let (status, _, to_a, _) = ask("GET", &format!("{url_b}/oauth/login?with=zetlyn:{url_a}&next=/after"), None, None);
        assert_eq!(status, 303);
        assert!(to_a.starts_with(&format!("{url_a}/oauth/authorize?")), "{to_a}");
        let q = query_of(&to_a);
        assert_eq!(q["client_id"], format!("{url_b}/oauth/client.json"));
        assert_eq!(q["code_challenge_method"], "S256");
        let (status, page, _, _) = ask("GET", &to_a, None, None);
        assert_eq!(status, 200);
        assert!(page.contains("Send me a link"), "{page}");

        // Signed in at A, Ann is asked, and says yes.
        let accounts_a = Accounts::open(&a).unwrap();
        let ann = accounts_a.ensure("ann@example.org").unwrap();
        let zo = format!("zo={}", accounts_a.new_session(ann.id).unwrap());
        let (_, consent, _, _) = ask("GET", &to_a, Some(&zo), None);
        assert!(consent.contains("asks who you are") && consent.contains("reader:"), "{consent}");
        let mut form: Vec<String> = q.iter().map(|(k, v)| format!("{k}={}", crate::serve::urlencode(v))).collect();
        form.push("allow=1".into());
        form.push("name=Ann".into());
        let (status, _, back, _) = ask("POST", &format!("{url_a}/oauth/authorize"), Some(&zo), Some(&form.join("&")));
        assert_eq!(status, 303);
        assert!(back.starts_with(&format!("{url_b}/oauth/callback?code=")), "{back}");

        // Back at B: a reader of its own, by A's word, with no address and no account made first.
        let (status, page, next, cookie) = ask("GET", &back, None, None);
        assert_eq!((status, next.as_str()), (303, "/after"), "{page}");
        assert!(cookie.starts_with("zr=") && cookie.contains("Path=/;"), "{cookie}");
        let accounts_b = Accounts::open(&b).unwrap();
        let pseudonym_a = crate::propose::pseudonym(&a, ann.id).unwrap();
        let there = accounts_b.by_identity(&url_a, &pseudonym_a).expect("linked to A's subject");
        assert!(there.email.ends_with(".invalid"), "no address was shared: {}", there.email);
        assert_eq!(accounts_b.name_of(there.id), "Ann");
        let session = cookie.split(';').next().unwrap().trim_start_matches("zr=").to_string();
        assert_eq!(accounts_b.by_session(&session).unwrap().id, there.id);
        // The same answer twice is nothing the second time.
        let (status, again, _, _) = ask("GET", &back, None, None);
        assert_eq!(status, 400, "{again}");

        // `readers` by world: A's people may propose at B, nobody else's.
        let reader = crate::propose::Reader { id: crate::propose::pseudonym(&b, there.id).unwrap(), name: "Ann".into(), email: there.email.clone(), owner: false, issuers: accounts_b.issuers_of(there.id) };
        assert!(crate::propose::admits(&[format!("@127.0.0.1:{pa}")], &reader));
        assert!(crate::propose::admits(&[format!("@{url_a}")], &reader));
        assert!(!crate::propose::admits(&["@zetlyn.com".to_string(), "domain:example.org".to_string()], &reader), "no verified address, so no domain");

        // A refuses a redirect the client's own document does not name, and a code twice.
        let evil = to_a.replace(&crate::serve::urlencode(&format!("{url_b}/oauth/callback")), &crate::serve::urlencode("http://127.0.0.1:1/steal"));
        let (status, page, _, _) = ask("GET", &evil, Some(&zo), None);
        assert_eq!(status, 400);
        assert!(page.contains("not where"), "{page}");
        let code = query_of(&back)["code"].clone();
        let (status, body, _, _) = ask("POST", &format!("{url_a}/oauth/token"), None, Some(&format!("grant_type=authorization_code&code={code}&redirect_uri=x&client_id=y&code_verifier=z")));
        assert_eq!(status, 400);
        assert!(body.contains("invalid_grant"));
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn a_token_is_given_for_the_verifier_the_challenge_was_made_from_and_says_only_what_was_allowed() {
        let pa = free_port();
        let a = tmp("token");
        let url_a = world(&a, pa, "");
        // A client whose document is served by a mock that learns its own address once it has one.
        let address = Arc::new(Mutex::new(String::new()));
        let a2 = address.clone();
        let client = mock(move |_, path, _, _| {
            let at = a2.lock().unwrap().clone();
            if path == "/client.json" { (200, json!({ "client_id": format!("{at}/client.json"), "redirect_uris": [format!("{at}/back")] }).to_string()) } else { (404, String::new()) }
        });
        *address.lock().unwrap() = client.clone();
        let client_id = format!("{client}/client.json");
        let redirect = format!("{client}/back");
        let accounts = Accounts::open(&a).unwrap();
        let ann = accounts.ensure("ann@example.org").unwrap();
        let zo = format!("zo={}", accounts.new_session(ann.id).unwrap());
        let verifier = crate::jwt::random();
        let challenge = crate::jwt::sha256_b64(verifier.as_bytes());
        let code_for = |share: bool| {
            let form = format!(
                "response_type=code&client_id={}&redirect_uri={}&scope=openid+profile+email&state=s&nonce=n1&code_challenge={challenge}&code_challenge_method=S256&allow=1&name=Ann{}",
                crate::serve::urlencode(&client_id), crate::serve::urlencode(&redirect), if share { "&share_email=1" } else { "" }
            );
            let (status, page, back, _) = ask("POST", &format!("{url_a}/oauth/authorize"), Some(&zo), Some(&form));
            assert_eq!(status, 303, "{page}");
            query_of(&back)["code"].clone()
        };
        let token = |code: &str, v: &str| ask("POST", &format!("{url_a}/oauth/token"), None, Some(&format!("grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={v}", crate::serve::urlencode(code), crate::serve::urlencode(&redirect), crate::serve::urlencode(&client_id))));
        // The wrong verifier, and the code is spent with it.
        let code = code_for(false);
        assert!(token(&code, "not-it").1.contains("invalid_grant"));
        assert!(token(&code, &verifier).1.contains("invalid_grant"), "a code is good once");
        // The right one: an ID token from A's key, the name, and no address unless it was allowed.
        let (status, body, _, _) = token(&code_for(false), &verifier);
        assert_eq!(status, 200, "{body}");
        let body: J = serde_json::from_str(&body).unwrap();
        let (_, jwks, _, _) = ask("GET", &format!("{url_a}/oauth/jwks"), None, None);
        let keys = serde_json::from_str::<J>(&jwks).unwrap()["keys"].as_array().unwrap().clone();
        let claims = crate::jwt::verify(body["id_token"].as_str().unwrap(), &keys).unwrap();
        crate::jwt::check(&claims, &url_a, &client_id, Some("n1"), crate::now()).unwrap();
        assert_eq!((claims["name"].as_str(), claims.get("email")), (Some("Ann"), None));
        let (_, info, _, _) = {
            let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
            let mut r = agent.get(&format!("{url_a}/oauth/userinfo")).header("Authorization", &format!("Bearer {}", body["access_token"].as_str().unwrap())).call().unwrap();
            (r.status().as_u16(), r.body_mut().read_to_string().unwrap(), String::new(), String::new())
        };
        assert_eq!(serde_json::from_str::<J>(&info).unwrap()["sub"], claims["sub"]);
        let (_, body, _, _) = token(&code_for(true), &verifier);
        let claims = crate::jwt::verify(serde_json::from_str::<J>(&body).unwrap()["id_token"].as_str().unwrap(), &keys).unwrap();
        assert_eq!((claims["email"].as_str(), claims["email_verified"].as_bool()), (Some("ann@example.org"), Some(true)));
        let _ = std::fs::remove_dir_all(&a);
    }

    /// A fake OpenID provider in the shape of Google or Apple: discovery, keys, and a token endpoint
    /// that hands out an RS256 ID token with `claims` and the nonce of the last authorize. What the
    /// token endpoint was sent is kept in `sent`.
    fn fake_provider(claims: J, sent: Arc<Mutex<String>>) -> (String, Arc<Mutex<String>>) {
        let nonce = Arc::new(Mutex::new(String::new()));
        let issuer = Arc::new(Mutex::new(String::new()));
        let (n, i) = (nonce.clone(), issuer.clone());
        let url = mock(move |_, path, _, body| match path {
            "/.well-known/openid-configuration" => {
                let iss = i.lock().unwrap().clone();
                (200, json!({ "issuer": iss, "authorization_endpoint": format!("{iss}/authorize"), "token_endpoint": format!("{iss}/token"), "jwks_uri": format!("{iss}/keys") }).to_string())
            }
            "/keys" => (200, format!("{{\"keys\":[{}]}}", RSA_JWK.trim())),
            "/token" => {
                *sent.lock().unwrap() = body.to_string();
                let mut c = claims.clone();
                c["iss"] = json!(i.lock().unwrap().clone());
                c["nonce"] = json!(n.lock().unwrap().clone());
                c["iat"] = json!(crate::now());
                c["exp"] = json!(crate::now() + 300);
                (200, json!({ "access_token": "x", "id_token": rs256(&c) }).to_string())
            }
            _ => (404, String::new()),
        });
        *issuer.lock().unwrap() = url.clone();
        (url, nonce)
    }

    #[test]
    fn google_is_taken_by_its_signed_word_and_only_for_its_domain_where_one_is_named() {
        let sent = Arc::new(Mutex::new(String::new()));
        let (google, nonce) = fake_provider(json!({ "aud": "cid", "sub": "g-123", "email": "ann@example.com", "email_verified": true, "hd": "example.com", "name": "Ann G" }), sent.clone());
        let pb = free_port();
        let b = tmp("google");
        let url_b = world(&b, pb, &format!("identity:\n- google:\n    client: cid\n    secret: shh\n    domain: example.com\n    issuer: {google}\n"));
        let (status, _, to, _) = ask("GET", &format!("{url_b}/oauth/login?with=google&next=/t/x/"), None, None);
        assert_eq!(status, 303);
        let q = query_of(&to);
        assert!(to.starts_with(&format!("{google}/authorize?")) && q["hd"] == "example.com" && q["scope"] == "openid email profile");
        *nonce.lock().unwrap() = q["nonce"].clone();
        let (status, page, next, cookie) = ask("GET", &format!("{url_b}/oauth/callback?code=c1&state={}", crate::serve::urlencode(&q["state"])), None, None);
        assert_eq!((status, next.as_str()), (303, "/t/x/"), "{page}");
        assert!(cookie.starts_with("zr="));
        assert!(sent.lock().unwrap().contains("client_secret=shh") && sent.lock().unwrap().contains("code_verifier="), "{}", sent.lock().unwrap());
        let accounts = Accounts::open(&b).unwrap();
        let ann = accounts.by_identity(&google, "g-123").unwrap();
        assert_eq!((ann.email.as_str(), accounts.name_of(ann.id).as_str()), ("ann@example.com", "Ann G"));
        // Somebody of another domain is turned away.
        let _ = std::fs::remove_dir_all(&b);
        let sent = Arc::new(Mutex::new(String::new()));
        let (google, nonce) = fake_provider(json!({ "aud": "cid", "sub": "g-9", "email": "eve@elsewhere.example", "email_verified": true, "hd": "elsewhere.example" }), sent);
        let pc = free_port();
        let c = tmp("google-other");
        let url_c = world(&c, pc, &format!("identity:\n- google:\n    client: cid\n    secret: shh\n    domain: example.com\n    issuer: {google}\n"));
        let (_, _, to, _) = ask("GET", &format!("{url_c}/oauth/login?with=google&next=/"), None, None);
        let q = query_of(&to);
        *nonce.lock().unwrap() = q["nonce"].clone();
        let (status, page, _, _) = ask("GET", &format!("{url_c}/oauth/callback?code=c&state={}", crate::serve::urlencode(&q["state"])), None, None);
        assert_eq!(status, 400);
        assert!(page.contains("only people of example.com"), "{page}");
        assert!(Accounts::open(&c).unwrap().by_identity(&google, "g-9").is_none());
        let _ = std::fs::remove_dir_all(&c);
    }

    #[test]
    fn apple_is_sent_a_secret_signed_here_and_its_once_only_name_is_kept() {
        let sent = Arc::new(Mutex::new(String::new()));
        let (apple, nonce) = fake_provider(json!({ "aud": "com.example.signin", "sub": "a-001", "email": "x1@privaterelay.appleid.com", "email_verified": "true" }), sent.clone());
        let pb = free_port();
        let b = tmp("apple");
        std::env::set_var("ZETLYN_TEST_APPLE_KEY", EC_P8);
        let url_b = world(&b, pb, &format!("identity:\n- apple:\n    client: com.example.signin\n    team: TEAM123\n    key_id: KEY123\n    key: ${{ZETLYN_TEST_APPLE_KEY}}\n    issuer: {apple}\n"));
        let (_, _, to, _) = ask("GET", &format!("{url_b}/oauth/login?with=apple&next=/"), None, None);
        let q = query_of(&to);
        assert_eq!((q["response_mode"].as_str(), q["scope"].as_str()), ("form_post", "name email"));
        *nonce.lock().unwrap() = q["nonce"].clone();
        // Apple answers with a form, from its own page, the name beside the code.
        let user = crate::serve::urlencode(r#"{"name":{"firstName":"Ann","lastName":"Apple"}}"#);
        let (status, page, _, cookie) = ask("POST", &format!("{url_b}/oauth/callback"), None, Some(&format!("code=c&state={}&user={user}", crate::serve::urlencode(&q["state"]))));
        assert_eq!(status, 303, "{page}");
        assert!(cookie.starts_with("zr="));
        // What it was sent as a secret is a token signed with the .p8, for Apple, from the team.
        let form = pairs(&sent.lock().unwrap());
        let pair = ring::signature::EcdsaKeyPair::from_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &base64::engine::general_purpose::STANDARD.decode(EC_P8.lines().filter(|l| !l.starts_with("-----")).collect::<String>()).unwrap(),
            &ring::rand::SystemRandom::new(),
        )
        .unwrap();
        use ring::signature::KeyPair;
        let point = pair.public_key().as_ref();
        let jwk = json!({ "kty": "EC", "crv": "P-256", "kid": "KEY123", "x": crate::jwt::b64(&point[1..33]), "y": crate::jwt::b64(&point[33..65]) });
        let secret = crate::jwt::verify(&form["client_secret"], &[jwk]).unwrap();
        assert_eq!((secret["iss"].as_str(), secret["sub"].as_str(), secret["aud"].as_str()), (Some("TEAM123"), Some("com.example.signin"), Some(apple.as_str())));
        let accounts = Accounts::open(&b).unwrap();
        let ann = accounts.by_identity(&apple, "a-001").unwrap();
        assert_eq!((ann.email.as_str(), accounts.name_of(ann.id).as_str()), ("x1@privaterelay.appleid.com", "Ann Apple"));
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn github_says_who_it_is_through_its_api_and_its_verified_primary_address() {
        let sent = Arc::new(Mutex::new(String::new()));
        let s = sent.clone();
        let github = mock(move |method, path, _, body| match (method, path) {
            ("POST", "/login/oauth/access_token") => {
                *s.lock().unwrap() = body.to_string();
                (200, json!({ "access_token": "gho_x", "token_type": "bearer" }).to_string())
            }
            ("GET", "/user") => (200, json!({ "id": 42, "login": "ann", "name": "Ann Hub" }).to_string()),
            ("GET", "/user/emails") => (200, json!([{ "email": "old@example.org", "primary": false, "verified": true }, { "email": "ann@example.org", "primary": true, "verified": true }]).to_string()),
            _ => (404, String::new()),
        });
        let pb = free_port();
        let b = tmp("github");
        let url_b = world(&b, pb, &format!("identity:\n- github:\n    client: gid\n    secret: gsecret\n    web: {github}\n    api: {github}\n"));
        // Somebody already a reader by address is the same person through GitHub.
        let accounts = Accounts::open(&b).unwrap();
        let before = accounts.ensure("ann@example.org").unwrap();
        let (_, _, to, _) = ask("GET", &format!("{url_b}/oauth/login?with=github&next=/"), None, None);
        assert!(to.starts_with(&format!("{github}/login/oauth/authorize?")), "{to}");
        let q = query_of(&to);
        let (status, page, _, _) = ask("GET", &format!("{url_b}/oauth/callback?code=c&state={}", crate::serve::urlencode(&q["state"])), None, None);
        assert_eq!(status, 303, "{page}");
        assert!(sent.lock().unwrap().contains("client_secret=gsecret"));
        let ann = accounts.by_identity(&github, "42").unwrap();
        assert_eq!(ann.id, before.id, "one person, one account");
        assert_eq!(accounts.name_of(ann.id), "Ann Hub");
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn a_world_offers_zetlyn_com_unless_it_says_otherwise() {
        let site: Site = serde_saphyr::from_str("title: t\n").unwrap();
        assert_eq!(options(&site), vec![("zetlyn:https://zetlyn.com".to_string(), "Sign in with zetlyn.com".to_string())]);
        let site: Site = serde_saphyr::from_str("identity: []\n").unwrap();
        assert!(options(&site).is_empty());
        let site: Site = serde_saphyr::from_str("identity:\n- zetlyn: any\n- github:\n    client: a\n    secret: b\n").unwrap();
        assert_eq!(options(&site).iter().map(|(i, _)| i.as_str()).collect::<Vec<_>>(), vec!["world", "github"]);
    }

    #[test]
    fn somebody_with_no_account_anywhere_signs_in_to_a_world_with_github_through_the_machine_and_the_world_registered_nothing() {
        // GitHub, as the machine knows it.
        let github = mock(move |method, path, _, _| match (method, path) {
            ("POST", "/login/oauth/access_token") => (200, json!({ "access_token": "gho_x" }).to_string()),
            ("GET", "/user") => (200, json!({ "id": 7, "login": "newcomer", "name": "New Comer" }).to_string()),
            ("GET", "/user/emails") => (200, json!([{ "email": "new@example.org", "primary": true, "verified": true }]).to_string()),
            _ => (404, String::new()),
        });
        // The machine: "Sign in with zetlyn.com", which takes GitHub.
        let pm = free_port();
        let m = tmp("machine");
        let _ = std::fs::remove_dir_all(&m);
        std::fs::create_dir_all(m.join("orgs")).unwrap();
        let url_m = format!("http://127.0.0.1:{pm}");
        std::fs::write(m.join("workspace.yaml"), format!("title: The machine\nurl: {url_m}\nidentity:\n- github:\n    client: gid\n    secret: gs\n    web: {github}\n    api: {github}\n")).unwrap();
        let args: Vec<String> = ["hosting", "serve", m.to_str().unwrap(), "--addr", &format!("127.0.0.1:{pm}")].iter().map(|s| s.to_string()).collect();
        // Ends with the test process; nothing outlives it.
        std::thread::spawn(move || crate::app::hosting(&args));
        up(pm);
        let (_, disc, _, _) = ask("GET", &format!("{url_m}/.well-known/openid-configuration"), None, None);
        let disc: J = serde_json::from_str(&disc).unwrap();
        assert_eq!((disc["issuer"].as_str(), disc["authorization_endpoint"].as_str()), (Some(url_m.as_str()), Some(format!("{url_m}/app/oauth/authorize").as_str())));
        // A world that takes the machine, and nothing else, and registered with nobody.
        let pb = free_port();
        let b = tmp("chain-b");
        let url_b = world(&b, pb, &format!("identity:\n- zetlyn: {url_m}\n"));

        let (_, _, to_m, _) = ask("GET", &format!("{url_b}/oauth/login?with=zetlyn:{url_m}&next=/t/x/"), None, None);
        assert!(to_m.starts_with(&format!("{url_m}/app/oauth/authorize?")), "{to_m}");
        // The machine asks who it is, and offers GitHub.
        let (_, page, _, _) = ask("GET", &to_m, None, None);
        let start = page.find("/app/oauth/login?with=github").expect("GitHub offered");
        let href: String = page[start..].chars().take_while(|c| *c != '"').collect::<String>().replace("&amp;", "&");
        let (status, _, to_github, _) = ask("GET", &format!("{url_m}{href}"), None, None);
        assert_eq!(status, 303);
        assert!(to_github.starts_with(&format!("{github}/login/oauth/authorize?")), "{to_github}");
        // GitHub sends them back to the machine, which signs them in there and returns to asking.
        let state = query_of(&to_github)["state"].clone();
        let (status, page, back_to_authorize, zo) = ask("GET", &format!("{url_m}/app/oauth/callback?code=c&state={}", crate::serve::urlencode(&state)), None, None);
        assert_eq!(status, 303, "{page}");
        assert!(zo.starts_with("zo=") && zo.contains("Path=/app/oauth"), "{zo}");
        assert!(back_to_authorize.starts_with("/app/oauth/authorize?"), "{back_to_authorize}");
        let zo = zo.split(';').next().unwrap().to_string();
        let (_, consent, _, _) = ask("GET", &format!("{url_m}{back_to_authorize}"), Some(&zo), None);
        assert!(consent.contains("asks who you are"), "{consent}");
        let mut form: Vec<String> = query_of(&to_m).iter().map(|(k, v)| format!("{k}={}", crate::serve::urlencode(v))).collect();
        form.push("allow=1".into());
        let (status, _, back_to_b, _) = ask("POST", &format!("{url_m}/app/oauth/authorize"), Some(&zo), Some(&form.join("&")));
        assert_eq!(status, 303);
        // And at the world: a reader, the machine's word for who, the name GitHub gave.
        let (status, page, next, cookie) = ask("GET", &back_to_b, None, None);
        assert_eq!((status, next.as_str()), (303, "/t/x/"), "{page}");
        assert!(cookie.starts_with("zr="));
        let machine = Accounts::open(&m).unwrap();
        let newcomer = machine.by_identity(&github, "7").expect("the machine knows them by GitHub");
        assert_eq!(newcomer.email, "new@example.org");
        let world_b = Accounts::open(&b).unwrap();
        let there = world_b.by_identity(&url_m, &crate::propose::pseudonym(&m, newcomer.id).unwrap()).expect("the world knows them by the machine");
        assert_eq!(world_b.name_of(there.id), "New Comer");
        assert!(there.email.ends_with(".invalid"), "the world was not told the address: {}", there.email);
        let _ = std::fs::remove_dir_all(&m);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn an_organisation_signs_in_through_the_machine_it_runs_on_without_waiting_for_itself() {
        let pm = free_port();
        let m = tmp("same-process");
        let _ = std::fs::remove_dir_all(&m);
        let url_m = format!("http://127.0.0.1:{pm}");
        std::fs::create_dir_all(m.join("orgs/acme/sources")).unwrap();
        std::fs::create_dir_all(m.join("orgs/acme/trackers")).unwrap();
        std::fs::write(m.join("workspace.yaml"), format!("title: The machine\nurl: {url_m}\n")).unwrap();
        // The organisation names no address of its own and takes the machine, as it does by default.
        std::fs::write(m.join("orgs/acme/workspace.yaml"), format!("title: Acme\nidentity:\n- zetlyn: {url_m}\n")).unwrap();
        let args: Vec<String> = ["hosting", "serve", m.to_str().unwrap(), "--addr", &format!("127.0.0.1:{pm}")].iter().map(|s| s.to_string()).collect();
        // Ends with the test process; nothing outlives it.
        std::thread::spawn(move || crate::app::hosting(&args));
        up(pm);
        let started = std::time::Instant::now();
        let (status, page, to_m, _) = ask("GET", &format!("{url_m}/acme/oauth/login?with=zetlyn:{url_m}&next=/acme/"), None, None);
        assert_eq!(status, 303, "{page}");
        assert!(to_m.starts_with(&format!("{url_m}/app/oauth/authorize?")), "{to_m}");
        let machine = Accounts::open(&m).unwrap();
        let ann = machine.ensure("ann@example.org").unwrap();
        let zo = format!("zo={}", machine.new_session(ann.id).unwrap());
        let (status, consent, _, _) = ask("GET", &to_m, Some(&zo), None);
        assert_eq!(status, 200, "the machine fetched the organisation's client document from itself: {consent}");
        let mut form: Vec<String> = query_of(&to_m).iter().map(|(k, v)| format!("{k}={}", crate::serve::urlencode(v))).collect();
        form.push("allow=1".into());
        let (_, _, back, _) = ask("POST", &format!("{url_m}/app/oauth/authorize"), Some(&zo), Some(&form.join("&")));
        assert!(back.starts_with(&format!("{url_m}/acme/oauth/callback?")), "{back}");
        let (status, page, next, cookie) = ask("GET", &back, None, None);
        assert_eq!((status, next.as_str()), (303, "/acme/"), "{page}");
        assert!(cookie.starts_with("zr=") && cookie.contains("Path=/acme;"), "{cookie}");
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "it waited for itself: {:?}", started.elapsed());
        let acme = Accounts::open(&m.join("orgs/acme")).unwrap();
        assert!(acme.by_identity(&url_m, &crate::propose::pseudonym(&m, ann.id).unwrap()).is_some());
        let _ = std::fs::remove_dir_all(&m);
    }
}
