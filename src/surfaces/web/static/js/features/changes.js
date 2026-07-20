// ---- Changes ----------------------------------------------------------------
//
// Lists approved implementation commits on the configured integration branch.
// Each implementation links to its Goal and offers an Undo button; Refine owns
// the repository operation and moves the linked Goal to cancelled.

const CHANGES_LIMIT_OPTIONS = [50, 100, 250, 500, 1000];
const CHANGES_DEFAULT_LIMIT = 50;
const CHANGES_DEFAULT_SORT = "committed";
const CHANGES_DEFAULT_DIR = "desc";
const CHANGES_DEFAULT_PERIOD = "day";
const CHANGES_PERIODS = ["day", "week", "month", "year"];

function changesFiltersFromHash() {
  const hashQs = new URLSearchParams(location.hash.split("?")[1] || "");
  return {
    q: hashQs.get("q") || "",
    status: hashQs.get("status") || "",
    priority: hashQs.get("priority") || "",
    limit: parseInt(hashQs.get("limit") || String(CHANGES_DEFAULT_LIMIT), 10)
           || CHANGES_DEFAULT_LIMIT,
    page: Math.max(1, parseInt(hashQs.get("page") || "1", 10) || 1),
    sort: hashQs.get("sort") || CHANGES_DEFAULT_SORT,
    dir: hashQs.get("dir") || CHANGES_DEFAULT_DIR,
    period: CHANGES_PERIODS.includes(hashQs.get("period")) ? hashQs.get("period") : CHANGES_DEFAULT_PERIOD,
  };
}

function changesHashFromFilters(f) {
  const next = new URLSearchParams();
  if (f.q) next.set("q", f.q);
  if (f.status) next.set("status", f.status);
  if (f.priority) next.set("priority", f.priority);
  if (f.limit && f.limit !== CHANGES_DEFAULT_LIMIT) {
    next.set("limit", String(f.limit));
  }
  if (f.page && f.page > 1) next.set("page", String(f.page));
  if (f.sort && f.sort !== CHANGES_DEFAULT_SORT) next.set("sort", f.sort);
  if (f.dir && f.dir !== CHANGES_DEFAULT_DIR) next.set("dir", f.dir);
  if (f.period && f.period !== CHANGES_DEFAULT_PERIOD) next.set("period", f.period);
  return "#/changes" + (next.toString() ? "?" + next : "");
}

async function renderChanges() {
  if (renderNoProjectIfDetached("Changes")) return;
  renderBanners([]);
  const f = changesFiltersFromHash();
  const filterShell = document.getElementById("changes-filter-shell");
  const filterShellOpen = filterShell ? filterShell.open : false;
  $("#main").innerHTML = `
    <h2>Changes</h2>
    <section class="changes-visualization-panel" data-testid="changes-visualization-panel">
      <div class="changes-visualization-head">
        <div class="segmented-control changes-period-control" role="group" aria-label="Git visualization period" data-testid="changes-period-control">
          ${CHANGES_PERIODS.map((period) => `
            <button type="button"
                    data-changes-period="${period}"
                    data-testid="changes-period-${period}"
                    ${f.period === period ? 'class="active" aria-pressed="true"' : 'aria-pressed="false"'}>
              ${period.charAt(0).toUpperCase() + period.slice(1)}
            </button>`).join("")}
        </div>
      </div>
      <div id="changes-visualization" data-testid="changes-visualization"><p class="muted">Loading...</p></div>
    </section>
    <details class="filter-shell" id="changes-filter-shell" data-testid="changes-filter-shell"${filterShellOpen ? " open" : ""}>
      <summary>
        <span class="filter-shell-title">Filters</span>
        <span class="spacer"></span>
        <span class="muted small"><span id="changes-count" data-testid="changes-count"></span></span>
        <span id="changes-filtered" class="filter-pill" data-testid="changes-filtered-pill" hidden>Filtered</span>
      </summary>
      <div class="filter-shell-body">
        <div class="filter-bar">
          <div class="filter-row filter-row-primary">
            <input type="text" id="changes-q"
                   class="filter-grow"
                   data-testid="changes-search"
                   placeholder="Search goal, commit, or status..."
                   value="${htmlEscape(f.q)}">
          </div>
          <div class="filter-row filter-row-filters">
            <select id="changes-status" data-testid="changes-status-filter">
              ${STATUS_FILTER_OPTIONS
                .map((s) => `<option value="${s}" ${s === f.status ? "selected" : ""}>${s ? workflowStatusLabel(s) : "all statuses"}</option>`).join("")}
            </select>
            <select id="changes-priority" data-testid="changes-priority-filter">
              <option value="" ${f.priority === "" ? "selected" : ""}>all priorities</option>
              <option value="high" ${f.priority === "high" ? "selected" : ""}>high</option>
              <option value="medium" ${f.priority === "medium" ? "selected" : ""}>medium</option>
              <option value="low" ${f.priority === "low" ? "selected" : ""}>low</option>
            </select>
            <select id="changes-limit" data-testid="changes-limit-filter">
              ${CHANGES_LIMIT_OPTIONS.map((n) =>
                `<option value="${n}" ${n === f.limit ? "selected" : ""}>${n} entries</option>`).join("")}
            </select>
            <span class="spacer"></span>
            <button class="secondary" id="changes-clear" data-testid="changes-clear-filters">Clear filters</button>
          </div>
        </div>
      </div>
    </details>
    <div id="changes-body" data-testid="changes-body"><p class="muted">Loading...</p></div>`;
  $("#changes-q").addEventListener("input", debounce(() => {
    updateChangesFilter({ q: $("#changes-q").value, page: 1 });
  }, 250));
  $("#changes-status").addEventListener("change", (e) =>
    updateChangesFilter({ status: e.target.value, page: 1 }));
  $("#changes-priority").addEventListener("change", (e) =>
    updateChangesFilter({ priority: e.target.value, page: 1 }));
  $("#changes-limit").addEventListener("change", (e) =>
    updateChangesFilter({
      limit: parseInt(e.target.value, 10) || CHANGES_DEFAULT_LIMIT,
      page: 1,
    }));
  $("#changes-clear").addEventListener("click", () => {
    history.replaceState(null, "", "#/changes");
    renderChanges();
  });
  $$("[data-changes-period]").forEach((btn) => {
    btn.addEventListener("click", () => updateChangesFilter({ period: btn.dataset.changesPeriod, page: 1 }));
  });
  await loadChanges();
}

