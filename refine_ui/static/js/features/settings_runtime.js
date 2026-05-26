// ---- System / Runtime -------------------------------------------------------

function renderSettingsRuntimeTab(s, activeInstanceLabel, cli) {
  const cliOption = (value, label) =>
    `<option value="${value}" ${cli === value ? "selected" : ""}>${htmlEscape(label)}</option>`;
  return `
    <section class="settings-section">
      <div id="runtime-upgrade-banner"></div>
      <h3>Runtime configuration</h3>
      <p class="scope-label muted small">Instance: ${htmlEscape(activeInstanceLabel)}</p>
      <div class="actions settings-section-actions">
        <button class="secondary" id="s-runtime-copy-instance">Copy from instance</button>
      </div>
      <div class="form-row"><label>Parallel-run cap</label>
        <input type="number" id="s-cap" value="${s.parallel_run_cap || 10}"></div>
      <div class="form-row"><label>Branch name pattern</label>
        <input type="text" id="s-pattern" value="${htmlEscape(s.branch_name_pattern || "refine/{gap_id}")}"></div>
      <div class="form-row"><label>Agent idle timeout (seconds)</label>
        <input type="number" id="s-idle" value="${s.agent_idle_timeout_seconds || 900}"></div>
      <div class="form-row"><label>Agent hard cap (seconds)</label>
        <input type="number" id="s-hard" value="${s.agent_hard_cap_seconds || 86400}"></div>
      <div class="form-grid two">
        <div class="form-row"><label>Worker memory limit (MB)
          <span class="muted small">— 0 disables the per-process limit</span></label>
          <input type="number" id="s-worker-memory" min="0" value="${s.worker_memory_limit_mb ?? 2000}"></div>
        <div class="form-row"><label>UI memory limit (MB)
          <span class="muted small">— 0 disables the supervised UI process limit</span></label>
          <input type="number" id="s-ui-memory" min="0" value="${s.ui_memory_limit_mb ?? 2000}"></div>
      </div>
      <div class="form-grid two">
        <div class="form-row"><label>Worker CPU priority</label>
          <select id="s-worker-cpu-priority">
            ${[
              ["normal", "Normal"],
              ["low", "Low"],
              ["very_low", "Very low"],
            ].map(([v, lbl]) => `<option value="${v}" ${String(s.worker_cpu_priority ?? "low") === v ? "selected" : ""}>${lbl}</option>`).join("")}
          </select></div>
        <div class="form-row"><label>Resource isolation mode</label>
          <select id="s-resource-isolation">
            ${[
              ["auto", "Auto"],
              ["enforced", "Enforced"],
              ["best_effort", "Best effort"],
            ].map(([v, lbl]) => `<option value="${v}" ${String(s.resource_isolation_mode ?? "auto") === v ? "selected" : ""}>${lbl}</option>`).join("")}
          </select></div>
      </div>
      <div class="form-row"><label>Rate/token limit pause
        <span class="muted small">— how long agents wait before continuing after provider rate-limit or token-limit errors.</span></label>
        <select id="s-agent-limit-pause">
          ${[
            ["30",    "30 seconds"],
            ["60",    "1 minute"],
            ["3600",  "1 hour"],
            ["10800", "3 hours"],
          ].map(([v, lbl]) => `<option value="${v}" ${String(s.agent_limit_pause_seconds ?? "60") === v ? "selected" : ""}>${lbl}</option>`).join("")}
        </select></div>
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
      <div class="form-row"><label>Target repo update pulse
        <span class="muted small">— checks for local commits or upstream commits and refreshes this instance's projected state.</span></label>
        <select id="s-project-update-pulse">
          ${[
            ["-1",   "Never"],
            ["30",   "30 seconds"],
            ["60",   "1 minute"],
            ["300",  "5 minutes"],
            ["900",  "15 minutes"],
            ["1800", "30 minutes"],
            ["3600", "1 hour"],
          ].map(([v, lbl]) => `<option value="${v}" ${String(s.project_update_pulse_interval_seconds ?? "60") === v ? "selected" : ""}>${lbl}</option>`).join("")}
        </select></div>
      <div class="form-row"><label>File browser ignore patterns
        <span class="muted small">— comma-delimited file or directory patterns hidden during normal browsing.</span></label>
        <input type="text" id="s-file-browser-ignore"
               value="${htmlEscape(s.file_browser_ignore_patterns || "node_modules, .git, .refine")}"></div>
    </section>

    <section class="settings-section">
      <h3>AI Provider</h3>
      <div class="form-row"><label>Which AI provider refine drives
        <span class="muted small">— used for Gap agent runs, conflict resolution, chat, import extraction, target-app actions, and pre-flight.</span></label>
        <select id="s-cli">
          ${cliOption("claude", "Claude Code (default)")}
          ${cliOption("codex", "OpenAI Codex")}
          ${cliOption("gemini", "Gemini")}
          ${cliOption("copilot", "GitHub Copilot")}
        </select></div>
      <p class="muted small" style="margin-top:6px">
        After switching: re-check auth below to confirm the chosen provider is
        installed and authed on the host. Round logs are structured for Claude
        Code, Codex, and Copilot where their CLIs expose machine-readable
        events; Gemini falls back to plain stdout passthrough.
      </p>
      <p class="muted" style="margin-top:14px">The selected provider's auth lives on the host. Use Re-check to re-run the pre-flight after running the relevant login command (<code>claude login</code> / <code>codex login</code> / <code>gemini auth login</code> / <code>copilot login</code>).</p>
      <button id="s-recheck">Re-check auth</button>
    </section>`;
}

