"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const { BrowserEvent, computedStyle, createBrowserDom } = require("./support/browser_dom");

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

function htmlEscape(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll('"', "&quot;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
}

function duplicatePrompt() {
  return `
    <div class="duplicate-prompt">
      <button type="button" data-duplicate-decision="duplicate">Use existing</button>
      <button type="button" data-duplicate-decision="original">Create anyway</button>
      <button type="button" data-duplicate-decision="move_original_to_backlog">Move original</button>
    </div>`;
}

function featureFixture(overrides = {}) {
  return {
    id: "FEATURE1",
    goals: [
      {
        id: "GOAL1",
        name: "Foundation",
        status: "todo",
        feature_order: 1,
        feature_authoring: { editable: true, reason: null },
      },
      {
        id: "GOAL2",
        name: "Review UI",
        status: "review",
        feature_order: 2,
        feature_authoring: { editable: true, reason: null },
      },
    ],
    ...overrides,
  };
}

function composerRuntime({ feature = featureFixture(), reporter = "Buddy", apiHandler } = {}) {
  const requests = [];
  const opened = [];
  const toasts = [];
  const actionErrors = [];
  let handler = apiHandler || (async (method, requestPath) => {
    if (method === "GET" && requestPath.startsWith("/api/features/")) return { feature };
    return { created: true, goal: { id: "GOAL3" } };
  });
  const context = vm.createContext({
    api: async (method, requestPath, body, options) => {
      requests.push({ method, path: requestPath, body, options });
      return handler(method, requestPath, body, options);
    },
    encodeURIComponent,
    htmlEscape,
    openFeatureModal: (nextFeature, options) => opened.push({ feature: nextFeature, options }),
    renderGoalDuplicatePrompt: duplicatePrompt,
    showActionError: async (error, title) => actionErrors.push({ error, title }),
    state: { lastReporter: reporter },
    toast: (message, kind) => toasts.push({ message, kind }),
  });
  vm.runInContext(source, context);
  vm.runInContext(`
    globalThis.featureGoalComposerTest = {
      bind: bindFeatureGoalInlineComposer,
      canEdit: featureGoalCanInlineEdit,
      placementRequest: featureGoalPlacementRequest,
      render: renderFeatureGoalInlineComposer,
    };
  `, context);
  const runtime = context.featureGoalComposerTest;
  const editButtons = feature.goals.map((goal) =>
    `<button type="button" data-feature-edit-goal="${goal.id}">Edit ${goal.id}</button>`).join("");
  const markup = `<div class="feature-modal">${editButtons}${runtime.render(feature.goals, reporter)}</div>`;
  const dom = createBrowserDom(markup);
  runtime.bind(dom.root, feature, { goalPage: 3, navigateAway: true });
  return {
    ...dom,
    actionErrors,
    context,
    feature,
    opened,
    requests,
    runtime,
    setApiHandler(next) { handler = next; },
    toasts,
  };
}

function formFields(browser) {
  const form = browser.root.querySelector("[data-feature-goal-form]");
  return {
    form,
    goal_id: form.elements.goal_id,
    name: form.elements.name,
    placement: form.elements.placement,
    priority: form.elements.priority,
    prompt: form.elements.prompt,
  };
}

async function settle() {
  for (let index = 0; index < 4; index += 1) await Promise.resolve();
  await new Promise((resolve) => setImmediate(resolve));
}

test("Feature detail binds one real inline create interaction and preserves prerequisite pagination", async () => {
  const browser = composerRuntime();
  const fields = formFields(browser);
  fields.prompt.value = "Create the interaction coverage";
  fields.name.value = "Interaction coverage";
  fields.priority.value = "high";
  fields.placement.value = "GOAL1";

  fields.prompt.dispatchEvent(new BrowserEvent("keydown", { key: "Enter", ctrlKey: true }));
  await settle();

  const mutations = browser.requests.filter((request) => request.method !== "GET");
  assert.equal(mutations.length, 1);
  assert.equal(mutations[0].path, "/api/features/FEATURE1/goals/author");
  assert.deepEqual(JSON.parse(JSON.stringify(mutations[0].body)), {
    name: "Interaction coverage",
    reporter: "Buddy",
    assignee: "Buddy",
    prompt: "Create the interaction coverage",
    priority: "high",
    placement: { after: "GOAL1" },
  });
  assert.equal(browser.opened.length, 1);
  assert.deepEqual(JSON.parse(JSON.stringify(browser.opened[0].options)), {
    goalPage: 3,
    navigateAway: true,
    focusComposer: true,
  });
  assert.deepEqual(browser.toasts[0], { message: "Goal created", kind: "success" });
});

test("review-state inline edit follows API capability and saves metadata, round, and placement once", async () => {
  const reviewGoal = {
    id: "GOAL2",
    name: "Review UI",
    status: "review",
    priority: "low",
    assignee: "Owner",
    feature_order: 2,
    rounds: [{ prompt: "Old prompt" }],
  };
  const browser = composerRuntime({
    apiHandler: async (method, requestPath) => {
      if (method === "GET" && requestPath === "/api/goals/GOAL2") return { goal: reviewGoal };
      if (method === "GET") return { feature: featureFixture() };
      return { created: false, goal: { ...reviewGoal, name: "Reviewed" } };
    },
  });
  assert.equal(browser.runtime.canEdit(browser.feature.goals[1]), true);
  browser.root.querySelector('[data-feature-edit-goal="GOAL2"]').click();
  await settle();

  const fields = formFields(browser);
  assert.equal(fields.prompt.value, "Old prompt");
  assert.equal(fields.name.value, "Review UI");
  assert.equal(fields.placement.value, "GOAL1");
  fields.prompt.value = "Revised in review";
  fields.name.value = "";
  fields.form.requestSubmit();
  assert.equal(browser.root.querySelector("[data-feature-goal-form-status]").textContent,
    "Name is required when editing.");
  assert.equal(browser.requests.filter((request) => request.method === "POST").length, 0);
  fields.name.value = "Reviewed";
  fields.priority.value = "medium";
  fields.placement.value = "first";
  fields.form.requestSubmit();
  await settle();

  const author = browser.requests.find((request) => request.method === "POST");
  assert.equal(author.path, "/api/features/FEATURE1/goals/author");
  assert.deepEqual(JSON.parse(JSON.stringify(author.body)), {
    goal_id: "GOAL2",
    name: "Reviewed",
    reporter: "Buddy",
    assignee: "Owner",
    prompt: "Revised in review",
    priority: "medium",
    placement: "first",
  });
  assert.equal(browser.requests.filter((request) => request.method === "POST").length, 1);
  assert.deepEqual(browser.toasts[0], { message: "Goal updated", kind: "success" });
});

test("duplicate and validation errors stay in the bound composer and recover without losing context", async () => {
  let authorAttempts = 0;
  const browser = composerRuntime({
    reporter: "",
    apiHandler: async (method, requestPath, body) => {
      if (method === "GET") return { feature: featureFixture() };
      authorAttempts += 1;
      if (authorAttempts === 1) {
        const error = new Error("Possible duplicate Goal");
        error.code = "duplicate_goal";
        error.error = { duplicate: { match: { id: "GOAL9", name: "Existing" } } };
        throw error;
      }
      return { created: true, goal: { id: "GOAL3" }, received: body };
    },
  });
  const fields = formFields(browser);
  fields.form.requestSubmit();
  assert.equal(browser.root.querySelector("[data-feature-goal-form-status]").textContent,
    "Pick a reporter in the top-right selector first.");
  assert.equal(browser.requests.length, 0);

  browser.context.state.lastReporter = "Buddy";
  fields.form.requestSubmit();
  assert.equal(browser.root.querySelector("[data-feature-goal-form-status]").textContent,
    "Prompt is required.");
  assert.equal(browser.requests.length, 0);

  fields.prompt.value = "Possible duplicate";
  fields.form.requestSubmit();
  await settle();
  assert.equal(browser.root.querySelector("[data-feature-goal-form-status]").textContent,
    "Choose how to handle the possible duplicate.");
  assert.ok(browser.root.querySelector('[data-duplicate-decision="original"]'));
  assert.equal(fields.prompt.value, "Possible duplicate");
});

test("duplicate choice retries through the same API and generic failures retain a retryable draft", async () => {
  let attempt = 0;
  const browser = composerRuntime({
    apiHandler: async (method) => {
      if (method === "GET") return { feature: featureFixture() };
      attempt += 1;
      if (attempt === 1) {
        const error = new Error("Possible duplicate Goal");
        error.code = "duplicate_goal";
        error.error = { duplicate: { match: { id: "GOAL9", name: "Existing" } } };
        throw error;
      }
      if (attempt === 2) throw new Error("Provider unavailable");
      return { created: true, goal: { id: "GOAL3" } };
    },
  });
  const fields = formFields(browser);
  fields.prompt.value = "Keep this draft";
  fields.form.requestSubmit();
  await settle();
  assert.equal(browser.root.querySelector("[data-feature-goal-form-status]").textContent,
    "Choose how to handle the possible duplicate.");
  browser.root.querySelector('[data-duplicate-decision="original"]').click();
  fields.form.requestSubmit();
  await settle();

  assert.equal(browser.requests.filter((request) => request.method === "POST").length, 2);
  const retry = browser.requests.filter((request) => request.method === "POST")[1];
  assert.equal(retry.body.duplicate_decision, "original");
  assert.equal(fields.prompt.value, "Keep this draft");
  assert.equal(browser.root.querySelector("[data-feature-composer-submit]").disabled, false);
  assert.equal(browser.root.querySelector("[data-feature-goal-form-status]").textContent,
    "Provider unavailable");
  assert.equal(browser.actionErrors.length, 1);
});

test("Cmd+Enter submits and Escape resets the live composer without closing Feature context", async () => {
  const browser = composerRuntime();
  const fields = formFields(browser);
  fields.prompt.value = "Submit from macOS";
  fields.prompt.dispatchEvent(new BrowserEvent("keydown", { key: "Enter", metaKey: true }));
  await settle();
  assert.equal(browser.requests.filter((request) => request.method === "POST").length, 1);

  fields.prompt.value = "Discard me";
  fields.name.value = "Draft";
  const escape = new BrowserEvent("keydown", { key: "Escape" });
  fields.prompt.dispatchEvent(escape);
  assert.equal(escape.defaultPrevented, true);
  assert.equal(escape.propagationStopped, true);
  assert.equal(fields.prompt.value, "");
  assert.equal(fields.name.value, "");
  assert.equal(browser.document.activeElement, fields.prompt);
});

test("desktop and narrow viewports compute the intended layout on rendered composer elements", () => {
  const browser = composerRuntime();
  const fields = browser.root.querySelector(".feature-goal-composer-fields");
  const actions = browser.root.querySelector(".feature-goal-composer-actions");
  assert.equal(computedStyle(fields, styles, 1200).display, "grid");
  assert.equal(
    computedStyle(fields, styles, 1200)["grid-template-columns"],
    "minmax(0, 3fr) minmax(250px, 2fr)",
  );
  assert.equal(computedStyle(fields, styles, 600).display, "block");
  assert.equal(computedStyle(actions, styles, 600)["flex-wrap"], "wrap");
});

test("Feature surface contains no compound Goal mutation or browser-owned editable status list", () => {
  assert.doesNotMatch(source, /FEATURE_GOAL_EDITABLE_STATUSES/);
  assert.doesNotMatch(source, /applyFeatureGoalPlacement|saveFeatureGoalInline/);
  assert.doesNotMatch(source, /\/api\/goals\/.*rounds|\/order|\/unorder|\/reorder/);
  assert.doesNotMatch(featureSource, /openFeatureNewGoalFlow|data-feature-new-goal/);
  assert.match(featureSource, /data-feature-edit-goal/);
});
