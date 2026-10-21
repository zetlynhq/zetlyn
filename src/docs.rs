//! `/docs`, `/llms-full.txt` and `/openapi.json`: how programs and LLMs use Zetlyn, made from one
//! source, `site/llms.txt`, so that what a person reads and what a model reads cannot part.
//!
//! The markdown here is the little that file and `site/pages` use: headings, paragraphs, lists,
//! tables, indented code, `code`, **bold**, *italic*, links and addresses; anything else is shown
//! as the text it is.

use crate::serve::esc;

const LLMS: &str = include_str!("../site/llms.txt");
const OPENAPI: &str = include_str!("../site/openapi.json");

/// The OpenAPI description, with this instance's address in it.
pub fn openapi(base: &str) -> String {
    OPENAPI.replace("{{BASE}}", base)
}

/// The guide, about this instance: another one's address in place of zetlyn.com's.
pub fn guide(base: &str) -> String {
    if base == "https://zetlyn.com" { LLMS.to_string() } else { LLMS.replace("https://zetlyn.com", base) }
}

/// Everything in one file for a model's context: the guide, the MCP tools as the server lists
/// them, and the OpenAPI description.
pub fn llms_full(base: &str) -> String {
    let tools = serde_json::to_string_pretty(&crate::api::tools()).unwrap_or_default();
    format!("{}\n## MCP tools, as the server lists them\n\n```json\n{tools}\n```\n\n## OpenAPI\n\n```json\n{}\n```\n", guide(base), openapi(base))
}

/// The guide as a page.
pub fn page(base: &str) -> String {
    format!("<article class=docs>{}<p class=dim>The same as text for a model: <a href=\"/llms.txt\">llms.txt</a>, everything in one file: <a href=\"/llms-full.txt\">llms-full.txt</a>, the API described: <a href=\"/openapi.json\">openapi.json</a>.</p></article>", html(&guide(base)))
}

/// The markdown of `site/llms.txt` as HTML.
pub fn html(md: &str) -> String {
    let lines: Vec<&str> = md.lines().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        if l.trim().is_empty() {
            i += 1;
        } else if let Some(h) = l.strip_prefix("# ") {
            out.push_str(&format!("<h1>{}</h1>", inline(h)));
            i += 1;
        } else if let Some(h) = l.strip_prefix("## ") {
            out.push_str(&format!("<h2 id=\"{}\">{}</h2>", slug(h), inline(h)));
            i += 1;
        } else if l.starts_with("> ") {
            let mut quote = Vec::new();
            while i < lines.len() && lines[i].starts_with('>') {
                quote.push(lines[i].trim_start_matches('>').trim());
                i += 1;
            }
            out.push_str(&format!("<p class=lead>{}</p>", inline(&quote.join(" "))));
        } else if l.starts_with("```") {
            i += 1;
            let mut code = Vec::new();
            while i < lines.len() && !lines[i].starts_with("```") {
                code.push(lines[i]);
                i += 1;
            }
            i += 1;
            out.push_str(&format!("<pre><code>{}</code></pre>", esc(&code.join("\n"))));
        } else if l.starts_with("    ") {
            let mut code = Vec::new();
            while i < lines.len() && (lines[i].starts_with("    ") || (lines[i].trim().is_empty() && lines.get(i + 1).is_some_and(|n| n.starts_with("    ")))) {
                code.push(lines[i].get(4..).unwrap_or(""));
                i += 1;
            }
            out.push_str(&format!("<pre><code>{}</code></pre>", esc(&code.join("\n"))));
        } else if l.starts_with('|') {
            let mut rows = Vec::new();
            while i < lines.len() && lines[i].starts_with('|') {
                rows.push(lines[i]);
                i += 1;
            }
            let cells = |r: &str| r.trim().trim_matches('|').split(" | ").map(|c| c.trim().to_string()).collect::<Vec<_>>();
            let mut t = String::from("<div class=scroll><table>");
            for (n, r) in rows.iter().enumerate() {
                if n == 1 && r.contains("---") {
                    continue;
                }
                let tag = if n == 0 { "th" } else { "td" };
                t.push_str("<tr>");
                for c in cells(r) {
                    t.push_str(&format!("<{tag}>{}</{tag}>", inline(&c)));
                }
                t.push_str("</tr>");
            }
            out.push_str(&t);
            out.push_str("</table></div>");
        } else if l.starts_with("- ") {
            out.push_str("<ul>");
            while i < lines.len() && lines[i].starts_with("- ") {
                let mut item = vec![&lines[i][2..]];
                i += 1;
                while i < lines.len() && lines[i].starts_with("  ") && !lines[i].trim().is_empty() {
                    item.push(lines[i].trim());
                    i += 1;
                }
                out.push_str(&format!("<li>{}</li>", inline(&item.join(" "))));
            }
            out.push_str("</ul>");
        } else if numbered(l).is_some() {
            out.push_str("<ol>");
            while let Some(first) = lines.get(i).and_then(|l| numbered(l)) {
                let mut item = vec![first];
                i += 1;
                while i < lines.len() && lines[i].starts_with("  ") && !lines[i].trim().is_empty() {
                    item.push(lines[i].trim());
                    i += 1;
                }
                out.push_str(&format!("<li>{}</li>", inline(&item.join(" "))));
            }
            out.push_str("</ol>");
        } else {
            let mut para = Vec::new();
            while i < lines.len() && !lines[i].trim().is_empty() && numbered(lines[i]).is_none() && !["# ", "## ", "- ", "|", "    ", "```", "> "].iter().any(|p| lines[i].starts_with(p)) {
                para.push(lines[i].trim());
                i += 1;
            }
            out.push_str(&format!("<p>{}</p>", inline(&para.join(" "))));
        }
    }
    out
}

