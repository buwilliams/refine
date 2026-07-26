const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

const SETTINGS_DIR = path.join(
  __dirname,
  "../src/surfaces/web/static/js/features",
);

function settingsSources() {
  return fs.readdirSync(SETTINGS_DIR)
    .filter((name) => /^settings.*\.js$/.test(name))
    .sort()
    .map((name) => ({
      name,
      source: fs.readFileSync(path.join(SETTINGS_DIR, name), "utf8"),
    }));
}

function directInnerHtmlAssignments({ name, source }) {
  let currentFunction = "<top-level>";
  return source.split("\n").flatMap((line, index) => {
    const declaration = line.match(/^(?:async\s+)?function\s+([A-Za-z0-9_$]+)\s*\(/);
    if (declaration) currentFunction = declaration[1];
    return /\.innerHTML\s*=/.test(line)
      ? [{ file: name, line: index + 1, functionName: currentFunction }]
      : [];
  });
}

// These are route-entry paints, modal first paints, or narrowly local editor
// mutations. Recurring, SSE, background, and post-save redraws are deliberately
// absent: adding an innerHTML assignment to one of those paths fails this audit.
const DOCUMENTED_LOCAL_INNER_HTML = new Set([
  "settings.js:renderSettingsSurface",
  "settings.js:setSettingsEditableButtonState",
  "settings.js:updateSettingsEditablePreview",
  "settings.js:setSettingsMarkdownButtonState",
  "settings.js:commitSettingsMarkdownField",
  "settings_governance.js:bindGovernanceRuleButtons",
  "settings_governance.js:addGovernanceRuleRow",
  "settings_guidance.js:openGuidanceModal",
  "settings_nodes.js:openNodeConnectionModal",
  "settings_target_app_tests.js:bindTargetAppTestCommandList",
]);

test("settings direct innerHTML is limited to documented first paints and local edits", () => {
  const assignments = settingsSources().flatMap(directInnerHtmlAssignments);
  const unexpected = assignments.filter((assignment) =>
    !DOCUMENTED_LOCAL_INNER_HTML.has(
      `${assignment.file}:${assignment.functionName}`,
    ));

  assert.deepEqual(
    unexpected,
    [],
    `settings refresh paths must use renderInto:\n${unexpected
      .map((item) => `${item.file}:${item.line} (${item.functionName})`)
      .join("\n")}`,
  );
});

test("every recurring settings redraw names the shared morph contract", () => {
  const combined = new Map(settingsSources().map(({ name, source }) => [name, source]));
  const expected = [
    ["settings.js", "drawSettingsSurface"],
    ["settings.js", "drawRuntimeRecovery"],
    ["settings.js", "drawSqliteCacheProgress"],
    ["settings_releases.js", "applySourcePromotionStatus"],
    ["settings_releases.js", "refreshSourcePromotionStatus"],
    ["settings_releases.js", "previewRelease"],
    ["settings_runtime.js", "refreshRuntimeUpgradeBanner"],
    ["settings_processes.js", "refreshTargetAppStatus"],
    ["settings_processes.js", "drawTargetAppStatusBlock"],
    ["settings_governance.js", "bindSettingsGovernanceTab"],
  ];

  for (const [file, functionName] of expected) {
    const source = combined.get(file);
    const start = source.indexOf(`function ${functionName}(`);
    assert.notEqual(start, -1, `${file} is missing ${functionName}`);
    const next = source.indexOf("\nfunction ", start + 1);
    const body = source.slice(start, next === -1 ? source.length : next);
    assert.match(
      body,
      /\brenderInto\s*\(/,
      `${file}:${functionName} must redraw with renderInto`,
    );
  }
});
