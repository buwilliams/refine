// ---- Missions: list ---------------------------------------------------------

const MISSIONS_DEFAULT_LIMIT = 50;
const MISSIONS_LIMIT_OPTIONS = [50, 100, 250, 500, 1000];
const MISSIONS_DEFAULT_DIR = {
  name: "asc", status: "asc", reporter: "asc", assignee: "asc", updated: "desc", id: "desc",
};
const MISSIONS_STATUS_OPTIONS = [
  "", "draft", "investigate", "plan", "execute", "synthesize", "quality",
  "governance", "review", "consolidate", "done", "failed", "cancelled",
];

let _missionsPage = { rows: [], sort: "", dir: "" };

function missionsHash(parts) {
  const next = new URLSearchParams();
  if (parts.q) next.set("q", parts.q);
  if (parts.status) next.set("status", parts.status);
  if (parts.reporter) next.set("reporter", parts.reporter);
  if (parts.assignee) next.set("assignee", parts.assignee);
  if (parts.outcome) next.set("outcome", parts.outcome);
  if (parts.limit && parts.limit !== MISSIONS_DEFAULT_LIMIT) next.set("limit", String(parts.limit));
  if (parts.page && parts.page > 1) next.set("page", String(parts.page));
  if (parts.sort) next.set("sort", parts.sort);
  if (parts.dir) next.set("dir", parts.dir);
  return "#/missions" + (next.toString() ? "?" + next : "");
}

function missionsFilterFromHash() {
  const hashQs = new URLSearchParams(location.hash.split("?")[1] || "");
  const sort = (hashQs.get("sort") || "").toLowerCase();
  const dir = (hashQs.get("dir") || "").toLowerCase();
  const effectiveSort = sort || "updated";
  const effectiveDir = dir || (MISSIONS_DEFAULT_DIR[effectiveSort] || "desc");
  return {
    q: hashQs.get("q") || "",
    status: hashQs.get("status") || "",
    reporter: hashQs.get("reporter") || "",
    assignee: hashQs.get("assignee") || "",
    outcome: hashQs.get("outcome") || "",
    limit: parseInt(hashQs.get("limit") || String(MISSIONS_DEFAULT_LIMIT), 10)
           || MISSIONS_DEFAULT_LIMIT,
    page: Math.max(1, parseInt(hashQs.get("page") || "1", 10) || 1),
    sort, dir, effectiveSort, effectiveDir,
  };
}

function missionStatusLabel(status) {
  const labels = {
    draft: "Draft", investigate: "Investigate", plan: "Plan", execute: "Execute",
    synthesize: "Synthesize", quality: "Quality", governance: "Governance",
    review: "Review", consolidate: "Consolidate", done: "Done",
    failed: "Failed", cancelled: "Cancelled",
  };
  return labels[status] || status || "Draft";
}

