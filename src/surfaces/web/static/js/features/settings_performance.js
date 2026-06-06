// ---- System / Performance ---------------------------------------------------

function fmtPerfMs(value) {
  const n = Number(value) || 0;
  if (n < 1000) return `${n.toFixed(n < 10 ? 1 : 0)} ms`;
  return `${(n / 1000).toFixed(n < 10_000 ? 2 : 1)} s`;
}

const PERFORMANCE_LIMIT_OPTIONS = [50, 100, 250, 500, 1000];
const PERFORMANCE_DEFAULT_LIMIT = 50;

function performanceFiltersFromHash() {
  const hashQs = new URLSearchParams(location.hash.split("?")[1] || "");
  return {
    operation: hashQs.get("operation") || "",
    success: hashQs.get("success") || "",
    limit: parseInt(hashQs.get("limit") || String(PERFORMANCE_DEFAULT_LIMIT), 10)
           || PERFORMANCE_DEFAULT_LIMIT,
    page: Math.max(1, parseInt(hashQs.get("page") || "1", 10) || 1),
  };
}

function performanceHashFromFilters(f) {
  const next = new URLSearchParams();
  if (f.operation) next.set("operation", f.operation);
  if (f.success) next.set("success", f.success);
  if (f.limit && f.limit !== PERFORMANCE_DEFAULT_LIMIT) {
    next.set("limit", String(f.limit));
  }
  if (f.page && f.page > 1) next.set("page", String(f.page));
  return "#/system/performance" + (next.toString() ? "?" + next : "");
}

function performanceApiPath(f = performanceFiltersFromHash()) {
  const params = new URLSearchParams();
  if (f.operation) params.set("operation", f.operation);
  if (f.success) params.set("success", f.success);
  params.set("limit", String(f.limit));
  params.set("offset", String((f.page - 1) * f.limit));
  return "/api/performance?" + params;
}

function renderPerformanceSummary(perf = {}) {
  const summary = perf.summary || [];
  if (!summary.length) return `<p class="muted">No performance metrics recorded yet.</p>`;
  return `
    <table class="table performance-summary-table mobile-card-table">
      <thead><tr>
        <th>Operation</th><th>Count</th><th>Failures</th>
        <th>Avg</th><th>P95</th><th>Max</th><th>Last seen</th>
      </tr></thead>
      <tbody>
        ${summary.map((row) => `
          <tr>
            <td data-label="Operation"><code>${htmlEscape(row.operation || "")}</code></td>
            <td data-label="Count">${fmtCount(row.count || 0)}</td>
            <td data-label="Failures">${fmtCount(row.failures || 0)}</td>
            <td data-label="Avg">${fmtPerfMs(row.avg_ms)}</td>
            <td data-label="P95">${fmtPerfMs(row.p95_ms)}</td>
            <td data-label="Max">${fmtPerfMs(row.max_ms)}</td>
            <td class="muted small" data-label="Last seen">${fmtTime(row.last_seen)}</td>
          </tr>`).join("")}
      </tbody>
    </table>`;
}

