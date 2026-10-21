//! The pages that answer what people search for: use cases, how to connect an assistant, and
//! how Zetlyn compares. Each is a file in `site/pages/`: lines of `name: value`, a `---`, then the
//! page in the markdown `docs.rs` reads.
//!
//! The fields: `title` and `description` for the head and search results; `name` and `card` for
//! the card that links to the page; `tile`, the small picture on that card; `demo` and `kicker`,
//! the picture and the line above the title at the top of the page; `layout: compare` for a page
//! whose table sets Zetlyn beside another service. A line of its own starting
//! with `::` places a part that is not text: `::cards /a /b` the cards of those pages,
//! `::channels` where a message can go.

use crate::serve::esc;

/// The pages, by the address they are found at.
const SOURCES: &[(&str, &str)] = &[
    ("/use-cases", include_str!("../site/pages/use-cases.md")),
    ("/price-drop-alert", include_str!("../site/pages/price-drop-alert.md")),
    ("/back-in-stock-alert", include_str!("../site/pages/back-in-stock-alert.md")),
    ("/release-monitor", include_str!("../site/pages/release-monitor.md")),
    ("/website-change-monitor", include_str!("../site/pages/website-change-monitor.md")),
    ("/monitor-json-api", include_str!("../site/pages/monitor-json-api.md")),
    ("/rss-feed-monitor", include_str!("../site/pages/rss-feed-monitor.md")),
    ("/docs/claude", include_str!("../site/pages/docs-claude.md")),
    ("/docs/chatgpt", include_str!("../site/pages/docs-chatgpt.md")),
    ("/docs/cursor", include_str!("../site/pages/docs-cursor.md")),
    ("/alternatives/visualping", include_str!("../site/pages/alternatives-visualping.md")),
    ("/alternatives/changedetection-io", include_str!("../site/pages/alternatives-changedetection-io.md")),
    ("/alternatives/distill", include_str!("../site/pages/alternatives-distill.md")),
];

/// The pictures, by the name a page gives in `tile:` or `demo:`. Each shows a change happening,
/// in a loop that stands still for whoever asked their system for less motion.
fn art(name: &str) -> Option<&'static str> {
    Some(match name {
        "price" => include_str!("../site/pages/art/price.html"),
        "stock" => include_str!("../site/pages/art/stock.html"),
        "release" => include_str!("../site/pages/art/release.html"),
        "change" => include_str!("../site/pages/art/change.html"),
        "json" => include_str!("../site/pages/art/json.html"),
        "feed" => include_str!("../site/pages/art/feed.html"),
        "chat-claude" => include_str!("../site/pages/art/chat-claude.html"),
        "chat-chatgpt" => include_str!("../site/pages/art/chat-chatgpt.html"),
        "chat-cursor" => include_str!("../site/pages/art/chat-cursor.html"),
        "tile-price" => include_str!("../site/pages/art/tile-price.html"),
        "tile-stock" => include_str!("../site/pages/art/tile-stock.html"),
        "tile-release" => include_str!("../site/pages/art/tile-release.html"),
        "tile-change" => include_str!("../site/pages/art/tile-change.html"),
        "tile-json" => include_str!("../site/pages/art/tile-json.html"),
        "tile-feed" => include_str!("../site/pages/art/tile-feed.html"),
        _ => return None,
    })
}

pub struct Page {
    pub path: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub body: &'static str,
    head: &'static str,
}

impl Page {
    fn field(&self, name: &str) -> Option<&'static str> {
        self.head.lines().find_map(|l| l.strip_prefix(name)?.strip_prefix(':')).map(str::trim).filter(|v| !v.is_empty())
    }
}

fn parse(path: &'static str, source: &'static str) -> Page {
    let (head, body) = source.split_once("\n---\n").unwrap_or(("", source));
    let mut page = Page { path, title: "", description: "", body, head };
    page.title = page.field("title").unwrap_or("");
    page.description = page.field("description").unwrap_or("");
    page
}

/// The page at a path, if there is one.
pub fn find(path: &str) -> Option<Page> {
    SOURCES.iter().find(|(p, _)| *p == path).map(|(p, s)| parse(p, s))
}

/// Every page's path, for the sitemap.
pub fn paths() -> Vec<&'static str> {
    SOURCES.iter().map(|(p, _)| *p).collect()
}

