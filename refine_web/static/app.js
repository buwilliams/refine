// refine — vanilla JS single-page app. No build step, no framework.

const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));

const state = {
  reporters: [],
  lastReporter: localStorage.getItem("refine_last_reporter") || "",
  dashboard: null,
  needsAttentionBanners: [],
  currentRoute: null,
  currentGap: null,
};

// ---- API helpers ------------------------------------------------------------

async function api(method, path, body) {
  const opts = { method, headers: {} };
  if (body !== undefined) {
    opts.headers["Content-Type"] = "application/json";
    opts.body = JSON.stringify(body);
  }
  const res = await fetch(path, opts);
  let data = null;
  try { data = await res.json(); } catch {}
  if (!res.ok) {
    const msg = data?.error?.message || res.statusText || "Request failed";
    const err = new Error(msg);
    err.status = res.status;
    err.details = data?.error?.details;
    err.code = data?.error?.code;
    throw err;
  }
  return data;
}

function toast(message, kind = "info") {
  const el = document.createElement("div");
  el.className = `toast ${kind}`;
  el.textContent = message;
  document.body.appendChild(el);
  setTimeout(() => el.remove(), 4000);
}

// ---- Modals (replace native prompt / confirm) -------------------------------
//
// Both return a Promise. modalPrompt resolves to the entered string or null
// if cancelled. modalConfirm resolves to a boolean.
//
// Keyboard: Enter submits, Escape cancels. Clicking the backdrop cancels.

function _openModal(buildBody, onResolveDefault, focusSel) {
  return new Promise((resolve) => {
    const root = document.createElement("div");
    root.className = "modal-backdrop";
    const body = buildBody();
    root.innerHTML = `<div class="modal" role="dialog" aria-modal="true">${body}</div>`;
    document.body.appendChild(root);

    let resolved = false;
    function close(value) {
      if (resolved) return;
      resolved = true;
      document.removeEventListener("keydown", onKey, true);
      root.remove();
      resolve(value);
    }
    function onKey(e) {
      if (e.key === "Escape") {
        e.preventDefault();
        close(onResolveDefault.cancel);
      } else if (e.key === "Enter") {
        // Allow Enter inside a textarea to insert newlines (none today, but safe).
        if (e.target && e.target.tagName === "TEXTAREA") return;
        e.preventDefault();
        const okBtn = root.querySelector("[data-ok]");
        if (okBtn) okBtn.click();
      }
    }
    document.addEventListener("keydown", onKey, true);
    root.addEventListener("click", (e) => {
      if (e.target === root) close(onResolveDefault.cancel);
    });

    root.querySelector("[data-cancel]").addEventListener("click", () =>
      close(onResolveDefault.cancel));
    root.querySelector("[data-ok]").addEventListener("click", () => {
      const input = root.querySelector(".modal-input");
      close(input ? input.value : onResolveDefault.ok);
    });

    const focus = root.querySelector(focusSel);
    if (focus) {
      focus.focus();
      if (focus.tagName === "INPUT") focus.select();
    }
  });
}

function modalPrompt(label, defaultValue = "", {
  title = null, okLabel = "OK", cancelLabel = "Cancel",
} = {}) {
  const body = () => `
    ${title ? `<div class="modal-title">${htmlEscape(title)}</div>` : ""}
    <div class="modal-body">
      <label>${htmlEscape(label)}</label>
      <input type="text" class="modal-input" value="${htmlEscape(defaultValue)}">
    </div>
    <div class="modal-actions">
      <button class="secondary" data-cancel>${htmlEscape(cancelLabel)}</button>
      <button data-ok>${htmlEscape(okLabel)}</button>
    </div>`;
  return _openModal(body, { cancel: null, ok: "" }, ".modal-input");
}

function modalConfirm(message, {
  title = null, okLabel = "OK", cancelLabel = "Cancel", danger = false,
} = {}) {
  const body = () => `
    ${title ? `<div class="modal-title">${htmlEscape(title)}</div>` : ""}
    <div class="modal-body">${htmlEscape(message)}</div>
    <div class="modal-actions">
      <button class="secondary" data-cancel>${htmlEscape(cancelLabel)}</button>
      <button ${danger ? 'class="danger"' : ""} data-ok>${htmlEscape(okLabel)}</button>
    </div>`;
  return _openModal(body, { cancel: false, ok: true }, "[data-ok]");
}

function fmtTime(iso) {
  if (!iso) return "";
  const d = new Date(iso);
  if (isNaN(d.getTime())) return iso;
  return d.toLocaleString();
}

function fmtElapsed(seconds) {
  seconds = Math.max(0, Math.floor(seconds));
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  return `${(seconds / 3600).toFixed(1)}h`;
}

// Abbreviate counter values so dashboard cards stay legible at scale:
// 999 → "999", 1300 → "1.3K", 1_300_000 → "1.3M", 1_300_000_000 → "1.3B".
// Drops a trailing ".0" (so 1000 reads as "1K", not "1.0K"). Promotes
// to the next tier when 1-decimal rounding would push the value over
// 999 at the current tier (e.g. 999_999 reads as "1M", not "1000K").
function fmtCount(n) {
  n = Number(n) || 0;
  if (n < 1000) return String(n);
  const tiers = [
    { div: 1_000_000_000, suffix: "B" },
    { div: 1_000_000,     suffix: "M" },
    { div: 1_000,         suffix: "K" },
  ];
  for (let i = 0; i < tiers.length; i++) {
    const t = tiers[i];
    if (n < t.div) continue;
    const rounded = Math.round((n / t.div) * 10) / 10;
    if (rounded >= 1000 && i > 0) {
      const up = tiers[i - 1];
      return (n / up.div).toFixed(1).replace(/\.0$/, "") + up.suffix;
    }
    return rounded.toFixed(1).replace(/\.0$/, "") + t.suffix;
  }
  return String(n);
}

function htmlEscape(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  }[c]));
}

// ---- Minimal Markdown → HTML ------------------------------------------------
//
// Used to render chat transcripts. Inputs come from the Claude CLI's
// stream-json `assistant.content[].text` blocks (text only — never raw HTML),
// plus the user-echoed `> message` lines we synthesize locally. We html-escape
// every text fragment before substitution and only emit a small whitelist of
// inline tags, so even if claude's text contained literal HTML we'd render it
// as literal text.
//
// Block-level: code fences (```), headings (#…######), unordered (- / *) and
// ordered (1.) lists, blockquotes (>), paragraphs separated by blank lines.
// Inline: **bold**, *italic*, `code`, [text](http(s)://...).
function mdToHtml(text) {
  if (!text) return "";
  const lines = String(text).replace(/\r\n/g, "\n").split("\n");
  const out = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    const fence = line.match(/^```\s*([^\s`]*)\s*$/);
    if (fence) {
      const lang = fence[1] || "";
      const code = [];
      i++;
      while (i < lines.length && !/^```\s*$/.test(lines[i])) {
        code.push(lines[i]);
        i++;
      }
      if (i < lines.length) i++;
      const cls = lang ? ` class="lang-${htmlEscape(lang)}"` : "";
      out.push(`<pre><code${cls}>${htmlEscape(code.join("\n"))}</code></pre>`);
      continue;
    }
    if (!line.trim()) { i++; continue; }
    const heading = line.match(/^(#{1,6})\s+(.+)$/);
    if (heading) {
      const lvl = heading[1].length;
      out.push(`<h${lvl}>${mdInline(heading[2])}</h${lvl}>`);
      i++;
      continue;
    }
    if (/^\s*>\s?/.test(line)) {
      const quoted = [];
      while (i < lines.length && /^\s*>\s?/.test(lines[i])) {
        quoted.push(lines[i].replace(/^\s*>\s?/, ""));
        i++;
      }
      // Recurse so nested formatting inside the quote works.
      out.push(`<blockquote>${mdToHtml(quoted.join("\n"))}</blockquote>`);
      continue;
    }
    if (/^\s*[-*]\s+/.test(line)) {
      const items = [];
      while (i < lines.length && /^\s*[-*]\s+/.test(lines[i])) {
        items.push(lines[i].replace(/^\s*[-*]\s+/, ""));
        i++;
      }
      out.push(`<ul>${items.map(it => `<li>${mdInline(it)}</li>`).join("")}</ul>`);
      continue;
    }
    if (/^\s*\d+\.\s+/.test(line)) {
      const items = [];
      while (i < lines.length && /^\s*\d+\.\s+/.test(lines[i])) {
        items.push(lines[i].replace(/^\s*\d+\.\s+/, ""));
        i++;
      }
      out.push(`<ol>${items.map(it => `<li>${mdInline(it)}</li>`).join("")}</ol>`);
      continue;
    }
    // Paragraph: gather until a blank line or a recognized block opener.
    const para = [];
    while (i < lines.length) {
      const ln = lines[i];
      if (!ln.trim()) break;
      if (/^```/.test(ln) || /^(#{1,6})\s+/.test(ln) ||
          /^\s*[-*]\s+/.test(ln) || /^\s*\d+\.\s+/.test(ln) ||
          /^\s*>\s?/.test(ln)) break;
      para.push(ln);
      i++;
    }
    if (para.length) {
      out.push(`<p>${mdInline(para.join("\n"))}</p>`);
    }
  }
  return out.join("\n");
}

function mdInline(text) {
  let s = htmlEscape(text);
  // Inline code first so its contents aren't mangled by later passes.
  s = s.replace(/`([^`\n]+)`/g, (_, c) => `<code>${c}</code>`);
  // Bold before italic so ** isn't greedily matched as italic.
  s = s.replace(/\*\*([^*\n]+)\*\*/g, "<strong>$1</strong>");
  s = s.replace(/__([^_\n]+)__/g, "<strong>$1</strong>");
  s = s.replace(/\*([^*\n]+)\*/g, "<em>$1</em>");
  s = s.replace(/(^|[^\w])_([^_\n]+)_(?![\w])/g, "$1<em>$2</em>");
  // Links: only http(s)/mailto pass through; anything else stays literal.
  s = s.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, (m, txt, url) => {
    if (!/^(https?:|mailto:)/i.test(url)) return m;
    return `<a href="${url}" target="_blank" rel="noopener noreferrer">${txt}</a>`;
  });
  // Convert remaining hard newlines inside a paragraph to <br>.
  s = s.replace(/\n/g, "<br>");
  return s;
}

// ---- reporter dropdown ------------------------------------------------------

async function refreshReporters() {
  const data = await api("GET", "/api/reporters");
  state.reporters = data.reporters || [];
  populateAllReporterDropdowns();
}

function populateAllReporterDropdowns() {
  const all = $$("[data-reporter-select]");
  // include the global reporter selector in the topbar
  const globalSel = $("#global-reporter");
  if (globalSel) all.push(globalSel);
  for (const sel of all) {
    const current = sel.value || sel.dataset.value || state.lastReporter;
    sel.innerHTML = "";
    const optBlank = document.createElement("option");
    optBlank.value = "";
    optBlank.textContent = "— pick reporter —";
    sel.appendChild(optBlank);
    for (const r of state.reporters) {
      const opt = document.createElement("option");
      opt.value = r.name;
      opt.textContent = r.name;
      sel.appendChild(opt);
    }
    const optAdd = document.createElement("option");
    optAdd.value = "__add__";
    optAdd.textContent = "+ Add new reporter…";
    sel.appendChild(optAdd);
    // Restore selection if still valid
    const stillValid = state.reporters.some((r) => r.name === current);
    sel.value = stillValid ? current : "";
  }
}

async function handleReporterAdd(sel) {
  const name = await modalPrompt("Name for the new reporter:",
                                  "", { title: "Add reporter" });
  if (!name || !name.trim()) {
    sel.value = state.lastReporter || "";
    return null;
  }
  try {
    const { reporter } = await api("POST", "/api/reporters", { name });
    await refreshReporters();
    sel.value = reporter.name;
    setLastReporter(reporter.name);
    return reporter.name;
  } catch (e) {
    toast(`Could not add reporter: ${e.message}`, "error");
    sel.value = state.lastReporter || "";
    return null;
  }
}

function setLastReporter(name) {
  const wasEmpty = !state.lastReporter;
  state.lastReporter = name;
  localStorage.setItem("refine_last_reporter", name);
  const g = $("#global-reporter");
  if (g) g.value = name;
  // Keep any in-page "Submitting as X" indicator in sync without re-rendering
  // the form (which would lose the user's typed-but-unsubmitted text).
  for (const el of $$(".js-reporter-name")) el.textContent = name;
  // If the user just picked their first reporter, re-render views gated on a
  // selected reporter so the form replaces the "pick a reporter" notice.
  if (wasEmpty && name) {
    const r = state.currentRoute;
    if (r === "gaps_new" || r === "gaps_import" || r === "gaps_detail") {
      navigate();
    }
  }
}

// react to "+ Add new reporter" selection on any dropdown
document.addEventListener("change", async (e) => {
  if (e.target.matches("[data-reporter-select], #global-reporter")) {
    if (e.target.value === "__add__") {
      const newName = await handleReporterAdd(e.target);
      if (newName) e.target.dispatchEvent(new Event("change-after-add"));
    } else if (e.target.value) {
      setLastReporter(e.target.value);
    }
  }
});

// "+ New Gap" and "Import…" in the topbar open modals in place rather than
// navigating to dedicated screens. The hrefs are kept for deep-linking /
// accessibility; click handlers intercept so the user's current view stays
// underneath.
document.addEventListener("click", (e) => {
  if (e.target.closest("#btn-new-gap")) {
    e.preventDefault();
    openNewGapModal();
  } else if (e.target.closest("#btn-import")) {
    e.preventDefault();
    openImportModal();
  }
});

// ---- Banners ----------------------------------------------------------------

function renderBanners(items) {
  const root = $("#banners");
  root.innerHTML = "";
  for (const item of items) {
    const tpl = $("#t-banner").content.cloneNode(true);
    const div = tpl.querySelector(".banner");
    div.classList.add(item.severity || "error");
    tpl.querySelector(".banner-msg").textContent = item.message;
    if (item.action) {
      const btn = document.createElement("button");
      btn.textContent = item.action.label;
      btn.onclick = item.action.onClick;
      tpl.querySelector(".banner-actions").appendChild(btn);
    }
    root.appendChild(tpl);
  }
}

// ---- SSE --------------------------------------------------------------------

let sseSource = null;

function initSSE() {
  if (sseSource) sseSource.close();
  sseSource = new EventSource("/api/sse");
  sseSource.addEventListener("activity_added", (e) => {
    // Refresh dashboard activity if visible; refresh current gap if relevant
    if (state.currentRoute === "dashboard") renderDashboard();
    if (state.currentRoute === "logs") loadLogs();
    if (state.currentRoute === "gaps_detail" && state.currentGap) {
      try {
        const data = JSON.parse(e.data);
        if (!data.gap_id || data.gap_id === state.currentGap) loadGapDetail(state.currentGap);
      } catch {}
    }
  });
  sseSource.addEventListener("status_change", () => {
    if (state.currentRoute === "dashboard") renderDashboard();
    // Refresh only the table on background updates so an in-progress
    // keystroke in the search box isn't interrupted by a full re-render.
    if (state.currentRoute === "gaps") refreshGapsTable();
    if (state.currentRoute === "logs") loadLogs();
    if (state.currentRoute === "gaps_detail" && state.currentGap) {
      loadGapDetail(state.currentGap);
    }
  });
  sseSource.addEventListener("round_log_added", (e) => {
    // Subprocess flushed new stdout to the active round's logs[]. If the user
    // is viewing that gap's detail, refresh so the new lines appear live.
    if (state.currentRoute !== "gaps_detail" || !state.currentGap) return;
    try {
      const data = JSON.parse(e.data);
      if (data.gap_id === state.currentGap) loadGapDetail(state.currentGap);
    } catch {}
  });
  sseSource.onerror = () => {
    // Browser will auto-reconnect.
  };
}

// ---- Router -----------------------------------------------------------------

const routes = {
  dashboard: renderDashboard,
  gaps: renderGapsList,
  gaps_detail: renderGapDetail,
  gaps_new: renderGapNew,
  gaps_import: renderGapImport,
  agents: renderAgents,
  logs: renderLogs,
  changes: renderChanges,
  settings: renderSettings,
};

function parseHash() {
  const raw = location.hash.slice(1) || "/";
  // "/" → dashboard, "/gaps" → list, "/gaps/<id>" → detail
  // Strip the query string (e.g. "?status=review") before path parsing;
  // views that care about query params read them off location.hash directly.
  const path = raw.split("?", 1)[0];
  const parts = path.split("/").filter(Boolean);
  if (parts.length === 0) return { route: "dashboard" };
  if (parts[0] === "gaps") {
    if (parts.length === 1) return { route: "gaps" };
    if (parts[1] === "new") return { route: "gaps_new" };
    if (parts[1] === "import") return { route: "gaps_import" };
    return { route: "gaps_detail", id: parts[1] };
  }
  if (parts[0] === "agents") return { route: "agents" };
  if (parts[0] === "chat") return { route: "chat_redirect" };
  if (parts[0] === "logs") return { route: "logs" };
  if (parts[0] === "changes") return { route: "changes" };
  if (parts[0] === "settings") return { route: "settings" };
  return { route: "dashboard" };
}

function navigate() {
  const r = parseHash();
  if (r.route === "chat_redirect") {
    // Legacy `#/chat[?gap=...]` deep links now open the dock and bounce to
    // the dashboard so the URL no longer points at a removed screen.
    const hashQs = new URLSearchParams(location.hash.split("?")[1] || "");
    const gapId = hashQs.get("gap") || null;
    openChatDock(gapId ? { gapId } : {});
    location.hash = "#/";
    return;
  }
  // Leaving the Gaps list forgets per-row bulk deselections on purpose —
  // a fresh visit starts with everything selected again.
  const prevRoute = state.currentRoute;
  if (prevRoute === "gaps" && r.route !== "gaps") {
    gapsExcludedIds.clear();
  }
  state.currentRoute = r.route;
  state.currentGap = r.id || null;
  highlightNav(r.route);
  const fn = routes[r.route];
  if (fn) fn(r);
  else $("#main").innerHTML = "<p>Not found</p>";
}

