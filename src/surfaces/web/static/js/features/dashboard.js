// ---- Dashboard --------------------------------------------------------------

const dashboardReviewSelectedIds = new Set();
let dashboardReviewSelectedReporter = "";
let dashboardRefreshSeq = 0;
let dashboardRefreshInFlight = false;
let dashboardRefreshQueued = false;
let dashboardRetryTimer = null;
const DASHBOARD_REFRESH_TIMEOUT_MS = 6000;
const DASHBOARD_PANEL_STORAGE_PREFIX = "refine_dashboard_panel_open:";
function dashboardScopeFromHash() {
  const hashQs = new URLSearchParams(location.hash.split("?")[1] || "");
  return hashQs.get("node") === "all" ? "all" : "current";
}

function dashboardHash(scope) {
  return scope === "all" ? "#/?node=all" : "#/";
}

function dashboardScopeParam(d = null) {
  return d?.node_filter || dashboardScopeFromHash();
}

function dashboardPanelStorageKey(panelId) {
  return `${DASHBOARD_PANEL_STORAGE_PREFIX}${panelId}`;
}

function dashboardPanelOpen(panelId, fallback) {
  const existing = document.getElementById(panelId);
  if (existing) return existing.open;
  try {
    const stored = localStorage.getItem(dashboardPanelStorageKey(panelId));
    if (stored === "open") return true;
    if (stored === "closed") return false;
  } catch (_) {}
  return fallback;
}

function wireDashboardPanelPersistence(panelId) {
  const panel = document.getElementById(panelId);
  if (!panel) return;
  panel.addEventListener("toggle", () => {
    try {
      localStorage.setItem(dashboardPanelStorageKey(panelId), panel.open ? "open" : "closed");
    } catch (_) {}
  });
}

async function renderDashboard() {
  if (renderNoProjectIfDetached("Dashboard")) return;
  // First paint only: lay out the outer chrome and a `Loading…`
  // placeholder. SSE-triggered refreshes route through `refreshDashboard`
  // below so the screen doesn't flicker back to `Loading…` between events.
  // Fresh navigation should not paint cached counts as current; once the
  // dashboard is already visible, refreshes redraw over the existing DOM.
  if (!document.getElementById("dash")) {
    $("#main").innerHTML = `
      <div class="dashboard-title-row">
        <h2>Dashboard</h2>
        <div class="segmented-control dashboard-scope-switch" role="group" aria-label="Dashboard node scope">
          <button type="button" data-dashboard-scope="current" data-testid="dashboard-scope-current">Current</button>
          <button type="button" data-dashboard-scope="all" data-testid="dashboard-scope-all">All</button>
        </div>
      </div>
      <div id="dash"><p class="muted">Loading…</p></div>`;
    wireDashboardScopeSwitch();
  }
  await refreshDashboard();
}

async function refreshDashboard() {
  if (renderNoProjectIfDetached("Dashboard")) return;
  // Silent refresh — fetch + redraw in place, no `Loading…` flash. Used
  // by both the route handler (after the first-paint scaffold above) and
  // every SSE handler that wants the dashboard to track live state.
  if (state.currentRoute !== "dashboard") return;
  if (dashboardRefreshInFlight) {
    dashboardRefreshQueued = true;
    return;
  }
  dashboardRefreshInFlight = true;
  dashboardRefreshQueued = false;
  if (dashboardRetryTimer) {
    clearTimeout(dashboardRetryTimer);
    dashboardRetryTimer = null;
  }
  const refreshSeq = ++dashboardRefreshSeq;
  try {
    const reporter = state.lastReporter || "";
    const scope = dashboardScopeFromHash();
    const nodeParam = encodeURIComponent(scope);
    const [d, reviews] = await Promise.all([
      dashboardApi("GET", `/api/dashboard?node=${nodeParam}`),
      reporter
        ? dashboardApi("GET", "/api/gaps?status=review&assignee=" + encodeURIComponent(reporter) + `&node=${nodeParam}&limit=200`)
        : Promise.resolve({ gaps: [] }),
    ]);
    if (refreshSeq !== dashboardRefreshSeq || state.currentRoute !== "dashboard") return;
    if (renderNoProjectIfApiDetached(d, "Dashboard")) return;
    state.dashboard = d;
    state.dashboardReviewSnapshot = { reviewsForReporter: reviews.gaps || [], reporter };
    drawDashboard(d, state.dashboardReviewSnapshot);
  } catch (e) {
    if (refreshSeq !== dashboardRefreshSeq || state.currentRoute !== "dashboard") return;
    const dash = document.getElementById("dash");
    const hasRenderedDashboard = !!dash?.querySelector(".dashboard-status-grid");
    if (dash && !hasRenderedDashboard) {
      const waiting = e.name === "AbortError"
        ? "Dashboard is still waiting for the backend. Retrying…"
        : `Failed to load: ${htmlEscape(e.message)}`;
      dash.innerHTML = `<p class="muted">${waiting}</p>`;
    }
    scheduleDashboardRetry();
  } finally {
    dashboardRefreshInFlight = false;
    if (dashboardRefreshQueued && state.currentRoute === "dashboard") {
      dashboardRefreshQueued = false;
      refreshDashboard();
    }
  }
}

