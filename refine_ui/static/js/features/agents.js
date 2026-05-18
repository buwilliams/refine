// ---- Agents -----------------------------------------------------------------

async function renderAgents() {
  // First paint only — same flicker-avoidance pattern as renderDashboard.
  // SSE handlers route through `refreshAgents` instead so live updates
  // don't reset the screen to `Loading…`.
  renderBanners([]);
  if (!document.getElementById("agents-content")) {
    $("#main").innerHTML = `
      <h2>Agents</h2>
      <div id="agents-content"><p class="muted">Loading…</p></div>
    `;
  }
  await refreshAgents();
}

async function refreshAgents() {
  if (state.currentRoute !== "agents") return;
  try {
    const [dash, settings] = await Promise.all([
      api("GET", "/api/dashboard"),
      api("GET", "/api/settings"),
    ]);
    drawAgents(dash, settings.settings);
  } catch (e) {
    const c = document.getElementById("agents-content");
    if (c) c.innerHTML = `<p class="muted">${htmlEscape(e.message)}</p>`;
  }
}

function drawAgents(dash, settings) {
  const paused = settings.paused === "1";
  const root = document.getElementById("agents-content");
  if (!root) return;  // late SSE refresh after navigation away
  const merger = dash.merger || null;
  const governance = dash.governance || null;
  const agents = dash.running || [];
  const mergerActive = !!(merger && merger.state === "merging" && merger.gap_id);
  const governanceActive = !!(governance && governance.state === "reviewing" && governance.gap_id);
  const mergerQueued = merger?.queued || 0;
  const governanceQueued = governance?.queued || 0;
  const hasWork = mergerActive || governanceActive || agents.length > 0;
  const anchorMs = Date.now();
  const mergerRow = mergerActive ? `
    <tr class="merger-row">
      <td>
        <span class="role-pill merger">merger</span>
        <a href="#/gaps/${htmlEscape(merger.gap_id)}">${htmlEscape(merger.gap_id.slice(0, 10))}…</a>
      </td>
      <td class="js-elapsed-tick"
          data-base="${merger.elapsed_seconds || 0}"
          data-anchor-ms="${anchorMs}">${fmtElapsed(merger.elapsed_seconds || 0)}</td>
      <td class="muted small">—</td>
      <td><span class="muted small">verifying merge</span></td>
    </tr>` : "";
  const governanceRow = governanceActive ? `
    <tr class="governance-row">
      <td>
        <span class="role-pill merger">governance</span>
        <a href="#/gaps/${htmlEscape(governance.gap_id)}">${htmlEscape(governance.gap_id.slice(0, 10))}…</a>
      </td>
      <td class="js-elapsed-tick"
          data-base="${governance.elapsed_seconds || 0}"
          data-anchor-ms="${anchorMs}">${fmtElapsed(governance.elapsed_seconds || 0)}</td>
      <td class="muted small">—</td>
      <td><span class="muted small">reviewing governance</span></td>
    </tr>` : "";
  const agentRows = agents.map((r) => `
    <tr>
      <td>
        <span class="role-pill agent">agent</span>
        <a href="#/gaps/${htmlEscape(r.gap_id)}">${htmlEscape(r.gap_id.slice(0, 10))}…</a>
      </td>
      <td class="js-elapsed-tick"
          data-base="${r.elapsed_seconds}"
          data-anchor-ms="${anchorMs}">${fmtElapsed(r.elapsed_seconds)}</td>
      <td class="js-idle-tick"
          data-base="${r.idle_seconds}"
          data-anchor-ms="${anchorMs}">${fmtElapsed(r.idle_seconds)}</td>
      <td><button class="danger" data-cancel="${r.gap_id}">Cancel</button></td>
    </tr>`).join("");
  // Footer line below the table: surface merger queue depth even when
  // the merger isn't currently working on anything, so the operator
  // can see how much is waiting on the host worktree lock.
  const queueLine = mergerQueued > 0
    ? `<p class="muted small" style="margin-top:8px">Merger queue: ${mergerQueued} Gap${mergerQueued === 1 ? "" : "s"} waiting.</p>`
    : (merger
        ? `<p class="muted small" style="margin-top:8px">Merger: ${merger.state}${merger.last_outcome ? ` · last outcome <code>${htmlEscape(merger.last_outcome)}</code>` : ""}.</p>`
        : "");
  const mergerUnreachable = !merger
    ? `<p class="muted small" style="margin-top:8px">Merger state unavailable — backend runner unavailable.</p>`
    : "";
  const governanceLine = governance
    ? `<p class="muted small" style="margin-top:8px">Governance: ${governance.configured ? governance.state : "not configured"}${governanceQueued ? ` · queue ${governanceQueued}` : ""}${governance.last_outcome ? ` · last outcome <code>${htmlEscape(governance.last_outcome)}</code>` : ""}.</p>`
    : "";
  root.innerHTML = `
    <div class="card">
      <h3>Agent spawning &amp; merger</h3>
      <div class="actions">
        <button id="btn-pause" class="${paused ? "" : "secondary"}">
          ${paused ? "Resume" : "Pause"} agents
        </button>
        <span class="muted small">
          ${paused
            ? "Paused — agent subprocesses are stopped, new subprocesses won't launch, and the merger won't pick up new merges."
            : "Active — new subprocesses launch on demand and the merger processes Gaps as they finish."}
        </span>
      </div>
      <p class="muted small" style="margin-top:8px">
        The merger is a single-threaded worker that owns the host
        worktree, cleans up any half-finished git operation, and merges
        <code>ready-merge</code> Gaps one at a time so concurrent agent
        runs can't race on <code>git merge</code>. Runtime limits and
        backend diagnostics live on the <a href="#/settings">Settings</a> page.
      </p>
    </div>

    <div class="card" style="margin-top:16px">
      <h3>Currently running</h3>
      ${hasWork ? `
        <table class="table">
          <thead><tr><th>Worker</th><th>Elapsed</th><th>Idle</th><th></th></tr></thead>
          <tbody>${governanceRow}${mergerRow}${agentRows}</tbody>
        </table>` : `<p class="muted">Nothing running.</p>`}
      ${governanceLine}
      ${queueLine}
      ${mergerUnreachable}
    </div>

    <div class="card" style="margin-top:16px">
      <h3>Recent activity</h3>
      <div class="card-scroll">
        ${renderActivityList(dash.activity || [])}
      </div>
    </div>
  `;
  $("#btn-pause").addEventListener("click", async () => {
    try {
      await api("PATCH", "/api/settings", { paused: paused ? "0" : "1" });
      await refreshAgents();
    } catch (e) { toast(e.message, "error"); }
  });
  $$("[data-cancel]").forEach((b) => {
    b.addEventListener("click", async () => {
      const id = b.dataset.cancel;
      const ok = await modalConfirm(
        "Cancel this Gap's running subprocess?",
        { title: "Cancel run", okLabel: "Cancel run", danger: true,
          cancelLabel: "Keep running" },
      );
      if (!ok) return;
      try { await api("POST", `/api/gaps/${id}/cancel`); await refreshAgents(); }
      catch (e) { toast(e.message, "error"); }
    });
  });
}
