// ---- Features ---------------------------------------------------------------

const FEATURES_DEFAULT_LIMIT = 50;
const FEATURES_LIMIT_OPTIONS = [50, 100, 250, 500, 1000];
const FEATURE_MODAL_GAP_PAGE_SIZE = 25;
const FEATURES_DEFAULT_DIR = {
  name: "asc", status: "asc", reporter: "asc", node: "asc", updated: "desc",
};
const FEATURES_STATUS_OPTIONS = [
  "", "backlog", "todo", "in-progress", "qa", "ready-merge",
  "awaiting-rebuild", "review", "done", "failed", "cancelled",
];

let _featureModalRoot = null;

function featuresHash(parts) {
  const next = new URLSearchParams();
  if (parts.q) next.set("q", parts.q);
  if (parts.status) next.set("status", parts.status);
  if (parts.reporter) next.set("reporter", parts.reporter);
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
  $("#main").innerHTML = `
    <div class="page-title-row">
      <h2>Features</h2>
    </div>
    <details class="filter-shell" id="features-filter-shell">
      <summary>
        <span class="filter-shell-title">Filters</span>
        <span class="spacer"></span>
        <span class="muted small"><span id="features-count"></span></span>
        <span id="features-filtered" class="filter-pill" hidden>Filtered</span>
      </summary>
      <div class="filter-shell-body">
        <div class="filter-bar">
          <div class="filter-row filter-row-primary">
            <input type="text" id="features-search" class="filter-grow"
                   placeholder="Search features..." value="${htmlEscape(f.q)}">
          </div>
          <div class="filter-row">
            <select id="features-status">
              ${FEATURES_STATUS_OPTIONS.map((s) =>
                `<option value="${s}" ${s === f.status ? "selected" : ""}>${s ? workflowStatusLabel(s) : "all statuses"}</option>`).join("")}
            </select>
            <select id="features-reporter">
              <option value="" ${f.reporter === "" ? "selected" : ""}>all reporters</option>
              ${(state.reporters || []).map((r) =>
                `<option value="${htmlEscape(r.name)}" ${r.name === f.reporter ? "selected" : ""}>${htmlEscape(r.name)}</option>`).join("")}
              ${f.reporter && !(state.reporters || []).some((r) => r.name === f.reporter)
                ? `<option value="${htmlEscape(f.reporter)}" selected>${htmlEscape(f.reporter)}</option>` : ""}
            </select>
            <select id="features-node">
              <option value="" ${f.node === "" ? "selected" : ""}>all nodes</option>
              <option value="current" ${f.node === "current" ? "selected" : ""}>current node</option>
              ${(state.project?.nodes || []).map((node) =>
                `<option value="${htmlEscape(node.id)}" ${node.id === f.node ? "selected" : ""}>${htmlEscape(node.display_name || node.id)}</option>`).join("")}
            </select>
            <select id="features-limit">
              ${FEATURES_LIMIT_OPTIONS.map((n) =>
                `<option value="${n}" ${n === f.limit ? "selected" : ""}>${n} entries</option>`).join("")}
            </select>
            <span class="spacer"></span>
            <button class="secondary" id="features-clear">Clear filters</button>
          </div>
        </div>
      </div>
    </details>
    <div id="features-table"><p class="muted">Loading...</p></div>
  `;
  $("#features-search")?.addEventListener("input", debounce((e) =>
    updateFeaturesFilter({ q: e.target.value, page: 1 }), 250));
  $("#features-status")?.addEventListener("change", (e) =>
    updateFeaturesFilter({ status: e.target.value, page: 1 }));
  $("#features-reporter")?.addEventListener("change", (e) =>
    updateFeaturesFilter({ reporter: e.target.value, page: 1 }));
  $("#features-node")?.addEventListener("change", (e) =>
    updateFeaturesFilter({ node: e.target.value, page: 1 }));
  $("#features-limit")?.addEventListener("change", (e) =>
    updateFeaturesFilter({ limit: parseInt(e.target.value, 10) || FEATURES_DEFAULT_LIMIT, page: 1 }));
  $("#features-clear")?.addEventListener("click", () => {
    history.replaceState(null, "", "#/features");
    renderFeaturesList();
  });
  await refreshFeaturesTable();
}