function renderRuntimeUpgradeBanner(upgrade) {
  if (!upgrade) return "";
  const current = upgrade.current_version || "unknown";
  const latest = upgrade.latest_version || "";
  if (upgrade.upgrade_available) {
    const command = upgrade.command || "";
    return `
      <div class="runtime-version-status runtime-version-status-upgrade">
        <h3>Upgrade available</h3>
        <p class="muted small" style="margin-top:0">
          Refine ${htmlEscape(latest)} is available.
          Current version: ${htmlEscape(current)}.
        </p>
        <div class="runtime-upgrade-command muted small">
          <code>${htmlEscape(command)}</code>
          <button
            class="secondary runtime-copy-upgrade-command"
            type="button"
            title="Copy upgrade command"
            aria-label="Copy upgrade command"
            data-runtime-copy-upgrade="${htmlEscape(command)}">
            <svg aria-hidden="true" viewBox="0 0 24 24" focusable="false">
              <rect x="9" y="9" width="10" height="10" rx="2"></rect>
              <path d="M5 15V7a2 2 0 0 1 2-2h8"></path>
            </svg>
          </button>
        </div>
      </div>`;
  }
  if (upgrade.local_development) {
    return `
      <div class="runtime-version-status">
        <h3>Local development checkout</h3>
        <p class="muted small" style="margin:0">
          This checkout is ahead of release ${htmlEscape(current)}.
          ${latest ? `Latest published release: ${htmlEscape(latest)}.` : ""}
        </p>
      </div>`;
  }
  if (current && latest && current === latest) {
    return `
      <div class="runtime-version-status">
        <h3>Refine is up to date</h3>
        <p class="muted small" style="margin:0">
          Running latest published release: ${htmlEscape(current)}.
        </p>
      </div>`;
  }
  if (upgrade.error) {
    return `
      <div class="runtime-version-status runtime-version-status-unknown">
        <h3>Version status unavailable</h3>
        <p class="muted small" style="margin:0">
          ${htmlEscape(upgrade.error)}
        </p>
      </div>`;
  }
  return `
    <div class="runtime-version-status runtime-version-status-unknown">
      <h3>Version status unavailable</h3>
      <p class="muted small" style="margin:0">
        Refine could not determine the latest published release.
      </p>
    </div>`;
}

function fallbackCopyText(text) {
  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.setAttribute("readonly", "");
  textarea.style.position = "fixed";
  textarea.style.top = "-1000px";
  textarea.style.left = "-1000px";
  document.body.appendChild(textarea);
  textarea.select();
  textarea.setSelectionRange(0, textarea.value.length);
  try {
    document.execCommand("copy");
  } finally {
    document.body.removeChild(textarea);
  }
}

async function copyRuntimeUpgradeCommand(command) {
  if (!command) return;
  try {
    if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(command);
    } else {
      fallbackCopyText(command);
    }
    toast("Upgrade command copied", "info");
  } catch (_e) {
    toast("Could not copy upgrade command", "error");
  }
}

async function refreshRuntimeUpgradeBanner() {
  const root = document.getElementById("runtime-upgrade-banner");
  if (!root) return;
  try {
    const result = await api("GET", "/api/upgrade");
    root.innerHTML = renderRuntimeUpgradeBanner(result.upgrade || {});
  } catch (_e) {
    root.innerHTML = "";
  }
}

async function autosaveSettingsRuntime(options = {}) {
  const chosen = $("#s-cli").value;
  await api("PATCH", "/api/settings", {
    parallel_run_cap: $("#s-cap").value,
    branch_name_pattern: $("#s-pattern").value,
    agent_idle_timeout_seconds: $("#s-idle").value,
    agent_hard_cap_seconds: $("#s-hard").value,
    worker_memory_limit_mb: $("#s-worker-memory").value,
    ui_memory_limit_mb: $("#s-ui-memory").value,
    worker_cpu_priority: $("#s-worker-cpu-priority").value,
    resource_isolation_mode: $("#s-resource-isolation").value,
    agent_limit_pause_seconds: $("#s-agent-limit-pause").value,
    chat_idle_timeout_seconds: $("#s-chat-idle").value,
    backlog_promote_after_seconds: $("#s-backlog-promote").value,
    project_update_pulse_interval_seconds: $("#s-project-update-pulse").value,
    file_browser_ignore_patterns: $("#s-file-browser-ignore").value,
    agent_cli: chosen,
  });
  if (options.refresh) await refreshSettingsTab("runtime", { force: true });
}

function bindSettingsRuntimeTab() {
  bindCommand("#s-runtime-copy-instance", "settings.runtime.copy_instance");
  const root = document.querySelector('[data-tab-pane="runtime"]');
  root?.addEventListener("click", (e) => {
    const button = e.target.closest("[data-runtime-copy-upgrade]");
    if (!button) return;
    copyRuntimeUpgradeCommand(button.getAttribute("data-runtime-copy-upgrade") || "");
  });
  const autosaveRuntime = bindSettingsAutosave(
    root,
    "#s-cap, #s-pattern, #s-idle, #s-hard, #s-worker-memory, #s-ui-memory, #s-worker-cpu-priority, #s-resource-isolation, #s-agent-limit-pause, #s-chat-idle, #s-backlog-promote, #s-project-update-pulse, #s-file-browser-ignore",
    autosaveSettingsRuntime,
  );
  const autosaveRuntimeAndRefresh = createSettingsAutosave(
    () => autosaveSettingsRuntime({ refresh: true }),
  );
  $("#s-cli")?.addEventListener("change", autosaveRuntimeAndRefresh);
  bindCommand("#s-recheck", "runtime.recheck_auth");
  if (document.querySelector('[data-tab-pane="runtime"].active')) {
    refreshRuntimeUpgradeBanner();
  }
  document.querySelector('[data-tab-target="runtime"]')?.addEventListener("click", () => {
    setTimeout(refreshRuntimeUpgradeBanner, 0);
  });
}