async function dashboardApi(method, path) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), DASHBOARD_REFRESH_TIMEOUT_MS);
  try {
    return await api(method, path, undefined, { signal: controller.signal });
  } finally {
    clearTimeout(timer);
  }
}

function scheduleDashboardRetry() {
  if (dashboardRetryTimer || state.currentRoute !== "dashboard") return;
  dashboardRetryTimer = setTimeout(() => {
    dashboardRetryTimer = null;
    if (state.currentRoute === "dashboard") refreshDashboard();
  }, 2000);
}

function drawDashboard(d, opts = {}) {
  const reviewsForReporter = opts.reviewsForReporter || [];
  const reviewReporter = opts.reporter || "";
  const scope = dashboardScopeParam(d);
  const reviewSelectionKey = `${scope}:${reviewReporter}`;
  if (dashboardReviewSelectedReporter !== reviewSelectionKey) {
    dashboardReviewSelectedIds.clear();
    dashboardReviewSelectedReporter = reviewSelectionKey;
  }
  if (!reviewsForReporter.length) dashboardReviewSelectedIds.clear();
  // Global banners
  const banners = (d.needs_attention || []).filter((x) => x.kind === "banner")
    .map((x) => ({
      severity: x.severity || "error",
      message: x.message,
      action: /Refine cannot reach/i.test(x.message) ? {
        label: "Re-check auth",
        onClick: async () => {
          try {
            await api("POST", "/api/settings/recheck-auth");
            toast("Pre-flight re-run requested", "info");
            await refreshDashboard();
          } catch (e) {
            toast(e.message, "error");
          }
        },
      } : null,
    }));
  renderBanners(banners);

  const needsAttention = (d.needs_attention || []).filter((x) => x.kind === "filter");
  const counts = d.counts || {};
  const orderedStatuses = workflowStatuses();
  const dash = $("#dash");
  const assigneeStats = d.assignee_stats || d.reporter_stats || [];
  const reviewsShellOpen = dashboardPanelOpen("reviews-for-reporter-card", true);
  const assigneeStatsShellOpen = dashboardPanelOpen("dashboard-assignee-stats-shell", false);
  const showReviewPanel = !!reviewReporter || needsAttention.length > 0;
  syncDashboardScopeSwitch(scope);
  // Guard against late-arriving SSE refreshes after the user navigated
  // away — the container is gone, so just bail silently.
  if (!dash) return;
  dash.innerHTML = `
    ${renderWorkflowVisualization({
      counts,
      statuses: orderedStatuses,
      hrefForStatus: (s) => gapsHash({ status: s, node: scope }),
      className: "dashboard-status-grid",
    })}

    ${showReviewPanel ? `
    <details class="filter-shell dashboard-collapsible-shell" id="reviews-for-reporter-card" data-testid="dashboard-review-panel"${reviewsShellOpen ? " open" : ""}>
      <summary data-testid="dashboard-review-summary">
        <span class="filter-shell-title">Awaiting your Review</span>
        ${reviewReporter ? `<span class="muted small">${htmlEscape(reviewReporter)}</span>` : ""}
        <span class="filter-pill" data-testid="dashboard-review-count">${fmtCount(reviewsForReporter.length)}</span>
        ${needsAttention.length ? `<span class="filter-pill">Needs attention</span>` : ""}
      </summary>
      <div class="filter-shell-body">
        ${needsAttention.length ? `
          <div class="actions dashboard-panel-actions">
            ${needsAttention.map((x) => `
              <a href="${gapsHash({
                status: x.filter?.status || "",
                node: x.filter?.node || scope,
              })}" class="btn">
                ${htmlEscape(x.message)}
              </a>`).join("")}
          </div>` : ""}
        ${reviewsForReporter.length === 0 ? "" : `
          <div class="actions dashboard-panel-actions">
            <button id="rev-bulk-verify" data-testid="dashboard-review-bulk-verify" disabled>Verify selected</button>
          </div>`}
      ${!reviewReporter
        ? ""
        : reviewsForReporter.length === 0
        ? `<div class="empty-state">
             <div class="empty-state-title">You're clear.</div>
             <div>No review items are assigned to you right now.</div>
           </div>`
        : `<table class="table">
            <thead><tr>
              <th class="gap-select-col">
                <input type="checkbox" id="rev-select-all"
                       data-testid="dashboard-review-select-all"
                       aria-label="Select all reviews">
              </th>
              <th>Gap</th>
              <th>Updated</th>
              <th class="actions-col" style="white-space:nowrap"></th>
            </tr></thead>
            <tbody>
              ${reviewsForReporter.map((g) => `
                <tr data-rev-row="${g.id}" data-testid="dashboard-review-row">
                  <td class="gap-select-col"><input type="checkbox" class="rev-row-check" data-testid="dashboard-review-check" data-rev-id="${g.id}"></td>
                  <td>
                    <a href="#/gaps/${g.id}" title="${htmlEscape(g.id)}">
                      ${htmlEscape(g.name)}
                    </a>
                  </td>
                  <td class="muted small">${fmtTime(g.updated)}</td>
                  <td class="actions" style="white-space:nowrap">
                    <button data-rev-verify="${g.id}" data-testid="dashboard-review-verify">Verify →</button>
                    <button class="secondary" data-rev-add-round="${g.id}"
                            data-testid="dashboard-review-add-round"
                            data-rev-name="${htmlEscape(g.name)}">Add round</button>
                  </td>
                </tr>`).join("")}
            </tbody>
          </table>`}
      </div>
    </details>` : ""}

    <details class="filter-shell dashboard-collapsible-shell" id="dashboard-assignee-stats-shell" data-testid="dashboard-assignee-stats-panel"${assigneeStatsShellOpen ? " open" : ""}>
      <summary data-testid="dashboard-assignee-stats-summary">
        <span class="filter-shell-title">Assignee throughput</span>
        <span class="filter-pill" data-testid="dashboard-assignee-stats-count">${fmtCount(assigneeStats.length)}</span>
      </summary>
      <div class="filter-shell-body">
        ${assigneeStats.length === 0
          ? `<p class="muted">No assignee activity yet.</p>`
          : `<table class="table">
              <thead><tr>
                <th>Assignee</th>
                <th>Active</th>
                <th>Done</th>
                <th>Assigned</th>
                <th>Review</th>
                <th>Done / Assigned</th>
              </tr></thead>
              <tbody>
                ${assigneeStats.map((s) => {
                  const assignee = s.assignee || s.reporter || "";
                  return `
                  <tr class="assignee-stats-row"
                      data-testid="dashboard-assignee-stats-row"
                      data-assignee="${htmlEscape(assignee)}"
                      title="See Gaps assigned to ${htmlEscape(assignee)}">
                    <td>${htmlEscape(assignee)}</td>
                    <td>${fmtCount(s.active)}</td>
                    <td>${fmtCount(s.done)}</td>
                    <td>${fmtCount(s.assigned || 0)}</td>
                    <td>${fmtCount(s.assigned_review || 0)}</td>
                    <td><span class="metric-good">${s.completion_rate.toFixed(1)}%</span></td>
                  </tr>`;
                }).join("")}
              </tbody>
            </table>`}
      </div>
    </details>

  `;
  // Click any assignee row -> deep-link into the Gaps list filtered by
  // that assignee. We use data-assignee + a delegated listener so the
  // name can contain spaces/quotes without HTML-escaping hazards.
  $$(".assignee-stats-row").forEach((row) => {
    row.addEventListener("click", () => {
      location.hash = gapsHash({ assignee: row.dataset.assignee, node: scope });
    });
  });

  wireDashboardPanelPersistence("reviews-for-reporter-card");
  wireDashboardPanelPersistence("dashboard-assignee-stats-shell");
  wireReviewsForReporter(reviewsForReporter);
}

