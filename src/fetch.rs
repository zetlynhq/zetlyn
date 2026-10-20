//! Reading a resource somebody named: one GET, its redirects followed by hand so that each place
//! it goes is checked (`outbound.rs`), its body bounded.

use std::time::Duration;

pub const AGENT: &str = "ZetlynBot/0.1 (+https://zetlyn.com)";

/// A page larger than this is not a page somebody reads.
pub const BODY_LIMIT: u64 = 5 * 1024 * 1024;

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
    let body = decode(&got.bytes, &got.content_type);
    Ok(Fetched { url: got.url, content_type: got.content_type, body })
}

/// A page's bytes as text, in the charset it is written in: the one its `Content-Type` names,
/// else its byte order mark, else a `<meta>` near its top; with none of them, UTF-8 where the
/// bytes are UTF-8 and windows-1252 where they are not, as browsers read such pages.
pub fn decode(bytes: &[u8], content_type: &str) -> String {
    let named = charset_in(content_type).or_else(|| {
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(2048)]).to_ascii_lowercase();
        head.find("<meta").and_then(|_| meta_charset(&head))
    });
    let encoding = encoding_rs::Encoding::for_bom(bytes)
        .map(|(e, _)| e)
        .or_else(|| named.and_then(|n| encoding_rs::Encoding::for_label(n.as_bytes())))
        .unwrap_or(if std::str::from_utf8(bytes).is_ok() { encoding_rs::UTF_8 } else { encoding_rs::WINDOWS_1252 });
    encoding.decode(bytes).0.into_owned()
}

/// `charset=…` in a header value or a meta's content.
fn charset_in(s: &str) -> Option<String> {
    let lower = s.to_ascii_lowercase();
    let at = lower.find("charset=")? + "charset=".len();
    let name: String = lower[at..].trim_start_matches(['"', '\'', ' ']).chars().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.')).collect();
    (!name.is_empty()).then_some(name)
}