function updateChangesFilter(patch) {
  const current = changesFiltersFromHash();
  const next = {
    q: "q" in patch ? patch.q : current.q,
    status: "status" in patch ? patch.status : current.status,
    priority: "priority" in patch ? patch.priority : current.priority,
    limit: "limit" in patch ? patch.limit : current.limit,
    page: "page" in patch ? patch.page : current.page,
    sort: "sort" in patch ? patch.sort : current.sort,
    dir: "dir" in patch ? patch.dir : current.dir,
    period: "period" in patch ? patch.period : current.period,
  };
  history.replaceState(null, "", changesHashFromFilters(next));
  loadChanges();
}

function updateChangesSort(key) {
  const current = changesFiltersFromHash();
  const dir = current.sort === key && current.dir === "asc" ? "desc" : "asc";
  updateChangesFilter({ sort: key, dir, page: 1 });
}

async function loadChanges() {
  if (state.currentRoute !== "changes") return;
  if (renderNoProjectIfDetached("Changes")) return;
  const f = changesFiltersFromHash();
  const params = new URLSearchParams();
  if (f.q) params.set("q", f.q);
  if (f.status) params.set("status", f.status);
  if (f.priority) params.set("priority", f.priority);
  if (f.sort) params.set("sort", f.sort);
  if (f.dir) params.set("dir", f.dir);
  params.set("limit", String(f.limit));
  params.set("offset", String((f.page - 1) * f.limit));
  try {
    const data = await api("GET", "/api/changes?" + params);
    if (renderNoProjectIfApiDetached(data, "Changes")) return;
    drawChanges(data, f);
  } catch (e) {
    const root = document.getElementById("changes-body");
    if (root) root.innerHTML = `<p class="muted">${htmlEscape(e.message)}</p>`;
  }
}

