// ---- System / Runtime -------------------------------------------------------

function renderNodeRuntimeConfigSections(s, activeNodeLabel, cli) {
  const cliOption = (value, label) =>
    `<option value="${value}" ${cli === value ? "selected" : ""}>${htmlEscape(label)}</option>`;
  return `
    <section class="settings-section">
      <h3>Runtime configuration</h3>
      <p class="scope-label muted small">Node: ${htmlEscape(activeNodeLabel)}</p>
      <div class="actions settings-section-actions">
        <button class="secondary" id="s-runtime-copy-node">Copy from node</button>
      </div>
      <div class="form-row"><label>${renderSettingsGuideLabel("Parallel-run cap", "runtime-parallel-run-cap")}</label>
        <input type="number" id="s-cap" data-testid="runtime-parallel-run-cap" value="${s.parallel_run_cap || 5}"></div>
      <div class="form-row"><label>${renderSettingsGuideLabel("Branch name pattern", "runtime-branch-name-pattern")}</label>
        <input type="text" id="s-pattern" data-testid="runtime-branch-name-pattern" value="${htmlEscape(s.branch_name_pattern || "refine/{gap_id}")}"></div>
      <div class="form-row"><label>${renderSettingsGuideLabel("Agent idle timeout (seconds)", "runtime-agent-idle-timeout")}</label>
        <input type="number" id="s-idle" data-testid="runtime-agent-idle-timeout" value="${s.agent_idle_timeout_seconds || 900}"></div>
      <div class="form-row"><label>${renderSettingsGuideLabel("Agent hard cap (seconds)", "runtime-agent-hard-cap")}</label>
        <input type="number" id="s-hard" data-testid="runtime-agent-hard-cap" value="${s.agent_hard_cap_seconds || 86400}"></div>
      <div class="form-grid two">
        <div class="form-row"><label>${renderSettingsGuideLabel(
          "Worker memory limit (MB)",
          "runtime-worker-memory-limit",
          "0 disables the per-process limit",
        )}</label>
          <input type="number" id="s-worker-memory" data-testid="runtime-worker-memory-limit" min="0" value="${s.worker_memory_limit_mb ?? 2000}"></div>
        <div class="form-row"><label>${renderSettingsGuideLabel(
          "UI memory limit (MB)",
          "runtime-ui-memory-limit",
          "0 disables the supervised UI process limit",
        )}</label>
          <input type="number" id="s-ui-memory" data-testid="runtime-ui-memory-limit" min="0" value="${s.ui_memory_limit_mb ?? 2000}"></div>
      </div>
      <div class="form-grid two">
        <div class="form-row"><label>${renderSettingsGuideLabel("Worker CPU priority", "runtime-worker-cpu-priority")}</label>
          <select id="s-worker-cpu-priority" data-testid="runtime-worker-cpu-priority">
            ${[
              ["normal", "Normal"],
              ["low", "Low"],
              ["very_low", "Very low"],
            ].map(([v, lbl]) => `<option value="${v}" ${String(s.worker_cpu_priority ?? "low") === v ? "selected" : ""}>${lbl}</option>`).join("")}
        </select></div>
        <div class="form-row"><label>${renderSettingsGuideLabel("Resource isolation mode", "runtime-resource-isolation")}</label>
          <select id="s-resource-isolation" data-testid="runtime-resource-isolation">
            ${[
              ["auto", "Auto"],
              ["enforced", "Enforced"],
              ["best_effort", "Best effort"],
            ].map(([v, lbl]) => `<option value="${v}" ${String(s.resource_isolation_mode ?? "auto") === v ? "selected" : ""}>${lbl}</option>`).join("")}
          </select></div>
      </div>
      <div class="form-row"><label>${renderSettingsGuideLabel(
        "Rate/token limit pause",
        "runtime-agent-limit-pause",
        "how long agents wait before continuing after provider rate-limit or token-limit errors.",
      )}</label>
        <select id="s-agent-limit-pause" data-testid="runtime-agent-limit-pause">
          ${[
            ["30",    "30 seconds"],
            ["60",    "1 minute"],
            ["3600",  "1 hour"],
            ["10800", "3 hours"],
          ].map(([v, lbl]) => `<option value="${v}" ${String(s.agent_limit_pause_seconds ?? "60") === v ? "selected" : ""}>${lbl}</option>`).join("")}
        </select></div>
      <div class="form-row"><label>${renderSettingsGuideLabel(
        "Standalone chat idle timeout (seconds)",
        "runtime-chat-idle-timeout",
        "set to 0 to disable auto-close",
      )}</label>
        <input type="number" id="s-chat-idle" data-testid="runtime-chat-idle-timeout" value="${s.chat_idle_timeout_seconds || 300}"></div>
      <div class="form-row"><label>${renderSettingsGuideLabel(
        "Auto-promote backlog → todo",
        "runtime-backlog-promote",
        "how long a Gap may sit in backlog before the Workflow Engine moves it to todo. Default 1 hour.",
      )}</label>
        <select id="s-backlog-promote" data-testid="runtime-backlog-promote">
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
      <div class="form-row"><label>${renderSettingsGuideLabel(
        "Target repo update pulse",
        "runtime-project-update-pulse",
        "checks for local commits or upstream commits and refreshes this node's projected state.",
      )}</label>
        <select id="s-project-update-pulse" data-testid="runtime-project-update-pulse">
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
      <div class="form-row"><label>${renderSettingsGuideLabel(
        "File browser ignore patterns",
        "runtime-file-browser-ignore",
        "comma-delimited file or directory patterns hidden during normal browsing.",
      )}</label>
        <input type="text" id="s-file-browser-ignore"
               data-testid="runtime-file-browser-ignore"
               value="${htmlEscape(s.file_browser_ignore_patterns || "node_modules, .git, .refine, run")}"></div>
    </section>

    <section class="settings-section">
      <h3>AI Provider</h3>
      <div class="form-row"><label>${renderSettingsGuideLabel(
        "Which AI provider refine drives",
        "runtime-ai-provider",
        "used for Gap agent runs, conflict resolution, chat, import extraction, target-app actions, and pre-flight.",
      )}</label>
        <select id="s-cli" data-testid="runtime-provider-select">
          ${cliOption("claude", "Claude Code (default)")}
          ${cliOption("codex", "OpenAI Codex")}
          ${cliOption("gemini", "Gemini")}
          ${cliOption("copilot", "GitHub Copilot")}
          ${cliOption("smoke-ai", "Smoke AI (deterministic testing)")}
        </select></div>
      <p class="muted small" style="margin-top:6px">
        After switching: re-check auth below to confirm the chosen provider is
        installed and authed on the host. Round logs are structured for Claude
        Code, Codex, and Copilot where their CLIs expose machine-readable
        events; Gemini falls back to plain stdout passthrough.
      </p>
      <p class="muted" style="margin-top:14px">The selected provider's auth lives on the host. Use Re-check to re-run the pre-flight after running the relevant login command (<code>claude login</code> / <code>codex login</code> / <code>gemini auth login</code> / <code>copilot login</code>), or after setting <code>REFINE_SMOKE_AI_PATH</code> for Smoke AI.</p>
      <button id="s-recheck" data-testid="runtime-recheck-auth">Re-check auth</button>
    </section>`;
}

