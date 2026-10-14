//! A list on a web page chosen by pointing at it: the page shown as it is, in a frame of its own,
//! an item clicked and every item like it marked, the parts of an item clicked to keep, and what
//! that reads shown before it is saved. What is saved is what `guess` writes for a list it found,
//! with the person's choice in place of the guess.
//!
//! The page is somebody else's and is shown here: its scripts are taken out, and the frame it is in
//! runs only the one script this module adds (a policy of its own on that answer: a sandbox with
//! no origin, scripts by nonce). It can neither reach the app nor send a form; what it says to the
//! app goes by message, and the app takes only what it asked for.

use std::collections::BTreeMap;

use maud::{html, Markup, PreEscaped};
use serde_json::{json, Value as J};

/// The page as it is shown in the frame: no script of its own, its addresses resolved against
/// where it is, and the picker's script and marks added.
pub fn framed(body: &str, url: &str, nonce: &str) -> String {
    let strip = |s: &str, pattern: &str, with: &str| regex::Regex::new(pattern).map(|re| re.replace_all(s, with).into_owned()).unwrap_or_else(|_| s.to_string());
    let mut page = body.to_string();
    for p in [r"(?is)<script\b.*?</script\s*>", r"(?is)<iframe\b.*?</iframe\s*>", r"(?is)<object\b.*?</object\s*>", r"(?is)<embed\b[^>]*>", r"(?is)<base\b[^>]*>", r"(?is)<meta\b[^>]*http-equiv[^>]*>"] {
        page = strip(&page, p, "");
    }
    // What a page shows without its scripts is often in a <noscript>: shown, as the reader of
    // the page without scripts sees it, and as Zetlyn reads it.
    page = strip(&page, r"(?is)</?noscript\b[^>]*>", "");
    page = strip(&page, r#"(?is)\son[a-z]+\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)"#, "");
    // Asked from a frame with no origin, a resource asked for with CORS is refused: asked plainly.
    page = strip(&page, r#"(?i)\scrossorigin(\s*=\s*("[^"]*"|'[^']*'|[^\s>]+))?"#, "");
    page = strip(&page, r#"(?i)(href|src|action)\s*=\s*(["'])\s*javascript:[^"']*(["'])"#, "$1=$2#$3");
    let base = format!(r#"<base href="{}"><style>{MARKS}</style>"#, attr(url));
    let script = format!(r#"<script nonce="{nonce}">{FRAME_SCRIPT}</script>"#);
    match regex::Regex::new(r"(?i)<head\b[^>]*>").ok().and_then(|re| re.find(&page).map(|m| m.end())) {
        Some(at) => page.insert_str(at, &base),
        None => page.insert_str(0, &base),
    }
    match page.to_lowercase().rfind("</body") {
        Some(at) => page.insert_str(at, &script),
        None => page.push_str(&script),
    }
    page
}

/// The policy of the frame's answer: a sandbox with no origin of its own, the picker's script
/// alone, nothing sent anywhere by a form.
pub fn frame_policy(nonce: &str) -> String {
    format!("sandbox allow-scripts; script-src 'nonce-{nonce}'; object-src 'none'; form-action 'none'; frame-ancestors 'self'")
}

fn attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;")
}

/// The rows a choice reads, the first few, and how many there are.
pub fn preview(body: &str, items: &str, fields: &BTreeMap<String, String>) -> Result<J, String> {
    let rows = crate::web::extract(body, items, fields)?;
    Ok(json!({ "count": rows.len(), "rows": rows.into_iter().take(8).collect::<Vec<_>>() }))
}

/// The lists Zetlyn found on the page itself, for the person to start from.
pub fn found(body: &str) -> J {
    J::Array(crate::web::candidates(body).into_iter().map(|c| json!({ "items": c.items, "count": c.count, "fields": c.fields.iter().map(|(n, s, _)| (n.clone(), J::String(s.clone()))).collect::<serde_json::Map<_, _>>() })).collect())
}

/// The page: the steps beside the frame. `now` is what the source reads today, to start from.
pub fn page(title: &str, url: &str, frame: &str, base: &str, jobs: &str, now: &J, found: &J) -> Markup {
    let start = json!({ "base": base, "jobs": jobs, "now": now, "found": found });
    html! {
        div.picker {
            header.picker-head {
                div {
                    p.overline { "Choose on the page" }
                    h1 { (title) }
                    p.dim { (url) }
                }
            }
            div.picker-body {
                aside.picker-steps {
                    section {
                        h2 { span.step { "1" } "The list" }
                        p.dim #pick-items-say { "Click one item of the list on the page: a product, a post, a row. Every item like it is marked." }
                        p.picker-found #pick-found {}
                    }
                    section {
                        h2 { span.step { "2" } "What to keep of each" }
                        p.dim { "Click a part of an item: its name, a price, a date, its link. It is marked in every item." }
                        table.picker-fields { thead { tr { th { "Name" } th { "Is" } th title="What tells one item from another" { "Names it" } th { "Title" } th {} } } tbody #pick-fields {} }
                    }
                    section {
                        h2 { span.step { "3" } "What Zetlyn reads" }
                        div #pick-preview { p.dim { "Nothing chosen yet." } }
                        div #pick-error .note hidden {}
                        p.picker-actions {
                            button.primary #pick-save type="button" disabled { "Save and read it" }
                            a.button href=(base) { "Cancel" }
                        }
                        p.dim { "Saving rewrites what this source reads on the page. What it read before stays in its history." }
                    }
                }
                div.picker-frame {
                    iframe #pick-frame src=(frame) sandbox="allow-scripts" referrerpolicy="no-referrer" title="The page" {}
                }
            }
        }
        script type="application/json" #pick-start { (PreEscaped(start.to_string().replace("</", "<\\/"))) }
        script { (PreEscaped(PAGE_SCRIPT)) }
    }
}

/// The marks in the frame: what the pointer is over, the items, a field in each.
const MARKS: &str = r#"
.zp-hover { outline: 2px dashed #e0512a !important; outline-offset: 1px !important; cursor: crosshair !important; }
.zp-item { outline: 2px solid #e0512a !important; outline-offset: 2px !important; }
.zp-field { background: rgba(255, 196, 0, .38) !important; box-shadow: 0 0 0 2px rgba(224, 81, 42, .55) !important; }
* { cursor: crosshair !important; }
html, body { opacity: 1 !important; visibility: visible !important; }
"#;

/// In the frame: what is pointed at, clicked and marked, said to the app by message.
const FRAME_SCRIPT: &str = r#"
(function () {
  var items = null, specs = [], over = null;
  // The page's own first class: never one of the marks this script adds.
  function first(e) { if (!e.classList) return null; for (var i = 0; i < e.classList.length; i++) { if (e.classList[i].indexOf('zp-') !== 0) return e.classList[i]; } return null; }
  function esc(s) { return window.CSS && CSS.escape ? CSS.escape(s) : s.replace(/[^a-zA-Z0-9_-]/g, '\\$&'); }
  function simple(e) { var c = first(e); return e.tagName.toLowerCase() + (c ? '.' + esc(c) : ''); }
  function path(e) {
    if (!e || e === document.body || e === document.documentElement) return 'body';
    if (e.id && /^[A-Za-z][\w-]*$/.test(e.id)) return '#' + e.id;
    if (first(e)) return simple(e);
    return path(e.parentElement) + ' > ' + e.tagName.toLowerCase();
  }
  // The item a click is in: the nearest element, it or above it, that the page has several of.
  function itemOf(e) {
    for (var a = e; a && a !== document.body; a = a.parentElement) {
      if (first(a) && document.querySelectorAll(simple(a)).length >= 3) return simple(a);
    }
    for (var b = e; b && b.parentElement && b !== document.body; b = b.parentElement) {
      var same = [].filter.call(b.parentElement.children, function (x) { return x.tagName === b.tagName; });
      if (same.length >= 3) return path(b.parentElement) + ' > ' + b.tagName.toLowerCase();
    }
    return null;
  }
  // A part of an item, as the item finds it: its first class, or the tags down to it.
  function specOf(e, item) {
    if (e === item) return '';
    var c = first(e);
    if (c && item.querySelector('.' + esc(c)) === e) return '.' + c;
    var steps = [];
    for (var a = e; a && a !== item; a = a.parentElement) {
      if (first(a) && a !== e && item.querySelector('.' + esc(first(a))) === a) { steps.unshift('.' + first(a)); break; }
      steps.unshift(a.tagName.toLowerCase());
    }
    return steps.join(' ');
  }
  function all(sel) { try { return [].slice.call(document.querySelectorAll(sel)); } catch (e) { return []; } }
  function clear(cls) { all('.' + cls).forEach(function (e) { e.classList.remove(cls); }); }
  function mark() {
    clear('zp-item'); clear('zp-field');
    if (!items) return;
    all(items).forEach(function (it) {
      it.classList.add('zp-item');
      specs.forEach(function (s) {
        var sel = s.split('@')[0].trim();
        var el = sel ? (function () { try { return it.querySelector(sel); } catch (e) { return null; } })() : it;
        if (el) el.classList.add('zp-field');
      });
    });
  }
  function say(m) { parent.postMessage(m, '*'); }
  document.addEventListener('mouseover', function (e) { if (over) over.classList.remove('zp-hover'); over = e.target; over.classList.add('zp-hover'); }, true);
  document.addEventListener('mouseout', function () { if (over) over.classList.remove('zp-hover'); over = null; }, true);
  document.addEventListener('submit', function (e) { e.preventDefault(); }, true);
  document.addEventListener('click', function (e) {
    e.preventDefault(); e.stopPropagation();
    var t = e.target;
    var inside = null;
    if (items) { try { inside = t.closest(items); } catch (x) { inside = null; } }
    if (!items || !inside) {
      var sel = itemOf(t);
      if (!sel) { say({ zp: 'none' }); return; }
      items = sel; specs = []; mark();
      say({ zp: 'items', items: items, count: all(items).length });
      return;
    }
    var link = t.closest('a');
    var img = t.tagName === 'IMG' ? t : null;
    say({ zp: 'field', spec: specOf(t, inside), text: (t.innerText || t.textContent || '').trim().slice(0, 120),
          href: link && inside.contains(link) ? specOf(link, inside) : null, src: img ? specOf(img, inside) : null,
          own: inside.tagName === 'A' && t === inside });
  }, true);
  window.addEventListener('message', function (e) {
    if (e.source !== parent || !e.data || !e.data.zp) return;
    if (e.data.zp === 'show') { items = e.data.items || null; specs = e.data.specs || []; mark();
      if (items) { var it = all(items)[0]; if (it) window.scrollTo(0, Math.max(0, it.getBoundingClientRect().top + window.scrollY - 80)); }
      say({ zp: 'shown', count: items ? all(items).length : 0 }); }
  });
  say({ zp: 'ready' });
})();
"#;

/// On the page: the steps, the fields named, the preview asked for, and the choice saved.
const PAGE_SCRIPT: &str = r#"
(function () {
  var start = JSON.parse(document.getElementById('pick-start').textContent);
  var frame = document.getElementById('pick-frame');
  var state = { items: null, count: 0, fields: [], id: null, title: null };
  var $ = function (id) { return document.getElementById(id); };
  // A name for a part: its class, or what its tags say (a heading is the title, a link the link).
  function nameFor(spec, taken) {
    var attr = spec.indexOf('@') >= 0 ? spec.split('@')[1] : '';
    var steps = spec.split('@')[0].split(/[\s>]+/).filter(Boolean), n = '';
    if (attr === 'href') n = 'link'; else if (attr === 'src') n = 'image';
    for (var k = steps.length - 1; !n && k >= 0; k--) {
      var s = steps[k];
      if (s.indexOf('.') >= 0) n = s.split('.').pop();
      else if (/^h[1-6]$/.test(s)) n = 'title';
      else if (s === 'time') n = 'date';
      else if (s === 'img') n = 'image';
    }
    n = (n || (spec ? 'field' : 'text')).toLowerCase().replace(/[^a-z0-9]+/g, '_').replace(/^_+|_+$/g, '') || 'field';
    var m = n, i = 2; while (taken.indexOf(m) >= 0) { m = n + '_' + i++; }
    return m;
  }
  function show() { frame.contentWindow.postMessage({ zp: 'show', items: state.items, specs: state.fields.map(function (f) { return f.spec; }) }, '*'); }
  function el(tag, attrs, text) { var e = document.createElement(tag); for (var k in attrs || {}) e.setAttribute(k, attrs[k]); if (text != null) e.textContent = text; return e; }
  function drawFound() {
    var box = $('pick-found'); box.textContent = '';
    if (!start.found.length) return;
    box.appendChild(el('span', { 'class': 'dim' }, 'Or one Zetlyn found: '));
    start.found.forEach(function (c) {
      var b = el('button', { type: 'button', 'class': 'chip' + (c.items === state.items ? ' on' : '') }, c.count + ' items');
      b.title = c.items;
      b.onclick = function () { take(c.items, c.count, Object.keys(c.fields).map(function (n) { return { name: n, spec: c.fields[n] }; })); };
      box.appendChild(b); box.appendChild(document.createTextNode(' '));
    });
  }
  function take(items, count, fields) {
    state.items = items; state.count = count; state.fields = fields || []; state.id = null; state.title = null;
    $('pick-items-say').textContent = count + ' items marked (' + items + '). Click another item to choose another list.';
    drawFound(); drawFields(); show(); ask();
  }
  function drawFields() {
    var body = $('pick-fields'); body.textContent = '';
    state.fields.forEach(function (f, i) {
      var tr = el('tr');
      var name = el('input', { type: 'text', value: f.name, spellcheck: 'false' }); name.oninput = function () { f.name = name.value.trim(); ask(); };
      tr.appendChild(el('td')).appendChild(name);
      var kind = el('td', { 'class': 'dim mono' }, f.spec.indexOf('@') >= 0 ? (f.spec.split('@')[1] === 'href' ? 'link' : f.spec.split('@')[1] === 'src' ? 'image' : f.spec.split('@')[1]) : 'text');
      tr.appendChild(kind);
      var id = el('input', { type: 'radio', name: 'pick-id' }); id.checked = state.id === f.name; id.onchange = function () { state.id = f.name; ask(); };
      tr.appendChild(el('td')).appendChild(id);
      var title = el('input', { type: 'radio', name: 'pick-title' }); title.checked = state.title === f.name; title.onchange = function () { state.title = f.name; ask(); };
      tr.appendChild(el('td')).appendChild(title);
      var drop = el('button', { type: 'button', 'class': 'link', title: 'Not this' }, '✕'); drop.onclick = function () { state.fields.splice(i, 1); drawFields(); show(); ask(); };
      tr.appendChild(el('td')).appendChild(drop);
      body.appendChild(tr);
    });
    if (!state.fields.length) { var tr = el('tr'); tr.appendChild(el('td', { colspan: '5', 'class': 'dim' }, state.items ? 'Click a part of an item on the page.' : 'Choose the list first.')); body.appendChild(tr); }
  }
  var asking = 0;
  function ask() {
    var n = ++asking;
    $('pick-save').disabled = true;
    if (!state.items || !state.fields.length) { $('pick-preview').innerHTML = '<p class="dim">Nothing chosen yet.</p>'; return; }
    post('preview', { items: state.items, fields: fields() }).then(function (r) {
      if (n !== asking) return;
      $('pick-error').hidden = !r.error; $('pick-error').textContent = r.error || '';
      if (r.error) return;
      var box = $('pick-preview'); box.textContent = '';
      box.appendChild(el('p', { 'class': 'dim' }, r.count + ' items read on the page'));
      var t = el('table', { 'class': 'admin-table' }), head = el('tr'), names = Object.keys(fields());
      names.forEach(function (k) { head.appendChild(el('th', {}, k)); });
      t.appendChild(el('thead')).appendChild(head);
      var tb = el('tbody'); t.appendChild(tb);
      r.rows.forEach(function (row) { var tr = el('tr'); names.forEach(function (k) { var v = row[k]; tr.appendChild(el('td', {}, v == null ? '' : (typeof v === 'string' ? v : JSON.stringify(v)))); }); tb.appendChild(tr); });
      box.appendChild(el('div', { 'class': 'scroll' })).appendChild(t);
      $('pick-save').disabled = r.count === 0;
    });
  }
  function fields() { var o = {}; state.fields.forEach(function (f) { if (f.name) o[f.name] = f.spec; }); return o; }
  function post(what, body) {
    return fetch(start.base + '/pick/' + what, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) })
      .then(function (r) { return r.json(); }).catch(function (e) { return { error: String(e) }; });
  }
  window.addEventListener('message', function (e) {
    if (e.source !== frame.contentWindow || !e.data || !e.data.zp) return;
    var m = e.data;
    if (m.zp === 'ready') { if (start.now && start.now.items) take(start.now.items, 0, Object.keys(start.now.fields || {}).map(function (n) { return { name: n, spec: start.now.fields[n] }; })); else drawFields(); drawFound(); }
    else if (m.zp === 'shown') { if (state.items) { state.count = m.count; $('pick-items-say').textContent = m.count + ' items marked (' + state.items + '). Click another item to choose another list.'; } }
    else if (m.zp === 'none') { $('pick-items-say').textContent = 'That is not part of a list Zetlyn can tell: click an item of the list itself.'; }
    else if (m.zp === 'items') { take(m.items, m.count, []); }
    else if (m.zp === 'field') {
      var spec = m.src != null ? (m.src + '@src') : m.spec;
      if (m.own) spec = '';
      var had = state.fields.length;
      var names = function () { return state.fields.map(function (f) { return f.name; }); };
      if (!state.fields.some(function (f) { return f.spec === spec; })) state.fields.push({ name: nameFor(spec, names()), spec: spec });
      // A link in it is kept as well, once: where an item is read in full.
      if (m.href != null && !state.fields.some(function (f) { return f.spec.indexOf('@href') >= 0; })) {
        state.fields.push({ name: nameFor('@href', names()), spec: (m.href ? m.href : '') + '@href' });
      }
      if (state.fields.length === had) return;
      drawFields(); show(); ask();
    }
  });
  $('pick-save').onclick = function () {
    $('pick-save').disabled = true;
    post('save', { items: state.items, fields: fields(), id: state.id, title: state.title }).then(function (r) {
      if (r.error) { $('pick-error').hidden = false; $('pick-error').textContent = r.error; $('pick-save').disabled = false; return; }
      (function follow() {
        fetch(start.jobs + r.job).then(function (x) { return x.json(); }).then(function (j) {
          if (!j.done) { $('pick-save').textContent = (j.progress && j.progress.text) || 'Reading…'; setTimeout(follow, 500); return; }
          if (j.error) { $('pick-error').hidden = false; $('pick-error').textContent = j.error; $('pick-save').textContent = 'Save and read it'; $('pick-save').disabled = false; return; }
          location.href = j.then;
        });
      })();
    });
  };
})();
"#;

#[cfg(test)]
mod tests {
    #[test]
    fn the_page_shown_runs_no_script_of_its_own() {
        let page = r#"<html><head><base href="https://evil.example/"><meta http-equiv="refresh" content="0;url=https://x"><link rel=stylesheet crossorigin href=/a.css><script>steal()</script></head><body onload="steal()"><a href="javascript:steal()" onclick='steal()'>x</a><iframe src="https://x"></iframe><noscript><p>shown</p></noscript></body></html>"#;
        let shown = super::framed(page, "https://shop.example/list", "N0NCE");
        assert!(!shown.contains("steal()"), "{shown}");
        assert!(!shown.contains("evil.example") && !shown.contains("http-equiv") && !shown.contains("<iframe") && !shown.contains("crossorigin"), "{shown}");
        assert!(shown.contains(r#"<base href="https://shop.example/list">"#) && shown.contains("<p>shown</p>"));
        assert_eq!(shown.matches("<script").count(), 1);
        assert!(shown.contains(r#"<script nonce="N0NCE">"#));
        assert!(super::frame_policy("N0NCE").contains("sandbox allow-scripts;") && super::frame_policy("N0NCE").contains("'nonce-N0NCE'"));
    }
}
