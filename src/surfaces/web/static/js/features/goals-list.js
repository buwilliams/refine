// ---- Goals: list -------------------------------------------------------------

const GOALS_DEFAULT_DIR = {
  name: "asc", status: "asc", priority: "asc",
  reporter: "asc", assignee: "asc", rounds: "asc", node: "asc", updated: "desc", id: "desc",
};

// Mirror Logs' entries-limit dropdown so the two screens feel consistent.
const GOALS_LIMIT_OPTIONS = [50, 100, 250, 500, 1000];
const GOALS_DEFAULT_LIMIT = 50;

function goalsHash(parts) {
  const next = new URLSearchParams();
  if (parts.q)        next.set("q", parts.q);
  if (parts.status)   next.set("status", parts.status);
  if (parts.reporter) next.set("reporter", parts.reporter);
  if (parts.assignee) next.set("assignee", parts.assignee);
  if (parts.feature)  next.set("feature", parts.feature);
  if (parts.rounds_gte !== undefined && parts.rounds_gte !== "") {
    next.set("rounds_gte", parts.rounds_gte);
  }
  if (parts.rounds_lte !== undefined && parts.rounds_lte !== "") {
    next.set("rounds_lte", parts.rounds_lte);
  }
  if (parts.node) next.set("node", parts.node);
  if (parts.severity) next.set("severity", parts.severity);
  if (parts.category) next.set("category", parts.category);
  if (parts.actor)    next.set("actor", parts.actor);
  if (parts.limit && parts.limit !== GOALS_DEFAULT_LIMIT) next.set("limit", String(parts.limit));
  if (parts.page && parts.page > 1) next.set("page", String(parts.page));
  if (parts.sort)     next.set("sort", parts.sort);
  if (parts.dir)      next.set("dir", parts.dir);
  return "#/goals" + (next.toString() ? "?" + next : "");
}

