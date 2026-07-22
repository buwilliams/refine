// ---- Features ---------------------------------------------------------------

const FEATURES_DEFAULT_LIMIT = 50;
const FEATURES_LIMIT_OPTIONS = [50, 100, 250, 500, 1000];
const FEATURE_MODAL_GOAL_PAGE_SIZE = 25;
const FEATURES_DEFAULT_DIR = {
  name: "asc", status: "asc", reporter: "asc", assignee: "asc", node: "asc", updated: "desc",
};
const FEATURES_STATUS_OPTIONS = [
  "", "backlog", "todo", "in-progress", "qa", "ready-merge",
  "build", "review", "done", "failed", "cancelled",
];
const FEATURE_WORKFLOW_PROTECTED_STATUSES = new Set([
  "review", "done", "ready-merge", "build",
]);

let _featureModalRoot = null;
let featuresSelectAllMatching = true;
const featuresExcludedIds = new Set();
const featuresIncludedIds = new Set();
let _lastFeaturesRender = null;

function featuresHash(parts) {
  const next = new URLSearchParams();
  if (parts.q) next.set("q", parts.q);
  if (parts.status) next.set("status", parts.status);
  if (parts.reporter) next.set("reporter", parts.reporter);
  if (parts.assignee) next.set("assignee", parts.assignee);
  if (parts.node) next.set("node", parts.node);
  if (parts.limit && parts.limit !== FEATURES_DEFAULT_LIMIT) next.set("limit", String(parts.limit));
  if (parts.page && parts.page > 1) next.set("page", String(parts.page));
  if (parts.sort) next.set("sort", parts.sort);
  if (parts.dir) next.set("dir", parts.dir);
  return "#/features" + (next.toString() ? "?" + next : "");
}

function featuresFilterFromHash() {
  const hashQs = new URLSearchParams(location.hash.split("?")[1] || "");
  const sort = (hashQs.get("sort") || "").toLowerCase();
  const dir = (hashQs.get("dir") || "").toLowerCase();
  const effectiveSort = sort || "updated";
  const effectiveDir = dir || (FEATURES_DEFAULT_DIR[effectiveSort] || "desc");
  return {
    q: hashQs.get("q") || "",
    status: hashQs.get("status") || "",
    reporter: hashQs.get("reporter") || "",
    assignee: hashQs.get("assignee") || "",
    node: hashQs.get("node") || "",
    limit: parseInt(hashQs.get("limit") || String(FEATURES_DEFAULT_LIMIT), 10)
           || FEATURES_DEFAULT_LIMIT,
    page: Math.max(1, parseInt(hashQs.get("page") || "1", 10) || 1),
    sort, dir, effectiveSort, effectiveDir,
  };
}

async function renderFeaturesList() {
  if (renderNoProjectIfDetached("Features")) return;
  renderBanners([]);
  const f = featuresFilterFromHash();
  const filterShell = document.getElementById("features-filter-shell");
  const filterShellOpen = filterShell ? filterShell.open : false;
  $("#main").innerHTML = `
    <div class="page-title-row">
      <h2>Features</h2>
    </div>
    <details class="filter-shell" id="features-filter-shell" data-testid="features-filter-shell"${filterShellOpen ? " open" : ""}>
      <summary data-testid="features-filter-summary">
        <span class="filter-shell-title">Filters &amp; bulk actions</span>
        <span class="spacer"></span>
        <span class="muted small"><span id="features-count" data-testid="features-count"></span></span>
        <span id="features-filtered" class="filter-pill" data-testid="features-filtered-pill" hidden>Filtered</span>
      </summary>
      <div class="filter-shell-body">
        <div class="filter-bar">
          <div class="filter-row filter-row-primary">
            <input type="text" id="features-search" class="filter-grow"
                   data-testid="features-search"
                   placeholder="Search features..." value="${htmlEscape(f.q)}">
          </div>
          <div class="filter-row">
            <select id="features-status" data-testid="features-status-filter">
              ${FEATURES_STATUS_OPTIONS.map((s) =>
                `<option value="${s}" ${s === f.status ? "selected" : ""}>${s ? workflowStatusLabel(s) : "all statuses"}</option>`).join("")}
            </select>
            <select id="features-reporter" data-testid="features-reporter-filter">
              <option value="" ${f.reporter === "" ? "selected" : ""}>all reporters</option>
              ${(state.reporters || []).map((r) =>
                `<option value="${htmlEscape(r.name)}" ${r.name === f.reporter ? "selected" : ""}>${htmlEscape(r.name)}</option>`).join("")}
              ${f.reporter && !(state.reporters || []).some((r) => r.name === f.reporter)
                ? `<option value="${htmlEscape(f.reporter)}" selected>${htmlEscape(f.reporter)}</option>` : ""}
            </select>
            <select id="features-assignee" data-testid="features-assignee-filter">
              <option value="" ${f.assignee === "" ? "selected" : ""}>all assignees</option>
              ${(state.reporters || []).map((r) =>
                `<option value="${htmlEscape(r.name)}" ${r.name === f.assignee ? "selected" : ""}>${htmlEscape(r.name)}</option>`).join("")}
              ${f.assignee && !(state.reporters || []).some((r) => r.name === f.assignee)
                ? `<option value="${htmlEscape(f.assignee)}" selected>${htmlEscape(f.assignee)}</option>` : ""}
            </select>
            <select id="features-node" data-testid="features-node-filter">
              <option value="" ${f.node === "" ? "selected" : ""}>all nodes</option>
              <option value="current" ${f.node === "current" ? "selected" : ""}>current node</option>
              ${(state.project?.nodes || []).map((node) =>
                `<option value="${htmlEscape(node.id)}" ${node.id === f.node ? "selected" : ""}>${htmlEscape(node.display_name || node.id)}</option>`).join("")}
            </select>
            <select id="features-limit" data-testid="features-limit-filter">
              ${FEATURES_LIMIT_OPTIONS.map((n) =>
                `<option value="${n}" ${n === f.limit ? "selected" : ""}>${n} entries</option>`).join("")}
            </select>
            <span class="spacer"></span>
            <button class="secondary" id="features-clear" data-testid="features-clear-filters">Clear filters</button>
          </div>
          <div class="filter-row filter-row-bulk">
            <span class="muted small">Bulk update selected:</span>
            <button class="secondary small" id="features-select-page" data-testid="features-select-page">Select page</button>
            <button class="secondary small" id="features-bulk-reporter" data-testid="features-bulk-reporter">Reporter…</button>
            <button class="secondary small" id="features-bulk-assignee" data-testid="features-bulk-assignee">Assignee…</button>
            <button class="secondary small" id="features-bulk-transfer-node" data-testid="features-bulk-transfer-node">Node…</button>
            <button class="secondary small" id="features-bulk-delete" data-testid="features-bulk-delete">Delete…</button>
          </div>
        </div>
      </div>
    </details>
    <div id="features-table" data-testid="features-table"><p class="muted">Loading...</p></div>
  `;
  $("#features-search")?.addEventListener("input", debounce((e) =>
    updateFeaturesFilter({ q: e.target.value, page: 1 }), 250));
  $("#features-status")?.addEventListener("change", (e) =>
    updateFeaturesFilter({ status: e.target.value, page: 1 }));
  $("#features-reporter")?.addEventListener("change", (e) =>
    updateFeaturesFilter({ reporter: e.target.value, page: 1 }));
  $("#features-assignee")?.addEventListener("change", (e) =>
    updateFeaturesFilter({ assignee: e.target.value, page: 1 }));
  $("#features-node")?.addEventListener("change", (e) =>
    updateFeaturesFilter({ node: e.target.value, page: 1 }));
  $("#features-limit")?.addEventListener("change", (e) =>
    updateFeaturesFilter({ limit: parseInt(e.target.value, 10) || FEATURES_DEFAULT_LIMIT, page: 1 }));
  $("#features-clear")?.addEventListener("click", () => {
    history.replaceState(null, "", "#/features");
    renderFeaturesList();
  });
  bindCommand("#features-select-page", "features.select_page");
  bindCommand("#features-bulk-reporter", "features.bulk.reporter");
  bindCommand("#features-bulk-assignee", "features.bulk.assignee");
  bindCommand("#features-bulk-transfer-node", "features.bulk.transfer_node");
  bindCommand("#features-bulk-delete", "features.bulk.delete");
  $("#features-filter-shell").addEventListener("toggle", () => {
    if (_lastFeaturesRender) {
      drawFeaturesTable(_lastFeaturesRender.features, _lastFeaturesRender.state);
    }
  });
  await refreshFeaturesTable();
}

