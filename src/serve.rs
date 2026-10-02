//! The surface. One stylesheet, server-rendered, no build step.

use std::collections::BTreeMap;

use maud::{html, Markup, PreEscaped, DOCTYPE};
use serde_json::{json, Value as J};

use crate::source::{Source, Query};
use crate::sourcedecl::View;
use crate::expr::{self, Lit, Op, Pred};
use crate::claim::{Claim, Value};
use crate::store::Hit;

pub const STYLE: &str = r#"
/* The palette is the website's, to the value. A reader who arrives from zetlyn.com or from a hub
   should not be told by the colours that they have left. The layout is this program's own: a
   scope surface is a dense thing and the site's vocabulary has no rows, facets or chips in it. */
:root {
  --bg: #f2efe7; --fg: #14202a; --dim: #667078; --line: #cfd1ca;
  --panel: #fbfaf6; --accent: #dc4a20; --chip: #dfe2db;
  --line-strong: #afb5af; --wash: rgba(20,32,42,.04); --mark-filter: none;
  color-scheme: light;
}
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) {
    color-scheme: dark;
    --bg: #11181d; --fg: #e9e6de; --dim: #98a3ab; --line: #2b353c;
    --panel: #161e24; --accent: #ff6a3d; --chip: #1c252b;
    --line-strong: #3a444b; --wash: rgba(233,230,222,.05); --mark-filter: invert(1);
  }
}
:root[data-theme="dark"] {
  color-scheme: dark;
  --bg: #11181d; --fg: #e9e6de; --dim: #98a3ab; --line: #2b353c;
  --panel: #161e24; --accent: #ff6a3d; --chip: #1c252b;
  --line-strong: #3a444b; --wash: rgba(233,230,222,.05); --mark-filter: invert(1);
}
:root[data-theme="light"] { color-scheme: light; }
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--fg);
       font: 15px/1.55 Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont,
             "Segoe UI", sans-serif; -webkit-font-smoothing: antialiased; }
main { width: min(1180px, calc(100% - 48px)); margin: 0 auto; padding: 2.2rem 0 5rem; }
a { color: var(--accent); text-decoration: none; }
a:hover { text-decoration: underline; }
h1 { font-size: 2.1rem; line-height: 1.15; margin: 0 0 .2rem; letter-spacing: -.035em; font-weight: 750; }
h2 { font-size: .8rem; text-transform: uppercase; letter-spacing: .12em;
     color: var(--dim); margin: 2.2rem 0 .7rem; font-weight: 600;
     font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace; }
h3 { font-size: 1rem; margin: 1.4rem 0 .4rem; }
.about { color: var(--dim); margin: 0 0 1.2rem; max-width: 48rem; }
.bar { display: flex; gap: .5rem; flex-wrap: wrap; align-items: center; margin: 1rem 0; }
input[type=search], input[type=text] { flex: 1 1 22rem; min-width: 0; padding: .55rem .7rem; font: inherit;
  background: var(--panel); color: var(--fg); border: 1px solid var(--line); border-radius: 6px; }
button { padding: .55rem .9rem; font: inherit; cursor: pointer; border-radius: 6px;
  border: 1px solid var(--line); background: var(--panel); color: var(--fg); }
.chip { display: inline-block; padding: .12rem .5rem; border-radius: 999px;
  background: var(--chip); color: var(--fg); font-size: .82rem; white-space: nowrap; }