function updateFeaturesFilter(patch) {
  const current = featuresFilterFromHash();
  const next = {
    q: "q" in patch ? patch.q : current.q,
    status: "status" in patch ? patch.status : current.status,
    reporter: "reporter" in patch ? patch.reporter : current.reporter,
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
    q: f.q, status: f.status, reporter: f.reporter, node: f.node,
    limit: f.limit, offset: (f.page - 1) * f.limit,
    sort: f.sort, dir: f.dir,
  })) {
    if (value !== "" && value != null) params.set(key, String(value));
  }
  const data = await api("GET", `/api/features?${params}`);
  drawFeaturesTable(data.features || [], { ...f, pageMeta: data.page || {} });
}

function drawFeaturesTable(features, stateForRender) {
  const root = $("#features-table");
  const page = stateForRender.pageMeta || {};
  const total = page.total ?? ((page.offset || 0) + features.length + (page.has_more ? 1 : 0));
  $("#features-count").textContent = `${total} feature${total === 1 ? "" : "s"}`;
  $("#features-filtered").hidden = !(
    stateForRender.q || stateForRender.status || stateForRender.reporter || stateForRender.node
  );
  if (!features.length) {
    root.innerHTML = `
      <p class="muted">No Features match the current filters.</p>
      ${renderPaginationControls("features", page, 0, "feature")}`;
    bindPaginationControls(root, "features", (pageNo) =>
      updateFeaturesFilter({ page: pageNo }));
    return;
  }
  const rows = features.length ? features.map((feature) => `
    <tr data-feature-id="${htmlEscape(feature.id)}">
      <td class="features-name-cell" data-label="Name">${htmlEscape(feature.name || "Untitled Feature")}</td>
      <td class="features-status-cell" data-label="Status"><span class="status-pill ${htmlEscape(feature.status || "backlog")}">${workflowStatusLabel(feature.status || "backlog")}</span></td>
      <td data-label="Progress">${feature.done_count || 0} / ${feature.gap_count || 0} done</td>
      <td data-label="Next">${feature.next_gap ? htmlEscape(feature.next_gap.name || feature.next_gap.id) : '<span class="muted small">-</span>'}</td>
      <td class="muted small" data-label="Reporter">${htmlEscape(feature.reporter || "-")}</td>
      <td class="muted small" data-label="Node">${htmlEscape(feature.node_display_name || feature.node_id || "-")}</td>
      <td class="muted small" data-label="Updated">${fmtTime(feature.updated)}</td>
    </tr>`).join("") : `
    <tr><td colspan="7" class="muted">No Features match the current filters.</td></tr>`;
  root.innerHTML = `
    <div class="table-scroll">
      <table class="table features-table mobile-card-table">
        <colgroup>
          <col class="features-col-name">
          <col class="features-col-status">
          <col class="features-col-progress">
          <col class="features-col-next">
          <col class="features-col-reporter">
          <col class="features-col-node">
          <col class="features-col-updated">
        </colgroup>
        <thead><tr>
          ${featureSortHeader("name", "Name", stateForRender)}
          ${featureSortHeader("status", "Status", stateForRender)}
          <th>Progress</th>
          <th>Current / next Gap</th>
          ${featureSortHeader("reporter", "Reporter", stateForRender)}
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
      if (e.target.closest("a, button, input, select, textarea")) return;
      location.hash = `#/features/${encodeURIComponent(row.dataset.featureId)}`;
    });
  });
  bindPaginationControls($("#features-table"), "features", (pageNo) =>
    updateFeaturesFilter({ page: pageNo }));
}

function featureSortHeader(key, label, stateForRender) {
  const active = stateForRender.effectiveSort === key;
  const dir = active ? stateForRender.effectiveDir : (FEATURES_DEFAULT_DIR[key] || "asc");
  const arrow = active
    ? (dir === "asc" ? "↑" : "↓")
    : `<span class="sort-arrow-placeholder">↕</span>`;
  return `<th class="sortable ${active ? "active" : ""}" data-sort="${key}">
    ${htmlEscape(label)} <span class="sort-arrow">${arrow}</span>
  </th>`;
}