fn slug(s: &str) -> String {
    s.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect::<String>().split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
}

/// The text of a numbered list's item ("1. Paste the address"), if the line is one.
fn numbered(l: &str) -> Option<&str> {
    let (n, rest) = l.split_once(". ")?;
    (!n.is_empty() && n.len() <= 2 && n.bytes().all(|b| b.is_ascii_digit())).then_some(rest)
}

/// `code`, **bold**, *italic* and links in a line of text; the rest escaped.
fn inline(s: &str) -> String {
    let mut out = String::new();
    for (n, part) in s.split('`').enumerate() {
        if n % 2 == 1 {
            out.push_str(&format!("<code>{}</code>", esc(part)));
            continue;
        }
        let mut text = String::new();
        for (m, piece) in part.split("**").enumerate() {
            let linked: String = piece.split('*').enumerate().map(|(k, p)| if k % 2 == 1 { format!("<em>{}</em>", links(&esc(p))) } else { links(&esc(p)) }).collect();
            text.push_str(&if m % 2 == 1 { format!("<b>{linked}</b>") } else { linked });
        }
        out.push_str(&text);
    }
    out
}

/// Written links (`[text](/path)`) and bare addresses in escaped text made links: to zetlyn.com
/// and its own paths in the same tab, elsewhere in a new one.
fn links(s: &str) -> String {
    let written = regex::Regex::new(r"\[([^\]]+)\]\(([^)\s]+)\)").expect("a fixed pattern");
    let mut out = String::new();
    let mut last = 0;
    for m in written.captures_iter(s) {
        let whole = m.get(0).expect("the match");
        out.push_str(&bare_links(&s[last..whole.start()]));
        let (text, href) = (&m[1], &m[2]);
        let own = href.starts_with('/') || href.starts_with("https://zetlyn.com");
        out.push_str(&format!("<a href=\"{href}\"{}>{text}</a>", if own { "" } else { " target=_blank rel=noopener" }));
        last = whole.end();
    }
    out.push_str(&bare_links(&s[last..]));
    out
}

/// Bare addresses in escaped text made links.
fn bare_links(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(at) = rest.find("https://") {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let end = tail.find(|c: char| c.is_whitespace() || c == ')' || c == '<').unwrap_or(tail.len());
        let url = tail[..end].trim_end_matches(['.', ',', ';', ':']);
        let own = url.starts_with("https://zetlyn.com");
        let shown = if own { url.trim_start_matches("https://zetlyn.com") } else { url };
        let shown = if shown.is_empty() { "zetlyn.com" } else { shown };
        out.push_str(&format!("<a href=\"{url}\"{}>{}</a>", if own { "" } else { " target=_blank rel=noopener" }, if own && shown.starts_with('/') { url } else { shown }));
        rest = &tail[url.len()..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guide_reads_as_a_page() {
        let h = html("# Zetlyn\n\n> One\n> two\n\n## How to connect\n\nSee `x` at https://zetlyn.com/mcp, **now**.\n\n- a\n  b\n- c\n\n| A | B |\n|---|---|\n| `x` | y |\n\n    code <b>\n");
        assert!(h.contains("<h1>Zetlyn</h1><p class=lead>One two</p><h2 id=\"how-to-connect\">How to connect</h2>"), "{h}");
        assert!(h.contains("<code>x</code> at <a href=\"https://zetlyn.com/mcp\">https://zetlyn.com/mcp</a>, <b>now</b>."), "{h}");
        assert!(h.contains("<ul><li>a b</li><li>c</li></ul>"));
        assert_eq!(html("Do this:\n1. Paste\n2. Click\n   it\n"), "<p>Do this:</p><ol><li>Paste</li><li>Click it</li></ol>");
        assert!(h.contains("<tr><th>A</th><th>B</th></tr><tr><td><code>x</code></td><td>y</td></tr>"));
        assert!(h.contains("<pre><code>code &lt;b&gt;</code></pre>"));
        assert!(links("see https://stripe.com/x.").contains("target=_blank"));
        assert_eq!(inline("ask: *\"Tell me\"* or **so**"), "ask: <em>&quot;Tell me&quot;</em> or <b>so</b>");
    }

    #[test]
    fn the_openapi_description_is_json_with_this_instances_address() {
        let j: serde_json::Value = serde_json::from_str(&openapi("https://zetlyn.test")).expect("valid JSON");
        assert_eq!(j["servers"][0]["url"], "https://zetlyn.test/api/v1");
        assert!(j["paths"]["/watches/{id}"]["patch"].is_object());
        assert!(llms_full("https://zetlyn.test").contains("\"name\": \"read_place\""));
    }
}