function renderPerformanceEvents(perf = {}) {
  const f = performanceFiltersFromHash();
  const events = perf.recent || [];
  const operations = perf.operations || [];
  const pageMeta = perf.page || {
    limit: f.limit,
    offset: (f.page - 1) * f.limit,
    has_more: false,
    total: perf.filtered_event_count || events.length,
  };
  const filterShellOpen = !!document.getElementById("performance-filter-shell")?.open;
  return `
    <details class="filter-shell" id="performance-filter-shell"${filterShellOpen ? " open" : ""}>
      <summary>
        <span class="filter-shell-title">Filters</span>
        <span class="spacer"></span>
        <span class="muted small"><span id="performance-count">${events.length} ${events.length === 1 ? "event" : "events"}</span></span>
        <span id="performance-filtered" class="filter-pill" hidden>Filtered</span>
      </summary>
      <div class="filter-shell-body">
        <div class="filter-bar">
          <div class="filter-row filter-row-filters">
            <label class="filter-field">${renderSettingsGuideLabel("Operation", "performance-operation-filter")}
              <select id="performance-operation-filter">
                <option value="">all operations</option>
                ${operations.map((op) => `
                  <option value="${htmlEscape(op)}" ${op === f.operation ? "selected" : ""}>${htmlEscape(op)}</option>`).join("")}
                ${f.operation && !operations.includes(f.operation)
                  ? `<option value="${htmlEscape(f.operation)}" selected>${htmlEscape(f.operation)}</option>` : ""}
              </select>
            </label>
            <label class="filter-field">${renderSettingsGuideLabel("Outcome", "performance-outcome-filter")}
              <select id="performance-success-filter">
                <option value="" ${f.success === "" ? "selected" : ""}>all outcomes</option>
                <option value="1" ${f.success === "1" ? "selected" : ""}>success</option>
                <option value="0" ${f.success === "0" ? "selected" : ""}>failure</option>
              </select>
            </label>
            <label class="filter-field">${renderSettingsGuideLabel("Limit", "performance-limit")}
              <select id="performance-limit">
                ${PERFORMANCE_LIMIT_OPTIONS.map((n) =>
                  `<option value="${n}" ${n === f.limit ? "selected" : ""}>${n} events</option>`).join("")}
              </select>
            </label>
            <span class="spacer"></span>
            <button class="secondary" id="performance-filter-clear">Clear filters</button>
          </div>
        </div>
      </div>
    </details>
    ${events.length ? `
      <table class="table performance-events-table mobile-card-table">
        <thead><tr>
          <th>When</th><th>Operation</th><th>Elapsed</th><th>Outcome</th>
          <th>Gap</th><th>Provider</th><th>Mode</th><th>Resource</th><th>Rows</th>
        </tr></thead>
        <tbody>
          ${events.map((event) => `
            <tr>
              <td class="muted small" data-label="When">${fmtTime(event.occurred_at)}</td>
              <td data-label="Operation"><code>${htmlEscape(event.operation || "")}</code></td>
              <td data-label="Elapsed">${fmtPerfMs(event.elapsed_ms)}</td>
              <td data-label="Outcome"><span class="status-pill ${event.success ? "done" : "failed"}">${event.success ? "success" : "failed"}</span></td>
              <td data-label="Gap">${event.gap_id ? `<a href="#/gaps/${htmlEscape(event.gap_id)}">${htmlEscape(event.gap_id.slice(0, 10))}...</a>` : ""}</td>
              <td data-label="Provider">${htmlEscape(event.provider || "")}</td>
              <td data-label="Mode">${htmlEscape(event.query_mode || "")}</td>
              <td class="muted small" data-label="Resource">${htmlEscape(performanceResourceLabel(event))}</td>
              <td class="muted small" data-label="Rows">${event.rows_returned ?? ""}${event.rows_scanned != null ? ` / ${event.rows_scanned}` : ""}</td>
            </tr>`).join("")}
        </tbody>
      </table>` : `<p class="muted">No recent events match the current filters.</p>`}
    ${renderPaginationControls("performance", pageMeta, events.length, "event")}`;
}

function performanceResourceLabel(event = {}) {
  const details = event.details || {};
  const parts = [];
  if (details.resource_backend) parts.push(details.resource_backend);
  if (details.resource_isolation) parts.push(details.resource_isolation);
  if (details.killed_reason) parts.push(details.killed_reason);
  return parts.join(" / ");
}

function renderSettingsPerformanceTab(performance, performanceBackend) {
  return `
    <section class="settings-section">
      <h3>${renderSettingsGuideLabel("Performance", "performance-overview")}</h3>
      <p class="scope-label muted small">Local runtime history</p>
      <p class="muted small" style="margin-top:0">
        SQLite-only metrics for Refine operations. Retention is
        ${Number(performance.retention_days || 30)} days.
      </p>
      <dl class="kv">
        <dt>Process model</dt><dd>${htmlEscape(backendProcessLabel(performanceBackend))}</dd>
        <dt>Metric store</dt><dd>Shared SQLite runtime history</dd>
        <dt>Events retained</dt><dd>${fmtCount(performance.event_count || 0)}</dd>
        <dt>Total stored</dt><dd>${fmtCount(performance.total_event_count || 0)}</dd>
      </dl>
      <div class="actions" style="margin-top:10px">
        <button class="secondary" id="performance-refresh">Refresh</button>
        <button class="secondary" id="performance-prune">Prune old metrics</button>
        <button class="danger" id="performance-clear">Clear metrics</button>
      </div>
    </section>
    <section class="settings-section">
      <h3>Summary</h3>
      ${renderPerformanceSummary(performance)}
    </section>
    <section class="settings-section">
      <h3>Recent events</h3>
      ${renderPerformanceEvents(performance)}
    </section>`;
}