async function renderMissionsList() {
  if (renderNoProjectIfDetached("Missions")) return;
  renderBanners([]);
  const f = missionsFilterFromHash();
  $("#main").innerHTML = `
    <div class="page-title-row">
      <h2>Missions</h2>
    </div>
    <details class="filter-shell" id="missions-filter-shell" data-testid="missions-filter-shell">
      <summary data-testid="missions-filter-summary">
        <span class="filter-shell-title">Filters</span>
        <span class="spacer"></span>
        <span class="muted small"><span id="missions-count" data-testid="missions-count"></span></span>
        <span id="missions-filtered" class="filter-pill" data-testid="missions-filtered-pill" hidden>Filtered</span>
      </summary>
      <div class="filter-shell-body">
        <div class="filter-bar">
          <div class="filter-row filter-row-primary">
            <input type="text" id="missions-search" class="filter-grow"
                   data-testid="missions-search"
                   placeholder="Search missions..." value="${htmlEscape(f.q)}">
          </div>
          <div class="filter-row">
            <select id="missions-status" data-testid="missions-status-filter">
              ${MISSIONS_STATUS_OPTIONS.map((s) =>
                `<option value="${s}" ${s === f.status ? "selected" : ""}>${s ? missionStatusLabel(s) : "all statuses"}</option>`).join("")}
            </select>
            <select id="missions-reporter" data-testid="missions-reporter-filter">
              <option value="" ${f.reporter === "" ? "selected" : ""}>all reporters</option>
              ${(state.reporters || []).map((r) =>
                `<option value="${htmlEscape(r.name)}" ${r.name === f.reporter ? "selected" : ""}>${htmlEscape(r.name)}</option>`).join("")}
              ${f.reporter && !(state.reporters || []).some((r) => r.name === f.reporter)
                ? `<option value="${htmlEscape(f.reporter)}" selected>${htmlEscape(f.reporter)}</option>` : ""}
            </select>
            <select id="missions-assignee" data-testid="missions-assignee-filter">
              <option value="" ${f.assignee === "" ? "selected" : ""}>all assignees</option>
              ${(state.reporters || []).map((r) =>
                `<option value="${htmlEscape(r.name)}" ${r.name === f.assignee ? "selected" : ""}>${htmlEscape(r.name)}</option>`).join("")}
              ${f.assignee && !(state.reporters || []).some((r) => r.name === f.assignee)
                ? `<option value="${htmlEscape(f.assignee)}" selected>${htmlEscape(f.assignee)}</option>` : ""}
            </select>
            <select id="missions-outcome" data-testid="missions-outcome-filter">
              <option value="" ${f.outcome === "" ? "selected" : ""}>any outcome</option>
              <option value="published" ${f.outcome === "published" ? "selected" : ""}>published</option>
              <option value="unpublished" ${f.outcome === "unpublished" ? "selected" : ""}>unpublished</option>
            </select>
            <select id="missions-limit" data-testid="missions-limit-filter">
              ${MISSIONS_LIMIT_OPTIONS.map((n) =>
                `<option value="${n}" ${n === f.limit ? "selected" : ""}>${n} entries</option>`).join("")}
            </select>
            <span class="spacer"></span>
            <button class="secondary" id="missions-clear" data-testid="missions-clear-filters">Clear filters</button>
          </div>
        </div>
      </div>
    </details>
    <div id="missions-table" data-testid="missions-table"><p class="muted">Loading...</p></div>
  `;
  bindOnce($("#missions-search"), "input", debounce((e) =>
    updateMissionsFilter({ q: e.target.value, page: 1 }), 250));
  bindOnce($("#missions-status"), "change", (e) =>
    updateMissionsFilter({ status: e.target.value, page: 1 }));
  bindOnce($("#missions-reporter"), "change", (e) =>
    updateMissionsFilter({ reporter: e.target.value, page: 1 }));
  bindOnce($("#missions-assignee"), "change", (e) =>
    updateMissionsFilter({ assignee: e.target.value, page: 1 }));
  bindOnce($("#missions-outcome"), "change", (e) =>
    updateMissionsFilter({ outcome: e.target.value, page: 1 }));
  bindOnce($("#missions-limit"), "change", (e) =>
    updateMissionsFilter({ limit: parseInt(e.target.value, 10) || MISSIONS_DEFAULT_LIMIT, page: 1 }));
  bindOnce($("#missions-clear"), "click", () => {
    history.replaceState(null, "", "#/missions");
    renderMissionsList();
  });
  await refreshMissionsTable();
}

function updateMissionsFilter(patch) {
  const current = missionsFilterFromHash();
  const next = {
    q: "q" in patch ? patch.q : current.q,
    status: "status" in patch ? patch.status : current.status,
    reporter: "reporter" in patch ? patch.reporter : current.reporter,
    assignee: "assignee" in patch ? patch.assignee : current.assignee,
    outcome: "outcome" in patch ? patch.outcome : current.outcome,
    limit: "limit" in patch ? patch.limit : current.limit,
    page: "page" in patch ? patch.page : current.page,
    sort: "sort" in patch ? patch.sort : current.sort,
    dir: "dir" in patch ? patch.dir : current.dir,
  };
  history.replaceState(null, "", missionsHash(next));
  refreshMissionsTable();
}

async function refreshMissionsTable() {
  if (state.currentRoute !== "missions") return;
  const nodeGeneration = captureNodeContextGeneration();
  const f = missionsFilterFromHash();
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries({
    q: f.q, status: f.status, reporter: f.reporter, assignee: f.assignee, outcome: f.outcome,
    limit: f.limit, offset: (f.page - 1) * f.limit,
    sort: f.sort, dir: f.dir,
  })) {
    if (value !== "" && value != null) params.set(key, String(value));
  }
  const data = await api("GET", `/api/missions?${params}`);
  if (!isNodeContextGenerationCurrent(nodeGeneration) || state.currentRoute !== "missions") return;
  const renderState = { ...f, pageMeta: data.page || {} };
  drawMissionsTable(data.missions || [], renderState);
}

