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