function renderRuntimeUpgradeBanner(upgrade) {
  if (!upgrade) return "";
  const current = upgrade.current_version || "unknown";
  const latest = upgrade.latest_version || "";
  if (upgrade.upgrade_available) {
    const command = "./r update";
    return `
      <div class="runtime-version-status runtime-version-status-upgrade" data-testid="runtime-upgrade-status">
        <span data-testid="runtime-upgrade-message">Upgrade available ${htmlEscape(latest || current)}</span>
        <button
          class="secondary runtime-copy-upgrade-command"
          type="button"
          data-testid="runtime-copy-upgrade"
          title="Copy ./r update"
          aria-label="Copy ./r update"
          data-runtime-copy-upgrade="${htmlEscape(command)}">
          <svg aria-hidden="true" viewBox="0 0 24 24" focusable="false">
            <rect x="9" y="9" width="10" height="10" rx="2"></rect>
            <path d="M5 15V7a2 2 0 0 1 2-2h8"></path>
          </svg>
        </button>
      </div>`;
  }
  if (upgrade.local_development) {
    return latest ? `
      <div class="runtime-version-status" data-testid="runtime-upgrade-status">
        <span data-testid="runtime-upgrade-message">Running latest ${htmlEscape(latest)}</span>
      </div>` : "";
  }
  if (current && latest && current === latest) {
    return `
      <div class="runtime-version-status" data-testid="runtime-upgrade-status">
        <span data-testid="runtime-upgrade-message">Running latest ${htmlEscape(current)}</span>
      </div>`;
  }
  return "";
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
  if (options.refresh) {
    await refreshSettingsTab(options.refreshTab || readSettingsTab(), { force: true });
  }
}

function bindNodeRuntimeConfigControls() {
  bindCommand("#s-runtime-copy-node", "settings.runtime.copy_node");
  const root = document.querySelector('[data-tab-pane="runtime"]');
  const autosaveRuntime = bindSettingsAutosave(
    root,
    "#s-cap, #s-pattern, #s-idle, #s-hard, #s-worker-memory, #s-ui-memory, #s-worker-cpu-priority, #s-resource-isolation, #s-agent-limit-pause, #s-chat-idle, #s-backlog-promote, #s-project-update-pulse, #s-file-browser-ignore",
    autosaveSettingsRuntime,
  );
  bindSettingsAutosave(
    root,
    "#s-cli",
    () => autosaveSettingsRuntime({ refresh: true, refreshTab: "runtime" }),
  );
  bindCommand("#s-recheck", "runtime.recheck_auth");
  return autosaveRuntime;
}

function bindRuntimeUpgradeBanner(rootSelector = ".settings-tabs-row") {
  const root = document.querySelector(rootSelector);
  root?.addEventListener("click", (e) => {
    const button = e.target.closest("[data-runtime-copy-upgrade]");
    if (!button) return;
    copyRuntimeUpgradeCommand(button.getAttribute("data-runtime-copy-upgrade") || "");
  });
}
