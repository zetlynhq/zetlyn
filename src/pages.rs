//! The pages that answer what people search for: use cases, how to connect an assistant, and
//! how Zetlyn compares. Each is a file in `site/pages/`: a `title:` and a `description:` line, a
//! `---`, then the page in the markdown `docs.rs` reads.

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

pub struct Page {
    pub path: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub body: &'static str,
}

fn parse(path: &'static str, source: &'static str) -> Page {
    let (head, body) = source.split_once("\n---\n").unwrap_or(("", source));
    let field = |name: &str| head.lines().find_map(|l| l.strip_prefix(name)).map(str::trim).unwrap_or("");
    Page { path, title: field("title:"), description: field("description:"), body }
}

/// The page at a path, if there is one.
pub fn find(path: &str) -> Option<Page> {
    SOURCES.iter().find(|(p, _)| *p == path).map(|(p, s)| parse(p, s))
}

/// Every page's path, for the sitemap.
pub fn paths() -> Vec<&'static str> {
    SOURCES.iter().map(|(p, _)| *p).collect()
}

/// A page as HTML, with the field to start watching under it.
pub fn html(page: &Page) -> String {
    format!(
        "<article class=\"docs page\">{}<div class=start><form class=field action=/mark><input name=url placeholder=\"https://example.com/pricing\" aria-label=\"The address of the page to watch\" required><button class=primary>Watch it</button></form><p class=dim>{}</p></div></article>",
        crate::docs::html(page.body),
        crate::plan::HERO_NOTE
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_has_a_title_a_description_and_a_heading() {
        for path in paths() {
            let page = find(path).unwrap();
            assert!(!page.title.is_empty() && !page.description.is_empty(), "{path}");
            assert!(page.description.chars().count() <= 200, "{path}: a description a search result shows whole");
            let html = html(&page);
            assert_eq!(html.matches("<h1>").count(), 1, "{path}");
            assert!(!html.contains("](") && !html.contains("*\""), "{path}: no markup left as text");
        }
        assert!(find("/nope").is_none());
        let p = find("/price-drop-alert").unwrap();
        assert!(html(&p).contains("<a href=\"/back-in-stock-alert\">Back in stock alerts</a>"));
    }
}