.chip.on { background: var(--accent); color: var(--bg); }
.state { font-size: .82rem; }
.state.current { color: #2e7d32; } .state.partial, .state.failing { color: #c0392b; }
.state.empty { color: var(--dim); }
table { border-collapse: collapse; width: 100%; font-size: .92rem; }
th { text-align: left; font-weight: 600; color: var(--dim); font-size: .78rem;
     text-transform: uppercase; letter-spacing: .05em; padding: .4rem .6rem .4rem 0;
     border-bottom: 1px solid var(--line); }
td { padding: .5rem .6rem .5rem 0; border-bottom: 1px solid var(--line);
     vertical-align: top; }
td.num { text-align: right; font-variant-numeric: tabular-nums; }
.grid { display: grid; gap: 1.2rem; grid-template-columns: repeat(auto-fit, minmax(15rem, 1fr)); }
.card { border: 1px solid var(--line); border-radius: 8px; padding: .8rem 1rem;
        background: var(--panel); }
.card h4 { margin: 0 0 .4rem; font-size: .9rem; }
.cover { color: var(--dim); font-size: .8rem; font-weight: 400; }
.facet { display: flex; justify-content: space-between; gap: .6rem; padding: .16rem 0;
         font-size: .88rem; }
.facet .n { color: var(--dim); font-variant-numeric: tabular-nums; }
.dim { color: var(--dim); }
.why { color: var(--dim); font-size: .8rem; }
.note { border-left: 3px solid var(--accent); padding: .5rem .8rem; background: var(--panel);
        margin: 1rem 0; font-size: .9rem; }
.text { white-space: pre-wrap; max-width: 46rem; }
footer { margin-top: 3rem; padding-top: 1rem; border-top: 1px solid var(--line);
         color: var(--dim); font-size: .82rem; }
/* A receipt: where one value came from, opened in place. */
details.receipt { margin-top: .25rem; font-size: .85rem; }
details.receipt > summary { cursor: pointer; color: var(--dim); list-style: none; }
details.receipt > summary::-webkit-details-marker { display: none; }
details.receipt > summary::before { content: "↗ "; color: var(--accent); }
details.receipt[open] { background: var(--panel); border-left: 3px solid var(--accent);
                        padding: .5rem .8rem; margin: .4rem 0; }
.receipt dl { display: grid; grid-template-columns: max-content 1fr; gap: .2rem .9rem; margin: .4rem 0; }
.receipt dt { color: var(--dim); }
.receipt dd { margin: 0; }
.receipt pre { max-height: 18rem; overflow: auto; font-size: .78rem; background: var(--bg);
               padding: .5rem; border: 1px solid var(--line); }
.receipt table { font-size: .82rem; }
/* The frame: the website's header, then where the reader is. Its sizes are the website's
   (zetlyn.com, assets/style.css: .shell, .site-header, .site-footer), so a reader moving between
   the site, the hub and a tracker sees one header and one footer. */
.wrap { width: min(1180px, calc(100% - 48px)); margin: 0 auto; }
header.top { background: var(--bg); }
header.top .wrap { height: 82px; display: flex; align-items: center; gap: 1.5rem;
  border-bottom: 1px solid var(--line); }
.brand { display: flex; align-items: center; gap: 10px; color: var(--fg); font-weight: 750;
  letter-spacing: -.03em; font-size: 20px; }
.brand:hover { text-decoration: none; }
.brand-mark { width: 26px; height: 26px; filter: var(--mark-filter); }
nav.links { margin-left: auto; display: flex; gap: 28px; font-size: 14px; }
nav.links a { color: var(--dim); }
nav.links a:hover, nav.links a[aria-current] { color: var(--fg); text-decoration: none; }
/* Where the reader is, marked where the header meets its line, as the website marks it. */
header.top nav.links { align-self: stretch; }
header.top nav.links a { display: flex; align-items: center; border-bottom: 2px solid transparent; margin-bottom: -1px; }
header.top nav.links a[aria-current] { border-bottom-color: var(--accent); }
/* Who is signed in, at the right of the header, where somebody can be. */
header.top .account { margin-left: 28px; display: flex; align-items: center; gap: .6rem; font-size: 13px; }
header.top a.account { border: 1px solid var(--fg); color: var(--fg); padding: 6px 12px; }
header.top a.account:hover { background: var(--fg); color: var(--bg); text-decoration: none; }
header.top form.account button { font: inherit; font-size: 13px; padding: 5px 10px; }
/* Three parts, one brand: the website, the hub and the app. Each names itself beside the logo,
   has an accent of its own, and keeps the other two a click away. */
.brand-area { font-weight: 400; color: var(--accent); margin-left: 1px; }
header.top .org-name, header.top .org-switch summary { font-size: 14px; color: var(--fg); border-left: 1px solid var(--line);
  padding-left: 1rem; cursor: default; }
header.top .org-switch { position: relative; }
header.top .org-switch summary { cursor: pointer; list-style: none; }
header.top .org-switch summary::after { content: " ▾"; color: var(--dim); }
header.top .org-switch ul { position: absolute; top: 2rem; left: .6rem; z-index: 5; list-style: none; margin: 0; padding: .4rem 0;
  background: var(--panel); border: 1px solid var(--line); min-width: 12rem; }
header.top .org-switch li a { display: block; padding: .35rem .9rem; color: var(--fg); }
header.top nav.areas { display: flex; gap: 8px; margin-left: 22px; }
header.top a.area-link { font: 11px/normal ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace; text-transform: uppercase;
  letter-spacing: .08em; color: var(--dim); border: 1px solid var(--line-strong); padding: 6px 10px; }
header.top a.area-link:hover { color: var(--fg); border-color: var(--fg); text-decoration: none; }
body.area-app { --accent: #2f7d4f; }
@media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) body.area-app { --accent: #52c486; } }
:root[data-theme="dark"] body.area-app { --accent: #52c486; }
@media (max-width: 40rem) { header.top nav.areas { display: none; } header.top .org-name, header.top .org-switch summary { display: none; } }
/* The switch between the three parts: the same control, in the same place, in each. */
.switcher { position: relative; }
.switcher summary { list-style: none; cursor: pointer; width: 30px; height: 30px; display: grid; place-items: center;
  border: 1px solid currentColor; opacity: .7; }
.switcher summary::-webkit-details-marker { display: none; }
.switcher summary:hover, .switcher[open] summary { opacity: 1; }
.switcher svg rect { fill: currentColor; }
.switcher-panel { position: absolute; top: 38px; left: 0; z-index: 30; width: 300px; background: var(--panel); color: var(--fg);
  border: 1px solid var(--fg); box-shadow: 8px 8px 0 rgba(0,0,0,.12); }
.switcher-panel a { display: block; padding: 12px 14px; border-bottom: 1px solid var(--line); color: var(--fg); }
.switcher-panel a:last-child { border-bottom: 0; }
.switcher-panel a:hover { background: var(--wash); text-decoration: none; }
.switcher-panel a.on { box-shadow: inset 3px 0 0 var(--accent); }
.switcher-panel b { display: block; font-size: 14px; }
.switcher-panel span { display: block; font-size: 12.5px; color: var(--dim); margin-top: 2px; }

/* app.zetlyn.com: software, not a page. A dark sidebar, and a light place to work beside it. */
body.dash { background: var(--panel); }
.dash-grid { display: grid; grid-template-columns: 248px minmax(0, 1fr); min-height: 100vh; }
.dash-side { background: #10171c; color: #d5dce0; display: flex; flex-direction: column; gap: 4px; padding: 16px 14px;
  position: sticky; top: 0; height: 100vh; overflow-y: auto; }
.dash-side a { color: #d5dce0; }
.dash-head { display: flex; align-items: center; gap: 12px; padding: 2px 4px 18px; }
.dash-head .switcher summary { color: #d5dce0; }
.dash-brand { display: flex; align-items: center; gap: 8px; font-weight: 750; letter-spacing: -.03em; font-size: 18px; }
.dash-brand:hover { text-decoration: none; }
.dash-brand .brand-mark { width: 22px; height: 22px; filter: invert(1); }
.dash-brand .brand-area { color: #52c486; font-weight: 400; }
.dash-org { display: flex; align-items: center; gap: 10px; padding: 9px 10px; margin-bottom: 10px; border: 1px solid #26323a;
  font-weight: 600; font-size: 14px; position: relative; }
details.dash-org summary { list-style: none; display: flex; align-items: center; gap: 10px; cursor: pointer; width: 100%; }
details.dash-org summary::after { content: "▾"; margin-left: auto; color: #8b98a1; }
details.dash-org ul { position: absolute; top: 100%; left: -1px; right: -1px; z-index: 20; list-style: none; margin: 0; padding: 4px 0;
  background: #172128; border: 1px solid #26323a; }
details.dash-org li a { display: block; padding: 7px 12px; }
.dash-org-mark { width: 22px; height: 22px; display: grid; place-items: center; background: #52c486; color: #10171c;
  font-size: 12px; font-weight: 750; }
.dash-label { margin: 14px 10px 4px; font: 11px/normal ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  text-transform: uppercase; letter-spacing: .1em; color: #7f8c95; }
.dash-nav { display: flex; flex-direction: column; }
.dash-nav a { padding: 8px 10px; font-size: 14px; border-left: 2px solid transparent; }
.dash-nav a:hover { background: rgba(255,255,255,.05); text-decoration: none; }
.dash-nav a[aria-current] { background: rgba(255,255,255,.07); border-left-color: #52c486; color: #fff; }
.dash-foot { margin-top: auto; padding-top: 16px; border-top: 1px solid #26323a; display: flex; flex-direction: column; gap: 12px; }
.dash-me { display: flex; flex-direction: column; gap: 8px; padding: 0 10px; }
.dash-email { font-size: 13px; color: #aab6bd; overflow: hidden; text-overflow: ellipsis; }
.dash-me button, a.dash-signin { font: inherit; font-size: 13px; background: none; color: #d5dce0; border: 1px solid #3a4851;
  padding: 7px 10px; cursor: pointer; text-align: center; }
a.dash-signin { margin: 0 10px; }
.dash-me button:hover, a.dash-signin:hover { border-color: #d5dce0; text-decoration: none; }
.dash-small { display: flex; align-items: center; justify-content: space-between; padding: 0 10px; font-size: 12.5px; }
.dash-small .theme-toggle { color: #aab6bd; border-color: #3a4851; }
.dash-main { min-width: 0; display: flex; flex-direction: column; }
.dash-top { display: flex; align-items: center; gap: 16px; min-height: 56px; padding: 0 36px; border-bottom: 1px solid var(--line);
  background: var(--bg); }
.dash-top nav.crumbs ol { padding: 0; }
.dash-top .autoupdate { margin-left: auto; display: flex; align-items: center; gap: .45rem; font-size: .84rem; color: var(--dim);
  border: 1px solid var(--line); border-radius: 999px; padding: .25rem .75rem; background: var(--panel); }
.dash-top .autoupdate .dot { width: .5rem; height: .5rem; border-radius: 50%; background: var(--line-strong); }
.dash-top .autoupdate.on .dot { background: #2e7d32; }
.dash-tabs { display: flex; gap: .25rem; padding: 0 28px; border-bottom: 1px solid var(--line); background: var(--bg); }
.dash-tabs a { padding: 12px .8rem 10px; color: var(--dim); font-size: .9rem; border-bottom: 2px solid transparent; margin-bottom: -1px; }
.dash-tabs a:hover { color: var(--fg); text-decoration: none; }
.dash-tabs a.on { color: var(--fg); border-bottom-color: var(--accent); font-weight: 600; }
.dash main.dash-body { width: auto; max-width: 1180px; margin: 0; padding: 30px 36px 72px; }
@media (max-width: 52rem) {
  .dash-grid { grid-template-columns: 1fr; }
  .dash-side { position: static; height: auto; flex-direction: row; flex-wrap: wrap; align-items: center; gap: 8px; padding: 10px 14px; }
  .dash-head { padding: 0; }
  .dash-org { margin: 0; }
  .dash-label { display: none; }
  .dash-nav { flex-direction: row; overflow-x: auto; }
  .dash-nav a { border-left: 0; border-bottom: 2px solid transparent; }
  .dash-nav a[aria-current] { border-bottom-color: #52c486; }
  .dash-foot { margin: 0 0 0 auto; padding: 0; border: 0; flex-direction: row; }
  .dash-small { display: none; }
  .dash-top { padding: 0 16px; }
  .dash-tabs { padding: 0 8px; overflow-x: auto; }
  .dash main.dash-body { padding: 20px 16px 56px; }
}
footer.site-footer { width: min(1180px, calc(100% - 48px)); margin: 0 auto; padding: 0; height: 90px;
  border-top: 1px solid var(--line); display: flex; align-items: center; color: var(--dim);
  font: 11px/normal ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", monospace; }
footer.site-footer div { margin-left: auto; display: flex; gap: 22px; align-items: center; }
footer.site-footer a { color: var(--dim); }
footer.site-footer a:hover { color: var(--fg); text-decoration: none; }
.theme-toggle { border: 1px solid var(--line); background: none; color: var(--dim); cursor: pointer;
  padding: 7px 11px; font: 11px/normal ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", monospace;
  text-transform: uppercase; letter-spacing: .1em; }
.theme-toggle:hover { border-color: var(--fg); color: var(--fg); }
.subbar { border-bottom: 1px solid var(--line); background: var(--panel); }
.subbar .wrap { display: flex; align-items: center; justify-content: space-between; gap: 1rem;
  flex-wrap: wrap; min-height: 46px; }
nav.crumbs ol { list-style: none; margin: 0; padding: .7rem 0; display: flex; flex-wrap: wrap;
  gap: .35rem; font-size: .86rem; }
nav.crumbs li { display: flex; gap: .35rem; align-items: center; color: var(--dim); }
nav.crumbs li + li::before { content: "/"; color: var(--line-strong); }
nav.crumbs a { color: var(--dim); }
nav.crumbs a:hover { color: var(--fg); }
nav.crumbs [aria-current] { color: var(--fg); font-weight: 600; }
nav.tabs { display: flex; gap: .25rem; align-self: stretch; }
nav.tabs a { display: flex; align-items: center; padding: 0 .8rem; color: var(--dim); font-size: .88rem;
  border-bottom: 2px solid transparent; margin-bottom: -1px; }
nav.tabs a:hover { color: var(--fg); text-decoration: none; }
nav.tabs a.on { color: var(--fg); border-bottom-color: var(--accent); font-weight: 600; }
/* The head of a page: its name, what it is, and the numbers that say how it stands. */
.lede { font-size: 1.05rem; color: var(--dim); max-width: 46rem; margin: .4rem 0 0; }
.meta { font-size: .85rem; color: var(--dim); margin: .8rem 0 0; display: flex; gap: .4rem 1rem; flex-wrap: wrap; }
.meta .current::before, .meta .partial::before { content: ""; display: inline-block; width: .5rem; height: .5rem;
  border-radius: 50%; margin-right: .4rem; background: #2e7d32; vertical-align: .05em; }
.meta .partial::before { background: #c0392b; }
.meta { align-items: center; }
.meta form.update { margin-left: auto; }
.meta form.update button { padding: .3rem .8rem; font-size: .85rem; }
.stats { display: grid; grid-template-columns: repeat(auto-fit, minmax(10rem, 1fr)); gap: 1px; background: var(--line);
  margin: 1.8rem 0 1.4rem; border: 1px solid var(--line); border-radius: 10px; overflow: hidden; }
.stats > * { padding: 1rem 1.2rem; background: var(--panel); color: var(--fg); display: block; }
.stats a:hover { background: var(--bg); text-decoration: none; }
.stats b { display: block; font-size: 1.9rem; font-weight: 750; letter-spacing: -.03em; line-height: 1.1;
  font-variant-numeric: tabular-nums; }
.stats span { color: var(--dim); font-size: .82rem; }
.stats .hot b { color: var(--accent); }
/* The list of things. */
.list-head { display: flex; justify-content: space-between; align-items: baseline; gap: 1rem; flex-wrap: wrap; }
.list-head h2 { margin-bottom: .5rem; }
.scroll { overflow-x: auto; }
table.things td.thing a { color: var(--fg); font-weight: 600; }
table.things td.thing a:hover { color: var(--accent); text-decoration: none; }
table.things tbody tr:hover { background: var(--wash); }
table.things td:first-child, table.things th:first-child { padding-left: .6rem; }
.pager { display: flex; justify-content: space-between; align-items: center; gap: 1rem; margin: 1.2rem 0; font-size: .9rem; }
.pager a, .pager span.off { padding: .45rem .9rem; border: 1px solid var(--line); border-radius: 6px; background: var(--panel); color: var(--fg); }
.pager a:hover { border-color: var(--fg); text-decoration: none; }
.pager span.off { color: var(--dim); opacity: .5; }
.mono { font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace; font-size: .92em; }
button.primary, .button.primary { background: var(--accent); color: #fff; border-color: var(--accent); font-weight: 600; }
a.button { display: inline-block; padding: .55rem .9rem; border-radius: 6px; border: 1px solid var(--line); background: var(--panel); color: var(--fg); }
a.button:hover { text-decoration: none; border-color: var(--fg); }
a.button.primary:hover { color: #fff; }
button:hover { border-color: var(--fg); }
input[type=file] { font: inherit; font-size: .88rem; color: var(--dim); }
input[type=file]::file-selector-button { font: inherit; padding: .45rem .9rem; margin-right: .7rem; cursor: pointer;
  border-radius: 6px; border: 1px solid var(--line); background: var(--panel); color: var(--fg); }
input[type=file]::file-selector-button:hover { border-color: var(--fg); }
#jobs { position: fixed; left: 0; right: 0; bottom: 0; background: var(--panel); border-top: 1px solid var(--line);
  padding: .6rem 1rem calc(.6rem + env(safe-area-inset-bottom, 0px)); font-size: .9rem; }
#jobs .job { display: flex; gap: .8rem; align-items: center; flex-wrap: wrap; max-width: 60rem; margin: 0 auto; }
#jobs progress, main progress { flex: 1 1 10rem; min-width: 6rem; width: 100%; }
body.busy main { padding-bottom: 6rem; }
header.top .autoupdate { margin-left: auto; display: flex; align-items: center; gap: .45rem; font-size: .84rem;
  color: var(--dim); border: 1px solid var(--line); border-radius: 999px; padding: .25rem .75rem; background: var(--panel); }
header.top .autoupdate:hover { color: var(--fg); border-color: var(--fg); text-decoration: none; }
header.top .autoupdate .dot { width: .5rem; height: .5rem; border-radius: 50%; background: var(--line-strong); }
header.top .autoupdate.on .dot { background: #2e7d32; }
header.top .autoupdate + nav.links { margin-left: 1.4rem; }
label.choice { display: flex; gap: .7rem; align-items: baseline; max-width: 34rem; margin: .45rem 0; padding: .65rem .9rem;
  border: 1px solid var(--line); border-radius: 8px; background: var(--panel); cursor: pointer; }
label.choice:has(input:checked) { border-color: var(--accent); box-shadow: 0 0 0 1px var(--accent); }
details { margin: 2rem 0; } details > summary { cursor: pointer; font-weight: 600; margin-bottom: .6rem; }
.offer { display: flex; gap: .8rem; align-items: center; flex-wrap: wrap; border: 1px solid var(--accent); border-radius: 10px;
  padding: .9rem 1.1rem; background: var(--panel); margin: 1.2rem 0; }
.offer p { margin: 0; flex: 1 1 18rem; } .offer form { margin: 0; }
@media (max-width: 40rem) {
  header.top .wrap { height: auto; min-height: 70px; padding-block: 10px; gap: 1rem; }
  nav.links { gap: 14px; flex-wrap: wrap; justify-content: flex-end; font-size: 12px; }
  header.top nav.links { align-self: center; }
  header.top nav.links a { border-bottom: 0; margin-bottom: 0; }
  header.top nav.links a[aria-current] { text-decoration: underline 2px var(--accent); text-underline-offset: 7px; }
  nav.tabs { width: 100%; overflow-x: auto; }
  .stats b { font-size: 1.5rem; }
}
@media (max-width: 40rem) { main, .wrap, footer.site-footer { width: calc(100% - 28px); } main { padding: 1.2rem 0 4rem; } footer.site-footer { height: auto; padding: 26px 0; flex-wrap: wrap; gap: 12px; } }
"#;

pub fn urldecode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("20");
                out.push(u8::from_str_radix(hex, 16).unwrap_or(b' '));
                i += 3;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|c| match c {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (c as char).to_string()
            }
            b' ' => "+".to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

pub fn params(url: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Some((_, q)) = url.split_once('?') {
        for pair in q.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                out.insert(urldecode(k), urldecode(v));
            } else if !pair.is_empty() {
                out.insert(urldecode(pair), String::new());
            }
        }
    }
    out
}

pub fn flatten(pred: &Pred, out: &mut Vec<(String, Op, Lit)>) {
    match pred {
        Pred::And(a, b) | Pred::Or(a, b) => {
            flatten(a, out);
            flatten(b, out);
        }
        Pred::Cmp { left, op, right } => out.push((left.clone(), *op, right.clone())),
    }
}

fn link(base: &str, q: &str, view: &str, sort: &str, page: usize) -> String {
    let mut url = format!("{base}?q={}", urlencode(q));
    if !view.is_empty() {
        url.push_str(&format!("&view={}", urlencode(view)));
    }
    if !sort.is_empty() {
        url.push_str(&format!("&sort={}", urlencode(sort)));
    }
    if page > 1 {
        url.push_str(&format!("&page={page}"));
    }
    url
}


// Where this surface is mounted, so several of them can sit on one host. Empty at the root.
//
// Set before a request is answered and read from everywhere a link is written. Per thread,
// because the local app answers for every tracker in a workspace, each under its own prefix,
// one request at a time. The router strips it; every address a browser is given carries it.
thread_local! {
    static MOUNT: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// A prefix with a leading slash and no trailing one, or nothing at all.
pub fn mount(prefix: &str) {
    let p = prefix.trim().trim_end_matches('/');
    let p = if p.is_empty() {
        String::new()
    } else if let Some(rest) = p.strip_prefix('/') {
        format!("/{rest}")
    } else {
        format!("/{p}")
    };
    MOUNT.with(|m| *m.borrow_mut() = p);
}

pub fn mounted() -> String {
    MOUNT.with(|m| m.borrow().clone())
}

/// An address on this surface, as a browser has to ask for it.
pub fn at(path: &str) -> String {
    format!("{}{}", mounted(), path)
}

/// What was asked for, with the mount taken off, so the router matches one set of addresses
/// whatever the surface is mounted under. A request for the mount itself is a request for `/`.
pub fn unmount(url: &str) -> String {
    let m = mounted();
    if m.is_empty() {
        return url.to_string();
    }
    match url.strip_prefix(m.as_str()) {
        Some("") => "/".to_string(),
        Some(rest) if rest.starts_with('/') || rest.starts_with('?') => {
            if rest.starts_with('?') {
                format!("/{rest}")
            } else {
                rest.to_string()
            }
        }
        _ => url.to_string(),
    }
}

/// What every page sits in: whose home the mark leads to, the links beside it, and the part of
/// it this page is in (a tracker, with its own tabs). Set per request, like the mount, so a page
/// needs to say nothing but its title for its header and its breadcrumb to be right.
#[derive(Clone, Default)]
pub struct Frame {
    pub home: (String, String),
    pub nav: Vec<(String, String)>,
    pub section: Option<(String, String)>,
    pub tabs: Vec<(String, String)>,
    /// Where the app answers `/jobs`, for the bar of what runs in the background; `None` where
    /// nothing runs for this reader (a hub, a visitor).
    pub jobs: Option<String>,
    /// The automatic-update switch at the right of the header: its words, where it leads, on.
    pub status: Option<(String, String, bool)>,
    /// Which of `nav` is where the reader is.
    pub current: Option<String>,
    /// The footer: what it says at the left, and its links at the right, before the theme switch.
    pub note: String,
    pub footer: Vec<(String, String)>,
    /// Where the mark leads, where it is not this frame's home.
    pub brand: Option<String>,
    /// A crumb before the home one: the machine an organisation's workspace is part of.
    pub above: Option<(String, String)>,
    /// Who is signed in, at the right of the header, where somebody can be: `None` where nobody
    /// signs in (this machine's own app), `Some(None)` for a visitor, `Some(Some(email))` signed in.
    pub account: Option<Option<String>>,
    /// Which of the three this page is part of: `app` on app.zetlyn.com, empty everywhere else.
    /// It names the logo, colours the page and says where the other two are.
    pub area: String,
    /// The organisation a page is in, and the others whoever is signed in belongs to.
    pub org: Option<String>,
    pub orgs: Vec<(String, String)>,
    /// In the app's sidebar, what the links under the organisation are: its pages, or what anybody
    /// may read.
    pub side_title: String,
}

/// What each part is for, one line each, in the switch every part carries.
pub const AREA_ABOUT: &[(&str, &str)] = &[
    ("site", "What Zetlyn is, and how it works"),
    ("hub", "Public trackers and sources to take"),
    ("app", "Your organisation's trackers, run for you"),
];

/// The switch between the three parts: the same control, in the same place, in each.
pub fn switcher(current: &str) -> Markup {
    html! {
        details.switcher {
            summary aria-label="Zetlyn, its hub and its app" title="Zetlyn, its hub and its app" {
                svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true" {
                    @for y in [2, 7, 12] { @for x in [2, 7, 12] { rect x=(x) y=(y) width="3" height="3" {} } }
                }
            }
            div.switcher-panel {
                @for (key, name, href) in AREAS {
                    @let about = AREA_ABOUT.iter().find(|(k, _)| k == key).map(|(_, a)| *a).unwrap_or("");
                    a.on[*key == current] href=(href) { b { @if *key == "site" { "Zetlyn" } @else { "Zetlyn " (name) } } span { (about) } }
                }
            }
        }
    }
}

/// The sidebar's heading above its links.
pub fn frame_side(title: &str) {
    FRAME.with(|f| f.borrow_mut().side_title = title.to_string());
}

/// The three parts of Zetlyn, each a site of its own: what it is called beside the logo, where it is.
pub const AREAS: &[(&str, &str, &str)] = &[
    ("site", "Zetlyn", "https://zetlyn.com/"),
    ("hub", "Hub", "https://hub.zetlyn.com/"),
    ("app", "App", "https://app.zetlyn.com/"),
];

/// This page is part of `area`, in `org`, whose reader belongs to `orgs` too. In the app the mark
/// leads to the app's own front page, as the hub's leads to the hub's.
pub fn frame_area(area: &str, org: Option<String>, orgs: Vec<(String, String)>) {
    FRAME.with(|f| {
        let mut f = f.borrow_mut();
        f.area = area.to_string();
        f.org = org;
        f.orgs = orgs;
        if area == "app" {
            f.brand = Some("/".into());
        }
    });
}

/// Which of the header's links is where the reader is, where it is not the one `frame_site` named.
pub fn frame_current(current: Option<String>) {
    FRAME.with(|f| f.borrow_mut().current = current);
}

/// The crumb above home, and who is signed in. Both are set by `zetlyn hosting` and by nothing else.
pub fn frame_hosted(above: Option<(String, String)>, account: Option<Option<String>>) {
    FRAME.with(|f| {
        let mut f = f.borrow_mut();
        f.above = above;
        f.account = account;
    });
}

/// The website's header links and footer links, as zetlyn.com carries them (its `page.html` and
/// the labels in its `PAGES`). Under the hub at hub.zetlyn.com the header and the footer are the
/// website's, so a reader moving between them sees one site.
pub const SITE_NAV: &[(&str, &str)] = &[
    ("Docs", "https://zetlyn.com/docs"),
    ("Trackers", "https://zetlyn.com/trackers"),
    ("Sources", "https://zetlyn.com/sources"),
    ("Interface", "https://zetlyn.com/api"),
    ("Hub", "https://hub.zetlyn.com/"),
    ("App", "https://app.zetlyn.com/"),
];
pub const SITE_FOOTER: &[(&str, &str)] = &[
    ("Contact", "mailto:hello@zetlyn.com"),
    ("Privacy", "https://zetlyn.com/privacy"),
    ("Legal", "https://zetlyn.com/legal"),
];

fn owned(links: &[(&str, &str)]) -> Vec<(String, String)> {
    links.iter().map(|(l, h)| (l.to_string(), h.to_string())).collect()
}

/// The website's header and footer, with `current` the header link where the reader is.
pub fn frame_site(current: &str) {
    FRAME.with(|f| {
        let mut f = f.borrow_mut();
        f.nav = owned(SITE_NAV);
        f.current = Some(current.to_string());
        f.note = "© Zetlyn".into();
        f.footer = owned(SITE_FOOTER);
        f.brand = Some("https://zetlyn.com".into());
    });
}

thread_local! {
    static FRAME: std::cell::RefCell<Frame> = std::cell::RefCell::new(Frame::default());
}

/// The home the mark and the first crumb lead to, and the links at the right of the header.
pub fn frame_home(label: &str, href: &str, nav: Vec<(String, String)>) {
    FRAME.with(|f| {
        let mut f = f.borrow_mut();
        f.home = (label.to_string(), href.to_string());
        f.nav = nav;
    });
}

/// The part of the site this request is in, and its tabs; `None` when it is in none.
pub fn frame_section(section: Option<(String, String)>, tabs: Vec<(String, String)>) {
    FRAME.with(|f| {
        let mut f = f.borrow_mut();
        f.section = section;
        f.tabs = tabs;
    });
}


/// The background bar and the automatic-update switch, for the person the app answers.
pub fn frame_app(jobs: Option<String>, status: Option<(String, String, bool)>) {
    FRAME.with(|f| {
        let mut f = f.borrow_mut();
        f.jobs = jobs;
        f.status = status;
    });
}
pub fn frame() -> Frame {
    FRAME.with(|f| f.borrow().clone())
}

const MARK: &str = include_str!("mark.b64");
const FAVICON: &str = include_str!("favicon.b64");

/// The website's theme, unchanged (zetlyn.com: the script in `page.html`'s head, and the first
/// part of `app.js`): a choice is kept per browser, and a reader who made none gets their system's.
const THEME_EARLY: &str = r#"document.documentElement.className+=" js";try{var t=localStorage.getItem("theme");if(t)document.documentElement.dataset.theme=t}catch(e){}"#;
const THEME_TOGGLE: &str = r##"(function () {
  var root = document.documentElement;
  var button = document.getElementById("theme-toggle");
  var meta = document.querySelector('meta[name="theme-color"]');
  var BAR = { dark: "#11181d", light: "#f2efe7" };
  var media = window.matchMedia ? window.matchMedia("(prefers-color-scheme: light)") : null;

  function current() {
    return root.dataset.theme || (media && media.matches ? "light" : "dark");
  }

  function paint() {
    var theme = current();
    if (meta) meta.setAttribute("content", BAR[theme]);
    if (!button) return;
    button.textContent = theme === "dark" ? "☾ Dark" : "☀ Light";
    button.setAttribute("aria-label", "Theme: " + theme + ". Switch to " +
      (theme === "dark" ? "light" : "dark") + ".");
  }

  if (button) {
    button.addEventListener("click", function () {
      var next = current() === "dark" ? "light" : "dark";
      root.dataset.theme = next;
      try { localStorage.setItem("theme", next); } catch (e) {}
      paint();
    });
  }

  // The system preference can change while the page is open, and a page with no stored choice
  // follows it.
  if (media) {
    var follow = function () { if (!root.dataset.theme) paint(); };
    if (media.addEventListener) media.addEventListener("change", follow);
    else if (media.addListener) media.addListener(follow);
  }

  paint();
})();
"##;

pub fn shell(title: &str, body: Markup) -> String {
    let f = frame();
    // Standing alone (a tracker served by itself, a hub), the part it is in is its home.
    let home_href = if !f.home.1.is_empty() { f.home.1.clone() } else { f.section.as_ref().map(|s| s.1.clone()).unwrap_or_else(|| "/".into()) };
    let brand_href = f.brand.clone().unwrap_or_else(|| home_href.clone());
    // Home, the part this is in, and this page: each named once.
    let mut crumbs: Vec<(String, Option<String>)> = Vec::new();
    if let Some((label, href)) = &f.above {
        crumbs.push((label.clone(), Some(href.clone())));
    }
    if !f.home.1.is_empty() {
        crumbs.push((f.home.0.clone(), Some(f.home.1.clone())));
    }
    if let Some((label, href)) = &f.section {
        crumbs.push((label.clone(), Some(href.clone())));
    }
    // The home page is where the crumbs start, not one of them.
    if title != "Zetlyn" && crumbs.iter().all(|(l, _)| l != title) {
        crumbs.push((title.to_string(), None));
    }
    // The last is where the reader is.
    if let Some(last) = crumbs.last_mut() {
        if last.0 == title {
            last.1 = None;
        }
    }
    // The app is software, not a page: a sidebar and a place to work, and nothing of the website.
    if f.area == "app" {
        return dashboard(title, body, &f, &crumbs, &brand_href);
    }
    let page = html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="theme-color" content="#f2efe7";
                // As the website does: a theme already chosen is applied before the first paint.
                script { (maud::PreEscaped(THEME_EARLY)) }
                title { (title) @if title != "Zetlyn" { " · Zetlyn" @if f.area == "app" { " App" } } }
                link rel="icon" type="image/png" href={"data:image/png;base64," (FAVICON)};
                link rel="stylesheet" href={(at("/style.css")) "?v=" (env!("CARGO_PKG_VERSION"))};
            }
            body class=(if f.area.is_empty() { String::new() } else { format!("area-{}", f.area) }) {
                header.top {
                    div.wrap {
                        a.brand href=(brand_href) aria-label="Zetlyn home" {
                            img.brand-mark src={"data:image/png;base64," (MARK)} alt="";
                            span { "Zetlyn" }
                            @if let Some((_, name, _)) = AREAS.iter().find(|(a, _, _)| *a == f.area && *a != "site") {
                                span.brand-area { (name) }
                            }
                        }
                        // Which organisation this is, and the others whoever is signed in belongs to.
                        @if let Some(org) = &f.org {
                            @if f.orgs.len() > 1 {
                                details.org-switch {
                                    summary { (org) }
                                    ul { @for (label, href) in &f.orgs { li { a href=(href) { (label) } } } }
                                }
                            } @else {
                                span.org-name { (org) }
                            }
                        }
                        @if let Some((words, href, on)) = &f.status {
                            a.autoupdate.on[*on] href=(href) title="Automatic updates" { span.dot {} (words) }
                        }
                        @if !f.nav.is_empty() {
                            nav.links {
                                @for (label, href) in &f.nav {
                                    @if f.current.as_deref() == Some(label.as_str()) {
                                        a href=(href) aria-current="page" { (label) }
                                    } @else {
                                        a href=(href) { (label) }
                                    }
                                }
                            }
                        }
                        @match &f.account {
                            Some(Some(email)) => {
                                form.account method="post" action="/signout" {
                                    span.dim { (email) }
                                    button type="submit" { "Sign out" }
                                }
                            }
                            Some(None) => { a.account href="/signin" { "Sign in" } }
                            None => {}
                        }
                        // The other two parts of Zetlyn, a click away from wherever one is.
                        @if !f.area.is_empty() {
                            nav.areas aria-label="Zetlyn" {
                                @for (key, name, href) in AREAS.iter().filter(|(a, _, _)| *a != f.area) {
                                    a.area-link.{"to-" (key)} href=(href) { (name) }
                                }
                            }
                        }
                    }
                }
                @if crumbs.len() > 1 || !f.tabs.is_empty() {
                    div.subbar {
                        div.wrap {
                            nav.crumbs aria-label="Breadcrumb" {
                                ol {
                                    @for (label, href) in &crumbs {
                                        li {
                                            @match href {
                                                Some(h) => a href=(h) { (label) },
                                                None => span aria-current="page" { (label) },
                                            }
                                        }
                                    }
                                }
                            }
                            @if !f.tabs.is_empty() {
                                nav.tabs {
                                    @for (label, href) in &f.tabs {
                                        @if label == title || (f.section.as_ref().is_some_and(|s| &s.0 == title) && Some(href) == f.section.as_ref().map(|s| &s.1)) {
                                            a.on href=(href) aria-current="page" { (label) }
                                        } @else {
                                            a href=(href) { (label) }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                main { (body) }
                footer.site-footer {
                    span { @if f.note.is_empty() { "Zetlyn " (env!("CARGO_PKG_VERSION")) } @else { (f.note) } }
                    div {
                        @for (label, href) in &f.footer { a href=(href) { (label) } }
                        button.theme-toggle type="button" id="theme-toggle" { "Theme" }
                    }
                }
                script { (maud::PreEscaped(THEME_TOGGLE)) }
                @if let Some(jobs) = &f.jobs {
                    div #jobs data-at=(jobs) hidden {}
                    (maud::PreEscaped(crate::app::BAR_SCRIPT))
                }
            }
        }
    };
    page.into_string()
}


/// app.zetlyn.com: a dark sidebar with the switch, the organisation and its pages, and beside it a
/// slim bar with where the reader is, the tabs of what they are in, and the work.
fn dashboard(title: &str, body: Markup, f: &Frame, crumbs: &[(String, Option<String>)], brand_href: &str) -> String {
    let tab_on = |label: &String, href: &String| {
        label == title || (f.section.as_ref().is_some_and(|s| s.0 == title) && Some(href) == f.section.as_ref().map(|s| &s.1))
    };
    let page = html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="theme-color" content="#10171c";
                script { (maud::PreEscaped(THEME_EARLY)) }
                title { (title) @if title != "Zetlyn" { " · " } "Zetlyn App" }
                link rel="icon" type="image/png" href={"data:image/png;base64," (FAVICON)};
                link rel="stylesheet" href={(at("/style.css")) "?v=" (env!("CARGO_PKG_VERSION"))};
            }
            body.area-app.dash {
                div.dash-grid {
                    aside.dash-side {
                        div.dash-head {
                            (switcher("app"))
                            a.dash-brand href=(brand_href) aria-label="Zetlyn App" {
                                img.brand-mark src={"data:image/png;base64," (MARK)} alt="";
                                span { "Zetlyn" } span.brand-area { "App" }
                            }
                        }
                        @if let Some(org) = &f.org {
                            @if f.orgs.len() > 1 {
                                details.dash-org {
                                    summary { span.dash-org-mark { (org.chars().next().unwrap_or('·').to_uppercase().to_string()) } (org) }
                                    ul { @for (label, href) in &f.orgs { li { a href=(href) { (label) } } } }
                                }
                            } @else {
                                div.dash-org { span.dash-org-mark { (org.chars().next().unwrap_or('·').to_uppercase().to_string()) } (org) }
                            }
                        }
                        @if !f.side_title.is_empty() { p.dash-label { (f.side_title) } }
                        nav.dash-nav {
                            @for (label, href) in &f.nav {
                                @if f.current.as_deref() == Some(label.as_str()) {
                                    a href=(href) aria-current="page" { (label) }
                                } @else {
                                    a href=(href) { (label) }
                                }
                            }
                        }
                        // At the machine's front page, somebody signed in sees where they belong.
                        @if f.org.is_none() && !f.orgs.is_empty() {
                            p.dash-label { "Your organisations" }
                            nav.dash-nav { @for (label, href) in &f.orgs { a href=(href) { (label) } } }
                        }
                        div.dash-foot {
                            @match &f.account {
                                Some(Some(email)) => {
                                    div.dash-me {
                                        span.dash-email { (email) }
                                        form method="post" action="/signout" { button type="submit" { "Sign out" } }
                                    }
                                }
                                _ => { a.dash-signin href="/signin" { "Sign in" } }
                            }
                            div.dash-small {
                                a href="https://zetlyn.com/docs" { "Docs" }
                                button.theme-toggle type="button" id="theme-toggle" { "Theme" }
                            }
                        }
                    }
                    div.dash-main {
                        header.dash-top {
                            nav.crumbs aria-label="Breadcrumb" {
                                ol {
                                    @for (label, href) in crumbs {
                                        li { @match href { Some(h) => a href=(h) { (label) }, None => span aria-current="page" { (label) } } }
                                    }
                                }
                            }
                            @if let Some((words, href, on)) = &f.status {
                                a.autoupdate.on[*on] href=(href) title="Automatic updates" { span.dot {} (words) }
                            }
                        }
                        @if !f.tabs.is_empty() {
                            nav.dash-tabs {
                                @for (label, href) in &f.tabs {
                                    @if tab_on(label, href) { a.on href=(href) aria-current="page" { (label) } } @else { a href=(href) { (label) } }
                                }
                            }
                        }
                        main.dash-body { (body) }
                    }
                }
                script { (maud::PreEscaped(THEME_TOGGLE)) }
                @if let Some(jobs) = &f.jobs {
                    div #jobs data-at=(jobs) hidden {}
                    (maud::PreEscaped(crate::app::BAR_SCRIPT))
                }
            }
        }
    };
    page.into_string()
}

fn value_cell(v: &Value) -> Markup {
    html! { (v.display()) }
}

fn column_of(hit: &Hit, name: &str) -> Markup {
    match name {
        "known" => html! { (hit.known) },
        "kind" => html! { (hit.kind) },
        "title" => html! { a href={(at("/claim/")) (hit.record_id)} { (hit.title) } },
        other => match hit.fields.get(other) {
            Some(v) => value_cell(v),
            None => html! { span.dim { "—" } },
        },
    }
}

fn numeric(ds: &Source, name: &str) -> bool {
    ds.decl
        .records
        .fields
        .get(name)
        .map(|f| matches!(f.kind, crate::sourcedecl::PropertyType::Number))
        .unwrap_or(false)
}

fn table(ds: &Source, hits: &[Hit], view: Option<&View>, q: &str, sort: &str) -> Markup {
    let mut cols: Vec<String> = view.map(|v| v.columns.clone()).unwrap_or_default();
    if cols.is_empty() {
        cols = vec!["known".into()];
    }
    let view_name = view.map(|v| v.name.clone()).unwrap_or_default();
    html! {
        table {
            thead { tr {
                th { "Title" }
                @for c in &cols {
                    th {
                        a href=(link("/", q, &view_name, &format!("{c} desc"), 1)) { (c) }
                    }
                }
            } }
            tbody {
                @for hit in hits {
                    tr {
                        td {
                            a href={(at("/claim/")) (hit.record_id)} { (hit.title) }
                            @if !hit.why_text.is_empty() || hit.why_id.is_some() {
                                div.why {
                                    @if let Some(id) = &hit.why_id {
                                        "identifier " (id.value)
                                    } @else {
                                        "matched " (hit.why_text.join(", "))
                                    }
                                }
                            }
                        }
                        @for c in &cols {
                            td.num[numeric(ds, c)] { (column_of(hit, c)) }
                        }
                    }
                }
            }
        }
        @if hits.is_empty() {
            p.dim { "Nothing here." }
        }
        @let _ = sort;
    }
}

fn overview(ds: &Source, url: &str) -> String {
    let p = params(url);
    let q = p.get("q").cloned().unwrap_or_default();
    let sort = p.get("sort").cloned().unwrap_or_default();
    let page: usize = p
        .get("page")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
        .max(1);
    let view_name = p
        .get("view")
        .cloned()
        .or_else(|| ds.decl.default_view().map(|v| v.name.clone()))
        .unwrap_or_default();

    let (text, pred) = expr::parse_query(&q);
    let limit = 50;
    let query = Query {
        text,
        pred: pred.clone(),
        ids: Vec::new(),
        seen_before: None,
        view: if view_name.is_empty() {
            None
        } else {
            Some(view_name.clone())
        },
        sort: if sort.is_empty() {
            None
        } else {
            Some(sort.clone())
        },
        limit,
        offset: (page - 1) * limit,
    };
    let view = ds.decl.view(&view_name);
    let (total, hits, unanswered) = match ds.search(&query) {
        Ok(v) => v,
        Err(e) => (0, Vec::new(), crate::store::Unanswered(vec![e])),
    };

    let d = &ds.decl;
    let report = ds.store.run_report(ds.store.last_run());
    let state = ds.state();
    let summaries = ds.store.fields(d);
    let total_records = ds.store.count();

    let mut active = Vec::new();
    if let Some(pr) = &pred {
        flatten(pr, &mut active);
    }

    let facets: Vec<String> = view
        .map(|v| v.facets.clone())
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| summaries.iter().map(|f| f.name.clone()).take(4).collect());

    let body = html! {
        h1 { (d.title) }
        @if !d.about.is_empty() { p.about { (d.about) } }
        p.state.(state) {
            (state) " · " (total_records) " claims"
            @if let Some(r) = &report {
                " · update " (r.id) " " (r.started)
                @if r.added + r.changed + r.removed > 0 {
                    " · +" (r.added) " ~" (r.changed) " −" (r.removed)
                }
            }
        }

        @if let Some(r) = &report {
            @if let Some(why) = &r.refused {
                div.note { "The last update was refused and the store was not replaced: " (why) }
            }
            @if let Some(err) = &r.error {
                div.note { "The last update did not finish: " (err) }
            }
            @if r.no_text > 0 || r.unparsed > 0 || r.no_known > 0 {
                div.note {
                    @if r.no_text > 0 { (r.no_text) " claims came out with no text. " }
                    @if r.no_known > 0 { (r.no_known) " took their date from the file. " }
                    @if r.unparsed > 0 {
                        (r.unparsed) " values did not parse as their type"
                        @if let Some(n) = &r.note { @if !n.is_empty() { " — " (n) } }
                        "."
                    }
                }
            }
        }

        form.bar method="get" action=(at("/")) {
            input type="search" name="q" value=(q) placeholder="Search, or filter with name=value";
            @if !view_name.is_empty() { input type="hidden" name="view" value=(view_name); }
            button type="submit" { "Search" }
        }

        @if q.is_empty() && !d.search.examples.is_empty() {
            p.bar {
                span.dim { "Try:" }
                @for ex in &d.search.examples {
                    a.chip href=(link("/", ex, &view_name, "", 1)) { (ex) }
                }
            }
        }

        @if !active.is_empty() {
            p.bar {
                @for (name, op, lit) in &active {
                    @let one = format!("{name}{}{}", op.sql(), lit.display());
                    @let without = q.replace(&one, "").split_whitespace()
                        .collect::<Vec<_>>().join(" ");
                    a.chip.on href=(link("/", &without, &view_name, &sort, 1)) {
                        (name) (op.sql()) (lit.display()) " ✕"
                    }
                }
            }
        }

        @if !unanswered.0.is_empty() {
            div.note {
                "Not answered here: "
                @for (i, u) in unanswered.0.iter().enumerate() {
                    @if i > 0 { "; " }
                    (u)
                }
            }
        }

        @if d.view.len() > 1 {
            p.bar {
                @for v in &d.view {
                    @let t = if v.title.is_empty() { v.name.clone() } else { v.title.clone() };
                    @if v.name == view_name {
                        span.chip.on { (t) }
                    } @else {
                        a.chip href=(link("/", &q, &v.name, "", 1)) { (t) }
                    }
                }
            }
        }

        h2 { (total) " of " (total_records) }
        (table(ds, &hits, view, &q, &sort))

        @if total > limit as u64 {
            p.bar {
                @if page > 1 {
                    a href=(link("/", &q, &view_name, &sort, page - 1)) { "← previous" }
                }
                span.dim { "page " (page) " of " ((total as usize).div_ceil(limit)) }
                @if (page * limit) < total as usize {
                    a href=(link("/", &q, &view_name, &sort, page + 1)) { "next →" }
                }
            }
        }

        @if !facets.is_empty() {
            h2 { "Facets" }
            div.grid {
                @for name in &facets {
                    @let counts = ds.facet(&query, name, 8);
                    @let summary = summaries.iter().find(|s| &s.name == name);
                    @if !counts.is_empty() {
                        div.card {
                            h4 {
                                (name)
                                @if let Some(s) = summary {
                                    " " span.cover {
                                        (s.records) " of " (total_records)
                                    }
                                }
                            }
                            @for (v, n) in &counts {
                                div.facet {
                                    a href=(link("/", format!("{q} {name}={v}").trim(),
                                                 &view_name, &sort, 1)) { (v) }
                                    span.n { (n) }
                                }
                            }
                        }
                    }
                }
            }
        }

        h2 { "What this source holds" }
        div.grid {
            div.card {
                h4 { "Identifiers" }
                @let schemes = ds.store.schemes();
                @if schemes.is_empty() {
                    p.dim { "None. Claims are addressed by where they came from." }
                } @else {
                    @for (s, n) in &schemes {
                        div.facet { span { (s) } span.n { (n) } }
                    }
                }
            }
            div.card {
                h4 { "Properties" }
                @for f in &summaries {
                    div.facet {
                        span { (f.name) " " span.dim { (f.kind) } }
                        span.n {
                            (f.records)
                            @if let (Some(lo), Some(hi)) = (&f.min, &f.max) {
                                " · " (lo) "–" (hi)
                            }
                        }
                    }
                }
                @if summaries.is_empty() { p.dim { "None declared." } }
            }
            div.card {
                h4 { "Can answer" }
                p { @for c in ds.can() { span.chip { (c) } " " } }
                @if !d.search.compare.is_empty() {
                    p.dim { "Compares: " (d.search.compare.join(", ")) }
                }
            }
        }

        footer {
            (d.name) " · " (d.source.kind_name()) " · kind " (d.kind)
            " · " a href=(at("/api/describe")) { "describe" }
            " · " a href=(at("/changes")) { "changes" }
        }
    };
    shell(&d.title, body)
}

fn record_page(ds: &Source, id: &str, url: &str) -> Option<String> {
    let asked = params(url).get("as_of").cloned();
    // `as_of` shows a claim as it stood, from the revisions the source kept. It reads one
    // claim: the text index is current, so it does not make a whole query answer as of a date.
    let then = asked.as_deref().and_then(|at| ds.store.as_of(id, at));
    let mut rec = ds.fetch(&[id.to_string()], true).into_iter().next()?;
    let answered = ds
        .store
        .run_report(ds.store.last_run())
        .and_then(|r| r.finished);
    let found = then.is_some();
    if let Some((title, fields)) = then {
        rec.title = title;
        rec.fields = fields;
    }
    let d = &ds.decl;
    let body = html! {
        p { a href=(at("/")) { "← " (d.title) } }
        h1 { (rec.title) }
        @if let Some(when) = &asked {
            @if found {
                div.note { "As it stood on " (when) ". "
                    a href={(at("/claim/")) (rec.record_id)} { "Now" } }
            } @else {
                div.note { "No version of this claim from on or before " (when)
                    " is kept, so these are today's values." }
            }
        }
        p.state {
            span.chip { (rec.kind) } " "
            @for i in &rec.ids { span.chip { (i.scheme) " " (i.value) } " " }
            span.dim { "known " (rec.known) }
        }
        @if let Some(u) = &rec.url {
            p { a href=(u) { (u) } }
        }
        @if !rec.fields.is_empty() {
            h2 { "Properties" }
            table {
                tbody {
                    @for (name, value) in &rec.fields {
                        tr {
                            th style="width: 12rem" { (name) }
                            td {
                                (value.display())
                                @if let Value::Code { code, vocabulary } = value {
                                    @if let Some(v) = vocabulary {
                                        @if let Some(means) = d.vocabulary.get(v)
                                            .and_then(|m| m.get(code)) {
                                            div.why { (means) }
                                        }
                                    }
                                }
                                (receipt(&rec, name, &d.title, answered.as_deref()))
                            }
                        }
                    }
                }
            }
        }
        @if !rec.text.trim().is_empty() {
            h2 { "Text" }
            div.text { (rec.text) }
        }
        footer {
            "from " (rec.from.address())
            " · " (rec.hash)
            " · " a href={(at("/api/fetch?id=")) (rec.record_id)} { "json" }
        }
    };
    Some(shell(&rec.title, body))
}

fn changes_page(ds: &Source, url: &str) -> String {
    let p = params(url);
    let since: i64 = p
        .get("since")
        .and_then(|s| s.parse().ok())
        .unwrap_or(ds.mark() - 1);
    let j = ds.changes(since, 200);
    let empty = Vec::new();
    let changed = j["changed"].as_array().unwrap_or(&empty);
    let removed = j["removed"].as_array().unwrap_or(&empty);
    let body = html! {
        p { a href=(at("/")) { "← " (ds.decl.title) } }
        h1 { "Changes" }
        p.dim { "Since update " (since) ". The mark now is " (ds.mark()) "." }
        @if changed.is_empty() && removed.is_empty() {
            p.dim { "Nothing since then." }
        }
        table { tbody {
            @for c in changed {
                tr {
                    td style="width: 6rem" { span.chip { (c["how"].as_str().unwrap_or("")) } }
                    td { a href={(at("/claim/")) (c["claim_id"].as_str().unwrap_or(""))} {
                        (c["title"].as_str().unwrap_or("")) } }
                }
            }
            @for c in removed {
                tr {
                    td { span.chip { "removed" } }
                    td.dim { (c["title"].as_str().unwrap_or("")) }
                }
            }
        } }
    };
    shell("Changes", body)
}

/// The same six calls, over HTTP. One interface and not two. The second value is true when the
/// path names no call, because a caller who mistypes one should hear that and not a search.
fn api(ds: &Source, path: &str, url: &str) -> (J, bool) {
    let p = params(url);
    let q = || {
        let (text, pred) = expr::parse_query(p.get("q").map(String::as_str).unwrap_or(""));
        Query {
            text,
            pred,
            view: p.get("view").cloned(),
            ids: p
                .get("ids")
                .map(|s| s.split(',').map(str::to_string).collect())
                .unwrap_or_default(),
            seen_before: p.get("seen_before").cloned(),
            sort: p.get("sort").cloned(),
            limit: p.get("limit").and_then(|s| s.parse().ok()).unwrap_or(50),
            offset: p.get("offset").and_then(|s| s.parse().ok()).unwrap_or(0),
        }
    };
    let answer = match path {
        "/api/describe" => ds.describe(),
        "/api/mark" => json!({ "mark": ds.mark() }),
        "/api/changes" => ds.changes(
            p.get("since").and_then(|s| s.parse().ok()).unwrap_or(0),
            p.get("limit").and_then(|s| s.parse().ok()).unwrap_or(200),
        ),
        "/api/facet" => {
            let field = p.get("property").cloned().unwrap_or_default();
            let counts = ds.facet(
                &q(),
                &field,
                p.get("limit").and_then(|s| s.parse().ok()).unwrap_or(50),
            );
            json!({ "property": field, "values":
                J::Array(counts.iter().map(|(v, n)| json!({ "value": v, "claims": n })).collect()) })
        }
        "/api/fetch" => {
            let ids: Vec<String> = p
                .get("id")
                .map(|s| s.split(',').map(str::to_string).collect())
                .unwrap_or_default();
            let versions = p.get("versions").is_some_and(|v| v == "1" || v == "true");
            json!({ "claims": J::Array(ds.fetch(&ids, versions).iter().map(|r| r.to_json()).collect()) })
        }
        "/api/search" => {
            let query = q();
            match ds.search(&query) {
                Ok((total, hits, unanswered)) => json!({
                    "total": total,
                    "unanswered": unanswered.0,
                    "hits": J::Array(hits.iter().map(|h| json!({
                        "claim_id": h.record_id,
                        "rank": h.rank,
                        "why": { "text": h.why_text, "property": h.why_field,
                                 "id": h.why_id.as_ref().map(|i| json!({
                                     "scheme": i.scheme, "value": i.value })) },
                        "title": h.title,
                        "url": h.url,
                        "kind": h.kind,
                        "known": h.known,
                        "ids": J::Array(h.ids.iter().map(|i| json!({
                            "scheme": i.scheme, "value": i.value })).collect()),
                        "properties": J::Object(h.fields.iter()
                            .map(|(k, v)| (k.clone(), v.to_json())).collect()),
                    })).collect()),
                }),
                Err(e) => json!({ "error": e }),
            }
        }
        _ => {
            return (
                json!({ "error": "no such call", "calls": [
                    "/api/describe", "/api/search", "/api/fetch",
                    "/api/facet", "/api/changes", "/api/mark",
                ] }),
                true,
            )
        }
    };
    (answer, false)
}

pub fn serve(ds: Source, addr: &str) -> Result<(), String> {
    let server = tiny_http::Server::http(addr).map_err(|e| e.to_string())?;
    println!("{} on http://{addr}", ds.decl.name);
    for request in server.incoming_requests() {
        let url = unmount(request.url());
        let path = url.split('?').next().unwrap_or("/").to_string();
        // The same rule as the tracker surface: a miss is a 404, and only the front page is the
        // front page. Every other address that matches nothing is nothing.
        let mut status = 200u16;
        let (body, content_type) = if path == "/style.css" {
            (STYLE.to_string(), "text/css; charset=utf-8")
        } else if path.starts_with("/api/") {
            let (answer, no_such_call) = api(&ds, &path, &url);
            if no_such_call {
                status = 404;
            }
            (answer.to_string(), "application/json")
        } else if let Some(id) = path.strip_prefix("/claim/") {
            match record_page(&ds, id, &url) {
                Some(html) => (html, "text/html; charset=utf-8"),
                None => {
                    status = 404;
                    (
                        shell("Not here", html! { h1 { "No such claim" } }),
                        "text/html; charset=utf-8",
                    )
                }
            }
        } else if path == "/changes" {
            (changes_page(&ds, &url), "text/html; charset=utf-8")
        } else if path != "/" {
            status = 404;
            (
                shell(
                    "Nothing here",
                    html! {
                        p { a href=(at("/")) { "← " (ds.decl.title) } }
                        h1 { "Nothing here at that address" }
                    },
                ),
                "text/html; charset=utf-8",
            )
        } else {
            (overview(&ds, &url), "text/html; charset=utf-8")
        };
        let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes())
            .map_err(|_| "bad header".to_string())?;
        let response = tiny_http::Response::from_string(body)
            .with_status_code(status)
            .with_header(header);
        let _ = request.respond(response);
    }
    Ok(())
}

#[allow(dead_code)]
fn unused(_: PreEscaped<String>) {}

/// Where one value came from: the words its source used and the expression that read them, since
/// when it has said so, when the source last answered, what the source handed over, and every
/// value it said before. Every fact has a receipt, and this is it.
pub fn receipt(claim: &Claim, property: &str, source: &str, answered: Option<&str>) -> Markup {
    let said = &claim.excerpt.as_ref().map(|e| e["properties"][property].clone()).unwrap_or(J::Null);
    let words: Vec<String> = said["raw"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let now = claim.fields.get(property).map(|v| v.display()).unwrap_or_default();
    // The value over time, one row per change rather than one per version: a version that moved
    // another property did not move this one.
    let mut changes: Vec<(String, String)> = Vec::new();
    for v in &claim.versions {
        let value = crate::claim::Value::from_json(&v.properties[property])
            .map(|x| x.display())
            .unwrap_or_else(|| "—".into());
        if changes.last().map(|(_, last)| last != &value).unwrap_or(true) {
            changes.push((v.at.clone(), value));
        }
    }
    let since = changes.last().filter(|(_, v)| *v == now).map(|(at, _)| at.clone());
    let original = claim.url.clone().or_else(|| claim.from.url.clone());
    let row = claim.excerpt.as_ref().and_then(|e| e.get("row")).cloned();
    let too_large = claim.excerpt.as_ref().and_then(|e| e["row_bytes"].as_u64());
    html! {
        details.receipt {
            summary { "receipt" }
            dl {
                dt { "Source" } dd { (source) }
                @if !words.is_empty() {
                    dt { "Its words" } dd { code { (words.join(", ")) } }
                }
                @if let Some(from) = said["from"].as_str() {
                    dt { "Read by" } dd { code { (from) } }
                }
                @if let Some(at) = &since {
                    dt { "Said since" } dd { (stamp(at)) }
                }
                @if let Some(at) = answered {
                    dt { "Last answered" } dd { (stamp(at)) }
                }
                @if let Some(u) = &original {
                    dt { "Original" } dd { a href=(u) { "open at the source" } }
                }
            }
            @if changes.len() > 1 {
                table { tbody {
                    @for (at, value) in changes.iter().rev() {
                        tr { td.dim { (stamp(at)) } td { (value) } }
                    }
                } }
            }
            @if let Some(row) = &row {
                details {
                    summary { "What the source handed over" }
                    pre { (serde_json::to_string_pretty(row).unwrap_or_default()) }
                }
            } @else if let Some(bytes) = too_large {
                p.dim { "The source handed over " (bytes) " bytes for this, which is kept whole at the source and not here." }
            }
            @if claim.excerpt.is_none() {
                p.dim { "This source has not kept a receipt for this claim yet. The next update that reads it will." }
            }
        }
    }
}

/// `2026-09-28T18:42:10Z` as a person reads it.
fn stamp(at: &str) -> String {
    match (at.get(..10), at.get(11..16)) {
        (Some(d), Some(t)) => format!("{d} {t} UTC"),
        _ => at.to_string(),
    }
}
