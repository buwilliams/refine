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
      <button id="features-new">New Feature</button>
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
  $("#features-new")?.addEventListener("click", () => openFeatureModal());
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
  const page = stateForRender.pageMeta || {};
  const total = page.total ?? ((page.offset || 0) + features.length + (page.has_more ? 1 : 0));
  $("#features-count").textContent = `${total} feature${total === 1 ? "" : "s"}`;
  $("#features-filtered").hidden = !(
    stateForRender.q || stateForRender.status || stateForRender.reporter || stateForRender.node
  );
  const rows = features.length ? features.map((feature) => `
    <tr data-feature-id="${htmlEscape(feature.id)}">
      <td data-label="Name"><a href="#/features/${encodeURIComponent(feature.id)}">${htmlEscape(feature.name || "Untitled Feature")}</a></td>
      <td data-label="Status"><span class="status-pill ${htmlEscape(feature.status || "backlog")}">${workflowStatusLabel(feature.status || "backlog")}</span></td>
      <td data-label="Progress">${feature.done_count || 0} / ${feature.gap_count || 0} done</td>
      <td data-label="Next">${feature.next_gap ? htmlEscape(feature.next_gap.name || feature.next_gap.id) : '<span class="muted small">-</span>'}</td>
      <td data-label="Reporter">${htmlEscape(feature.reporter || "-")}</td>
      <td data-label="Node">${htmlEscape(feature.node_display_name || feature.node_id || "-")}</td>
      <td data-label="Updated">${fmtTime(feature.updated)}</td>
    </tr>`).join("") : `
    <tr><td colspan="7" class="muted">No Features match the current filters.</td></tr>`;
  $("#features-table").innerHTML = `
    <div class="table-scroll">
      <table>
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
  return `<th class="sortable ${active ? "active" : ""}" data-sort="${key}">
    ${htmlEscape(label)} <span class="sort-indicator">${active ? (dir === "asc" ? "▲" : "▼") : ""}</span>
  </th>`;
}

async function renderFeatureDetail(route) {
  if (renderNoProjectIfDetached("Features")) return;
  const featureId = route.id;
  const data = await api("GET", `/api/features/${encodeURIComponent(featureId)}`);
  const feature = data.feature;
  $("#main").innerHTML = `
    <div class="page-title-row">
      <div>
        <h2>${htmlEscape(feature.name || "Untitled Feature")}</h2>
        <p class="muted">${htmlEscape(feature.description || "")}</p>
      </div>
      <div class="actions">
        <button id="feature-new-gap">New Gap</button>
        <button class="secondary" id="feature-assign-gap">Assign existing</button>
        <button class="secondary" id="feature-edit">Edit</button>
        <button class="secondary" id="feature-cancel">Cancel Feature</button>
        <button class="danger" id="feature-delete">Delete Feature</button>
      </div>
    </div>
    <div class="panel">
      <p>
        <span class="status-pill ${htmlEscape(feature.status || "backlog")}">${workflowStatusLabel(feature.status || "backlog")}</span>
        <span class="muted small">${feature.done_count || 0} / ${feature.gap_count || 0} done</span>
      </p>
      ${renderFeatureGapTable(feature.gaps || [], { actions: true })}
    </div>
  `;
  $("#feature-edit")?.addEventListener("click", () => openFeatureModal(feature));
  $("#feature-new-gap")?.addEventListener("click", () => openFeatureNewGapFlow(feature.id, async () => {
    await renderFeatureDetail({ id: feature.id });
  }));
  $("#feature-assign-gap")?.addEventListener("click", async () => {
    await openFeatureAssignGapModal(feature.id);
    await renderFeatureDetail({ id: feature.id });
  });
  $("#feature-cancel")?.addEventListener("click", () => cancelFeatureFromUi(feature.id));
  $("#feature-delete")?.addEventListener("click", () => deleteFeatureFromUi(feature.id));
  bindFeatureGapActions(document, feature.id, async () => {
    await renderFeatureDetail({ id: feature.id });
  });
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
    <tr>
      <td data-label="Order">${gap.feature_order || ""}</td>
      <td data-label="Gap"><a href="#/gaps/${encodeURIComponent(gap.id)}">${htmlEscape(gap.name || gap.id)}</a></td>
      <td data-label="Status"><span class="status-pill ${htmlEscape(gap.status || "backlog")}">${workflowStatusLabel(gap.status || "backlog")}</span></td>
      <td data-label="Priority">${htmlEscape(gap.priority || "low")}</td>
      <td data-label="Reporter">${htmlEscape(gap.reporter || "-")}</td>
      <td data-label="Updated">${fmtTime(gap.updated)}</td>
      ${actions ? `<td data-label="Actions">
        <div class="actions compact-actions">
          <button class="secondary small" data-feature-move="up" data-gap-id="${htmlEscape(gap.id)}" data-neighbor-id="${htmlEscape(gaps[globalIdx - 1]?.id || "")}" ${globalIdx === 0 ? "disabled" : ""}>Up</button>
          <button class="secondary small" data-feature-move="down" data-gap-id="${htmlEscape(gap.id)}" data-neighbor-id="${htmlEscape(gaps[globalIdx + 1]?.id || "")}" ${globalIdx === gaps.length - 1 ? "disabled" : ""}>Down</button>
          <button class="secondary small" data-feature-remove-gap="${htmlEscape(gap.id)}">Remove</button>
        </div>
      </td>` : ""}
    </tr>`;
  }).join("") : `
    <tr><td colspan="${actions ? 7 : 6}" class="muted">No Gaps are assigned to this Feature.</td></tr>`;
  return `
    <div class="table-scroll">
      <table>
        <thead><tr><th>Order</th><th>Gap</th><th>Status</th><th>Priority</th><th>Reporter</th><th>Updated</th>${actions ? "<th>Actions</th>" : ""}</tr></thead>
        <tbody>${rows}</tbody>
      </table>
    </div>
    ${pageMeta ? renderPaginationControls("feature-modal-gaps", pageMeta, visible.length, "gap") : ""}`;
}

