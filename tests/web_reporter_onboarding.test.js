"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const { BrowserEvent, createBrowserDom } = require("./support/browser_dom");

const staticRoot = path.join(__dirname, "../src/surfaces/web/static");
const source = fs.readFileSync(
  path.join(staticRoot, "js/features/reporter-onboarding.js"),
  "utf8",
);
const commonSource = fs.readFileSync(path.join(staticRoot, "js/common.js"), "utf8");
const initSource = fs.readFileSync(path.join(staticRoot, "js/init.js"), "utf8");

function htmlEscape(value) {
  return String(value ?? "").replace(/[&<>"']/g, (character) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  }[character]));
}

async function settle() {
  for (let index = 0; index < 6; index += 1) await Promise.resolve();
  await new Promise((resolve) => setImmediate(resolve));
}

function reporterRuntime({
  attached = true,
  lastReporter = "",
  reporters = [{ id: 1, name: "Buddy Williams" }, { id: 2, name: "Ethan" }],
  apiHandler = null,
  onSelected = null,
} = {}) {
  const dom = createBrowserDom("");
  const state = {
    project: {
      attached,
      target_root: "/tmp/app-a",
      active_node_id: "node-a",
    },
    reporters: [],
    lastReporter,
  };
  const storage = new Map(lastReporter ? [["refine_last_reporter", lastReporter]] : []);
  const requests = [];
  const order = [];
  const toasts = [];
  let serverReporters = reporters.map((reporter) => ({ ...reporter }));
  let handler = apiHandler || (async (method, requestPath, body) => {
    if (method === "POST" && requestPath === "/api/reporters") {
      const reporter = { id: 99, name: body.name.trim() };
      serverReporters.push(reporter);
      return { reporter };
    }
    if (method === "GET" && requestPath === "/api/reporters") {
      return { reporters: serverReporters.map((reporter) => ({ ...reporter })) };
    }
    throw new Error(`Unexpected request: ${method} ${requestPath}`);
  });
  let context;

  function setLastReporter(name) {
    order.push(`set:${name}`);
    state.lastReporter = name;
    if (name) storage.set("refine_last_reporter", name);
    else storage.delete("refine_last_reporter");
    onSelected?.(name, dom);
  }

  function populateAllReporterDropdowns() {
    order.push(`populate:retired=${!dom.document.querySelector('[data-testid="reporter-onboarding-dialog"]')}`);
  }

  function mergeReporterIntoProjection(reporter) {
    const index = state.reporters.findIndex((candidate) =>
      (reporter.id != null && candidate.id === reporter.id)
        || candidate.name === reporter.name);
    if (index >= 0) state.reporters[index] = { ...state.reporters[index], ...reporter };
    else state.reporters.push(reporter);
  }

  async function api(method, requestPath, body) {
    requests.push({ method, path: requestPath, body });
    order.push(`${method} ${requestPath}`);
    return handler(method, requestPath, body);
  }

  async function refreshReporters() {
    const data = await api("GET", "/api/reporters");
    state.reporters = data.reporters || [];
    if (state.lastReporter
        && !state.reporters.some((reporter) => reporter.name === state.lastReporter)) {
      setLastReporter("");
    }
    populateAllReporterDropdowns();
    context.notifyReporterOnboardingHydrated();
  }

  context = vm.createContext({
    $: (selector, root = dom.document) => root.querySelector(selector),
    $$: (selector, root = dom.document) => Array.from(root.querySelectorAll(selector)),
    api,
    console,
    document: dom.document,
    hasAttachedProject: () => state.project?.attached === true,
    htmlEscape,
    mergeReporterIntoProjection,
    MutationObserver: dom.MutationObserver,
    populateAllReporterDropdowns,
    refreshReporters,
    setLastReporter,
    state,
    toast(message, kind, options) { toasts.push({ message, kind, options }); },
  });
  vm.runInContext(source, context);
  vm.runInContext(`
    globalThis.reporterOnboardingTest = {
      notify: notifyReporterOnboardingHydrated,
      reevaluate: reevaluateReporterOnboarding,
      outcome: () => activeReporterOnboarding?.outcome || "none",
      scopeCount: () => reporterOnboardingScopes.size,
    };
  `, context);

  return {
    ...dom,
    context,
    order,
    requests,
    runtime: context.reporterOnboardingTest,
    state,
    storage,
    toasts,
    async hydrate(nextReporters = serverReporters) {
      serverReporters = nextReporters.map((reporter) => ({ ...reporter }));
      await refreshReporters();
    },
    async refresh() { await refreshReporters(); },
    setApiHandler(next) { handler = next; },
    setServerReporters(next) {
      serverReporters = next.map((reporter) => ({ ...reporter }));
    },
  };
}

function onboardingDialog(runtime) {
  return runtime.document.querySelector('[data-testid="reporter-onboarding-dialog"]');
}

function appendBlockingModal(runtime, testId = "blocking-modal") {
  const root = runtime.document.createElement("div");
  root.className = "modal-backdrop";
  root.innerHTML = `
    <div role="dialog" aria-modal="true" data-testid="${testId}">
      <button type="button">Blocking action</button>
    </div>`;
  runtime.document.body.appendChild(root);
  return root;
}

test("detached and valid browser-local Reporter selections suppress onboarding", async (t) => {
  await t.test("detached", async () => {
    const runtime = reporterRuntime({ attached: false });
    runtime.runtime.notify();
    assert.equal(onboardingDialog(runtime), null);
  });

  await t.test("valid selection", async () => {
    const runtime = reporterRuntime({ lastReporter: "Ethan" });
    await runtime.hydrate();
    assert.equal(runtime.state.lastReporter, "Ethan");
    assert.equal(onboardingDialog(runtime), null);
    assert.equal(runtime.runtime.outcome(), "completed");
  });
});

test("every existing modal defers onboarding and removal safely re-triggers it", async (t) => {
  for (const route of [
    "goal-detail",
    "goals-new",
    "goals-import",
    "feature-detail",
    "command-palette",
    "guide",
    "utility",
  ]) {
    await t.test(route, async () => {
      const runtime = reporterRuntime();
      const blocker = appendBlockingModal(runtime, route);
      const blockerButton = blocker.querySelector("button");
      blockerButton.focus();
      await runtime.hydrate();
      assert.equal(onboardingDialog(runtime), null);
      assert.equal(runtime.runtime.outcome(), "deferred");
      assert.equal(runtime.document.activeElement, blockerButton);

      blocker.remove();
      assert.ok(onboardingDialog(runtime));
      assert.equal(runtime.document.querySelectorAll('[aria-modal="true"]').length, 1);
    });
  }
});

test("an asynchronously opened route dialog makes onboarding yield without stealing focus", async () => {
  const runtime = reporterRuntime();
  await runtime.hydrate();
  assert.ok(onboardingDialog(runtime));

  const blocker = appendBlockingModal(runtime, "async-feature-detail");
  const blockerButton = blocker.querySelector("button");
  blockerButton.focus();
  assert.equal(onboardingDialog(runtime), null);
  assert.equal(runtime.document.activeElement, blockerButton);
  assert.equal(runtime.document.querySelectorAll('[aria-modal="true"]').length, 1);

  blocker.remove();
  assert.ok(onboardingDialog(runtime));
  assert.equal(runtime.document.querySelectorAll('[aria-modal="true"]').length, 1);
});

test("dialog labeling, deterministic focus, containment, Escape, and Enter isolation are accessible", async () => {
  const runtime = reporterRuntime();
  const underlay = runtime.document.createElement("button");
  underlay.textContent = "Underlay";
  runtime.document.body.appendChild(underlay);
  underlay.focus();
  await runtime.hydrate();

  const dialog = onboardingDialog(runtime);
  assert.equal(dialog.getAttribute("role"), "dialog");
  assert.equal(dialog.getAttribute("aria-modal"), "true");
  assert.equal(dialog.getAttribute("aria-labelledby"), "reporter-onboarding-title");
  assert.equal(
    dialog.getAttribute("aria-describedby"),
    "reporter-onboarding-description reporter-onboarding-guidance",
  );
  const choices = runtime.document.querySelectorAll("[data-reporter-onboarding-choice]");
  assert.equal(runtime.document.activeElement, choices[0], "first existing Reporter gets focus");

  const controls = dialog.querySelectorAll("button, input");
  controls[controls.length - 1].focus();
  const tab = new BrowserEvent("keydown", { key: "Tab" });
  runtime.document.dispatchEvent(tab);
  assert.equal(tab.defaultPrevented, true);
  assert.equal(runtime.document.activeElement, controls[0]);

  controls[0].focus();
  runtime.document.dispatchEvent(new BrowserEvent("keydown", { key: "Tab", shiftKey: true }));
  assert.equal(runtime.document.activeElement, controls[controls.length - 1]);

  const enter = new BrowserEvent("keydown", { key: "Enter" });
  runtime.document.dispatchEvent(enter);
  assert.equal(enter.defaultPrevented, false, "onboarding adds no document-level Enter action");
  assert.equal(runtime.state.lastReporter, "");

  let secondEscapeConsumer = 0;
  runtime.document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") secondEscapeConsumer += 1;
  });
  const escape = new BrowserEvent("keydown", { key: "Escape" });
  runtime.document.dispatchEvent(escape);
  assert.equal(secondEscapeConsumer, 0, "only the top dialog consumes Escape");
  assert.equal(onboardingDialog(runtime), null);
  assert.equal(runtime.document.activeElement, underlay, "ordinary dismissal restores focus");
});

