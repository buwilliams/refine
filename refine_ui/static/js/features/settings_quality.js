// ---- System / Quality -------------------------------------------------------

function renderSettingsQualityTab(quality) {
  const qualityEnabled = String(quality.enabled || "0") === "1";
  const regressionsEnabled = String(quality.regressions_enabled || "0") === "1";
  const regressions = Array.isArray(quality.regressions) ? quality.regressions : [];
  return `
    <section class="settings-section">
      <h3>Quality gate</h3>
      <p class="scope-label muted small">Instance-scoped</p>
      <p class="muted small" style="margin-top:0">
        Runs pre-merge QA in the Gap worktree before the Merge agent lands work.
      </p>
      <button type="button"
              id="s-quality-enabled"
              class="${qualityEnabled ? "" : "warn"}"
              aria-pressed="${qualityEnabled ? "true" : "false"}"
              data-enabled="${qualityEnabled ? "1" : "0"}">
        QA ${qualityEnabled ? "enabled" : "disabled"}
      </button>
    </section>

    <section class="settings-section">
      <h3>Regression checks</h3>
      <p class="scope-label muted small">Instance-scoped</p>
      <p class="muted small" style="margin-top:0">
        Workflow QA runs these checks against each Gap worktree. Manual runs
        use the current targeted application checkout.
      </p>
      <div class="actions settings-section-actions">
        <button type="button"
                id="s-quality-regressions-enabled"
                class="${regressionsEnabled ? "" : "warn"}"
                aria-pressed="${regressionsEnabled ? "true" : "false"}"
                data-enabled="${regressionsEnabled ? "1" : "0"}">
          Regressions ${regressionsEnabled ? "enabled" : "disabled"}
        </button>
        <button type="button" class="secondary" id="s-quality-regression-new">New regression</button>
        <button type="button" class="secondary" id="s-quality-regression-run" ${regressions.length ? "" : "disabled"}>Run current checkout</button>
      </div>
      <div class="settings-list" id="quality-regression-list">
        ${renderQualityRegressionList(regressions)}
      </div>
    </section>

    <section class="settings-section">
      <h3>Business requirements</h3>
      <p class="scope-label muted small">Project-wide</p>
      <p class="muted small" style="margin-top:0">
        Product behavior and requirements the Quality agent checks against tests.
      </p>
      <textarea id="s-quality-business-requirements" rows="9">${htmlEscape(quality.business_requirements || "")}</textarea>
    </section>

    <section class="settings-section">
      <h3>Instructions</h3>
      <p class="muted small" style="margin-top:0">
        How the Quality agent should choose and evaluate test coverage.
      </p>
      <textarea id="s-quality-instructions" rows="9">${htmlEscape(quality.instructions || "")}</textarea>
      ${quality.configured ? "" : `
        <p class="muted small" style="color:var(--warn)">
          Quality can run once business requirements and instructions are both filled in.
        </p>`}
    </section>`;
}

function renderQualityRegressionList(regressions) {
  if (!regressions.length) {
    return `<p class="muted small">No managed regressions yet.</p>`;
  }
  return regressions.map((reg) => {
    const latest = reg.latest_run || null;
    const status = latest ? (latest.ok ? "passed" : "failed") : "not run";
    return `
      <div class="settings-list-row" data-regression-id="${htmlEscape(reg.id)}">
        <div>
          <strong>${htmlEscape(reg.title || reg.id)}</strong>
          <p class="muted small" style="margin:4px 0 0">${htmlEscape(reg.description || reg.spec_path || "")}</p>
          <p class="muted small" style="margin:4px 0 0">Last run: ${htmlEscape(status)}${latest?.message ? ` - ${htmlEscape(latest.message)}` : ""}</p>
          ${latest?.screenshot_data_url ? `<img class="quality-regression-thumb" alt="" src="${latest.screenshot_data_url}">` : ""}
          ${latest?.screenshot_path ? `<p class="muted small" style="margin:4px 0 0"><code>${htmlEscape(latest.screenshot_path)}</code></p>` : ""}
        </div>
        <div class="actions">
          <button type="button" class="secondary" data-regression-toggle>${reg.enabled ? "Disable" : "Enable"}</button>
          <button type="button" class="danger" data-regression-delete>Delete</button>
        </div>
      </div>`;
  }).join("");
}

