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
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  return `${(seconds / 3600).toFixed(1)}h`;
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
  const orderedStatuses = ["todo", "in-progress", "review", "done", "failed", "cancelled"];
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
        <a class="card" href="#/gaps?status=${s}" style="text-decoration:none;color:inherit">
          <div class="muted small">${s}</div>
          <div style="font-size:28px;font-weight:600;margin-top:4px">${counts[s] || 0}</div>
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
              ${d.reporter_stats.map((s) => `<tr>
                <td>${htmlEscape(s.reporter)}</td>
                <td>${s.active}</td>
                <td>${s.done}</td>
                <td>${s.reported}</td>
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
            : `<table class="table"><thead><tr><th>Gap</th><th>Elapsed</th><th>Idle</th><th>PID</th></tr></thead><tbody>
              ${d.running.map((r) => `<tr onclick="location.hash='#/gaps/${r.gap_id}'">
                <td><code>${r.gap_id.slice(0,8)}…</code></td>
                <td>${fmtElapsed(r.elapsed_seconds)}</td>
                <td>${fmtElapsed(r.idle_seconds)}</td>
                <td class="muted small">${r.pid}</td>
              </tr>`).join("")}
              </tbody></table>`}
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
  updated: "desc", id: "desc",
};

function gapsHash(parts) {
  const next = new URLSearchParams();
  if (parts.q)      next.set("q", parts.q);
  if (parts.status) next.set("status", parts.status);
  if (parts.sort)   next.set("sort", parts.sort);
  if (parts.dir)    next.set("dir", parts.dir);
  return "#/gaps" + (next.toString() ? "?" + next : "");
}

async function renderGapsList() {
  renderBanners([]);
  const hashQs = new URLSearchParams(location.hash.split("?")[1] || "");
  const status = hashQs.get("status") || "";
  const q = hashQs.get("q") || "";
  const sort = (hashQs.get("sort") || "").toLowerCase();
  const dir = (hashQs.get("dir") || "").toLowerCase();
  // Effective sort/dir for indicator rendering: fall back to server defaults
  // when nothing is in the hash.
  const effectiveSort = sort || "updated";
  const effectiveDir = dir || (GAPS_DEFAULT_DIR[effectiveSort] || "desc");

  $("#main").innerHTML = `
    <h2>Gaps</h2>
    <div class="filter-bar">
      <div class="filter-row filter-row-primary">
        <input type="text" id="search" class="filter-grow"
               placeholder="Search gaps…" value="${htmlEscape(q)}">
        <select id="filter-status">
          ${["", "todo", "in-progress", "review", "done", "failed", "cancelled"]
            .map((s) => `<option value="${s}" ${s === status ? "selected" : ""}>${s || "all statuses"}</option>`).join("")}
        </select>
      </div>
      <div class="filter-row filter-row-bulk">
        <span class="muted small">
          <span id="gaps-count"></span>, bulk update matching:
        </span>
        <button class="secondary small" id="bulk-set-priority">Priority…</button>
        <button class="secondary small" id="bulk-set-status">Status…</button>
        <button class="secondary small" id="bulk-set-reporter">Reporter…</button>
      </div>
    </div>
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
  $("#filter-status").addEventListener("change", (e) => {
    updateGapsFilter({ status: e.target.value });
  });
  // The bulk-action buttons read the current filter from the hash at click
  // time, so they always reflect what the user can see.
  $("#bulk-set-priority").addEventListener("click", () => openBulkModal("priority"));
  $("#bulk-set-status").addEventListener("click", () => openBulkModal("status"));
  $("#bulk-set-reporter").addEventListener("click", () => openBulkModal("reporter"));

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
  if (f.sort) params.set("sort", f.sort);
  if (f.dir) params.set("dir", f.dir);
  try {
    const data = await api("GET", "/api/gaps?" + params);
    const gaps = data.gaps || [];
    const countEl = $("#gaps-count");
    if (countEl) {
      countEl.textContent = `${gaps.length} gap${gaps.length === 1 ? "" : "s"}`;
    }
    drawGapsTable(gaps, {
      q: f.q, status: f.status,
      sort: f.effectiveSort, dir: f.effectiveDir,
    });
  } catch (e) {
    const tbl = $("#gaps-table");
    if (tbl) tbl.innerHTML = `<p class="muted">${htmlEscape(e.message)}</p>`;
  }
}

function drawGapsTable(gaps, state) {
  const root = $("#gaps-table");
  if (!gaps.length) {
    root.innerHTML = `<p class="muted">No gaps match the current filters.</p>`;
    return;
  }
  const columns = [
    { key: "name",     label: "Name"     },
    { key: "status",   label: "Status"   },
    { key: "priority", label: "Priority" },
    { key: "updated",  label: "Updated"  },
    { key: "id",       label: "ID"       },
  ];
  const headHtml = columns.map((c) => {
    const isActive = c.key === state.sort;
    const arrow = isActive
      ? (state.dir === "asc" ? "↑" : "↓")
      : `<span class="sort-arrow-placeholder">↕</span>`;
    return `<th class="sortable ${isActive ? "active" : ""}"
                data-sort-key="${c.key}">
              ${c.label} <span class="sort-arrow">${arrow}</span>
            </th>`;
  }).join("");
  root.innerHTML = `
    <table class="table">
      <thead><tr>${headHtml}</tr></thead>
      <tbody>
        ${gaps.map((g) => `
          <tr data-id="${g.id}">
            <td>${htmlEscape(g.name)}</td>
            <td><span class="status-pill ${g.status}">${g.status}</span></td>
            <td><span class="priority-pill priority-${g.priority || "low"}">${g.priority || "low"}</span></td>
            <td class="muted small">${fmtTime(g.updated)}</td>
            <td class="muted small"><code>${g.id.slice(0,10)}…</code></td>
          </tr>`).join("")}
      </tbody>
    </table>
  `;
  $$(".table tbody tr", root).forEach((row) => {
    row.addEventListener("click", () => location.hash = "#/gaps/" + row.dataset.id);
  });
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

// ---- Gaps: bulk-update modal ------------------------------------------------
//
// Each bulk action prompts for a new value and confirms the change against
// the *current* filter (read from the URL hash at click time). The server
// re-runs the filter query, so what the user sees in the table is what gets
// updated — no client-side ID list to drift out of sync. Exactly one field
// is changed per call so the confirmation reads cleanly.

const BULK_PRIORITY_OPTIONS = ["low", "medium", "high"];
const BULK_STATUS_OPTIONS = [
  "todo", "in-progress", "review", "done", "failed", "cancelled",
];

async function openBulkModal(field) {
  // Snapshot the current filter the same way renderGapsList parses it.
  const hashQs = new URLSearchParams(location.hash.split("?")[1] || "");
  const filter = {
    status: hashQs.get("status") || "",
    q: hashQs.get("q") || "",
  };
  const filterDesc = describeGapsFilter(filter);
  const countText = ($("#gaps-count")?.textContent || "").trim();
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
      filter, update: { [field]: next },
    });
    toast(`Updated ${r.updated} gap${r.updated === 1 ? "" : "s"}`, "info");
    await renderGapsList();
  } catch (e) {
    toast(`Bulk update failed: ${e.message}`, "error");
  }
}