function wireDashboardScopeSwitch() {
  $$(".dashboard-scope-switch [data-dashboard-scope]").forEach((btn) => {
    btn.addEventListener("click", () => {
      location.hash = dashboardHash(btn.dataset.dashboardScope || "current");
    });
  });
  syncDashboardScopeSwitch(dashboardScopeFromHash());
}

function syncDashboardScopeSwitch(scope) {
  $$(".dashboard-scope-switch [data-dashboard-scope]").forEach((btn) => {
    const active = btn.dataset.dashboardScope === scope;
    btn.classList.toggle("active", active);
    btn.setAttribute("aria-pressed", active ? "true" : "false");
  });
}

function wireReviewsForReporter(reviews) {
  if (!reviews || !reviews.length) return;
  const card = document.getElementById("reviews-for-reporter-card");
  if (!card) return;
  const reviewIds = new Set(reviews.map((g) => g.id));
  for (const id of Array.from(dashboardReviewSelectedIds)) {
    if (!reviewIds.has(id)) dashboardReviewSelectedIds.delete(id);
  }
  const checks = () => $$(".rev-row-check", card);
  const selected = () => reviews
    .map((g) => g.id)
    .filter((id) => dashboardReviewSelectedIds.has(id));
  const syncBulkButton = () => {
    const btn = $("#rev-bulk-verify", card);
    if (!btn) return;
    const n = selected().length;
    btn.disabled = n === 0;
    btn.textContent = n === 0 ? "Verify selected" : `Verify selected (${n})`;
    if (selectAll) {
      selectAll.checked = n > 0 && n === checks().length;
      selectAll.indeterminate = n > 0 && n < checks().length;
    }
  };
  const selectAll = $("#rev-select-all", card);
  selectAll?.addEventListener("change", () => {
    checks().forEach((c) => {
      c.checked = selectAll.checked;
      if (selectAll.checked) dashboardReviewSelectedIds.add(c.dataset.revId);
      else dashboardReviewSelectedIds.delete(c.dataset.revId);
    });
    syncBulkButton();
  });
  checks().forEach((c) => {
    c.checked = dashboardReviewSelectedIds.has(c.dataset.revId);
    c.addEventListener("change", () => {
      if (c.checked) dashboardReviewSelectedIds.add(c.dataset.revId);
      else dashboardReviewSelectedIds.delete(c.dataset.revId);
      syncBulkButton();
    });
  });

  $$("[data-rev-verify]", card).forEach((btn) => {
    btn.addEventListener("click", async () => {
      const id = btn.dataset.revVerify;
      await withButtonBusy(btn, "Verifying…", async () => {
        try {
          const r = await api("POST", `/api/gaps/${id}/verify`);
          if (r.ok) toast(r.message || "Verified", "info");
          else toast(r.message || "Verify did not complete", "error");
          if (r.ok) dashboardReviewSelectedIds.delete(id);
        } catch (e) { await showActionError(e); }
        await refreshDashboard();
      });
    });
  });

  $$("[data-rev-add-round]", card).forEach((btn) => {
    btn.addEventListener("click", () => {
      openAddRoundModal({
        gapId: btn.dataset.revAddRound,
        gapName: btn.dataset.revName || "",
      });
    });
  });

  $("#rev-bulk-verify", card)?.addEventListener("click", async () => {
    const ids = selected();
    if (!ids.length) return;
    const ok = await modalConfirm(
      `Verify ${ids.length} gap${ids.length === 1 ? "" : "s"}?`,
      { title: "Bulk verify", okLabel: "Verify all" },
    );
    if (!ok) return;
    const btn = $("#rev-bulk-verify", card);
    await withButtonBusy(btn, `Verifying 0/${ids.length}…`, async () => {
      let done = 0, failed = 0;
      let ownershipError = null;
      for (const id of ids) {
        btn.textContent = `Verifying ${done + 1}/${ids.length}…`;
        try {
          const r = await api("POST", `/api/gaps/${id}/verify`);
          if (!r.ok) failed++;
          else dashboardReviewSelectedIds.delete(id);
        } catch (e) {
          failed++;
          if (isNodeOwnershipError(e) && !ownershipError) ownershipError = e;
        }
        done++;
      }
      if (ownershipError) await showActionError(ownershipError);
      const msg = failed
        ? `Verified ${done - failed} of ${ids.length} — ${failed} did not complete`
        : `Verified ${done} gap${done === 1 ? "" : "s"}`;
      toast(msg, failed ? "error" : "info");
      await refreshDashboard();
    });
  });

  syncBulkButton();
}

