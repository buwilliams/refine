// ---- System / Quality -------------------------------------------------------

function renderSettingsQualityTab(quality) {
  const qualityEnabled = String(quality.enabled || "0") === "1";
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

async function autosaveSettingsQuality() {
  await api("PATCH", "/api/quality", {
    enabled: $("#s-quality-enabled").dataset.enabled === "1" ? "1" : "0",
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
}