function describeGapsFilter(filter) {
  const parts = [];
  if (filter.status) parts.push(`status=${filter.status}`);
  if (filter.q) parts.push(`q="${filter.q}"`);
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

function drawGapDetail(gap) {
  renderBanners([]);
  // Preserve the notes-card open state across re-renders of the same gap so
  // saving a note (or an SSE-driven refresh) doesn't snap it shut.
  const notesOpen = document.querySelector(
    `.notes-card[data-gap-id="${gap.id}"]`,
  )?.open ?? false;
  const rounds = gap.rounds || [];
  // Merge gap-scoped activity into each round so users see lifecycle events
  // and runner errors alongside the round's own logs[]. Each activity entry
  // goes into the latest round whose `created` is at or before the entry's
  // datetime.
  attachActivityToRounds(rounds, gap.activity || []);
  const latest = rounds[rounds.length - 1] || null;
  const failureBanner = computeFailureBanner(gap, latest);

  const isLatestEditable = (gap.status === "todo" || gap.status === "failed");
  const verifyEnabled = gap.status === "review";
  const cancelEnabled = !["done", "cancelled"].includes(gap.status);
  // Chat is "attach to the running agent" — only meaningful while in-progress.
  const chatEnabled = gap.status === "in-progress";
  // Reopen pulls a terminal Gap back to todo so the dispatcher picks it up.
  const reopenEnabled = ["failed", "done", "cancelled"].includes(gap.status);

  $("#main").innerHTML = `
    <div class="gap-detail">
      <div class="row" style="align-items:center;margin-bottom:8px">
        <h2 style="margin:0">${htmlEscape(gap.name)}</h2>
        <span class="status-pill ${gap.status}">${gap.status}</span>
        <span class="priority-pill priority-${gap.priority || "low"}">priority: ${gap.priority || "low"}</span>
      </div>
      <div class="actions" style="margin-bottom:10px">
        <button id="btn-verify" ${verifyEnabled ? "" : "disabled"}>Verify</button>
        <button id="btn-chat" ${chatEnabled ? "" : "disabled"}>Open Chat</button>
        <button id="btn-reopen" ${reopenEnabled ? "" : "disabled"}>Reopen</button>
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
        rounds.map((rnd, idx) => renderRound(rnd, idx, idx === rounds.length - 1, isLatestEditable && idx === rounds.length - 1)).join("")}

      ${(gap.status === "todo" || gap.status === "failed") ? `
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

  $("#btn-verify")?.addEventListener("click", async () => {
    const btn = $("#btn-verify");
    if (btn.disabled) return;
    await withButtonBusy(btn, "Verifying…", async () => {
      try {
        const r = await api("POST", `/api/gaps/${gap.id}/verify`);
        if (r.ok) toast("Merged + pushed", "info");
        else toast(r.message || "Verify did not complete", "error");
        await loadGapDetail(gap.id);
      } catch (e) { toast(e.message, "error"); }
    });
  });
  $("#btn-chat")?.addEventListener("click", () => {
    if ($("#btn-chat").disabled) return;
    openChatDock({ gapId: gap.id });
  });
  $("#btn-reopen")?.addEventListener("click", async () => {
    const btn = $("#btn-reopen");
    if (btn.disabled) return;
    const ok = await modalConfirm(
      `Reopen this Gap? It will move from ${gap.status} back to todo and the dispatcher will pick it up again.`,
      { title: "Reopen Gap", okLabel: "Reopen", cancelLabel: "Keep as-is" },
    );
    if (!ok) return;
    await withButtonBusy(btn, "Reopening…", async () => {
      try {
        await api("POST", `/api/gaps/${gap.id}/retry`);
        toast("Reopened", "info");
        await loadGapDetail(gap.id);
      } catch (e) { toast(e.message, "error"); }
    });
  });
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

function renderRound(rnd, idx, isLatest, editable) {
  const logs = rnd._mergedLogs || rnd.logs || [];
  return `
    <details class="round" ${isLatest ? "open" : ""}>
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
          <details>
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
      const r = await api("POST", "/api/gaps", {
        reporter: currentReporter, actual, target, priority,
      });
      toast("Gap created", "info");
      close(false);
      location.hash = "#/gaps/" + r.gap.id;
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
      close(false);
      location.hash = "#/gaps";
    } catch (e) { toast(e.message, "error"); }
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
        : `<table class="table">
            <thead><tr><th>Gap</th><th>Elapsed</th><th>Idle</th><th></th></tr></thead>
            <tbody>
              ${dash.running.map((r) => `<tr>
                <td><a href="#/gaps/${r.gap_id}">${r.gap_id.slice(0,10)}…</a></td>
                <td>${fmtElapsed(r.elapsed_seconds)}</td>
                <td>${fmtElapsed(r.idle_seconds)}</td>
                <td><button class="danger" data-cancel="${r.gap_id}">Cancel</button></td>
              </tr>`).join("")}
            </tbody>
          </table>`}
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
  $("#main").innerHTML = `
    <h2>Logs</h2>
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
        <span id="logs-count" class="muted small"></span>
        <button class="secondary" id="logs-clear">Clear filters</button>
      </div>
    </div>
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
  const root = $("#logs-list");
  if (!entries.length) {
    root.innerHTML = `<p class="muted">No log entries match the current filters.</p>`;
    return;
  }
  root.innerHTML = renderActivityList(entries);
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
  $("#s-recheck").addEventListener("click", async () => {
    await withButtonBusy($("#s-recheck"), "Re-checking…", async () => {
      try {
        const r = await api("POST", "/api/settings/recheck-auth");
        toast(r.ok ? "Auth OK" : `Auth failed: ${r.message || "(no message)"}`, r.ok ? "info" : "error");
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

async function init() {
  try {
    await refreshReporters();
  } catch (e) {
    // not fatal — likely fresh install with no reporters yet
  }
  initChatDock();
  initSSE();
  navigate();
}

init();