function highlightNav(route) {
  for (const a of $$(".nav a")) {
    const r = a.dataset.route;
    a.classList.toggle("active",
      r === route || (r === "gaps" && route.startsWith("gaps")));
  }
}

window.addEventListener("hashchange", navigate);

// ---- Dashboard --------------------------------------------------------------

async function renderDashboard() {
  $("#main").innerHTML = `<h2>Dashboard</h2><div id="dash"><p class="muted">Loading…</p></div>`;
  try {
    const d = await api("GET", "/api/dashboard");
    state.dashboard = d;
    drawDashboard(d);
  } catch (e) {
    $("#dash").innerHTML = `<p class="muted">Failed to load: ${htmlEscape(e.message)}</p>`;
  }
}

function drawDashboard(d) {
  // Global banners
  const banners = (d.needs_attention || []).filter((x) => x.kind === "banner")
    .map((x) => ({
      severity: x.severity || "error",
      message: x.message,
      action: x.message.includes("Claude") ? {
        label: "Re-check auth",
        onClick: async () => {
          try {
            await api("POST", "/api/settings/recheck-auth");
            toast("Pre-flight re-run requested", "info");
            await renderDashboard();
          } catch (e) {
            toast(e.message, "error");
          }
        },
      } : null,
    }));
  renderBanners(banners);

  const needsAttention = (d.needs_attention || []).filter((x) => x.kind === "filter");
  const counts = d.counts || {};
  const orderedStatuses = ["backlog", "todo", "in-progress", "review", "done", "failed", "cancelled"];
  const dash = $("#dash");
  dash.innerHTML = `
    ${needsAttention.length ? `
      <section class="card">
        <h3>Needs attention</h3>
        <div class="actions">
          ${needsAttention.map((x) => `
            <a href="#/gaps?status=${encodeURIComponent(x.filter?.status || "")}" class="btn">
              ${htmlEscape(x.message)}
            </a>`).join("")}
        </div>
      </section>` : ""}

    <section class="card-grid">
      ${orderedStatuses.map((s) => `
        <a class="card" href="#/gaps?status=${s}" style="text-decoration:none;color:inherit"
           title="${counts[s] || 0} ${s} gap${(counts[s] || 0) === 1 ? "" : "s"}">
          <div class="muted small">${s}</div>
          <div style="font-size:28px;font-weight:600;margin-top:4px">${fmtCount(counts[s] || 0)}</div>
        </a>`).join("")}
    </section>

    <section class="card">
      <h3>Reporter stats</h3>
      ${(d.reporter_stats || []).length === 0
        ? `<p class="muted">No reporter activity yet.</p>`
        : `<table class="table">
            <thead><tr>
              <th>Reporter</th>
              <th>Active</th>
              <th>Done</th>
              <th>Reported</th>
              <th>Done / Reported</th>
            </tr></thead>
            <tbody>
              ${d.reporter_stats.map((s) => `
                <tr class="reporter-stats-row"
                    data-reporter="${htmlEscape(s.reporter)}"
                    title="See Gaps reported by ${htmlEscape(s.reporter)}">
                  <td>${htmlEscape(s.reporter)}</td>
                  <td>${fmtCount(s.active)}</td>
                  <td>${fmtCount(s.done)}</td>
                  <td>${fmtCount(s.reported)}</td>
                  <td>${s.completion_rate.toFixed(1)}%</td>
                </tr>`).join("")}
            </tbody>
          </table>`}
    </section>

    <section class="row">
      <div class="card">
        <h3>Currently running</h3>
        <div class="card-scroll">
          ${(d.running || []).length === 0
            ? `<p class="muted">No agent subprocesses running.</p>`
            : (() => {
                // Anchor seconds-at-fetch + a single `data-anchor-ms`
                // timestamp so the tick can compute a live value
                // without re-fetching: shown = base + (now - anchor) / 1000.
                const anchorMs = Date.now();
                return `<table class="table"><thead><tr><th>Gap</th><th>Elapsed</th><th>Idle</th><th>PID</th></tr></thead><tbody>
                  ${d.running.map((r) => `<tr onclick="location.hash='#/gaps/${r.gap_id}'">
                    <td><code>${r.gap_id.slice(0,8)}…</code></td>
                    <td class="js-elapsed-tick"
                        data-base="${r.elapsed_seconds}"
                        data-anchor-ms="${anchorMs}">${fmtElapsed(r.elapsed_seconds)}</td>
                    <td class="js-idle-tick"
                        data-base="${r.idle_seconds}"
                        data-anchor-ms="${anchorMs}">${fmtElapsed(r.idle_seconds)}</td>
                    <td class="muted small">${r.pid}</td>
                  </tr>`).join("")}
                  </tbody></table>`;
              })()}
        </div>
      </div>

      <div class="card">
        <h3>Recent activity</h3>
        <div class="card-scroll">
          ${renderActivityList(d.activity || [])}
        </div>
      </div>
    </section>
  `;
  // Click any reporter row → deep-link into the Gaps list filtered by
  // that reporter. We use data-reporter + a delegated listener so the
  // name can contain spaces/quotes without HTML-escaping hazards.
  $$(".reporter-stats-row").forEach((row) => {
    row.addEventListener("click", () => {
      location.hash = gapsHash({ reporter: row.dataset.reporter });
    });
  });
}

function renderActivityList(entries) {
  if (!entries.length) return `<p class="muted">No activity yet.</p>`;
  return entries.map((e) => `
    <div class="log-entry ${e.severity || 'info'}">
      <div>${htmlEscape(e.message)}</div>
      <div class="meta">
        ${fmtTime(e.datetime)} · ${htmlEscape(e.category || '')}
        ${e.actor ? ' · ' + htmlEscape(e.actor) : ''}
        ${e.gap_id ? ` · <a href="#/gaps/${e.gap_id}">Gap ${e.gap_id.slice(0,8)}…</a>` : ''}
      </div>
      ${e.details ? `<details><summary class="diff-show-details">Show details</summary><pre>${htmlEscape(e.details)}</pre></details>` : ''}
    </div>`).join("");
}

// ---- Gaps: list -------------------------------------------------------------

const GAPS_DEFAULT_DIR = {
  name: "asc", status: "asc", priority: "asc",
  reporter: "asc", updated: "desc", id: "desc",
};

// Mirror Logs' entries-limit dropdown so the two screens feel consistent.
const GAPS_LIMIT_OPTIONS = [50, 100, 250, 500, 1000];
const GAPS_DEFAULT_LIMIT = 100;

function gapsHash(parts) {
  const next = new URLSearchParams();
  if (parts.q)        next.set("q", parts.q);
  if (parts.status)   next.set("status", parts.status);
  if (parts.reporter) next.set("reporter", parts.reporter);
  if (parts.severity) next.set("severity", parts.severity);
  if (parts.category) next.set("category", parts.category);
  if (parts.actor)    next.set("actor", parts.actor);
  if (parts.limit && parts.limit !== GAPS_DEFAULT_LIMIT) next.set("limit", String(parts.limit));
  if (parts.sort)     next.set("sort", parts.sort);
  if (parts.dir)      next.set("dir", parts.dir);
  return "#/gaps" + (next.toString() ? "?" + next : "");
}

async function renderGapsList() {
  renderBanners([]);
  const f = gapsFilterFromHash();
  // Preserve the filter shell's open/closed state across full re-renders
  // (Clear filters, bulk-op completion, etc.). First-ever render has no
  // prior element, so it falls through to the default (closed).
  const filterShellOpen = !!document.getElementById("gaps-filter-shell")?.open;

  $("#main").innerHTML = `
    <h2>Gaps</h2>
    <details class="filter-shell" id="gaps-filter-shell"${filterShellOpen ? " open" : ""}>
      <summary>
        <span class="filter-shell-title">Filters &amp; bulk actions</span>
        <span class="spacer"></span>
        <span class="muted small"><span id="gaps-count"></span></span>
        <span id="gaps-filtered" class="filter-pill" hidden>Filtered</span>
      </summary>
      <div class="filter-shell-body">
    <div class="filter-bar">
      <div class="filter-row filter-row-primary">
        <input type="text" id="search" class="filter-grow"
               placeholder="Search gaps…" value="${htmlEscape(f.q)}">
      </div>
      <div class="filter-row filter-row-activity">
        <select id="filter-status">
          ${["", "backlog", "todo", "in-progress", "review", "done", "failed", "cancelled"]
            .map((s) => `<option value="${s}" ${s === f.status ? "selected" : ""}>${s || "all statuses"}</option>`).join("")}
        </select>
        <select id="filter-reporter">
          <option value="" ${f.reporter === "" ? "selected" : ""}>all reporters</option>
          ${(state.reporters || []).map((r) =>
            `<option value="${htmlEscape(r.name)}" ${r.name === f.reporter ? "selected" : ""}>${htmlEscape(r.name)}</option>`).join("")}
          ${f.reporter && !(state.reporters || []).some((r) => r.name === f.reporter)
            ? `<option value="${htmlEscape(f.reporter)}" selected>${htmlEscape(f.reporter)}</option>` : ""}
        </select>
        <select id="gaps-severity">
          <option value="" ${f.severity === "" ? "selected" : ""}>all severities</option>
          <option value="info"  ${f.severity === "info"  ? "selected" : ""}>info</option>
          <option value="warn"  ${f.severity === "warn"  ? "selected" : ""}>warn</option>
          <option value="error" ${f.severity === "error" ? "selected" : ""}>error</option>
        </select>
        <select id="gaps-category"><option value="">all categories</option></select>
        <select id="gaps-actor"><option value="">all actors</option></select>
        <select id="gaps-limit">
          ${GAPS_LIMIT_OPTIONS.map((n) =>
            `<option value="${n}" ${n === f.limit ? "selected" : ""}>${n} entries</option>`).join("")}
        </select>
        <span class="spacer"></span>
        <button class="secondary" id="gaps-clear">Clear filters</button>
      </div>
      <div class="filter-row filter-row-bulk">
        <span class="muted small">Bulk update matching:</span>
        <button class="secondary small" id="bulk-set-status">Status…</button>
        <button class="secondary small" id="bulk-set-priority">Priority…</button>
        <button class="secondary small" id="bulk-set-reporter">Reporter…</button>
        <button class="secondary small" id="bulk-delete">Delete…</button>
      </div>
    </div>
      </div>
    </details>
    <div id="gaps-table"><p class="muted">Loading…</p></div>
  `;
  // In-view filter changes update the URL via replaceState (which does NOT
  // fire `hashchange`) and refresh only the table. Going through
  // `location.hash =` would trigger renderGapsList again, which rebuilds
  // `#main` from scratch — that destroys the focused search input mid-
  // keystroke. Sort-header clicks go through the same path
  // (`refreshGapsTable`); see drawGapsTable.
  $("#search").addEventListener("input", debounce(() => {
    updateGapsFilter({ q: $("#search").value });
  }, 250));
  $("#filter-status").addEventListener("change", (e) =>
    updateGapsFilter({ status: e.target.value }));
  $("#filter-reporter").addEventListener("change", (e) =>
    updateGapsFilter({ reporter: e.target.value }));
  $("#gaps-severity").addEventListener("change", (e) =>
    updateGapsFilter({ severity: e.target.value }));
  $("#gaps-category").addEventListener("change", (e) =>
    updateGapsFilter({ category: e.target.value }));
  $("#gaps-actor").addEventListener("change", (e) =>
    updateGapsFilter({ actor: e.target.value }));
  $("#gaps-limit").addEventListener("change", (e) =>
    updateGapsFilter({ limit: parseInt(e.target.value, 10) || GAPS_DEFAULT_LIMIT }));
  $("#gaps-clear").addEventListener("click", () => {
    history.replaceState(null, "", "#/gaps");
    renderGapsList();
  });
  // The bulk-action buttons read the current filter from the hash at click
  // time, so they always reflect what the user can see.
  $("#bulk-set-priority").addEventListener("click", () => openBulkModal("priority"));
  $("#bulk-set-status").addEventListener("click", () => openBulkModal("status"));
  $("#bulk-set-reporter").addEventListener("click", () => openBulkModal("reporter"));
  $("#bulk-delete").addEventListener("click", () => confirmBulkDelete());

  // Expanding / collapsing the filter shell shows / hides the per-row
  // checkbox column. Redraw from the cached results so we don't re-fetch.
  $("#gaps-filter-shell").addEventListener("toggle", () => {
    if (_lastGapsRender) {
      drawGapsTable(_lastGapsRender.gaps, _lastGapsRender.state);
    }
  });

  await refreshGapsTable();
}