async function renderGoalsList() {
  if (renderNoProjectIfDetached("Goals")) return;
  renderBanners([]);
  await ensureGoalsNodeOptions();
  const f = goalsFilterFromHash();
  // Preserve the filter shell's open/closed state across full re-renders
  // (Clear filters, bulk-op completion, etc.). First-ever render defaults
  // closed, with the summary strip visible as the open control.
  const filterShell = document.getElementById("goals-filter-shell");
  const filterShellOpen = filterShell ? filterShell.open : false;

  $("#main").innerHTML = `
    <h2>Goals</h2>
    <div id="goals-workflow" class="goals-workflow" data-testid="goals-workflow">
      ${renderWorkflowVisualization({
        counts: {},
        hrefForStatus: (status) => goalsWorkflowStatusHash(status, f),
        className: "goals-workflow-grid",
      })}
    </div>
    <details class="filter-shell" id="goals-filter-shell" data-testid="goals-filter-shell"${filterShellOpen ? " open" : ""}>
      <summary data-testid="goals-filter-summary">
        <span class="filter-shell-title">Filters &amp; bulk actions</span>
        <span class="spacer"></span>
        <span class="muted small"><span id="goals-count" data-testid="goals-count"></span></span>
        <span id="goals-filtered" class="filter-pill" data-testid="goals-filtered-pill" hidden>Filtered</span>
      </summary>
      <div class="filter-shell-body">
    <div class="filter-bar">
      <div class="filter-row filter-row-primary">
        <input type="text" id="search" class="filter-grow"
               data-testid="goals-search"
               placeholder="Search goals…" value="${htmlEscape(f.q)}">
      </div>
      <div class="filter-row filter-row-activity">
        <select id="filter-status" data-testid="goals-status-filter">
          ${STATUS_FILTER_OPTIONS
            .map((s) => `<option value="${s}" ${s === f.status ? "selected" : ""}>${s ? workflowStatusLabel(s) : "all statuses"}</option>`).join("")}
        </select>
        <select id="filter-reporter" data-testid="goals-reporter-filter">
          <option value="" ${f.reporter === "" ? "selected" : ""}>all reporters</option>
          ${(state.reporters || []).map((r) =>
            `<option value="${htmlEscape(r.name)}" ${r.name === f.reporter ? "selected" : ""}>${htmlEscape(r.name)}</option>`).join("")}
          ${f.reporter && !(state.reporters || []).some((r) => r.name === f.reporter)
            ? `<option value="${htmlEscape(f.reporter)}" selected>${htmlEscape(f.reporter)}</option>` : ""}
        </select>
        <select id="filter-assignee" data-testid="goals-assignee-filter">
          <option value="" ${f.assignee === "" ? "selected" : ""}>all assignees</option>
          ${(state.reporters || []).map((r) =>
            `<option value="${htmlEscape(r.name)}" ${r.name === f.assignee ? "selected" : ""}>${htmlEscape(r.name)}</option>`).join("")}
          ${f.assignee && !(state.reporters || []).some((r) => r.name === f.assignee)
            ? `<option value="${htmlEscape(f.assignee)}" selected>${htmlEscape(f.assignee)}</option>` : ""}
        </select>
        <select id="filter-node" data-testid="goals-node-filter">
          <option value="all" ${f.node === "" || f.node === "all" ? "selected" : ""}>all nodes</option>
          <option value="current" ${f.node === "current" ? "selected" : ""}>current node</option>
          ${(state.project?.nodes || []).map((inst) =>
            `<option value="${htmlEscape(inst.id)}" ${inst.id === f.node ? "selected" : ""}>${htmlEscape(inst.display_name || inst.id)}</option>`).join("")}
          ${f.node && !["all", "current"].includes(f.node) && !(state.project?.nodes || []).some((inst) => inst.id === f.node)
            ? `<option value="${htmlEscape(f.node)}" selected>${htmlEscape(f.node)}</option>` : ""}
        </select>
        <input type="text" id="filter-feature" class="filter-feature"
               data-testid="goals-feature-filter"
               placeholder="Feature ID or standalone" value="${htmlEscape(f.feature)}">
        <input type="number" id="filter-rounds-gte" class="filter-number"
               data-testid="goals-rounds-gte-filter"
               min="0" step="1" inputmode="numeric"
               placeholder="Rounds ≥" value="${htmlEscape(f.rounds_gte)}">
        <input type="number" id="filter-rounds-lte" class="filter-number"
               data-testid="goals-rounds-lte-filter"
               min="0" step="1" inputmode="numeric"
               placeholder="Rounds ≤" value="${htmlEscape(f.rounds_lte)}">
        <select id="goals-severity" data-testid="goals-severity-filter">
          <option value="" ${f.severity === "" ? "selected" : ""}>all severities</option>
          <option value="info"  ${f.severity === "info"  ? "selected" : ""}>info</option>
          <option value="warn"  ${f.severity === "warn"  ? "selected" : ""}>warn</option>
          <option value="error" ${f.severity === "error" ? "selected" : ""}>error</option>
        </select>
        <select id="goals-category" data-testid="goals-category-filter"><option value="">all categories</option></select>
        <select id="goals-actor" data-testid="goals-actor-filter"><option value="">all actors</option></select>
        <select id="goals-limit" data-testid="goals-limit-filter">
          ${GOALS_LIMIT_OPTIONS.map((n) =>
            `<option value="${n}" ${n === f.limit ? "selected" : ""}>${n} entries</option>`).join("")}
        </select>
        <span class="spacer"></span>
        <button class="secondary" id="goals-clear" data-testid="goals-clear-filters">Clear filters</button>
      </div>
      <div class="filter-row filter-row-bulk">
        <span class="muted small">Selected Goals:</span>
        <button class="secondary small" id="goal-select-page" data-testid="goals-select-page">Select page</button>
        <button class="secondary small" id="bulk-export-jira" data-testid="goals-bulk-export-jira">Export for Jira</button>
        <button class="secondary small" id="bulk-set-status" data-testid="goals-bulk-status">Status…</button>
        <button class="secondary small" id="bulk-set-priority" data-testid="goals-bulk-priority">Priority…</button>
        <button class="secondary small" id="bulk-set-reporter" data-testid="goals-bulk-reporter">Reporter…</button>
        <button class="secondary small" id="bulk-set-assignee" data-testid="goals-bulk-assignee">Assignee…</button>
        <button class="secondary small" id="bulk-assign-feature" data-testid="goals-bulk-feature">Feature…</button>
        <button class="secondary small" id="bulk-transfer-node" data-testid="goals-bulk-transfer-node">Node…</button>
        <button class="secondary small" id="bulk-delete" data-testid="goals-bulk-delete">Delete…</button>
      </div>
    </div>
      </div>
    </details>
    <div id="goals-table" data-testid="goals-table"><p class="muted">Loading…</p></div>
  `;
  // In-view filter changes update the URL via replaceState (which does NOT
  // fire `hashchange`) and refresh only the table. Going through
  // `location.hash =` would trigger renderGoalsList again, which rebuilds
  // `#main` from scratch — that destroys the focused search input mid-
  // keystroke. Sort-header clicks go through the same path
  // (`refreshGoalsTable`); see drawGoalsTable.
  $("#search").addEventListener("input", debounce(() => {
    updateGoalsFilter({ q: $("#search").value, page: 1 });
  }, 250));
  $("#filter-status").addEventListener("change", (e) =>
    updateGoalsFilter({ status: e.target.value, page: 1 }));
  $("#filter-reporter").addEventListener("change", (e) =>
    updateGoalsFilter({ reporter: e.target.value, page: 1 }));
  $("#filter-assignee").addEventListener("change", (e) =>
    updateGoalsFilter({ assignee: e.target.value, page: 1 }));
  $("#filter-feature").addEventListener("input", debounce((e) =>
    updateGoalsFilter({ feature: e.target.value.trim(), page: 1 }), 250));
  $("#filter-node").addEventListener("change", (e) =>
    updateGoalsFilter({ node: e.target.value, page: 1 }));
  $("#filter-rounds-gte").addEventListener("input", debounce((e) =>
    updateGoalsFilter({ rounds_gte: e.target.value, page: 1 }), 250));
  $("#filter-rounds-lte").addEventListener("input", debounce((e) =>
    updateGoalsFilter({ rounds_lte: e.target.value, page: 1 }), 250));
  $("#goals-severity").addEventListener("change", (e) =>
    updateGoalsFilter({ severity: e.target.value, page: 1 }));
  $("#goals-category").addEventListener("change", (e) =>
    updateGoalsFilter({ category: e.target.value, page: 1 }));
  $("#goals-actor").addEventListener("change", (e) =>
    updateGoalsFilter({ actor: e.target.value, page: 1 }));
  $("#goals-limit").addEventListener("change", (e) =>
    updateGoalsFilter({
      limit: parseInt(e.target.value, 10) || GOALS_DEFAULT_LIMIT,
      page: 1,
    }));
  $("#goals-clear").addEventListener("click", () => {
    history.replaceState(null, "", "#/goals");
    renderGoalsList();
  });
  // The bulk-action buttons read the current filter from the hash at click
  // time, so they always reflect what the user can see.
  bindCommand("#bulk-export-jira", "goals.bulk.export_jira");
  bindCommand("#bulk-set-priority", "goals.bulk.priority");
  bindCommand("#bulk-set-status", "goals.bulk.status");
  bindCommand("#bulk-set-reporter", "goals.bulk.reporter");
  bindCommand("#bulk-set-assignee", "goals.bulk.assignee");
  bindCommand("#bulk-assign-feature", "goals.bulk.feature");
  bindCommand("#bulk-transfer-node", "goals.bulk.transfer_node");
  bindCommand("#bulk-delete", "goals.bulk.delete");
  bindCommand("#goal-select-page", "goals.select_page");

  // Expanding / collapsing the filter shell shows / hides the per-row
  // checkbox column. Redraw from the cached results so we don't re-fetch.
  $("#goals-filter-shell").addEventListener("toggle", () => {
    if (_lastGoalsRender) {
      drawGoalsTable(_lastGoalsRender.goals, _lastGoalsRender.state);
    }
  });

  await refreshGoalsTable();
  syncGoalsJiraExportOperation();
}

