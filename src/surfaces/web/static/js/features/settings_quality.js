// ---- System / Quality -------------------------------------------------------

function renderSettingsQualityNodeSections(quality, settings = {}) {
  const qualityEnabled = String(quality.enabled || "0") === "1";
  const qualityTiming = quality.timing === "post_build" ? "post_build" : "pre_merge";
  return `
    <section class="settings-section">
      <h3>Quality gate</h3>
      <p class="scope-label muted small">Project-wide</p>
      <p class="muted small" style="margin-top:0">
        QA runs the target-app test command as a supervised workflow subprocess.
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
      ${renderTargetAppTestCommandsField(settings, {
        guideItemId: "application-test",
        description: "CLI commands Refine runs during workflow QA.",
      })}
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

function renderSettingsQualityTab(quality, settings = {}) {
  return `
    ${renderSettingsQualityNodeSections(quality, settings)}
    ${renderSettingsQualityProjectSections(quality)}`;
}

async function autosaveSettingsQuality(root = document) {
  const body = {};
  const qualityEnabled = root.querySelector("#s-quality-enabled");
  const qualityTiming = root.querySelector("#s-quality-timing");
  const requirements = root.querySelector("#s-quality-business-requirements");
  const instructions = root.querySelector("#s-quality-instructions");
  if (qualityEnabled) body.enabled = qualityEnabled.dataset.enabled === "1" ? "1" : "0";
  if (qualityTiming) body.timing = qualityTiming.value;
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
  bindTargetAppTestCommandList(root);
  bindSettingsAutosave(
    root,
    "#s-target-test-commands",
    () => autosaveSettingsTargetAppTests(root),
    { event: "settings-editable-commit" },
  );
  bindSettingsEditableFields(root);
  const autosaveQuality = createSettingsAutosave(
    () => autosaveSettingsQuality(root),
    {
      controls: $$("#s-quality-enabled, #s-quality-timing", root),
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
}
