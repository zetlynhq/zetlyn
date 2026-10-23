//! A page shown to be marked: somebody else's page in a frame of its own, with one script added.
//!
//! Its scripts are taken out, and the frame runs only the marking script (a sandbox with no origin,
//! scripts by nonce, no forms). What the frame says to the app goes by message, and the app takes
//! only a description of the place that was clicked. The same page without scripts is what is
//! read on every check, so the element counted in the frame is the element counted on the server.
//! (After the old Zetlyn's `picker.rs`.)

/// The page as it is read: no scripts, frames or redirects of its own, and what it shows to a
/// reader without scripts (`<noscript>`) shown.
pub fn sanitized(body: &str) -> String {
    let mut page = body.to_string();
    for p in [
        r"(?is)<script\b.*?</script\s*>",
        r"(?is)<iframe\b.*?</iframe\s*>",
        r"(?is)<object\b.*?</object\s*>",
        r"(?is)<embed\b[^>]*>",
        r"(?is)<base\b[^>]*>",
        r"(?is)<meta\b[^>]*http-equiv[^>]*>",
        r"(?is)</?noscript\b[^>]*>",
    ] {
        page = strip(&page, p, "");
    }
    page = strip(&page, r#"(?is)\son[a-z]+\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)"#, "");
    page = strip(&page, r#"(?i)\scrossorigin(\s*=\s*("[^"]*"|'[^']*'|[^\s>]+))?"#, "");
    strip(&page, r#"(?i)(href|src|action)\s*=\s*(["'])\s*javascript:[^"']*(["'])"#, "$1=$2#$3")
}

/// The page in the frame: sanitized, what it loads (images, stylesheets, fonts) loaded through
/// this app at `origin` (`through`), the marks' style in its head and the marking script at the
/// end of its body. None of it changes how the page's own elements are counted: a `style` is
/// counted only among styles, a `script` among scripts, and attributes not at all.
pub fn framed(body: &str, url: &str, nonce: &str, origin: &str) -> String {
    // Script preloads are blocked by the frame's policy anyway; out, they do not fill the console.
    let page = strip(&sanitized(body), r#"(?i)<link\b[^>]*\b(modulepreload|as\s*=\s*["']?script)\b[^>]*>"#, "");
    let mut page = through(&page, url, origin);
    let head = format!(r#"<base href="{}"><style>{MARKS}</style>"#, attr(url));
    let script = format!(r#"<script nonce="{nonce}">{FRAME_SCRIPT}</script>"#);
    match regex::Regex::new(r"(?i)<head\b[^>]*>").ok().and_then(|re| re.find(&page).map(|m| m.end())) {
        Some(at) => page.insert_str(at, &head),
        None => page.insert_str(0, &head),
    }
    match page.to_lowercase().rfind("</body") {
        Some(at) => page.insert_str(at, &script),
        None => page.push_str(&script),
    }
    page
}

/// The policy on the frame's answer: the marking script runs, and what the page loads comes from
/// this app or nowhere, so whatever `through` did not catch does not reach the page's site.
pub fn frame_policy(nonce: &str, origin: &str) -> String {
    format!("sandbox allow-scripts; default-src 'none'; script-src 'nonce-{nonce}'; style-src {origin} 'unsafe-inline'; img-src {origin} data:; font-src {origin} data:; object-src 'none'; form-action 'none'; frame-ancestors 'self'")
}

/// The page with what it loads loaded through this app: `src`, `srcset` and `poster` on any
/// element, `href` on a `link`, `url()` and `@import` in its styles. The site sees this app ask,
/// not the person marking. Links (`a href`) stay as they are; nothing in the frame follows them.
fn through(page: &str, base: &str, origin: &str) -> String {
    let tag = regex::Regex::new(r"(?is)<([a-z][a-z0-9-]*)\b[^>]*>").unwrap();
    let attribute = regex::Regex::new(r#"(?is)(\s)(src|srcset|poster|href|style)(\s*=\s*)("[^"]*"|'[^']*'|[^\s>"']+)"#).unwrap();
    let page = tag.replace_all(page, |t: &regex::Captures| {
        let name = t[1].to_ascii_lowercase();
        attribute
            .replace_all(&t[0], |m: &regex::Captures| {
                let raw = unentity(m[4].trim_matches(|c| c == '"' || c == '\''));
                let new = match m[2].to_ascii_lowercase().as_str() {
                    "href" if name != "link" => return m[0].to_string(),
                    "srcset" => srcset(&raw, base, origin),
                    "style" => css_through(&raw, base, origin),
                    _ => via_page(&raw, base, origin),
                };
                format!("{}{}{}\"{}\"", &m[1], &m[2], &m[3], attr(&new))
            })
            .into_owned()
    });
    let style = regex::Regex::new(r"(?is)(<style\b[^>]*>)(.*?)(</style\s*>)").unwrap();
    style.replace_all(&page, |s: &regex::Captures| format!("{}{}{}", &s[1], css_through(&s[2], base, origin), &s[3])).into_owned()
}

/// A stylesheet with what it loads loaded through this app; `base` is where the stylesheet is.
pub fn css_through(css: &str, base: &str, origin: &str) -> String {
    let url = regex::Regex::new(r#"(?i)url\(\s*(?:"([^"]*)"|'([^']*)'|([^)'"\s]*))\s*\)"#).unwrap();
    let css = url.replace_all(css, |c: &regex::Captures| {
        let u = c.get(1).or(c.get(2)).or(c.get(3)).map_or("", |m| m.as_str());
        format!("url(\"{}\")", via_page(u, base, origin))
    });
    let import = regex::Regex::new(r#"(?i)@import\s+(?:"([^"]*)"|'([^']*)')"#).unwrap();
    import.replace_all(&css, |c: &regex::Captures| format!("@import url(\"{}\")", via_page(c.get(1).or(c.get(2)).map_or("", |m| m.as_str()), base, origin))).into_owned()
}

fn srcset(set: &str, base: &str, origin: &str) -> String {
    let mut out = Vec::new();
    let mut rest = set.trim();
    while !rest.is_empty() {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let (mut url, after) = rest.split_at(end);
        let mut descriptor = "";
        rest = after;
        if let Some(u) = url.strip_suffix(',') {
            url = u;
        } else {
            let d = rest.find(',').unwrap_or(rest.len());
            descriptor = rest[..d].trim();
            rest = rest[d..].strip_prefix(',').unwrap_or(&rest[d..]);
        }
        rest = rest.trim_start();
        out.push(format!("{} {descriptor}", via_page(url, base, origin)).trim_end().to_string());
    }
    out.join(", ")
}

/// One address the page loads, read against `base`, as this app's address for it.
fn via_page(u: &str, base: &str, origin: &str) -> String {
    let u = u.trim();
    if u.is_empty() || u.starts_with('#') || u.starts_with("data:") {
        return u.to_string();
    }
    let at = crate::fetch::join(base, u);
    if at.starts_with("https://") || at.starts_with("http://") { via(origin, &at) } else { String::new() }
}

/// This app's address for loading `target`: signed, so that it loads only what a frame asked for
/// and is nobody's open proxy.
pub fn via(origin: &str, target: &str) -> String {
    let u: String = form_urlencoded::byte_serialize(target.as_bytes()).collect();
    format!("{origin}/r?u={u}&s={}", signature(target))
}

/// Whether `s` is this app's signature of `target`.
pub fn signed(target: &str, s: &str) -> bool {
    crate::crypto::same(&signature(target), s)
}

/// The key lives as long as the process: a frame open across a restart loses its images, no more.
fn signature(target: &str) -> String {
    static KEY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let key = KEY.get_or_init(crate::crypto::token);
    crate::crypto::hex(&crate::crypto::hmac_sha256(key.as_bytes(), target.as_bytes()))[..32].to_string()
}

fn unentity(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&#39;", "'").replace("&#x27;", "'").replace("&amp;", "&")
}

fn strip(s: &str, pattern: &str, with: &str) -> String {
    regex::Regex::new(pattern).map(|re| re.replace_all(s, with).into_owned()).unwrap_or_else(|_| s.to_string())
}

fn attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;")
}

/// The marks, and a page shown although it waits for its own scripts to show itself (`body {
/// opacity: 0 }` until hydrated): they never run in the frame.
const MARKS: &str = "html,body{opacity:1!important;visibility:visible!important}.zt-removing *{cursor:crosshair!important}.zt-hover{outline:2px dashed #dc4a20!important;outline-offset:2px!important;cursor:pointer!important}.zt-picked{outline:3px solid #dc4a20!important;outline-offset:2px!important;background:rgba(220,74,32,.08)!important}";

/// The marking script. `labelOf` is `anchor::label_of`, line for line.
const FRAME_SCRIPT: &str = r#"(() => {
  let current = null;
  const norm = s => (s || '').replace(/\s+/g, ' ').trim();
  const textOf = n => n.nodeType === 8 ? '' : n.textContent;
  const labelOf = el => {
    let node = el;
    for (let up = 0; up < 3 && node; up++) {
      const tag = node.tagName ? node.tagName.toLowerCase() : '';
      if (tag === 'body' || tag === 'html') break;
      for (let s = node.previousSibling; s; s = s.previousSibling) {
        if (s.nodeType === 8) continue;
        const t = norm(textOf(s));
        if (t) { if (t.length <= 80) return t; break; }
      }
      node = node.parentElement;
    }
    return '';
  };
  const pathOf = el => {
    const parts = [];
    for (let n = el; n && n.nodeType === 1 && n.tagName !== 'HTML'; n = n.parentElement) {
      let i = 1;
      for (let s = n.previousElementSibling; s; s = s.previousElementSibling) if (s.tagName === n.tagName) i++;
      parts.unshift(n.tagName.toLowerCase() + ':nth-of-type(' + i + ')');
    }
    return 'html > ' + parts.join(' > ');
  };
  const describe = el => ({
    path: pathOf(el), tag: el.tagName.toLowerCase(), id: el.id || '',
    classes: [...el.classList].filter(c => !c.startsWith('zt-')),
    text: norm(el.textContent).slice(0, 500), label: labelOf(el)
  });
  // What sits over the page (a consent box, a popup, the dark layer under it) is hidden, never
  // taken out: taken out, it would change how the elements after it are counted.
  const consent = /cookie|consent|gdpr|einwillig|zustimm|akzeptier/i;
  const floats = el => { const p = getComputedStyle(el).position; return p === 'fixed' || p === 'sticky'; };
  const hide = el => el.style.setProperty('display', 'none', 'important');
  const clearWay = () => {
    if (!document.body) return 0;
    const area = innerWidth * innerHeight;
    let n = 0;
    for (const el of document.body.querySelectorAll('*')) {
      if (!floats(el) || el.style.display === 'none') continue;
      const r = el.getBoundingClientRect();
      if (r.width === 0 && r.height === 0) continue;
      if (r.width * r.height >= area * 0.3 || consent.test(el.textContent.slice(0, 3000))) { hide(el); n++; }
    }
    for (const el of [document.documentElement, document.body]) el.style.setProperty('overflow', 'auto', 'important');
    return n;
  };
  let removing = false;
  const remove = el => {
    for (let n = el; n && n !== document.body; n = n.parentElement) if (floats(n)) { el = n; break; }
    hide(el);
    removing = false;
    document.documentElement.classList.remove('zt-removing');
    parent.postMessage({ zetlyn: 'removed' }, '*');
  };
  // Larger walks up from the element clicked, Smaller back down the same way: on a phone a
  // finger hits what lies under it, rarely the very place meant.
  let trail = [];
  const larger = el => el && el.parentElement && el.parentElement.tagName !== 'BODY' ? el.parentElement : null;
  const pick = el => {
    if (current) current.classList.remove('zt-picked');
    current = el;
    el.classList.remove('zt-hover');
    el.classList.add('zt-picked');
    parent.postMessage({ zetlyn: 'picked', anchor: describe(el), larger: !!larger(el), smaller: trail.length > 0 }, '*');
  };
  // Hover marks only under a mouse; a tap would leave it behind.
  document.addEventListener('pointerover', e => e.pointerType === 'mouse' && e.target.classList && e.target.classList.add('zt-hover'));
  document.addEventListener('pointerout', e => e.target.classList && e.target.classList.remove('zt-hover'));
  document.addEventListener('click', e => { e.preventDefault(); e.stopPropagation(); if (e.target.nodeType !== 1) return; if (removing) remove(e.target); else { trail = []; pick(e.target); } }, true);
  window.addEventListener('message', e => {
    if (e.source !== parent) return;
    if (e.data === 'bigger' && larger(current)) { trail.push(current); pick(larger(current)); }
    if (e.data === 'smaller' && trail.length) pick(trail.pop());
    if (e.data === 'reveal' && current) current.scrollIntoView({ block: 'nearest' });
    if (e.data === 'clear' && current) { current.classList.remove('zt-picked'); current = null; trail = []; }
    if (e.data === 'remove') { removing = true; document.documentElement.classList.add('zt-removing'); }
  });
  parent.postMessage({ zetlyn: 'cleared', count: clearWay() }, '*');
})();"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_keeps_none_of_its_own_scripts() {
        let page = r#"<html><head><link rel="modulepreload" href="/a.js"><link as="script" rel="preload" href="/b.js"><link rel="preload" as="font" href="/f.woff"><base href="https://evil.example/"><meta http-equiv="refresh" content="0;url=https://x"></head><body onload="steal()"><a href="javascript:steal()" onclick='steal()'>x</a><script>steal()</script><iframe src="https://x"></iframe><noscript><p>shown</p></noscript></body></html>"#;
        let out = framed(page, "https://acme.example/pricing", "n0nce", "https://zetlyn.test");
        assert!(!out.contains("steal()"), "{out}");
        assert!(!out.contains("evil.example") && !out.contains("refresh") && !out.contains("<iframe"));
        assert!(out.contains("<p>shown</p>"));
        assert!(!out.contains("a.js") && !out.contains("b.js") && out.contains("f.woff"), "{out}");
        assert!(out.contains("html,body{opacity:1!important"));
        assert!(out.contains(r#"<base href="https://acme.example/pricing">"#));
        assert!(out.contains(r#"<script nonce="n0nce">"#));
    }

    #[test]
    fn what_the_page_loads_comes_through_this_app_signed() {
        let page = r#"<head><link rel=stylesheet href="/s.css"><style>@import 'a.css'; b{background:url(img/b.png)}</style></head><body><a href="/x">x</a><img src="p.jpg?a=1&amp;b=2" srcset="p1.jpg 1x, https://cdn.example/p,2.jpg 2x" style="background:url(&quot;/c.png&quot;)"><img src="data:image/png;base64,AA"></body>"#;
        let out = framed(page, "https://acme.example/shop/item", "n", "https://zetlyn.test");
        let target = "https://acme.example/shop/p.jpg?a=1&b=2";
        assert!(out.contains(&attr(&via("https://zetlyn.test", target))), "{out}");
        assert!(out.contains(&attr(&via("https://zetlyn.test", "https://acme.example/s.css"))));
        assert!(out.contains(&via("https://zetlyn.test", "https://acme.example/shop/a.css")));
        assert!(out.contains(&via("https://zetlyn.test", "https://acme.example/shop/img/b.png")));
        assert!(out.contains(&attr(&via("https://zetlyn.test", "https://cdn.example/p,2.jpg"))));
        assert!(out.contains(&attr(&via("https://zetlyn.test", "https://acme.example/c.png"))));
        assert!(out.contains(r#"<a href="/x">"#) && out.contains("data:image/png;base64,AA"));
        let s = via("https://zetlyn.test", target).rsplit_once("&s=").unwrap().1.to_string();
        assert!(signed(target, &s) && !signed("https://evil.example/", &s));
    }
}
