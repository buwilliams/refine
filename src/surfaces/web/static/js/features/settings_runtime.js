// ---- System / Runtime -------------------------------------------------------

function renderNodeRuntimeConfigSections(s, activeNodeLabel, cli) {
  const cliOption = (value, label) =>
    `<option value="${value}" ${cli === value ? "selected" : ""}>${htmlEscape(label)}</option>`;
  const optionLabel = (options, value) => {
    const match = options.find(([v]) => String(v) === String(value));
    return match?.[1] || String(value || "none");
  };
  const cpuOptions = [
    ["normal", "Normal"],
    ["low", "Low"],
    ["very_low", "Very low"],
  ];
  const isolationOptions = [
    ["auto", "Auto"],
    ["enforced", "Enforced"],
    ["best_effort", "Best effort"],
  ];
  const agentLimitOptions = [
    ["30",    "30 seconds"],
    ["60",    "1 minute"],
    ["3600",  "1 hour"],
    ["10800", "3 hours"],
  ];
  const backlogOptions = [
    ["-1",    "Never"],
    ["0",     "Instant"],
    ["300",   "5 minutes"],
    ["1800",  "30 minutes"],
    ["3600",  "1 hour"],
    ["10800", "3 hours"],
    ["21600", "6 hours"],
    ["86400", "24 hours"],
  ];
  const stateDebounceOptions = [
    ["1",  "1 second"],
    ["5",  "5 seconds"],
    ["15", "15 seconds"],
    ["30", "30 seconds"],
    ["60", "1 minute"],
  ];
  const remoteFetchOptions = [
    ["-1",    "Manual only"],
    ["300",   "5 minutes"],
    ["900",   "15 minutes"],
    ["1800",  "30 minutes"],
    ["3600",  "1 hour"],
  ];
  const providerOptions = [
    ["claude", "Claude Code (default)"],
    ["codex", "OpenAI Codex"],
    ["gemini", "Gemini"],
    ["copilot", "GitHub Copilot"],
    ["smoke-ai", "Smoke AI (deterministic testing)"],
  ];
  const workerCpuPriority = String(s.worker_cpu_priority ?? "low");
  const resourceIsolation = String(s.resource_isolation_mode ?? "auto");
  const agentLimitPause = String(s.agent_limit_pause_seconds ?? "60");
  const backlogPromote = String(s.backlog_promote_after_seconds ?? "3600");
  const stateDebounce = String(s.state_sync_debounce_seconds ?? "5");
  const remoteFetchInterval = String(s.project_update_pulse_interval_seconds ?? "300");
  return `
    <section class="settings-section">
      <h3>Runtime configuration</h3>
      <p class="scope-label muted small">Node: ${htmlEscape(activeNodeLabel)}</p>
      <div class="actions settings-section-actions">
        <button class="secondary" id="s-runtime-copy-node">Copy from node</button>
        <button class="secondary" id="s-state-sync-now" data-testid="runtime-state-sync-now">Sync state now</button>
      </div>
      ${renderSettingsEditableField({
        id: "s-cap",
        label: "Parallel-run cap",
        guideItemId: "runtime-parallel-run-cap",
        valueLabel: s.parallel_run_cap || 5,
        control: `<input type="number" id="s-cap" data-testid="runtime-parallel-run-cap" value="${s.parallel_run_cap || 5}">`,
      })}
      ${renderSettingsEditableField({
        id: "s-pattern",
        label: "Branch name pattern",
        guideItemId: "runtime-branch-name-pattern",
        valueLabel: s.branch_name_pattern || "refine/{goal_id}",
        control: `<input type="text" id="s-pattern" data-testid="runtime-branch-name-pattern" value="${htmlEscape(s.branch_name_pattern || "refine/{goal_id}")}">`,
      })}
      ${renderSettingsEditableField({
        id: "s-idle",
        label: "Agent idle timeout (seconds)",
        guideItemId: "runtime-agent-idle-timeout",
        valueLabel: s.agent_idle_timeout_seconds || 900,
        control: `<input type="number" id="s-idle" data-testid="runtime-agent-idle-timeout" value="${s.agent_idle_timeout_seconds || 900}">`,
      })}
      ${renderSettingsEditableField({
        id: "s-hard",
        label: "Agent hard cap (seconds)",
        guideItemId: "runtime-agent-hard-cap",
        valueLabel: s.agent_hard_cap_seconds || 86400,
        control: `<input type="number" id="s-hard" data-testid="runtime-agent-hard-cap" value="${s.agent_hard_cap_seconds || 86400}">`,
      })}
      <div class="form-grid two">
        ${renderSettingsEditableField({
          id: "s-worker-memory",
          label: "Worker memory limit (MB)",
          guideItemId: "runtime-worker-memory-limit",
          description: "0 disables the per-process limit",
          valueLabel: s.worker_memory_limit_mb ?? 2000,
          control: `<input type="number" id="s-worker-memory" data-testid="runtime-worker-memory-limit" min="0" value="${s.worker_memory_limit_mb ?? 2000}">`,
        })}
        ${renderSettingsEditableField({
          id: "s-ui-memory",
          label: "UI memory limit (MB)",
          guideItemId: "runtime-ui-memory-limit",
          description: "0 disables the supervised UI process limit",
          valueLabel: s.ui_memory_limit_mb ?? 2000,
          control: `<input type="number" id="s-ui-memory" data-testid="runtime-ui-memory-limit" min="0" value="${s.ui_memory_limit_mb ?? 2000}">`,
        })}
      </div>
      <div class="form-grid two">
        ${renderSettingsEditableField({
          id: "s-worker-cpu-priority",
          label: "Worker CPU priority",
          guideItemId: "runtime-worker-cpu-priority",
          valueLabel: optionLabel(cpuOptions, workerCpuPriority),
          control: `<select id="s-worker-cpu-priority" data-testid="runtime-worker-cpu-priority">
            ${cpuOptions.map(([v, lbl]) => `<option value="${v}" ${workerCpuPriority === v ? "selected" : ""}>${lbl}</option>`).join("")}
          </select>`,
        })}
        ${renderSettingsEditableField({
          id: "s-resource-isolation",
          label: "Resource isolation mode",
          guideItemId: "runtime-resource-isolation",
          valueLabel: optionLabel(isolationOptions, resourceIsolation),
          control: `<select id="s-resource-isolation" data-testid="runtime-resource-isolation">
            ${isolationOptions.map(([v, lbl]) => `<option value="${v}" ${resourceIsolation === v ? "selected" : ""}>${lbl}</option>`).join("")}
          </select>`,
        })}
      </div>
      ${renderSettingsEditableField({
        id: "s-agent-limit-pause",
        label: "Rate/token limit pause",
        guideItemId: "runtime-agent-limit-pause",
        description: "how long agents wait before continuing after provider rate-limit or token-limit errors.",
        valueLabel: optionLabel(agentLimitOptions, agentLimitPause),
        control: `<select id="s-agent-limit-pause" data-testid="runtime-agent-limit-pause">
          ${agentLimitOptions.map(([v, lbl]) => `<option value="${v}" ${agentLimitPause === v ? "selected" : ""}>${lbl}</option>`).join("")}
        </select>`,
      })}
      ${renderSettingsEditableField({
        id: "s-chat-idle",
        label: "Standalone chat idle timeout (seconds)",
        guideItemId: "runtime-chat-idle-timeout",
        description: "set to 0 to disable auto-close",
        valueLabel: s.chat_idle_timeout_seconds || 300,
        control: `<input type="number" id="s-chat-idle" data-testid="runtime-chat-idle-timeout" value="${s.chat_idle_timeout_seconds || 300}">`,
      })}
      ${renderSettingsEditableField({
        id: "s-backlog-promote",
        label: "Auto-promote backlog → todo",
        guideItemId: "runtime-backlog-promote",
        description: "how long a Goal may sit in backlog before the Workflow Engine moves it to todo. Default 1 hour.",
        valueLabel: optionLabel(backlogOptions, backlogPromote),
        control: `<select id="s-backlog-promote" data-testid="runtime-backlog-promote">
          ${backlogOptions.map(([v, lbl]) => `<option value="${v}" ${backlogPromote === v ? "selected" : ""}>${lbl}</option>`).join("")}
        </select>`,
      })}
      ${renderSettingsEditableField({
        id: "s-state-sync-debounce",
        label: "State sync debounce",
        guideItemId: "runtime-project-update-pulse",
        description: "coalesces nearby .refine mutations before publishing one readable state commit.",
        valueLabel: optionLabel(stateDebounceOptions, stateDebounce),
        control: `<select id="s-state-sync-debounce" data-testid="runtime-state-sync-debounce">
          ${stateDebounceOptions.map(([v, lbl]) => `<option value="${v}" ${stateDebounce === v ? "selected" : ""}>${lbl}</option>`).join("")}
        </select>`,
      })}
      ${renderSettingsEditableField({
        id: "s-project-update-pulse",
        label: "Project update pulse",
        guideItemId: "runtime-project-update-pulse",
        description: "fetches human and Refine remote branches without changing the checked-out application branch.",
        valueLabel: optionLabel(remoteFetchOptions, remoteFetchInterval),
        control: `<select id="s-project-update-pulse" data-testid="runtime-project-update-pulse">
          ${remoteFetchOptions.map(([v, lbl]) => `<option value="${v}" ${remoteFetchInterval === v ? "selected" : ""}>${lbl}</option>`).join("")}
        </select>`,
      })}
      ${renderSettingsEditableField({
        id: "s-file-browser-ignore",
        label: "File browser ignore patterns",
        guideItemId: "runtime-file-browser-ignore",
        description: "comma-delimited file or directory patterns hidden during normal browsing.",
        valueLabel: s.file_browser_ignore_patterns || "node_modules, .git, .refine, run",
        control: `<input type="text" id="s-file-browser-ignore"
                         data-testid="runtime-file-browser-ignore"
                         value="${htmlEscape(s.file_browser_ignore_patterns || "node_modules, .git, .refine, run")}">`,
      })}
    </section>

    <section class="settings-section">
      <h3>AI Provider</h3>
      ${renderSettingsEditableField({
        id: "s-cli",
        label: "Which AI provider refine drives",
        guideItemId: "runtime-ai-provider",
        description: "used for Goal agent runs, conflict resolution, chat, import extraction, target-app actions, and pre-flight.",
        valueLabel: optionLabel(providerOptions, cli),
        control: `<select id="s-cli" data-testid="runtime-provider-select">
          ${providerOptions.map(([value, label]) => cliOption(value, label)).join("")}
        </select>`,
      })}
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
    state_sync_debounce_seconds: $("#s-state-sync-debounce").value,
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
    "#s-cap, #s-pattern, #s-idle, #s-hard, #s-worker-memory, #s-ui-memory, #s-worker-cpu-priority, #s-resource-isolation, #s-agent-limit-pause, #s-chat-idle, #s-backlog-promote, #s-state-sync-debounce, #s-project-update-pulse, #s-file-browser-ignore",
    autosaveSettingsRuntime,
    { event: "settings-editable-commit" },
  );
  bindSettingsAutosave(
    root,
    "#s-cli",
    () => autosaveSettingsRuntime({ refresh: true, refreshTab: "runtime" }),
    { event: "settings-editable-commit" },
  );
  bindCommand("#s-recheck", "runtime.recheck_auth");
  const syncNow = document.querySelector("#s-state-sync-now");
  syncNow?.addEventListener("click", async () => {
    await withButtonBusy(syncNow, "Syncing...", async () => {
      try {
        const result = await api("POST", "/api/project/sync", {});
        const state = result.git_sync || {};
        toast(
          state.committed
            ? "Refine state committed and synchronized"
            : state.pulled
              ? "Remote Refine state synchronized"
              : "Refine state is already synchronized",
          "info",
        );
      } catch (error) {
        toast(error.message || "State synchronization failed", "error");
      }
    });
  });
  bindSettingsEditableFields(root);
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
