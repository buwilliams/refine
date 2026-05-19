// ---- System -----------------------------------------------------------------

async function renderSettings() {
  renderBanners([]);
  // First-paint scaffold only; subsequent refreshes route through
  // `refreshSettings` so SSE / post-save reloads don't flash `Loading…`.
  if (!document.getElementById("settings-content")) {
    $("#main").innerHTML = `<h2>System</h2><div id="settings-content"><p class="muted">Loading…</p></div>`;
  }
  await refreshSettings();
}

async function refreshSettings() {
  if (state.currentRoute !== "settings") return;
  try {
    const [s, diag, reps, feats, project, gov, dash, instances] = await Promise.all([
      api("GET", "/api/settings"),
      api("GET", "/api/diagnostics"),
      api("GET", "/api/reporters"),
      api("GET", "/api/features"),
      api("GET", "/api/project/status"),
      api("GET", "/api/governance"),
      api("GET", "/api/dashboard"),
      api("GET", "/api/instances"),
    ]);
    // Keep the cached matrix fresh so gates elsewhere react too.
    state.features = feats;
    state.project = project;
    drawSettings(s.settings || {}, diag, reps.reporters || [], feats, gov || {}, dash || {}, instances || {});
  } catch (e) {
    const root = document.getElementById("settings-content");
    if (root) root.innerHTML = `<p class="muted">${htmlEscape(e.message)}</p>`;
  }
}

const SETTINGS_TAB_STORAGE_KEY = "refine_settings_tab";
const SETTINGS_TABS = [
  { slug: "project",      label: "Project" },
  { slug: "application",  label: "Application" },
  { slug: "instances",    label: "Instances" },
  { slug: "reporters",    label: "Reporters" },
  { slug: "governance",   label: "Governance" },
  { slug: "runtime",      label: "Runtime" },
];

function normalizeSettingsTab(slug) {
  if (slug === "system") return "project";
  return SETTINGS_TABS.some((t) => t.slug === slug) ? slug : null;
}

function activeSettingsTabFromRoute() {
  const parsed = typeof parseHash === "function" ? parseHash() : {};
  return parsed.route === "settings" ? normalizeSettingsTab(parsed.tab) : null;
}

function readSettingsTab(tabs = SETTINGS_TABS) {
  const routed = activeSettingsTabFromRoute();
  if (routed) {
    localStorage.setItem(SETTINGS_TAB_STORAGE_KEY, routed);
    return routed;
  }
  const stored = localStorage.getItem(SETTINGS_TAB_STORAGE_KEY);
  const normalizedStored = normalizeSettingsTab(stored);
  if (normalizedStored && tabs.some((t) => t.slug === normalizedStored)) {
    if (normalizedStored !== stored) {
      localStorage.setItem(SETTINGS_TAB_STORAGE_KEY, normalizedStored);
    }
    return normalizedStored;
  }
  return tabs[0]?.slug;
}

function setSettingsTab(slug) {
  const normalized = normalizeSettingsTab(slug);
  if (!normalized) return;
  localStorage.setItem(SETTINGS_TAB_STORAGE_KEY, normalized);
  // Toggle classes immediately; hashchange handles normal linked tab
  // navigation, and this keeps repeated clicks on the current hash responsive.
  $$("[data-tab-pane]").forEach((pane) => {
    pane.classList.toggle("active", pane.dataset.tabPane === normalized);
  });
  $$(".settings-tab").forEach((btn) => {
    btn.classList.toggle("active", btn.dataset.tabTarget === normalized);
  });
}

function renderFeatureFlagsCard(feats) {
  if (!feats || !feats.features?.length) return "";
  const providers = feats.providers || [];
  const current = feats.current_provider;
  const cell = (provider, featureKey) => {
    const slot = feats.matrix?.[`${provider}.${featureKey}`] || {};
    const enabled = !!slot.enabled;
    const overridden = !!slot.override;
    const isCurrent = provider === current;
    return `
      <td class="${isCurrent ? "feature-current-col" : ""}">
        <label class="feature-toggle ${enabled ? "on" : "off"}"
               title="${overridden ? "Operator override" : "Default"}">
          <input type="checkbox"
                 data-feature-cell="${provider}.${featureKey}"
                 data-provider="${htmlEscape(provider)}"
                 data-feature="${htmlEscape(featureKey)}"
                 data-feature-default="${slot.default ? "1" : "0"}"
                 data-feature-original-enabled="${enabled ? "1" : "0"}"
                 data-feature-original-override="${overridden ? "1" : "0"}"
                 ${enabled ? "checked" : ""}>
          <span class="feature-toggle-state">${enabled ? "on" : "off"}</span>
        </label>
        ${overridden
          ? `<button class="link-button"
                     data-feature-clear="${provider}.${featureKey}"
                     data-provider="${htmlEscape(provider)}"
                     data-feature="${htmlEscape(featureKey)}"
                     type="button"
                     title="Clear override and use the code-defined default on save">
              clear override
            </button>`
          : ""}
      </td>`;
  };
  return `
    <section class="settings-section">
      <h3>Feature flags</h3>
      <p class="muted small" style="margin-top:0">
        Provider-scoped capability matrix. The current provider is
        <strong>${htmlEscape(current)}</strong>. Defaults are the
        code-defined set of features known to work; overriding a cell
        is experimental and may produce errors at runtime.
      </p>
      <table class="table">
        <thead><tr>
          <th>Feature</th>
          ${providers.map((p) => `
            <th class="${p === current ? "feature-current-col" : ""}">
              ${htmlEscape(p)}${p === current ? " (current)" : ""}
            </th>`).join("")}
        </tr></thead>
        <tbody>
          ${feats.features.map((f) => `
            <tr>
              <td>
                <div><strong>${htmlEscape(f.label)}</strong></div>
                <div class="muted small">${htmlEscape(f.description)}</div>
              </td>
              ${providers.map((p) => cell(p, f.key)).join("")}
            </tr>`).join("")}
        </tbody>
      </table>
      <p class="muted small" style="margin-top:8px">
        Feature flag changes are saved with Save runtime.
      </p>
    </section>`;
}

function updateFeatureToggleLabel(box) {
  const label = box.closest(".feature-toggle");
  const text = label?.querySelector(".feature-toggle-state");
  if (!label || !text) return;
  label.classList.toggle("on", box.checked);
  label.classList.toggle("off", !box.checked);
  text.textContent = box.checked ? "on" : "off";
}

