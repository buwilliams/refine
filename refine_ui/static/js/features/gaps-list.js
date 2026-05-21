// ---- Gaps: list -------------------------------------------------------------

const GAPS_DEFAULT_DIR = {
  name: "asc", status: "asc", priority: "asc",
  reporter: "asc", instance: "asc", updated: "desc", id: "desc",
};

// Mirror Logs' entries-limit dropdown so the two screens feel consistent.
const GAPS_LIMIT_OPTIONS = [50, 100, 250, 500, 1000];
const GAPS_DEFAULT_LIMIT = 100;

function gapsHash(parts) {
  const next = new URLSearchParams();
  if (parts.q)        next.set("q", parts.q);
  if (parts.status)   next.set("status", parts.status);
  if (parts.reporter) next.set("reporter", parts.reporter);
  if (parts.instance) next.set("instance", parts.instance);
  if (parts.severity) next.set("severity", parts.severity);
  if (parts.category) next.set("category", parts.category);
  if (parts.actor)    next.set("actor", parts.actor);
  if (parts.limit && parts.limit !== GAPS_DEFAULT_LIMIT) next.set("limit", String(parts.limit));
  if (parts.page && parts.page > 1) next.set("page", String(parts.page));
  if (parts.sort)     next.set("sort", parts.sort);
  if (parts.dir)      next.set("dir", parts.dir);
  return "#/gaps" + (next.toString() ? "?" + next : "");
}