// Snapshot the current Gaps filter from the URL hash.
function gapsFilterFromHash() {
  const hashQs = new URLSearchParams(location.hash.split("?")[1] || "");
  const sort = (hashQs.get("sort") || "").toLowerCase();
  const dir = (hashQs.get("dir") || "").toLowerCase();
  const effectiveSort = sort || "updated";
  const effectiveDir = dir || (GAPS_DEFAULT_DIR[effectiveSort] || "desc");
  return {
    q: hashQs.get("q") || "",
    status: hashQs.get("status") || "",
    reporter: hashQs.get("reporter") || "",
    severity: hashQs.get("severity") || "",
    category: hashQs.get("category") || "",
    actor: hashQs.get("actor") || "",
    limit: parseInt(hashQs.get("limit") || String(GAPS_DEFAULT_LIMIT), 10)
           || GAPS_DEFAULT_LIMIT,
    sort, dir,
    effectiveSort, effectiveDir,
  };
}

// Patch one or more filter fields and refresh the table without
// triggering a full view re-render. The URL stays in sync via
// `history.replaceState` so reload / share / back behave correctly.
function updateGapsFilter(patch) {
  const current = gapsFilterFromHash();
  const next = {
    q: "q" in patch ? patch.q : current.q,
    status: "status" in patch ? patch.status : current.status,
    reporter: "reporter" in patch ? patch.reporter : current.reporter,
    severity: "severity" in patch ? patch.severity : current.severity,
    category: "category" in patch ? patch.category : current.category,
    actor: "actor" in patch ? patch.actor : current.actor,
    limit: "limit" in patch ? patch.limit : current.limit,
    sort: "sort" in patch ? patch.sort : current.sort,
    dir: "dir" in patch ? patch.dir : current.dir,
  };
  history.replaceState(null, "", gapsHash(next));
  refreshGapsTable();
}

async function refreshGapsTable() {
  if (state.currentRoute !== "gaps") return;
  const f = gapsFilterFromHash();
  const params = new URLSearchParams();
  if (f.status) params.set("status", f.status);
  if (f.q) params.set("q", f.q);
  if (f.reporter) params.set("reporter", f.reporter);
  if (f.severity) params.set("severity", f.severity);
  if (f.category) params.set("category", f.category);
  if (f.actor) params.set("actor", f.actor);
  if (f.limit) params.set("limit", String(f.limit));
  if (f.sort) params.set("sort", f.sort);
  if (f.dir) params.set("dir", f.dir);
  params.set("facets", "1");
  try {
    const data = await api("GET", "/api/gaps?" + params);
    const gaps = data.gaps || [];
    const facets = data.facets || {};
    // Refresh the category / actor dropdowns from the server-side
    // distinct values — same pattern as the Logs screen.
    const catSel = $("#gaps-category");
    if (catSel) {
      const cats = facets.categories || [];
      catSel.innerHTML = `<option value="">all categories</option>` +
        cats.map((c) => `<option value="${htmlEscape(c)}" ${c === f.category ? "selected" : ""}>${htmlEscape(c)}</option>`).join("");
    }
    const actSel = $("#gaps-actor");
    if (actSel) {
      const acts = facets.actors || [];
      actSel.innerHTML = `<option value="">all actors</option>` +
        acts.map((a) => `<option value="${htmlEscape(a)}" ${a === f.actor ? "selected" : ""}>${htmlEscape(a)}</option>`).join("");
    }
    const countEl = $("#gaps-count");
    if (countEl) {
      countEl.textContent = `${gaps.length} gap${gaps.length === 1 ? "" : "s"}`;
    }
    applyGapsFilterIndicator(f);
    const renderState = {
      q: f.q, status: f.status,
      sort: f.effectiveSort, dir: f.effectiveDir,
    };
    _lastGapsRender = { gaps, state: renderState };
    drawGapsTable(gaps, renderState);
  } catch (e) {
    const tbl = $("#gaps-table");
    if (tbl) tbl.innerHTML = `<p class="muted">${htmlEscape(e.message)}</p>`;
  }
}

// IDs the user has explicitly DESELECTED from bulk operations. Every Gap
// starts selected by default — the bulk endpoints apply to "every Gap
// matching the filter, minus this set". Persisted only in-memory; the
// excluded set survives filter tweaks and re-expanding the filter shell
// but resets on a hard navigation away from the Gaps screen.
const gapsExcludedIds = new Set();

// Cached snapshot of the last refresh, so toggling the filter shell open
// or closed can redraw the table without re-fetching.
let _lastGapsRender = null;

function drawGapsTable(gaps, state) {
  const root = $("#gaps-table");
  // Selection UI follows the filter shell — only show checkboxes when the
  // shell is expanded (i.e. the user has indicated they want to interact
  // with bulk actions). Collapsed = focus on results.
  const shell = document.getElementById("gaps-filter-shell");
  const showSelection = !!(shell && shell.open);

  if (!gaps.length) {
    root.innerHTML = `<p class="muted">No gaps match the current filters.</p>`;
    return;
  }
  const columns = [
    { key: "name",     label: "Name",     sortable: true },
    { key: "status",   label: "Status",   sortable: true },
    { key: "priority", label: "Priority", sortable: true },
    { key: "reporter", label: "Reporter", sortable: true },
    { key: "updated",  label: "Updated",  sortable: true },
  ];
  const sortHeads = columns.map((c) => {
    if (!c.sortable) {
      return `<th>${c.label}</th>`;
    }
    const isActive = c.key === state.sort;
    const arrow = isActive
      ? (state.dir === "asc" ? "↑" : "↓")
      : `<span class="sort-arrow-placeholder">↕</span>`;
    return `<th class="sortable ${isActive ? "active" : ""}"
                data-sort-key="${c.key}">
              ${c.label} <span class="sort-arrow">${arrow}</span>
            </th>`;
  }).join("");
  const selectionHead = showSelection
    ? `<th class="gap-select-col">
         <input type="checkbox" id="gap-select-all"
                aria-label="Select all on this page">
       </th>`
    : "";
  root.innerHTML = `
    <table class="table">
      <thead><tr>${selectionHead}${sortHeads}</tr></thead>
      <tbody>
        ${gaps.map((g) => {
          const selected = !gapsExcludedIds.has(g.id);
          const cell = showSelection
            ? `<td class="gap-select-col">
                 <input type="checkbox" class="gap-select"
                        data-id="${g.id}"
                        ${selected ? "checked" : ""}
                        aria-label="Select gap ${htmlEscape(g.name)}">
               </td>`
            : "";
          return `<tr data-id="${g.id}">
            ${cell}
            <td>${htmlEscape(g.name)}</td>
            <td><span class="status-pill ${g.status}">${g.status}</span></td>
            <td><span class="priority-pill priority-${g.priority || "low"}">${g.priority || "low"}</span></td>
            <td class="muted small">${g.reporter ? htmlEscape(g.reporter) : "—"}</td>
            <td class="muted small">${fmtTime(g.updated)}</td>
          </tr>`;
        }).join("")}
      </tbody>
    </table>
  `;
  // Row click navigates to gap detail — but a click on the checkbox (or
  // its surrounding td) should toggle selection, not navigate.
  $$(".table tbody tr", root).forEach((row) => {
    row.addEventListener("click", (e) => {
      if (e.target.closest(".gap-select-col")) return;
      location.hash = "#/gaps/" + row.dataset.id;
    });
  });
  $$(".gap-select", root).forEach((cb) => {
    cb.addEventListener("click", (e) => e.stopPropagation());
    cb.addEventListener("change", (e) => {
      const id = e.target.dataset.id;
      if (e.target.checked) gapsExcludedIds.delete(id);
      else gapsExcludedIds.add(id);
      _updateSelectAllState(gaps);
    });
  });
  const selectAll = root.querySelector("#gap-select-all");
  if (selectAll) {
    _updateSelectAllState(gaps);
    selectAll.addEventListener("click", (e) => {
      e.stopPropagation();
      const shouldCheck = selectAll.checked;
      for (const g of gaps) {
        if (shouldCheck) gapsExcludedIds.delete(g.id);
        else gapsExcludedIds.add(g.id);
      }
      // Re-sync the row checkboxes without a full redraw.
      $$(".gap-select", root).forEach((cb) => {
        cb.checked = !gapsExcludedIds.has(cb.dataset.id);
      });
      selectAll.indeterminate = false;
    });
  }
  $$(".table th.sortable", root).forEach((th) => {
    th.addEventListener("click", () => {
      const key = th.dataset.sortKey;
      let nextDir;
      if (key === state.sort) {
        // Same column — flip the direction.
        nextDir = state.dir === "asc" ? "desc" : "asc";
      } else {
        // New column — use its natural default direction.
        nextDir = GAPS_DEFAULT_DIR[key] || "desc";
      }
      updateGapsFilter({ sort: key, dir: nextDir });
    });
  });
}

// Sync the header checkbox's checked / indeterminate state to the page
// selection: all checked → checked, none checked → unchecked, mix →
// indeterminate. Called after every selection change.
function _updateSelectAllState(gaps) {
  const master = document.getElementById("gap-select-all");
  if (!master) return;
  let selected = 0;
  for (const g of gaps) if (!gapsExcludedIds.has(g.id)) selected++;
  if (selected === 0) {
    master.checked = false;
    master.indeterminate = false;
  } else if (selected === gaps.length) {
    master.checked = true;
    master.indeterminate = false;
  } else {
    master.checked = false;
    master.indeterminate = true;
  }
}

// ---- Gaps: bulk-update modal ------------------------------------------------
//
// Each bulk action prompts for a new value and confirms the change against
// the *current* filter (read from the URL hash at click time). The server
// re-runs the filter query, so what the user sees in the table is what gets
// updated — no client-side ID list to drift out of sync. Exactly one field
// is changed per call so the confirmation reads cleanly.

const BULK_PRIORITY_OPTIONS = ["low", "medium", "high"];
const BULK_STATUS_OPTIONS = [
  "backlog", "todo", "in-progress", "review", "done", "failed", "cancelled",
];

async function openBulkModal(field) {
  // Snapshot the current filter so the modal + the server-side bulk
  // operation see exactly what the user sees in the table.
  const f = gapsFilterFromHash();
  const filter = {
    status: f.status, q: f.q, reporter: f.reporter,
    severity: f.severity, category: f.category, actor: f.actor,
  };
  const filterDesc = describeGapsFilter(filter);
  // When the filter shell is open, the user may have unchecked some of
  // the rows. Translate that into an explicit exclude list and a
  // selected-count for the modal text.
  const excludeIds = _selectionSnapshot();
  const matchingCount = _lastGapsRender?.gaps?.length || 0;
  const selectedCount = matchingCount - excludeIds.filter(
    (id) => (_lastGapsRender?.gaps || []).some((g) => g.id === id),
  ).length;
  const countText = excludeIds.length && _lastGapsRender
    ? `${selectedCount} of ${matchingCount} selected`
    : ($("#gaps-count")?.textContent || "").trim();
  const label = { priority: "Priority", status: "Status", reporter: "Reporter" }[field];

  let valueControlHtml = "";
  if (field === "priority") {
    valueControlHtml = `
      <select class="modal-input" id="bulk-value-priority" style="width:100%">
        ${BULK_PRIORITY_OPTIONS.map((p) => `<option value="${p}">${p}</option>`).join("")}
      </select>`;
  } else if (field === "status") {
    valueControlHtml = `
      <select class="modal-input" id="bulk-value-status" style="width:100%">
        ${BULK_STATUS_OPTIONS.map((s) => `<option value="${s}">${s}</option>`).join("")}
      </select>
      <p class="muted small" style="margin-top:6px">
        Bookkeeping-only — won't kill a running subprocess or clean up a
        worktree. Use the per-Gap actions for full state transitions.
      </p>`;
  } else if (field === "reporter") {
    const opts = (state.reporters || [])
      .map((r) => `<option value="${htmlEscape(r.name)}">${htmlEscape(r.name)}</option>`)
      .join("");
    valueControlHtml = `
      <select class="modal-input" id="bulk-value-reporter" style="width:100%">
        <option value="">— pick reporter —</option>
        ${opts}
      </select>
      <p class="muted small" style="margin-top:6px">
        Rewrites the latest round's <strong>reporter</strong> on each Gap.
        Earlier rounds keep their original reporter.
      </p>`;
  }

  const body = () => `
    <div class="modal-title">Bulk set ${htmlEscape(label.toLowerCase())}</div>
    <div class="modal-body">
      <div class="muted small" style="margin-bottom:8px">
        Applies to ${htmlEscape(countText || "all matching")} —
        ${htmlEscape(filterDesc)}.
      </div>
      <label for="bulk-value-${field}">New ${htmlEscape(label.toLowerCase())}</label>
      ${valueControlHtml}
    </div>
    <div class="modal-actions">
      <button class="secondary" data-cancel>Cancel</button>
      <button data-ok>Apply</button>
    </div>`;
  const next = await _openModal(
    body, { cancel: null, ok: "" }, ".modal-input",
  );
  if (next === null) return;
  if (!next) return;          // user opened the picker but didn't choose
  try {
    const r = await api("POST", "/api/gaps/bulk", {
      filter, exclude_ids: excludeIds, update: { [field]: next },
    });
    toast(`Updated ${r.updated} gap${r.updated === 1 ? "" : "s"}`, "info");
    // Preserve the user's unchecked rows across the refresh — they
    // explicitly opted those out of the operation that just ran and
    // will likely want them excluded from follow-up actions too.
    // Stale IDs (rows that no longer match the filter) are harmless;
    // they're just ignored at the next selection-state pass.
    await renderGapsList();
  } catch (e) {
    toast(`Bulk update failed: ${e.message}`, "error");
  }
}

// Frozen-at-call-time copy of the user's deselected IDs (so a slow
// network request doesn't see live edits).
function _selectionSnapshot() {
  return Array.from(gapsExcludedIds);
}

// Highlight each non-default Gaps filter control with the accent
// border + show the "Filtered" pill next to the count when any filter
// is active. Called after every table refresh.
function applyGapsFilterIndicator(f) {
  const active = {
    "search": !!f.q,
    "filter-status": !!f.status,
    "filter-reporter": !!f.reporter,
    "gaps-severity": !!f.severity,
    "gaps-category": !!f.category,
    "gaps-actor": !!f.actor,
    "gaps-limit": f.limit !== GAPS_DEFAULT_LIMIT,
  };
  let anyActive = false;
  for (const [id, on] of Object.entries(active)) {
    const el = document.getElementById(id);
    if (!el) continue;
    el.classList.toggle("filter-active", on);
    if (on) anyActive = true;
  }
  const pill = $("#gaps-filtered");
  if (pill) pill.hidden = !anyActive;
  const tbl = $("#gaps-table");
  if (tbl) tbl.classList.toggle("results-filtered", anyActive);
}