function updateFeaturesFilter(patch) {
  const current = featuresFilterFromHash();
  const next = {
    q: "q" in patch ? patch.q : current.q,
    status: "status" in patch ? patch.status : current.status,
    reporter: "reporter" in patch ? patch.reporter : current.reporter,
    assignee: "assignee" in patch ? patch.assignee : current.assignee,
    node: "node" in patch ? patch.node : current.node,
    limit: "limit" in patch ? patch.limit : current.limit,
    page: "page" in patch ? patch.page : current.page,
    sort: "sort" in patch ? patch.sort : current.sort,
    dir: "dir" in patch ? patch.dir : current.dir,
  };
  history.replaceState(null, "", featuresHash(next));
  refreshFeaturesTable();
}

async function refreshFeaturesTable() {
  if (state.currentRoute !== "features") return;
  const f = featuresFilterFromHash();
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries({
    q: f.q, status: f.status, reporter: f.reporter, assignee: f.assignee, node: f.node,
    limit: f.limit, offset: (f.page - 1) * f.limit,
    sort: f.sort, dir: f.dir,
  })) {
    if (value !== "" && value != null) params.set(key, String(value));
  }
  const data = await api("GET", `/api/features?${params}`);
  const renderState = { ...f, pageMeta: data.page || {} };
  _lastFeaturesRender = { features: data.features || [], state: renderState };
  drawFeaturesTable(_lastFeaturesRender.features, renderState);
}

function drawFeaturesTable(features, stateForRender) {
  const root = $("#features-table");
  const shell = document.getElementById("features-filter-shell");
  const showSelection = !!(shell && shell.open);
  const page = stateForRender.pageMeta || {};
  const total = page.total ?? ((page.offset || 0) + features.length + (page.has_more ? 1 : 0));
  $("#features-count").textContent = `${total} feature${total === 1 ? "" : "s"}`;
  $("#features-filtered").hidden = !(
    stateForRender.q || stateForRender.status || stateForRender.reporter || stateForRender.assignee || stateForRender.node
  );
  if (!features.length) {
    root.innerHTML = `
      <p class="muted">No Features match the current filters.</p>
      ${renderPaginationControls("features", page, 0, "feature")}`;
    bindPaginationControls(root, "features", (pageNo) =>
      updateFeaturesFilter({ page: pageNo }));
    return;
  }
  const rows = features.length ? features.map((entry) => {
    const feature = normalizeFeatureEntry(entry);
    const selected = _isFeatureSelected(feature.id);
    const cell = showSelection
      ? `<td class="feature-select-col" data-label="Select">
           <input type="checkbox" class="feature-select"
                  data-testid="features-row-select"
                  data-id="${htmlEscape(feature.id)}"
                  ${selected ? "checked" : ""}
                  aria-label="Select feature ${htmlEscape(feature.name || feature.id)}">
         </td>`
      : "";
    return `
    <tr data-feature-id="${htmlEscape(feature.id)}" data-testid="features-row">
      ${cell}
      <td class="work-item-name-cell features-name-cell" data-label="Name">${htmlEscape(feature.name || "Untitled Feature")}</td>
      <td class="features-status-cell" data-label="Status"><span class="status-pill ${htmlEscape(feature.status || "backlog")}">${workflowStatusLabel(feature.status || "backlog")}</span></td>
      <td data-label="Progress">${feature.done_count || 0} / ${feature.goal_count || 0} done</td>
      <td data-label="Next">${feature.next_goal ? htmlEscape(feature.next_goal.name || feature.next_goal.id) : '<span class="muted small">-</span>'}</td>
      <td class="muted small" data-label="Reporter">${htmlEscape(feature.reporter || "-")}</td>
      <td class="muted small" data-label="Assignee">${htmlEscape(feature.assignee || "-")}</td>
      <td class="muted small" data-label="Node">${htmlEscape(feature.node_display_name || feature.node_id || "-")}</td>
      <td class="muted small" data-label="Updated">${fmtTime(feature.updated)}</td>
    </tr>`;
  }).join("") : `
    <tr><td colspan="${showSelection ? 9 : 8}" class="muted">No Features match the current filters.</td></tr>`;
  const selectionHead = showSelection
    ? `<th class="feature-select-col">
         <input type="checkbox" id="feature-select-all"
                data-testid="features-select-all"
                aria-label="Select all matching Features">
       </th>`
    : "";
  root.innerHTML = `
    <div class="table-scroll">
      <table class="table work-items-table features-table mobile-card-table">
        <colgroup>
          ${showSelection ? '<col class="features-col-select">' : ""}
          <col class="work-item-name-col features-col-name">
          <col class="features-col-status">
          <col class="features-col-progress">
          <col class="features-col-next">
          <col class="features-col-reporter">
          <col class="features-col-assignee">
          <col class="features-col-node">
          <col class="features-col-updated">
        </colgroup>
        <thead><tr>
          ${selectionHead}
          ${featureSortHeader("name", "Name", stateForRender)}
          ${featureSortHeader("status", "Status", stateForRender)}
          <th>Progress</th>
          <th>Current / next Goal</th>
          ${featureSortHeader("reporter", "Reporter", stateForRender)}
          ${featureSortHeader("assignee", "Assignee", stateForRender)}
          ${featureSortHeader("node", "Node", stateForRender)}
          ${featureSortHeader("updated", "Updated", stateForRender)}
        </tr></thead>
        <tbody>${rows}</tbody>
      </table>
    </div>
    ${renderPaginationControls("features", page, features.length, "feature")}
  `;
  $$("#features-table [data-sort]").forEach((th) => {
    th.addEventListener("click", () => {
      const key = th.dataset.sort;
      const nextDir = stateForRender.effectiveSort === key && stateForRender.effectiveDir === "asc" ? "desc" : "asc";
      updateFeaturesFilter({ sort: key, dir: nextDir, page: 1 });
    });
  });
  $$("#features-table tbody tr[data-feature-id]").forEach((row) => {
    row.addEventListener("click", (e) => {
      if (e.target.closest(".feature-select-col")) return;
      if (e.target.closest("a, button, input, select, textarea")) return;
      location.hash = `#/features/${encodeURIComponent(row.dataset.featureId)}`;
    });
  });
  $$(".feature-select", root).forEach((cb) => {
    cb.addEventListener("click", (e) => e.stopPropagation());
    cb.addEventListener("change", (e) => {
      const id = e.target.dataset.id;
      if (featuresSelectAllMatching) {
        if (e.target.checked) featuresExcludedIds.delete(id);
        else featuresExcludedIds.add(id);
      } else if (e.target.checked) {
        featuresIncludedIds.add(id);
      } else {
        featuresIncludedIds.delete(id);
      }
      _updateFeatureSelectAllState(features.map((entry) => normalizeFeatureEntry(entry)));
    });
  });
  const selectAll = root.querySelector("#feature-select-all");
  if (selectAll) {
    const normalized = features.map((entry) => normalizeFeatureEntry(entry));
    _updateFeatureSelectAllState(normalized);
    selectAll.addEventListener("click", (e) => {
      e.stopPropagation();
      const shouldCheck = selectAll.checked;
      featuresSelectAllMatching = shouldCheck;
      featuresExcludedIds.clear();
      featuresIncludedIds.clear();
      $$(".feature-select", root).forEach((cb) => {
        cb.checked = shouldCheck;
      });
      selectAll.indeterminate = false;
    });
  }
  bindPaginationControls($("#features-table"), "features", (pageNo) =>
    updateFeaturesFilter({ page: pageNo }));
}