async function renderGapsList() {
  renderBanners([]);
  const f = gapsFilterFromHash();
  // Preserve the filter shell's open/closed state across full re-renders
  // (Clear filters, bulk-op completion, etc.). First-ever render has no
  // prior element, so it falls through to the default (closed).
  const filterShellOpen = !!document.getElementById("gaps-filter-shell")?.open;

  $("#main").innerHTML = `
    <h2>Gaps</h2>
    <details class="filter-shell" id="gaps-filter-shell"${filterShellOpen ? " open" : ""}>
      <summary>
        <span class="filter-shell-title">Filters &amp; bulk actions</span>
        <span class="spacer"></span>
        <span class="muted small"><span id="gaps-count"></span></span>
        <span id="gaps-filtered" class="filter-pill" hidden>Filtered</span>
      </summary>
      <div class="filter-shell-body">
    <div class="filter-bar">
      <div class="filter-row filter-row-primary">
        <input type="text" id="search" class="filter-grow"
               placeholder="Search gaps…" value="${htmlEscape(f.q)}">
      </div>
      <div class="filter-row filter-row-activity">
        <select id="filter-status">
          ${STATUS_FILTER_OPTIONS
            .map((s) => `<option value="${s}" ${s === f.status ? "selected" : ""}>${s || "all statuses"}</option>`).join("")}
        </select>
        <select id="filter-reporter">
          <option value="" ${f.reporter === "" ? "selected" : ""}>all reporters</option>
          ${(state.reporters || []).map((r) =>
            `<option value="${htmlEscape(r.name)}" ${r.name === f.reporter ? "selected" : ""}>${htmlEscape(r.name)}</option>`).join("")}
          ${f.reporter && !(state.reporters || []).some((r) => r.name === f.reporter)
            ? `<option value="${htmlEscape(f.reporter)}" selected>${htmlEscape(f.reporter)}</option>` : ""}
        </select>
        <select id="filter-instance">
          <option value="" ${f.instance === "" ? "selected" : ""}>all instances</option>
          <option value="current" ${f.instance === "current" ? "selected" : ""}>current instance</option>
          <option value="unknown" ${f.instance === "unknown" ? "selected" : ""}>unknown instance</option>
          ${(state.project?.instances || []).map((inst) =>
            `<option value="${htmlEscape(inst.id)}" ${inst.id === f.instance ? "selected" : ""}>${htmlEscape(inst.display_name || inst.id)}</option>`).join("")}
        </select>
        <select id="gaps-severity">
          <option value="" ${f.severity === "" ? "selected" : ""}>all severities</option>
          <option value="info"  ${f.severity === "info"  ? "selected" : ""}>info</option>
          <option value="warn"  ${f.severity === "warn"  ? "selected" : ""}>warn</option>
          <option value="error" ${f.severity === "error" ? "selected" : ""}>error</option>
        </select>
        <select id="gaps-category"><option value="">all categories</option></select>
        <select id="gaps-actor"><option value="">all actors</option></select>
        <select id="gaps-limit">
          ${GAPS_LIMIT_OPTIONS.map((n) =>
            `<option value="${n}" ${n === f.limit ? "selected" : ""}>${n} entries</option>`).join("")}
        </select>
        <span class="spacer"></span>
        <button class="secondary" id="gaps-clear">Clear filters</button>
      </div>
      <div class="filter-row filter-row-bulk">
        <span class="muted small">Bulk update selected:</span>
        <button class="secondary small" id="bulk-set-status">Status…</button>
        <button class="secondary small" id="bulk-set-priority">Priority…</button>
        <button class="secondary small" id="bulk-set-reporter">Reporter…</button>
        <button class="secondary small" id="bulk-transfer-instance">Instance…</button>
        <button class="secondary small" id="bulk-delete">Delete…</button>
      </div>
    </div>
      </div>
    </details>
    <div id="gaps-table"><p class="muted">Loading…</p></div>
  `;
  // In-view filter changes update the URL via replaceState (which does NOT
  // fire `hashchange`) and refresh only the table. Going through
  // `location.hash =` would trigger renderGapsList again, which rebuilds
  // `#main` from scratch — that destroys the focused search input mid-
  // keystroke. Sort-header clicks go through the same path
  // (`refreshGapsTable`); see drawGapsTable.
  $("#search").addEventListener("input", debounce(() => {
    updateGapsFilter({ q: $("#search").value, page: 1 });
  }, 250));
  $("#filter-status").addEventListener("change", (e) =>
    updateGapsFilter({ status: e.target.value, page: 1 }));
  $("#filter-reporter").addEventListener("change", (e) =>
    updateGapsFilter({ reporter: e.target.value, page: 1 }));
  $("#filter-instance").addEventListener("change", (e) =>
    updateGapsFilter({ instance: e.target.value, page: 1 }));
  $("#gaps-severity").addEventListener("change", (e) =>
    updateGapsFilter({ severity: e.target.value, page: 1 }));
  $("#gaps-category").addEventListener("change", (e) =>
    updateGapsFilter({ category: e.target.value, page: 1 }));
  $("#gaps-actor").addEventListener("change", (e) =>
    updateGapsFilter({ actor: e.target.value, page: 1 }));
  $("#gaps-limit").addEventListener("change", (e) =>
    updateGapsFilter({
      limit: parseInt(e.target.value, 10) || GAPS_DEFAULT_LIMIT,
      page: 1,
    }));
  $("#gaps-clear").addEventListener("click", () => {
    history.replaceState(null, "", "#/gaps");
    renderGapsList();
  });
  // The bulk-action buttons read the current filter from the hash at click
  // time, so they always reflect what the user can see.
  $("#bulk-set-priority").addEventListener("click", () => openBulkModal("priority"));
  $("#bulk-set-status").addEventListener("click", () => openBulkModal("status"));
  $("#bulk-set-reporter").addEventListener("click", () => openBulkModal("reporter"));
  $("#bulk-transfer-instance").addEventListener("click", () => openBulkTransferInstanceModal());
  $("#bulk-delete").addEventListener("click", () => confirmBulkDelete());

  // Expanding / collapsing the filter shell shows / hides the per-row
  // checkbox column. Redraw from the cached results so we don't re-fetch.
  $("#gaps-filter-shell").addEventListener("toggle", () => {
    if (_lastGapsRender) {
      drawGapsTable(_lastGapsRender.gaps, _lastGapsRender.state);
    }
  });

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
    reporter: hashQs.get("reporter") || "",
    instance: hashQs.get("instance") || "",
    severity: hashQs.get("severity") || "",
    category: hashQs.get("category") || "",
    actor: hashQs.get("actor") || "",
    limit: parseInt(hashQs.get("limit") || String(GAPS_DEFAULT_LIMIT), 10)
           || GAPS_DEFAULT_LIMIT,
    page: Math.max(1, parseInt(hashQs.get("page") || "1", 10) || 1),
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
    reporter: "reporter" in patch ? patch.reporter : current.reporter,
    instance: "instance" in patch ? patch.instance : current.instance,
    severity: "severity" in patch ? patch.severity : current.severity,
    category: "category" in patch ? patch.category : current.category,
    actor: "actor" in patch ? patch.actor : current.actor,
    limit: "limit" in patch ? patch.limit : current.limit,
    page: "page" in patch ? patch.page : current.page,
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
  if (f.reporter) params.set("reporter", f.reporter);
  if (f.instance) params.set("instance", f.instance);
  if (f.severity) params.set("severity", f.severity);
  if (f.category) params.set("category", f.category);
  if (f.actor) params.set("actor", f.actor);
  if (f.limit) params.set("limit", String(f.limit));
  params.set("offset", String((f.page - 1) * f.limit));
  if (f.sort) params.set("sort", f.sort);
  if (f.dir) params.set("dir", f.dir);
  params.set("facets", "1");
  try {
    const data = await api("GET", "/api/gaps?" + params);
    const gaps = data.gaps || [];
    const facets = data.facets || {};
    // Refresh the category / actor dropdowns from the server-side
    // distinct values — same pattern as the Logs screen.
    const catSel = $("#gaps-category");
    if (catSel) {
      const cats = facets.categories || [];
      catSel.innerHTML = `<option value="">all categories</option>` +
        cats.map((c) => `<option value="${htmlEscape(c)}" ${c === f.category ? "selected" : ""}>${htmlEscape(c)}</option>`).join("");
    }
    const actSel = $("#gaps-actor");
    if (actSel) {
      const acts = facets.actors || [];
      actSel.innerHTML = `<option value="">all actors</option>` +
        acts.map((a) => `<option value="${htmlEscape(a)}" ${a === f.actor ? "selected" : ""}>${htmlEscape(a)}</option>`).join("");
    }
    const countEl = $("#gaps-count");
    if (countEl) {
      countEl.textContent = `${gaps.length} gap${gaps.length === 1 ? "" : "s"}`;
    }
    applyGapsFilterIndicator(f);
    const renderState = {
      q: f.q, status: f.status,
      sort: f.effectiveSort, dir: f.effectiveDir,
      page: data.page || {
        limit: f.limit,
        offset: (f.page - 1) * f.limit,
        has_more: false,
      },
    };
    _lastGapsRender = { gaps, state: renderState };
    drawGapsTable(gaps, renderState);
  } catch (e) {
    const tbl = $("#gaps-table");
    if (tbl) tbl.innerHTML = `<p class="muted">${htmlEscape(e.message)}</p>`;
  }
}