function renderGovernanceRuleRows(rules) {
  const rows = (rules || []).map((rule) => `
    <div class="governance-rule-row">
      <input type="text" class="governance-rule-input"
             value="${htmlEscape(rule.text || "")}"
             data-rule-id="${htmlEscape(rule.id || "")}"
             data-created="${htmlEscape(rule.created || "")}"
             data-source="${htmlEscape(rule.source || "manual")}">
      <button class="danger" data-governance-remove-rule>Remove</button>
    </div>`).join("");
  return rows || `<p class="muted small" data-empty-governance-rules>No rules yet.</p>`;
}

function renderGovernanceRules(rules) {
  return `
    <div id="governance-rules-list">
      ${renderGovernanceRuleRows(rules)}
    </div>`;
}

function collectGovernanceRules() {
  return $$(".governance-rule-input").map((input) => ({
    id: input.dataset.ruleId || "",
    text: (input.value || "").trim(),
    created: input.dataset.created || "",
    source: input.dataset.source || "manual",
  })).filter((rule) => rule.text);
}

function bindGovernanceRuleButtons() {
  $$("[data-governance-remove-rule]").forEach((btn) => {
    btn.addEventListener("click", () => {
      btn.closest(".governance-rule-row")?.remove();
      if (!$(".governance-rule-input")) {
        $("#governance-rules-list").innerHTML = `<p class="muted small" data-empty-governance-rules>No rules yet.</p>`;
      }
    });
  });
}

function addGovernanceRuleRow(text = "") {
  const list = $("#governance-rules-list");
  if (!list) return;
  list.querySelector("[data-empty-governance-rules]")?.remove();
  const row = document.createElement("div");
  row.className = "governance-rule-row";
  row.innerHTML = `
    <input type="text" class="governance-rule-input"
           value="${htmlEscape(text)}"
           data-rule-id="" data-created="" data-source="manual">
    <button class="danger" data-governance-remove-rule>Remove</button>
  `;
  list.appendChild(row);
  row.querySelector("[data-governance-remove-rule]").addEventListener("click", () => {
    row.remove();
    if (!$(".governance-rule-input")) {
      list.innerHTML = `<p class="muted small" data-empty-governance-rules>No rules yet.</p>`;
    }
  });
  row.querySelector("input")?.focus();
}