function openAddRoundModal({ gapId, gapName }) {
  const reporter = state.lastReporter || "";
  if (!reporter) {
    toast("Pick a reporter in the top-right selector first", "error");
    return;
  }
  const root = document.createElement("div");
  root.className = "modal-backdrop";
  root.innerHTML = `
    <div class="modal" role="dialog" aria-modal="true"
         data-testid="dashboard-add-round-modal"
         aria-labelledby="add-round-title" style="max-width:560px">
      <div class="modal-title" id="add-round-title">
        Add round — ${htmlEscape(gapName || gapId)}
      </div>
      <div class="modal-body">
        <div class="muted small" style="margin-bottom:8px">
          Submitting as <strong>${htmlEscape(reporter)}</strong>
          — change in the top-right reporter selector.
        </div>
        <form id="add-round-form">
          <div class="form-row">
            <label>Actual (current behavior)</label>
            <textarea name="actual" data-testid="dashboard-add-round-actual" placeholder="What's still happening?"></textarea>
          </div>
          <div class="form-row">
            <label>Target (desired behavior)</label>
            <textarea name="target" data-testid="dashboard-add-round-target" placeholder="What should be happening?"></textarea>
          </div>
        </form>
      </div>
      <div class="modal-actions">
        <button class="secondary" data-cancel data-testid="dashboard-add-round-cancel">Cancel</button>
        <button data-ok data-testid="dashboard-add-round-submit">Submit new round</button>
      </div>
    </div>`;
  document.body.appendChild(root);
  let closed = false;
  const close = () => {
    if (closed) return;
    closed = true;
    document.removeEventListener("keydown", onKey, true);
    root.remove();
  };
  const onKey = (e) => { if (e.key === "Escape") close(); };
  document.addEventListener("keydown", onKey, true);
  root.addEventListener("click", (e) => { if (e.target === root) close(); });
  root.querySelector("[data-cancel]").addEventListener("click", close);
  const submit = async () => {
    const form = root.querySelector("#add-round-form");
    const fd = new FormData(form);
    const actual = (fd.get("actual") || "").toString().trim();
    const target = (fd.get("target") || "").toString().trim();
    if (!actual && !target) return toast("Provide actual or target", "error");
    const okBtn = root.querySelector("[data-ok]");
    await withButtonBusy(okBtn, "Submitting…", async () => {
      try {
        await api("POST", `/api/gaps/${gapId}/rounds`,
                  { reporter, actual, target });
        toast("New round submitted", "info");
        close();
        await refreshDashboard();
      } catch (err) { await showActionError(err); }
    });
  };
  root.querySelector("[data-ok]").addEventListener("click", submit);
  root.querySelector("#add-round-form").addEventListener("submit", (e) => {
    e.preventDefault(); submit();
  });
  root.querySelector("textarea[name='actual']")?.focus();
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
