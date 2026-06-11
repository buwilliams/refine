// ---- System / Quality -------------------------------------------------------

function renderSettingsQualityNodeSections(quality) {
  const qualityEnabled = String(quality.enabled || "0") === "1";
  const qualityTiming = quality.timing === "post_build" ? "post_build" : "pre_merge";
  const regressionsEnabled = String(quality.regressions_enabled || "0") === "1";
  const regressions = Array.isArray(quality.regressions) ? quality.regressions : [];
  return `
    <section class="settings-section">
      <h3>Quality gate</h3>
      <p class="scope-label muted small">Project-wide</p>
      <p class="muted small" style="margin-top:0">
        Choose whether QA runs before merge in the Gap worktree or after the shared application build.
      </p>
      <div class="form-grid two">
        <div class="form-row"><label>${renderSettingsGuideLabel("QA enabled", "quality-enabled")}</label>
          <button type="button"
                  id="s-quality-enabled"
                  data-testid="quality-enabled-toggle"
                  class="${qualityEnabled ? "" : "warn"}"
                  aria-pressed="${qualityEnabled ? "true" : "false"}"
                  data-enabled="${qualityEnabled ? "1" : "0"}">
            QA ${qualityEnabled ? "enabled" : "disabled"}
          </button></div>
        <div class="form-row"><label>${renderSettingsGuideLabel("Quality timing", "quality-gate")}</label>
          <select id="s-quality-timing" aria-label="Quality timing" data-testid="quality-timing-select">
            <option value="pre_merge" ${qualityTiming === "pre_merge" ? "selected" : ""}>Pre-merge QA</option>
            <option value="post_build" ${qualityTiming === "post_build" ? "selected" : ""}>Post-build QA</option>
          </select></div>
      </div>
    </section>

    <section class="settings-section">
      <h3>Regression checks</h3>
      <p class="scope-label muted small">Project-wide</p>
      <p class="muted small" style="margin-top:0">
        Workflow QA runs these checks in the active QA environment. Manual runs
        use the current targeted application checkout.
      </p>
      <div class="form-row"><label>${renderSettingsGuideLabel("Regression checks enabled", "quality-regressions-enabled")}</label>
        <div class="actions settings-section-actions">
          <button type="button"
                  id="s-quality-regressions-enabled"
                  data-testid="quality-regressions-toggle"
                  class="${regressionsEnabled ? "" : "warn"}"
                  aria-pressed="${regressionsEnabled ? "true" : "false"}"
                  data-enabled="${regressionsEnabled ? "1" : "0"}">
            Regressions ${regressionsEnabled ? "enabled" : "disabled"}
          </button>
          <button type="button" class="secondary" id="s-quality-regression-new" data-testid="quality-regression-new">New regression</button>
          <button type="button" class="secondary" id="s-quality-regression-run" data-testid="quality-regression-run" ${regressions.length ? "" : "disabled"}>Run current checkout</button>
        </div>
      </div>
      <div class="settings-list" id="quality-regression-list">
        ${renderQualityRegressionList(regressions)}
      </div>
    </section>`;
}

function renderSettingsQualityProjectSections(quality) {
  return `
    ${renderSettingsMarkdownField({
      id: "s-quality-business-requirements",
      title: "Business requirements",
      value: quality.business_requirements || "",
      scope: "Project-wide",
      description: "Product behavior and requirements the Quality agent checks against tests.",
      rows: 9,
      guideItemId: "quality-requirements",
    })}

    ${renderSettingsMarkdownField({
      id: "s-quality-instructions",
      title: "Instructions",
      value: quality.instructions || "",
      scope: "Project-wide",
      description: "How the Quality agent should choose and evaluate test coverage.",
      rows: 9,
      guideItemId: "quality-instructions",
    })}
    ${quality.configured ? "" : `
      <section class="settings-section settings-quality-configured-message" data-testid="quality-config-warning">
        <p class="muted small" style="color:var(--warn)">
          Quality can run once business requirements and instructions are both filled in.
        </p>
      </section>`}`;
}

function renderSettingsQualityTab(quality) {
  return `
    ${renderSettingsQualityNodeSections(quality)}
    ${renderSettingsQualityProjectSections(quality)}`;
}

