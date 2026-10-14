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

/// The page: what is tracked beside the page it is chosen on. `now` is what the source reads
/// today, to start from.
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
                        h2 { span.step { "1" } "Click what to track" }
                        p #pick-say { "On the page: a price, a number, a name, a date. Each one you click is tracked." }
                        div #pick-ask .picker-ask hidden {
                            p #pick-ask-say {}
                            p.picker-actions {
                                button.primary #pick-all type="button" {}
                                button #pick-one type="button" { "Only this one" }
                            }
                        }
                        p.picker-found #pick-found {}
                    }
                    section #pick-tracked-box {
                        h2 { span.step { "2" } "Tracked" }
                        ul.picker-tracked #pick-tracked {}
                        p.dim #pick-more {}
                        details #pick-advanced hidden {
                            summary { "How items are told apart" }
                            p.dim { "Zetlyn follows each item over time. To know an item tomorrow is the one it read today, it needs something of it that does not change: its name, its link. Never a price." }
                            p { label { "Each item is known by " select #pick-id {} } }
                            p { label { "Shown with its " select #pick-title {} } }
                        }
                    }
                    section {
                        h2 { span.step { "3" } "What Zetlyn reads" }
                        div #pick-preview { p.dim { "Nothing chosen yet." } }
                        div #pick-error .note hidden {}
                        p.picker-actions {
                            button.primary #pick-save type="button" disabled { "Save and read it" }
                            button #pick-reset type="button" { "Start over" }
                            a.button href=(base) { "Cancel" }
                        }
                        p.dim { "Saving changes what this source reads on the page. What it read before stays in its history." }
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

/// The marks in the frame: what the pointer is over, the items, what is tracked in each.
const MARKS: &str = r#"
.zp-hover { outline: 2px dashed #e0512a !important; outline-offset: 1px !important; cursor: crosshair !important; }
.zp-item { outline: 2px solid #e0512a !important; outline-offset: 2px !important; }
.zp-field { background: rgba(255, 196, 0, .38) !important; box-shadow: 0 0 0 2px rgba(224, 81, 42, .55) !important; }
* { cursor: crosshair !important; }
html, body { opacity: 1 !important; visibility: visible !important; }
"#;

/// In the frame: what is clicked, said to the app by message: where it is on the page, and, where
/// it is one of several alike, the list it is in and where it is in its item.
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
  // Where one element is on the whole page, and nowhere else: down from the body, each step
  // counted among its siblings of the same tag where there are several.
  function where(e) {
    var steps = [];
    for (var a = e; a && a !== document.body && a !== document.documentElement; a = a.parentElement) {
      if (a.id && /^[A-Za-z][\w-]*$/.test(a.id) && document.querySelectorAll('#' + a.id).length === 1) { steps.unshift('#' + a.id); break; }
      var s = simple(a), p = a.parentElement;
      if (p) { var same = [].filter.call(p.children, function (x) { return x.tagName === a.tagName; }); if (same.length > 1) s += ':nth-of-type(' + (same.indexOf(a) + 1) + ')'; }
      steps.unshift(s);
    }
    if (!steps.length || steps[0].charAt(0) !== '#') steps.unshift('body');
    return steps.join(' > ');
  }
  // The item a click is in: the nearest element, it or above it, that the page has several of,
  // and then the outermost around it that there are as many of, each holding one: a price is one
  // of twenty, and so is the book it is the price of.
  function itemOf(e) {
    var near = null, n = 0;
    for (var a = e; a && a !== document.body; a = a.parentElement) {
      if (first(a)) { var c = all(simple(a)).length; if (c >= 3) { near = a; n = c; break; } }
    }
    if (near) {
      var best = near, sel = simple(near);
      for (var up = near.parentElement; up && up !== document.body; up = up.parentElement) {
        if (!first(up)) continue;
        var k = all(simple(up)).length;
        if (k < n) break;
        if (k === n && up.querySelectorAll(sel).length === 1) best = up;
      }
      return simple(best);
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
  function one(root, sel) { try { return sel ? root.querySelector(sel) : root; } catch (e) { return null; } }
  function clear(cls) { all('.' + cls).forEach(function (e) { e.classList.remove(cls); }); }
  function mark() {
    clear('zp-item'); clear('zp-field');
    if (!items) return;
    all(items).forEach(function (it) {
      if (items !== 'html') it.classList.add('zp-item');
      specs.forEach(function (s) { var el = one(it, s.split('@')[0].trim()); if (el && el !== document.documentElement) el.classList.add('zp-field'); });
    });
  }
  function say(m) { parent.postMessage(m, '*'); }
  function text(e) { return (e.innerText || e.textContent || '').trim().replace(/\s+/g, ' ').slice(0, 120); }
  document.addEventListener('mouseover', function (e) { if (over) over.classList.remove('zp-hover'); over = e.target; over.classList.add('zp-hover'); }, true);
  document.addEventListener('mouseout', function () { if (over) over.classList.remove('zp-hover'); over = null; }, true);
  document.addEventListener('submit', function (e) { e.preventDefault(); }, true);
  document.addEventListener('click', function (e) {
    e.preventDefault(); e.stopPropagation();
    var t = e.target, m = { zp: 'click', text: text(t), where: where(t), img: t.tagName === 'IMG' };
    // A text shortened on the page (a long name cut to fit) is whole in the title it carries.
    var full = t.getAttribute && t.getAttribute('title'), cut = m.text.replace(/(\.\.\.|…)\s*$/, '').trim();
    if (full && full.length > m.text.length && full.indexOf(cut) === 0) { m.attr = 'title'; m.text = full; }
    var list = itemOf(t);
    if (list) {
      var it = t.closest(list);
      m.list = list; m.count = all(list).length; m.part = specOf(t, it);
      var link = t.closest('a'); if (link && it.contains(link)) m.link = specOf(link, it);
    }
    if (items && items !== 'html') {
      var inside = null; try { inside = t.closest(items); } catch (x) { inside = null; }
      if (inside) { m.inside = specOf(t, inside); var l = t.closest('a'); if (l && inside.contains(l)) m.insideLink = specOf(l, inside); }
    }
    say(m);
  }, true);
  window.addEventListener('message', function (e) {
    if (e.source !== parent || !e.data || !e.data.zp) return;
    if (e.data.zp === 'show') {
      items = e.data.items || null; specs = e.data.specs || []; mark();
      if (items && items !== 'html' && e.data.scroll) { var it = all(items)[0]; if (it) window.scrollTo(0, Math.max(0, it.getBoundingClientRect().top + window.scrollY - 80)); }
    }
  });
  say({ zp: 'ready' });
})();
"#;

/// On the page: what is clicked becomes what is tracked, of one thing or of every item of a list;
/// what that reads is asked for; the choice is saved.
const PAGE_SCRIPT: &str = r#"
(function () {
  var start = JSON.parse(document.getElementById('pick-start').textContent);
  var frame = document.getElementById('pick-frame');
  var $ = function (id) { return document.getElementById(id); };
  // mode: null until the first click says; 'list' for every item alike, 'single' for one thing.
  var state = { mode: null, items: null, count: 0, fields: [], id: '', title: '', pending: null };
  function el(tag, attrs, text) { var e = document.createElement(tag); for (var k in attrs || {}) e.setAttribute(k, attrs[k]); if (text != null) e.textContent = text; return e; }
  // A name for what is tracked: its class, or what its tags say.
  function nameFor(spec, taken) {
    var attr = spec.indexOf('@') >= 0 ? spec.split('@')[1] : '';
    var steps = spec.split('@')[0].split(/[\s>]+/).filter(Boolean), n = '';
    if (attr === 'href') n = 'link'; else if (attr === 'src') n = 'image';
    for (var k = steps.length - 1; !n && k >= 0; k--) {
      var s = steps[k].replace(/:.*$/, '');
      var tag = s.split('.')[0];
      if (/^h[1-6]$/.test(tag)) n = 'title';
      else if (tag === 'time') n = 'date';
      else if (tag === 'img') n = 'image';
      else if (s.indexOf('.') >= 0) n = s.split('.').pop();
      else if (/^h[1-6]$/.test(s)) n = 'title';
      else if (s === 'time') n = 'date';
      else if (s === 'img') n = 'image';
    }
    n = (n || 'value').toLowerCase().replace(/[^a-z0-9]+/g, '_').replace(/^_+|_+$/g, '') || 'value';
    var m = n, i = 2; while (taken.indexOf(m) >= 0) { m = n + '_' + i++; }
    return m;
  }
  function names() { return state.fields.map(function (f) { return f.name; }); }
  // What a click is kept as: an image by its address, a shortened name by its whole title.
  function specFor(at, m) { return m.img ? at + '@src' : m.attr ? at + '@' + m.attr : at; }
  function add(spec, sample) {
    if (state.fields.some(function (f) { return f.spec === spec; })) return false;
    state.fields.push({ name: nameFor(spec, names()), spec: spec, sample: sample || '' });
    return true;
  }
  function show(scroll) { frame.contentWindow.postMessage({ zp: 'show', items: state.items, specs: state.fields.map(function (f) { return f.spec; }), scroll: !!scroll }, '*'); }
  function say() {
    var s = $('pick-say'), more = $('pick-more');
    if (!state.mode) { s.textContent = 'On the page: a price, a number, a name, a date. Each one you click is tracked.'; more.textContent = ''; }
    else if (state.mode === 'single') { s.textContent = 'Tracking this page.'; more.textContent = 'Click more on the page to track it too.'; }
    else { s.textContent = 'Tracking each of the ' + state.count + ' items marked on the page.'; more.textContent = 'Click more in any item to track it in all of them.'; }
    $('pick-advanced').hidden = state.mode !== 'list' || state.fields.length < 1;
    drawFound();
  }
  function drawFound() {
    var box = $('pick-found'); box.textContent = '';
    if (state.mode === 'single' || !start.found.length) return;
    box.appendChild(el('span', { 'class': 'dim' }, state.mode ? 'Another list Zetlyn found: ' : 'Or a list Zetlyn found: '));
    start.found.forEach(function (c) {
      var b = el('button', { type: 'button', 'class': 'chip' + (c.items === state.items ? ' on' : '') }, c.count + ' items');
      b.title = c.items;
      b.onclick = function () {
        state.mode = 'list'; state.items = c.items; state.count = c.count; state.id = ''; state.title = '';
        state.fields = Object.keys(c.fields).map(function (n) { return { name: n, spec: c.fields[n], sample: '' }; });
        changed(true);
      };
      box.appendChild(b); box.appendChild(document.createTextNode(' '));
    });
  }
  function drawTracked() {
    var ul = $('pick-tracked'); ul.textContent = '';
    state.fields.forEach(function (f, i) {
      var li = el('li');
      var name = el('input', { type: 'text', value: f.name, spellcheck: 'false', 'aria-label': 'Name' });
      name.oninput = function () { var was = f.name; f.name = name.value.trim(); if (state.id === was) state.id = f.name; if (state.title === was) state.title = f.name; ask(); drawAdvanced(); };
      li.appendChild(name);
      li.appendChild(el('span', { 'class': 'dim picker-sample' }, f.sample || (f.spec.indexOf('@href') >= 0 ? 'its link' : '')));
      var drop = el('button', { type: 'button', 'class': 'link', title: 'Stop tracking this' }, '✕');
      drop.onclick = function () { state.fields.splice(i, 1); if (!state.fields.length) reset(); else changed(false); };
      li.appendChild(drop);
      ul.appendChild(li);
    });
    if (!state.fields.length) ul.appendChild(el('li', { 'class': 'dim' }, 'Nothing yet.'));
    drawAdvanced();
  }
  function drawAdvanced() {
    [['pick-id', 'id', 'chosen by Zetlyn'], ['pick-title', 'title', 'chosen by Zetlyn']].forEach(function (x) {
      var sel = $(x[0]); sel.textContent = '';
      sel.appendChild(el('option', { value: '' }, x[2]));
      state.fields.forEach(function (f) { var o = el('option', { value: f.name }, f.name); if (state[x[1]] === f.name) o.selected = true; sel.appendChild(o); });
      sel.onchange = function () { state[x[1]] = sel.value; ask(); };
    });
  }
  function changed(scroll) { say(); drawTracked(); show(scroll); ask(); }
  function reset() {
    state = { mode: null, items: null, count: 0, fields: [], id: '', title: '', pending: null };
    $('pick-ask').hidden = true; $('pick-error').hidden = true;
    changed(false);
  }
  var asking = 0;
  function fields() { var o = {}; state.fields.forEach(function (f) { if (f.name) o[f.name] = f.spec; }); return o; }
  function post(what, body) {
    return fetch(start.base + '/pick/' + what, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) })
      .then(function (r) { return r.json(); }).catch(function (e) { return { error: String(e) }; });
  }
  function ask() {
    var n = ++asking;
    $('pick-save').disabled = true;
    if (!state.items || !state.fields.length) { $('pick-preview').innerHTML = '<p class="dim">Nothing chosen yet.</p>'; return; }
    post('preview', { items: state.items, fields: fields() }).then(function (r) {
      if (n !== asking) return;
      $('pick-error').hidden = !r.error; $('pick-error').textContent = r.error || '';
      if (r.error) return;
      var box = $('pick-preview'); box.textContent = '';
      box.appendChild(el('p', { 'class': 'dim' }, state.mode === 'single' ? 'Read on the page now:' : r.count + ' items read on the page'));
      var t = el('table', { 'class': 'admin-table' }), head = el('tr'), keys = Object.keys(fields());
      keys.forEach(function (k) { head.appendChild(el('th', {}, k)); });
      t.appendChild(el('thead')).appendChild(head);
      var tb = el('tbody'); t.appendChild(tb);
      r.rows.forEach(function (row) { var tr = el('tr'); keys.forEach(function (k) { var v = row[k]; tr.appendChild(el('td', {}, v == null ? '' : (typeof v === 'string' ? v : JSON.stringify(v)))); }); tb.appendChild(tr); });
      box.appendChild(el('div', { 'class': 'scroll' })).appendChild(t);
      // What was read is what each tracked value says now, shown beside its name.
      if (r.rows[0]) state.fields.forEach(function (f, i) { var v = r.rows[0][f.name]; var s = document.querySelectorAll('#pick-tracked .picker-sample')[i]; if (s && v != null) s.textContent = typeof v === 'string' ? v : JSON.stringify(v); });
      $('pick-save').disabled = r.count === 0;
    });
  }
  // One thing, or every item like it: what the first click into a list is asked.
  function choose(m) {
    state.pending = m;
    $('pick-ask-say').textContent = '“' + (m.text || 'This') + '” is in one of ' + m.count + ' items alike on this page.';
    $('pick-all').textContent = 'Track it for all ' + m.count;
    $('pick-ask').hidden = false;
  }
  $('pick-all').onclick = function () {
    var m = state.pending; $('pick-ask').hidden = true; if (!m) return;
    state.mode = 'list'; state.items = m.list; state.count = m.count; state.fields = [];
    add(specFor(m.part, m), m.text);
    if (m.link != null) add((m.link ? m.link : '') + '@href', '');
    changed(true);
  };
  $('pick-one').onclick = function () {
    var m = state.pending; $('pick-ask').hidden = true; if (!m) return;
    state.mode = 'single'; state.items = 'html'; state.count = 1; state.fields = [];
    add(specFor(m.where, m), m.text);
    changed(false);
  };
  $('pick-reset').onclick = reset;
  window.addEventListener('message', function (e) {
    if (e.source !== frame.contentWindow || !e.data || !e.data.zp) return;
    var m = e.data;
    if (m.zp === 'ready') {
      var now = start.now || {};
      if (now.items === 'html') { state.mode = 'single'; state.items = 'html'; state.count = 1; }
      else if (now.items) { state.mode = 'list'; state.items = now.items; }
      state.fields = Object.keys(now.fields || {}).map(function (n) { return { name: n, spec: now.fields[n], sample: '' }; });
      if (!state.fields.length) { state.mode = null; state.items = null; }
      changed(true);
      return;
    }
    if (m.zp !== 'click') return;
    if (!state.mode) {
      if (m.list) choose(m);
      else { state.mode = 'single'; state.items = 'html'; state.count = 1; add(specFor(m.where, m), m.text); changed(false); }
    } else if (state.mode === 'single') {
      if (add(specFor(m.where, m), m.text)) changed(false);
    } else if (m.inside != null) {
      var added = add(specFor(m.inside, m), m.text);
      if (m.insideLink != null && !state.fields.some(function (f) { return f.spec.indexOf('@href') >= 0; })) added = add((m.insideLink ? m.insideLink : '') + '@href', '') || added;
      if (added) changed(false);
    } else {
      $('pick-say').textContent = 'That is outside the items you are tracking. Start over to track something else.';
    }
  });
  $('pick-save').onclick = function () {
    $('pick-save').disabled = true;
    post('save', { items: state.items, fields: fields(), id: state.id || null, title: state.title || null, single: state.mode === 'single' }).then(function (r) {
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
