//! Reading a resource somebody named: one GET, its redirects followed by hand so that each place
//! it goes is checked (`outbound.rs`), its body bounded.

use std::time::Duration;

pub const AGENT: &str = "ZetlynBot/0.1 (+https://zetlyn.com)";

/// A page larger than this is not a page somebody reads.
const BODY_LIMIT: u64 = 5 * 1024 * 1024;

pub struct Fetched {
    /// Where it was read, after redirects.
    pub url: String,
    pub content_type: String,
    pub body: String,
}

impl Fetched {
    pub fn is_html(&self) -> bool {
        let ct = self.content_type.to_ascii_lowercase();
        ct.contains("html") || (ct.is_empty() && self.body.trim_start().starts_with('<'))
    }
}

pub fn get(url: &str) -> Result<Fetched, String> {
    let got = get_bytes(url, BODY_LIMIT)?;
    Ok(Fetched { url: got.url, content_type: got.content_type, body: String::from_utf8_lossy(&got.bytes).into_owned() })
}

/// What was read, as it came: an image, a font, a stylesheet.
pub struct Got {
    pub url: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// One GET of anything, at most `limit` bytes of it.
pub fn get_bytes(url: &str, limit: u64) -> Result<Got, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(AGENT)
        .timeout_global(Some(Duration::from_secs(20)))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into();
    let mut at = url.to_string();
    for _ in 0..6 {
        crate::outbound::public_target(&at)?;
        let mut response = agent.get(&at).call().map_err(|e| format!("{at}: {e}"))?;
        let status = response.status().as_u16();
        if (300..400).contains(&status) {
            let location = response.headers().get("location").and_then(|v| v.to_str().ok()).ok_or_else(|| format!("{at}: HTTP {status} without a Location"))?;
            at = join(&at, location);
            continue;
        }
        if status >= 400 {
            return Err(format!("{at}: HTTP {status}"));
        }
        let content_type = response.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        let bytes = response.body_mut().with_config().limit(limit).read_to_vec().map_err(|e| format!("{at}: {e}"))?;
        return Ok(Got { url: at, content_type, bytes });
    }
    Err(format!("{url}: more than five redirects"))
}

/// One POST of JSON to an address somebody gave (a webhook), checked like every address Zetlyn
/// goes to (`outbound.rs`) and not following redirects: a webhook that moved is a webhook gone.
/// The status and the first bytes of the answer.
pub fn post_json(url: &str, body: &str, headers: &[(&str, String)]) -> Result<(u16, String), String> {
    crate::outbound::public_target(url)?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(AGENT)
        .timeout_global(Some(Duration::from_secs(10)))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into();
    let mut req = agent.post(url).header("Content-Type", "application/json");
    for (k, v) in headers {
        req = req.header(*k, v.as_str());
    }
    let mut response = req.send(body).map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let text = response.body_mut().with_config().limit(16 * 1024).read_to_string().unwrap_or_default();
    Ok((status, text))
}

/// An address as somebody pasted it, without what only tracked where they came from: the same
/// page, read as the plain address it is. `utm_…`, click ids and referral marks go; an Amazon
/// product is `/dp/<ASIN>`, which Amazon answers where a link out of its search, read by a
/// program, gets a robot check. Nothing is disguised: the page is the one asked for.
pub fn clean(url: &str) -> String {
    let url = url.split('#').next().unwrap_or(url);
    let (front, query) = url.split_once('?').map_or((url, ""), |(f, q)| (f, q));
    let (scheme, rest) = front.split_once("://").unwrap_or(("https", front));
    let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let bare = host.to_ascii_lowercase();
    let bare = bare.trim_start_matches("www.");
    if bare.starts_with("amazon.") || bare.starts_with("smile.amazon.") {
        let asin = path.split('/').skip_while(|s| *s != "dp" && *s != "product").nth(1).filter(|a| a.len() == 10 && a.chars().all(|c| c.is_ascii_alphanumeric()));
        if let Some(asin) = asin {
            return format!("{scheme}://{host}/dp/{asin}");
        }
    }
    const TRACKING: &[&str] = &["fbclid", "gclid", "dclid", "gbraid", "wbraid", "msclkid", "yclid", "igshid", "mc_cid", "mc_eid", "_ga", "_gl", "srsltid", "ref", "ref_", "ref_src"];
    let kept: Vec<&str> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .filter(|p| {
            let key = p.split('=').next().unwrap_or("").to_ascii_lowercase();
            !key.starts_with("utm_") && !TRACKING.contains(&key.as_str())
        })
        .collect();
    if kept.is_empty() { front.to_string() } else { format!("{front}?{}", kept.join("&")) }
}

/// `location` read against `base`, as a browser reads a redirect.
pub fn join(base: &str, location: &str) -> String {
    if location.contains("://") {
        return location.to_string();
    }
    let (scheme, rest) = base.split_once("://").unwrap_or(("https", base));
    if let Some(l) = location.strip_prefix("//") {
        return format!("{scheme}://{l}");
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if location.starts_with('/') {
        return format!("{scheme}://{host}{location}");
    }
    let path = rest[host.len()..].split(['?', '#']).next().unwrap_or("");
    let dir = match path.rfind('/') {
        Some(i) => &path[..=i],
        None => "/",
    };
    format!("{scheme}://{host}{dir}{location}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_loses_what_only_tracked_where_it_came_from() {
        let amazon = "https://www.amazon.de/-/en/Playstation-5-Pro-2-TB/dp/B0FRYBZR91/ref=sr_1_2?crid=1RRKV7DE07WQE&dib=eyJ2&keywords=ps5+pro&qid=1792487321&sr=1-2";
        assert_eq!(clean(amazon), "https://www.amazon.de/dp/B0FRYBZR91");
        assert_eq!(clean("https://www.amazon.com/gp/product/B08N5WRWNW?th=1"), "https://www.amazon.com/dp/B08N5WRWNW");
        assert_eq!(clean("https://shop.example/p?id=4&utm_source=x&size=XL&gclid=abc#top"), "https://shop.example/p?id=4&size=XL");
        assert_eq!(clean("https://shop.example/p?utm_medium=mail"), "https://shop.example/p");
        assert_eq!(clean("https://www.amazon.de/s?k=ps5"), "https://www.amazon.de/s?k=ps5", "not a product: left as it is");
    }

    #[test]
    fn a_redirect_reads_against_where_it_came_from() {
        assert_eq!(join("https://a.example/x/y?q=1", "https://b.example/z"), "https://b.example/z");
        assert_eq!(join("https://a.example/x/y", "//c.example/z"), "https://c.example/z");
        assert_eq!(join("https://a.example/x/y", "/z"), "https://a.example/z");
        assert_eq!(join("https://a.example/x/y?q=1", "z"), "https://a.example/x/z");
        assert_eq!(join("https://a.example", "z"), "https://a.example/z");
    }
}