function normalizeFeatureEntry(entry) {
  const feature = { ...(entry?.feature || entry || {}) };
  const rollup = entry?.rollup || feature.rollup || {};
  feature.status = feature.status || rollup.status || "backlog";
  feature.goal_count = feature.goal_count ?? rollup.goal_count ?? (entry?.goal_ids || feature.goal_ids || []).length;
  feature.done_count = feature.done_count ?? rollup.done_count ?? 0;
  feature.active_count = feature.active_count ?? rollup.active_count ?? 0;
  feature.failed_count = feature.failed_count ?? rollup.failed_count ?? 0;
  feature.cancelled_count = feature.cancelled_count ?? rollup.cancelled_count ?? 0;
  feature.blocked_count = feature.blocked_count ?? rollup.blocked_count ?? 0;
  feature.next_goal = feature.next_goal || rollup.next_goal || null;
  feature.goal_ids = feature.goal_ids || entry?.goal_ids || [];
  feature.rollup = feature.rollup || rollup;
  return feature;
}

function featureSortHeader(key, label, stateForRender) {
  const active = stateForRender.effectiveSort === key;
  const dir = active ? stateForRender.effectiveDir : (FEATURES_DEFAULT_DIR[key] || "asc");
  const arrow = active
    ? (dir === "asc" ? "↑" : "↓")
    : `<span class="sort-arrow-placeholder">↕</span>`;
  return `<th class="sortable ${active ? "active" : ""}" data-sort="${key}" data-testid="features-sort-${htmlEscape(key)}">
    ${htmlEscape(label)} <span class="sort-arrow">${arrow}</span>
  </th>`;
}

function featureBulkFilterFromHash() {
  const f = featuresFilterFromHash();
  const filter = {};
  for (const key of ["status", "q", "reporter", "assignee", "node"]) {
    if (f[key]) filter[key] = f[key];
  }
  return filter;
}

function resetFeaturesSelection() {
  featuresSelectAllMatching = true;
  featuresExcludedIds.clear();
  featuresIncludedIds.clear();
}

function selectCurrentFeaturesPage() {
  const features = (_lastFeaturesRender?.features || []).map((entry) => normalizeFeatureEntry(entry));
  if (!features.length) {
    toast("No Features on this page.", "warn");
    return;
  }
  featuresSelectAllMatching = false;
  featuresExcludedIds.clear();
  featuresIncludedIds.clear();
  for (const feature of features) featuresIncludedIds.add(feature.id);
  drawFeaturesTable(_lastFeaturesRender.features, _lastFeaturesRender.state);
}

function _isFeatureSelected(id) {
  return featuresSelectAllMatching
    ? !featuresExcludedIds.has(id)
    : featuresIncludedIds.has(id);
}

function _featureSelectionRequestFields() {
  if (featuresSelectAllMatching) {
    return { exclude_ids: Array.from(featuresExcludedIds) };
  }
  return { selected_ids: Array.from(featuresIncludedIds) };
}

function _hasAnyFeatureSelection() {
  return featuresSelectAllMatching || featuresIncludedIds.size > 0;
}

function _featureSelectionCountText(noun = "selected") {
  if (featuresSelectAllMatching) {
    if (featuresExcludedIds.size) {
      return `all matching Features except ${featuresExcludedIds.size} excluded`;
    }
    return "all matching Features selected";
  }
  const selectedCount = featuresIncludedIds.size;
  const visibleIds = (_lastFeaturesRender?.features || [])
    .map((entry) => normalizeFeatureEntry(entry).id);
  const currentPageOnly = visibleIds.length > 0
    && visibleIds.length === selectedCount
    && visibleIds.every((id) => featuresIncludedIds.has(id));
  if (currentPageOnly) {
    return `${selectedCount} Features on this page ${noun}`;
  }
  return `${selectedCount} explicitly ${noun}`;
}

function _updateFeatureSelectAllState(features) {
  const master = document.getElementById("feature-select-all");
  if (!master) return;
  if (!features.length && !featuresIncludedIds.size) {
    master.checked = false;
    master.indeterminate = false;
  } else if (featuresSelectAllMatching && featuresExcludedIds.size === 0) {
    master.checked = true;
    master.indeterminate = false;
  } else if (!featuresSelectAllMatching && featuresIncludedIds.size === 0) {
    master.checked = false;
    master.indeterminate = false;
  } else {
    master.checked = false;
    master.indeterminate = true;
  }
}