async function ensureGoalsNodeOptions() {
  try {
    const data = await api("GET", "/api/nodes");
    if (!Array.isArray(data?.nodes)) return;
    state.project = {
      ...(state.project || {}),
      nodes: data.nodes,
      active_node_id: data.active_node_id || state.project?.active_node_id,
      active_node: data.active_node || state.project?.active_node,
    };
  } catch (_) {
    // Keep rendering with the project-status nodes if the node registry is unavailable.
  }
}

// Snapshot the current Goals filter from the URL hash.
function goalsFilterFromHash() {
  const hashQs = new URLSearchParams(location.hash.split("?")[1] || "");
  const sort = (hashQs.get("sort") || "").toLowerCase();
  const dir = (hashQs.get("dir") || "").toLowerCase();
  const effectiveSort = sort || "updated";
  const effectiveDir = dir || (GOALS_DEFAULT_DIR[effectiveSort] || "desc");
  return {
    q: hashQs.get("q") || "",
    status: hashQs.get("status") || "",
    reporter: hashQs.get("reporter") || "",
    assignee: hashQs.get("assignee") || "",
    feature: hashQs.get("feature") || "",
    rounds_gte: hashQs.get("rounds_gte") || "",
    rounds_lte: hashQs.get("rounds_lte") || "",
    node: hashQs.get("node") || "",
    severity: hashQs.get("severity") || "",
    category: hashQs.get("category") || "",
    actor: hashQs.get("actor") || "",
    limit: parseInt(hashQs.get("limit") || String(GOALS_DEFAULT_LIMIT), 10)
           || GOALS_DEFAULT_LIMIT,
    page: Math.max(1, parseInt(hashQs.get("page") || "1", 10) || 1),
    sort, dir,
    effectiveSort, effectiveDir,
  };
}

