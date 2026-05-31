// ---- Logs -------------------------------------------------------------------

const LOGS_LIMIT_OPTIONS = [50, 100, 250, 500, 1000];
const LOGS_DEFAULT_LIMIT = 50;
const LOGS_DEFAULT_DIR = {
  datetime: "desc", severity: "asc", category: "asc",
  actor: "asc", gap_id: "asc", message: "asc", id: "desc",
};

function logsFiltersFromHash() {
  const hashQs = new URLSearchParams(location.hash.split("?")[1] || "");
  const sort = (hashQs.get("sort") || "").toLowerCase();
  const dir = (hashQs.get("dir") || "").toLowerCase();
  const effectiveSort = sort || "datetime";
  const effectiveDir = dir || (LOGS_DEFAULT_DIR[effectiveSort] || "desc");
  return {
    severity: hashQs.get("severity") || "",
    category: hashQs.get("category") || "",
    actor: hashQs.get("actor") || "",
    gap_id: hashQs.get("gap_id") || "",
    q: hashQs.get("q") || "",
    limit: parseInt(hashQs.get("limit") || String(LOGS_DEFAULT_LIMIT), 10)
           || LOGS_DEFAULT_LIMIT,
    page: Math.max(1, parseInt(hashQs.get("page") || "1", 10) || 1),
    sort, dir,
    effectiveSort, effectiveDir,
  };
}

function logsHashFromFilters(f) {
  const next = new URLSearchParams();
  if (f.severity) next.set("severity", f.severity);
  if (f.category) next.set("category", f.category);
  if (f.actor) next.set("actor", f.actor);
  if (f.gap_id) next.set("gap_id", f.gap_id);
  if (f.q) next.set("q", f.q);
  if (f.limit && f.limit !== LOGS_DEFAULT_LIMIT) {
    next.set("limit", String(f.limit));
  }
  if (f.page && f.page > 1) next.set("page", String(f.page));
  if (f.sort) next.set("sort", f.sort);
  if (f.dir) next.set("dir", f.dir);
  return "#/logs" + (next.toString() ? "?" + next : "");
}

async function renderLogs() {
  if (renderNoProjectIfDetached("Logs")) return;
  renderBanners([]);
  const f = logsFiltersFromHash();
  // Preserve the filter shell's open/closed state across full re-renders.
  const logsFilterShell = document.getElementById("logs-filter-shell");
  const logsFilterShellOpen = logsFilterShell ? logsFilterShell.open : false;
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

  $("#logs-q").addEventListener("input", debounce(() => {
    updateLogsFilter({ q: $("#logs-q").value, page: 1 });
  }, 300));
  $("#logs-severity").addEventListener("change", (e) =>
    updateLogsFilter({ severity: e.target.value, page: 1 }));
  $("#logs-category").addEventListener("change", (e) =>
    updateLogsFilter({ category: e.target.value, page: 1 }));
  $("#logs-actor").addEventListener("change", (e) =>
    updateLogsFilter({ actor: e.target.value, page: 1 }));
  $("#logs-gap-id").addEventListener("input", debounce(() => {
    updateLogsFilter({ gap_id: $("#logs-gap-id").value.trim(), page: 1 });
  }, 300));
  $("#logs-limit").addEventListener("change", (e) =>
    updateLogsFilter({
      limit: parseInt(e.target.value, 10) || LOGS_DEFAULT_LIMIT,
      page: 1,
    }));
  $("#logs-clear").addEventListener("click", () => {
    history.replaceState(null, "", "#/logs");
    renderLogs();
  });

  await loadLogs();
}

function updateLogsFilter(patch) {
  const current = logsFiltersFromHash();
  const next = {
    severity: "severity" in patch ? patch.severity : current.severity,
    category: "category" in patch ? patch.category : current.category,
    actor: "actor" in patch ? patch.actor : current.actor,
    gap_id: "gap_id" in patch ? patch.gap_id : current.gap_id,
    q: "q" in patch ? patch.q : current.q,
    limit: "limit" in patch ? patch.limit : current.limit,
    page: "page" in patch ? patch.page : current.page,
    sort: "sort" in patch ? patch.sort : current.sort,
    dir: "dir" in patch ? patch.dir : current.dir,
  };
  history.replaceState(null, "", logsHashFromFilters(next));
  loadLogs();
}

function navigateLogsPage(page) {
  updateLogsFilter({ page });
}