async function confirmBulkDelete() {
  const f = gapsFilterFromHash();
  const filter = {
    status: f.status, q: f.q, reporter: f.reporter,
    severity: f.severity, category: f.category, actor: f.actor,
  };
  const filterDesc = describeGapsFilter(filter);
  const excludeIds = _selectionSnapshot();
  const matchingCount = _lastGapsRender?.gaps?.length || 0;
  const selectedCount = matchingCount - excludeIds.filter(
    (id) => (_lastGapsRender?.gaps || []).some((g) => g.id === id),
  ).length;
  const countText = excludeIds.length && _lastGapsRender
    ? `${selectedCount} of ${matchingCount} selected gaps`
    : (($("#gaps-count")?.textContent || "matching gaps").trim());
  const ok = await modalConfirm(
    `Permanently delete ${countText} (${filterDesc})? This cancels any ` +
    "running subprocesses, removes worktrees and branches for non-done " +
    "Gaps, and erases their gap.json files. This cannot be undone.",
    {
      title: "Delete Gaps",
      okLabel: `Delete ${countText}`,
      cancelLabel: "Keep them",
      danger: true,
    },
  );
  if (!ok) return;
  try {
    const r = await api("POST", "/api/gaps/bulk/delete", {
      filter, exclude_ids: excludeIds,
    });
    const failedN = (r.failures || []).length;
    if (failedN) {
      toast(`Deleted ${r.deleted} gap${r.deleted === 1 ? "" : "s"}, ` +
            `${failedN} failed.`, "warn");
    } else {
      toast(`Deleted ${r.deleted} gap${r.deleted === 1 ? "" : "s"}.`, "info");
    }
    // Preserve the user's unchecked rows so follow-up bulk actions
    // continue to skip them. IDs of deleted gaps drop out of the next
    // fetch naturally — they remain in the set but are inert.
    await renderGapsList();
  } catch (e) {
    toast(`Bulk delete failed: ${e.message}`, "error");
  }
}

function describeGapsFilter(filter) {
  const parts = [];
  if (filter.status)   parts.push(`status=${filter.status}`);
  if (filter.reporter) parts.push(`reporter=${filter.reporter}`);
  if (filter.q)        parts.push(`q="${filter.q}"`);
  if (filter.severity) parts.push(`severity=${filter.severity}`);
  if (filter.category) parts.push(`category=${filter.category}`);
  if (filter.actor)    parts.push(`actor=${filter.actor}`);
  return parts.length ? parts.join(", ") : "all gaps";
}

function debounce(fn, ms) {
  let t;
  return (...args) => { clearTimeout(t); t = setTimeout(() => fn(...args), ms); };
}

// Run `fn` while the button shows a busy label and is disabled. Used for
// operations that may take noticeable time (verify, fetch+merge+push, auth
// recheck, etc.) so the user sees that something is happening and can't
// accidentally double-fire the request.
async function withButtonBusy(btn, busyLabel, fn) {
  if (!btn) return await fn();
  const wasDisabled = btn.disabled;
  const orig = btn.textContent;
  btn.disabled = true;
  btn.textContent = busyLabel;
  try {
    return await fn();
  } finally {
    // The button may have been re-rendered by the awaited work (e.g., a
    // reload of the view); setting properties on a detached node is a no-op.
    btn.disabled = wasDisabled;
    btn.textContent = orig;
  }
}

// ---- Gaps: detail -----------------------------------------------------------

async function renderGapDetail(r) {
  state.currentGap = r.id;
  $("#main").innerHTML = `<p class="muted">Loading…</p>`;
  await loadGapDetail(r.id);
}

async function loadGapDetail(gapId) {
  try {
    const { gap } = await api("GET", "/api/gaps/" + gapId);
    drawGapDetail(gap);
  } catch (e) {
    $("#main").innerHTML = `<p class="muted">Could not load gap: ${htmlEscape(e.message)}</p>`;
  }
}

// User-driven workflow transitions for a Gap. Each state declares its
// `back` and `forward` neighbors; `in-progress` has neither because the
// dispatcher owns that state (agent picks up todo → in-progress → review
// automatically). Forward from `review` goes through the dedicated
// /verify endpoint (real git merge + push); every other transition is a
// status PATCH with no workflow side effects.
//
// failed / cancelled only expose a back arrow — there's no obvious
// forward target for them (they're terminal-ish in opposite directions
// from done). Use back to send the Gap back to todo and rerun.
const GAP_WORKFLOW = {
  backlog:   { forward: { label: "Todo →",     next: "todo"   } },
  todo:      { back:    { label: "← Backlog",  next: "backlog" },
               forward: { label: "Review →",   next: "review" } },
  // in-progress: no user buttons — dispatcher transitions in and out.
  review:    { back:    { label: "← Todo",     next: "todo"   },
               forward: { label: "Verify →",   next: "done", verify: true } },
  done:      { back:    { label: "← Review",   next: "review" } },
  failed:    { back:    { label: "← Todo",     next: "todo"   } },
  cancelled: { back:    { label: "← Todo",     next: "todo"   } },
};

function drawGapDetail(gap) {
  renderBanners([]);
  // Preserve the notes-card open state across re-renders of the same gap so
  // saving a note (or an SSE-driven refresh) doesn't snap it shut.
  const notesOpen = document.querySelector(
    `.notes-card[data-gap-id="${gap.id}"]`,
  )?.open ?? false;
  // Same idea for the per-round wrapper and its inner Logs disclosure.
  // SSE round_log_added events trigger a full drawGapDetail re-render
  // every time the agent emits a new event; without this, an expanded
  // round or expanded Logs collapses each time a new log arrives.
  const prevRoundOpen = {};
  const prevLogsOpen = {};
  document.querySelectorAll("details.round[data-round-idx]").forEach((el) => {
    prevRoundOpen[el.dataset.roundIdx] = el.open;
  });
  document.querySelectorAll('details[data-role="round-logs"][data-round-idx]').forEach((el) => {
    prevLogsOpen[el.dataset.roundIdx] = el.open;
  });
  const rounds = gap.rounds || [];
  // Merge gap-scoped activity into each round so users see lifecycle events
  // and runner errors alongside the round's own logs[]. Each activity entry
  // goes into the latest round whose `created` is at or before the entry's
  // datetime.
  attachActivityToRounds(rounds, gap.activity || []);
  const latest = rounds[rounds.length - 1] || null;
  const failureBanner = computeFailureBanner(gap, latest);

  const isLatestEditable = (gap.status === "backlog" ||
                            gap.status === "todo" ||
                            gap.status === "failed");
  const cancelEnabled = !["done", "cancelled"].includes(gap.status);
  // Chat is always available — the value is the Gap context the runner
  // primes into claude's session. The chat runs in the Gap's worktree
  // when one exists and falls back to the client repo when it doesn't.

  // Dynamic workflow buttons: each state shows the previous/next state
  // it can move to as back / forward buttons. The user-driven workflow
  // skips `in-progress` (the dispatcher owns that). Forward from review
  // goes through the existing `verify` endpoint (the only transition
  // with real git side effects); everything else is a bookkeeping
  // status update via PATCH /api/gaps/<id>.
  const workflow = GAP_WORKFLOW[gap.status] || {};
  const backBtn = workflow.back ? `
    <button id="btn-state-back">${htmlEscape(workflow.back.label)}</button>
  ` : "";
  const forwardBtn = workflow.forward ? `
    <button id="btn-state-forward">${htmlEscape(workflow.forward.label)}</button>
  ` : "";

  $("#main").innerHTML = `
    <div class="gap-detail">
      <div class="row" style="align-items:center;margin-bottom:8px">
        <h2 style="margin:0">${htmlEscape(gap.name)}</h2>
        <span class="status-pill ${gap.status}">${gap.status}</span>
        <span class="priority-pill priority-${gap.priority || "low"}">priority: ${gap.priority || "low"}</span>
      </div>
      <div class="actions" style="margin-bottom:10px">
        ${backBtn}
        ${forwardBtn}
        <button id="btn-chat">Open Chat</button>
        <button class="warn" id="btn-rename">Rename</button>
        <button class="warn" id="btn-priority">Change Priority</button>
        <button class="warn" id="btn-cancel" ${cancelEnabled ? "" : "disabled"}>Cancel Gap</button>
        <button class="danger" id="btn-delete">Delete</button>
      </div>
      <div class="muted small" style="margin-bottom:14px">
        ID <code>${gap.id}</code> · created ${fmtTime(gap.created)} · updated ${fmtTime(gap.updated)}
        ${gap.branch_name ? ` · branch <code>${gap.branch_name}</code>` : ""}
      </div>

      ${failureBanner ? `
        <div class="banner ${failureBanner.severity}">
          <span class="banner-msg">${htmlEscape(failureBanner.message)}</span>
          <span class="banner-actions">${failureBanner.actionsHtml}</span>
        </div>` : ""}

      <h3>Rounds (${rounds.length})</h3>
      ${rounds.length === 0 ? `<p class="muted">No rounds yet.</p>` :
        rounds.map((rnd, idx) => renderRound(
          rnd, idx,
          idx === rounds.length - 1,
          isLatestEditable && idx === rounds.length - 1,
          prevRoundOpen, prevLogsOpen,
        )).join("")}

      ${(gap.status === "backlog" || gap.status === "todo" || gap.status === "failed") ? `
        <div class="card" style="margin-top:14px">
          <h3>Edit latest round</h3>
          ${renderRoundForm("edit", latest)}
        </div>` : ""}

      ${gap.status === "review" ? `
        <div class="card" style="margin-top:14px">
          <h3>Submit follow-up round</h3>
          ${renderRoundForm("submit", null)}
        </div>` : ""}

      <details class="card notes-card" data-gap-id="${gap.id}" style="margin-top:14px" ${notesOpen ? "open" : ""}>
        <summary class="notes-card-summary">
          <span><strong>Notes (${(gap.notes || []).length})</strong></span>
          <span class="muted small">Saved to gap.json and included in attached
            Chat context.</span>
          <span class="spacer"></span>
          <span id="gap-notes-status" class="muted small"></span>
        </summary>
        <div style="margin-top:10px">
          <div id="notes-list">
            ${(gap.notes || []).length === 0
              ? `<p class="muted small">No notes yet.</p>`
              : (gap.notes || []).map(renderNote).join("")}
          </div>
          <details class="note-composer" style="margin-top:10px">
            <summary>+ Add a note</summary>
            <div class="form-row" style="margin-top:8px">
              <textarea id="new-note-body" rows="3"
                        placeholder="Anything Claude or the team should know — links to specs, prior decisions, constraints, related code paths."></textarea>
            </div>
            <div class="actions">
              <button id="btn-add-note">Save note</button>
            </div>
          </details>
        </div>
      </details>
    </div>
  `;

  $("#btn-chat")?.addEventListener("click", () => {
    openChatDock({ gapId: gap.id });
  });

  // Workflow back / forward buttons. Forward from `review` calls the
  // existing /verify endpoint (the only transition with real git side
  // effects); every other arrow is a plain status PATCH.
  const wireWorkflow = (btnId, target) => {
    if (!target) return;
    $(btnId)?.addEventListener("click", async () => {
      const btn = $(btnId);
      const busyLabel = target.verify ? "Verifying…" : `Moving to ${target.next}…`;
      await withButtonBusy(btn, busyLabel, async () => {
        try {
          if (target.verify) {
            const r = await api("POST", `/api/gaps/${gap.id}/verify`);
            if (r.ok) toast(r.message || "Verified", "info");
            else toast(r.message || "Verify did not complete", "error");
          } else {
            await api("PATCH", `/api/gaps/${gap.id}`, { status: target.next });
            toast(`Moved to ${target.next}`, "info");
          }
          await loadGapDetail(gap.id);
        } catch (e) { toast(e.message, "error"); }
      });
    });
  };
  wireWorkflow("#btn-state-back", workflow.back);
  wireWorkflow("#btn-state-forward", workflow.forward);
  $("#btn-rename")?.addEventListener("click", async () => {
    const name = await modalPrompt("New name", gap.name,
                                   { title: "Rename Gap" });
    if (!name || !name.trim()) return;
    try {
      await api("PATCH", "/api/gaps/" + gap.id, { name: name.trim() });
      await loadGapDetail(gap.id);
    } catch (e) { toast(e.message, "error"); }
  });
  $(".note-composer")?.addEventListener("toggle", (e) => {
    if (e.target.open) $("#new-note-body")?.focus();
  });
  $("#btn-add-note")?.addEventListener("click", async () => {
    const btn = $("#btn-add-note");
    const ta = $("#new-note-body");
    if (!ta) return;
    const body = (ta.value || "").trim();
    if (!body) return toast("Note can't be empty", "error");
    const author = state.lastReporter || "";
    const nextNotes = [...(gap.notes || []), { author, body }];
    await withButtonBusy(btn, "Saving…", async () => {
      try {
        await api("PATCH", "/api/gaps/" + gap.id, { notes: nextNotes });
        toast("Note added", "info");
        await loadGapDetail(gap.id);
      } catch (e) { toast(e.message, "error"); }
    });
  });
  $$("[data-note-edit]").forEach((el) => el.addEventListener("click", async (e) => {
    e.preventDefault();
    const id = el.dataset.noteEdit;
    const existing = (gap.notes || []).find((n) => n.id === id);
    if (!existing) return;
    const body = await modalPrompt(
      "Edit note", existing.body,
      { title: "Edit note", okLabel: "Save" },
    );
    if (body === null) return;
    const trimmed = (body || "").trim();
    if (!trimmed) return toast("Note can't be empty", "error");
    const nextNotes = (gap.notes || []).map(
      (n) => n.id === id ? { ...n, body: trimmed } : n,
    );
    try {
      await api("PATCH", "/api/gaps/" + gap.id, { notes: nextNotes });
      toast("Note updated", "info");
      await loadGapDetail(gap.id);
    } catch (err) { toast(err.message, "error"); }
  }));
  $$("[data-note-delete]").forEach((el) => el.addEventListener("click", async (e) => {
    e.preventDefault();
    const id = el.dataset.noteDelete;
    const ok = await modalConfirm(
      "Delete this note?",
      { title: "Delete note", okLabel: "Delete", danger: true },
    );
    if (!ok) return;
    const nextNotes = (gap.notes || []).filter((n) => n.id !== id);
    try {
      await api("PATCH", "/api/gaps/" + gap.id, { notes: nextNotes });
      toast("Note deleted", "info");
      await loadGapDetail(gap.id);
    } catch (err) { toast(err.message, "error"); }
  }));
  $("#btn-priority")?.addEventListener("click", async () => {
    const current = gap.priority || "low";
    const body = () => `
      <div class="modal-title">Change priority</div>
      <div class="modal-body">
        <label for="modal-priority-select">Priority</label>
        <select class="modal-input" id="modal-priority-select" style="width:100%">
          ${["low", "medium", "high"].map((p) =>
            `<option value="${p}" ${p === current ? "selected" : ""}>${p}</option>`,
          ).join("")}
        </select>
      </div>
      <div class="modal-actions">
        <button class="secondary" data-cancel>Cancel</button>
        <button data-ok>Save</button>
      </div>`;
    const next = await _openModal(
      body, { cancel: null, ok: current }, ".modal-input",
    );
    if (next === null || next === current) return;
    try {
      await api("PATCH", "/api/gaps/" + gap.id, { priority: next });
      toast(`Priority set to ${next}`, "info");
      await loadGapDetail(gap.id);
    } catch (err) {
      toast(err.message, "error");
    }
  });
  $("#btn-cancel")?.addEventListener("click", async () => {
    const btn = $("#btn-cancel");
    if (btn.disabled) return;
    const ok = await modalConfirm(
      "Cancel this Gap? Any running subprocess will be stopped and the worktree + branch cleaned up.",
      { title: "Cancel Gap", okLabel: "Cancel Gap", danger: true,
        cancelLabel: "Keep working" },
    );
    if (!ok) return;
    await withButtonBusy(btn, "Cancelling…", async () => {
      try {
        await api("POST", `/api/gaps/${gap.id}/cancel`);
        toast("Cancelled", "info");
        await loadGapDetail(gap.id);
      } catch (e) { toast(e.message, "error"); }
    });
  });
  $("#btn-delete")?.addEventListener("click", async () => {
    const ok = await modalConfirm(
      `Delete Gap "${gap.name}"? This cannot be undone.`,
      { title: "Delete Gap", okLabel: "Delete", danger: true },
    );
    if (!ok) return;
    try {
      await api("DELETE", "/api/gaps/" + gap.id);
      location.hash = "#/gaps";
    } catch (e) { toast(e.message, "error"); }
  });

  bindFailureBannerActions(gap);
  bindRoundFormSubmit(gap);
}

