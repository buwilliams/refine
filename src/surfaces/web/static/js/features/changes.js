// ---- Changes ----------------------------------------------------------------
//
// Lists refine merge commits on the configured merge target branch (or the
// host's current branch if no target is set). Each row links the commit
// to its Gap and offers an Undo button — Undo runs `git revert -m 1` on
// the merge commit, pushes if there's an upstream, and moves the Gap to
// `cancelled` with a log entry.

const CHANGES_LIMIT_OPTIONS = [50, 100, 250, 500, 1000];
const CHANGES_DEFAULT_LIMIT = 50;

function changesFiltersFromHash() {
  const hashQs = new URLSearchParams(location.hash.split("?")[1] || "");
  return {
    q: hashQs.get("q") || "",
    status: hashQs.get("status") || "",
    priority: hashQs.get("priority") || "",
    limit: parseInt(hashQs.get("limit") || String(CHANGES_DEFAULT_LIMIT), 10)
           || CHANGES_DEFAULT_LIMIT,
    page: Math.max(1, parseInt(hashQs.get("page") || "1", 10) || 1),
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
    <details class="filter-shell" id="changes-filter-shell"${filterShellOpen ? " open" : ""}>
      <summary>
        <span class="filter-shell-title">Filters</span>
        <span class="spacer"></span>
        <span class="muted small"><span id="changes-count"></span></span>
        <span id="changes-filtered" class="filter-pill" hidden>Filtered</span>
      </summary>
      <div class="filter-shell-body">
        <div class="filter-bar">
          <div class="filter-row filter-row-primary">
            <input type="text" id="changes-q"
                   class="filter-grow"
                   placeholder="Search gap, commit, or status..."
                   value="${htmlEscape(f.q)}">
          </div>
          <div class="filter-row filter-row-filters">
            <select id="changes-status">
              ${STATUS_FILTER_OPTIONS
                .map((s) => `<option value="${s}" ${s === f.status ? "selected" : ""}>${s ? workflowStatusLabel(s) : "all statuses"}</option>`).join("")}
            </select>
            <select id="changes-priority">
              <option value="" ${f.priority === "" ? "selected" : ""}>all priorities</option>
              <option value="high" ${f.priority === "high" ? "selected" : ""}>high</option>
              <option value="medium" ${f.priority === "medium" ? "selected" : ""}>medium</option>
              <option value="low" ${f.priority === "low" ? "selected" : ""}>low</option>
            </select>
            <select id="changes-limit">
              ${CHANGES_LIMIT_OPTIONS.map((n) =>
                `<option value="${n}" ${n === f.limit ? "selected" : ""}>${n} entries</option>`).join("")}
            </select>
            <span class="spacer"></span>
            <button class="secondary" id="changes-clear">Clear filters</button>
          </div>
        </div>
      </div>
    </details>
    <div id="changes-body"><p class="muted">Loading...</p></div>`;
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
  };
  history.replaceState(null, "", changesHashFromFilters(next));
  loadChanges();
}

async function loadChanges() {
  if (state.currentRoute !== "changes") return;
  if (renderNoProjectIfDetached("Changes")) return;
  const f = changesFiltersFromHash();
  const params = new URLSearchParams();
  if (f.q) params.set("q", f.q);
  if (f.status) params.set("status", f.status);
  if (f.priority) params.set("priority", f.priority);
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
  if (!branch || branch === "(unknown)") {
    root.innerHTML = `
      <p class="muted">
        No merge target branch resolved. Set <code>merge_target_branch</code>
        in <a href="#/node/application">Node → Application</a>, or check that the host
        repo has a branch checked out.
      </p>`;
    return;
  }
  if (!changes.length) {
    root.innerHTML = `
      <p class="muted">
        ${f.q || f.status || f.priority
          ? `No changes match the current filters on <code>${htmlEscape(branch)}</code>.`
          : `No refine merges on <code>${htmlEscape(branch)}</code> yet. When the Merge agent lands a Gap, its merge commit shows up here.`}
      </p>
      ${renderPaginationControls("changes", pageMeta, 0, "change")}`;
    bindPaginationControls(root, "changes", (page) =>
      updateChangesFilter({ page }));
    return;
  }
  root.innerHTML = `
    <p class="muted small" style="margin-bottom:10px">
      Merges on <code>${htmlEscape(branch)}</code> (newest first).
      Each row maps to a Gap via the <code>Refine Gap:</code> trailer in
      the commit message.
    </p>
    <table class="table changes-table mobile-card-table">
      <thead><tr>
        <th>When</th>
        <th>Gap</th>
        <th>Status</th>
        <th>Priority</th>
        <th>Merge commit</th>
        <th></th>
      </tr></thead>
      <tbody>
        ${changes.map((c) => `
          <tr data-commit="${htmlEscape(c.commit)}" data-gap-id="${htmlEscape(c.gap_id)}">
            <td class="muted small" data-label="When">${fmtTime(c.committed)}</td>
            <td data-label="Gap">${renderChangeGapCell(c)}</td>
            <td data-label="Status">${c.status ? `<span class="status-pill ${c.status}">${c.status}</span>` : `<span class="muted small">-</span>`}</td>
            <td data-label="Priority">${c.priority
              ? `<span class="priority-pill priority-${c.priority}">${c.priority}</span>`
              : `<span class="muted small">-</span>`}</td>
            <td class="muted small" data-label="Merge commit"><code>${c.commit.slice(0, 10)}...</code></td>
            <td data-label="Actions"><button class="secondary" data-undo-commit="${htmlEscape(c.commit)}"
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
  $$("[data-undo-commit]", root).forEach((btn) => {
    btn.addEventListener("click", async (e) => {
      e.stopPropagation();
      const commit = btn.dataset.undoCommit;
      const row = btn.closest("tr");
      const gapName = row?.querySelector("td:nth-child(2)")?.textContent?.trim() || "this Gap";
      const ok = await modalConfirm(
        `Revert the merge commit ${commit.slice(0, 10)}... for ${gapName}? ` +
        "Refine will run `git revert -m 1`, push to the upstream if one " +
        "exists, and move the Gap to `cancelled`. The original commits " +
        "stay in history; the revert is a new commit on top.",
        { title: "Undo Gap", okLabel: "Undo", cancelLabel: "Keep merge",
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

function renderChangeGapCell(change = {}) {
  const gapId = String(change.gap_id || "").trim();
  const name = String(change.name || "").trim();
  const label = name || (gapId ? `Gap ${gapId}` : "Unlinked Gap");
  if (!gapId) return `<span class="muted">${htmlEscape(label)}</span>`;
  return `<a href="#/gaps/${htmlEscape(gapId)}" ${name ? "" : `class="muted"`}>${htmlEscape(label)}</a>`;
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