/// A page as HTML: its top with the field to start watching (beside its picture, if it has one),
/// its text with the parts placed in it, and the field again at the end.
pub fn html(page: &Page) -> String {
    let body = page.body.trim_start();
    let (title, rest) = match body.strip_prefix("# ") {
        Some(b) => b.split_once('\n').unwrap_or((b, "")),
        None => (page.title, body),
    };
    let rest = rest.trim_start_matches('\n');
    let (lead, rest) = match rest.split_once("\n\n") {
        Some((lead, rest)) if !lead.starts_with(['#', '-', '|', ':', ' ']) && !lead.ends_with(':') => (lead, rest),
        _ => ("", rest),
    };
    let lead = if lead.is_empty() { String::new() } else { format!("<p class=lead>{}</p>", crate::docs::html(lead).trim_start_matches("<p>").trim_end_matches("</p>")) };
    let kicker = page.field("kicker").map(|k| format!("<p class=overline>{}</p>", esc(k))).unwrap_or_default();
    let top = match page.field("demo").and_then(art) {
        Some(demo) => format!("<section class=\"hero uc-hero\"><div>{kicker}<h1>{}</h1>{lead}{}</div>{demo}</section>", esc(title), start()),
        None => format!("<section class=uc-top>{kicker}<h1>{}</h1>{lead}</section>", esc(title)),
    };
    format!(
        "{top}<article class=\"docs page uc-body{}\">{}</article><section class=uc-cta><h2>Watch your first page.<br><span>{}</span></h2>{}</section>",
        page.field("layout").map(|l| format!(" uc-{l}")).unwrap_or_default(),
        parts(rest),
        crate::plan::HERO_NOTE,
        start()
    )
}

/// The field to paste an address into, which goes on to marking the page.
fn start() -> &'static str {
    "<form class=field action=/mark><input name=url placeholder=\"https://example.com/pricing\" aria-label=\"The address of the page to watch\" required><button class=primary>Watch it</button></form>"
}

/// The text, with each `::` line made the part it names.
fn parts(md: &str) -> String {
    let mut out = String::new();
    let mut text = Vec::new();
    for line in md.lines() {
        let Some(part) = line.strip_prefix("::") else {
            text.push(line);
            continue;
        };
        out.push_str(&crate::docs::html(&text.join("\n")));
        text.clear();
        let mut words = part.split_whitespace();
        match words.next() {
            Some("cards") => out.push_str(&cards(&words.collect::<Vec<_>>())),
            Some("channels") => out.push_str(CHANNELS),
            _ => {}
        }
    }
    out.push_str(&crate::docs::html(&text.join("\n")));
    out
}

/// Links to other pages as cards: their picture, if they have one, their name and a line.
fn cards(paths: &[&str]) -> String {
    let cards: String = paths
        .iter()
        .filter_map(|p| find(p))
        .map(|p| {
            let tile = p.field("tile").and_then(art).unwrap_or("");
            format!(
                "<a class=\"uc-card{}\" href=\"{}\">{tile}<h3>{}</h3><p>{}</p></a>",
                if tile.is_empty() { "" } else { " has-tile" },
                p.path,
                esc(p.field("name").unwrap_or(p.title)),
                esc(p.field("card").unwrap_or(p.description))
            )
        })
        .collect();
    format!("<div class=uc-cards>{cards}</div>")
}

/// Where a message can go, and what one looks like.
const CHANNELS: &str = r#"<div class=uc-channels><ul><li>Email</li><li>Telegram</li><li>Slack</li><li>Discord</li><li>Webhook</li></ul><div class=uc-message><p class=uc-from>Zetlyn</p><p><b>Below your mark:</b> shop.example/oled-55</p><p><s>449,00 €</s> → <b class=signal>389,00 €</b></p></div></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_has_a_title_a_description_and_a_heading() {
        for path in paths() {
            let page = find(path).unwrap();
            assert!(!page.title.is_empty() && !page.description.is_empty(), "{path}");
            assert!(page.description.chars().count() <= 200, "{path}: a description a search result shows whole");
            for name in ["demo", "tile"] {
                if let Some(n) = page.field(name) {
                    assert!(art(n).is_some(), "{path}: no picture {n}");
                }
            }
            let html = html(&page);
            assert_eq!(html.matches("<h1>").count(), 1, "{path}");
            assert!(!html.contains("](") && !html.contains("*\"") && !html.contains("<p>::"), "{path}: no markup left as text");
            assert!(!html.contains(":</p><form") && !html.contains(":</p></section>"), "{path}: the lead does not point at what comes after it");
        }
        assert!(find("/nope").is_none());
        let p = find("/price-drop-alert").unwrap();
        let h = html(&p);
        assert!(h.contains("class=\"hero uc-hero\""), "the page with a picture starts beside it");
        assert!(h.contains("<a class=\"uc-card has-tile\" href=\"/back-in-stock-alert\">"), "{h}");
        assert_eq!(h.matches("action=/mark").count(), 2, "the field at the top and at the end");
    }
}