async function autosaveSettingsQuality() {
  await api("PATCH", "/api/quality", {
    enabled: $("#s-quality-enabled").dataset.enabled === "1" ? "1" : "0",
    regressions_enabled: $("#s-quality-regressions-enabled").dataset.enabled === "1" ? "1" : "0",
    business_requirements: $("#s-quality-business-requirements").value,
    instructions: $("#s-quality-instructions").value,
  });
}

function bindSettingsQualityTab() {
  const root = document.querySelector('[data-tab-pane="quality"]');
  const autosaveQuality = bindSettingsAutosave(
    root,
    "#s-quality-business-requirements, #s-quality-instructions",
    autosaveSettingsQuality,
  );
  $("#s-quality-enabled")?.addEventListener("click", () => {
    const btn = $("#s-quality-enabled");
    const enabled = btn.dataset.enabled !== "1";
    btn.dataset.enabled = enabled ? "1" : "0";
    btn.setAttribute("aria-pressed", enabled ? "true" : "false");
    btn.classList.toggle("warn", !enabled);
    btn.textContent = enabled ? "QA enabled" : "QA disabled";
    autosaveQuality();
  });
  $("#s-quality-regressions-enabled")?.addEventListener("click", () => {
    const btn = $("#s-quality-regressions-enabled");
    const enabled = btn.dataset.enabled !== "1";
    btn.dataset.enabled = enabled ? "1" : "0";
    btn.setAttribute("aria-pressed", enabled ? "true" : "false");
    btn.classList.toggle("warn", !enabled);
    btn.textContent = enabled ? "Regressions enabled" : "Regressions disabled";
    autosaveQuality();
  });
  bindCommand("#s-quality-regression-new", "quality.regression.new");
  bindCommand("#s-quality-regression-run", "quality.regression.run");
  $$("[data-regression-toggle]", root).forEach((btn) => {
    btn.addEventListener("click", async () => {
      const row = btn.closest("[data-regression-id]");
      const id = row?.dataset.regressionId || "";
      const enabled = btn.textContent.trim() !== "Disable";
      await api("PATCH", `/api/quality/regressions/${id}`, { enabled });
      await refreshSettingsTab("quality", { force: true });
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
      await api("DELETE", `/api/quality/regressions/${id}`);
      await refreshSettingsTab("quality", { force: true });
    });
  });
}

async function openRegressionCreateModal(initialPrompt = "") {
  const values = await new Promise((resolve) => {
    const root = document.createElement("div");
    root.className = "modal-backdrop";
    root.innerHTML = `
      <div class="modal regression-create-modal" role="dialog" aria-modal="true"
           aria-labelledby="regression-create-title">
        <div class="modal-title" id="regression-create-title">New regression</div>
        <div class="modal-body">
          <form id="regression-create-form">
            <div class="form-row">
              <label>Title</label>
              <input type="text" id="regression-create-input-title"
                     placeholder="Dashboard smoke">
            </div>
            <div class="form-row">
              <label>Scenario</label>
              <textarea id="regression-create-input-prompt" rows="7"
                        placeholder="Navigate to the page, set up the state, wait for the key selector, then capture a screenshot.">${htmlEscape(initialPrompt || "")}</textarea>
            </div>
          </form>
        </div>
        <div class="modal-actions">
          <button class="secondary" data-cancel>Cancel</button>
          <button data-ok>Create</button>
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
  const result = await api("POST", "/api/quality/regressions", {
    title,
    prompt,
    description: prompt,
  });
  toast("Regression created", "info");
  if (state.currentRoute === "settings") await refreshSettingsTab("quality", { force: true });
  return result.regression;
}

async function runQualityRegressions(button = null) {
  await withButtonBusy(button, "Running...", async () => {
    const result = await api("POST", "/api/quality/regressions/run", {});
    toast(result.message || "Current-checkout regression run complete", result.ok ? "info" : "warn");
    if (state.currentRoute === "settings") await refreshSettingsTab("quality", { force: true });
  });
}
