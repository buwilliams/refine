// ---- System / Quality -------------------------------------------------------

function renderSettingsQualityNodeSections(quality, settings = {}) {
  return `
    <section class="settings-section">
      <h3>Workflow position</h3>
      <p class="scope-label muted small">Project-wide</p>
      <p class="muted small" style="margin-top:0">
        Quality always reviews and corrects the isolated candidate after Implement and before Governance.
        Automatic post-merge rebuild is not part of Goal workflow.
      </p>
    </section>`;
}

function renderSettingsQualityProjectSections(quality) {
  const tests = Array.isArray(quality.tests) ? quality.tests.join("\n") : "";
  const legacyCommands = Array.isArray(quality.legacy_commands) ? quality.legacy_commands : [];
  return `
    ${legacyCommands.length ? `
      <section class="settings-section" data-testid="quality-legacy-transition">
        <h3>Migrated Quality commands</h3>
        <p class="muted small">These migrated target-app test commands remain enforced: ${legacyCommands.map(escapeHtml).join(", ")}. Saving at least one plain-text test replaces them.</p>
      </section>` : ""}
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

    ${renderSettingsMarkdownField({
      id: "s-quality-tests",
      title: "Tests",
      value: tests,
      scope: "Project-wide · one plain-text test per line",
      description: "Observable outcomes the Quality agent evaluates. The agent decides how to run each test and reports pass or fail.",
      rows: 9,
      guideItemId: "quality-tests",
    })}`;
}

function renderSettingsQualityTab(quality, settings = {}) {
  return `
    <section class="settings-section" data-testid="quality-explanation">
      <h3>How Quality works</h3>
      <p class="muted small" style="margin-bottom:0">
        For every Goal candidate, the configured agent evaluates each plain-text Quality test
        and proposes the appropriate command. Refine runs that command as a supervised process;
        its observed exit and output are the authoritative evidence for pass or fail.
        Passing checks advance the Goal to Governance; failures preserve the candidate for recovery
        and stop the workflow. Existing repository tests may be sufficient when they cover the change. Changes save automatically
        and do not start a run now.
      </p>
    </section>

    ${renderSettingsQualityNodeSections(quality, settings)}
    ${renderSettingsQualityProjectSections(quality)}`;
}

async function autosaveSettingsQuality(root = document) {
  const body = {};
  const requirements = root.querySelector("#s-quality-business-requirements");
  const instructions = root.querySelector("#s-quality-instructions");
  const tests = root.querySelector("#s-quality-tests");
  if (requirements) body.business_requirements = requirements.value;
  if (instructions) body.instructions = instructions.value;
  if (tests) body.tests = tests.value.split(/\r?\n/).map((test) => test.trim()).filter(Boolean);
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
    "#s-quality-business-requirements, #s-quality-instructions, #s-quality-tests",
    () => autosaveSettingsQuality(root),
  );
}

function bindSettingsQualityNodeSections(tabSlug = "nodes") {
  return tabSlug;
}
