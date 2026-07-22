const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const staticRoot = path.join(__dirname, "../src/surfaces/web/static");
const source = fs.readFileSync(
  path.join(staticRoot, "js/features/feature-goal-inline.js"),
  "utf8",
);
const featureSource = fs.readFileSync(
  path.join(staticRoot, "js/features/features.js"),
  "utf8",
);
const styles = fs.readFileSync(path.join(staticRoot, "css/modals.css"), "utf8");

function inlineRuntime() {
  const requests = [];
  const context = vm.createContext({
    api: async (method, requestPath, body) => {
      requests.push({ method, path: requestPath, body });
      return {};
    },
    encodeURIComponent,
    htmlEscape: (value) => String(value)
      .replaceAll("&", "&amp;")
      .replaceAll('"', "&quot;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;"),
    Set,
  });
  vm.runInContext(source, context);
  vm.runInContext(`
    globalThis.inlineFeatureGoalTest = {
      applyPlacement: applyFeatureGoalPlacement,
      canEdit: featureGoalCanInlineEdit,
      placement: featureGoalPlacementValue,
      renderComposer: renderFeatureGoalInlineComposer,
      save: saveFeatureGoalInline,
    };
  `, context);
  return { requests, runtime: context.inlineFeatureGoalTest };
}

test("Feature detail uses one inline create/edit composer instead of the nested Goal modal", () => {
  const browser = inlineRuntime();
  const html = browser.runtime.renderComposer([
    { id: "GOAL1", name: "Foundation", feature_order: 1 },
  ], "Buddy");

  assert.match(html, /data-testid="feature-goal-composer"/);
  assert.match(html, /data-testid="feature-goal-form"/);
  assert.match(html, /Sequence \/ dependency/);
  assert.match(html, /After Foundation/);
  assert.match(html, /aria-live="polite"/);
  assert.match(html, /Submitting as <strong class="js-reporter-name">Buddy<\/strong>/);
  assert.match(source, /event\.ctrlKey \|\| event\.metaKey/);
  assert.doesNotMatch(featureSource, /openFeatureNewGoalFlow/);
  assert.doesNotMatch(featureSource, /data-feature-new-goal/);
  assert.match(featureSource, /data-feature-edit-goal/);
});

test("Feature Goal sequence placement preserves independent and prerequisite ordering APIs", async () => {
  const browser = inlineRuntime();
  const goals = [
    { id: "GOAL1", name: "Foundation", feature_order: 1 },
    { id: "GOAL2", name: "UI", feature_order: 2 },
    { id: "GOAL3", name: "Docs", feature_order: null },
  ];

  assert.equal(browser.runtime.placement(goals[0], goals), "first");
  assert.equal(browser.runtime.placement(goals[1], goals), "GOAL1");
  assert.equal(browser.runtime.placement(goals[2], goals), "unordered");
  assert.equal(browser.runtime.canEdit({ status: "todo" }), true);
  assert.equal(browser.runtime.canEdit({ status: "done" }), false);

  await browser.runtime.applyPlacement("FEATURE1", goals[2], "GOAL1");
  assert.deepEqual(JSON.parse(JSON.stringify(browser.requests)), [
    {
      method: "POST",
      path: "/api/features/FEATURE1/goals/GOAL3/order",
    },
    {
      method: "POST",
      path: "/api/features/FEATURE1/goals/GOAL3/reorder",
      body: { after: "GOAL1" },
    },
  ]);
});

test("inline editing reuses Goal metadata and latest-round backend contracts", async () => {
  const browser = inlineRuntime();
  await browser.runtime.save("FEATURE1", {
    id: "GOAL2",
    name: "Old name",
    priority: "low",
    assignee: "Owner",
    rounds: [{ prompt: "Old prompt" }],
  }, {
    reporter: "Buddy",
    prompt: "Revised prompt",
    name: "Revised name",
    priority: "high",
  });
  assert.deepEqual(JSON.parse(JSON.stringify(browser.requests)), [
    {
      method: "PATCH",
      path: "/api/goals/GOAL2",
      body: { name: "Revised name", priority: "high" },
    },
    {
      method: "PATCH",
      path: "/api/goals/GOAL2/rounds/latest",
      body: { reporter: "Buddy", assignee: "Owner", prompt: "Revised prompt" },
    },
  ]);
});

test("Feature Goal composer has deterministic desktop and narrow layout coverage", () => {
  assert.match(
    styles,
    /\.feature-goal-composer-fields\s*\{[^}]*display:\s*grid;[^}]*grid-template-columns:\s*minmax\(0, 3fr\) minmax\(250px, 2fr\);/s,
  );
  assert.match(
    styles,
    /@media \(max-width: 760px\)[\s\S]*?\.feature-goal-composer-fields\s*\{[^}]*display:\s*block;/,
  );
  assert.match(
    styles,
    /@media \(max-width: 760px\)[\s\S]*?\.feature-goal-composer-actions\s*\{[^}]*flex-wrap:\s*wrap;/,
  );
});