async function loadLogs() {
  if (state.currentRoute !== "logs") return;
  if (renderNoProjectIfDetached("Logs")) return;
  const f = logsFiltersFromHash();
  const params = new URLSearchParams();
  if (f.severity) params.set("severity", f.severity);
  if (f.category) params.set("category", f.category);
  if (f.actor) params.set("actor", f.actor);
  if (f.gap_id) params.set("gap_id", f.gap_id);
  if (f.q) params.set("q", f.q);
  params.set("limit", String(f.limit));
  params.set("offset", String((f.page - 1) * f.limit));
  if (f.sort) params.set("sort", f.sort);
  if (f.dir) params.set("dir", f.dir);
  params.set("facets", "1");
  try {
    const data = await api("GET", "/api/activity?" + params);
    if (renderNoProjectIfApiDetached(data, "Logs")) return;
    drawLogsList(data, f);
  } catch (e) {
    $("#logs-list").innerHTML = `<p class="muted">${htmlEscape(e.message)}</p>`;
  }
}

function drawLogsList(data, f) {
  const entries = data.activity || [];
  const facets = data.facets || {};
  const pageMeta = data.page || {
    limit: f.limit,
    offset: (f.page - 1) * f.limit,
    has_more: false,
  };
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
    root.innerHTML = `
      <p class="muted">No log entries match the current filters.</p>
      ${renderPaginationControls("logs", pageMeta, 0, "entry", { boundaries: true })}`;
    bindPaginationControls(root, "logs", navigateLogsPage);
    return;
  }
  const columns = [
    { key: "datetime", label: "When" },
    { key: "severity", label: "Severity" },
    { key: "category", label: "Category" },
    { key: "actor", label: "Actor" },
    { key: "gap_id", label: "Gap" },
    { key: "message", label: "Message" },
  ];
  const sortHeads = columns.map((c) => {
    const isActive = c.key === f.effectiveSort;
    const arrow = isActive
      ? (f.effectiveDir === "asc" ? "↑" : "↓")
      : `<span class="sort-arrow-placeholder">↕</span>`;
    return `<th class="sortable ${isActive ? "active" : ""}"
                data-sort-key="${c.key}">
              ${c.label} <span class="sort-arrow">${arrow}</span>
            </th>`;
  }).join("");
  root.innerHTML = `
    <table class="table logs-table mobile-card-table">
      <thead><tr>${sortHeads}</tr></thead>
      <tbody>
        ${entries.map((e) => `
          <tr>
            <td class="muted small" data-label="When">${fmtTime(e.datetime)}</td>
            <td data-label="Severity"><span class="log-severity ${htmlEscape(e.severity || "info")}">${htmlEscape(e.severity || "info")}</span></td>
            <td class="muted small" data-label="Category">${htmlEscape(e.category || "")}</td>
            <td class="muted small" data-label="Actor">${htmlEscape(e.actor || "")}</td>
            <td class="muted small" data-label="Gap">${e.gap_id
              ? `<a href="#/gaps/${htmlEscape(e.gap_id)}">Gap ${htmlEscape(e.gap_id.slice(0, 8))}...</a>`
              : "-"}</td>
            <td class="logs-message-cell" data-label="Message">
              <div>${htmlEscape(e.message)}</div>
              ${e.details ? `<details><summary class="diff-show-details">Show details</summary><pre>${htmlEscape(e.details)}</pre></details>` : ""}
            </td>
          </tr>`).join("")}
      </tbody>
    </table>
    ${renderPaginationControls("logs", pageMeta, entries.length, "entry", { boundaries: true })}
  `;
  bindPaginationControls(root, "logs", navigateLogsPage);
  $$(".table th.sortable", root).forEach((th) => {
    th.addEventListener("click", () => {
      const key = th.dataset.sortKey;
      const nextDir = key === f.effectiveSort
        ? (f.effectiveDir === "asc" ? "desc" : "asc")
        : (LOGS_DEFAULT_DIR[key] || "desc");
      updateLogsFilter({ sort: key, dir: nextDir, page: 1 });
    });
  });
}

// Mirror of applyGapsFilterIndicator for the Logs screen.
function applyLogsFilterIndicator(f) {
  const active = {
    "logs-q": !!f.q,
    "logs-gap-id": !!f.gap_id,
    "logs-severity": !!f.severity,
    "logs-category": !!f.category,
    "logs-actor": !!f.actor,
    "logs-limit": f.limit !== LOGS_DEFAULT_LIMIT,
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
