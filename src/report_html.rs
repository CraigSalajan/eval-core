//! Self-contained HTML comparison report over the persisted eval runs.
//!
//! [`generate_report`] loads every `*.json` [`RunRecord`](crate::report::RunRecord) in a results
//! directory, embeds them verbatim as a JSON blob inside a SINGLE `report.html` (no CDN, no external
//! scripts/fonts — it opens offline by double-click), and renders, with vanilla JS + inline CSS, a
//! layout that scales to comparing 10–50 models at a glance:
//!
//! - a sticky **controls bar** (name filter, latest/best/all-runs view toggle, sort selector),
//! - a **leaderboard** (one row per model in the active view; ranked by accuracy with color-scaled
//!   accuracy bars + latency/token heat, sortable columns, row-click run-history detail),
//! - a **model × case heatmap** (rows = models, columns = the union of case names, green/red cells,
//!   sticky first column + header, per-case pass-rate footer + per-model accuracy column),
//! - a collapsible **raw runs** list (every saved run for reference).
//!
//! Correctness over polish: the JS recomputes EVERY aggregate client-side from the embedded records
//! (accuracy = passed/total; latency p50/p95/mean from each outcome's `latency.secs + nanos/1e9`; mean
//! tokens over outcomes with a non-null `tokens`), so the generator only embeds the data + page shell.
//! None of EvalReport's Rust-side aggregates are serialized — only `outcomes` — so JS must derive them.

use std::path::{Path, PathBuf};

use crate::report::RunRecord;

/// Load every `*.json` run record under `results_dir`, embed them in a self-contained `report.html`
/// written to `results_dir/report.html`, and return that path. Files that fail to parse as a
/// [`RunRecord`] are skipped with a warning (a stray/old-format file shouldn't sink the whole report).
pub fn generate_report(results_dir: &Path) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(results_dir)
        .map_err(|e| anyhow::anyhow!("creating results dir {}: {e}", results_dir.display()))?;

    let records = load_records(results_dir)?;
    let json = serde_json::to_string(&records)
        .map_err(|e| anyhow::anyhow!("serializing run records: {e}"))?;

    let html = render_page(&json, records.len());
    let out = results_dir.join("report.html");
    std::fs::write(&out, html).map_err(|e| anyhow::anyhow!("writing {}: {e}", out.display()))?;
    Ok(out)
}

/// Read + parse every `*.json` (except the report itself, which is `.html`) in `dir` into
/// [`RunRecord`]s. Unparseable files are skipped (warned), not fatal.
fn load_records(dir: &Path) -> anyhow::Result<Vec<RunRecord>> {
    let mut records = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        // An empty/missing dir yields an empty report rather than an error (the bin creates it).
        Err(_) => return Ok(records),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_json = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("json"));
        if !is_json {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<RunRecord>(&text) {
                Ok(rec) => records.push(rec),
                Err(e) => tracing::warn!("skipping {} (not a RunRecord: {e})", path.display()),
            },
            Err(e) => tracing::warn!("skipping {} (read error: {e})", path.display()),
        }
    }
    // Newest first by the sortable file-timestamp (client JS re-sorts anyway, but a stable default
    // order makes the raw embedded blob readable).
    records.sort_by(|a, b| b.timestamp_file.cmp(&a.timestamp_file));
    Ok(records)
}

/// Escape a JSON document so it can be embedded as a double-quoted JS string literal inside a
/// `<script>` element and recovered with `JSON.parse("…")`.
///
/// Two concerns, in this order:
/// 1. **JS string-literal** safety: escape backslashes, double-quotes, and newlines/CRs so the
///    surrounding `"…"` literal isn't terminated early or broken across lines.
/// 2. **HTML-parser** safety: neutralize `<`, `>`, and `&` (so a `</script>` or `<!--` inside the
///    data can't end the script element). These become `\uXXXX` — still valid once `JSON.parse` runs.
///
/// Backslash must be escaped FIRST so the `\u` sequences we add in step 2 aren't themselves doubled.
fn escape_for_script(json: &str) -> String {
    json.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
}

/// The full static page: inline CSS, the embedded data blob, and the vanilla-JS renderer.
///
/// `CSS`/`JS` are passed as named `format!` args (NOT inlined into the template literal), so their
/// own `{`/`}` characters never need brace-escaping. The only braces in the format string below are
/// the four `{...}` placeholders.
fn render_page(json_blob: &str, count: usize) -> String {
    let data = escape_for_script(json_blob);
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>AetherCore AI eval report</title>
<style>{css}</style>
</head>
<body>
<header>
  <h1>AetherCore AI eval report</h1>
  <p class="meta">{count} run(s) embedded. Generated offline; this file is fully self-contained.</p>
</header>

<div id="controls" class="controls">
  <label class="ctl">Filter
    <input id="filter" type="search" placeholder="model name substring…" autocomplete="off">
  </label>
  <span class="ctl-group" id="view-toggle" role="radiogroup" aria-label="View">
    <button class="view-btn active" data-view="latest">Latest per model</button>
    <button class="view-btn" data-view="best">Best per model</button>
    <button class="view-btn" data-view="all">All runs</button>
  </span>
  <label class="ctl">Sort
    <select id="sort-select"></select>
  </label>
  <span id="count-badge" class="badge"></span>
</div>

<main>
  <section id="leaderboard-section">
    <h2>Leaderboard <span class="sub">(ranking by the active view; click a row for run history)</span></h2>
    <div class="scroll-x"><table id="leaderboard"></table></div>
  </section>

  <section id="heatmap-section">
    <h2>Model × case heatmap <span class="sub">(green = pass, red = fail; hover a cell for latency/tokens/error)</span></h2>
    <div class="scroll-both"><table id="case-heatmap" class="heatmap"></table></div>
  </section>

  <section id="raw-section">
    <details id="raw-runs">
      <summary>Raw runs &amp; metadata (<span id="raw-count">0</span>)</summary>
      <div class="scroll-x"><table id="raw-table"></table></div>
    </details>
  </section>
</main>
<script>
const RECORDS = JSON.parse("{data}");
{js}
</script>
</body>
</html>
"#,
        css = CSS,
        count = count,
        data = data,
        js = JS,
    )
}