// Patch one or more filter fields and refresh the table without
// triggering a full view re-render. The URL stays in sync via
// `history.replaceState` so reload / share / back behave correctly.
function updateGoalsFilter(patch) {
  const current = goalsFilterFromHash();
  const next = {
    q: "q" in patch ? patch.q : current.q,
    status: "status" in patch ? patch.status : current.status,
    reporter: "reporter" in patch ? patch.reporter : current.reporter,
    assignee: "assignee" in patch ? patch.assignee : current.assignee,
    feature: "feature" in patch ? patch.feature : current.feature,
    rounds_gte: "rounds_gte" in patch ? patch.rounds_gte : current.rounds_gte,
    rounds_lte: "rounds_lte" in patch ? patch.rounds_lte : current.rounds_lte,
    node: "node" in patch ? patch.node : current.node,
    severity: "severity" in patch ? patch.severity : current.severity,
    category: "category" in patch ? patch.category : current.category,
    actor: "actor" in patch ? patch.actor : current.actor,
    limit: "limit" in patch ? patch.limit : current.limit,
    page: "page" in patch ? patch.page : current.page,
    sort: "sort" in patch ? patch.sort : current.sort,
    dir: "dir" in patch ? patch.dir : current.dir,
  };
  history.replaceState(null, "", goalsHash(next));
  refreshGoalsTable();
}