test("dismissal is page-lifetime scoped while Controls remains the explicit selection path", async () => {
  const runtime = reporterRuntime();
  await runtime.hydrate();
  runtime.document.querySelector('[data-testid="reporter-onboarding-dismiss"]').click();
  assert.equal(runtime.runtime.outcome(), "dismissed");
  assert.equal(onboardingDialog(runtime), null);

  await runtime.hydrate();
  await runtime.hydrate();
  assert.equal(onboardingDialog(runtime), null, "later hydrations do not reopen after dismissal");
  assert.match(commonSource, /\+ Add new reporter…/);
  assert.match(commonSource, /if \(e\.target\.value === "__add__"\)/);
});

test("initial hydration failure can recover through each later event path exactly once", async (t) => {
  for (const eventName of ["reporters_changed", "project_updated"]) {
    await t.test(eventName, async () => {
      let reads = 0;
      const runtime = reporterRuntime({
        apiHandler: async (method, requestPath) => {
          if (method === "GET" && requestPath === "/api/reporters") {
            reads += 1;
            if (reads === 1) throw new Error("transient hydration failure");
            return { reporters: [{ id: 1, name: "Buddy Williams" }] };
          }
          throw new Error("unexpected request");
        },
      });
      await assert.rejects(runtime.refresh(), /transient hydration failure/);
      assert.equal(onboardingDialog(runtime), null);

      await Promise.all([runtime.refresh(), runtime.refresh()]);
      assert.equal(runtime.document.querySelectorAll('[data-testid="reporter-onboarding-dialog"]').length, 1);
      assert.equal(runtime.runtime.scopeCount(), 1);
    });
  }
  assert.match(commonSource, /addEventListener\("reporters_changed"[\s\S]*?await refreshReporters\(\)/);
  assert.match(commonSource, /addEventListener\("project_updated"[\s\S]*?if \(hasAttachedProject\(\)\) \{\s*await refreshReporters\(\)/);
  assert.match(initSource, /await refreshReporters\(\)/);
});

test("project attach and scope switch never persist the first Reporter as fallback", async () => {
  assert.doesNotMatch(commonSource, /selectReporterFallback|selectFallback/);
  const nodeRefreshSource = commonSource.match(
    /async function refreshNodeScopedState\(\) \{[\s\S]*?\n\}/,
  )?.[0] || "";
  assert.match(nodeRefreshSource, /await refreshReporters\(\)/);
  assert.doesNotMatch(nodeRefreshSource, /setLastReporter\(""\)/);
  assert.match(commonSource, /function reconcileLastReporter\(\)[\s\S]*?setLastReporter\(""\)/);

  const runtime = reporterRuntime({ lastReporter: "Someone from another app" });
  await runtime.hydrate();
  assert.equal(runtime.state.lastReporter, "");
  assert.equal(runtime.storage.has("refine_last_reporter"), false);
  assert.ok(onboardingDialog(runtime));

  runtime.document.querySelector('[data-testid="reporter-onboarding-dismiss"]').click();
  runtime.state.project = {
    attached: true,
    target_root: "/tmp/app-b",
    active_node_id: "node-b",
  };
  await runtime.hydrate([{ id: 3, name: "First" }, { id: 4, name: "Second" }]);
  assert.equal(runtime.state.lastReporter, "");
  assert.ok(onboardingDialog(runtime), "a new project/node scope gets its own orientation");
});

test("existing selection retires onboarding before setLastReporter can navigate", async () => {
  let routeButton = null;
  const runtime = reporterRuntime({
    onSelected(name, dom) {
      if (!name) return;
      const route = dom.document.createElement("div");
      route.className = "modal-backdrop";
      route.innerHTML = '<div role="dialog" aria-modal="true"><button>Route dialog</button></div>';
      dom.document.body.appendChild(route);
      routeButton = route.querySelector("button");
      routeButton.focus();
    },
  });
  await runtime.hydrate();
  runtime.document.querySelector('[data-reporter-onboarding-choice="Ethan"]').click();

  assert.equal(runtime.state.lastReporter, "Ethan");
  assert.equal(runtime.storage.get("refine_last_reporter"), "Ethan");
  assert.equal(onboardingDialog(runtime), null);
  assert.equal(runtime.document.querySelectorAll('[aria-modal="true"]').length, 1);
  assert.equal(runtime.document.activeElement, routeButton);
});

test("create failure preserves draft and supports one request per retry", async () => {
  let posts = 0;
  const runtime = reporterRuntime({
    apiHandler: async (method, requestPath, body) => {
      if (method === "GET") {
        return {
          reporters: [
            { id: 1, name: "Buddy Williams" },
            ...(posts > 1 ? [{ id: 5, name: "Ada Lovelace" }] : []),
          ],
        };
      }
      posts += 1;
      if (posts === 1) throw new Error("service unavailable");
      return { reporter: { id: 5, name: body.name.trim() } };
    },
  });
  await runtime.hydrate();
  const input = runtime.document.querySelector("#reporter-onboarding-name");
  input.value = "Ada Lovelace";
  runtime.document.querySelector("#reporter-onboarding-form").requestSubmit();
  await settle();

  assert.equal(posts, 1);
  assert.equal(input.value, "Ada Lovelace");
  assert.equal(runtime.state.lastReporter, "");
  const error = runtime.document.querySelector("#reporter-onboarding-error");
  assert.equal(error.hidden, false);
  assert.match(error.textContent, /service unavailable/);
  assert.equal(runtime.document.activeElement, input);
  assert.equal(input.disabled, false);
  assert.equal(runtime.document.querySelector('[data-testid="reporter-onboarding-create"]').disabled, false);

  const form = runtime.document.querySelector("#reporter-onboarding-form");
  form.requestSubmit();
  form.requestSubmit();
  await settle();
  assert.equal(posts, 2, "the submitting state ignores a duplicate retry submission");
  assert.equal(runtime.state.lastReporter, "Ada Lovelace");
});

test("POST success selects canonical Reporter before GET failure and later reconciliation", async () => {
  let reads = 0;
  const runtime = reporterRuntime({
    apiHandler: async (method, requestPath, body) => {
      if (method === "POST") return { reporter: { id: 42, name: body.name.trim() } };
      reads += 1;
      if (reads === 1) return { reporters: [{ id: 1, name: "Buddy Williams" }] };
      if (reads === 2) throw new Error("list unavailable");
      return {
        reporters: [
          { id: 1, name: "Buddy Williams" },
          { id: 42, name: "Grace Hopper" },
        ],
      };
    },
  });
  await runtime.refresh();
  const input = runtime.document.querySelector("#reporter-onboarding-name");
  input.value = "Grace Hopper";
  runtime.document.querySelector("#reporter-onboarding-form").requestSubmit();
  await settle();

  assert.equal(runtime.state.lastReporter, "Grace Hopper");
  assert.equal(runtime.storage.get("refine_last_reporter"), "Grace Hopper");
  assert.equal(onboardingDialog(runtime), null);
  assert.ok(runtime.state.reporters.some((reporter) => reporter.id === 42));
  assert.equal(runtime.toasts.length, 1);
  assert.equal(runtime.toasts[0].kind, "warn");
  assert.match(runtime.toasts[0].message, /created and selected.*could not be refreshed/i);
  assert.deepEqual(runtime.order.slice(runtime.order.indexOf("POST /api/reporters")), [
    "POST /api/reporters",
    "populate:retired=true",
    "set:Grace Hopper",
    "GET /api/reporters",
  ]);

  await runtime.refresh();
  assert.equal(runtime.state.lastReporter, "Grace Hopper");
  assert.equal(runtime.state.reporters.find((reporter) => reporter.id === 42).name, "Grace Hopper");
  assert.equal(onboardingDialog(runtime), null);
});