async function openFeatureBulkModal(field) {
  const filter = featureBulkFilterFromHash();
  const filterDesc = describeFeatureFilter(filter);
  const selectionFields = _featureSelectionRequestFields();
  if (!_hasAnyFeatureSelection()) {
    toast("No Features selected.", "warn");
    return;
  }
  const countText = _featureSelectionCountText("selected");
  const label = { reporter: "Reporter", assignee: "Assignee" }[field];
  const opts = (state.reporters || [])
    .map((r) => `<option value="${htmlEscape(r.name)}">${htmlEscape(r.name)}</option>`)
    .join("");
  const body = () => `
    <div class="modal-title">Bulk set ${htmlEscape(label.toLowerCase())}</div>
    <div class="modal-body">
      <div class="muted small" style="margin-bottom:8px">
        Applies to ${htmlEscape(countText || "all matching")} —
        ${htmlEscape(filterDesc)}.
      </div>
      <label for="feature-bulk-${htmlEscape(field)}-value">New ${htmlEscape(label.toLowerCase())}</label>
      <select class="modal-input" id="feature-bulk-${htmlEscape(field)}-value" data-testid="feature-bulk-${htmlEscape(field)}-value" style="width:100%">
        <option value="">— pick ${htmlEscape(label.toLowerCase())} —</option>
        ${opts}
      </select>
    </div>
    <div class="modal-actions">
      <button class="secondary" data-cancel data-testid="feature-bulk-cancel">Cancel</button>
      <button data-ok data-testid="feature-bulk-apply">Apply</button>
    </div>`;
  const next = await _openModal(body, { cancel: null, ok: "" }, ".modal-input");
  if (next === null || !next) return;
  try {
    const r = await api("POST", "/api/features/bulk", {
      filter, ...selectionFields, update: { [field]: next },
    });
    toast(`Updated ${r.updated} feature${r.updated === 1 ? "" : "s"}`, "info");
    await refreshFeaturesTable();
  } catch (e) {
    await showActionError(e, "Feature bulk update failed");
  }
}

async function openFeatureBulkTransferNodeModal() {
  const filter = featureBulkFilterFromHash();
  const filterDesc = describeFeatureFilter(filter);
  const selectionFields = _featureSelectionRequestFields();
  if (!_hasAnyFeatureSelection()) {
    toast("No Features selected.", "warn");
    return;
  }
  const countText = _featureSelectionCountText("selected");

  let nodes = state.project?.nodes || [];
  try {
    const snap = await api("GET", "/api/nodes");
    nodes = snap.nodes || [];
    state.project = {
      ...(state.project || {}),
      nodes,
      active_node_id: snap.active_node_id || state.project?.active_node_id || "",
    };
  } catch {
  }
  const choices = nodes.filter((inst) => !inst.archived);
  if (!choices.length) {
    toast("No active nodes available.", "warn");
    return;
  }
  const opts = choices.map((inst) => `
    <option value="${htmlEscape(inst.id)}">${htmlEscape(inst.display_name || inst.id)}</option>
  `).join("");
  const body = () => `
    <div class="modal-title">Transfer to node</div>
    <div class="modal-body">
      <div class="muted small" style="margin-bottom:8px">
        Applies to ${htmlEscape(countText || "all matching")} —
        ${htmlEscape(filterDesc)}.
      </div>
      <label for="feature-bulk-transfer-node-value">Target node</label>
      <select class="modal-input" id="feature-bulk-transfer-node-value" data-testid="feature-bulk-transfer-node-value" style="width:100%">
        ${opts}
      </select>
      <p class="muted small" style="margin-top:6px">
        A Feature moves with its Goals. Features with active Goals are skipped.
      </p>
    </div>
    <div class="modal-actions">
      <button class="secondary" data-cancel data-testid="feature-bulk-transfer-cancel">Cancel</button>
      <button data-ok data-testid="feature-bulk-transfer-apply">Transfer</button>
    </div>`;
  const target = await _openModal(body, { cancel: null, ok: choices[0].id }, ".modal-input");
  if (target === null) return;
  try {
    const r = await api("POST", "/api/nodes/transfer-features", {
      filter, ...selectionFields, target_node_id: target,
    });
    toast(`Transferred ${r.updated}; skipped ${r.skipped}.`, "info");
    await refreshFeaturesTable();
  } catch (e) {
    await showActionError(e, "Feature transfer failed");
  }
}

async function confirmFeatureBulkDelete() {
  const filter = featureBulkFilterFromHash();
  const filterDesc = describeFeatureFilter(filter);
  const selectionFields = _featureSelectionRequestFields();
  if (!_hasAnyFeatureSelection()) {
    toast("No Features selected.", "warn");
    return;
  }
  const countText = _featureSelectionCountText("selected features");
  const ok = await modalConfirm(
    `Permanently delete ${countText} (${filterDesc})? This also deletes their assigned Goals and cannot be undone.`,
    {
      title: "Delete Features",
      okLabel: `Delete ${countText}`,
      cancelLabel: "Keep them",
      danger: true,
    },
  );
  if (!ok) return;
  try {
    const r = await api("POST", "/api/features/bulk/delete", {
      filter, ...selectionFields,
    });
    const failedN = (r.failures || []).length;
    if (failedN) {
      toast(`Deleted ${r.deleted} feature${r.deleted === 1 ? "" : "s"}, ${failedN} failed.`, "warn");
    } else {
      toast(`Deleted ${r.deleted} feature${r.deleted === 1 ? "" : "s"}.`, "info");
    }
    await refreshFeaturesTable();
  } catch (e) {
    await showActionError(e, "Feature bulk delete failed");
  }
}

function describeFeatureFilter(filter) {
  const parts = [];
  if (filter.status) parts.push(`status=${filter.status}`);
  if (filter.reporter) parts.push(`reporter=${filter.reporter}`);
  if (filter.assignee) parts.push(`assignee=${filter.assignee}`);
  if (filter.node && filter.node !== "all") parts.push(`node=${filter.node}`);
  if (filter.q) parts.push(`q="${filter.q}"`);
  return parts.length ? parts.join(", ") : "all features";
}

async function renderFeatureDetail(route) {
  if (renderNoProjectIfDetached("Features")) return;
  openFeatureDetailModal(route.id);
}