function goalsWorkflowStatusHash(status, filter = goalsFilterFromHash()) {
  return goalsHash({
    q: filter.q,
    status,
    reporter: filter.reporter,
    assignee: filter.assignee,
    feature: filter.feature,
    rounds_gte: filter.rounds_gte,
    rounds_lte: filter.rounds_lte,
    node: filter.node,
    severity: filter.severity,
    category: filter.category,
    actor: filter.actor,
    limit: filter.limit,
    sort: filter.sort,
    dir: filter.dir,
    page: 1,
  });
}

function drawGoalsWorkflowVisualization(filter, counts) {
  const root = document.getElementById("goals-workflow");
  if (!root) return;
  root.innerHTML = renderWorkflowVisualization({
    counts,
    hrefForStatus: (status) => goalsWorkflowStatusHash(status, filter),
    className: "goals-workflow-grid",
  });
}

async function refreshGoalsTable() {
  if (state.currentRoute !== "goals") return;
  if (renderNoProjectIfDetached("Goals")) return;
  const f = goalsFilterFromHash();
  const params = new URLSearchParams();
  if (f.status) params.set("status", f.status);
  if (f.q) params.set("q", f.q);
  if (f.reporter) params.set("reporter", f.reporter);
  if (f.assignee) params.set("assignee", f.assignee);
  if (f.feature) params.set("feature", f.feature);
  if (f.rounds_gte) params.set("rounds_gte", f.rounds_gte);
  if (f.rounds_lte) params.set("rounds_lte", f.rounds_lte);
  if (f.node) params.set("node", f.node);
  if (f.severity) params.set("severity", f.severity);
  if (f.category) params.set("category", f.category);
  if (f.actor) params.set("actor", f.actor);
  if (f.limit) params.set("limit", String(f.limit));
  params.set("offset", String((f.page - 1) * f.limit));
  if (f.sort) params.set("sort", f.sort);
  if (f.dir) params.set("dir", f.dir);
  params.set("facets", "1");
  try {
    const data = await api("GET", "/api/goals?" + params);
    if (renderNoProjectIfApiDetached(data, "Goals")) return;
    const goals = data.goals || [];
    const facets = data.facets || {};
    drawGoalsWorkflowVisualization(f, facets.status_counts || {});
    // Refresh the category / actor dropdowns from the server-side
    // distinct values — same pattern as the Logs screen.
    const catSel = $("#goals-category");
    if (catSel) {
      const cats = facets.categories || [];
      catSel.innerHTML = `<option value="">all categories</option>` +
        cats.map((c) => `<option value="${htmlEscape(c)}" ${c === f.category ? "selected" : ""}>${htmlEscape(c)}</option>`).join("");
    }
    const actSel = $("#goals-actor");
    if (actSel) {
      const acts = facets.actors || [];
      actSel.innerHTML = `<option value="">all actors</option>` +
        acts.map((a) => `<option value="${htmlEscape(a)}" ${a === f.actor ? "selected" : ""}>${htmlEscape(a)}</option>`).join("");
    }
    const countEl = $("#goals-count");
    if (countEl) {
      countEl.textContent = `${goals.length} goal${goals.length === 1 ? "" : "s"}`;
    }
    applyGoalsFilterIndicator(f);
    const renderState = {
      q: f.q, status: f.status, feature: f.feature,
      assignee: f.assignee,
      rounds_gte: f.rounds_gte, rounds_lte: f.rounds_lte,
      sort: f.effectiveSort, dir: f.effectiveDir,
      page: data.page || {
        limit: f.limit,
        offset: (f.page - 1) * f.limit,
        has_more: false,
      },
    };
    _lastGoalsRender = { goals, state: renderState };
    drawGoalsTable(goals, renderState);
  } catch (e) {
    const tbl = $("#goals-table");
    if (tbl) tbl.innerHTML = `<p class="muted">${htmlEscape(e.message)}</p>`;
  }
}

// Bulk selection is filter-scoped. By default, bulk actions target every
// matching Goal across pagination, with checked row changes stored as explicit
// include/exclude exceptions. State survives filter tweaks and re-expanding
// the filter shell but resets on a hard navigation away from the Goals screen.
let goalsSelectAllMatching = true;
const goalsExcludedIds = new Set();
const goalsIncludedIds = new Set();