function bindSettingsPerformanceTab(
  s, diag, reps, feats, gov, dash, nodeData, guidanceData, performanceBackend = null,
) {
  $("#performance-refresh")?.addEventListener("click", async (e) => {
    await withButtonBusy(e.currentTarget, "Refreshing…", async () => {
      await refreshSettingsTab("performance", { force: true });
    });
  });
  $("#performance-prune")?.addEventListener("click", async (e) => {
    await withButtonBusy(e.currentTarget, "Pruning…", async () => {
      try {
        const r = await api("POST", "/api/performance/cleanup", {});
        toast(`Deleted ${r.deleted} old metric event${r.deleted === 1 ? "" : "s"}.`, "info");
        await refreshSettingsTab("performance", { force: true });
      } catch (e) { await showActionError(e); }
    });
  });
  $("#performance-clear")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    const ok = await modalConfirm(
      "Delete all local performance metrics? This cannot be undone.",
      { title: "Clear metrics", okLabel: "Clear", danger: true },
    );
    if (!ok) return;
    await withButtonBusy(btn, "Clearing…", async () => {
      try {
        const r = await api("POST", "/api/performance/cleanup", { clear: true });
        toast(`Deleted ${r.deleted} metric event${r.deleted === 1 ? "" : "s"}.`, "info");
        await refreshSettingsTab("performance", { force: true });
      } catch (e) { await showActionError(e); }
    });
  });
  $("#performance-operation-filter")?.addEventListener("change", (e) =>
    updatePerformanceFilter({ operation: e.target.value, page: 1 }));
  $("#performance-success-filter")?.addEventListener("change", (e) =>
    updatePerformanceFilter({ success: e.target.value, page: 1 }));
  $("#performance-limit")?.addEventListener("change", (e) =>
    updatePerformanceFilter({
      limit: parseInt(e.target.value, 10) || PERFORMANCE_DEFAULT_LIMIT,
      page: 1,
    }));
  $("#performance-filter-clear")?.addEventListener("click", () => {
    history.replaceState(null, "", "#/system/performance");
    loadPerformanceEvents(performanceBackend || diag?.backend || {});
  });
  const root = document.querySelector('[data-tab-pane="performance"]');
  if (root) {
    bindPaginationControls(root, "performance", (page) =>
      updatePerformanceFilter({ page }));
  }
  applyPerformanceFilterIndicator(performanceFiltersFromHash());
}

function updatePerformanceFilter(patch) {
  const current = performanceFiltersFromHash();
  const next = {
    operation: "operation" in patch ? patch.operation : current.operation,
    success: "success" in patch ? patch.success : current.success,
    limit: "limit" in patch ? patch.limit : current.limit,
    page: "page" in patch ? patch.page : current.page,
  };
  history.replaceState(null, "", performanceHashFromFilters(next));
  loadPerformanceEvents();
}

async function loadPerformanceEvents(performanceBackend = null) {
  try {
    const filtered = await api("GET", performanceApiPath());
    const backend = performanceBackend || filtered.backend || {};
    updateSettingsTabContent(
      "performance",
      renderSettingsPerformanceTab(filtered, backend),
      () => bindSettingsPerformanceTab(null, {}, [], null, {}, {}, {}, {}, backend),
    );
  } catch (e) { await showActionError(e); }
}

function applyPerformanceFilterIndicator(f) {
  const active = {
    "performance-operation-filter": !!f.operation,
    "performance-success-filter": !!f.success,
    "performance-limit": f.limit !== PERFORMANCE_DEFAULT_LIMIT,
  };
  let anyActive = false;
  for (const [id, on] of Object.entries(active)) {
    const el = document.getElementById(id);
    if (!el) continue;
    el.classList.toggle("filter-active", on);
    if (on) anyActive = true;
  }
  const pill = $("#performance-filtered");
  if (pill) pill.hidden = !anyActive;
  const pane = document.querySelector('[data-tab-pane="performance"]');
  if (pane) {
    pane.querySelector(".settings-section:last-child")?.classList.toggle(
      "results-filtered",
      anyActive,
    );
  }
}