function renderRuntimeAgentCards(dash, settings, diag) {
  const paused = settings.paused === "1";
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
        <a href="#/gaps/${htmlEscape(merger.gap_id)}">${htmlEscape(merger.gap_id.slice(0, 10))}...</a>
      </td>
      <td class="js-elapsed-tick"
          data-base="${merger.elapsed_seconds || 0}"
          data-anchor-ms="${anchorMs}">${fmtElapsed(merger.elapsed_seconds || 0)}</td>
      <td class="muted small">-</td>
      <td><span class="muted small">merging</span></td>
    </tr>` : "";
  const governanceRow = governanceActive ? `
    <tr class="governance-row">
      <td>
        <span class="role-pill merger">governance</span>
        <a href="#/gaps/${htmlEscape(governance.gap_id)}">${htmlEscape(governance.gap_id.slice(0, 10))}...</a>
      </td>
      <td class="js-elapsed-tick"
          data-base="${governance.elapsed_seconds || 0}"
          data-anchor-ms="${anchorMs}">${fmtElapsed(governance.elapsed_seconds || 0)}</td>
      <td class="muted small">-</td>
      <td><span class="muted small">reviewing governance</span></td>
    </tr>` : "";
  const agentRows = agents.map((r) => `
    <tr>
      <td>
        <span class="role-pill agent">agent</span>
        <a href="#/gaps/${htmlEscape(r.gap_id)}">${htmlEscape(r.gap_id.slice(0, 10))}...</a>
      </td>
      <td class="js-elapsed-tick"
          data-base="${r.elapsed_seconds}"
          data-anchor-ms="${anchorMs}">${fmtElapsed(r.elapsed_seconds)}</td>
      <td class="js-idle-tick"
          data-base="${r.idle_seconds}"
          data-anchor-ms="${anchorMs}">${fmtElapsed(r.idle_seconds)}</td>
      <td><button class="danger" data-cancel-agent="${r.gap_id}">Cancel</button></td>
    </tr>`).join("");
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
  return `
    <section class="settings-section">
      <h3>Agents</h3>
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
        The merger is a single-threaded worker that owns the host worktree,
        cleans up any half-finished git operation, and merges
        <code>ready-merge</code> Gaps one at a time so concurrent agent runs
        can't race on <code>git merge</code>.
      </p>

      <dl class="kv" style="margin-top:12px">
        <dt>Backend reachable</dt><dd>${diag.reachable ? "yes" : "no"}</dd>
        ${diag.mode ? `<dt>Backend mode</dt><dd>${htmlEscape(diag.mode)}</dd>` : ""}
        ${diag.last_call_at ? `<dt>Last backend call</dt><dd>${fmtTime(diag.last_call_at)}</dd>` : ""}
      </dl>

      <h4 style="margin:16px 0 8px">Currently running</h4>
      ${hasWork ? `
        <table class="table">
          <thead><tr><th>Worker</th><th>Elapsed</th><th>Idle</th><th></th></tr></thead>
          <tbody>${governanceRow}${mergerRow}${agentRows}</tbody>
        </table>` : `<p class="muted">Nothing running.</p>`}
      ${governanceLine}
      ${queueLine}
      ${mergerUnreachable}
    </section>`;
}

function drawSettings(s, diag, reps, feats, gov = {}, dash = {}, instanceData = {}) {
  const cli = (s.agent_cli || "claude").toLowerCase();
  const projectApps = state.project?.apps || [];
  const currentProject = state.project?.client_repo || "";
  const projectRegistryEnabled = state.project?.registry_enabled !== false;
  const instances = instanceData.instances || state.project?.instances || [];
  const activeInstanceId = instanceData.active_instance_id || state.project?.active_instance_id || "";
  const activeInstance = instances.find((i) => i.id === activeInstanceId) || null;
  const activeInstanceLabel = activeInstance?.display_name || activeInstanceId || "Default";
  const instanceCounts = instanceData.counts || {};
  const appOptions = projectApps.map((app) => `
    <option value="${htmlEscape(app.path)}" ${app.path === currentProject ? "selected" : ""}>
      ${htmlEscape(app.name || app.path)}
    </option>`).join("");
  const cliOption = (value, label) =>
    `<option value="${value}" ${cli === value ? "selected" : ""}>${htmlEscape(label)}</option>`;
  // Tab definitions. Order here drives the tab strip; `slug` is the
  // localStorage key, route segment, and DOM hook.
  const tabs = SETTINGS_TABS;
  const activeSlug = readSettingsTab(tabs);
  const tabStrip = `
    <nav class="settings-tabs" id="settings-tabs">
      ${tabs.map((t) => `
        <a class="settings-tab ${t.slug === activeSlug ? "active" : ""}"
           href="#/system/${t.slug}"
           data-tab-target="${t.slug}">${htmlEscape(t.label)}</a>`).join("")}
    </nav>`;
  const pane = (slug, body) => `
    <section class="settings-pane ${slug === activeSlug ? "active" : ""}"
             data-tab-pane="${slug}">
      <div class="card settings-tab-card">${body}</div>
    </section>`;
  $("#settings-content").innerHTML = `
    ${tabStrip}
    ${pane("project", `
    <section class="settings-section">
      <h3>Applications</h3>
      <p class="scope-label muted small">Project-wide</p>
      <p class="muted small">
        Current app: <code>${htmlEscape(state.project?.client_repo || "Not attached")}</code>
      </p>
      ${projectRegistryEnabled ? "" : `
        <p class="muted small" style="color:var(--warn)">
          App switching requires the host-native setup UI. Start from the refine
          source checkout with <code>uv run refine start</code> before a project
          is attached.
        </p>`}
      <div class="form-row"><label>Known apps
        <span class="muted small">— add an existing repo or a new directory, then switch between apps here.</span></label>
        <select id="s-project-select" ${projectApps.length ? "" : "disabled"}>
          ${appOptions || `<option value="">No apps yet</option>`}
        </select></div>
      <div class="actions">
        <button class="secondary" id="s-project-add" ${projectRegistryEnabled ? "" : "disabled"}>Add app</button>
        <button class="warn" id="s-project-switch" ${projectApps.length && projectRegistryEnabled ? "" : "disabled"}>Switch to selected</button>
        <button class="danger" id="s-project-remove" ${projectApps.length && projectRegistryEnabled ? "" : "disabled"}>Remove selected</button>
      </div>
    </section>`)}

    ${pane("application", `
    <section class="settings-section">
      <h3>Application</h3>
      <p class="scope-label muted small">Instance: ${htmlEscape(activeInstanceLabel)}</p>
    </section>

    <section class="settings-section">
      <h3>Scope</h3>
      <p class="muted small">
        Where refine's agent work lands inside the client repo. The base
        repo location still owns all git plumbing — worktree create, fetch,
        merge, push.
      </p>
      <div class="form-row"><label>Agent subpath
        <span class="muted small">— optional sub-project (relative to the repo root) used as the cwd for agent + chat subprocesses. Leave blank to use the repo root.</span></label>
        <input type="text" id="s-subpath"
               placeholder="e.g. apps/web"
               value="${htmlEscape(s.agent_subpath || "")}"></div>
      <div class="form-row"><label>Merge target branch
        <span class="muted small">— branch all Gap worktrees are based on and all Merge agent work lands on. Leave blank to follow the host's currently-checked-out branch. When set, the Merge agent auto-stashes WIP, switches HEAD, and restores the host's original branch afterward.</span></label>
        <input type="text" id="s-merge-target"
               placeholder="e.g. main"
               value="${htmlEscape(s.merge_target_branch || "")}"></div>
    </section>

    <section class="settings-section">
      <h3>Current status</h3>
      <div id="target-app-status-block" class="muted">Loading…</div>
      <div class="actions" style="margin-top:10px">
        <button id="s-target-run-start">Start application</button>
        <button class="secondary" id="s-target-run-rebuild">Rebuild application</button>
        <button class="danger" id="s-target-run-stop">Stop application</button>
        <span class="spacer"></span>
        <button class="secondary" id="s-target-health-now">Check status now</button>
      </div>
      <p class="muted small" style="margin-top:6px">
        Start / Stop live here on purpose — the indicator next to the
        reporter dropdown is read-only so typical users can't take the
        application down by accident.
      </p>
    </section>

    <section class="settings-section">
      <h3>Target application</h3>
      <p class="muted small" style="margin-top:0">
        The AI provider drafts this configuration from the codebase.
        Refine then runs the saved shell commands directly on the host
        and checks status through CLI / HTTP / TCP / process probes.
      </p>
      ${(s.target_app_start_instructions || s.target_app_stop_instructions || s.target_app_health_url) ? `
        <p class="muted small" style="color:var(--warn)">
          Legacy prose target-app settings are present. Generate from codebase
          to convert them into structured commands, then Save.
        </p>` : ""}
      <div class="actions" style="margin-bottom:10px">
        <button class="secondary" id="s-target-generate">Generate from codebase</button>
        <span class="muted small">Review generated fields before saving.</span>
      </div>
      <div class="form-row"><label>Start command
        <span class="muted small">— one-line shell command that starts the app and returns promptly.</span></label>
        <input type="text" id="s-target-start-command"
               placeholder="nohup npm run dev > /tmp/refine-target.log 2>&1 &"
               value="${htmlEscape(s.target_app_start_command || "")}"></div>
      <div class="form-row"><label>Stop command
        <span class="muted small">— one-line shell command that stops the app; should be idempotent when practical.</span></label>
        <input type="text" id="s-target-stop-command"
               placeholder="pkill -f 'npm run dev' || true"
               value="${htmlEscape(s.target_app_stop_command || "")}"></div>
      <div class="form-row"><label>Rebuild command
        <span class="muted small">— one-line shell command that prepares generated artifacts for review.</span></label>
        <input type="text" id="s-target-rebuild-command"
               placeholder="npm run build"
               value="${htmlEscape(s.target_app_rebuild_command || "")}"></div>
      <div class="form-row"><label>Status command
        <span class="muted small">— exit 0 only when the app is healthy or running.</span></label>
        <input type="text" id="s-target-status-command"
               placeholder="pgrep -f 'npm run dev' >/dev/null"
               value="${htmlEscape(s.target_app_status_command || "")}"></div>
      <div class="form-row"><label>Working directory
        <span class="muted small">— repo-relative path, or blank for repo root.</span></label>
        <input type="text" id="s-target-cwd"
               placeholder="."
               value="${htmlEscape(s.target_app_cwd || "")}"></div>
      <div class="form-row"><label>Environment overrides
        <span class="muted small">— JSON object merged into the host environment.</span></label>
        <textarea id="s-target-env" rows="3" placeholder='{"PORT":"3000"}'>${htmlEscape(s.target_app_env_json || "{}")}</textarea></div>
      <div class="form-grid two">
        <div class="form-row"><label>Start timeout (s)</label>
          <input type="number" id="s-target-start-timeout" value="${htmlEscape(s.target_app_start_timeout_seconds || "120")}"></div>
        <div class="form-row"><label>Stop timeout (s)</label>
          <input type="number" id="s-target-stop-timeout" value="${htmlEscape(s.target_app_stop_timeout_seconds || "60")}"></div>
        <div class="form-row"><label>Rebuild timeout (s)</label>
          <input type="number" id="s-target-rebuild-timeout" value="${htmlEscape(s.target_app_rebuild_timeout_seconds || "300")}"></div>
        <div class="form-row"><label>Status timeout (s)</label>
          <input type="number" id="s-target-status-timeout" value="${htmlEscape(s.target_app_status_timeout_seconds || "10")}"></div>
        <div class="form-row"><label>Log path</label>
          <input type="text" id="s-target-log-path" value="${htmlEscape(s.target_app_log_path || "")}"></div>
      </div>
      <h4 style="margin:16px 0 8px">Optional checks</h4>
      <div class="form-row"><label>HTTP check URL
        <span class="muted small">— optional; 2xx means healthy. Runs on the host.</span></label>
        <input type="text" id="s-target-http-url"
               placeholder="http://localhost:3000/health"
               value="${htmlEscape(s.target_app_http_check_url || s.target_app_health_url || "")}"></div>
      <div class="form-grid two">
        <div class="form-row"><label>TCP host</label>
          <input type="text" id="s-target-tcp-host" value="${htmlEscape(s.target_app_tcp_check_host || "")}"></div>
        <div class="form-row"><label>TCP port</label>
          <input type="number" id="s-target-tcp-port" value="${htmlEscape(s.target_app_tcp_check_port || "")}"></div>
      </div>
      <div class="form-row"><label>Process check command
        <span class="muted small">— optional one-line command; exit 0 when the expected process exists.</span></label>
        <input type="text" id="s-target-process-command"
               value="${htmlEscape(s.target_app_process_check_command || "")}"></div>
      <div class="form-row" id="s-target-notes-row" style="display:none"><label>Generated notes</label>
        <p class="muted small" id="s-target-notes"></p></div>
    </section>

    <section class="settings-section settings-save-section">
      <div class="actions"><button id="s-save-application">Save application</button></div>
    </section>`)}

    ${pane("runtime", `
    ${renderRuntimeAgentCards(dash, s, diag)}

    <section class="settings-section">
      <h3>Runtime configuration</h3>
      <p class="scope-label muted small">Instance: ${htmlEscape(activeInstanceLabel)}</p>
      <div class="form-row"><label>Parallel-run cap</label>
        <input type="number" id="s-cap" value="${s.parallel_run_cap || 3}"></div>
      <div class="form-row"><label>Branch name pattern</label>
        <input type="text" id="s-pattern" value="${htmlEscape(s.branch_name_pattern || "refine/{gap_id}")}"></div>
      <div class="form-row"><label>Agent idle timeout (seconds)</label>
        <input type="number" id="s-idle" value="${s.agent_idle_timeout_seconds || 900}"></div>
      <div class="form-row"><label>Agent hard cap (seconds)</label>
        <input type="number" id="s-hard" value="${s.agent_hard_cap_seconds || 86400}"></div>
      <div class="form-row"><label>Standalone chat idle timeout (seconds)
        <span class="muted small">— set to 0 to disable auto-close</span></label>
        <input type="number" id="s-chat-idle" value="${s.chat_idle_timeout_seconds || 300}"></div>
      <div class="form-row"><label>Auto-promote backlog → todo
        <span class="muted small">— how long a Gap may sit in backlog before the dispatcher moves it to todo. Default 1 hour.</span></label>
        <select id="s-backlog-promote">
          ${[
            ["-1",    "Never"],
            ["0",     "Instant"],
            ["300",   "5 minutes"],
            ["1800",  "30 minutes"],
            ["3600",  "1 hour"],
            ["10800", "3 hours"],
            ["21600", "6 hours"],
            ["86400", "24 hours"],
          ].map(([v, lbl]) => `<option value="${v}" ${String(s.backlog_promote_after_seconds ?? "3600") === v ? "selected" : ""}>${lbl}</option>`).join("")}
        </select></div>
    </section>

    <section class="settings-section">
      <h3>AI Provider</h3>
      <div class="form-row"><label>Which AI provider refine drives
        <span class="muted small">— used for Gap agent runs, conflict resolution, chat, import extraction, target-app actions, and pre-flight. Chat and Import are supported for Claude Code and Codex.</span></label>
        <select id="s-cli">
          ${cliOption("claude", "Claude Code (default)")}
          ${cliOption("codex", "OpenAI Codex")}
          ${cliOption("gemini", "Gemini")}
        </select></div>
      <p class="muted small" style="margin-top:6px">
        After switching: re-check auth below to confirm the chosen provider is
        installed and authed on the host. Round logs are structured for Claude
        Code and Codex where their CLIs expose machine-readable events; Gemini
        falls back to plain stdout passthrough.
      </p>
      <p class="muted" style="margin-top:14px">The selected provider's auth lives on the host. Use Re-check to re-run the pre-flight after running the relevant login command (<code>claude login</code> / <code>codex login</code> / <code>gemini auth login</code>).</p>
      <button id="s-recheck">Re-check auth</button>
    </section>

    ${renderFeatureFlagsCard(feats)
      || `<section class="settings-section"><p class="muted">Feature flag matrix unavailable — backend runner unavailable.</p></section>`}

    <section class="settings-section">
      <h3>Logs retention</h3>
      <p class="muted small">
        Delete activity entries older than the chosen window. Newer entries
        and gap state are untouched.
      </p>
      <div class="actions">
        <label for="logs-cleanup-days" class="muted small">Keep</label>
        <select id="logs-cleanup-days">
          ${[0, 7, 30, 60, 90, 365].map((n) =>
            `<option value="${n}" ${n === 7 ? "selected" : ""}>${n === 0 ? "0 (don't keep any)" : `${n} days`}</option>`).join("")}
        </select>
        <button class="danger" id="logs-cleanup">Clean up old logs</button>
      </div>
    </section>

    <section class="settings-section settings-save-section">
      <div class="actions"><button id="s-save-runtime">Save runtime</button></div>
    </section>`)}

    ${pane("governance", `
    <section class="settings-section">
      <h3>Product</h3>
      <p class="scope-label muted small">Project-wide</p>
      <p class="muted small" style="margin-top:0">
        The what and why: who the product is for, what problems it solves,
        and what success looks like.
      </p>
      <textarea id="s-governance-product" rows="7">${htmlEscape(gov.product || "")}</textarea>
    </section>

    <section class="settings-section">
      <h3>Constitution</h3>
      <p class="muted small" style="margin-top:0">
        Non-negotiable principles for the entire project.
      </p>
      <textarea id="s-governance-constitution" rows="7">${htmlEscape(gov.constitution || "")}</textarea>
    </section>

    <section class="settings-section">
      <h3>Rules</h3>
      <p class="muted small" style="margin-top:0">
        One-line rules the Governance agent applies before implementation.
      </p>
      ${gov.configured ? "" : `
        <p class="muted small" style="color:var(--warn)">
          Governance is incomplete. Gap execution continues until Product and Constitution are both filled in.
        </p>`}
      ${renderGovernanceRules(gov.rules || [])}
      <div class="actions" style="margin-top:10px">
        <button class="secondary" id="s-governance-add-rule">Add rule</button>
        <button class="secondary" id="s-governance-generate">Generate rules</button>
        <span class="spacer"></span>
        <button id="s-governance-save">Save governance</button>
      </div>
    </section>`)}

    ${pane("instances", `
    <section class="settings-section">
      <h3>Instances</h3>
      <p class="scope-label muted small">Project-wide</p>
      <table class="table">
        <thead><tr><th>Name</th><th>ID</th><th>Gaps</th><th></th></tr></thead>
        <tbody>
          ${instances.map((inst) => {
            const counts = instanceCounts[inst.id] || {};
            const total = Object.values(counts).reduce((a, b) => a + Number(b || 0), 0);
            const isActive = inst.id === activeInstanceId;
            return `<tr>
              <td>${htmlEscape(inst.display_name || inst.id)} ${isActive ? `<span class="filter-pill">active</span>` : ""}${inst.archived ? ` <span class="muted small">archived</span>` : ""}</td>
              <td><code>${htmlEscape(inst.id)}</code></td>
              <td class="muted small">${total}</td>
              <td class="actions">
                <button class="secondary" data-instance-activate="${htmlEscape(inst.id)}" ${isActive || inst.archived ? "disabled" : ""}>Activate</button>
                <button class="secondary" data-instance-rename="${htmlEscape(inst.id)}" data-name="${htmlEscape(inst.display_name || inst.id)}">Rename</button>
                <button class="danger" data-instance-archive="${htmlEscape(inst.id)}" ${isActive ? "disabled" : ""}>Archive</button>
              </td>
            </tr>`;
          }).join("")}
        </tbody>
      </table>
      <div class="actions" style="margin-top:8px">
        <button id="instance-add">Create instance</button>
      </div>
    </section>
    <section class="settings-section">
      <h3>Transfer Gaps</h3>
      <p class="muted small" style="margin-top:0">
        Transfers matching Gaps to another instance. If active work is present,
        Refine pauses agents, stops agent processes, cancels in-progress and
        ready-merge Gaps, then transfers them.
      </p>
      <div class="form-grid two">
        <div class="form-row"><label>From</label>
          <select id="instance-transfer-source">
            <option value="">All instances</option>
            ${instances.map((inst) => `<option value="${htmlEscape(inst.id)}">${htmlEscape(inst.display_name || inst.id)}</option>`).join("")}
          </select></div>
        <div class="form-row"><label>To</label>
          <select id="instance-transfer-target">
            ${instances.map((inst) => `<option value="${htmlEscape(inst.id)}" ${inst.id === activeInstanceId ? "selected" : ""}>${htmlEscape(inst.display_name || inst.id)}</option>`).join("")}
          </select></div>
      </div>
      <div class="actions"><button class="warn" id="instance-transfer">Transfer matching Gaps</button></div>
    </section>`)}

    ${pane("reporters", `
    <section class="settings-section">
      <h3>Reporters</h3>
      <p class="scope-label muted small">Instance: ${htmlEscape(activeInstanceLabel)}</p>
      <table class="table">
        <thead><tr><th>Name</th><th></th></tr></thead>
        <tbody>
          ${reps.map((r) => `<tr>
            <td>${htmlEscape(r.name)}</td>
            <td class="actions">
              <button class="secondary" data-rename="${r.id}" data-name="${htmlEscape(r.name)}">Rename</button>
              <button class="danger" data-rdel="${r.id}">Remove</button>
            </td>
          </tr>`).join("")}
        </tbody>
      </table>
      <div class="actions" style="margin-top:8px">
        <button id="r-add">+ Add reporter</button>
      </div>
      <p class="muted small" style="margin-top:6px">
        Renaming a reporter cascades through every Gap's rounds so historical
        data stays in sync. Removing a reporter only affects the dropdown —
        historical rounds keep their original reporter string so audit
        history is preserved.
      </p>
    </section>`)}

  `;
  // Tab click handlers — purely DOM-local, no re-fetch.
  $$(".settings-tab", $("#settings-tabs")).forEach((btn) => {
    btn.addEventListener("click", () => {
      setSettingsTab(btn.dataset.tabTarget);
    });
  });
  $("#btn-pause")?.addEventListener("click", async () => {
    const paused = s.paused === "1";
    await withButtonBusy($("#btn-pause"), paused ? "Resuming…" : "Pausing…", async () => {
      try {
        await api("PATCH", "/api/settings", { paused: paused ? "0" : "1" });
        await refreshSettings();
      } catch (e) { toast(e.message, "error"); }
    });
  });
  $$("[data-cancel-agent]").forEach((b) => {
    b.addEventListener("click", async () => {
      const id = b.dataset.cancelAgent;
      const ok = await modalConfirm(
        "Cancel this Gap's running subprocess?",
        { title: "Cancel run", okLabel: "Cancel run", danger: true,
          cancelLabel: "Keep running" },
      );
      if (!ok) return;
      await withButtonBusy(b, "Cancelling…", async () => {
        try {
          await api("POST", `/api/gaps/${id}/cancel`);
          await refreshSettings();
        } catch (e) { toast(e.message, "error"); }
      });
    });
  });
  bindGovernanceRuleButtons();
  $("#s-governance-add-rule")?.addEventListener("click", () => addGovernanceRuleRow());
  $("#s-governance-save")?.addEventListener("click", async () => {
    await withButtonBusy($("#s-governance-save"), "Saving…", async () => {
      try {
        await api("PATCH", "/api/governance", {
          product: $("#s-governance-product").value,
          constitution: $("#s-governance-constitution").value,
          rules: collectGovernanceRules(),
        });
        toast("Governance saved", "info");
        await refreshSettings();
      } catch (e) { toast(e.message, "error"); }
    });
  });
  $("#s-governance-generate")?.addEventListener("click", async () => {
    const product = ($("#s-governance-product")?.value || "").trim();
    const constitution = ($("#s-governance-constitution")?.value || "").trim();
    if (!product || !constitution) {
      toast("Product and Constitution are required to generate rules", "error");
      return;
    }
    await withButtonBusy($("#s-governance-generate"), "Generating…", async () => {
      try {
        const r = await api("POST", "/api/governance/generate-rules", {
          product, constitution,
        });
        $("#governance-rules-list").innerHTML = renderGovernanceRuleRows(r.rules || []);
        bindGovernanceRuleButtons();
        toast("Rules generated — review and save", "info");
      } catch (e) { toast(e.message, "error"); }
    });
  });
  $("#s-project-add")?.addEventListener("click", async () => {
    await openAddAppModal();
  });
  $("#s-project-switch")?.addEventListener("click", async () => {
    const path = ($("#s-project-select")?.value || "").trim();
    if (!path || path === currentProject) return;
    const ok = await modalConfirm(
      "Switch refine to the selected app? Running agents will be stopped and the current app must be clean.",
      { title: "Switch app", okLabel: "Switch" },
    );
    if (!ok) return;
    await withButtonBusy($("#s-project-switch"), "Switching…", async () => {
      try {
        const result = await api("POST", "/api/project/attach", { path });
        await applyProjectAttachResult(result);
      } catch (e) {
        if (e.status === 409 && /migration required/i.test(e.message || "")) {
          const migrate = await modalConfirm(
            "This app uses an older Refine schema. Migrate .refine state and open it?",
            { title: "Migrate app", okLabel: "Migrate and open" },
          );
          if (!migrate) return;
          const result = await api("POST", "/api/project/attach", { path, migrate: true });
          await applyProjectAttachResult(result);
          return;
        }
        toast(e.details || e.message, "error");
      }
    });
  });
  $("#s-project-remove")?.addEventListener("click", async () => {
    const path = ($("#s-project-select")?.value || "").trim();
    if (!path) return;
    const ok = await modalConfirm(
      "Remove this app from the known-apps list? This does not delete files.",
      { title: "Remove app", okLabel: "Remove", danger: true },
    );
    if (!ok) return;
    await withButtonBusy($("#s-project-remove"), "Removing…", async () => {
      try {
        const result = await api("DELETE", "/api/projects", { path });
        state.project = { ...(state.project || {}), apps: result.apps || [] };
        toast("App removed", "info");
        await refreshSettings();
      } catch (e) { toast(e.details || e.message, "error"); }
    });
  });
  $("#s-save-runtime")?.addEventListener("click", async () => {
    await withButtonBusy($("#s-save-runtime"), "Saving…", async () => {
      try {
        const chosen = $("#s-cli").value;
        await api("PATCH", "/api/settings", {
          parallel_run_cap: $("#s-cap").value,
          branch_name_pattern: $("#s-pattern").value,
          agent_idle_timeout_seconds: $("#s-idle").value,
          agent_hard_cap_seconds: $("#s-hard").value,
          chat_idle_timeout_seconds: $("#s-chat-idle").value,
          backlog_promote_after_seconds: $("#s-backlog-promote").value,
          agent_cli: chosen,
        });
        for (const box of $$("[data-feature-cell]")) {
          const { provider, feature } = box.dataset;
          const enabled = box.checked;
          const wasEnabled = box.dataset.featureOriginalEnabled === "1";
          const clearPending = box.dataset.featureClearPending === "1";
          if (!clearPending && enabled === wasEnabled) continue;
          await api("POST", "/api/features/override", {
            provider, feature, enabled: clearPending ? null : enabled,
          });
        }
        // Pull the matrix for the new provider and surface what
        // changed. Chat / Import will be hidden or labeled disabled
        // immediately by the gates.
        await refreshFeatures();
        const matrix = state.features?.matrix || {};
        const disabled = (state.features?.features || [])
          .filter((f) => !(matrix[`${chosen}.${f.key}`] || {}).enabled)
          .map((f) => f.label);
        if (disabled.length) {
          toast(
            `Saved. Disabled for ${chosen}: ${disabled.join(", ")}. ` +
            "See Feature flags on this tab.",
            "info",
          );
        } else {
          toast("Saved — re-check auth to confirm the new CLI is reachable", "info");
        }
      } catch (e) { toast(e.message, "error"); }
    });
  });
  // Feature flag toggles.
  $$("[data-feature-cell]").forEach((box) => {
    box.addEventListener("change", () => {
      delete box.dataset.featureClearPending;
      updateFeatureToggleLabel(box);
    });
  });
  $$("[data-feature-clear]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const { provider, feature } = btn.dataset;
      const box = $(`[data-feature-cell="${provider}.${feature}"]`);
      if (!box) return;
      box.checked = box.dataset.featureDefault === "1";
      box.dataset.featureClearPending = "1";
      updateFeatureToggleLabel(box);
      btn.textContent = "clear on save";
    });
  });
  $("#s-recheck").addEventListener("click", async () => {
    await withButtonBusy($("#s-recheck"), "Re-checking…", async () => {
      try {
        const r = await api("POST", "/api/settings/recheck-auth");
        toast(r.ok ? "Auth OK" : `Auth failed: ${r.message || "(no message)"}`, r.ok ? "info" : "error");
      } catch (e) { toast(e.message, "error"); }
    });
  });
  $("#logs-cleanup").addEventListener("click", async () => {
    const days = parseInt($("#logs-cleanup-days").value, 10);
    const human = days === 0
      ? "Delete ALL activity log entries? This cannot be undone."
      : `Delete activity log entries older than ${days} day${days === 1 ? "" : "s"}? This cannot be undone.`;
    const ok = await modalConfirm(human, {
      title: "Clean up old logs",
      okLabel: days === 0 ? "Delete all" : "Delete",
      danger: true,
    });
    if (!ok) return;
    await withButtonBusy($("#logs-cleanup"), "Cleaning…", async () => {
      try {
        const r = await api("POST", "/api/activity/cleanup", { days });
        toast(`Deleted ${r.deleted} log entr${r.deleted === 1 ? "y" : "ies"}.`, "info");
      } catch (e) { toast(e.message, "error"); }
    });
  });
  $$("[data-rdel]").forEach((b) => b.addEventListener("click", async () => {
    const ok = await modalConfirm(
      "Remove this reporter from the dropdown? Historical rounds keep their original reporter string.",
      { title: "Remove reporter", okLabel: "Remove", danger: true },
    );
    if (!ok) return;
    try { await api("DELETE", "/api/reporters/" + b.dataset.rdel); await renderSettings(); }
    catch (e) { toast(e.message, "error"); }
  }));
  $$("[data-rename]").forEach((b) => b.addEventListener("click", async () => {
    const oldName = b.dataset.name;
    const name = await modalPrompt("New name", oldName,
                                   { title: "Rename reporter" });
    if (!name || !name.trim()) return;
    const newName = name.trim();
    try {
      await api("PATCH", "/api/reporters/" + b.dataset.rename, { name: newName });
      if (state.lastReporter === oldName) setLastReporter(newName);
      await refreshReporters();
      await renderSettings();
    } catch (e) { toast(e.message, "error"); }
  }));
  $("#r-add").addEventListener("click", async () => {
    const name = await modalPrompt("Reporter name", "",
                                   { title: "Add reporter" });
    if (!name || !name.trim()) return;
    try { await api("POST", "/api/reporters", { name: name.trim() }); await refreshReporters(); await renderSettings(); }
    catch (e) { toast(e.message, "error"); }
  });
  $("#instance-add")?.addEventListener("click", async () => {
    const name = await modalPrompt("Instance name", "",
                                   { title: "Create instance" });
    if (!name || !name.trim()) return;
    try {
      await api("POST", "/api/instances", { display_name: name.trim() });
      await refreshSettings();
    } catch (e) { toast(e.message, "error"); }
  });
  $$("[data-instance-activate]").forEach((b) => b.addEventListener("click", async () => {
    try {
      await api("POST", "/api/instances/activate", { instance_id: b.dataset.instanceActivate });
      toast("Instance activated", "info");
      await refreshSettings();
    } catch (e) { toast(e.message, "error"); }
  }));
  $$("[data-instance-rename]").forEach((b) => b.addEventListener("click", async () => {
    const name = await modalPrompt("Instance name", b.dataset.name || "",
                                   { title: "Rename instance" });
    if (!name || !name.trim()) return;
    try {
      await api("PATCH", "/api/instances/" + encodeURIComponent(b.dataset.instanceRename), {
        display_name: name.trim(),
      });
      await refreshSettings();
    } catch (e) { toast(e.message, "error"); }
  }));
  $$("[data-instance-archive]").forEach((b) => b.addEventListener("click", async () => {
    const ok = await modalConfirm(
      "Archive this instance? Gap ownership IDs stay unchanged and can still be transferred.",
      { title: "Archive instance", okLabel: "Archive", danger: true },
    );
    if (!ok) return;
    try {
      await api("PATCH", "/api/instances/" + encodeURIComponent(b.dataset.instanceArchive), {
        archived: true,
      });
      await refreshSettings();
    } catch (e) { toast(e.message, "error"); }
  }));
  $("#instance-transfer")?.addEventListener("click", async () => {
    const source = $("#instance-transfer-source")?.value || "";
    const target = $("#instance-transfer-target")?.value || "";
    if (!target) return;
    const ok = await modalConfirm(
      "Refine will pause agents, stop all running agent processes, mark matching " +
      "in-progress and ready-merge Gaps as cancelled, then transfer all matching " +
      "Gaps to the selected instance.",
      {
        title: "Transfer Gaps",
        okLabel: "Pause, cancel, and transfer",
        cancelLabel: "Keep Gaps unchanged",
        danger: true,
      },
    );
    if (!ok) return;
    try {
      const r = await api("POST", "/api/instances/transfer-gaps", {
        source_instance_id: source,
        target_instance_id: target,
        cancel_active: true,
      });
      toast(
        `Transferred ${r.updated}; cancelled ${r.cancelled || 0}; ` +
        `stopped ${r.stopped_processes || 0} processes; skipped ${r.skipped}.`,
        "info",
      );
      await refreshSettings();
    } catch (e) { toast(e.message, "error"); }
  });

  // --- Target application controls on the Application tab -------------------
  $("#s-save-application")?.addEventListener("click", async () => {
    await withButtonBusy($("#s-save-application"), "Saving…", async () => {
      try {
        await api("PATCH", "/api/settings", {
          agent_subpath: $("#s-subpath").value,
          merge_target_branch: $("#s-merge-target").value,
          target_app_start_command: $("#s-target-start-command").value,
          target_app_stop_command: $("#s-target-stop-command").value,
          target_app_rebuild_command: $("#s-target-rebuild-command").value,
          target_app_status_command: $("#s-target-status-command").value,
          target_app_cwd: $("#s-target-cwd").value,
          target_app_env_json: $("#s-target-env").value,
          target_app_start_timeout_seconds: $("#s-target-start-timeout").value,
          target_app_stop_timeout_seconds: $("#s-target-stop-timeout").value,
          target_app_rebuild_timeout_seconds: $("#s-target-rebuild-timeout").value,
          target_app_status_timeout_seconds: $("#s-target-status-timeout").value,
          target_app_log_path: $("#s-target-log-path").value,
          target_app_http_check_url: $("#s-target-http-url").value,
          target_app_tcp_check_host: $("#s-target-tcp-host").value,
          target_app_tcp_check_port: $("#s-target-tcp-port").value,
          target_app_process_check_command: $("#s-target-process-command").value,
        });
        toast("Saved", "info");
        refreshTargetAppStatus();
      } catch (e) { toast(e.message, "error"); }
    });
  });
  $("#s-target-generate")?.addEventListener("click", async () => {
    const btn = $("#s-target-generate");
    const ok = await modalConfirm(
      "Ask the agent to analyse the codebase and draft target-app configuration? This can take a minute or two and overwrites the fields above.",
      { title: "Generate target-app config", okLabel: "Generate" },
    );
    if (!ok) return;
    await withButtonBusy(btn, "Generating…", async () => {
      try {
        const r = await api("POST", "/api/target-app/generate-instructions",
                            { kind: "all" });
        if (r.ok && r.config) {
          applyGeneratedTargetAppConfig(r.config);
          toast("Generated — review and click Save application to persist", "info");
        } else {
          toast("Generation produced no configuration", "error");
        }
      } catch (e) { toast(e.message, "error"); }
    });
  });
  $("#s-target-run-start")?.addEventListener("click", async () => {
    const btn = $("#s-target-run-start");
    await withButtonBusy(btn, "Starting…", async () => {
      await runTargetAppAction("start");
    });
  });
  $("#s-target-run-stop")?.addEventListener("click", async () => {
    const btn = $("#s-target-run-stop");
    await withButtonBusy(btn, "Stopping…", async () => {
      await runTargetAppAction("stop");
    });
  });
  $("#s-target-run-rebuild")?.addEventListener("click", async () => {
    const btn = $("#s-target-run-rebuild");
    await withButtonBusy(btn, "Rebuilding…", async () => {
      await runTargetAppAction("rebuild");
    });
  });
  $("#s-target-health-now")?.addEventListener("click", async () => {
    const btn = $("#s-target-health-now");
    await withButtonBusy(btn, "Probing…", async () => {
      try {
        const r = await api("POST", "/api/target-app/health");
        const ok = "last_check_ok" in r ? r.last_check_ok : r.last_health_ok;
        toast(ok ? "Status check OK" : (r.probe_message || "Unhealthy"),
              ok ? "info" : "error");
        drawTargetAppStatusBlock(r);
      } catch (e) { toast(e.message, "error"); }
    });
  });
  // Kick off the initial status load (and let SSE refresh later).
  refreshTargetAppStatus();
}

function applyGeneratedTargetAppConfig(cfg) {
  const set = (id, value) => {
    const el = $(id);
    if (el) el.value = value == null ? "" : String(value);
  };
  set("#s-target-start-command", cfg.start_command || "");
  set("#s-target-stop-command", cfg.stop_command || "");
  set("#s-target-rebuild-command", cfg.rebuild_command || "");
  set("#s-target-status-command", cfg.status_command || "");
  set("#s-target-cwd", cfg.cwd || "");
  set("#s-target-env", JSON.stringify(cfg.env || {}, null, 2));
  set("#s-target-start-timeout", cfg.start_timeout_seconds || 120);
  set("#s-target-stop-timeout", cfg.stop_timeout_seconds || 60);
  set("#s-target-rebuild-timeout", cfg.rebuild_timeout_seconds || 300);
  set("#s-target-status-timeout", cfg.status_timeout_seconds || 10);
  set("#s-target-log-path", cfg.log_path || "");
  set("#s-target-http-url", cfg.http_check_url || "");
  set("#s-target-tcp-host", cfg.tcp_check_host || "");
  set("#s-target-tcp-port", cfg.tcp_check_port || "");
  set("#s-target-process-command", cfg.process_check_command || "");
  const notesRow = $("#s-target-notes-row");
  const notes = $("#s-target-notes");
  if (notesRow && notes) {
    notes.textContent = cfg.notes || "";
    notesRow.style.display = cfg.notes ? "" : "none";
  }
}

// Re-fetch + repaint the target-app status block inside the System panel.
// Cheap to call from anywhere — silently no-ops if System isn't rendered.
async function refreshTargetAppStatus() {
  const block = document.getElementById("target-app-status-block");
  if (!block) return;
  try {
    const r = await api("GET", "/api/target-app/status");
    drawTargetAppStatusBlock(r);
  } catch (e) {
    block.innerHTML = `<span class="muted">Status unavailable: ${htmlEscape(e.message)}</span>`;
  }
}

function drawTargetAppStatusBlock(snap) {
  const block = document.getElementById("target-app-status-block");
  if (!block) return;
  const stateLabel = {
    running:  "Running",
    degraded: "Degraded",
    starting: "Starting…",
    rebuilding: "Rebuilding…",
    stopping: "Stopping…",
    stopped:  "Stopped",
    failed:   "Failed",
    unknown:  "Unknown",
  }[snap.state] || snap.state || "Unknown";
  const checkAt = snap.last_check_at || snap.last_health_at || "";
  const checkOk = "last_check_ok" in snap ? snap.last_check_ok : snap.last_health_ok;
  const checkMessage = snap.last_check_message || snap.last_health_message || "";
  const healthBits = checkAt
    ? `Last status check: ${checkOk ? "OK" : "FAIL"} · ${fmtTime(checkAt)}`
    : "No status checks yet.";
  const healthDetail = checkMessage && !checkOk
    ? `<p class="muted small" style="margin-top:6px;color:var(--error)">Check: ${htmlEscape(checkMessage)}</p>`
    : "";
  const op = snap.last_operation
    ? `<p class="muted small" style="margin-top:6px">Last operation: ${htmlEscape(snap.last_operation.kind)} → ${htmlEscape(snap.last_operation.state)} · ${fmtTime(snap.last_operation.finished_at)}</p>`
    : "";
  block.innerHTML = `
    <div style="display:flex;align-items:center;gap:10px">
      <span class="target-app-dot" data-status-dot></span>
      <strong>${htmlEscape(stateLabel)}</strong>
      ${snap.has_status_checks ? `<span class="muted small">status checks configured</span>` : `<span class="muted small">No status checks configured</span>`}
    </div>
    <p class="muted small" style="margin:8px 0 0">${htmlEscape(healthBits)}</p>
    ${healthDetail}
    ${op}
    ${snap.last_error ? `<p class="muted small" style="margin-top:6px;color:var(--error)">Last error: ${htmlEscape(snap.last_error)}</p>` : ""}
    ${snap.legacy_config_present ? `<p class="muted small" style="margin-top:6px;color:var(--warn)">Legacy target-app settings detected.</p>` : ""}
  `;
  // Apply dot colour from the parent state via a CSS hook — the .target-app-dot
  // colour rules key off `data-state` on an ancestor, so set it here too.
  const dot = block.querySelector("[data-status-dot]");
  if (dot) {
    dot.style.background = ({
      running:  "#1f9d4d",
      degraded: "#d4a106",
      stopped:  "#c63838",
      starting: "#d4a106",
      rebuilding: "#d4a106",
      stopping: "#d4a106",
      failed:   "#c63838",
    }[snap.state]) || "#b8bcc6";
  }
  // Reflect state on the Start / Stop buttons: only one applies at a
  // time. Both are disabled while a transition is in flight so the
  // user can't fire a second action mid-agent-run.
  const startBtn = document.getElementById("s-target-run-start");
  const rebuildBtn = document.getElementById("s-target-run-rebuild");
  const stopBtn  = document.getElementById("s-target-run-stop");
  if (startBtn && stopBtn && rebuildBtn) {
    const isRunning  = snap.state === "running" || snap.state === "degraded";
    const isStopped  = snap.state === "stopped" || snap.state === "unknown" || snap.state === "failed";
    const inFlight   = snap.state === "starting" || snap.state === "stopping" || snap.state === "rebuilding";
    startBtn.style.display = (isStopped || inFlight) ? "" : "none";
    stopBtn.style.display  = (isRunning || inFlight) ? "" : "none";
    startBtn.disabled = inFlight || !snap.has_start_command;
    rebuildBtn.disabled = inFlight || !snap.has_rebuild_command;
    stopBtn.disabled  = inFlight || !snap.has_stop_command;
    if (!snap.has_start_command) {
      startBtn.title = "Configure a start command above first.";
    } else {
      startBtn.title = "";
    }
    if (!snap.has_stop_command) {
      stopBtn.title = "Configure a stop command above first.";
    } else {
      stopBtn.title = "";
    }
    if (!snap.has_rebuild_command) {
      rebuildBtn.title = "Configure a rebuild command above first.";
    } else {
      rebuildBtn.title = "";
    }
  }
}