// Cached snapshot of the last refresh, so toggling the filter shell open
// or closed can redraw the table without re-fetching.
let _lastGoalsRender = null;

function resetGoalsSelection() {
  goalsSelectAllMatching = true;
  goalsExcludedIds.clear();
  goalsIncludedIds.clear();
}

function selectCurrentGoalsPage() {
  const goals = _lastGoalsRender?.goals || [];
  if (!goals.length) {
    toast("No Goals on this page.", "warn");
    return;
  }
  goalsSelectAllMatching = false;
  goalsExcludedIds.clear();
  goalsIncludedIds.clear();
  for (const goal of goals) goalsIncludedIds.add(goal.id);
  drawGoalsTable(goals, _lastGoalsRender.state);
}

function _isGoalSelected(id) {
  return goalsSelectAllMatching
    ? !goalsExcludedIds.has(id)
    : goalsIncludedIds.has(id);
}

function renderGoalFeatureCell(goal) {
  if (!goal.feature_id) return "—";
  const featureId = String(goal.feature_id);
  const featureLabel = featureId.length > 12 ? `${featureId.slice(0, 10)}…` : featureId;
  const order = goal.feature_order ? ` #${goal.feature_order}` : "";
  return `<a href="#/features/${encodeURIComponent(featureId)}" title="${htmlEscape(featureId)}">${htmlEscape(featureLabel)}${htmlEscape(order)}</a>`;
}