function renderQualityRegressionList(regressions) {
  if (!regressions.length) {
    return `<p class="muted small">No managed regressions yet.</p>`;
  }
  return regressions.map((reg) => {
    const latest = reg.latest_run || null;
    const status = latest ? (latest.ok ? "passed" : "failed") : "not run";
    return `
      <div class="settings-list-row"
           data-testid="quality-regression-row"
           data-regression-id="${htmlEscape(reg.id)}">
        <div>
          <strong data-testid="quality-regression-title">${htmlEscape(reg.title || reg.id)}</strong>
          <p class="muted small" style="margin:4px 0 0">${htmlEscape(reg.description || reg.spec_path || "")}</p>
          <p class="muted small" style="margin:4px 0 0" data-testid="quality-regression-last-run">Last run: ${htmlEscape(status)}${latest?.message ? ` - ${htmlEscape(latest.message)}` : ""}</p>
          ${latest?.screenshot_data_url ? `<img class="quality-regression-thumb" alt="" src="${latest.screenshot_data_url}">` : ""}
          ${latest?.screenshot_path ? `<p class="muted small" style="margin:4px 0 0"><code>${htmlEscape(latest.screenshot_path)}</code></p>` : ""}
        </div>
        <div class="actions">
          <button type="button" class="secondary" data-testid="quality-regression-toggle" data-regression-toggle>${reg.enabled ? "Disable" : "Enable"}</button>
          <button type="button" class="danger" data-testid="quality-regression-delete" data-regression-delete>Delete</button>
        </div>
      </div>`;
  }).join("");
}

async function autosaveSettingsQuality(root = document) {
  const body = {};
  const qualityEnabled = root.querySelector("#s-quality-enabled");
  const qualityTiming = root.querySelector("#s-quality-timing");
  const regressionsEnabled = root.querySelector("#s-quality-regressions-enabled");
  const requirements = root.querySelector("#s-quality-business-requirements");
  const instructions = root.querySelector("#s-quality-instructions");
  if (qualityEnabled) body.enabled = qualityEnabled.dataset.enabled === "1" ? "1" : "0";
  if (qualityTiming) body.timing = qualityTiming.value;
  if (regressionsEnabled) {
    body.regressions_enabled = regressionsEnabled.dataset.enabled === "1" ? "1" : "0";
  }
  if (requirements) body.business_requirements = requirements.value;
  if (instructions) body.instructions = instructions.value;
  await api("PATCH", "/api/quality", body);
}

function bindSettingsQualityTab() {
  bindSettingsQualityNodeSections("quality");
  bindSettingsQualityProjectSections("quality");
}

function bindSettingsQualityProjectSections(tabSlug = "runtime") {
  const root = document.querySelector(`[data-tab-pane="${tabSlug}"]`);
  bindSettingsMarkdownFields(root);
  bindSettingsAutosave(
    root,
    "#s-quality-business-requirements, #s-quality-instructions",
    () => autosaveSettingsQuality(root),
  );
}

function bindSettingsQualityNodeSections(tabSlug = "nodes") {
  const root = document.querySelector(`[data-tab-pane="${tabSlug}"]`);
  const autosaveQuality = createSettingsAutosave(
    () => autosaveSettingsQuality(root),
    {
      controls: $$("#s-quality-enabled, #s-quality-timing, #s-quality-regressions-enabled", root),
      errorPrefix: "Save failed",
    },
  );
  $("#s-quality-enabled")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    btn.dataset.settingsSavedValue = btn.dataset.enabled || "0";
    const enabled = btn.dataset.enabled !== "1";
    btn.dataset.enabled = enabled ? "1" : "0";
    btn.setAttribute("aria-pressed", enabled ? "true" : "false");
    btn.classList.toggle("warn", !enabled);
    btn.textContent = enabled ? "QA enabled" : "QA disabled";
    await withButtonBusy(btn, "Saving...", async () => {
      try {
        await autosaveQuality();
      } catch (err) { await showActionError(err); }
    });
  });
  $("#s-quality-timing")?.addEventListener("change", async () => {
    await autosaveQuality();
  });
  $("#s-quality-regressions-enabled")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    btn.dataset.settingsSavedValue = btn.dataset.enabled || "0";
    const enabled = btn.dataset.enabled !== "1";
    btn.dataset.enabled = enabled ? "1" : "0";
    btn.setAttribute("aria-pressed", enabled ? "true" : "false");
    btn.classList.toggle("warn", !enabled);
    btn.textContent = enabled ? "Regressions enabled" : "Regressions disabled";
    await withButtonBusy(btn, "Saving...", async () => {
      try {
        await autosaveQuality();
      } catch (err) { await showActionError(err); }
    });
  });
  bindCommand("#s-quality-regression-new", "quality.regression.new");
  bindCommand("#s-quality-regression-run", "quality.regression.run");
  $$("[data-regression-toggle]", root).forEach((btn) => {
    btn.addEventListener("click", async () => {
      const row = btn.closest("[data-regression-id]");
      const id = row?.dataset.regressionId || "";
      const enabled = btn.textContent.trim() !== "Disable";
      await withButtonBusy(btn, enabled ? "Enabling..." : "Disabling...", async () => {
        try {
          await api("PATCH", `/api/quality/regressions/${id}`, { enabled });
          await refreshSettingsTab("quality", { force: true });
        } catch (e) { await showActionError(e); }
      });
    });
  });
  $$("[data-regression-delete]", root).forEach((btn) => {
    btn.addEventListener("click", async () => {
      const row = btn.closest("[data-regression-id]");
      const id = row?.dataset.regressionId || "";
      const ok = await modalConfirm("Delete this managed regression?", {
        title: "Delete regression",
        okLabel: "Delete",
        danger: true,
      });
      if (!ok) return;
      await withButtonBusy(btn, "Deleting...", async () => {
        try {
          await api("DELETE", `/api/quality/regressions/${id}`);
          await refreshSettingsTab("quality", { force: true });
        } catch (e) { await showActionError(e); }
      });
    });
  });
}

