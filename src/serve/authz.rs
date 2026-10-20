//! OAuth 2.1 for the MCP server, as the MCP authorization spec has it, so that an LLM's client
//! needs nothing but `https://zetlyn.com/mcp`: it finds out where to sign in (`/.well-known/…`,
//! RFC 9728 and RFC 8414), registers itself (RFC 7591), sends the person to sign in to Zetlyn and
//! allow it (the authorization code flow, PKCE with S256 required), and trades the code for an
//! access token of an hour and a refresh token of 90 days, which turns once each use.
//!
//! The person signs in as anywhere on the site (a link by mail, Google, Apple), which makes the
//! account when there is none. Clients are public: no secret; PKCE and the registered addresses
//! to send people back to keep a code from being taken by another. Under Account, API keys, the
//! person sees every client let in and can let it out.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::{json, Value as J};
use sha2::{Digest, Sha256};

use super::*;

const ACCESS_TTL: i64 = 3600;
const REFRESH_TTL: i64 = 90 * 86400;
const CODE_TTL: i64 = 600;

/// Its pages, or None for an address that is not one of them.
pub fn route(app: &App, store: &Store, a: &Asked, segments: &[&str]) -> Option<Result<Reply, String>> {
    if app.hosted.is_none() {
        return None;
    }
    let base = public_base(app, a);
    Some(Ok(match (&a.method, segments) {
        (Method::Options, [".well-known", ..] | ["oauth", "register" | "token"] | ["mcp"]) => cors(Response::from_string("").with_status_code(204)),
        (Method::Get, [".well-known", "oauth-protected-resource", ..]) => cors(json_reply(
            200,
            &json!({
                "resource": format!("{base}/mcp"),
                "authorization_servers": [base],
                "bearer_methods_supported": ["header"],
                "scopes_supported": ["watch"],
                "resource_name": "Zetlyn",
                "resource_documentation": format!("{base}/llms.txt"),
            }),
        )),
        (Method::Get, [".well-known", "oauth-authorization-server" | "openid-configuration", ..]) => cors(json_reply(
            200,
            &json!({
                "issuer": base,
                "authorization_endpoint": format!("{base}/oauth/authorize"),
                "token_endpoint": format!("{base}/oauth/token"),
                "registration_endpoint": format!("{base}/oauth/register"),
                "response_types_supported": ["code"],
                "grant_types_supported": ["authorization_code", "refresh_token"],
                "code_challenge_methods_supported": ["S256"],
                "token_endpoint_auth_methods_supported": ["none"],
                "scopes_supported": ["watch"],
                "service_documentation": format!("{base}/llms.txt"),
            }),
        )),
        (Method::Post, ["oauth", "register"]) => cors(register(app, store, a)),
        (Method::Get | Method::Post, ["oauth", "authorize"]) => return Some(authorize(app, store, a)),
        (Method::Post, ["oauth", "token"]) => cors(token(store, a)),
        _ => return None,
    }))
}

/// Answers a browser's page on another site may read: these are called by MCP clients that run
/// in a browser, with a bearer token, never a cookie.
pub fn cors(r: Reply) -> Reply {
    r.with_header(header("Access-Control-Allow-Origin", "*"))
        .with_header(header("Access-Control-Allow-Methods", "GET, POST, OPTIONS"))
        .with_header(header("Access-Control-Allow-Headers", "Authorization, Content-Type, Mcp-Protocol-Version, Mcp-Session-Id"))
        .with_header(header("Access-Control-Expose-Headers", "WWW-Authenticate, Mcp-Session-Id"))
}

fn oauth_error(status: u16, error: &str, description: &str) -> Reply {
    json_reply(status, &json!({ "error": error, "error_description": description })).with_header(header("Cache-Control", "no-store"))
}