function renderFeatureGoalTable(goals, options = {}) {
  const actions = !!options.actions;
  const pageSize = Math.max(0, parseInt(options.pageSize || "0", 10) || 0);
  const pageNo = Math.max(1, parseInt(options.page || "1", 10) || 1);
  const start = pageSize ? (pageNo - 1) * pageSize : 0;
  const visible = pageSize ? goals.slice(start, start + pageSize) : goals;
  const pageMeta = pageSize ? {
    limit: pageSize,
    offset: start,
    has_more: start + visible.length < goals.length,
    total: goals.length,
  } : null;
  const rows = visible.length ? visible.map((goal, idx) => {
    const globalIdx = start + idx;
    const ordered = goal.feature_order !== null && goal.feature_order !== undefined;
    const previousOrdered = ordered ? findOrderedFeatureGoal(goals, globalIdx, -1) : null;
    const nextOrdered = ordered ? findOrderedFeatureGoal(goals, globalIdx, 1) : null;
    return `
    <tr data-feature-goal-row="${htmlEscape(goal.id)}" data-feature-goal-ordered="${ordered ? "1" : "0"}" data-testid="feature-goal-row">
      ${actions ? `<td class="feature-goal-drag-cell" data-label="Move">
        <button type="button" class="feature-goal-drag-handle" draggable="true"
                data-feature-drag-goal="${htmlEscape(goal.id)}"
                data-testid="feature-goal-drag"
                aria-label="Drag to reorder ${htmlEscape(goal.name || goal.id)}"
                title="Drag to reorder">
          <svg aria-hidden="true" viewBox="0 0 24 24" focusable="false">
            <circle cx="9" cy="5" r="1.5"></circle>
            <circle cx="15" cy="5" r="1.5"></circle>
            <circle cx="9" cy="12" r="1.5"></circle>
            <circle cx="15" cy="12" r="1.5"></circle>
            <circle cx="9" cy="19" r="1.5"></circle>
            <circle cx="15" cy="19" r="1.5"></circle>
          </svg>
        </button>
      </td>` : ""}
      <td data-label="Order" data-testid="feature-goal-order">${ordered ? htmlEscape(goal.feature_order) : '<span class="muted">Unordered</span>'}</td>
      <td data-label="Goal">
        <a href="#/goals/${encodeURIComponent(goal.id)}" data-testid="feature-goal-link">${htmlEscape(goal.name || goal.id)}</a>
        ${ordered && previousOrdered
          ? `<div class="muted small feature-goal-dependency" data-testid="feature-goal-dependency">After ${htmlEscape(previousOrdered.name || previousOrdered.id)}</div>`
          : ordered
            ? '<div class="muted small feature-goal-dependency" data-testid="feature-goal-dependency">First in sequence</div>'
            : '<div class="muted small feature-goal-dependency" data-testid="feature-goal-dependency">Independent</div>'}
      </td>
      <td data-label="Status"><span class="status-pill ${htmlEscape(goal.status || "backlog")}" data-testid="feature-goal-status">${workflowStatusLabel(goal.status || "backlog")}</span></td>
      <td data-label="Priority" data-testid="feature-goal-priority">${htmlEscape(goal.priority || "low")}</td>
      <td data-label="Reporter" data-testid="feature-goal-reporter">${htmlEscape(goal.reporter || "-")}</td>
      <td data-label="Assignee" data-testid="feature-goal-assignee">${htmlEscape(goal.assignee || "-")}</td>
      <td data-label="Updated" data-testid="feature-goal-updated">${fmtTime(goal.updated)}</td>
      ${actions ? `<td data-label="Actions">
        <div class="actions compact-actions">
          <button class="secondary small feature-goal-edit-btn" data-feature-edit-goal="${htmlEscape(goal.id)}"
                  data-testid="feature-goal-edit"
                  aria-label="Edit ${htmlEscape(goal.name || goal.id)} inline"
                  title="${featureGoalCanInlineEdit(goal) ? "Edit inline" : htmlEscape(goal.feature_authoring?.reason || "This Goal cannot be edited") }"
                  ${featureGoalCanInlineEdit(goal) ? "" : "disabled"}>Edit</button>
          <button class="secondary small feature-goal-icon-btn" data-feature-order-toggle="${ordered ? "unorder" : "order"}"
                  data-goal-id="${htmlEscape(goal.id)}"
                  data-testid="feature-goal-order-toggle"
                  aria-label="${ordered ? "Remove" : "Add"} ${htmlEscape(goal.name || goal.id)} ${ordered ? "from" : "to"} Feature order"
                  title="${ordered ? "Remove from order" : "Add to order"}">${featureGoalActionIcon(ordered ? "list-minus" : "list-plus")}</button>
          <button class="secondary small feature-goal-icon-btn" data-feature-move="up"
                  data-goal-id="${htmlEscape(goal.id)}"
                  data-neighbor-id="${htmlEscape(previousOrdered?.id || "")}"
                  data-testid="feature-goal-move-up"
                  aria-label="Move ${htmlEscape(goal.name || goal.id)} up"
                  title="Move up"
                  ${!previousOrdered ? "disabled" : ""}>${featureGoalActionIcon("chevron-up")}</button>
          <button class="secondary small feature-goal-icon-btn" data-feature-move="down"
                  data-goal-id="${htmlEscape(goal.id)}"
                  data-neighbor-id="${htmlEscape(nextOrdered?.id || "")}"
                  data-testid="feature-goal-move-down"
                  aria-label="Move ${htmlEscape(goal.name || goal.id)} down"
                  title="Move down"
                  ${!nextOrdered ? "disabled" : ""}>${featureGoalActionIcon("chevron-down")}</button>
          <button class="secondary small feature-goal-icon-btn" data-feature-delete-goal="${htmlEscape(goal.id)}"
                  data-testid="feature-goal-delete"
                  aria-label="Delete ${htmlEscape(goal.name || goal.id)}"
                  title="Delete Goal">${featureGoalActionIcon("trash")}</button>
        </div>
      </td>` : ""}
    </tr>`;
  }).join("") : `
    <tr><td colspan="${actions ? 9 : 7}" class="muted">No Goals are assigned to this Feature.</td></tr>`;
  return `
    <div class="table-scroll">
      <table class="table feature-goals-table mobile-card-table">
        <thead><tr>${actions ? '<th class="feature-goal-drag-col"></th>' : ""}<th>Order</th><th>Goal</th><th>Status</th><th>Priority</th><th>Reporter</th><th>Assignee</th><th>Updated</th>${actions ? "<th>Actions</th>" : ""}</tr></thead>
        <tbody>${rows}</tbody>
      </table>
    </div>
    ${pageMeta ? renderPaginationControls("feature-modal-goals", pageMeta, visible.length, "goal") : ""}`;
}

function findOrderedFeatureGoal(goals, fromIndex, direction) {
  for (let i = fromIndex + direction; i >= 0 && i < goals.length; i += direction) {
    const candidate = goals[i];
    if (candidate?.feature_order !== null && candidate?.feature_order !== undefined) return candidate;
  }
  return null;
}

function featureGoalActionIcon(name) {
  const icons = {
    "chevron-up": '<path d="M6 15l6-6 6 6"></path>',
    "chevron-down": '<path d="M6 9l6 6 6-6"></path>',
    "list-plus": '<path d="M8 6h10"></path><path d="M8 12h6"></path><path d="M8 18h6"></path><path d="M4 6h.01"></path><path d="M4 12h.01"></path><path d="M4 18h.01"></path><path d="M18 15v6"></path><path d="M15 18h6"></path>',
    "list-minus": '<path d="M8 6h10"></path><path d="M8 12h10"></path><path d="M8 18h6"></path><path d="M4 6h.01"></path><path d="M4 12h.01"></path><path d="M4 18h.01"></path><path d="M16 18h6"></path>',
    trash: '<path d="M3 6h18"></path><path d="M8 6V4h8v2"></path><path d="M6 6l1 15h10l1-15"></path><path d="M10 11v6"></path><path d="M14 11v6"></path>',
  };
  return `<svg aria-hidden="true" viewBox="0 0 24 24" focusable="false">${icons[name] || ""}</svg>`;
}