/// Inline stylesheet. Color encoding everywhere for glance-ability; sticky headers + sticky first
/// column for the big tables. No `format!` here — this is a plain `const`, braces are literal CSS.
const CSS: &str = r#"
:root {
  --bg:#0f1115; --panel:#171a21; --fg:#e6e8ec; --muted:#9aa3b2; --line:#2a2f3a; --accent:#4f8cff;
  --pass:#1b8a3a; --fail:#c0392b;
}
* { box-sizing: border-box; }
body { margin:0; font:14px/1.5 -apple-system,Segoe UI,Roboto,Helvetica,Arial,sans-serif; background:var(--bg); color:var(--fg); }
header { padding:16px 24px 10px; }
h1 { margin:0; font-size:20px; }
h2 { font-size:16px; margin:0 0 8px; }
.sub { color:var(--muted); font-weight:400; font-size:12px; }
.meta, .hint { color:var(--muted); font-size:12px; margin:4px 0; }
main { padding:8px 24px 48px; display:grid; gap:24px; max-width:1600px; }
section { background:var(--panel); border:1px solid var(--line); border-radius:8px; padding:16px; }

/* sticky controls bar */
.controls {
  position:sticky; top:0; z-index:30; display:flex; flex-wrap:wrap; align-items:center; gap:14px;
  padding:10px 24px; background:rgba(15,17,21,0.96); border-bottom:1px solid var(--line);
  backdrop-filter:blur(4px);
}
.ctl { display:flex; align-items:center; gap:6px; color:var(--muted); font-size:12px; }
.ctl input, .ctl select {
  background:#0c0e12; color:var(--fg); border:1px solid var(--line); border-radius:6px;
  padding:5px 8px; font:inherit;
}
.ctl input { min-width:220px; }
.ctl-group { display:inline-flex; border:1px solid var(--line); border-radius:6px; overflow:hidden; }
.view-btn {
  background:#0c0e12; color:var(--muted); border:0; border-right:1px solid var(--line);
  padding:6px 12px; font:inherit; cursor:pointer;
}
.view-btn:last-child { border-right:0; }
.view-btn:hover { color:var(--fg); }
.view-btn.active { background:var(--accent); color:#fff; }
.badge { margin-left:auto; color:var(--muted); font-size:12px; }

/* generic tables */
table { border-collapse:collapse; width:100%; }
th, td { padding:6px 10px; border-bottom:1px solid var(--line); text-align:right; white-space:nowrap; }
th:first-child, td:first-child { text-align:left; }
thead th { color:var(--muted); font-weight:600; position:sticky; top:0; background:var(--panel); z-index:2; }
.scroll-x { overflow-x:auto; }
.scroll-both { overflow:auto; max-height:75vh; }

/* leaderboard */
#leaderboard th { cursor:pointer; user-select:none; }
#leaderboard th:hover { color:var(--fg); }
#leaderboard tbody tr { cursor:pointer; }
#leaderboard tbody tr:hover td { background:rgba(79,140,255,0.08); }
.rank { color:var(--muted); }
.model-cell { font-weight:600; }
.acc-bar { position:relative; height:18px; width:160px; background:#0c0e12; border:1px solid var(--line); border-radius:4px; overflow:hidden; margin-left:auto; }
.acc-fill { position:absolute; left:0; top:0; bottom:0; }
.acc-text { position:relative; z-index:1; display:block; text-align:center; font-size:12px; color:#fff; text-shadow:0 1px 2px rgba(0,0,0,0.65); line-height:18px; }
.heat { color:#0b0d10; font-weight:600; }
td.expander { padding:0; }
.detail-host { background:#0c0e12; }
.detail-host table { width:auto; min-width:60%; }
.detail-host th, .detail-host td { border-bottom:1px solid var(--line); font-size:12px; }
.detail-spark { display:flex; align-items:center; gap:12px; padding:8px 10px; flex-wrap:wrap; }
.detail-spark .legend { color:var(--muted); font-size:11px; }

/* heatmap */
.heatmap th, .heatmap td { border:1px solid var(--line); padding:0; }
.heatmap thead th { z-index:5; background:var(--panel); }
.heatmap th.corner { position:sticky; left:0; top:0; z-index:6; }
.heatmap th.case-head { height:120px; vertical-align:bottom; padding:4px; }
.heatmap th.case-head > div { writing-mode:vertical-rl; transform:rotate(180deg); white-space:nowrap; font-weight:600; color:var(--muted); font-size:11px; margin:0 auto; }
.heatmap th.right-head { writing-mode:initial; }
.heatmap th.model-head, .heatmap td.model-head {
  position:sticky; left:0; z-index:3; background:var(--panel); text-align:left; padding:4px 10px; font-weight:600; min-width:160px; max-width:240px; overflow:hidden; text-overflow:ellipsis;
}
.heatmap td.cell { width:22px; height:22px; text-align:center; font-size:11px; }
.cell-pass { background:var(--pass); color:#fff; }
.cell-fail { background:var(--fail); color:#fff; }
.cell-na { background:#1d2129; color:var(--muted); }
.heatmap td.acc-col, .heatmap th.acc-head { text-align:center; font-weight:600; color:#0b0d10; }
.heatmap tfoot td { font-weight:600; color:#0b0d10; text-align:center; }
.heatmap tfoot td.model-head { background:var(--panel); color:var(--fg); }

.empty { color:var(--muted); padding:12px; }
svg { display:block; }
.legend { color:var(--muted); font-size:11px; }

/* transcript expander (raw runs) */
.transcript-sys, .transcript-case { margin:8px 10px; }
.transcript-sys > summary, .transcript-case > summary { cursor:pointer; color:var(--fg); font-weight:600; padding:4px 0; }
.transcript-case > summary.tc-pass { color:#5fd07a; }
.transcript-case > summary.tc-fail { color:#e07a6e; }
.msg { border-left:3px solid var(--line); margin:6px 0 6px 12px; padding:4px 0 4px 10px; }
.msg-role { display:inline-block; font-size:10px; text-transform:uppercase; letter-spacing:0.05em; color:var(--muted); font-weight:700; }
.msg-user { border-left-color:#4f8cff; }
.msg-assistant { border-left-color:#9b6cff; }
.msg-tool { border-left-color:#2a9d8f; }
.msg-content { white-space:pre-wrap; word-break:break-word; margin:4px 0 0; font:12px/1.45 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace; color:var(--fg); }
.msg-tool-call { white-space:pre-wrap; word-break:break-word; margin:4px 0 0; font:12px/1.45 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace; color:#c9a35f; }
.msg-meta { display:block; color:var(--muted); font-size:10px; margin-top:2px; }
"#;

/// The vanilla-JS renderer. Recomputes everything from `RECORDS` (the embedded `RunRecord[]`).
/// A plain `const` — its braces are literal JS, never seen by `format!`.
const JS: &str = r##"
// ---- DOM helper ----------------------------------------------------------
function el(tag, attrs, children) {
  const e = document.createElement(tag);
  if (attrs) for (const k in attrs) {
    if (k === 'class') e.className = attrs[k];
    else if (k === 'title') e.title = attrs[k];
    else if (k === 'html') e.innerHTML = attrs[k];
    else if (k === 'style') e.style.cssText = attrs[k];
    else e.setAttribute(k, attrs[k]);
  }
  if (children != null) for (const c of [].concat(children)) {
    if (c == null) continue;
    e.appendChild(typeof c === 'string' ? document.createTextNode(c) : c);
  }
  return e;
}

// ---- formatting ----------------------------------------------------------
function durMs(d) {
  // Serialized std::time::Duration is { secs, nanos }; tolerate a bare number too.
  if (d == null) return null;
  if (typeof d === 'number') return d * 1000;
  return (d.secs || 0) * 1000 + (d.nanos || 0) / 1e6;
}
function fmt(n, digits) { return (n == null || isNaN(n)) ? '—' : Number(n).toFixed(digits == null ? 1 : digits); }
function pct(n) { return (n == null || isNaN(n)) ? '—' : (n * 100).toFixed(0) + '%'; }

// ---- color scales (red -> amber -> green), text stays legible -------------
// Accuracy: 0 = red, 0.5 = amber, 1 = green.
function accColor(t) {
  t = Math.max(0, Math.min(1, t));
  const r = t < 0.5 ? 210 : Math.round(210 - (t - 0.5) * 2 * 170);
  const g = t < 0.5 ? Math.round(80 + t * 2 * 110) : 190;
  return 'rgb(' + r + ',' + g + ',60)';
}
// Metric heat where LOW is good (latency, tokens): norm in [0,1], 0 = green, 1 = red.
function heatLowGood(norm) {
  norm = Math.max(0, Math.min(1, isNaN(norm) ? 0 : norm));
  const r = norm < 0.5 ? Math.round(60 + norm * 2 * 150) : 210;
  const g = norm < 0.5 ? 190 : Math.round(190 - (norm - 0.5) * 2 * 130);
  return 'rgb(' + r + ',' + g + ',60)';
}

// ---- per-run aggregate metrics (all derived client-side from outcomes) ----
function metrics(rec) {
  const r = rec.report || {};
  const outs = r.outcomes || [];
  const total = outs.length;
  const passed = outs.filter(o => o.passed).length;
  const acc = total ? passed / total : 0;
  const lats = outs.map(o => durMs(o.latency)).filter(x => x != null).sort((a, b) => a - b);
  const mean = lats.length ? lats.reduce((a, b) => a + b, 0) / lats.length : null;
  // Nearest-rank quantile, matching the Rust side.
  const pq = (p) => { if (!lats.length) return null; const rank = Math.max(1, Math.ceil(p * lats.length)); return lats[Math.min(rank, lats.length) - 1]; };
  const toks = outs.map(o => o.tokens).filter(x => x != null);
  const meanTok = toks.length ? toks.reduce((a, b) => a + b, 0) / toks.length : null;
  return {
    model: rec.model, when: rec.timestamp_display, whenKey: rec.timestamp_file,
    backend: rec.backend, casesDir: rec.cases_dir,
    // The run-level system prompt (the record carries it; fall back to the nested report copy).
    systemPrompt: rec.system_prompt || r.system_prompt || '',
    total, passed, acc, mean, p50: pq(0.5), p95: pq(0.95), meanTok, outs,
  };
}

const RUNS = RECORDS.map(metrics);
const MODELS = Array.from(new Set(RUNS.map(r => r.model)));

// ---- view selection: latest | best | all ---------------------------------
// Grouping key = model. latest = max timestamp_file; best = max accuracy.
function rowsForView(view, filter) {
  const f = (filter || '').trim().toLowerCase();
  let runs = RUNS;
  if (f) runs = runs.filter(r => r.model.toLowerCase().includes(f));
  if (view === 'all') return runs.slice();
  const by = {};
  for (const r of runs) {
    const cur = by[r.model];
    if (!cur) { by[r.model] = r; continue; }
    if (view === 'best') {
      // highest accuracy, tie -> lowest p50, tie -> latest.
      if (r.acc > cur.acc ||
          (r.acc === cur.acc && (r.p50 || 1e9) < (cur.p50 || 1e9)) ||
          (r.acc === cur.acc && r.p50 === cur.p50 && r.whenKey > cur.whenKey)) by[r.model] = r;
    } else { // latest
      if (r.whenKey > cur.whenKey) by[r.model] = r;
    }
  }
  return Object.values(by);
}
// runsPerModel honors the current filter so the "#runs" column matches what's shown.
function runsPerModel(filter) {
  const f = (filter || '').trim().toLowerCase();
  const counts = {};
  for (const r of RUNS) {
    if (f && !r.model.toLowerCase().includes(f)) continue;
    counts[r.model] = (counts[r.model] || 0) + 1;
  }
  return counts;
}
function lastRunDate(model) {
  let best = null;
  for (const r of RUNS) if (r.model === model && (best == null || r.whenKey > best.whenKey)) best = r;
  return best;
}

// ---- shared UI state -----------------------------------------------------
const STATE = { view: 'latest', filter: '', sortKey: 'acc', sortDir: -1 };

// Leaderboard columns. `num` => numeric sort; `get` => sort value.
const LB_COLS = [
  { key: 'rank', label: '#',           num: true,  get: r => r._rank,  sortable: false },
  { key: 'model', label: 'Model',      num: false, get: r => r.model },
  { key: 'backend', label: 'Backend',  num: false, get: r => r.backend || '' },
  { key: 'acc', label: 'Accuracy',     num: true,  get: r => r.acc },
  { key: 'pt', label: 'Pass/Total',    num: true,  get: r => r.passed },
  { key: 'p50', label: 'p50 ms',       num: true,  get: r => r.p50 == null ? Infinity : r.p50 },
  { key: 'p95', label: 'p95 ms',       num: true,  get: r => r.p95 == null ? Infinity : r.p95 },
  { key: 'tok', label: 'Mean tokens',  num: true,  get: r => r.meanTok == null ? Infinity : r.meanTok },
  { key: 'runs', label: '#runs',       num: true,  get: r => r._runs },
  { key: 'last', label: 'Last run',    num: false, get: r => r._lastKey || '' },
];

function sortRows(rows) {
  const col = LB_COLS.find(c => c.key === STATE.sortKey) || LB_COLS[3];
  return rows.slice().sort((a, b) => {
    const av = col.get(a), bv = col.get(b);
    let c;
    if (col.num) c = (av === Infinity ? 1e18 : (av || 0)) - (bv === Infinity ? 1e18 : (bv || 0));
    else c = String(av).localeCompare(String(bv));
    return c * STATE.sortDir;
  });
}

// ---- Leaderboard ---------------------------------------------------------
function renderLeaderboard() {
  const t = document.getElementById('leaderboard');
  t.innerHTML = '';
  let rows = rowsForView(STATE.view, STATE.filter);
  const perModel = runsPerModel(STATE.filter);
  rows.forEach(r => {
    r._runs = perModel[r.model] || 1;
    const lr = lastRunDate(r.model);
    r._lastKey = lr ? lr.whenKey : r.whenKey;
    r._lastDisp = lr ? lr.when : r.when;
  });

  document.getElementById('count-badge').textContent =
    rows.length + (STATE.view === 'all' ? ' run(s)' : ' model(s)') + ' / ' + MODELS.length + ' total';

  if (!rows.length) {
    t.appendChild(el('caption', { class: 'empty' }, RUNS.length ? 'No models match the filter.' : 'No runs yet.'));
    return;
  }

  rows = sortRows(rows);
  rows.forEach((r, i) => { r._rank = i + 1; });

  // heat ranges across the VISIBLE set
  const p50s = rows.map(r => r.p50).filter(x => x != null);
  const p95s = rows.map(r => r.p95).filter(x => x != null);
  const toks = rows.map(r => r.meanTok).filter(x => x != null);
  const range = (arr) => { if (!arr.length) return null; const lo = Math.min(...arr), hi = Math.max(...arr); return { lo, hi, span: (hi - lo) || 1 }; };
  const rP50 = range(p50s), rP95 = range(p95s), rTok = range(toks);
  const norm = (v, rg) => rg == null || v == null ? NaN : (v - rg.lo) / rg.span;

  const thead = el('thead');
  const htr = el('tr');
  for (const c of LB_COLS) {
    const arrow = (c.sortable !== false && c.key === STATE.sortKey) ? (STATE.sortDir < 0 ? ' ▼' : ' ▲') : '';
    const th = el('th', c.sortable === false ? null : { title: 'Sort by ' + c.label }, c.label + arrow);
    if (c.sortable !== false) th.onclick = () => {
      if (STATE.sortKey === c.key) STATE.sortDir = -STATE.sortDir;
      else { STATE.sortKey = c.key; STATE.sortDir = c.num ? -1 : 1; }
      syncSortSelect(); renderLeaderboard();
    };
    htr.appendChild(th);
  }
  thead.appendChild(htr); t.appendChild(thead);

  const tb = el('tbody');
  for (const r of rows) {
    const tr = el('tr');
    tr.appendChild(el('td', { class: 'rank' }, String(r._rank)));
    tr.appendChild(el('td', { class: 'model-cell' }, r.model));
    tr.appendChild(el('td', null, r.backend || '—'));

    // accuracy bar
    const fill = el('span', { class: 'acc-fill', style: 'width:' + (r.acc * 100) + '%;background:' + accColor(r.acc) });
    const bar = el('div', { class: 'acc-bar' }, [fill, el('span', { class: 'acc-text' }, pct(r.acc))]);
    tr.appendChild(el('td', null, bar));

    tr.appendChild(el('td', null, r.passed + '/' + r.total));

    const tdP50 = el('td', { class: 'heat' }, fmt(r.p50));
    if (r.p50 != null) tdP50.style.background = heatLowGood(norm(r.p50, rP50));
    tr.appendChild(tdP50);
    const tdP95 = el('td', { class: 'heat' }, fmt(r.p95));
    if (r.p95 != null) tdP95.style.background = heatLowGood(norm(r.p95, rP95));
    tr.appendChild(tdP95);
    const tdTok = el('td', { class: 'heat' }, fmt(r.meanTok, 0));
    if (r.meanTok != null) tdTok.style.background = heatLowGood(norm(r.meanTok, rTok));
    tr.appendChild(tdTok);

    tr.appendChild(el('td', null, String(r._runs)));
    tr.appendChild(el('td', null, (r._lastDisp || '—').split(' ')[0]));

    // expandable run-history detail row
    const detailTr = el('tr', { class: 'detail-row', style: 'display:none' });
    const detailTd = el('td', { class: 'expander', colspan: String(LB_COLS.length) });
    detailTr.appendChild(detailTd);
    let built = false;
    tr.onclick = () => {
      if (!built) { detailTd.appendChild(modelDetail(r.model)); built = true; }
      detailTr.style.display = detailTr.style.display === 'none' ? '' : 'none';
    };

    tb.appendChild(tr);
    tb.appendChild(detailTr);
  }
  t.appendChild(tb);
}

// Per-model run history: a compact sub-table + accuracy/p50 sparklines over time.
function modelDetail(model) {
  const host = el('div', { class: 'detail-host' });
  const runs = RUNS.filter(r => r.model === model).sort((a, b) => a.whenKey.localeCompare(b.whenKey));
  if (runs.length > 1) {
    const sp = el('div', { class: 'detail-spark' });
    sp.appendChild(el('span', { class: 'legend' }, 'accuracy'));
    sp.appendChild(spark(runs, r => r.acc * 100, '#39c46a', v => v.toFixed(0) + '%'));
    sp.appendChild(el('span', { class: 'legend' }, 'p50 latency'));
    sp.appendChild(spark(runs, r => r.p50 || 0, '#7aa9ff', v => v.toFixed(0) + 'ms'));
    host.appendChild(sp);
  }
  const tbl = el('table');
  const head = el('tr');
  ['When', 'Accuracy', 'Pass/Total', 'p50 ms', 'Mean tokens', 'Backend'].forEach(h => head.appendChild(el('th', null, h)));
  tbl.appendChild(el('thead', null, head));
  const body = el('tbody');
  for (const r of runs.slice().reverse()) {
    const tr = el('tr');
    tr.appendChild(el('td', null, r.when || r.whenKey));
    tr.appendChild(el('td', null, pct(r.acc)));
    tr.appendChild(el('td', null, r.passed + '/' + r.total));
    tr.appendChild(el('td', null, fmt(r.p50)));
    tr.appendChild(el('td', null, fmt(r.meanTok, 0)));
    tr.appendChild(el('td', null, r.backend || '—'));
    body.appendChild(tr);
  }
  tbl.appendChild(body);
  host.appendChild(tbl);
  return host;
}

// ---- Model × case heatmap ------------------------------------------------
function renderHeatmap() {
  const t = document.getElementById('case-heatmap');
  t.innerHTML = '';
  const rows = rowsForView(STATE.view, STATE.filter);
  if (!rows.length) {
    t.appendChild(el('caption', { class: 'empty' }, RUNS.length ? 'No models match the filter.' : 'No runs yet.'));
    return;
  }
  // Stable model-row order: by accuracy desc, then name.
  const modelRows = rows.slice().sort((a, b) => (b.acc - a.acc) || a.model.localeCompare(b.model));

  // Column set = union of case names across the VISIBLE rows, sorted stably (alpha).
  const nameSet = new Set();
  for (const r of modelRows) for (const o of r.outs) nameSet.add(o.name);
  const names = Array.from(nameSet).sort();

  // index each visible row's outcomes by case name
  for (const r of modelRows) { r._byName = {}; for (const o of r.outs) r._byName[o.name] = o; }

  // header: corner + rotated case names + per-model accuracy header
  const thead = el('thead');
  const htr = el('tr');
  htr.appendChild(el('th', { class: 'corner' }, 'Model'));
  for (const name of names) htr.appendChild(el('th', { class: 'case-head', title: name }, el('div', null, name)));
  htr.appendChild(el('th', { class: 'right-head acc-head' }, 'Accuracy'));
  thead.appendChild(htr); t.appendChild(thead);

  const tb = el('tbody');
  for (const r of modelRows) {
    const tr = el('tr');
    tr.appendChild(el('td', { class: 'model-head', title: r.model + (STATE.view === 'all' ? ' @' + r.whenKey : '') }, r.model));
    for (const name of names) {
      const o = r._byName[name];
      if (!o) { tr.appendChild(el('td', { class: 'cell cell-na', title: name + ': not run' }, '·')); continue; }
      const ms = durMs(o.latency);
      const tip = name + '\n' + (o.passed ? 'PASS' : 'FAIL') + ' · ' + fmt(ms) + 'ms' +
        (o.tokens != null ? ' · ' + o.tokens + ' tok' : '') + (o.error ? '\n' + o.error : '');
      tr.appendChild(el('td', { class: 'cell ' + (o.passed ? 'cell-pass' : 'cell-fail'), title: tip }, o.passed ? '✓' : '✗'));
    }
    const accTd = el('td', { class: 'acc-col', title: r.passed + '/' + r.total }, pct(r.acc));
    accTd.style.background = accColor(r.acc);
    tr.appendChild(accTd);
    tb.appendChild(tr);
  }
  t.appendChild(tb);

  // footer: per-case pass-rate across the shown models (color-scaled)
  const tfoot = el('tfoot');
  const ftr = el('tr');
  ftr.appendChild(el('td', { class: 'model-head' }, 'Pass rate'));
  for (const name of names) {
    let ran = 0, pass = 0;
    for (const r of modelRows) { const o = r._byName[name]; if (o) { ran++; if (o.passed) pass++; } }
    const rate = ran ? pass / ran : null;
    const td = el('td', { title: name + ': ' + pass + '/' + ran + ' passed' }, rate == null ? '—' : pct(rate));
    if (rate != null) td.style.background = accColor(rate);
    ftr.appendChild(td);
  }
  // overall accuracy across the visible matrix
  let allRan = 0, allPass = 0;
  for (const r of modelRows) for (const name of names) { const o = r._byName[name]; if (o) { allRan++; if (o.passed) allPass++; } }
  const overall = allRan ? allPass / allRan : null;
  const overTd = el('td', null, overall == null ? '—' : pct(overall));
  if (overall != null) overTd.style.background = accColor(overall);
  ftr.appendChild(overTd);
  tfoot.appendChild(ftr);
  t.appendChild(tfoot);
}

// ---- Raw runs / metadata (collapsed) -------------------------------------
function renderRaw() {
  document.getElementById('raw-count').textContent = String(RUNS.length);
  const t = document.getElementById('raw-table');
  t.innerHTML = '';
  if (!RUNS.length) { t.appendChild(el('caption', { class: 'empty' }, 'No runs yet.')); return; }
  const COLS = ['Model', 'Timestamp', 'Backend', 'Cases dir', 'Accuracy'];
  const head = el('tr');
  COLS.forEach(h => head.appendChild(el('th', null, h)));
  t.appendChild(el('thead', null, head));
  const body = el('tbody');
  const sorted = RUNS.slice().sort((a, b) => b.whenKey.localeCompare(a.whenKey));
  for (const r of sorted) {
    const tr = el('tr');
    tr.appendChild(el('td', null, r.model));
    tr.appendChild(el('td', null, r.when || r.whenKey));
    tr.appendChild(el('td', null, r.backend || '—'));
    tr.appendChild(el('td', null, r.casesDir || '—'));
    tr.appendChild(el('td', null, pct(r.acc)));

    // Expand-on-click detail row: the system prompt + per-case transcripts (built lazily, once).
    const detailTr = el('tr', { class: 'detail-row', style: 'display:none' });
    const detailTd = el('td', { class: 'expander', colspan: String(COLS.length) });
    detailTr.appendChild(detailTd);
    let built = false;
    tr.style.cursor = 'pointer';
    tr.onclick = () => {
      if (!built) { detailTd.appendChild(runDetail(r)); built = true; }
      detailTr.style.display = detailTr.style.display === 'none' ? '' : 'none';
    };
    body.appendChild(tr);
    body.appendChild(detailTr);
  }
  t.appendChild(body);
}

// Per-run detail: the shared system prompt at top, then one collapsible block per case rendering its
// transcript as role-tagged message blocks (user / assistant [content + tool_calls] / tool [result]).
function runDetail(run) {
  const host = el('div', { class: 'detail-host' });

  // System prompt (shown once for the whole run).
  const sysWrap = el('details', { class: 'transcript-sys' });
  sysWrap.appendChild(el('summary', null, 'System prompt'));
  sysWrap.appendChild(el('pre', { class: 'msg-content' }, run.systemPrompt || '(none recorded)'));
  host.appendChild(sysWrap);

  const outs = run.outs || [];
  if (!outs.length) { host.appendChild(el('div', { class: 'empty' }, 'No cases.')); return host; }

  for (const o of outs) {
    const block = el('details', { class: 'transcript-case' });
    const status = o.passed ? 'PASS' : 'FAIL';
    block.appendChild(el('summary', { class: o.passed ? 'tc-pass' : 'tc-fail' },
      status + ' · ' + o.name + ' (' + ((o.transcript || []).length) + ' msgs)'));
    const tx = o.transcript || [];
    if (!tx.length) {
      block.appendChild(el('div', { class: 'empty' }, 'No transcript recorded for this case.'));
    } else {
      for (const m of tx) block.appendChild(messageBlock(m));
    }
    host.appendChild(block);
  }
  return host;
}

// Render one transcript message as a role-tagged block. Assistant blocks show both content AND any
// tool_calls; tool blocks show the result content. Text is escaped by `el()` (textNode children).
function messageBlock(m) {
  const role = (m && m.role) || 'unknown';
  const wrap = el('div', { class: 'msg msg-' + role });
  wrap.appendChild(el('span', { class: 'msg-role' }, role));
  if (m && m.content != null && String(m.content).length) {
    wrap.appendChild(el('pre', { class: 'msg-content' }, String(m.content)));
  }
  // Assistant tool calls: one line per call, name + JSON-stringified arguments.
  const calls = m && m.tool_calls;
  if (Array.isArray(calls) && calls.length) {
    for (const c of calls) {
      const fn = (c && c.function) || {};
      const line = (fn.name || '(unnamed)') + '(' + (fn.arguments || '') + ')';
      wrap.appendChild(el('pre', { class: 'msg-tool-call' }, line));
    }
  }
  // Tool reply tool_call_id, when present, for correlation.
  if (role === 'tool' && m.tool_call_id) {
    wrap.appendChild(el('span', { class: 'msg-meta' }, '↳ id ' + m.tool_call_id));
  }
  return wrap;
}

// ---- inline SVG sparkline (accuracy / p50 over time) ---------------------
function spark(points, valueFn, color, fmtFn) {
  const W = 200, H = 36, pad = 4;
  const xmlns = 'http://www.w3.org/2000/svg';
  const svg = document.createElementNS(xmlns, 'svg');
  svg.setAttribute('width', W); svg.setAttribute('height', H); svg.setAttribute('viewBox', '0 0 ' + W + ' ' + H);
  const vals = points.map(valueFn);
  const min = Math.min(...vals), max = Math.max(...vals);
  const span = (max - min) || 1;
  const n = points.length;
  const xs = i => n <= 1 ? W / 2 : pad + (i * (W - 2 * pad)) / (n - 1);
  const ys = v => H - pad - ((v - min) / span) * (H - 2 * pad);
  let d = '';
  points.forEach((p, i) => { d += (i ? ' L ' : 'M ') + xs(i).toFixed(1) + ' ' + ys(vals[i]).toFixed(1); });
  const path = document.createElementNS(xmlns, 'path');
  path.setAttribute('d', d); path.setAttribute('fill', 'none'); path.setAttribute('stroke', color); path.setAttribute('stroke-width', '2');
  svg.appendChild(path);
  points.forEach((p, i) => {
    const c = document.createElementNS(xmlns, 'circle');
    c.setAttribute('cx', xs(i)); c.setAttribute('cy', ys(vals[i])); c.setAttribute('r', '2.5'); c.setAttribute('fill', color);
    const title = document.createElementNS(xmlns, 'title');
    title.textContent = (points[i].when || points[i].whenKey) + ': ' + fmtFn(vals[i]);
    c.appendChild(title); svg.appendChild(c);
  });
  return svg;
}

// ---- controls wiring -----------------------------------------------------
function syncSortSelect() {
  const sel = document.getElementById('sort-select');
  if (!sel) return;
  sel.value = STATE.sortKey + ':' + (STATE.sortDir < 0 ? 'desc' : 'asc');
}
function buildSortSelect() {
  const sel = document.getElementById('sort-select');
  sel.innerHTML = '';
  for (const c of LB_COLS) {
    if (c.sortable === false) continue;
    for (const dir of ['desc', 'asc']) {
      const opt = el('option', { value: c.key + ':' + dir }, c.label + ' ' + (dir === 'desc' ? '▼' : '▲'));
      sel.appendChild(opt);
    }
  }
  sel.onchange = () => {
    const [k, d] = sel.value.split(':');
    STATE.sortKey = k; STATE.sortDir = d === 'desc' ? -1 : 1;
    renderLeaderboard();
  };
  syncSortSelect();
}

function renderAll() { renderLeaderboard(); renderHeatmap(); }

function wireControls() {
  const filter = document.getElementById('filter');
  filter.oninput = () => { STATE.filter = filter.value; renderAll(); };
  for (const btn of document.querySelectorAll('.view-btn')) {
    btn.onclick = () => {
      for (const b of document.querySelectorAll('.view-btn')) b.classList.remove('active');
      btn.classList.add('active');
      STATE.view = btn.getAttribute('data-view');
      renderAll();
    };
  }
  buildSortSelect();
}

wireControls();
renderAll();
renderRaw();
"##;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{CaseOutcome, EvalReport, RunRecord};
    use std::time::Duration;

    /// Build a tiny synthetic RunRecord with one passing + one failing case.
    fn sample(model: &str, ts_file: &str, ts_display: &str) -> RunRecord {
        use serde_json::json;
        let outcomes = vec![
            CaseOutcome {
                name: "place_block".into(),
                passed: true,
                predicates: vec![("solid_placed>=1".into(), true)],
                latency: Duration::from_millis(120),
                tokens: Some(42),
                tool_calls: vec!["place(...)".into()],
                final_text: Some("done".into()),
                error: None,
                // A small role-tagged transcript so the embedded-data test can assert it round-trips.
                transcript: vec![
                    json!({"role": "user", "content": "place a block here"}),
                    json!({"role": "assistant", "content": "", "tool_calls": [
                        {"id": "c0", "type": "function",
                         "function": {"name": "set_voxel", "arguments": "{}"}}]}),
                    json!({"role": "tool", "tool_call_id": "c0", "content": "queued 1 block"}),
                    json!({"role": "assistant", "content": "done"}),
                ],
            },
            CaseOutcome {
                name: "dig_hole".into(),
                passed: false,
                predicates: vec![("removed".into(), false)],
                latency: Duration::from_millis(300),
                tokens: Some(50),
                tool_calls: vec![],
                final_text: None,
                error: Some("backend error".into()),
                transcript: vec![],
            },
        ];
        RunRecord {
            model: model.into(),
            timestamp_display: ts_display.into(),
            timestamp_file: ts_file.into(),
            backend: "local".into(),
            cases_dir: "eval/cases".into(),
            system_prompt: "TEST SYSTEM PROMPT".into(),
            report: EvalReport::new(
                outcomes,
                0.0,
                format!("local: {model}"),
                "TEST SYSTEM PROMPT".into(),
            ),
        }
    }

    #[test]
    fn generates_self_contained_html_with_embedded_data() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path();

        for (model, tf, td) in [
            ("qwen2.5-7b", "20260618-140000", "2026-06-18 14:00:00"),
            ("qwen2.5-3b", "20260618-150000", "2026-06-18 15:00:00"),
            ("qwen2.5-7b", "20260618-160000", "2026-06-18 16:00:00"),
        ] {
            let rec = sample(model, tf, td);
            let json = serde_json::to_string(&rec).expect("serialize record");
            std::fs::write(p.join(format!("{model}_{tf}.json")), json).expect("write record");
        }

        let out = generate_report(p).expect("generate");
        assert!(
            out.is_file(),
            "report.html should exist at {}",
            out.display()
        );
        let html = std::fs::read_to_string(&out).expect("read report");

        // Data is embedded (model names + the case names + an accuracy-bearing field present).
        assert!(html.contains("qwen2.5-7b"), "model name embedded");
        assert!(html.contains("qwen2.5-3b"), "second model embedded");
        assert!(html.contains("place_block"), "case name embedded");
        assert!(
            html.contains("\\\"passed\\\""),
            "outcome flags embedded (JSON escaped for script)"
        );

        // The new per-case transcript + run-level system prompt round-trip into the embedded JSON.
        assert!(
            html.contains("\\\"transcript\\\""),
            "per-case transcript field embedded (JSON escaped for script)"
        );
        assert!(
            html.contains("place a block here"),
            "transcript message content embedded"
        );
        assert!(
            html.contains("\\\"system_prompt\\\""),
            "run-level system_prompt field embedded (JSON escaped for script)"
        );
        assert!(
            html.contains("TEST SYSTEM PROMPT"),
            "system prompt text embedded"
        );

        // The redesigned section containers are emitted (tests pin these ids).
        assert!(
            html.contains("id=\"leaderboard\""),
            "leaderboard table present"
        );
        assert!(
            html.contains("id=\"case-heatmap\""),
            "case heatmap table present"
        );
        assert!(html.contains("id=\"controls\""), "controls bar present");

        // No external resources — must open offline. (The SVG xmlns namespace URI is not a fetched
        // resource, so we check for the tags that would PULL something over the network instead.)
        assert!(!html.contains("<script src"), "no external script tags");
        assert!(!html.contains("<link "), "no external stylesheet links");
        assert!(!html.contains("cdn"), "no CDN references");
        assert!(html.contains("<script>"), "inline script present");
    }

    #[test]
    fn empty_dir_yields_a_report_with_no_runs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = generate_report(dir.path()).expect("generate empty");
        let html = std::fs::read_to_string(&out).expect("read");
        assert!(html.contains("0 run(s) embedded"));
        // Still a valid shell with the section containers, just no data rows.
        assert!(html.contains("id=\"leaderboard\""));
        assert!(html.contains("id=\"case-heatmap\""));
    }

    #[test]
    fn collapses_duplicate_model_runs_to_distinct_models() {
        // 3 runs across 2 distinct model names; "model-a" runs twice on different timestamp_files.
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path();
        for (model, tf, td) in [
            ("model-a", "20260618-100000", "2026-06-18 10:00:00"),
            ("model-a", "20260618-110000", "2026-06-18 11:00:00"),
            ("model-b", "20260618-120000", "2026-06-18 12:00:00"),
        ] {
            let rec = sample(model, tf, td);
            let json = serde_json::to_string(&rec).expect("serialize record");
            std::fs::write(p.join(format!("{model}_{tf}.json")), json).expect("write record");
        }

        let out = generate_report(p).expect("generate");
        let html = std::fs::read_to_string(&out).expect("read report");

        // Both model names appear in the embedded data.
        assert!(html.contains("model-a"), "model-a embedded");
        assert!(html.contains("model-b"), "model-b embedded");

        // All 3 runs are embedded (so "All runs" view has every run available)...
        let run_count = html.matches("\\\"timestamp_file\\\"").count();
        assert_eq!(run_count, 3, "all three RunRecords embedded");

        // ...but the default "latest per model" grouping (computed in JS from the embedded data)
        // collapses to TWO distinct models. We assert the distinct-model count is derivable from the
        // embedded data: exactly two unique `model` values across the three records.
        let mut models: Vec<&str> = html
            .match_indices("\\\"model\\\":\\\"")
            .filter_map(|(i, m)| {
                let start = i + m.len();
                html[start..]
                    .find("\\\"")
                    .map(|end| &html[start..start + end])
            })
            .collect();
        models.sort();
        models.dedup();
        assert_eq!(
            models,
            vec!["model-a", "model-b"],
            "two distinct models after dedup"
        );
    }

    #[test]
    fn escape_for_script_neutralizes_tag_breaks() {
        let s = escape_for_script(r#"{"x":"</script><a>"}"#);
        assert!(!s.contains("</script>"));
        assert!(!s.contains('<'));
        assert!(!s.contains('>'));
    }
}