// Bulk selection is filter-scoped. By default, bulk actions target every
// matching Gap across pagination, with checked row changes stored as explicit
// include/exclude exceptions. State survives filter tweaks and re-expanding
// the filter shell but resets on a hard navigation away from the Gaps screen.
let gapsSelectAllMatching = true;
const gapsExcludedIds = new Set();
const gapsIncludedIds = new Set();

// Cached snapshot of the last refresh, so toggling the filter shell open
// or closed can redraw the table without re-fetching.
let _lastGapsRender = null;

function resetGapsSelection() {
  gapsSelectAllMatching = true;
  gapsExcludedIds.clear();
  gapsIncludedIds.clear();
}

function _isGapSelected(id) {
  return gapsSelectAllMatching
    ? !gapsExcludedIds.has(id)
    : gapsIncludedIds.has(id);
}

function drawGapsTable(gaps, state) {
  const root = $("#gaps-table");
  // Selection UI follows the filter shell — only show checkboxes when the
  // shell is expanded (i.e. the user has indicated they want to interact
  // with bulk actions). Collapsed = focus on results.
  const shell = document.getElementById("gaps-filter-shell");
  const showSelection = !!(shell && shell.open);

  if (!gaps.length) {
    root.innerHTML = `
      <p class="muted">No gaps match the current filters.</p>
      ${renderPaginationControls("gaps", state.page, 0, "gap")}`;
    bindPaginationControls(root, "gaps", (page) =>
      updateGapsFilter({ page }));
    return;
  }
  const columns = [
    { key: "name",     label: "Name",     sortable: true },
    { key: "status",   label: "Status",   sortable: true },
    { key: "priority", label: "Priority", sortable: true },
    { key: "reporter", label: "Reporter", sortable: true },
    { key: "instance", label: "Instance", sortable: true },
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
                data-sort-key="${c.key}">
              ${c.label} <span class="sort-arrow">${arrow}</span>
            </th>`;
  }).join("");
  const selectionHead = showSelection
    ? `<th class="gap-select-col">
         <input type="checkbox" id="gap-select-all"
                aria-label="Select all matching Gaps">
       </th>`
    : "";
  root.innerHTML = `
    <table class="table gaps-table">
      <colgroup>
        ${showSelection ? '<col class="gaps-col-select">' : ""}
        <col class="gaps-col-name">
        <col class="gaps-col-status">
        <col class="gaps-col-priority">
        <col class="gaps-col-reporter">
        <col class="gaps-col-instance">
        <col class="gaps-col-updated">
      </colgroup>
      <thead><tr>${selectionHead}${sortHeads}</tr></thead>
      <tbody>
        ${gaps.map((g) => {
          const selected = _isGapSelected(g.id);
          const cell = showSelection
            ? `<td class="gap-select-col">
                 <input type="checkbox" class="gap-select"
                        data-id="${g.id}"
                        ${selected ? "checked" : ""}
                        aria-label="Select gap ${htmlEscape(g.name)}">
               </td>`
            : "";
          return `<tr data-id="${g.id}">
            ${cell}
            <td class="gaps-name-cell">${htmlEscape(g.name)}</td>
            <td class="gaps-status-cell"><span class="status-pill ${g.status}">${g.status}</span></td>
            <td><span class="priority-pill priority-${g.priority || "low"}">${g.priority || "low"}</span></td>
            <td class="muted small">${g.reporter ? htmlEscape(g.reporter) : "—"}</td>
            <td class="muted small">${htmlEscape(g.instance_display_name || g.instance_id || "Unknown")}</td>
            <td class="muted small">${fmtTime(g.updated)}</td>
          </tr>`;
        }).join("")}
      </tbody>
	    </table>
      ${renderPaginationControls("gaps", state.page, gaps.length, "gap")}
	  `;
  bindPaginationControls(root, "gaps", (page) =>
    updateGapsFilter({ page }));
  // Row click navigates to gap detail — but a click on the checkbox (or
  // its surrounding td) should toggle selection, not navigate.
  $$(".table tbody tr", root).forEach((row) => {
    row.addEventListener("click", (e) => {
      if (e.target.closest(".gap-select-col")) return;
      location.hash = "#/gaps/" + row.dataset.id;
    });
  });
  $$(".gap-select", root).forEach((cb) => {
    cb.addEventListener("click", (e) => e.stopPropagation());
    cb.addEventListener("change", (e) => {
      const id = e.target.dataset.id;
      if (gapsSelectAllMatching) {
        if (e.target.checked) gapsExcludedIds.delete(id);
        else gapsExcludedIds.add(id);
      } else if (e.target.checked) {
        gapsIncludedIds.add(id);
      } else {
        gapsIncludedIds.delete(id);
      }
      _updateSelectAllState(gaps);
    });
  });
  const selectAll = root.querySelector("#gap-select-all");
  if (selectAll) {
    _updateSelectAllState(gaps);
    selectAll.addEventListener("click", (e) => {
      e.stopPropagation();
      const shouldCheck = selectAll.checked;
      gapsSelectAllMatching = shouldCheck;
      gapsExcludedIds.clear();
      gapsIncludedIds.clear();
      // Re-sync the current page checkboxes without a full redraw.
      $$(".gap-select", root).forEach((cb) => {
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
        nextDir = GAPS_DEFAULT_DIR[key] || "desc";
      }
      updateGapsFilter({ sort: key, dir: nextDir, page: 1 });
    });
  });
}

// Sync the header checkbox to the global filter-scoped selection:
// all matching selected -> checked, none selected -> unchecked, per-row
// exceptions -> indeterminate.
function _updateSelectAllState(gaps) {
  const master = document.getElementById("gap-select-all");
  if (!master) return;
  if (!gaps.length && !gapsIncludedIds.size) {
    master.checked = false;
    master.indeterminate = false;
  } else if (gapsSelectAllMatching && gapsExcludedIds.size === 0) {
    master.checked = true;
    master.indeterminate = false;
  } else if (!gapsSelectAllMatching && gapsIncludedIds.size === 0) {
    master.checked = false;
    master.indeterminate = false;
  } else {
    master.checked = false;
    master.indeterminate = true;
  }
}