function featureWorkflowEligibleCount(feature, targetStatus) {
  return (feature?.goals || []).filter((goal) => {
    const status = goal.status || "";
    return status !== targetStatus && !FEATURE_WORKFLOW_PROTECTED_STATUSES.has(status);
  }).length;
}

function renderFeatureNew() {
  location.hash = "#/features";
  setTimeout(() => openFeatureModal(), 0);
}

function ensureFeatureModalUnderlay() {
  const main = $("#main");
  if (main && main.innerHTML.trim()) return;
  renderDashboard();
}

async function openFeatureDetailModal(featureId) {
  ensureFeatureModalUnderlay();
  if (typeof closeGoalDetailModal === "function") {
    closeGoalDetailModal({ navigateAway: false });
  }
  closeFeatureModal({ navigateAway: false });
  try {
    const data = await api("GET", `/api/features/${encodeURIComponent(featureId)}`);
    openFeatureModal(data.feature, { navigateAway: true });
  } catch (e) {
    const root = document.createElement("div");
    root.className = "modal-backdrop";
    root.innerHTML = `
      <div class="modal feature-modal" role="dialog" aria-modal="true" aria-label="Feature detail">
        <button class="modal-close" type="button" aria-label="Close">×</button>
        <div class="modal-body"><p class="muted">Could not load Feature: ${htmlEscape(e.message)}</p></div>
      </div>`;
    document.body.appendChild(root);
    _featureModalRoot = root;
    const dismiss = () => closeFeatureModal({ navigateAway: true });
    function onKey(evt) {
      if (evt.key === "Escape") { evt.preventDefault(); dismiss(); }
    }
    document.addEventListener("keydown", onKey, true);
    root._cleanup = () => document.removeEventListener("keydown", onKey, true);
    root.addEventListener("click", (evt) => {
      if (evt.target === root) dismiss();
    });
    root.querySelector(".modal-close")?.addEventListener("click", dismiss);
  }
}

function closeFeatureModal({ navigateAway = false } = {}) {
  if (!_featureModalRoot) return;
  _featureModalRoot._cleanup?.();
  _featureModalRoot.remove();
  _featureModalRoot = null;
  if (navigateAway) {
    const target = state.underlayHash || "#/features";
    if (location.hash !== target) location.hash = target;
    else state.currentRoute = parseHash().route;
  }
}

function openFeatureModal(feature = null, options = {}) {
  closeFeatureModal({ navigateAway: false });
  const root = document.createElement("div");
  root.className = "modal-backdrop";
  const goals = feature?.goals || [];
  const goalPage = Math.max(1, parseInt(options.goalPage || "1", 10) || 1);
  const navigateAway = !!options.navigateAway;
  const nodeDisplayName = feature
    ? (feature.node_display_name || feature.node_id || "Unknown")
    : "";
  const nodeOwnerTitle = feature?.node_id
    ? `Node owner: ${nodeDisplayName} (${feature.node_id})`
    : `Node owner: ${nodeDisplayName}`;
  const reporterOptions = (state.reporters || [])
    .map((r) => `<option value="${htmlEscape(r.name)}">${htmlEscape(r.name)}</option>`)
    .join("");
  const featureReporter = feature?.reporter || state.lastReporter || "";
  const missingFeatureReporter = featureReporter
    && !(state.reporters || []).some((r) => r.name === featureReporter)
    ? `<option value="${htmlEscape(featureReporter)}">${htmlEscape(featureReporter)}</option>`
    : "";
  const featureAssignee = feature?.assignee || state.lastReporter || "";
  const missingFeatureAssignee = featureAssignee
    && !(state.reporters || []).some((r) => r.name === featureAssignee)
    ? `<option value="${htmlEscape(featureAssignee)}">${htmlEscape(featureAssignee)}</option>`
    : "";
  root.innerHTML = `
    <div class="modal feature-modal ${feature ? "feature-detail-modal" : "feature-create-modal"}" role="dialog" aria-modal="true" aria-labelledby="feature-modal-title" data-testid="${feature ? "feature-detail-modal" : "feature-create-modal"}">
      <button class="modal-close" type="button" aria-label="Close" data-testid="feature-modal-close">×</button>
      ${feature ? `
      <div class="feature-modal-head">
        <div class="feature-modal-title-block">
          <div class="feature-modal-title-row">
            <div class="modal-title" id="feature-modal-title">Feature</div>
            <span class="status-pill ${htmlEscape(feature.status || "backlog")}" data-testid="feature-status-pill">${workflowStatusLabel(feature.status || "backlog")}</span>
            <span class="muted small" data-testid="feature-progress">${feature.done_count || 0} / ${feature.goal_count || 0} done</span>
          </div>
          <div class="feature-modal-meta muted small" data-testid="feature-metadata">
            ID <code>${htmlEscape(feature.id)}</code> · created ${fmtTime(feature.created)} · updated ${fmtTime(feature.updated)} · node <span title="${htmlEscape(nodeOwnerTitle)}">${htmlEscape(nodeDisplayName)}</span>
            · reporter <strong>${htmlEscape(feature.reporter || "unreported")}</strong>
            · assignee <strong>${htmlEscape(feature.assignee || "unassigned")}</strong>
          </div>
        </div>
        <div class="actions feature-modal-top-actions">
          <button type="button" class="small"
                  data-feature-workflow="backlog"
                  data-testid="feature-workflow-backlog"
                  ${featureWorkflowEligibleCount(feature, "backlog") ? "" : "disabled"}>&lt;- Backlog</button>
          <button type="button" class="small"
                  data-feature-workflow="todo"
                  data-testid="feature-workflow-todo"
                  ${featureWorkflowEligibleCount(feature, "todo") ? "" : "disabled"}>Todo -&gt;</button>
          <button type="button" class="secondary small" data-feature-cancel data-testid="feature-cancel">Cancel Feature</button>
          <button type="button" class="danger small" data-feature-delete data-testid="feature-delete">Delete Feature</button>
        </div>
      </div>` : `
      <div class="modal-title" id="feature-modal-title">New Feature</div>`}
      <div class="modal-body">
        <label>Name</label>
        <input type="text" id="feature-name" class="modal-input" data-testid="feature-name" value="${htmlEscape(feature?.name || "")}">
        <label>Description</label>
        <textarea id="feature-description" data-testid="feature-description">${htmlEscape(feature?.description || "")}</textarea>
        <label>Reporter</label>
        <select id="feature-reporter" class="modal-input" data-testid="feature-reporter">
          <option value="">— pick reporter —</option>
          ${missingFeatureReporter}
          ${reporterOptions}
        </select>
        <label>Assignee</label>
        <select id="feature-assignee" class="modal-input" data-testid="feature-assignee">
          <option value="">— pick assignee —</option>
          ${missingFeatureAssignee}
          ${reporterOptions}
        </select>
        ${feature ? `${renderFeatureGoalInlineComposer(goals, state.lastReporter || "")}
        <div class="feature-modal-goal-heading">
          <div class="modal-title compact">Feature Goals</div>
        </div>
        ${renderFeatureGoalTable(goals, {
          actions: true,
          page: goalPage,
          pageSize: FEATURE_MODAL_GOAL_PAGE_SIZE,
        })}` : ""}
      </div>
      ${feature ? "" : `<div class="modal-actions">
        <button class="secondary" data-cancel data-testid="feature-create-cancel">Cancel</button>
        <button data-ok data-testid="feature-create-submit">Create</button>
      </div>`}
    </div>`;
  document.body.appendChild(root);
  _featureModalRoot = root;
  const close = () => closeFeatureModal({ navigateAway });
  function onKey(e) {
    if (e.key === "Escape") {
      const composer = root.querySelector("[data-feature-goal-composer]");
      if (composer?.contains(e.target) && root._featureComposerHasDraft?.()) {
        e.preventDefault();
        root._featureComposerReset?.({ focus: true });
        return;
      }
      e.preventDefault();
      close();
    }
  }
  document.addEventListener("keydown", onKey, true);
  root._cleanup = () => document.removeEventListener("keydown", onKey, true);
  root.addEventListener("click", (e) => {
    if (e.target === root) close();
  });
  root.querySelector(".modal-close")?.addEventListener("click", close);
  if (feature) {
    bindFeatureAutosave(root, feature);
  } else {
    const reporterSelect = root.querySelector("#feature-reporter");
    if (reporterSelect) reporterSelect.value = featureReporter;
    const assigneeSelect = root.querySelector("#feature-assignee");
    if (assigneeSelect) assigneeSelect.value = featureAssignee;
    root.querySelector("[data-cancel]")?.addEventListener("click", close);
    root.querySelector("[data-ok]")?.addEventListener("click", async () => {
      const body = {
        name: root.querySelector("#feature-name")?.value.trim() || "",
        description: root.querySelector("#feature-description")?.value.trim() || "",
        reporter: root.querySelector("#feature-reporter")?.value.trim() || state.lastReporter || "",
        assignee: root.querySelector("#feature-assignee")?.value.trim() || state.lastReporter || "",
      };
      if (!body.name) {
        toast("Feature name is required", "error");
        return;
      }
      try {
        const saved = await api("POST", "/api/features", body);
        close();
        toast("Feature created", "success");
        location.hash = `#/features/${encodeURIComponent(saved.feature.id)}`;
      } catch (e) {
        showActionError(e);
      }
    });
  }
  if (feature) {
    const reporterSelect = root.querySelector("#feature-reporter");
    if (reporterSelect) reporterSelect.value = feature.reporter || "";
    const assigneeSelect = root.querySelector("#feature-assignee");
    if (assigneeSelect) assigneeSelect.value = feature.assignee || "";
    const reloadModal = async () => {
      const data = await api("GET", `/api/features/${encodeURIComponent(feature.id)}`);
      openFeatureModal(data.feature, { goalPage, navigateAway });
    };
    bindFeatureGoalInlineComposer(root, feature, { goalPage, navigateAway });
    bindFeatureGoalActions(root, feature.id, reloadModal);
    bindFeatureGoalDragReorder(root, feature.id, reloadModal);
    bindPaginationControls(root, "feature-modal-goals", (pageNo) => {
      openFeatureModal(feature, { goalPage: pageNo, navigateAway });
    });
    root.querySelector("[data-feature-cancel]")?.addEventListener("click", () =>
      cancelFeatureFromUi(feature.id));
    root.querySelector("[data-feature-delete]")?.addEventListener("click", () =>
      deleteFeatureFromUi(feature.id));
    root.querySelectorAll("[data-feature-workflow]").forEach((btn) => {
      btn.addEventListener("click", () =>
        moveFeatureWorkflowFromUi(feature.id, btn.dataset.featureWorkflow, {
          button: btn,
          reload: reloadModal,
        }));
    });
    if (options.focusComposer) {
      root.querySelector("[data-feature-goal-form] textarea[name='prompt']")?.focus();
    }
  }
  if (!options.focusComposer) root.querySelector("#feature-name")?.focus();
}

