//! Who looks at the site's pages, counted without cookies and without keeping an address. Each
//! view of a public page is kept with its path, where the visitor came from (the site that linked
//! to it, or `direct`), and a mark that tells visitors apart within one day only: a digest of the
//! address and the browser with a secret that is drawn anew each day, and the day before's
//! forgotten. Views are kept 90 days.

/// How long views are kept.
pub const KEPT: i64 = 90 * 86400;

/// A view that came from another of the site's own pages.
pub const INSIDE: &str = "inside";

/// Whether a view of this path is counted: the pages anybody can find, signing in, and the start
/// of marking a page.
pub fn counted(path: &str) -> bool {
    let p = crate::seo::canonical_path(path);
    crate::seo::PUBLIC.contains(&p.as_str()) || crate::pages::find(&p).is_some() || p == "/mark" || p == "/signin"
}

/// Whether the asker is a program by its own say: crawlers, link previews, monitors, scripts,
/// headless browsers.
pub fn is_bot(agent: &str) -> bool {
    const SAYS: &[&str] = &[
        "bot", "crawl", "spider", "slurp", "preview", "fetch", "monitor", "curl", "wget", "python", "go-http", "headless", "lighthouse", "scan", "http-client", "java/", "okhttp", "axios",
        "node-fetch", "facebookexternalhit", "embedly", "whatsapp", "zetlyn",
    ];
    let a = agent.to_ascii_lowercase();
    !a.starts_with("mozilla/") || SAYS.iter().any(|w| a.contains(w))
}

/// Where a view came from: what a tagged link says (`?ref=` or `?utm_source=`), else the site
/// that linked to it by a short name, `inside` from the site's own pages (`own`, its host), or
/// `direct`.
pub fn source(referer: &str, query: &str, own: &str) -> String {
    let tag = form_urlencoded::parse(query.as_bytes()).find(|(k, _)| k == "ref" || k == "utm_source").map(|(_, v)| v.to_lowercase());
    if let Some(tag) = tag {
        let tag: String = tag.chars().filter(|c| c.is_ascii_alphanumeric() || ".-_".contains(*c)).take(40).collect();
        if !tag.is_empty() {
            return tag;
        }
    }
    let host = referer.split_once("://").map_or("", |(_, r)| r).split(['/', '?', '#', ':']).next().unwrap_or("").trim_start_matches("www.").to_ascii_lowercase();
    if host.is_empty() {
        return "direct".into();
    }
    if host == own.trim_start_matches("www.") {
        return INSIDE.into();
    }
    named(&host)
}

/// The sites people come from most, by the name one knows them by.
fn named(host: &str) -> String {
    let is = |d: &str| host == d || host.ends_with(&format!(".{d}"));
    let name = if host.split('.').any(|p| p == "google") {
        "google"
    } else if is("bing.com") {
        "bing"
    } else if is("duckduckgo.com") {
        "duckduckgo"
    } else if is("reddit.com") || is("redd.it") {
        "reddit"
    } else if is("news.ycombinator.com") {
        "hacker news"
    } else if is("t.co") || is("x.com") || is("twitter.com") {
        "x"
    } else if is("github.com") {
        "github"
    } else if is("chatgpt.com") || is("chat.openai.com") {
        "chatgpt"
    } else if is("claude.ai") {
        "claude"
    } else if is("perplexity.ai") {
        "perplexity"
    } else if is("alternativeto.net") {
        "alternativeto"
    } else {
        host
    };
    name.to_string()
}

/// The mark that tells one visitor from another on one day, and on no other.
pub fn visitor(secret: &str, ip: &str, agent: &str) -> String {
    crate::crypto::digest(&format!("{secret}\n{ip}\n{agent}"))[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_pages_are_counted_and_only_people() {
        assert!(counted("/") && counted("/price-drop-alert") && counted("/docs/") && counted("/mark") && counted("/signin"));
        assert!(!counted("/admin") && !counted("/watches") && !counted("/api/v1/watches") && !counted("/og.png") && !counted("/nope"));
        assert!(!is_bot("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15"));
        assert!(is_bot("Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)"));
        assert!(is_bot("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/130.0 Safari/537.36"));
        assert!(is_bot("curl/8.7.1") && is_bot(""));
    }

    #[test]
    fn a_view_says_where_it_came_from() {
        assert_eq!(source("https://www.google.de/", "", "zetlyn.com"), "google");
        assert_eq!(source("https://out.reddit.com/t3_x", "", "zetlyn.com"), "reddit");
        assert_eq!(source("https://news.ycombinator.com/item?id=1", "", "zetlyn.com"), "hacker news");
        assert_eq!(source("https://zetlyn.com/use-cases", "", "zetlyn.com"), INSIDE);
        assert_eq!(source("", "", "zetlyn.com"), "direct");
        assert_eq!(source("https://blog.example.org/post", "", "zetlyn.com"), "blog.example.org");
        assert_eq!(source("https://www.google.com/", "ref=Selfh.st", "zetlyn.com"), "selfh.st", "a tagged link says it best");
    }

    #[test]
    fn a_visitor_is_told_apart_only_within_a_day() {
        let a = visitor("monday", "203.0.113.7", "Mozilla/5.0");
        assert_eq!(a, visitor("monday", "203.0.113.7", "Mozilla/5.0"));
        assert_ne!(a, visitor("tuesday", "203.0.113.7", "Mozilla/5.0"));
        assert_ne!(a, visitor("monday", "203.0.113.8", "Mozilla/5.0"));
        assert!(!a.contains("203"));
    }
}
