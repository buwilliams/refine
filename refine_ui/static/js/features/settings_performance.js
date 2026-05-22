// ---- System / Performance ---------------------------------------------------

function fmtPerfMs(value) {
  const n = Number(value) || 0;
  if (n < 1000) return `${n.toFixed(n < 10 ? 1 : 0)} ms`;
  return `${(n / 1000).toFixed(n < 10_000 ? 2 : 1)} s`;
}

function renderPerformanceSummary(perf = {}) {
  const summary = perf.summary || [];
  if (!summary.length) return `<p class="muted">No performance metrics recorded yet.</p>`;
  return `
    <table class="table">
      <thead><tr>
        <th>Operation</th><th>Count</th><th>Failures</th>
        <th>Avg</th><th>P95</th><th>Max</th><th>Last seen</th>
      </tr></thead>
      <tbody>
        ${summary.map((row) => `
          <tr>
            <td><code>${htmlEscape(row.operation || "")}</code></td>
            <td>${fmtCount(row.count || 0)}</td>
            <td>${fmtCount(row.failures || 0)}</td>
            <td>${fmtPerfMs(row.avg_ms)}</td>
            <td>${fmtPerfMs(row.p95_ms)}</td>
            <td>${fmtPerfMs(row.max_ms)}</td>
            <td class="muted small">${fmtTime(row.last_seen)}</td>
          </tr>`).join("")}
      </tbody>
    </table>`;
}

function renderPerformanceEvents(perf = {}) {
  const events = perf.recent || [];
  const operations = perf.operations || [];
  const option = (op) => `<option value="${htmlEscape(op)}">${htmlEscape(op)}</option>`;
  return `
    <div class="filter-row" style="margin-bottom:10px">
      <select id="performance-operation-filter">
        <option value="">All operations</option>
        ${operations.map(option).join("")}
      </select>
      <select id="performance-success-filter">
        <option value="">All outcomes</option>
        <option value="1">Success</option>
        <option value="0">Failure</option>
      </select>
      <button class="secondary" id="performance-filter-apply">Apply</button>
    </div>
    ${events.length ? `
      <table class="table">
        <thead><tr>
          <th>When</th><th>Operation</th><th>Elapsed</th><th>Outcome</th>
          <th>Gap</th><th>Provider</th><th>Mode</th><th>Resource</th><th>Rows</th>
        </tr></thead>
        <tbody>
          ${events.map((event) => `
            <tr>
              <td class="muted small">${fmtTime(event.occurred_at)}</td>
              <td><code>${htmlEscape(event.operation || "")}</code></td>
              <td>${fmtPerfMs(event.elapsed_ms)}</td>
              <td><span class="status-pill ${event.success ? "done" : "failed"}">${event.success ? "success" : "failed"}</span></td>
              <td>${event.gap_id ? `<a href="#/gaps/${htmlEscape(event.gap_id)}">${htmlEscape(event.gap_id.slice(0, 10))}...</a>` : ""}</td>
              <td>${htmlEscape(event.provider || "")}</td>
              <td>${htmlEscape(event.query_mode || "")}</td>
              <td class="muted small">${htmlEscape(performanceResourceLabel(event))}</td>
              <td class="muted small">${event.rows_returned ?? ""}${event.rows_scanned != null ? ` / ${event.rows_scanned}` : ""}</td>
            </tr>`).join("")}
        </tbody>
      </table>` : `<p class="muted">No recent events match the current filters.</p>`}`;
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
      <h3>Performance</h3>
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

function bindSettingsPerformanceTab(s, diag, reps, feats, gov, dash, instanceData, guidanceData) {
  $("#performance-refresh")?.addEventListener("click", async () => {
    await withButtonBusy($("#performance-refresh"), "Refreshing…", async () => {
      await refreshSettings({ force: true });
    });
  });
  $("#performance-prune")?.addEventListener("click", async () => {
    await withButtonBusy($("#performance-prune"), "Pruning…", async () => {
      try {
        const r = await api("POST", "/api/performance/cleanup", {});
        toast(`Deleted ${r.deleted} old metric event${r.deleted === 1 ? "" : "s"}.`, "info");
        await refreshSettings({ force: true });
      } catch (e) { await showActionError(e); }
    });
  });
  $("#performance-clear")?.addEventListener("click", async () => {
    const ok = await modalConfirm(
      "Delete all local performance metrics? This cannot be undone.",
      { title: "Clear metrics", okLabel: "Clear", danger: true },
    );
    if (!ok) return;
    await withButtonBusy($("#performance-clear"), "Clearing…", async () => {
      try {
        const r = await api("POST", "/api/performance/cleanup", { clear: true });
        toast(`Deleted ${r.deleted} metric event${r.deleted === 1 ? "" : "s"}.`, "info");
        await refreshSettings({ force: true });
      } catch (e) { await showActionError(e); }
    });
  });
  $("#performance-filter-apply")?.addEventListener("click", async () => {
    const params = new URLSearchParams();
    const op = $("#performance-operation-filter")?.value || "";
    const outcome = $("#performance-success-filter")?.value || "";
    if (op) params.set("operation", op);
    if (outcome) params.set("success", outcome);
    try {
      const filtered = await api("GET", "/api/performance?" + params);
      drawSettings(s, diag, reps, feats, gov, dash, instanceData, guidanceData, filtered);
      const opSel = $("#performance-operation-filter");
      const successSel = $("#performance-success-filter");
      if (opSel) opSel.value = op;
      if (successSel) successSel.value = outcome;
    } catch (e) { await showActionError(e); }
  });
}