function attachActivityToRounds(rounds, activity) {
  // Reset any prior merge — we always recompute from gap.activity + rnd.logs.
  for (const r of rounds) r._mergedLogs = (r.logs || []).slice();
  if (!rounds.length) return;
  // Sort rounds ascending by `created`; bucket each activity entry into the
  // last round whose `created` ≤ entry.datetime.
  const bucket = (ts) => {
    let idx = 0;
    for (let i = 0; i < rounds.length; i++) {
      if ((rounds[i].created || "") <= ts) idx = i;
      else break;
    }
    return idx;
  };
  for (const a of activity) {
    const idx = bucket(a.datetime || "");
    rounds[idx]._mergedLogs.push(a);
  }
  // Sort each round's merged logs by datetime ascending.
  for (const r of rounds) {
    r._mergedLogs.sort((x, y) => (x.datetime || "").localeCompare(y.datetime || ""));
  }
}

function renderRound(rnd, idx, isLatest, editable,
                     prevRoundOpen = {}, prevLogsOpen = {}) {
  const logs = rnd._mergedLogs || rnd.logs || [];
  // Preserve the user's open/closed choice across re-renders. New rounds
  // (no prior entry in the snapshot) default to "open on latest" — the
  // historical behavior — and Logs default closed.
  const key = String(idx);
  const roundOpen = key in prevRoundOpen ? prevRoundOpen[key] : isLatest;
  const logsOpen = key in prevLogsOpen ? prevLogsOpen[key] : false;
  return `
    <details class="round" data-round-idx="${idx}" ${roundOpen ? "open" : ""}>
      <summary class="round-head">
        <strong>Round ${idx + 1}</strong>
        ${isLatest ? `<span class="status-pill review">latest</span>` : ""}
        <span class="spacer"></span>
        <span class="muted small">${htmlEscape(rnd.reporter || "(no reporter)")} · ${fmtTime(rnd.created)}</span>
      </summary>
      <div class="round-body">
        <dl class="pair">
          <dt>actual</dt><dd>${htmlEscape(rnd.actual || "").replace(/\n/g, "<br>")}</dd>
          <dt>target</dt><dd>${htmlEscape(rnd.target || "").replace(/\n/g, "<br>")}</dd>
        </dl>
        ${logs.length ? `
          <details data-role="round-logs" data-round-idx="${idx}" ${logsOpen ? "open" : ""}>
            <summary>Logs (${logs.length})</summary>
            ${logs.map((l) => renderLogEntry(l)).join("")}
          </details>` : `<p class="muted small">No logs.</p>`}
      </div>
    </details>
  `;
}

function renderNote(n) {
  const firstLine = (n.body || "").split("\n", 1)[0];
  const preview = firstLine.length > 80
    ? firstLine.slice(0, 77) + "…"
    : firstLine;
  const meta = [n.author, n.created ? fmtTime(n.created) : ""].filter(Boolean).join(" · ");
  return `
    <details class="note">
      <summary>
        <span class="note-preview">${htmlEscape(preview || "(empty)")}</span>
        ${meta ? `<span class="muted small note-meta">${htmlEscape(meta)}</span>` : ""}
      </summary>
      <div class="note-body">${htmlEscape(n.body || "").replace(/\n/g, "<br>")}</div>
      <div class="actions" style="margin-top:6px">
        <button class="secondary" data-note-edit="${htmlEscape(n.id)}">Edit</button>
        <button class="danger" data-note-delete="${htmlEscape(n.id)}">Delete</button>
      </div>
    </details>`;
}

function renderLogEntry(l) {
  return `
    <div class="log-entry ${l.severity || 'info'}">
      <div>${htmlEscape(l.message)}</div>
      <div class="meta">${fmtTime(l.datetime)} · ${htmlEscape(l.category || '')}${l.actor ? ' · ' + htmlEscape(l.actor) : ''}</div>
      ${l.details ? `<details><summary class="diff-show-details">Show details</summary><pre>${htmlEscape(l.details)}</pre></details>` : ''}
    </div>`;
}

function renderRoundForm(kind, prefill) {
  const actual = prefill?.actual || "";
  const target = prefill?.target || "";
  const reporter = state.lastReporter || "";
  if (!reporter) return renderPickReporterNotice();
  const submitLabel = kind === "submit" ? "Submit new round" : "Save changes";
  return `
    <form id="round-form" data-kind="${kind}">
      <div class="muted small" style="margin-bottom:8px">
        Submitting as <strong class="js-reporter-name">${htmlEscape(reporter)}</strong>
        — change in the top-right reporter selector.
      </div>
      <div class="form-row">
        <label>Actual (current behavior)</label>
        <textarea name="actual" placeholder="What's happening today?">${htmlEscape(actual)}</textarea>
      </div>
      <div class="form-row">
        <label>Target (desired behavior)</label>
        <textarea name="target" placeholder="What should be happening?">${htmlEscape(target)}</textarea>
      </div>
      <div class="actions">
        <button type="submit">${submitLabel}</button>
      </div>
    </form>
  `;
}

function renderPickReporterNotice() {
  return `
    <p class="muted">
      Pick a reporter in the top-right selector to enable this form.
    </p>
  `;
}

function bindRoundFormSubmit(gap) {
  const form = $("#round-form");
  if (!form) return;
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const reporter = state.lastReporter || "";
    if (!reporter) return toast("Pick a reporter in the top-right selector", "error");
    const fd = new FormData(form);
    const actual = (fd.get("actual") || "").toString().trim();
    const target = (fd.get("target") || "").toString().trim();
    if (!actual && !target) return toast("Provide actual or target", "error");
    const kind = form.dataset.kind;
    try {
      if (kind === "submit") {
        await api("POST", `/api/gaps/${gap.id}/rounds`, { reporter, actual, target });
        toast("New round submitted", "info");
      } else {
        await api("PATCH", `/api/gaps/${gap.id}/rounds/latest`, { reporter, actual, target });
        toast("Round updated", "info");
      }
      await loadGapDetail(gap.id);
    } catch (err) {
      toast(err.message, "error");
    }
  });
}

function computeFailureBanner(gap, latest) {
  if (gap.status === "failed") {
    const lastLog = (latest?.logs || []).slice(-1)[0];
    return {
      severity: "error",
      message: lastLog?.message || "Agent run failed",
      actionsHtml: "",
    };
  }
  if (gap.status === "review") {
    // Was the last log an error? Then we treat it as a stuck-review state.
    const errLog = (latest?.logs || []).slice().reverse().find((l) => l.severity === "error");
    if (errLog) {
      return {
        severity: "error",
        message: errLog.message || "Review needs attention",
        actionsHtml: "",
      };
    }
  }
  return null;
}

function bindFailureBannerActions(_gap) {
  // No banner-level actions: Verify / Open Chat / Reopen / Rename / Cancel /
  // Delete all live in the unified action menu at the top of the page.
}

// ---- Gaps: new --------------------------------------------------------------

async function renderGapNew() {
  // The "New Gap" screen is a modal layered over the gaps list — render the
  // list underneath so the URL #/gaps/new still has meaningful context, then
  // open the modal on top.
  await renderGapsList();
  openNewGapModal();
}

let _newGapModalOpen = false;

function openNewGapModal() {
  if (_newGapModalOpen) return;
  const reporter = state.lastReporter || "";
  if (!reporter) {
    toast("Pick a reporter in the top-right selector first", "error");
    return;
  }
  _newGapModalOpen = true;

  const root = document.createElement("div");
  root.className = "modal-backdrop";
  root.innerHTML = `
    <div class="modal" role="dialog" aria-modal="true" aria-labelledby="new-gap-title" style="max-width:560px">
      <div class="modal-title" id="new-gap-title">New Gap</div>
      <div class="modal-body">
        <div class="muted small" style="margin-bottom:8px">
          Submitting as <strong class="js-reporter-name">${htmlEscape(reporter)}</strong>
          — change in the top-right reporter selector.
        </div>
        <form id="new-gap-form">
          <div class="form-row">
            <label>Actual (current behavior)</label>
            <textarea name="actual" placeholder="What's happening today?"></textarea>
          </div>
          <div class="form-row">
            <label>Target (desired behavior)</label>
            <textarea name="target" placeholder="What should be happening?"></textarea>
          </div>
          <div class="form-row">
            <label>Priority</label>
            <select name="priority">
              <option value="low" selected>Low (default)</option>
              <option value="medium">Medium</option>
              <option value="high">High</option>
            </select>
          </div>
          <p class="muted small">
            A name will be auto-generated from the text above — you can rename
            the Gap on its detail page afterwards. High-priority Gaps run before
            medium, and medium before low.
          </p>
        </form>
      </div>
      <div class="modal-actions">
        <button class="secondary" data-cancel>Cancel</button>
        <button data-ok>Create Gap</button>
      </div>
    </div>
  `;
  document.body.appendChild(root);

  let closed = false;
  function close(navigateAway) {
    if (closed) return;
    closed = true;
    _newGapModalOpen = false;
    document.removeEventListener("keydown", onKey, true);
    root.remove();
    // If the modal was opened via the #/gaps/new route, send the user back
    // to the gaps list when they dismiss it (so the URL no longer points at
    // a "screen" that no longer exists).
    if (navigateAway && location.hash.startsWith("#/gaps/new")) {
      location.hash = "#/gaps";
    }
  }
  function onKey(e) {
    if (e.key === "Escape") {
      e.preventDefault();
      close(true);
    } else if (e.key === "Enter") {
      // Allow Enter inside textareas to insert newlines.
      if (e.target && e.target.tagName === "TEXTAREA") return;
      e.preventDefault();
      submit();
    }
  }
  document.addEventListener("keydown", onKey, true);
  root.addEventListener("click", (e) => {
    if (e.target === root) close(true);
  });
  root.querySelector("[data-cancel]").addEventListener("click", () => close(true));
  root.querySelector("[data-ok]").addEventListener("click", submit);

  const form = root.querySelector("#new-gap-form");
  form.addEventListener("submit", (e) => { e.preventDefault(); submit(); });

  async function submit() {
    const currentReporter = state.lastReporter || "";
    if (!currentReporter) return toast("Pick a reporter in the top-right selector", "error");
    const fd = new FormData(form);
    const actual = (fd.get("actual") || "").toString().trim();
    const target = (fd.get("target") || "").toString().trim();
    const priority = (fd.get("priority") || "low").toString();
    if (!actual && !target) return toast("Provide actual or target", "error");
    try {
      await api("POST", "/api/gaps", {
        reporter: currentReporter, actual, target, priority,
      });
      toast("Gap created", "info");
      // Stay on whatever screen the modal was layered over — Dashboard,
      // Gaps list, etc. `close(true)` only re-routes if we came in via
      // the `#/gaps/new` deep link; otherwise the underlying hash is
      // preserved so the user doesn't lose their place.
      close(true);
    } catch (err) {
      toast(err.message, "error");
    }
  }

  const firstField = root.querySelector("textarea[name='actual']");
  if (firstField) firstField.focus();
}

// ---- Gaps: import -----------------------------------------------------------

async function renderGapImport() {
  // Import is a modal layered over the gaps list, mirroring New Gap.
  await renderGapsList();
  openImportModal();
}

let _importModalOpen = false;

function openImportModal() {
  if (_importModalOpen) return;
  const reporter = state.lastReporter || "";
  if (!reporter) {
    toast("Pick a reporter in the top-right selector first", "error");
    return;
  }
  _importModalOpen = true;

  const root = document.createElement("div");
  root.className = "modal-backdrop";
  root.innerHTML = `
    <div class="modal" role="dialog" aria-modal="true"
         aria-labelledby="import-title" style="max-width:680px">
      <div class="modal-title" id="import-title">Import gaps</div>
      <div class="modal-body" style="max-height:70vh;overflow:auto">
        <p class="muted small">Paste free-form text (meeting transcript, bug report,
        feedback dump). refine extracts a draft list — review and edit before saving.</p>
        <div class="muted small" style="margin-bottom:8px">
          Submitting as <strong class="js-reporter-name">${htmlEscape(reporter)}</strong>
          — applies to all extracted gaps. Change in the top-right reporter selector.
        </div>
        <div class="form-row">
          <label>Source text</label>
          <textarea id="import-text" rows="8" placeholder="Paste here…"></textarea>
        </div>
        <div id="import-drafts" class="import-drafts" style="margin-top:14px"></div>
      </div>
      <div class="modal-actions">
        <button class="secondary" data-cancel>Cancel</button>
        <button id="btn-extract" data-ok>Extract drafts</button>
      </div>
    </div>
  `;
  document.body.appendChild(root);

  let closed = false;
  function close(navigateAway) {
    if (closed) return;
    closed = true;
    _importModalOpen = false;
    document.removeEventListener("keydown", onKey, true);
    root.remove();
    if (navigateAway && location.hash.startsWith("#/gaps/import")) {
      location.hash = "#/gaps";
    }
  }
  function onKey(e) {
    if (e.key === "Escape") {
      e.preventDefault();
      close(true);
    }
    // Enter inside textareas always inserts a newline; no global Enter
    // submit, since this modal has two distinct submit steps.
  }
  document.addEventListener("keydown", onKey, true);
  root.addEventListener("click", (e) => {
    if (e.target === root) close(true);
  });
  root.querySelector("[data-cancel]").addEventListener("click", () => close(true));

  root.querySelector("#btn-extract").addEventListener("click", async () => {
    const btn = root.querySelector("#btn-extract");
    if (btn.disabled) return;
    const text = root.querySelector("#import-text").value.trim();
    if (!text) return toast("Paste some text first", "error");
    // Show an explicit loading indicator in the drafts area — the LLM
    // call typically takes 20-90s and the busy button alone isn't enough
    // signal that something's happening.
    const draftsRoot = root.querySelector("#import-drafts");
    if (draftsRoot) {
      draftsRoot.innerHTML = `
        <div class="loading-row">
          <span class="loading-spinner"></span>
          <span>Loading… asking Claude to extract Gaps from your text. This may take up to a minute.</span>
        </div>`;
    }
    await withButtonBusy(btn, "Extracting…", async () => {
      try {
        const r = await api("POST", "/api/import/extract", { text });
        drawImportDrafts(root, r.drafts || [], close);
      } catch (e) {
        if (draftsRoot) draftsRoot.innerHTML = "";
        toast(e.message, "error");
      }
    });
  });

  root.querySelector("#import-text").focus();
}