/// The charset a page's `<meta charset=…>` or `<meta http-equiv=content-type content=…>` names,
/// from its lowercased top.
fn meta_charset(head: &str) -> Option<String> {
    head.match_indices("<meta").find_map(|(at, _)| {
        let tag = &head[at..head[at..].find('>').map_or(head.len(), |e| at + e)];
        charset_in(tag)
    })
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
        // A redirect goes on; "300 Multiple Choices" with nowhere named is an answer with a body.
        let location = response.headers().get("location").and_then(|v| v.to_str().ok()).map(str::to_string);
        if (300..400).contains(&status) && !(status == 300 && location.is_none()) {
            let location = location.ok_or_else(|| format!("{at}: HTTP {status} without a Location"))?;
            at = normal(&join(&at, &location));
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
    let normal = normal(url);
    let url = normal.as_str();
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

/// An address as a browser sends it: a host written in other letters than ASCII as Punycode
/// (`münchen.de` is `xn--mnchen-3ya.de`), and the spaces and other characters an address cannot
/// carry percent-encoded (`a b` is `a%20b`). What is encoded already stays as it is.
pub fn normal(url: &str) -> String {
    let url = url.trim();
    let Some((scheme, rest)) = url.split_once("://") else { return url.to_string() };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let (userinfo, hostport) = authority.rsplit_once('@').map_or((None, authority), |(u, h)| (Some(u), h));
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && p.chars().all(|c| c.is_ascii_digit()) && !hostport.starts_with('[') => (h, Some(p)),
        _ => (hostport, None),
    };
    let host = if host.starts_with('[') {
        host.to_string()
    } else {
        host.split('.')
            .map(|label| {
                let label = label.to_lowercase();
                if label.is_ascii() { label } else { punycode(&label).map(|p| format!("xn--{p}")).unwrap_or(label) }
            })
            .collect::<Vec<_>>()
            .join(".")
    };
    let mut tail_out = String::new();
    for c in tail.chars() {
        if c.is_ascii() && !matches!(c, ' ' | '"' | '<' | '>' | '`' | '{' | '}' | '|' | '\\' | '^') && !c.is_ascii_control() {
            tail_out.push(c);
        } else {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                tail_out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    let userinfo = userinfo.map(|u| format!("{u}@")).unwrap_or_default();
    let port = port.map(|p| format!(":{p}")).unwrap_or_default();
    format!("{scheme}://{userinfo}{host}{port}{tail_out}")
}

/// A label in Punycode, as RFC 3492 encodes it (without the `xn--` before it).
fn punycode(label: &str) -> Option<String> {
    const BASE: u32 = 36;
    const TMIN: u32 = 1;
    const TMAX: u32 = 26;
    let adapt = |mut delta: u32, points: u32, first: bool| {
        delta = if first { delta / 700 } else { delta / 2 };
        delta += delta / points;
        let mut k = 0;
        while delta > ((BASE - TMIN) * TMAX) / 2 {
            delta /= BASE - TMIN;
            k += BASE;
        }
        k + (BASE - TMIN + 1) * delta / (delta + 38)
    };
    let digit = |d: u32| char::from(if d < 26 { b'a' + d as u8 } else { b'0' + (d - 26) as u8 });
    let input: Vec<u32> = label.chars().map(|c| c as u32).collect();
    let mut out: String = label.chars().filter(char::is_ascii).collect();
    let basic = out.chars().count() as u32;
    if basic > 0 {
        out.push('-');
    }
    let (mut n, mut delta, mut bias, mut h) = (128u32, 0u32, 72u32, basic);
    while (h as usize) < input.len() {
        let m = *input.iter().filter(|&&c| c >= n).min()?;
        delta = delta.checked_add((m - n).checked_mul(h + 1)?)?;
        n = m;
        for &c in &input {
            if c < n {
                delta = delta.checked_add(1)?;
            }
            if c == n {
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = if k <= bias { TMIN } else if k >= bias + TMAX { TMAX } else { k - bias };
                    if q < t {
                        break;
                    }
                    out.push(digit(t + (q - t) % (BASE - t)));
                    q = (q - t) / (BASE - t);
                    k += BASE;
                }
                out.push(digit(q));
                bias = adapt(delta, h + 1, h == basic);
                delta = 0;
                h += 1;
            }
        }
        delta += 1;
        n += 1;
    }
    Some(out)
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

    #[test]
    fn a_page_is_read_in_the_charset_it_is_written_in() {
        let sjis = [b"<html><head><meta charset=\"Shift_JIS\"></head><body>".as_slice(), &[0x83, 0x81, 0x83, 0x43, 0x83, 0x93], b"</body></html>"].concat();
        assert!(decode(&sjis, "text/html").contains("メイン"), "named by its meta");
        let latin1 = [b"<p>".as_slice(), &[0x47, 0x72, 0xfc, 0xdf, 0x65], b"</p>"].concat();
        assert!(decode(&latin1, "text/html; charset=ISO-8859-1").contains("Grüße"), "named by its header");
        assert!(decode(&latin1, "text/html").contains("Grüße"), "not UTF-8 and nothing named: windows-1252");
        let equiv = [b"<meta http-equiv=\"Content-Type\" content=\"text/html; charset=iso-8859-1\"><p>".as_slice(), &[0xfc], b"</p>"].concat();
        assert!(decode(&equiv, "text/html").contains('ü'));
        assert_eq!(decode("Grüße".as_bytes(), "text/html"), "Grüße", "UTF-8 stays UTF-8");
        assert_eq!(decode(&[0xef, 0xbb, 0xbf, b'a'], "text/html; charset=iso-8859-1"), "a", "a byte order mark wins");
    }

    #[test]
    fn an_address_is_sent_as_a_browser_sends_it() {
        assert_eq!(punycode("münchen").as_deref(), Some("mnchen-3ya"));
        assert_eq!(punycode("bücher").as_deref(), Some("bcher-kva"));
        assert_eq!(punycode("例え").as_deref(), Some("r8jz45g"));
        assert_eq!(normal("https://münchen.de/"), "https://xn--mnchen-3ya.de/");
        assert_eq!(normal("https://Bücher.de:8443/Sale Angebote?q=Grüße#x"), "https://xn--bcher-kva.de:8443/Sale%20Angebote?q=Gr%C3%BC%C3%9Fe#x");
        assert_eq!(normal("https://例え.jp/"), "https://xn--r8jz45g.jp/");
        assert_eq!(normal("https://example.com/a%20b?x=1"), "https://example.com/a%20b?x=1", "encoded already: as it is");
        assert_eq!(normal("http://[::1]:4851/"), "http://[::1]:4851/");
        assert_eq!(clean("https://www.rust-lang.org/le arn"), "https://www.rust-lang.org/le%20arn");
    }
}