async function openRegressionCreateModal(initialPrompt = "", button = null) {
  const values = await new Promise((resolve) => {
    const root = document.createElement("div");
    root.className = "modal-backdrop";
    root.innerHTML = `
      <div class="modal regression-create-modal" role="dialog" aria-modal="true"
           aria-labelledby="regression-create-title"
           data-testid="quality-regression-modal">
        <div class="modal-title" id="regression-create-title">New regression</div>
        <div class="modal-body">
          <form id="regression-create-form">
            <div class="form-row">
              <label>${renderSettingsGuideLabel("Title", "quality-regression-title")}</label>
              <input type="text" id="regression-create-input-title"
                     data-testid="quality-regression-title-input"
                     placeholder="Dashboard smoke">
            </div>
            <div class="form-row">
              <label>${renderSettingsGuideLabel("Scenario", "quality-regression-scenario")}</label>
              <textarea id="regression-create-input-prompt" rows="7"
                        data-testid="quality-regression-prompt-input"
                        placeholder="Navigate to the page, set up the state, wait for the key selector, then capture a screenshot.">${htmlEscape(initialPrompt || "")}</textarea>
            </div>
          </form>
        </div>
        <div class="modal-actions">
          <button class="secondary" data-testid="quality-regression-cancel" data-cancel>Cancel</button>
          <button data-testid="quality-regression-create" data-ok>Create</button>
        </div>
      </div>`;
    document.body.appendChild(root);

    let closed = false;
    function close(value) {
      if (closed) return;
      closed = true;
      document.removeEventListener("keydown", onKey, true);
      root.remove();
      resolve(value);
    }
    function submit() {
      const title = root.querySelector("#regression-create-input-title")?.value.trim() || "";
      const prompt = root.querySelector("#regression-create-input-prompt")?.value.trim() || "";
      if (!title && !prompt) {
        toast("Provide a title or scenario first.", "error");
        root.querySelector("#regression-create-input-title")?.focus();
        return;
      }
      close({ title, prompt });
    }
    function onKey(e) {
      if (e.key === "Escape") {
        e.preventDefault();
        close(null);
      } else if (e.key === "Enter") {
        if (e.target && e.target.tagName === "TEXTAREA") return;
        e.preventDefault();
        submit();
      }
    }
    document.addEventListener("keydown", onKey, true);
    root.addEventListener("click", (e) => {
      if (e.target === root) close(null);
    });
    root.querySelector("[data-cancel]")?.addEventListener("click", () => close(null));
    root.querySelector("[data-ok]")?.addEventListener("click", submit);
    root.querySelector("#regression-create-form")?.addEventListener("submit", (e) => {
      e.preventDefault();
      submit();
    });
    root.querySelector("#regression-create-input-title")?.focus();
  });
  if (!values) return null;
  const title = values.title;
  const prompt = values.prompt;
  return await withButtonBusy(button, "Creating...", async () => {
    const result = await api("POST", "/api/quality/regressions", {
      title,
      prompt,
      description: prompt,
    });
    toast("Regression created", "info");
    if (state.currentRoute === "project") await refreshSettingsTab("quality", { force: true });
    return result.regression;
  });
}

async function runQualityRegressions(button = null) {
  await withButtonBusy(button, "Running...", async () => {
    const result = await api("POST", "/api/quality/regressions/run", {});
    toast(result.message || "Current-checkout regression run complete", result.ok ? "info" : "warn");
    if (state.currentRoute === "project") await refreshSettingsTab("quality", { force: true });
  });
}