/// An address a client may have people sent back to: https; http only to this machine (a
/// program on the person's computer, RFC 8252); or an app's own scheme. Never a script or a file.
fn sendable(uri: &str) -> bool {
    let Some((scheme, rest)) = uri.split_once(':') else { return false };
    let scheme = scheme.to_ascii_lowercase();
    if uri.contains('#') || matches!(scheme.as_str(), "javascript" | "data" | "file" | "vbscript" | "about" | "blob") {
        return false;
    }
    match scheme.as_str() {
        "https" => rest.starts_with("//") && rest.len() > 2,
        "http" => loopback(uri),
        // An app's own scheme: reverse-domain names (com.example.app) or a product's (cursor).
        s => !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-')),
    }
}

fn loopback(uri: &str) -> bool {
    let host = uri.split_once("://").map_or("", |(_, r)| r).split(['/', '?']).next().unwrap_or("");
    let host = host.rsplit_once(':').map_or(host, |(h, p)| if p.chars().all(|c| c.is_ascii_digit()) { h } else { host });
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

/// Whether `uri` is one the client registered; for this machine the port may differ (RFC 8252).
fn registered(uris: &[String], uri: &str) -> bool {
    let without_port = |u: &str| {
        let (scheme, rest) = u.split_once("://").unwrap_or(("", u));
        let (host, path) = rest.split_at(rest.find(['/', '?']).unwrap_or(rest.len()));
        let host = host.rsplit_once(':').map_or(host, |(h, _)| h);
        format!("{scheme}://{host}{path}")
    };
    uris.iter().any(|r| r == uri || (loopback(r) && loopback(uri) && without_port(r) == without_port(uri)))
}

/// RFC 7591: a client tells its name and where people go back to; it gets an id.
fn register(app: &App, store: &Store, a: &Asked) -> Reply {
    if !app.allow("register", &a.ip, 30, 3600) {
        return oauth_error(429, "invalid_request", "Too many registrations from this address; try again later.");
    }
    let body: J = serde_json::from_slice(&a.body).unwrap_or(J::Null);
    let uris: Vec<String> = body["redirect_uris"].as_array().map(|u| u.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    if uris.is_empty() || uris.len() > 10 || !uris.iter().all(|u| u.len() <= 500 && sendable(u)) {
        return oauth_error(400, "invalid_redirect_uri", "redirect_uris: https addresses, http on this machine (localhost), or an app's own scheme.");
    }
    let name: String = body["client_name"].as_str().unwrap_or("An app").trim().chars().filter(|c| !c.is_control()).take(60).collect();
    let name = if name.is_empty() { "An app".to_string() } else { name };
    match store.add_client(&name, &uris) {
        Ok(id) => json_reply(
            201,
            &json!({
                "client_id": id,
                "client_id_issued_at": now(),
                "client_name": name,
                "redirect_uris": uris,
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "token_endpoint_auth_method": "none",
            }),
        )
        .with_header(header("Cache-Control", "no-store")),
        Err(e) => oauth_error(500, "server_error", &e),
    }
}

/// What a request to authorize asks, from the address or from the form that answers it.
struct Ask {
    client: String,
    redirect: String,
    state: String,
    challenge: String,
    method: String,
    response_type: String,
}

fn ask_of(a: &Asked) -> Ask {
    let v = |k: &str| if a.method == Method::Post { a.f(k) } else { a.q(k).unwrap_or_default() };
    Ask { client: v("client_id"), redirect: v("redirect_uri"), state: v("state"), challenge: v("code_challenge"), method: v("code_challenge_method"), response_type: v("response_type") }
}

/// The address to send the person back to, with what to tell the client.
fn back_to(redirect: &str, params: &[(&str, &str)]) -> String {
    let q: String = form_urlencoded::Serializer::new(String::new()).extend_pairs(params.iter().filter(|(_, v)| !v.is_empty())).finish();
    format!("{redirect}{}{q}", if redirect.contains('?') { "&" } else { "?" })
}

/// The person allows a client, or not. Signed out, they sign in first and come back here.
fn authorize(app: &App, store: &Store, a: &Asked) -> Result<Reply, String> {
    let ask = ask_of(a);
    let bad = |what: &str| page_reply(app, a, "Cannot connect", &format!("<section class=narrow><h1>This app cannot connect</h1><p>{}</p><p class=dim>Nothing was shared. Close this page and try again from the app.</p></section>", esc(what)), 400);
    // Who to send back to must be known before anything goes back there.
    let Some((name, uris)) = store.client(&ask.client)? else { return Ok(bad("The app is not registered with Zetlyn.")) };
    if !registered(&uris, &ask.redirect) {
        return Ok(bad("The app asked to be answered at an address it did not register."));
    }
    let refuse = |error: &str| redirect(&back_to(&ask.redirect, &[("error", error), ("state", &ask.state)]));
    if ask.response_type != "code" && a.method == Method::Get {
        return Ok(refuse("unsupported_response_type"));
    }
    if ask.challenge.len() < 43 || ask.method != "S256" {
        return Ok(refuse("invalid_request"));
    }
    let Some(acc) = &a.account else {
        // Signing in first; the sign-in comes back here (`zn`).
        let here = format!("{}?{}", a.path, a.query);
        return Ok(redirect("/signin?connect=1").with_header(next_cookie(app, &here)));
    };
    if a.method == Method::Post {
        if a.f("decision") != "allow" {
            return Ok(refuse("access_denied"));
        }
        let code = store.new_grant("code", &ask.client, acc.id, &ask.redirect, &ask.challenge, CODE_TTL)?;
        return Ok(redirect(&back_to(&ask.redirect, &[("code", &code), ("state", &ask.state)])));
    }
    let to = ask.redirect.split_once("://").map_or(ask.redirect.as_str(), |(_, r)| r).split(['/', '?']).next().unwrap_or("");
    let hidden: String = [("client_id", &ask.client), ("redirect_uri", &ask.redirect), ("state", &ask.state), ("code_challenge", &ask.challenge), ("code_challenge_method", &ask.method)]
        .iter()
        .map(|(k, v)| format!("<input type=hidden name={k} value=\"{}\">", esc(v)))
        .collect();
    Ok(page_reply(
        app,
        a,
        "Connect an app",
        &format!(
            r#"<section class=sent><div class="sent-card signin">
<img src="/mark.png" alt="" class=signin-mark>
<p class=overline>Connect an app</p><h1>Let {n} use Zetlyn?</h1>
<p class=lead>It will act for <b>{email}</b>: read pages and find the places on them, start, change and end watches, and see your watches, their changes and your plan.</p>
<p class=dim>It cannot pay, change your address or delete your account. You can let it out any time under Account, API keys. It returns to {to}.</p>
<form method=post action="/oauth/authorize" class=connect-form>{hidden}<button name=decision value=allow class=primary>Allow</button><button name=decision value=deny>Cancel</button></form>
<div class="dim small not-you">Not you? <form method=post action=/signout class=inline-form><button class=link>Sign out</button></form></div>
</div></section>"#,
            n = esc(&name),
            email = esc(&acc.email),
            to = esc(to),
        ),
        200,
    ))
}

/// Where to go after signing in, for ten minutes: back to the app's request to connect.
pub fn next_cookie(app: &App, path: &str) -> Header {
    let secure = app.hosted.as_ref().is_some_and(|h| h.base.starts_with("https://"));
    let age = if path.is_empty() { 0 } else { 600 };
    header("Set-Cookie", &format!("zn={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={age}{}", enc(path), if secure { "; Secure" } else { "" }))
}

/// The page to go to after signing in, if an app's request to connect is waiting (`zn`).
pub fn next_after_signin(a: &Asked) -> Option<String> {
    let raw = cookie_named(a, "zn");
    let next: String = form_urlencoded::parse(format!("n={raw}").as_bytes()).find(|(k, _)| k == "n").map(|(_, v)| v.into_owned()).unwrap_or_default();
    next.starts_with("/oauth/authorize?").then_some(next)
}

/// A code traded for tokens (PKCE checked), or a refresh for new ones (the old one used up).
fn token(store: &Store, a: &Asked) -> Reply {
    let grant = a.f("grant_type");
    let client = a.f("client_id");
    let given = match grant.as_str() {
        "authorization_code" => match store.take_grant(&a.f("code"), "code") {
            Ok(Some((c, account, redirect_uri, challenge))) => {
                let verifier = a.f("code_verifier");
                let proved = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) == challenge;
                if c != client || redirect_uri != a.f("redirect_uri") || !proved {
                    return oauth_error(400, "invalid_grant", "The code was not given to this client, for this address, or the verifier does not match.");
                }
                (c, account)
            }
            Ok(None) => return oauth_error(400, "invalid_grant", "The code is unknown, used or expired."),
            Err(e) => return oauth_error(500, "server_error", &e),
        },
        "refresh_token" => match store.take_grant(&a.f("refresh_token"), "refresh") {
            Ok(Some((c, account, _, _))) if client.is_empty() || c == client => (c, account),
            Ok(_) => return oauth_error(400, "invalid_grant", "The refresh token is unknown, used or expired."),
            Err(e) => return oauth_error(500, "server_error", &e),
        },
        _ => return oauth_error(400, "unsupported_grant_type", "authorization_code or refresh_token."),
    };
    let (client, account) = given;
    let issued = store.new_grant("access", &client, account, "", "", ACCESS_TTL).and_then(|access| store.new_grant("refresh", &client, account, "", "", REFRESH_TTL).map(|refresh| (access, refresh)));
    match issued {
        Ok((access, refresh)) => json_reply(200, &json!({ "access_token": access, "token_type": "Bearer", "expires_in": ACCESS_TTL, "refresh_token": refresh, "scope": "watch" }))
            .with_header(header("Cache-Control", "no-store")),
        Err(e) => oauth_error(500, "server_error", &e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_is_sent_back_only_where_it_may_be() {
        assert!(sendable("https://claude.ai/api/mcp/auth_callback"));
        assert!(sendable("http://localhost:33418/callback") && sendable("http://127.0.0.1/cb"));
        assert!(sendable("cursor://anysphere.cursor-retrieval/oauth/callback") && sendable("com.example.app:/oauth"));
        assert!(!sendable("http://evil.example/cb"), "plain http only on this machine");
        assert!(!sendable("javascript:alert(1)") && !sendable("data:text/html,x") && !sendable("https://a.example/cb#frag"));
        let uris = vec!["http://127.0.0.1:5000/callback".to_string(), "https://claude.ai/api/mcp/auth_callback".to_string()];
        assert!(registered(&uris, "http://127.0.0.1:61234/callback"), "on this machine the port may change");
        assert!(!registered(&uris, "http://127.0.0.1:61234/other"));
        assert!(registered(&uris, "https://claude.ai/api/mcp/auth_callback"));
        assert!(!registered(&uris, "https://claude.ai/api/mcp/auth_callback2"));
    }

    #[test]
    fn a_code_is_traded_once_by_the_client_that_proves_it_asked() {
        let store = Store::in_memory();
        let account = store.account("a@example.com").unwrap();
        let client = store.add_client("Test", &["https://app.example/cb".into()]).unwrap();
        let verifier = "a-verifier-of-enough-length-for-pkce-0123456789";
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let code = store.new_grant("code", &client, account, "https://app.example/cb", &challenge, CODE_TTL).unwrap();
        let ask = |form: String| Asked { method: Method::Post, path: "/oauth/token".into(), query: String::new(), body: form.clone().into_bytes(), form, headers: vec![], session: None, ip: "1.2.3.4".into(), bearer: None, account: None };
        let form = |verifier: &str| format!("grant_type=authorization_code&code={code}&client_id={client}&redirect_uri=https%3A%2F%2Fapp.example%2Fcb&code_verifier={verifier}");
        assert_eq!(token(&store, &ask(form("wrong-verifier"))).status_code().0, 400, "a verifier that does not match");
        let code2 = store.new_grant("code", &client, account, "https://app.example/cb", &challenge, CODE_TTL).unwrap();
        let ok = token(&store, &ask(form(verifier).replace(&code, &code2)));
        assert_eq!(ok.status_code().0, 200);
        assert_eq!(token(&store, &ask(form(verifier).replace(&code, &code2))).status_code().0, 400, "a code is traded once");
    }
}
