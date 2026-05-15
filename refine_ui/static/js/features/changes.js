// ---- Changes ----------------------------------------------------------------
//
// Lists refine merge commits on the configured merge target branch (or the
// host's current branch if no target is set). Each row links the commit
// to its Gap and offers an Undo button — Undo runs `git revert -m 1` on
// the merge commit, pushes if there's an upstream, and moves the Gap to
// `cancelled` with a log entry.

async function renderChanges() {
  // First-paint scaffold only; SSE handlers call `loadChanges` directly
  // so the table redraws in place without a `Loading…` flash.
  renderBanners([]);
  if (!document.getElementById("changes-body")) {
    $("#main").innerHTML = `<h2>Changes</h2><div id="changes-body"><p class="muted">Loading…</p></div>`;
  }
  await loadChanges();
}

async function loadChanges() {
  if (state.currentRoute !== "changes") return;
  try {
    const data = await api("GET", "/api/changes");
    drawChanges(data);
  } catch (e) {
    const root = document.getElementById("changes-body");
    if (root) root.innerHTML = `<p class="muted">${htmlEscape(e.message)}</p>`;
  }
}

function drawChanges(data) {
  const root = document.getElementById("changes-body");
  // Guard against a late SSE refresh after the user navigated away.
  if (!root) return;
  const branch = data.branch || "(unknown)";
  const changes = data.changes || [];
  if (!branch || branch === "(unknown)") {
    root.innerHTML = `
      <p class="muted">
        No merge target branch resolved. Set <code>merge_target_branch</code>
        in <a href="#/settings">Settings → Scope</a>, or check that the host
        repo has a branch checked out.
      </p>`;
    return;
  }
  if (!changes.length) {
    root.innerHTML = `
      <p class="muted">
        No refine merges on <code>${htmlEscape(branch)}</code> yet. When a
        Gap moves <em>review → done</em>, its merge commit shows up here.
      </p>`;
    return;
  }
  root.innerHTML = `
    <p class="muted small" style="margin-bottom:10px">
      Merges on <code>${htmlEscape(branch)}</code> (newest first).
      Each row maps to a Gap via the <code>Refine Gap:</code> trailer in
      the commit message.
    </p>
    <table class="table">
      <thead><tr>
        <th>When</th>
        <th>Gap</th>
        <th>Status</th>
        <th>Merge commit</th>
        <th></th>
      </tr></thead>
      <tbody>
        ${changes.map((c) => `
          <tr data-commit="${htmlEscape(c.commit)}" data-gap-id="${htmlEscape(c.gap_id)}">
            <td class="muted small">${fmtTime(c.committed)}</td>
            <td>${c.name
              ? `<a href="#/gaps/${htmlEscape(c.gap_id)}">${htmlEscape(c.name)}</a>`
              : `<a href="#/gaps/${htmlEscape(c.gap_id)}" class="muted">(deleted)</a>`}</td>
            <td>${c.status ? `<span class="status-pill ${c.status}">${c.status}</span>` : `<span class="muted small">—</span>`}</td>
            <td class="muted small"><code>${c.commit.slice(0, 10)}…</code></td>
            <td><button class="secondary" data-undo-commit="${htmlEscape(c.commit)}"
                       ${c.status === "cancelled" ? "disabled" : ""}>
              Undo
            </button></td>
          </tr>`).join("")}
      </tbody>
    </table>
  `;
  $$("[data-undo-commit]", root).forEach((btn) => {
    btn.addEventListener("click", async (e) => {
      e.stopPropagation();
      const commit = btn.dataset.undoCommit;
      const row = btn.closest("tr");
      const gapName = row?.querySelector("td:nth-child(2)")?.textContent?.trim() || "this Gap";
      const ok = await modalConfirm(
        `Revert the merge commit ${commit.slice(0, 10)}… for ${gapName}? ` +
        "Refine will run `git revert -m 1`, push to the upstream if one " +
        "exists, and move the Gap to `cancelled`. The original commits " +
        "stay in history; the revert is a new commit on top.",
        { title: "Undo Gap", okLabel: "Undo", cancelLabel: "Keep merge",
          danger: true },
      );
      if (!ok) return;
      await withButtonBusy(btn, "Undoing…", async () => {
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
        } catch (e) { toast(e.message, "error"); }
      });
    });
  });
}