async function renderFeatureDetail(route) {
  if (renderNoProjectIfDetached("Features")) return;
  openFeatureDetailModal(route.id);
}

function renderFeatureGapTable(gaps, options = {}) {
  const actions = !!options.actions;
  const pageSize = Math.max(0, parseInt(options.pageSize || "0", 10) || 0);
  const pageNo = Math.max(1, parseInt(options.page || "1", 10) || 1);
  const start = pageSize ? (pageNo - 1) * pageSize : 0;
  const visible = pageSize ? gaps.slice(start, start + pageSize) : gaps;
  const pageMeta = pageSize ? {
    limit: pageSize,
    offset: start,
    has_more: start + visible.length < gaps.length,
    total: gaps.length,
  } : null;
  const rows = visible.length ? visible.map((gap, idx) => {
    const globalIdx = start + idx;
    return `
    <tr data-feature-gap-row="${htmlEscape(gap.id)}">
      ${actions ? `<td class="feature-gap-drag-cell" data-label="Move">
        <button type="button" class="feature-gap-drag-handle" draggable="true"
                data-feature-drag-gap="${htmlEscape(gap.id)}"
                aria-label="Drag to reorder ${htmlEscape(gap.name || gap.id)}"
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
      <td data-label="Order">${gap.feature_order || ""}</td>
      <td data-label="Gap"><a href="#/gaps/${encodeURIComponent(gap.id)}">${htmlEscape(gap.name || gap.id)}</a></td>
      <td data-label="Status"><span class="status-pill ${htmlEscape(gap.status || "backlog")}">${workflowStatusLabel(gap.status || "backlog")}</span></td>
      <td data-label="Priority">${htmlEscape(gap.priority || "low")}</td>
      <td data-label="Reporter">${htmlEscape(gap.reporter || "-")}</td>
      <td data-label="Updated">${fmtTime(gap.updated)}</td>
      ${actions ? `<td data-label="Actions">
        <div class="actions compact-actions">
          <button class="secondary small feature-gap-icon-btn" data-feature-move="up"
                  data-gap-id="${htmlEscape(gap.id)}"
                  data-neighbor-id="${htmlEscape(gaps[globalIdx - 1]?.id || "")}"
                  aria-label="Move ${htmlEscape(gap.name || gap.id)} up"
                  title="Move up"
                  ${globalIdx === 0 ? "disabled" : ""}>${featureGapActionIcon("chevron-up")}</button>
          <button class="secondary small feature-gap-icon-btn" data-feature-move="down"
                  data-gap-id="${htmlEscape(gap.id)}"
                  data-neighbor-id="${htmlEscape(gaps[globalIdx + 1]?.id || "")}"
                  aria-label="Move ${htmlEscape(gap.name || gap.id)} down"
                  title="Move down"
                  ${globalIdx === gaps.length - 1 ? "disabled" : ""}>${featureGapActionIcon("chevron-down")}</button>
          <button class="secondary small feature-gap-icon-btn" data-feature-delete-gap="${htmlEscape(gap.id)}"
                  aria-label="Delete ${htmlEscape(gap.name || gap.id)}"
                  title="Delete Gap">${featureGapActionIcon("trash")}</button>
        </div>
      </td>` : ""}
    </tr>`;
  }).join("") : `
    <tr><td colspan="${actions ? 8 : 6}" class="muted">No Gaps are assigned to this Feature.</td></tr>`;
  return `
    <div class="table-scroll">
      <table class="table feature-gaps-table mobile-card-table">
        <thead><tr>${actions ? '<th class="feature-gap-drag-col"></th>' : ""}<th>Order</th><th>Gap</th><th>Status</th><th>Priority</th><th>Reporter</th><th>Updated</th>${actions ? "<th>Actions</th>" : ""}</tr></thead>
        <tbody>${rows}</tbody>
      </table>
    </div>
    ${pageMeta ? renderPaginationControls("feature-modal-gaps", pageMeta, visible.length, "gap") : ""}`;
}

function featureGapActionIcon(name) {
  const icons = {
    "chevron-up": '<path d="M6 15l6-6 6 6"></path>',
    "chevron-down": '<path d="M6 9l6 6 6-6"></path>',
    trash: '<path d="M3 6h18"></path><path d="M8 6V4h8v2"></path><path d="M6 6l1 15h10l1-15"></path><path d="M10 11v6"></path><path d="M14 11v6"></path>',
  };
  return `<svg aria-hidden="true" viewBox="0 0 24 24" focusable="false">${icons[name] || ""}</svg>`;
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
  if (typeof closeGapDetailModal === "function") {
    closeGapDetailModal({ navigateAway: false });
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
  const gaps = feature?.gaps || [];
  const gapPage = Math.max(1, parseInt(options.gapPage || "1", 10) || 1);
  const navigateAway = !!options.navigateAway;
  const nodeDisplayName = feature
    ? (feature.node_display_name || feature.node_id || "Unknown")
    : "";
  const nodeOwnerTitle = feature?.node_id
    ? `Node owner: ${nodeDisplayName} (${feature.node_id})`
    : `Node owner: ${nodeDisplayName}`;
  root.innerHTML = `
    <div class="modal feature-modal ${feature ? "feature-detail-modal" : "feature-create-modal"}" role="dialog" aria-modal="true" aria-labelledby="feature-modal-title">
      <button class="modal-close" type="button" aria-label="Close">×</button>
      ${feature ? `
      <div class="feature-modal-head">
        <div class="feature-modal-title-block">
          <div class="feature-modal-title-row">
            <div class="modal-title" id="feature-modal-title">Feature</div>
            <span class="status-pill ${htmlEscape(feature.status || "backlog")}">${workflowStatusLabel(feature.status || "backlog")}</span>
            <span class="muted small">${feature.done_count || 0} / ${feature.gap_count || 0} done</span>
          </div>
          <div class="feature-modal-meta muted small">
            ID <code>${htmlEscape(feature.id)}</code> · created ${fmtTime(feature.created)} · updated ${fmtTime(feature.updated)} · node <span title="${htmlEscape(nodeOwnerTitle)}">${htmlEscape(nodeDisplayName)}</span>
          </div>
        </div>
        <div class="actions feature-modal-top-actions">
          <button type="button" class="secondary small" data-feature-cancel>Cancel Feature</button>
          <button type="button" class="danger small" data-feature-delete>Delete Feature</button>
        </div>
      </div>` : `
      <div class="modal-title" id="feature-modal-title">New Feature</div>`}
      <div class="modal-body">
        <label>Name</label>
        <input type="text" id="feature-name" class="modal-input" value="${htmlEscape(feature?.name || "")}">
        <label>Description</label>
        <textarea id="feature-description">${htmlEscape(feature?.description || "")}</textarea>
        ${feature ? `<div class="feature-modal-gap-heading">
          <div class="modal-title compact">Ordered Gaps</div>
          <div class="actions feature-gap-heading-actions">
            <button type="button" class="secondary small feature-gap-add-btn"
                    data-feature-new-gap aria-label="New Gap" title="New Gap">+</button>
            <button type="button" class="secondary small" data-feature-assign-gap>Assign existing</button>
          </div>
        </div>
        ${renderFeatureGapTable(gaps, {
          actions: true,
          page: gapPage,
          pageSize: FEATURE_MODAL_GAP_PAGE_SIZE,
        })}` : ""}
      </div>
      ${feature ? "" : `<div class="modal-actions">
        <button class="secondary" data-cancel>Cancel</button>
        <button data-ok>Create</button>
      </div>`}
    </div>`;
  document.body.appendChild(root);
  _featureModalRoot = root;
  const close = () => closeFeatureModal({ navigateAway });
  function onKey(e) {
    if (e.key === "Escape") { e.preventDefault(); close(); }
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
    root.querySelector("[data-cancel]")?.addEventListener("click", close);
    root.querySelector("[data-ok]")?.addEventListener("click", async () => {
      const body = {
        name: root.querySelector("#feature-name")?.value.trim() || "",
        description: root.querySelector("#feature-description")?.value.trim() || "",
        reporter: state.lastReporter || "",
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
    const reloadModal = async () => {
      const data = await api("GET", `/api/features/${encodeURIComponent(feature.id)}`);
      openFeatureModal(data.feature, { gapPage, navigateAway });
    };
    root.querySelector("[data-feature-new-gap]")?.addEventListener("click", () => {
      close();
      openFeatureNewGapFlow(feature.id, async () => {
        const data = await api("GET", `/api/features/${encodeURIComponent(feature.id)}`);
        openFeatureModal(data.feature, { navigateAway });
      });
    });
    root.querySelector("[data-feature-assign-gap]")?.addEventListener("click", async () => {
      await openFeatureAssignGapModal(feature.id);
      await reloadModal();
    });
    bindFeatureGapActions(root, feature.id, reloadModal);
    bindFeatureGapDragReorder(root, feature.id, reloadModal);
    bindPaginationControls(root, "feature-modal-gaps", (pageNo) => {
      openFeatureModal(feature, { gapPage: pageNo, navigateAway });
    });
    root.querySelector("[data-feature-cancel]")?.addEventListener("click", () =>
      cancelFeatureFromUi(feature.id));
    root.querySelector("[data-feature-delete]")?.addEventListener("click", () =>
      deleteFeatureFromUi(feature.id));
  }
  root.querySelector("#feature-name")?.focus();
}

function bindFeatureAutosave(root, feature) {
  const controls = [
    root.querySelector("#feature-name"),
    root.querySelector("#feature-description"),
  ].filter(Boolean);
  const saved = {
    name: feature.name || "",
    description: feature.description || "",
  };
  let inFlight = false;
  let pending = false;
  const restoreSaved = () => {
    const name = root.querySelector("#feature-name");
    const description = root.querySelector("#feature-description");
    if (name) name.value = saved.name;
    if (description) description.value = saved.description;
  };
  const save = async () => {
    if (inFlight) {
      pending = true;
      return;
    }
    const body = {
      name: root.querySelector("#feature-name")?.value.trim() || "",
      description: root.querySelector("#feature-description")?.value.trim() || "",
      reporter: feature.reporter || "",
    };
    if (!body.name) {
      toast("Feature name is required", "error");
      restoreSaved();
      return;
    }
    if (body.name === saved.name && body.description === saved.description) return;
    inFlight = true;
    try {
      const result = await api("PATCH", `/api/features/${encodeURIComponent(feature.id)}`, body);
      const updated = result.feature || {};
      saved.name = updated.name || body.name;
      saved.description = updated.description || body.description;
      if (state.currentRoute === "features") await refreshFeaturesTable();
    } catch (e) {
      restoreSaved();
      showActionError(e, "Feature autosave failed");
    } finally {
      inFlight = false;
      if (pending) {
        pending = false;
        await save();
      }
    }
  };
  const autosave = debounce(save, 450);
  controls.forEach((control) => {
    control.addEventListener("input", autosave);
    control.addEventListener("change", save);
  });
}

function bindFeatureGapDragReorder(root, featureId, onChanged) {
  let draggedGapId = "";
  root.querySelectorAll("[data-feature-drag-gap]").forEach((handle) => {
    handle.addEventListener("click", (e) => e.preventDefault());
    handle.addEventListener("dragstart", (e) => {
      draggedGapId = handle.dataset.featureDragGap || "";
      if (!draggedGapId) {
        e.preventDefault();
        return;
      }
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", draggedGapId);
      handle.closest("[data-feature-gap-row]")?.classList.add("dragging");
    });
    handle.addEventListener("dragend", () => {
      draggedGapId = "";
      clearFeatureGapDragState(root);
    });
  });
  root.querySelectorAll("[data-feature-gap-row]").forEach((row) => {
    row.addEventListener("dragover", (e) => {
      if (!draggedGapId) return;
      const targetGapId = row.dataset.featureGapRow || "";
      if (!targetGapId || targetGapId === draggedGapId) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = "move";
      const rect = row.getBoundingClientRect();
      const position = e.clientY < rect.top + rect.height / 2 ? "before" : "after";
      root.querySelectorAll("[data-feature-gap-row]").forEach((candidate) => {
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
      const sourceGapId = e.dataTransfer.getData("text/plain") || draggedGapId;
      const targetGapId = row.dataset.featureGapRow || "";
      const position = row.dataset.featureDropPosition || "after";
      clearFeatureGapDragState(root);
      if (!sourceGapId || !targetGapId || sourceGapId === targetGapId) return;
      e.preventDefault();
      try {
        await api("POST", `/api/features/${encodeURIComponent(featureId)}/gaps/${encodeURIComponent(sourceGapId)}/reorder`, {
          [position]: targetGapId,
        });
        toast("Feature order updated", "info");
        await onChanged?.();
      } catch (err) {
        showActionError(err, "Reorder failed");
      }
    });
  });
}

function clearFeatureGapDragState(root) {
  root.querySelectorAll("[data-feature-gap-row]").forEach((row) => {
    row.classList.remove("dragging", "drop-before", "drop-after");
    delete row.dataset.featureDropPosition;
  });
}

function bindFeatureGapActions(root, featureId, onChanged) {
  root.querySelectorAll("[data-feature-move]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const gapId = btn.dataset.gapId;
      const siblingId = btn.dataset.neighborId;
      if (!gapId || !siblingId) return;
      const body = btn.dataset.featureMove === "up"
        ? { before: siblingId }
        : { after: siblingId };
      try {
        await api("POST", `/api/features/${encodeURIComponent(featureId)}/gaps/${encodeURIComponent(gapId)}/reorder`, body);
        toast("Feature order updated", "info");
        await onChanged?.();
      } catch (e) {
        showActionError(e, "Reorder failed");
      }
    });
  });
  root.querySelectorAll("[data-feature-delete-gap]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const gapId = btn.dataset.featureDeleteGap;
      if (!gapId) return;
      const ok = await modalConfirm(
        "Delete this Gap from the Feature? This cannot be undone.",
        { title: "Delete Gap", okLabel: "Delete Gap", cancelLabel: "Keep it", danger: true },
      );
      if (!ok) return;
      try {
        await api("DELETE", `/api/gaps/${encodeURIComponent(gapId)}`);
        toast("Gap deleted", "info");
        await onChanged?.();
      } catch (e) {
        showActionError(e, "Delete failed");
      }
    });
  });
}

function openFeatureNewGapFlow(featureId, onSaved) {
  openNewGapModal({
    featureId,
    onSaved: async () => {
      await onSaved?.();
    },
  });
}

async function openFeatureAssignGapModal(featureId) {
  const data = await api("GET", `/api/features/${encodeURIComponent(featureId)}/candidate-gaps?limit=100`);
  const gaps = data.gaps || [];
  if (!gaps.length) {
    await modalAlert("No standalone Gaps are available for this Feature.", {
      title: "Assign Gap",
    });
    return;
  }
  const body = () => `
    <div class="modal-title">Assign Existing Gap</div>
    <div class="modal-body">
      <label>Gap</label>
      <select class="modal-input">
        ${gaps.map((gap) => `
          <option value="${htmlEscape(gap.id)}">
            ${htmlEscape(gap.name || gap.id)} · ${htmlEscape(gap.status || "backlog")} · ${htmlEscape(gap.priority || "low")}
          </option>`).join("")}
      </select>
    </div>
    <div class="modal-actions">
      <button class="secondary" data-cancel>Cancel</button>
      <button data-ok>Assign</button>
    </div>`;
  const selected = await _openModal(body, { cancel: null, ok: "" }, ".modal-input");
  if (!selected) return;
  try {
    await api("POST", `/api/features/${encodeURIComponent(featureId)}/gaps/${encodeURIComponent(selected)}`);
    toast("Gap assigned to Feature", "info");
  } catch (e) {
    showActionError(e, "Assign failed");
  }
}

async function cancelFeatureFromUi(featureId) {
  const ok = await modalConfirm(
    "Cancel this Feature? Completed Gaps stay done and every non-terminal Gap in the Feature will be cancelled.",
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

async function deleteFeatureFromUi(featureId) {
  const ok = await modalConfirm(
    "Delete this Feature and all Gaps in it? This cannot be undone.",
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