function drawImportDrafts(root, drafts, close) {
  const drafts_root = root.querySelector("#import-drafts");
  if (!drafts.length) {
    drafts_root.innerHTML = `<p class="muted">No drafts extracted.</p>`;
    return;
  }
  drafts_root.innerHTML = `
    <h3 style="margin-top:0">Extracted drafts (${drafts.length}) — review &amp; confirm</h3>
    ${drafts.map((d, i) => `
      <div class="draft" data-idx="${i}">
        <input type="text" class="d-name" value="${htmlEscape(d.name)}" placeholder="Name">
        <div class="form-row" style="margin-top:6px">
          <label class="small muted">Actual</label>
          <textarea class="d-actual" rows="2">${htmlEscape(d.actual)}</textarea>
        </div>
        <div class="form-row">
          <label class="small muted">Target</label>
          <textarea class="d-target" rows="3">${htmlEscape(d.target)}</textarea>
        </div>
      </div>`).join("")}
  `;
  // Swap the primary action from "Extract drafts" to "Save N gap(s)".
  const actions = root.querySelector(".modal-actions");
  actions.innerHTML = `
    <button class="secondary" data-cancel>Cancel</button>
    <button id="btn-persist">Save ${drafts.length} gap${drafts.length === 1 ? "" : "s"}</button>
  `;
  actions.querySelector("[data-cancel]").addEventListener("click", () => close(true));
  actions.querySelector("#btn-persist").addEventListener("click", async () => {
    const reporter = state.lastReporter || "";
    if (!reporter) return toast("Pick a reporter in the top-right selector", "error");
    const payload = $$(".draft", drafts_root).map((row) => ({
      name: row.querySelector(".d-name").value.trim(),
      actual: row.querySelector(".d-actual").value.trim(),
      target: row.querySelector(".d-target").value.trim(),
    }));
    try {
      const r = await api("POST", "/api/import/persist", { reporter, drafts: payload });
      toast(`Created ${r.count} gap(s)`, "info");
      // Stay on the underlying screen — same behavior as the New Gap
      // modal. `close(true)` only redirects when the user came in via
      // the `#/gaps/import` deep link.
      close(true);
    } catch (e) { toast(e.message, "error"); }
  });
}

// ---- Changes ----------------------------------------------------------------
//
// Lists refine merge commits on the configured merge target branch (or the
// host's current branch if no target is set). Each row links the commit
// to its Gap and offers an Undo button — Undo runs `git revert -m 1` on
// the merge commit, pushes if there's an upstream, and moves the Gap to
// `cancelled` with a log entry.

async function renderChanges() {
  renderBanners([]);
  $("#main").innerHTML = `<h2>Changes</h2><div id="changes-body"><p class="muted">Loading…</p></div>`;
  await loadChanges();
}

async function loadChanges() {
  try {
    const data = await api("GET", "/api/changes");
    drawChanges(data);
  } catch (e) {
    $("#changes-body").innerHTML = `<p class="muted">${htmlEscape(e.message)}</p>`;
  }
}

function drawChanges(data) {
  const root = $("#changes-body");
  const branch = data.branch || "(unknown)";
  const changes = data.changes || [];
  if (!branch || branch === "(unknown)") {
    root.innerHTML = `
      <p class="muted">
        No merge target branch resolved. Set <code>merge_target_branch</code>
        in <a href="#/settings">Settings → Scope</a>, or check that the host
        repo has a branch checked out.
      </p>`;
    return;
  }
  if (!changes.length) {
    root.innerHTML = `
      <p class="muted">
        No refine merges on <code>${htmlEscape(branch)}</code> yet. When a
        Gap moves <em>review → done</em>, its merge commit shows up here.
      </p>`;
    return;
  }
  root.innerHTML = `
    <p class="muted small" style="margin-bottom:10px">
      Merges on <code>${htmlEscape(branch)}</code> (newest first).
      Each row maps to a Gap via the <code>Refine Gap:</code> trailer in
      the commit message.
    </p>
    <table class="table">
      <thead><tr>
        <th>When</th>
        <th>Gap</th>
        <th>Status</th>
        <th>Merge commit</th>
        <th></th>
      </tr></thead>
      <tbody>
        ${changes.map((c) => `
          <tr data-commit="${htmlEscape(c.commit)}" data-gap-id="${htmlEscape(c.gap_id)}">
            <td class="muted small">${fmtTime(c.committed)}</td>
            <td>${c.name
              ? `<a href="#/gaps/${htmlEscape(c.gap_id)}">${htmlEscape(c.name)}</a>`
              : `<a href="#/gaps/${htmlEscape(c.gap_id)}" class="muted">(deleted)</a>`}</td>
            <td>${c.status ? `<span class="status-pill ${c.status}">${c.status}</span>` : `<span class="muted small">—</span>`}</td>
            <td class="muted small"><code>${c.commit.slice(0, 10)}…</code></td>
            <td><button class="secondary" data-undo-commit="${htmlEscape(c.commit)}"
                       ${c.status === "cancelled" ? "disabled" : ""}>
              Undo
            </button></td>
          </tr>`).join("")}
      </tbody>
    </table>
  `;
  $$("[data-undo-commit]", root).forEach((btn) => {
    btn.addEventListener("click", async (e) => {
      e.stopPropagation();
      const commit = btn.dataset.undoCommit;
      const row = btn.closest("tr");
      const gapName = row?.querySelector("td:nth-child(2)")?.textContent?.trim() || "this Gap";
      const ok = await modalConfirm(
        `Revert the merge commit ${commit.slice(0, 10)}… for ${gapName}? ` +
        "Refine will run `git revert -m 1`, push to the upstream if one " +
        "exists, and move the Gap to `cancelled`. The original commits " +
        "stay in history; the revert is a new commit on top.",
        { title: "Undo Gap", okLabel: "Undo", cancelLabel: "Keep merge",
          danger: true },
      );
      if (!ok) return;
      await withButtonBusy(btn, "Undoing…", async () => {
        try {
          const r = await api("POST", "/api/changes/undo", { commit });
          if (r.ok) {
            toast(`Undone${r.pushed ? " and pushed" : ""}`, "info");
            await loadChanges();
          } else {
            toast(r.message || "Undo failed", "error");
          }
        } catch (e) { toast(e.message, "error"); }
      });
    });
  });
}

// ---- Agents -----------------------------------------------------------------

async function renderAgents() {
  renderBanners([]);
  $("#main").innerHTML = `
    <h2>Agents</h2>
    <div id="agents-content"><p class="muted">Loading…</p></div>
  `;
  try {
    const [dash, settings] = await Promise.all([
      api("GET", "/api/dashboard"),
      api("GET", "/api/settings"),
    ]);
    drawAgents(dash, settings.settings);
  } catch (e) {
    $("#agents-content").innerHTML = `<p class="muted">${htmlEscape(e.message)}</p>`;
  }
}

function drawAgents(dash, settings) {
  const paused = settings.paused === "1";
  $("#agents-content").innerHTML = `
    <div class="card">
      <h3>Agent spawning</h3>
      <div class="actions">
        <button id="btn-pause" class="${paused ? "" : "secondary"}">
          ${paused ? "Resume" : "Pause"} agent spawning
        </button>
        <span class="muted small">
          ${paused
            ? "Paused — new subprocesses won't launch; running ones continue."
            : "Active — new subprocesses launch on demand."}
        </span>
      </div>
      <p class="muted small" style="margin-top:8px">
        Runtime limits (parallel-run cap, idle timeout, hard cap) and IPC
        diagnostics live on the <a href="#/settings">Settings</a> page.
      </p>
    </div>

    <div class="card" style="margin-top:16px">
      <h3>Currently running</h3>
      ${(dash.running || []).length === 0
        ? `<p class="muted">Nothing running.</p>`
        : (() => {
            const anchorMs = Date.now();
            return `<table class="table">
              <thead><tr><th>Gap</th><th>Elapsed</th><th>Idle</th><th></th></tr></thead>
              <tbody>
                ${dash.running.map((r) => `<tr>
                  <td><a href="#/gaps/${r.gap_id}">${r.gap_id.slice(0,10)}…</a></td>
                  <td class="js-elapsed-tick"
                      data-base="${r.elapsed_seconds}"
                      data-anchor-ms="${anchorMs}">${fmtElapsed(r.elapsed_seconds)}</td>
                  <td class="js-idle-tick"
                      data-base="${r.idle_seconds}"
                      data-anchor-ms="${anchorMs}">${fmtElapsed(r.idle_seconds)}</td>
                  <td><button class="danger" data-cancel="${r.gap_id}">Cancel</button></td>
                </tr>`).join("")}
              </tbody>
            </table>`;
          })()}
    </div>
  `;
  $("#btn-pause").addEventListener("click", async () => {
    try {
      await api("PATCH", "/api/settings", { paused: paused ? "0" : "1" });
      await renderAgents();
    } catch (e) { toast(e.message, "error"); }
  });
  $$("[data-cancel]").forEach((b) => {
    b.addEventListener("click", async () => {
      const id = b.dataset.cancel;
      const ok = await modalConfirm(
        "Cancel this Gap's running subprocess?",
        { title: "Cancel run", okLabel: "Cancel run", danger: true,
          cancelLabel: "Keep running" },
      );
      if (!ok) return;
      try { await api("POST", `/api/gaps/${id}/cancel`); await renderAgents(); }
      catch (e) { toast(e.message, "error"); }
    });
  });
}

// ---- Chat -------------------------------------------------------------------

// chatState holds one tab per chat: the permanent "standalone" tab plus one
// per Gap that the user opened via Open Chat. Each tab carries its own
// session id, accumulated output, and closed-reason. Only the active tab is
// polled; output for other tabs accumulates server-side in the runner's
// per-session deque until the user switches to that tab.
const CHAT_TABS_STORAGE_KEY = "refine_chat_tabs";
const chatState = {
  tabs: {},                // tabId → { gapId, label, sessionId, output, closedReason }
  activeTabId: "standalone",
  pollTimer: null,
  open: false,             // dock expanded?
  bodyHeight: null,        // user-resized body height in px; null → 20vh default
  fullscreen: false,       // when true, panel fills viewport below the topbar
};

function ensureStandaloneTab() {
  if (!chatState.tabs.standalone) {
    chatState.tabs.standalone = {
      gapId: null, label: "Standalone",
      sessionId: null, output: "", closedReason: null,
    };
  }
}

function loadChatStateFromStorage() {
  try {
    const raw = localStorage.getItem(CHAT_TABS_STORAGE_KEY);
    if (!raw) return;
    const parsed = JSON.parse(raw);
    if (parsed && typeof parsed === "object" && parsed.tabs) {
      chatState.tabs = parsed.tabs;
      if (parsed.activeTabId && chatState.tabs[parsed.activeTabId]) {
        chatState.activeTabId = parsed.activeTabId;
      }
      if (typeof parsed.open === "boolean") chatState.open = parsed.open;
      if (typeof parsed.bodyHeight === "number" && parsed.bodyHeight > 0) {
        chatState.bodyHeight = parsed.bodyHeight;
      }
      if (typeof parsed.fullscreen === "boolean") {
        chatState.fullscreen = parsed.fullscreen;
      }
    }
  } catch {}
  ensureStandaloneTab();
}

function saveChatStateToStorage() {
  // We persist sessionIds too: the runner can keep them alive across page
  // navigations. On a stale id the next read returns alive=false and we
  // clear it.
  const tabs = {};
  for (const [id, t] of Object.entries(chatState.tabs)) {
    tabs[id] = {
      gapId: t.gapId, label: t.label,
      sessionId: t.sessionId,
      output: (t.output || "").slice(-50_000),
      closedReason: t.closedReason,
    };
  }
  try {
    localStorage.setItem(CHAT_TABS_STORAGE_KEY, JSON.stringify({
      tabs, activeTabId: chatState.activeTabId,
      open: chatState.open, bodyHeight: chatState.bodyHeight,
      fullscreen: chatState.fullscreen,
    }));
  } catch {}
}

function defaultChatBodyHeight() {
  return Math.max(120, Math.round(window.innerHeight * 0.20));
}

function clampChatBodyHeight(px) {
  const min = 120;
  const max = Math.max(min, Math.round(window.innerHeight * 0.85));
  return Math.max(min, Math.min(max, Math.round(px)));
}

function initChatDock() {
  loadChatStateFromStorage();
  ensureStandaloneTab();
  drawChatDock();
  observeChatDockSize();
  observeTopbarHeight();
}

// Publish the topbar's actual height as --topbar-height on <html> so the
// fullscreen chat dock can anchor its top edge just below the main nav.
function observeTopbarHeight() {
  const topbar = document.querySelector(".topbar");
  if (!topbar) return;
  const apply = () => {
    document.documentElement.style.setProperty(
      "--topbar-height", `${topbar.offsetHeight}px`,
    );
  };
  apply();
  if (typeof ResizeObserver === "function") {
    new ResizeObserver(apply).observe(topbar);
  } else {
    window.addEventListener("resize", apply);
  }
}

// Keep --chat-dock-height in sync with whatever vertical space the dock
// actually occupies (collapsed bar, expanded panel, or mid-drag). `body`
// reads this variable as its bottom padding so page content never slides
// underneath the dock.
function observeChatDockSize() {
  const root = $("#chat-dock");
  if (!root) return;
  const apply = () => {
    document.documentElement.style.setProperty(
      "--chat-dock-height", `${root.offsetHeight}px`,
    );
  };
  apply();
  if (typeof ResizeObserver === "function") {
    new ResizeObserver(apply).observe(root);
  } else {
    window.addEventListener("resize", apply);
  }
}