function bindFeatureAutosave(root, feature) {
  const controls = [
    root.querySelector("#feature-name"),
    root.querySelector("#feature-description"),
    root.querySelector("#feature-reporter"),
    root.querySelector("#feature-assignee"),
  ].filter(Boolean);
  const saved = {
    name: feature.name || "",
    description: feature.description || "",
    reporter: feature.reporter || "",
    assignee: feature.assignee || "",
  };
  let inFlight = false;
  let pending = false;
  const restoreSaved = () => {
    const name = root.querySelector("#feature-name");
    const description = root.querySelector("#feature-description");
    const reporter = root.querySelector("#feature-reporter");
    const assignee = root.querySelector("#feature-assignee");
    if (name) name.value = saved.name;
    if (description) description.value = saved.description;
    if (reporter) reporter.value = saved.reporter;
    if (assignee) assignee.value = saved.assignee;
  };
  const currentBody = () => ({
    name: root.querySelector("#feature-name")?.value.trim() || "",
    description: root.querySelector("#feature-description")?.value.trim() || "",
    reporter: root.querySelector("#feature-reporter")?.value.trim() || "",
    assignee: root.querySelector("#feature-assignee")?.value.trim() || "",
  });
  const currentDiffersFromSaved = () => {
    const body = currentBody();
    return body.name !== saved.name
      || body.description !== saved.description
      || body.reporter !== saved.reporter
      || body.assignee !== saved.assignee;
  };
  const save = async () => {
    if (inFlight) {
      pending = true;
      return;
    }
    const body = currentBody();
    if (!body.name) {
      toast("Feature name is required", "error");
      restoreSaved();
      return;
    }
    if (body.name === saved.name
        && body.description === saved.description
        && body.reporter === saved.reporter
        && body.assignee === saved.assignee) return;
    inFlight = true;
    try {
      const result = await api("PATCH", `/api/features/${encodeURIComponent(feature.id)}`, body);
      const updated = result.feature || {};
      saved.name = updated.name || body.name;
      saved.description = updated.description || body.description;
      saved.reporter = updated.reporter || body.reporter;
      saved.assignee = updated.assignee || body.assignee;
      if (state.currentRoute === "features") await refreshFeaturesTable();
    } catch (e) {
      restoreSaved();
      showActionError(e, "Feature autosave failed");
    } finally {
      inFlight = false;
      const shouldSaveAgain = pending || currentDiffersFromSaved();
      pending = false;
      if (shouldSaveAgain) {
        await save();
      }
    }
  };
  const autosave = debounce(save, 450);
  const scheduleAutosave = () => {
    if (inFlight) pending = true;
    autosave();
  };
  controls.forEach((control) => {
    control.addEventListener("input", scheduleAutosave);
    control.addEventListener("change", save);
  });
}

