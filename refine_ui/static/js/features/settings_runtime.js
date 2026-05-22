// ---- System / Runtime -------------------------------------------------------

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


function renderSettingsRuntimeTab(s, feats, activeInstanceLabel, cli) {
  const cliOption = (value, label) =>
    `<option value="${value}" ${cli === value ? "selected" : ""}>${htmlEscape(label)}</option>`;
  return `
    <section class="settings-section">
      <h3>Runtime configuration</h3>
      <p class="scope-label muted small">Instance: ${htmlEscape(activeInstanceLabel)}</p>
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

    <section class="settings-section settings-save-section">
      <div class="actions"><button id="s-save-runtime">Save runtime</button></div>
    </section>`;
}

function bindSettingsRuntimeTab() {
  $("#s-save-runtime")?.addEventListener("click", async () => {
    await withButtonBusy($("#s-save-runtime"), "Saving…", async () => {
      try {
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
      } catch (e) { await showActionError(e); }
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
      } catch (e) { await showActionError(e); }
    });
  });
}