function drawChanges(data, f) {
  const root = document.getElementById("changes-body");
  // Guard against a late SSE refresh after the user navigated away.
  if (!root) return;
  const branch = data.branch || "(unknown)";
  const changes = data.changes || [];
  const pageMeta = data.page || {
    limit: f.limit,
    offset: (f.page - 1) * f.limit,
    has_more: false,
  };
  const countEl = $("#changes-count");
  if (countEl) {
    countEl.textContent = `${changes.length} ${changes.length === 1 ? "change" : "changes"}`;
  }
  applyChangesFilterIndicator(f);
  syncChangesPeriodControls(f.period);
  drawChangesVisualization(changes, f.period);
  if (!branch || branch === "(unknown)") {
    root.innerHTML = `
      <p class="muted" data-testid="changes-branch-unresolved">
        No integration branch resolved. Set <code>merge_target_branch</code>
        in <a href="#/node/target-app">Node → Target App Config</a>, or check that the host
        repo has a branch checked out.
      </p>`;
    return;
  }
  if (!changes.length) {
    root.innerHTML = `
      <p class="muted" data-testid="changes-empty-state">
        ${f.q || f.status || f.priority
          ? `No changes match the current filters on <code>${htmlEscape(branch)}</code>.`
          : `No approved implementations on <code>${htmlEscape(branch)}</code> yet. When a reviewed Goal is approved, its integration commit shows up here.`}
      </p>
      ${renderPaginationControls("changes", pageMeta, 0, "change")}`;
    bindPaginationControls(root, "changes", (page) =>
      updateChangesFilter({ page }));
    return;
  }
  const columns = [
    { key: "committed", label: "When" },
    { key: "goal", label: "Goal" },
    { key: "status", label: "Status" },
    { key: "priority", label: "Priority" },
    { key: "assignee", label: "Assignee" },
    { key: "commit", label: "Merge commit" },
  ];
  const sortHeads = columns.map((c) => {
    const isActive = c.key === f.sort;
    const arrow = isActive
      ? (f.dir === "asc" ? "↑" : "↓")
      : `<span class="sort-arrow-placeholder">↕</span>`;
    return `<th class="sortable ${isActive ? "active" : ""}"
            data-sort-key="${c.key}"
            data-testid="changes-sort-${c.key}">
              ${c.label} <span class="sort-arrow">${arrow}</span>
            </th>`;
  }).join("");
  root.innerHTML = `
    <p class="muted small" style="margin-bottom:10px" data-testid="changes-branch-info">
      Merges on <code>${htmlEscape(branch)}</code> (newest first).
      Each row maps to a Goal via the <code>Refine Goal:</code> trailer in
      the commit message.
    </p>
    <table class="table changes-table mobile-card-table" data-testid="changes-table">
      <thead><tr>${sortHeads}<th></th></tr></thead>
      <tbody>
        ${changes.map((c) => `
          <tr data-commit="${htmlEscape(c.commit)}" data-goal-id="${htmlEscape(c.goal_id)}" data-testid="changes-row">
            <td class="muted small" data-label="When">${fmtTime(c.committed)}</td>
            <td data-label="Goal" data-testid="changes-goal-cell">${renderChangeGoalCell(c)}</td>
            <td data-label="Status" data-testid="changes-status-cell">${c.status ? `<span class="status-pill ${c.status}">${c.status}</span>` : `<span class="muted small">-</span>`}</td>
            <td data-label="Priority" data-testid="changes-priority-cell">${c.priority
              ? `<span class="priority-pill priority-${c.priority}">${c.priority}</span>`
              : `<span class="muted small">-</span>`}</td>
            <td class="muted small" data-label="Assignee" data-testid="changes-assignee-cell">${c.assignee ? htmlEscape(c.assignee) : "-"}</td>
            <td class="muted small" data-label="Merge commit" data-testid="changes-commit-cell"><code>${c.commit.slice(0, 10)}...</code></td>
            <td data-label="Actions"><button class="secondary" data-undo-commit="${htmlEscape(c.commit)}"
                       data-testid="changes-undo"
                       ${c.status === "cancelled" ? "disabled" : ""}>
              Undo
            </button></td>
          </tr>`).join("")}
      </tbody>
    </table>
    ${renderPaginationControls("changes", pageMeta, changes.length, "change")}
  `;
  bindPaginationControls(root, "changes", (page) =>
    updateChangesFilter({ page }));
  $$("[data-sort-key]", root).forEach((head) => {
    head.addEventListener("click", () => updateChangesSort(head.dataset.sortKey));
  });
  $$("[data-undo-commit]", root).forEach((btn) => {
    btn.addEventListener("click", async (e) => {
      e.stopPropagation();
      const commit = btn.dataset.undoCommit;
      const row = btn.closest("tr");
      const goalName = row?.querySelector("td:nth-child(2)")?.textContent?.trim() || "this Goal";
      const ok = await modalConfirm(
        `Undo implementation ${commit.slice(0, 10)}... for ${goalName}? ` +
        "Refine will reverse the approved implementation, reconcile the project, " +
        "and move the linked Goal to cancelled.",
        { title: "Undo Goal", okLabel: "Undo", cancelLabel: "Keep merge",
          danger: true },
      );
      if (!ok) return;
      await withButtonBusy(btn, "Undoing...", async () => {
        try {
          const r = await api("POST", "/api/changes/undo", { commit });
          if (r.ok) {
            // Surface the push-failed-but-revert-succeeded case
            // prominently — the local state is ahead of remote and
            // the user needs to push manually.
            if (r.push_warning) {
              toast(r.push_warning, "error");
            } else {
              toast(`Undone${r.pushed ? " and pushed" : ""}`, "info");
            }
            await loadChanges();
          } else {
            toast(r.message || "Undo failed", "error");
          }
        } catch (e) { await showActionError(e); }
      });
    });
  });
}

