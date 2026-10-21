//! What a search engine or a link preview reads of a page besides the page: its description, the
//! one address it is known by, its card for a shared link (Open Graph), and on the home page what
//! Zetlyn is, costs and answers (JSON-LD). And the list of the pages there are to find
//! (`sitemap.xml`).

use serde_json::json;

use crate::serve::esc;

/// The pages there are for anybody to find: what the sitemap lists.
pub const PUBLIC: &[&str] = &["/", "/docs", "/terms", "/privacy", "/legal", "/withdrawal"];

/// What a page is about, in a sentence or two: the text under its title in a search result.
pub fn description(path: &str) -> &'static str {
    if let Some(page) = crate::pages::find(path) {
        return page.description;
    }
    match path {
        "/docs" => "How to use Zetlyn from Claude, ChatGPT and other MCP clients, and through its HTTP API: watching a place on a page or the whole page, conditions, JSON and feeds, webhooks.",
        "/terms" => "The terms for using Zetlyn: the free trial, the subscription, what Zetlyn reads and what it does not, cancelling.",
        "/privacy" => "What Zetlyn keeps about you and your watches, who processes it, how long it is kept, and how to have it deleted.",
        "/legal" => "Who runs Zetlyn, and how to reach them.",
        "/withdrawal" => "Your right to withdraw from a Zetlyn subscription, and how.",
        "/signin" => "Sign in to Zetlyn, or make an account, with an email link, Google or Apple.",
        _ => "Zetlyn watches a web page, or one place on it such as a price, stock or a version, and tells you when it changes: by email, Telegram, Slack, Discord or webhook. Also from Claude and ChatGPT. The first watch is free for three days.",
    }
}

/// A path as the one address it is known by: no trailing slash, no query.
pub fn canonical_path(path: &str) -> String {
    let p = path.split(['?', '#']).next().unwrap_or("/").trim_end_matches('/');
    if p.is_empty() { "/".into() } else { p.to_string() }
}

/// The tags for a page's head: its description, its canonical address, its card for a shared link,
/// and on the home page what Zetlyn is (JSON-LD).
pub fn head(base: &str, path: &str, title: &str, faq_html: &str) -> String {
    let path = canonical_path(path);
    let url = format!("{base}{}", if path == "/" { "" } else { path.as_str() });
    let url = if url.is_empty() { format!("{base}/") } else { url };
    let desc = description(&path);
    let image = format!("{base}/og.png");
    let mut out = format!(
        "<meta name=description content=\"{d}\"><link rel=canonical href=\"{u}\">\
         <meta property=og:type content=website><meta property=og:site_name content=Zetlyn><meta property=og:title content=\"{t}\"><meta property=og:description content=\"{d}\"><meta property=og:url content=\"{u}\"><meta property=og:image content=\"{i}\"><meta property=og:image:width content=1200><meta property=og:image:height content=630>\
         <meta name=twitter:card content=summary_large_image><meta name=twitter:title content=\"{t}\"><meta name=twitter:description content=\"{d}\"><meta name=twitter:image content=\"{i}\">",
        d = esc(desc),
        u = esc(&url),
        t = esc(title),
        i = esc(&image)
    );
    if path == "/" {
        out.push_str(&format!("<script type=\"application/ld+json\">{}</script>", home_data(base, faq_html).replace("</", "<\\/")));
    }
    out
}

/// What the home page says Zetlyn is, for search engines: the application and its price, the
/// organisation, the site, and the questions it answers.
fn home_data(base: &str, faq_html: &str) -> String {
    let questions: Vec<serde_json::Value> = faq(faq_html).into_iter().map(|(q, a)| json!({ "@type": "Question", "name": q, "acceptedAnswer": { "@type": "Answer", "text": a } })).collect();
    let mut graph = vec![
        json!({
            "@type": "SoftwareApplication",
            "name": "Zetlyn",
            "url": format!("{base}/"),
            "applicationCategory": "UtilitiesApplication",
            "operatingSystem": "Web, Linux, macOS",
            "description": description("/"),
            "offers": [
                { "@type": "Offer", "name": "Free trial", "price": "0", "priceCurrency": "EUR", "description": "One watch for three days." },
                { "@type": "Offer", "name": "Subscription", "price": "9.99", "priceCurrency": "EUR", "description": "Up to 10 watches, 0.50 € a month for each one more, VAT included.", "priceSpecification": { "@type": "UnitPriceSpecification", "price": "9.99", "priceCurrency": "EUR", "unitText": "MONTH" } }
            ],
            "license": "https://opensource.org/license/mit",
            "codeRepository": "https://github.com/zetlynhq/zetlyn"
        }),
        json!({ "@type": "Organization", "name": "Zetlyn", "url": format!("{base}/"), "logo": format!("{base}/mark.png"), "sameAs": ["https://github.com/zetlynhq/zetlyn"] }),
        json!({ "@type": "WebSite", "name": "Zetlyn", "url": format!("{base}/") }),
    ];
    if !questions.is_empty() {
        graph.push(json!({ "@type": "FAQPage", "mainEntity": questions }));
    }
    json!({ "@context": "https://schema.org", "@graph": graph }).to_string()
}

/// The questions and answers of the home page's FAQ, as text.
fn faq(html: &str) -> Vec<(String, String)> {
    let doc = scraper::Html::parse_fragment(html);
    let details = scraper::Selector::parse("details").expect("a fixed selector");
    let summary = scraper::Selector::parse("summary").expect("a fixed selector");
    let answer = scraper::Selector::parse("p").expect("a fixed selector");
    doc.select(&details)
        .filter_map(|d| {
            let q = crate::text::norm(&d.select(&summary).next()?.text().collect::<String>());
            let a = crate::text::norm(&d.select(&answer).map(|p| p.text().collect::<String>()).collect::<Vec<_>>().join(" "));
            (!q.is_empty() && !a.is_empty()).then_some((q, a))
        })
        .collect()
}

/// `sitemap.xml`: the pages there are to find.
pub fn sitemap(base: &str, more: &[&str]) -> String {
    let urls: String = PUBLIC.iter().chain(more.iter()).map(|p| format!("<url><loc>{}{}</loc></url>", esc(base), if *p == "/" { "/" } else { p })).collect();
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">{urls}</urlset>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_says_what_it_is_once_and_by_one_address() {
        assert_eq!(canonical_path("/docs/"), "/docs");
        assert_eq!(canonical_path("/?utm_source=x"), "/");
        let h = head("https://zetlyn.com", "/docs/", "Docs — Zetlyn", "");
        assert!(h.contains("<link rel=canonical href=\"https://zetlyn.com/docs\">"), "{h}");
        assert!(h.contains("og:image content=\"https://zetlyn.com/og.png\""));
        assert!(!h.contains("ld+json"), "only the home page carries the data");
        let faq_html = "<details><summary>Do I need an account?</summary><p>No, not one <b>you</b> make.</p></details>";
        let home = head("https://zetlyn.com", "/", "Zetlyn", faq_html);
        let data = home.split("ld+json\">").nth(1).unwrap().trim_end_matches("</script>");
        let j: serde_json::Value = serde_json::from_str(&data.replace("<\\/", "</")).expect("valid JSON-LD");
        assert_eq!(j["@graph"][3]["mainEntity"][0]["acceptedAnswer"]["text"], "No, not one you make.");
        assert_eq!(j["@graph"][0]["offers"][1]["price"], "9.99");
        assert!(sitemap("https://zetlyn.com", &["/x"]).contains("<loc>https://zetlyn.com/x</loc>"));
    }
}