// Opens the dock and (optionally) ensures a tab for a specific gap is active.
// Wired up by the "Open Chat" button on the gap detail page and by any
// surviving `#/chat?gap=...` deep links. For gap tabs with no live session,
// kicks off a chat session immediately so the runner can inject the Gap
// context into claude's session memory before the user types.
function openChatDock({ gapId = null } = {}) {
  ensureStandaloneTab();
  if (gapId) {
    if (!chatState.tabs[gapId]) {
      chatState.tabs[gapId] = {
        gapId,
        label: `Gap ${gapId.slice(0, 8)}…`,
        sessionId: null, output: "", closedReason: null,
      };
    }
    chatState.activeTabId = gapId;
  }
  chatState.open = true;
  saveChatStateToStorage();
  drawChatDock();
  if (gapId) {
    const t = chatState.tabs[gapId];
    if (t && !t.sessionId) startGapChatSession(t);
  }
}

async function startGapChatSession(tab) {
  try {
    const r = await api("POST", "/api/chat/start", { gap_id: tab.gapId });
    tab.sessionId = r.session_id;
    tab.closedReason = null;
    saveChatStateToStorage();
    drawChatDock();
    $("#chat-input")?.focus();
  } catch (e) {
    toast("Could not start chat: " + e.message, "error");
  }
}

function toggleChatDock() {
  chatState.open = !chatState.open;
  // Collapsing the dock also exits fullscreen — leaving fullscreen on
  // while the body is hidden would orphan the topbar offset.
  if (!chatState.open) chatState.fullscreen = false;
  saveChatStateToStorage();
  drawChatDock();
}

function toggleChatFullscreen() {
  chatState.fullscreen = !chatState.fullscreen;
  if (chatState.fullscreen) chatState.open = true;  // fullscreen implies open
  saveChatStateToStorage();
  drawChatDock();
}

function drawChatDock() {
  const root = $("#chat-dock");
  if (!root) return;
  const tabs = chatState.tabs;
  const activeId = chatState.activeTabId;
  const active = tabs[activeId] || tabs.standalone;
  const hasSession = !!active.sessionId;

  const startLabel = active.gapId
    ? `Start attached to Gap ${active.gapId.slice(0, 10)}…`
    : "Start standalone";
  const toggleLabel = hasSession
    ? (active.gapId ? "Stop session" : "Stop standalone")
    : startLabel;
  const toggleClass = hasSession ? "danger" : "";

  const statusLine = !active.sessionId
    ? "No active session."
    : (active.closedReason
        ? `Session ${active.sessionId} ended — ${active.closedReason}.`
        : `Session ${active.sessionId} active.`);

  root.classList.toggle("open", !!chatState.open);
  root.classList.toggle("fullscreen", !!chatState.fullscreen);
  if (chatState.open && !chatState.bodyHeight) {
    chatState.bodyHeight = defaultChatBodyHeight();
  }
  root.innerHTML = `
    <div class="chat-dock-resize" id="chat-dock-resize"
         role="separator" aria-orientation="horizontal"
         aria-label="Resize chat panel"
         title="Drag to resize"></div>
    <div class="chat-dock-bar" id="chat-dock-bar"
         title="${chatState.open ? "Click to collapse" : "Click a tab to expand chat"}">
      <span class="chat-dock-label">Chat</span>
      <div class="chat-tabs">
        ${Object.entries(tabs).map(([id, t]) => `
          <button class="chat-tab ${id === activeId ? "active" : ""}"
                  data-tab-id="${htmlEscape(id)}"
                  title="${htmlEscape(t.gapId || "Standalone chat")}">
            ${htmlEscape(t.label)}${t.sessionId ? ` <span class="chat-tab-dot" title="active session"></span>` : ""}
            ${id === "standalone" ? "" : `<span class="chat-tab-close" data-close-tab="${htmlEscape(id)}" title="Close tab">×</span>`}
          </button>`).join("")}
      </div>
      <button class="chat-dock-toggle chat-dock-fullscreen-btn${chatState.fullscreen ? " active" : ""}"
              id="btn-dock-fullscreen"
              aria-label="${chatState.fullscreen ? "Exit fullscreen chat" : "Fullscreen chat"}"
              aria-pressed="${chatState.fullscreen ? "true" : "false"}"
              title="${chatState.fullscreen ? "Exit fullscreen" : "Fullscreen"}">⛶</button>
      <button class="chat-dock-toggle chat-dock-collapse" id="btn-dock-toggle"
              aria-label="${chatState.open ? "Collapse chat" : "Expand chat"}"
              title="${chatState.open ? "Collapse chat" : "Expand chat"}">▾</button>
    </div>
    <div class="chat-dock-body"
         style="${chatState.bodyHeight ? `height:${chatState.bodyHeight}px` : ""}">
      <div class="actions" style="margin-bottom:10px">
        <button id="btn-chat-toggle" class="${toggleClass}">${htmlEscape(toggleLabel)}</button>
        <button id="btn-chat-clear" class="secondary"
                ${(active.output || active.sessionId) ? "" : "disabled"}>
          Clear history
        </button>
        <span class="spacer"></span>
        <span id="chat-status" class="muted small">${htmlEscape(statusLine)}</span>
      </div>
      <div class="chat-output-box">
        <div id="chat-output" class="chat-output">${mdToHtml(active.output || "")}</div>
        <div id="chat-pending" class="chat-pending" hidden>
          <span class="chat-pending-dots"><span></span><span></span><span></span></span>
          Claude is thinking…
        </div>
      </div>
      <div class="actions" style="margin-top:8px">
        <input type="text" id="chat-input"
               placeholder="${hasSession
                 ? "Type and press Enter…"
                 : "Click Start to begin session before sending messages is enabled."}"
               ${hasSession && !active.pending ? "" : "disabled"}>
      </div>
    </div>
  `;
  applyPendingIndicator(active);

  if (chatState.open) {
    const out = $("#chat-output");
    if (out) out.scrollTop = out.scrollHeight;
  }

  $$(".chat-tab", root).forEach((el) => {
    el.addEventListener("click", (e) => {
      if (e.target.matches("[data-close-tab]")) return;
      const id = el.dataset.tabId;
      if (!id) return;
      if (id === chatState.activeTabId) {
        // Clicking the active tab toggles the dock open/closed.
        toggleChatDock();
      } else {
        switchChatTab(id);
        if (!chatState.open) {
          chatState.open = true;
          saveChatStateToStorage();
          drawChatDock();
        }
      }
    });
  });
  $$("[data-close-tab]", root).forEach((el) => {
    el.addEventListener("click", (e) => {
      e.stopPropagation();
      closeChatTab(el.dataset.closeTab);
    });
  });
  $("#btn-dock-toggle")?.addEventListener("click", toggleChatDock);
  $("#btn-dock-fullscreen")?.addEventListener("click", toggleChatFullscreen);
  $("#btn-chat-toggle")?.addEventListener("click", toggleActiveChat);
  $("#btn-chat-clear")?.addEventListener("click", clearActiveChat);
  $("#chat-input")?.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      sendChatLine();
    }
  });

  wireChatDockResize(root);
  restartPollForActiveTab();
}

function wireChatDockResize(root) {
  const handle = root.querySelector("#chat-dock-resize");
  const body = root.querySelector(".chat-dock-body");
  if (!handle || !body) return;
  handle.addEventListener("pointerdown", (e) => {
    if (!chatState.open) return;
    e.preventDefault();
    const startY = e.clientY;
    const startH = body.getBoundingClientRect().height;
    handle.setPointerCapture(e.pointerId);
    root.classList.add("resizing");
    function onMove(ev) {
      // Drag up grows the panel; drag down shrinks it.
      const next = clampChatBodyHeight(startH + (startY - ev.clientY));
      body.style.height = next + "px";
      chatState.bodyHeight = next;
    }
    function onUp(ev) {
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", onUp);
      handle.removeEventListener("pointercancel", onUp);
      try { handle.releasePointerCapture(ev.pointerId); } catch {}
      root.classList.remove("resizing");
      saveChatStateToStorage();
    }
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", onUp);
    handle.addEventListener("pointercancel", onUp);
  });
}

// Back-compat alias used by helpers below; thin wrapper.
function drawChat() { drawChatDock(); }

function applyPendingIndicator(tab) {
  const ind = $("#chat-pending");
  const input = $("#chat-input");
  if (ind) ind.hidden = !tab || !tab.pending;
  if (input) input.disabled = !tab || !tab.sessionId || tab.pending;
}

function restartPollForActiveTab() {
  if (chatState.pollTimer) {
    clearInterval(chatState.pollTimer);
    chatState.pollTimer = null;
  }
  const t = chatState.tabs[chatState.activeTabId];
  if (!t || !t.sessionId) return;
  chatState.pollTimer = setInterval(pollChat, 800);
  // Fire an immediate poll so the user doesn't wait 800ms for the first read.
  pollChat();
}

function switchChatTab(tabId) {
  if (!chatState.tabs[tabId]) return;
  chatState.activeTabId = tabId;
  saveChatStateToStorage();
  drawChat();
}

async function closeChatTab(tabId) {
  if (tabId === "standalone") return;            // never closeable
  const t = chatState.tabs[tabId];
  if (!t) return;
  if (t.sessionId) {
    try { await api("POST", `/api/chat/${t.sessionId}/stop`); } catch {}
  }
  delete chatState.tabs[tabId];
  if (chatState.activeTabId === tabId) chatState.activeTabId = "standalone";
  saveChatStateToStorage();
  drawChat();
}

async function clearActiveChat() {
  const t = chatState.tabs[chatState.activeTabId];
  if (!t) return;
  if (!t.output && !t.sessionId) return;     // nothing to clear
  const btn = $("#btn-chat-clear");
  const ok = await modalConfirm(
    "Clear this chat's history? Any active session will be stopped and " +
    "the transcript wiped. Starting again gives Claude a fresh conversation.",
    { title: "Clear chat history", okLabel: "Clear", danger: true,
      cancelLabel: "Keep history" },
  );
  if (!ok) return;
  await withButtonBusy(btn, "Clearing…", async () => {
    if (t.sessionId) {
      try { await api("POST", `/api/chat/${t.sessionId}/stop`); } catch {}
    }
    t.sessionId = null;
    t.output = "";
    t.closedReason = null;
    t.pending = false;
    saveChatStateToStorage();
    drawChat();
  });
}

async function toggleActiveChat() {
  const t = chatState.tabs[chatState.activeTabId];
  if (!t) return;
  const btn = $("#btn-chat-toggle");
  if (t.sessionId) {
    await withButtonBusy(btn, "Stopping…", async () => {
      try { await api("POST", `/api/chat/${t.sessionId}/stop`); } catch {}
      t.sessionId = null;
      t.closedReason = "stopped by user";
      saveChatStateToStorage();
      drawChat();
    });
    return;
  }
  await withButtonBusy(btn, "Starting…", async () => {
    try {
      const r = await api("POST", "/api/chat/start",
                          t.gapId ? { gap_id: t.gapId } : {});
      t.sessionId = r.session_id;
      t.closedReason = null;
      t.output = "";
      saveChatStateToStorage();
      drawChat();
      $("#chat-input")?.focus();
    } catch (e) {
      toast("Could not start chat: " + e.message, "error");
    }
  });
}

async function pollChat() {
  const t = chatState.tabs[chatState.activeTabId];
  if (!t || !t.sessionId) return;
  const sid = t.sessionId;
  try {
    const r = await api("GET", `/api/chat/${sid}/read`);
    if (r.lines && r.lines.length) {
      t.output = (t.output || "") + r.lines.join("\n") + "\n";
      // Only update the DOM if this tab is still active.
      if (chatState.activeTabId in chatState.tabs &&
          chatState.tabs[chatState.activeTabId].sessionId === sid) {
        const out = $("#chat-output");
        if (out) {
          const atBottom = out.scrollHeight - out.scrollTop - out.clientHeight < 50;
          // Re-render the full transcript as markdown — incremental
          // append won't work since block elements (code fences, lists)
          // can span multiple chunks.
          out.innerHTML = mdToHtml(t.output || "");
          if (atBottom) out.scrollTop = out.scrollHeight;
        }
      }
      saveChatStateToStorage();
    }
    // Pending state is authoritative from the runner: `in_flight` is true
    // while a `claude --print` subprocess is running for this session.
    const wasPending = !!t.pending;
    t.pending = !!r.in_flight;
    if (wasPending !== t.pending) applyPendingIndicator(t);
    if (r.alive === false) {
      t.closedReason = r.closed_reason || "session ended";
      t.sessionId = null;
      t.pending = false;
      saveChatStateToStorage();
      drawChat();
    }
  } catch {
    // Tolerate transient errors; SSE/poller reconnects on its own.
  }
}

async function sendChatLine() {
  const t = chatState.tabs[chatState.activeTabId];
  if (!t || !t.sessionId || t.pending) return;
  const input = $("#chat-input");
  const text = input.value;
  if (!text.trim()) return;
  input.value = "";
  const echo = `\n> ${text}\n`;
  t.output = (t.output || "") + echo;
  const out = $("#chat-output");
  if (out) {
    out.innerHTML = mdToHtml(t.output || "");
    out.scrollTop = out.scrollHeight;
  }
  // Optimistically flip into pending so the indicator appears immediately
  // (the next poll will confirm via `in_flight`).
  t.pending = true;
  applyPendingIndicator(t);
  saveChatStateToStorage();
  try {
    await api("POST", `/api/chat/${t.sessionId}/input`, { text });
    // Trigger a poll right away so we pick up `in_flight` and (likely soon)
    // the response without waiting the full 800ms tick.
    pollChat();
  } catch (e) {
    t.pending = false;
    applyPendingIndicator(t);
    toast("Could not send: " + e.message, "error");
  }
}

// ---- Logs -------------------------------------------------------------------

const LOGS_LIMIT_OPTIONS = [50, 100, 250, 500, 1000];

function logsFiltersFromHash() {
  const hashQs = new URLSearchParams(location.hash.split("?")[1] || "");
  return {
    severity: hashQs.get("severity") || "",
    category: hashQs.get("category") || "",
    actor: hashQs.get("actor") || "",
    gap_id: hashQs.get("gap_id") || "",
    q: hashQs.get("q") || "",
    limit: parseInt(hashQs.get("limit") || "100", 10) || 100,
  };
}

function logsHashFromFilters(f) {
  const next = new URLSearchParams();
  for (const [k, v] of Object.entries(f)) {
    if (v && !(k === "limit" && v === 100)) next.set(k, String(v));
  }
  return "#/logs" + (next.toString() ? "?" + next : "");
}