function syncChangesPeriodControls(period = CHANGES_DEFAULT_PERIOD) {
  $$("[data-changes-period]").forEach((btn) => {
    const active = btn.dataset.changesPeriod === period;
    btn.classList.toggle("active", active);
    btn.setAttribute("aria-pressed", active ? "true" : "false");
  });
}

function changeBucketLabel(datetime, period) {
  const date = new Date(datetime);
  if (Number.isNaN(date.getTime())) return "Unknown";
  const year = date.getUTCFullYear();
  const month = String(date.getUTCMonth() + 1).padStart(2, "0");
  const day = String(date.getUTCDate()).padStart(2, "0");
  if (period === "year") return String(year);
  if (period === "month") return `${year}-${month}`;
  if (period === "week") {
    const weekStart = new Date(Date.UTC(year, date.getUTCMonth(), date.getUTCDate()));
    weekStart.setUTCDate(weekStart.getUTCDate() - weekStart.getUTCDay());
    const weekYear = weekStart.getUTCFullYear();
    const weekMonth = String(weekStart.getUTCMonth() + 1).padStart(2, "0");
    const weekDay = String(weekStart.getUTCDate()).padStart(2, "0");
    return `${weekYear}-${weekMonth}-${weekDay}`;
  }
  return `${year}-${month}-${day}`;
}

function drawChangesVisualization(changes, period = CHANGES_DEFAULT_PERIOD) {
  const root = $("#changes-visualization");
  if (!root) return;
  const buckets = new Map();
  (changes || []).forEach((change) => {
    const label = changeBucketLabel(change.committed, period);
    if (!buckets.has(label)) {
      buckets.set(label, { label, total: 0, linked: 0 });
    }
    const bucket = buckets.get(label);
    bucket.total += 1;
    if (change.goal_id) bucket.linked += 1;
  });
  const rows = Array.from(buckets.values()).sort((a, b) => b.label.localeCompare(a.label));
  if (!rows.length) {
    root.innerHTML = `<p class="muted" data-testid="changes-visualization-empty">No Git changes to visualize.</p>`;
    return;
  }
  const maxTotal = Math.max(1, ...rows.map((row) => row.total));
  root.innerHTML = `
    <section class="logs-visualization-grid changes-visualization-grid" data-testid="changes-visualization-grid">
      ${rows.map((row) => {
        const width = Math.max(8, Math.round((row.total / maxTotal) * 100));
        return `
          <div class="card logs-visualization-bucket changes-visualization-bucket" data-testid="changes-bucket">
            <strong class="changes-bucket-label" data-testid="changes-bucket-label">${htmlEscape(row.label)}</strong>
            <span class="muted small changes-bucket-total" data-testid="changes-bucket-total">${row.total} ${row.total === 1 ? "change" : "changes"}</span>
            <div class="logs-visualization-bar changes-visualization-bar" aria-hidden="true">
              <span class="info" style="width:${width}%"></span>
            </div>
            <span class="logs-visualization-counts changes-bucket-linked" data-testid="changes-bucket-linked">${row.linked} linked ${row.linked === 1 ? "Goal" : "Goals"}</span>
          </div>`;
      }).join("")}
    </section>`;
}

function renderChangeGoalCell(change = {}) {
  const goalId = String(change.goal_id || "").trim();
  const name = String(change.name || "").trim();
  const label = name || (goalId ? `Goal ${goalId}` : "Unlinked Goal");
  if (!goalId) return `<span class="muted">${htmlEscape(label)}</span>`;
  return `<a href="#/goals/${htmlEscape(goalId)}" ${name ? "" : `class="muted"`}>${htmlEscape(label)}</a>`;
}

function applyChangesFilterIndicator(f) {
  const active = {
    "changes-q": !!f.q,
    "changes-status": !!f.status,
    "changes-priority": !!f.priority,
    "changes-limit": f.limit !== CHANGES_DEFAULT_LIMIT,
  };
  let anyActive = false;
  for (const [id, on] of Object.entries(active)) {
    const el = document.getElementById(id);
    if (!el) continue;
    el.classList.toggle("filter-active", on);
    if (on) anyActive = true;
  }
  const pill = $("#changes-filtered");
  if (pill) pill.hidden = !anyActive;
  const list = $("#changes-body");
  if (list) list.classList.toggle("results-filtered", anyActive);
}