function bindFeatureGoalDragReorder(root, featureId, onChanged) {
  let draggedGoalId = "";
  root.querySelectorAll("[data-feature-drag-goal]").forEach((handle) => {
    handle.addEventListener("click", (e) => e.preventDefault());
    handle.addEventListener("dragstart", (e) => {
      draggedGoalId = handle.dataset.featureDragGoal || "";
      if (!draggedGoalId || handle.closest("[data-feature-goal-row]")?.dataset.featureGoalOrdered !== "1") {
        e.preventDefault();
        return;
      }
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", draggedGoalId);
      handle.closest("[data-feature-goal-row]")?.classList.add("dragging");
    });
    handle.addEventListener("dragend", () => {
      draggedGoalId = "";
      clearFeatureGoalDragState(root);
    });
  });
  root.querySelectorAll("[data-feature-goal-row]").forEach((row) => {
    row.addEventListener("dragover", (e) => {
      if (!draggedGoalId) return;
      const targetGoalId = row.dataset.featureGoalRow || "";
      if (!targetGoalId || targetGoalId === draggedGoalId || row.dataset.featureGoalOrdered !== "1") return;
      e.preventDefault();
      e.dataTransfer.dropEffect = "move";
      const rect = row.getBoundingClientRect();
      const position = e.clientY < rect.top + rect.height / 2 ? "before" : "after";
      root.querySelectorAll("[data-feature-goal-row]").forEach((candidate) => {
        candidate.classList.remove("drop-before", "drop-after");
      });
      row.classList.add(position === "before" ? "drop-before" : "drop-after");
      row.dataset.featureDropPosition = position;
    });
    row.addEventListener("dragleave", () => {
      row.classList.remove("drop-before", "drop-after");
      delete row.dataset.featureDropPosition;
    });
    row.addEventListener("drop", async (e) => {
      const sourceGoalId = e.dataTransfer.getData("text/plain") || draggedGoalId;
      const targetGoalId = row.dataset.featureGoalRow || "";
      const position = row.dataset.featureDropPosition || "after";
      clearFeatureGoalDragState(root);
      if (!sourceGoalId || !targetGoalId || sourceGoalId === targetGoalId) return;
      e.preventDefault();
      try {
        await api("POST", `/api/features/${encodeURIComponent(featureId)}/goals/${encodeURIComponent(sourceGoalId)}/reorder`, {
          [position]: targetGoalId,
        });
        toast("Feature order updated", "info");
        await onChanged?.();
      } catch (err) {
        showActionError(err, "Reorder failed");
      }
    });
  });
}

function clearFeatureGoalDragState(root) {
  root.querySelectorAll("[data-feature-goal-row]").forEach((row) => {
    row.classList.remove("dragging", "drop-before", "drop-after");
    delete row.dataset.featureDropPosition;
  });
}

function bindFeatureGoalActions(root, featureId, onChanged) {
  root.querySelectorAll("[data-feature-move]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const goalId = btn.dataset.goalId;
      const siblingId = btn.dataset.neighborId;
      if (!goalId || !siblingId) return;
      const body = btn.dataset.featureMove === "up"
        ? { before: siblingId }
        : { after: siblingId };
      try {
        await api("POST", `/api/features/${encodeURIComponent(featureId)}/goals/${encodeURIComponent(goalId)}/reorder`, body);
        toast("Feature order updated", "info");
        await onChanged?.();
      } catch (e) {
        showActionError(e, "Reorder failed");
      }
    });
  });
  root.querySelectorAll("[data-feature-order-toggle]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const goalId = btn.dataset.goalId;
      const action = btn.dataset.featureOrderToggle;
      if (!goalId || !["order", "unorder"].includes(action)) return;
      try {
        await api("POST", `/api/features/${encodeURIComponent(featureId)}/goals/${encodeURIComponent(goalId)}/${action}`);
        toast(action === "order" ? "Goal added to Feature order" : "Goal removed from Feature order", "info");
        await onChanged?.();
      } catch (e) {
        showActionError(e, "Feature order update failed");
      }
    });
  });
  root.querySelectorAll("[data-feature-delete-goal]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const goalId = btn.dataset.featureDeleteGoal;
      if (!goalId) return;
      const ok = await modalConfirm(
        "Delete this Goal from the Feature? This cannot be undone.",
        { title: "Delete Goal", okLabel: "Delete Goal", cancelLabel: "Keep it", danger: true },
      );
      if (!ok) return;
      try {
        await api("DELETE", `/api/goals/${encodeURIComponent(goalId)}`);
        toast("Goal deleted", "info");
        await onChanged?.();
      } catch (e) {
        showActionError(e, "Delete failed");
      }
    });
  });
}

async function cancelFeatureFromUi(featureId) {
  const ok = await modalConfirm(
    "Cancel this Feature? Completed Goals stay done and every non-terminal Goal in the Feature will be cancelled.",
    { title: "Cancel Feature", okLabel: "Cancel Feature", cancelLabel: "Keep working", danger: true },
  );
  if (!ok) return;
  try {
    await api("POST", `/api/features/${encodeURIComponent(featureId)}/cancel`);
    toast("Feature cancelled", "info");
    if (state.currentRoute === "features_detail") {
      await openFeatureDetailModal(featureId);
    } else if (state.currentRoute === "features") {
      await refreshFeaturesTable();
    }
  } catch (e) {
    showActionError(e, "Cancel Feature failed");
  }
}

async function moveFeatureWorkflowFromUi(featureId, targetStatus, { button = null, reload = null } = {}) {
  const target = String(targetStatus || "").trim();
  if (!["backlog", "todo"].includes(target)) return;
  const label = target === "backlog" ? "backlog" : "todo";
  const busy = target === "backlog" ? "Moving to backlog…" : "Moving to todo…";
  await withButtonBusy(button, busy, async () => {
    try {
      const result = await api("POST", `/api/features/${encodeURIComponent(featureId)}/workflow`, {
        status: target,
      });
      const updated = result.updated || 0;
      const skipped = result.skipped || 0;
      const stopped = result.stopped || 0;
      const stopText = stopped ? `; stopped ${stopped}` : "";
      toast(`Moved ${updated} Goal${updated === 1 ? "" : "s"} to ${label}${stopText}${skipped ? `; skipped ${skipped}` : ""}`, "info");
      if (typeof reload === "function") {
        await reload();
      } else if (state.currentRoute === "features_detail") {
        await openFeatureDetailModal(featureId);
      } else if (state.currentRoute === "features") {
        await refreshFeaturesTable();
      }
    } catch (e) {
      await showActionError(e, "Feature workflow action failed");
    }
  });
}

async function deleteFeatureFromUi(featureId) {
  const ok = await modalConfirm(
    "Delete this Feature and all Goals in it? This cannot be undone.",
    { title: "Delete Feature", okLabel: "Delete Feature", cancelLabel: "Keep it", danger: true },
  );
  if (!ok) return;
  try {
    await api("DELETE", `/api/features/${encodeURIComponent(featureId)}`);
    toast("Feature deleted", "info");
    closeFeatureModal({ navigateAway: false });
    location.hash = "#/features";
  } catch (e) {
    showActionError(e, "Delete Feature failed");
  }
}