async function renderLogs() {
  renderBanners([]);
  const f = logsFiltersFromHash();
  // Preserve the filter shell's open/closed state across full re-renders.
  const logsFilterShellOpen = !!document.getElementById("logs-filter-shell")?.open;
  $("#main").innerHTML = `
    <h2>Logs</h2>
    <details class="filter-shell" id="logs-filter-shell"${logsFilterShellOpen ? " open" : ""}>
      <summary>
        <span class="filter-shell-title">Filters</span>
        <span class="spacer"></span>
        <span class="muted small"><span id="logs-count"></span></span>
        <span id="logs-filtered" class="filter-pill" hidden>Filtered</span>
      </summary>
      <div class="filter-shell-body">
    <div class="filter-bar">
      <div class="filter-row filter-row-primary">
        <input type="text" id="logs-q"
               class="filter-grow"
               placeholder="Search message or details…"
               value="${htmlEscape(f.q)}">
        <input type="text" id="logs-gap-id"
               class="filter-gap-id"
               placeholder="Gap ID"
               value="${htmlEscape(f.gap_id)}">
      </div>
      <div class="filter-row filter-row-filters">
        <select id="logs-severity">
          <option value="" ${f.severity === "" ? "selected" : ""}>all severities</option>
          <option value="info"  ${f.severity === "info"  ? "selected" : ""}>info</option>
          <option value="warn"  ${f.severity === "warn"  ? "selected" : ""}>warn</option>
          <option value="error" ${f.severity === "error" ? "selected" : ""}>error</option>
        </select>
        <select id="logs-category"><option value="">all categories</option></select>
        <select id="logs-actor"><option value="">all actors</option></select>
        <select id="logs-limit">
          ${LOGS_LIMIT_OPTIONS.map((n) =>
            `<option value="${n}" ${n === f.limit ? "selected" : ""}>${n} entries</option>`).join("")}
        </select>
        <span class="spacer"></span>
        <button class="secondary" id="logs-clear">Clear filters</button>
      </div>
    </div>
      </div>
    </details>
    <div id="logs-list"><p class="muted">Loading…</p></div>
  `;

  $("#logs-q").addEventListener("input", debounce(() => navigateLogs(), 300));
  $("#logs-severity").addEventListener("change", () => navigateLogs());
  $("#logs-category").addEventListener("change", () => navigateLogs());
  $("#logs-actor").addEventListener("change", () => navigateLogs());
  $("#logs-gap-id").addEventListener("input", debounce(() => navigateLogs(), 300));
  $("#logs-limit").addEventListener("change", () => navigateLogs());
  $("#logs-clear").addEventListener("click", () => { location.hash = "#/logs"; });

  await loadLogs();
}

function navigateLogs() {
  const next = {
    severity: $("#logs-severity").value,
    category: $("#logs-category").value,
    actor: $("#logs-actor").value,
    gap_id: $("#logs-gap-id").value.trim(),
    q: $("#logs-q").value,
    limit: parseInt($("#logs-limit").value, 10) || 100,
  };
  location.hash = logsHashFromFilters(next);
}

async function loadLogs() {
  if (state.currentRoute !== "logs") return;
  const f = logsFiltersFromHash();
  const params = new URLSearchParams();
  if (f.severity) params.set("severity", f.severity);
  if (f.category) params.set("category", f.category);
  if (f.actor) params.set("actor", f.actor);
  if (f.gap_id) params.set("gap_id", f.gap_id);
  if (f.q) params.set("q", f.q);
  params.set("limit", String(f.limit));
  params.set("facets", "1");
  try {
    const data = await api("GET", "/api/activity?" + params);
    drawLogsList(data, f);
  } catch (e) {
    $("#logs-list").innerHTML = `<p class="muted">${htmlEscape(e.message)}</p>`;
  }
}

function drawLogsList(data, f) {
  const entries = data.activity || [];
  const facets = data.facets || {};
  // Re-populate facet dropdowns from server-side distinct values.
  const catSel = $("#logs-category");
  if (catSel) {
    const existing = facets.categories || [];
    catSel.innerHTML = `<option value="">all categories</option>` +
      existing.map((c) => `<option value="${htmlEscape(c)}" ${c === f.category ? "selected" : ""}>${htmlEscape(c)}</option>`).join("");
  }
  const actSel = $("#logs-actor");
  if (actSel) {
    const existing = facets.actors || [];
    actSel.innerHTML = `<option value="">all actors</option>` +
      existing.map((a) => `<option value="${htmlEscape(a)}" ${a === f.actor ? "selected" : ""}>${htmlEscape(a)}</option>`).join("");
  }
  const countEl = $("#logs-count");
  if (countEl) {
    countEl.textContent = `${entries.length} ${entries.length === 1 ? "entry" : "entries"}`;
  }
  applyLogsFilterIndicator(f);
  const root = $("#logs-list");
  if (!entries.length) {
    root.innerHTML = `<p class="muted">No log entries match the current filters.</p>`;
    return;
  }
  root.innerHTML = renderActivityList(entries);
}

// Mirror of applyGapsFilterIndicator for the Logs screen.
function applyLogsFilterIndicator(f) {
  const active = {
    "logs-q": !!f.q,
    "logs-gap-id": !!f.gap_id,
    "logs-severity": !!f.severity,
    "logs-category": !!f.category,
    "logs-actor": !!f.actor,
    "logs-limit": f.limit !== 100,
  };
  let anyActive = false;
  for (const [id, on] of Object.entries(active)) {
    const el = document.getElementById(id);
    if (!el) continue;
    el.classList.toggle("filter-active", on);
    if (on) anyActive = true;
  }
  const pill = $("#logs-filtered");
  if (pill) pill.hidden = !anyActive;
  const list = $("#logs-list");
  if (list) list.classList.toggle("results-filtered", anyActive);
}

// ---- Settings ---------------------------------------------------------------

async function renderSettings() {
  renderBanners([]);
  $("#main").innerHTML = `<h2>Settings</h2><div id="settings-content"><p class="muted">Loading…</p></div>`;
  try {
    const [s, diag, reps] = await Promise.all([
      api("GET", "/api/settings"),
      api("GET", "/api/diagnostics"),
      api("GET", "/api/reporters"),
    ]);
    drawSettings(s.settings || {}, diag, reps.reporters || []);
  } catch (e) {
    $("#settings-content").innerHTML = `<p class="muted">${htmlEscape(e.message)}</p>`;
  }
}

function drawSettings(s, diag, reps) {
  $("#settings-content").innerHTML = `
    <div class="card">
      <h3>Runtime configuration</h3>
      <div class="form-row"><label>Parallel-run cap</label>
        <input type="number" id="s-cap" value="${s.parallel_run_cap || 3}"></div>
      <div class="form-row"><label>Branch name pattern</label>
        <input type="text" id="s-pattern" value="${htmlEscape(s.branch_name_pattern || "refine/{gap_id}")}"></div>
      <div class="form-row"><label>Agent idle timeout (seconds)</label>
        <input type="number" id="s-idle" value="${s.agent_idle_timeout_seconds || 900}"></div>
      <div class="form-row"><label>Agent hard cap (seconds)</label>
        <input type="number" id="s-hard" value="${s.agent_hard_cap_seconds || 86400}"></div>
      <div class="form-row"><label>Standalone chat idle timeout (seconds)
        <span class="muted small">— set to 0 to disable auto-close</span></label>
        <input type="number" id="s-chat-idle" value="${s.chat_idle_timeout_seconds || 300}"></div>
      <div class="actions"><button id="s-save">Save</button></div>
    </div>

    <div class="card" style="margin-top:16px">
      <h3>Scope</h3>
      <p class="muted small">
        Where refine's Claude work lands inside the client repo. The base
        repo location still owns all git plumbing — worktree create, fetch,
        merge, push.
      </p>
      <div class="form-row"><label>Agent subpath
        <span class="muted small">— optional sub-project (relative to the repo root) used as the cwd for agent + chat Claude subprocesses. Leave blank to use the repo root.</span></label>
        <input type="text" id="s-subpath"
               placeholder="e.g. apps/web"
               value="${htmlEscape(s.agent_subpath || "")}"></div>
      <div class="form-row"><label>Merge target branch
        <span class="muted small">— branch all Gap worktrees are based on and all <code>verify</code> merges land on. Leave blank to follow the host's currently-checked-out branch. When set, <code>verify</code> auto-stashes WIP, switches HEAD, and restores the host's original branch afterward.</span></label>
        <input type="text" id="s-merge-target"
               placeholder="e.g. main"
               value="${htmlEscape(s.merge_target_branch || "")}"></div>
      <div class="actions"><button id="s-save-scope">Save</button></div>
    </div>

    <div class="card" style="margin-top:16px">
      <h3>Auth</h3>
      <p class="muted">Claude Code auth lives on the host. Use Re-check to re-run the pre-flight after running <code>claude login</code>.</p>
      <button id="s-recheck">Re-check auth</button>
    </div>

    <div class="card" style="margin-top:16px">
      <h3>Reporters</h3>
      <table class="table">
        <thead><tr><th>Name</th><th></th></tr></thead>
        <tbody>
          ${reps.map((r) => `<tr>
            <td>${htmlEscape(r.name)}</td>
            <td class="actions">
              <button class="secondary" data-rename="${r.id}" data-name="${htmlEscape(r.name)}">Rename</button>
              <button class="danger" data-rdel="${r.id}">Remove</button>
            </td>
          </tr>`).join("")}
        </tbody>
      </table>
      <div class="actions" style="margin-top:8px">
        <button id="r-add">+ Add reporter</button>
      </div>
      <p class="muted small" style="margin-top:6px">
        Renaming a reporter cascades through every Gap's rounds so historical
        data stays in sync. Removing a reporter only affects the dropdown —
        historical rounds keep their original reporter string so audit
        history is preserved.
      </p>
    </div>

    <div class="card" style="margin-top:16px">
      <h3>Logs retention</h3>
      <p class="muted small">
        Delete activity entries older than the chosen window. Newer entries
        and gap state are untouched.
      </p>
      <div class="actions">
        <label for="logs-cleanup-days" class="muted small">Keep</label>
        <select id="logs-cleanup-days">
          ${[0, 7, 30, 60, 90, 365].map((n) =>
            `<option value="${n}" ${n === 7 ? "selected" : ""}>${n === 0 ? "0 (don't keep any)" : `${n} days`}</option>`).join("")}
        </select>
        <button class="danger" id="logs-cleanup">Clean up old logs</button>
      </div>
    </div>

    <div class="card" style="margin-top:16px">
      <h3>IPC diagnostics</h3>
      <dl class="kv">
        <dt>Reachable</dt><dd>${diag.reachable ? "yes" : "no"}</dd>
        ${diag.socket_path ? `<dt>Socket</dt><dd><code>${htmlEscape(diag.socket_path)}</code></dd>` : ""}
        ${diag.last_contact_at ? `<dt>Last contact</dt><dd>${fmtTime(diag.last_contact_at)}</dd>` : ""}
      </dl>
    </div>
  `;
  $("#s-save").addEventListener("click", async () => {
    await withButtonBusy($("#s-save"), "Saving…", async () => {
      try {
        await api("PATCH", "/api/settings", {
          parallel_run_cap: $("#s-cap").value,
          branch_name_pattern: $("#s-pattern").value,
          agent_idle_timeout_seconds: $("#s-idle").value,
          agent_hard_cap_seconds: $("#s-hard").value,
          chat_idle_timeout_seconds: $("#s-chat-idle").value,
        });
        toast("Saved", "info");
      } catch (e) { toast(e.message, "error"); }
    });
  });
  $("#s-save-scope").addEventListener("click", async () => {
    await withButtonBusy($("#s-save-scope"), "Saving…", async () => {
      try {
        await api("PATCH", "/api/settings", {
          agent_subpath: $("#s-subpath").value,
          merge_target_branch: $("#s-merge-target").value,
        });
        toast("Saved", "info");
      } catch (e) { toast(e.message, "error"); }
    });
  });
  $("#s-recheck").addEventListener("click", async () => {
    await withButtonBusy($("#s-recheck"), "Re-checking…", async () => {
      try {
        const r = await api("POST", "/api/settings/recheck-auth");
        toast(r.ok ? "Auth OK" : `Auth failed: ${r.message || "(no message)"}`, r.ok ? "info" : "error");
      } catch (e) { toast(e.message, "error"); }
    });
  });
  $("#logs-cleanup").addEventListener("click", async () => {
    const days = parseInt($("#logs-cleanup-days").value, 10);
    const human = days === 0
      ? "Delete ALL activity log entries? This cannot be undone."
      : `Delete activity log entries older than ${days} day${days === 1 ? "" : "s"}? This cannot be undone.`;
    const ok = await modalConfirm(human, {
      title: "Clean up old logs",
      okLabel: days === 0 ? "Delete all" : "Delete",
      danger: true,
    });
    if (!ok) return;
    await withButtonBusy($("#logs-cleanup"), "Cleaning…", async () => {
      try {
        const r = await api("POST", "/api/activity/cleanup", { days });
        toast(`Deleted ${r.deleted} log entr${r.deleted === 1 ? "y" : "ies"}.`, "info");
      } catch (e) { toast(e.message, "error"); }
    });
  });
  $$("[data-rdel]").forEach((b) => b.addEventListener("click", async () => {
    const ok = await modalConfirm(
      "Remove this reporter from the dropdown? Historical rounds keep their original reporter string.",
      { title: "Remove reporter", okLabel: "Remove", danger: true },
    );
    if (!ok) return;
    try { await api("DELETE", "/api/reporters/" + b.dataset.rdel); await renderSettings(); }
    catch (e) { toast(e.message, "error"); }
  }));
  $$("[data-rename]").forEach((b) => b.addEventListener("click", async () => {
    const oldName = b.dataset.name;
    const name = await modalPrompt("New name", oldName,
                                   { title: "Rename reporter" });
    if (!name || !name.trim()) return;
    const newName = name.trim();
    try {
      await api("PATCH", "/api/reporters/" + b.dataset.rename, { name: newName });
      if (state.lastReporter === oldName) setLastReporter(newName);
      await refreshReporters();
      await renderSettings();
    } catch (e) { toast(e.message, "error"); }
  }));
  $("#r-add").addEventListener("click", async () => {
    const name = await modalPrompt("Reporter name", "",
                                   { title: "Add reporter" });
    if (!name || !name.trim()) return;
    try { await api("POST", "/api/reporters", { name: name.trim() }); await refreshReporters(); await renderSettings(); }
    catch (e) { toast(e.message, "error"); }
  });
}

// ---- Init -------------------------------------------------------------------

// Increment the live "Elapsed" / "Idle" cells once per second so the
// dashboard and Agents page feel responsive even between SSE refreshes.
// Cells without `.js-elapsed-tick` / `.js-idle-tick` no-op cheaply.
function tickRunningCells() {
  const now = Date.now();
  document.querySelectorAll(".js-elapsed-tick, .js-idle-tick").forEach((el) => {
    const base = parseInt(el.dataset.base, 10);
    const anchor = parseInt(el.dataset.anchorMs, 10);
    if (Number.isNaN(base) || Number.isNaN(anchor)) return;
    const seconds = base + Math.floor((now - anchor) / 1000);
    const next = fmtElapsed(seconds);
    if (el.textContent !== next) el.textContent = next;
  });
}

async function init() {
  try {
    await refreshReporters();
  } catch (e) {
    // not fatal — likely fresh install with no reporters yet
  }
  initChatDock();
  initSSE();
  setInterval(tickRunningCells, 1000);
  navigate();
}

init();