function drawMissionsTable(missions, stateForRender) {
  const root = $("#missions-table");
  _missionsPage = {
    rows: missions,
    sort: stateForRender.effectiveSort,
    dir: stateForRender.effectiveDir,
  };
  const page = stateForRender.pageMeta || {};
  const total = page.total ?? ((page.offset || 0) + missions.length + (page.has_more ? 1 : 0));
  $("#missions-count").textContent = `${total} mission${total === 1 ? "" : "s"}`;
  $("#missions-filtered").hidden = !(
    stateForRender.q || stateForRender.status || stateForRender.reporter || stateForRender.assignee || stateForRender.outcome
  );
  if (!missions.length) {
    renderInto(root, `
      <p class="muted">No Missions match the current filters.</p>
      ${renderPaginationControls("missions", page, 0, "mission")}`);
    bindPaginationControls(root, "missions", (pageNo) =>
      updateMissionsFilter({ page: pageNo }));
    return;
  }
  const rows = missions.map((mission) => {
    const criteria = mission.criteria_summary || {};
    const criteriaText = criteria.total
      ? `${criteria.met || 0}/${criteria.total} met`
      : "—";
    return `
    <tr data-mission-id="${htmlEscape(mission.id)}" data-testid="missions-row">
      <td class="work-item-name-cell missions-name-cell" data-label="Mission">${htmlEscape(mission.name || "Untitled Mission")}</td>
      <td class="missions-status-cell" data-label="Status"><span class="status-pill ${htmlEscape(mission.status || "draft")}">${missionStatusLabel(mission.status)}</span></td>
      <td class="muted small" data-label="Round">${mission.current_round ? `#${mission.current_round}` : "—"}</td>
      <td class="muted small" data-label="Wave">${mission.current_wave ? `#${mission.current_wave}` : "—"}</td>
      <td class="muted small" data-label="Criteria">${criteriaText}</td>
      <td class="muted small" data-label="Reporter">${htmlEscape(mission.reporter || "—")}</td>
      <td class="muted small" data-label="Assignee">${htmlEscape(mission.assignee || "—")}</td>
      <td class="muted small" data-label="Outcome">${mission.outcome_available ? "Published" : "—"}</td>
      <td class="muted small" data-label="Updated">${fmtTime(mission.updated)}</td>
    </tr>`;
  }).join("");
  renderInto(root, `
    <div class="table-scroll">
      <table class="table work-items-table missions-table mobile-card-table">
        <colgroup>
          <col class="work-item-name-col missions-col-name">
          <col class="missions-col-status">
          <col class="missions-col-round">
          <col class="missions-col-wave">
          <col class="missions-col-criteria">
          <col class="missions-col-reporter">
          <col class="missions-col-assignee">
          <col class="missions-col-outcome">
          <col class="missions-col-updated">
        </colgroup>
        <thead><tr>
          ${missionSortHeader("name", "Mission", stateForRender)}
          ${missionSortHeader("status", "Status", stateForRender)}
          <th>Round</th>
          <th>Wave</th>
          <th>Criteria</th>
          ${missionSortHeader("reporter", "Reporter", stateForRender)}
          ${missionSortHeader("assignee", "Assignee", stateForRender)}
          <th>Outcome</th>
          ${missionSortHeader("updated", "Updated", stateForRender)}
        </tr></thead>
        <tbody>${rows}</tbody>
      </table>
    </div>
    ${renderPaginationControls("missions", page, missions.length, "mission")}
  `, () => {
    $$("#missions-table [data-sort]").forEach((th) => {
      bindOnce(th, "click", () => {
        const key = th.dataset.sort;
        const nextDir = _missionsPage.sort === key && _missionsPage.dir === "asc" ? "desc" : "asc";
        updateMissionsFilter({ sort: key, dir: nextDir, page: 1 });
      });
    });
    $$("#missions-table tbody tr[data-mission-id]").forEach((row) => {
      bindOnce(row, "click", (e) => {
        if (e.target.closest("a, button, input, select, textarea")) return;
        location.hash = `#/missions/${encodeURIComponent(row.dataset.missionId)}`;
      });
    });
    bindPaginationControls($("#missions-table"), "missions", (pageNo) =>
      updateMissionsFilter({ page: pageNo }));
  });
}

function missionSortHeader(key, label, stateForRender) {
  const active = stateForRender.effectiveSort === key;
  const dir = active ? stateForRender.effectiveDir : (MISSIONS_DEFAULT_DIR[key] || "asc");
  const arrow = active
    ? (dir === "asc" ? "↑" : "↓")
    : `<span class="sort-arrow-placeholder">↕</span>`;
  return `<th class="sortable ${active ? "active" : ""}" data-sort="${key}" data-testid="missions-sort-${htmlEscape(key)}">
    ${htmlEscape(label)} <span class="sort-arrow">${arrow}</span>
  </th>`;
}