function drawGoalsTable(goals, state) {
  const root = $("#goals-table");
  // Selection UI follows the filter shell — only show checkboxes when the
  // shell is expanded (i.e. the user has indicated they want to interact
  // with bulk actions). Collapsed = focus on results.
  const shell = document.getElementById("goals-filter-shell");
  const showSelection = !!(shell && shell.open);

  if (!goals.length) {
    root.innerHTML = `
      <p class="muted">No goals match the current filters.</p>
      ${renderPaginationControls("goals", state.page, 0, "goal")}`;
    bindPaginationControls(root, "goals", (page) =>
      updateGoalsFilter({ page }));
    return;
  }
  const columns = [
    { key: "name",     label: "Name",     sortable: true },
    { key: "status",   label: "Status",   sortable: true },
    { key: "priority", label: "Priority", sortable: true },
    { key: "reporter", label: "Reporter", sortable: true },
    { key: "assignee", label: "Assignee", sortable: true },
    { key: "feature", label: "Feature", sortable: false },
    { key: "node", label: "Node", sortable: true },
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
                data-sort-key="${c.key}"
                data-testid="goals-sort-${c.key}">
              ${c.label} <span class="sort-arrow">${arrow}</span>
            </th>`;
  }).join("");
  const selectionHead = showSelection
    ? `<th class="goal-select-col">
         <input type="checkbox" id="goal-select-all"
                data-testid="goals-select-all"
                aria-label="Select all matching Goals">
       </th>`
    : "";
  root.innerHTML = `
    <div class="table-scroll">
      <table class="table work-items-table goals-table mobile-card-table">
        <colgroup>
          ${showSelection ? '<col class="goals-col-select">' : ""}
          <col class="work-item-name-col goals-col-name">
          <col class="goals-col-status">
          <col class="goals-col-priority">
          <col class="goals-col-reporter">
          <col class="goals-col-assignee">
          <col class="goals-col-feature">
          <col class="goals-col-node">
          <col class="goals-col-updated">
        </colgroup>
        <thead><tr>${selectionHead}${sortHeads}</tr></thead>
        <tbody>
          ${goals.map((g) => {
            const selected = _isGoalSelected(g.id);
            const cell = showSelection
              ? `<td class="goal-select-col" data-label="Select">
                   <input type="checkbox" class="goal-select"
                          data-testid="goals-row-select"
                          data-id="${g.id}"
                          ${selected ? "checked" : ""}
                          aria-label="Select goal ${htmlEscape(g.name)}">
                 </td>`
              : "";
            return `<tr data-id="${g.id}" data-testid="goals-row">
              ${cell}
              <td class="work-item-name-cell goals-name-cell" data-label="Name">${htmlEscape(g.name)}</td>
              <td class="goals-status-cell" data-label="Status"><span class="status-pill ${g.status}">${workflowStatusLabel(g.status)}</span></td>
              <td data-label="Priority"><span class="priority-pill priority-${g.priority || "low"}">${g.priority || "low"}</span></td>
              <td class="muted small" data-label="Reporter">${g.reporter ? htmlEscape(g.reporter) : "—"}</td>
              <td class="muted small" data-label="Assignee">${g.assignee ? htmlEscape(g.assignee) : "—"}</td>
              <td class="muted small" data-label="Feature">${renderGoalFeatureCell(g)}</td>
              <td class="muted small" data-label="Node">${htmlEscape(g.node_display_name || g.node_id || "Unknown")}</td>
              <td class="muted small" data-label="Updated">${fmtTime(g.updated)}</td>
            </tr>`;
          }).join("")}
        </tbody>
      </table>
    </div>
    ${renderPaginationControls("goals", state.page, goals.length, "goal")}
		  `;
  bindPaginationControls(root, "goals", (page) =>
    updateGoalsFilter({ page }));
  // Row click navigates to goal detail — but a click on the checkbox (or
  // its surrounding td) should toggle selection, not navigate.
  $$(".table tbody tr", root).forEach((row) => {
    row.addEventListener("click", (e) => {
      if (e.target.closest(".goal-select-col")) return;
      if (e.target.closest("a, button, input, select, textarea")) return;
      location.hash = "#/goals/" + row.dataset.id;
    });
  });
  $$(".goal-select", root).forEach((cb) => {
    cb.addEventListener("click", (e) => e.stopPropagation());
    cb.addEventListener("change", (e) => {
      const id = e.target.dataset.id;
      if (goalsSelectAllMatching) {
        if (e.target.checked) goalsExcludedIds.delete(id);
        else goalsExcludedIds.add(id);
      } else if (e.target.checked) {
        goalsIncludedIds.add(id);
      } else {
        goalsIncludedIds.delete(id);
      }
      _updateSelectAllState(goals);
    });
  });
  const selectAll = root.querySelector("#goal-select-all");
  if (selectAll) {
    _updateSelectAllState(goals);
    selectAll.addEventListener("click", (e) => {
      e.stopPropagation();
      const shouldCheck = selectAll.checked;
      goalsSelectAllMatching = shouldCheck;
      goalsExcludedIds.clear();
      goalsIncludedIds.clear();
      // Re-sync the current page checkboxes without a full redraw.
      $$(".goal-select", root).forEach((cb) => {
        cb.checked = shouldCheck;
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
        nextDir = GOALS_DEFAULT_DIR[key] || "desc";
      }
      updateGoalsFilter({ sort: key, dir: nextDir, page: 1 });
    });
  });
}

// Sync the header checkbox to the global filter-scoped selection:
// all matching selected -> checked, none selected -> unchecked, per-row
// exceptions -> indeterminate.
function _updateSelectAllState(goals) {
  const master = document.getElementById("goal-select-all");
  if (!master) return;
  if (!goals.length && !goalsIncludedIds.size) {
    master.checked = false;
    master.indeterminate = false;
  } else if (goalsSelectAllMatching && goalsExcludedIds.size === 0) {
    master.checked = true;
    master.indeterminate = false;
  } else if (!goalsSelectAllMatching && goalsIncludedIds.size === 0) {
    master.checked = false;
    master.indeterminate = false;
  } else {
    master.checked = false;
    master.indeterminate = true;
  }
}