function renderFeatureNew() {
  location.hash = "#/features";
  setTimeout(() => openFeatureModal(), 0);
}

function openFeatureModal(feature = null, options = {}) {
  const root = document.createElement("div");
  root.className = "modal-backdrop";
  const gaps = feature?.gaps || [];
  const gapPage = Math.max(1, parseInt(options.gapPage || "1", 10) || 1);
  root.innerHTML = `
    <div class="modal feature-modal" role="dialog" aria-modal="true" aria-labelledby="feature-modal-title">
      <div class="modal-title" id="feature-modal-title">${feature ? "Edit Feature" : "New Feature"}</div>
      <div class="modal-body">
        <label>Name</label>
        <input type="text" id="feature-name" class="modal-input" value="${htmlEscape(feature?.name || "")}">
        <label>Description</label>
        <textarea id="feature-description">${htmlEscape(feature?.description || "")}</textarea>
        <label>Reporter</label>
        <input type="text" id="feature-reporter" value="${htmlEscape(feature?.reporter || state.lastReporter || "")}">
        <div class="feature-modal-gap-heading">
          <div class="modal-title compact">Ordered Gaps</div>
          ${feature ? `<div class="actions">
            <button type="button" class="secondary small" data-feature-new-gap>New Gap</button>
            <button type="button" class="secondary small" data-feature-assign-gap>Assign existing</button>
          </div>` : ""}
        </div>
        ${feature
          ? renderFeatureGapTable(gaps, {
              actions: true,
              page: gapPage,
              pageSize: FEATURE_MODAL_GAP_PAGE_SIZE,
            })
          : `<p class="muted small">Create the Feature before adding ordered Gaps.</p>`}
      </div>
      <div class="modal-actions">
        <button class="secondary" data-cancel>Cancel</button>
        <button data-ok>${feature ? "Save" : "Create"}</button>
      </div>
    </div>`;
  document.body.appendChild(root);
  const close = () => root.remove();
  root.addEventListener("click", (e) => {
    if (e.target === root) close();
  });
  root.querySelector("[data-cancel]")?.addEventListener("click", close);
  root.querySelector("[data-ok]")?.addEventListener("click", async () => {
    const body = {
      name: root.querySelector("#feature-name")?.value.trim() || "",
      description: root.querySelector("#feature-description")?.value.trim() || "",
      reporter: root.querySelector("#feature-reporter")?.value.trim() || "",
    };
    if (!body.name) {
      toast("Feature name is required", "error");
      return;
    }
    try {
      const saved = feature
        ? await api("PATCH", `/api/features/${encodeURIComponent(feature.id)}`, body)
        : await api("POST", "/api/features", body);
      close();
      toast(feature ? "Feature updated" : "Feature created", "success");
      location.hash = `#/features/${encodeURIComponent(saved.feature.id)}`;
    } catch (e) {
      showActionError(e);
    }
  });
  if (feature) {
    const reloadModal = async () => {
      const data = await api("GET", `/api/features/${encodeURIComponent(feature.id)}`);
      close();
      openFeatureModal(data.feature, { gapPage });
      if (state.currentRoute === "features_detail") {
        await renderFeatureDetail({ id: feature.id });
      }
    };
    root.querySelector("[data-feature-new-gap]")?.addEventListener("click", () => {
      close();
      openFeatureNewGapFlow(feature.id, async () => {
        const data = await api("GET", `/api/features/${encodeURIComponent(feature.id)}`);
        openFeatureModal(data.feature);
        if (state.currentRoute === "features_detail") {
          await renderFeatureDetail({ id: feature.id });
        }
      });
    });
    root.querySelector("[data-feature-assign-gap]")?.addEventListener("click", async () => {
      await openFeatureAssignGapModal(feature.id);
      await reloadModal();
    });
    bindFeatureGapActions(root, feature.id, reloadModal);
    bindPaginationControls(root, "feature-modal-gaps", (pageNo) => {
      close();
      openFeatureModal(feature, { gapPage: pageNo });
    });
  }
  root.querySelector("#feature-name")?.focus();
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
  root.querySelectorAll("[data-feature-remove-gap]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const gapId = btn.dataset.featureRemoveGap;
      if (!gapId) return;
      const ok = await modalConfirm(
        "Remove this Gap from the Feature? The Gap will not be deleted.",
        { title: "Remove Gap", okLabel: "Remove", cancelLabel: "Keep it" },
      );
      if (!ok) return;
      try {
        await api("DELETE", `/api/features/${encodeURIComponent(featureId)}/gaps/${encodeURIComponent(gapId)}`);
        toast("Gap removed from Feature", "info");
        await onChanged?.();
      } catch (e) {
        showActionError(e, "Remove failed");
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
    await renderFeatureDetail({ id: featureId });
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
    location.hash = "#/features";
  } catch (e) {
    showActionError(e, "Delete Feature failed");
  }
}
