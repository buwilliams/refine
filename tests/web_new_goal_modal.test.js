"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const { BrowserEvent, createBrowserDom } = require("./support/browser_dom");

const staticRoot = path.join(__dirname, "../src/surfaces/web/static");
const source = fs.readFileSync(
  path.join(staticRoot, "js/features/goals-new.js"),
  "utf8",
);
const commonSource = fs.readFileSync(
  path.join(staticRoot, "js/common.js"),
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

function deferred() {
  let resolve;
  const promise = new Promise((next) => { resolve = next; });
  return { promise, resolve };
}

function eventTarget() {
  const listeners = new Map();
  return {
    addEventListener(type, listener) {
      if (!listeners.has(type)) listeners.set(type, []);
      listeners.get(type).push(listener);
    },
    removeEventListener(type, listener) {
      listeners.set(
        type,
        (listeners.get(type) || []).filter((candidate) => candidate !== listener),
      );
    },
    dispatchEvent(event) {
      for (const listener of [...(listeners.get(event.type) || [])]) listener(event);
      return !event.defaultPrevented;
    },
    listenerCount(type) { return (listeners.get(type) || []).length; },
  };
}

function duplicateError() {
  const error = new Error("Possible duplicate Goal");
  error.code = "duplicate_goal";
  error.error = {
    duplicate: {
      match: {
        id: "GOAL1",
        name: "Existing Goal",
        node_display_name: "Default",
        prompt: "An existing prompt",
        status: "todo",
      },
    },
  };
  return error;
}

function newGoalRuntime({ apiHandler, hash = "#/goals/new" } = {}) {
  const dom = createBrowserDom("");
  const window = eventTarget();
  const location = { hash };
  const confirmations = [];
  const requests = [];
  const toasts = [];
  let handler = apiHandler || (async () => ({ created: true, goal: { id: "GOAL2" } }));
  const nodeContext = { generation: 0 };

  class TestFormData {
    constructor(form) { this.form = form; }
    get(name) { return this.form.elements[name]?.value ?? null; }
  }

  const context = vm.createContext({
    $$: (selector, root) => root.querySelectorAll(selector),
    captureNodeContextGeneration: () => nodeContext.generation,
    isNodeContextGenerationCurrent: (generation) => generation === nodeContext.generation,
    FormData: TestFormData,
    api: async (method, requestPath, body) => {
      requests.push({ method, path: requestPath, body });
      return handler(method, requestPath, body);
    },
    document: dom.document,
    history: {
      replaceState(_state, _title, nextHash) { location.hash = nextHash; },
    },
    htmlEscape,
    location,
    modalConfirm: (message, options) => {
      const pending = deferred();
      confirmations.push({ message, options, ...pending });
      dom.document.activeElement = null;
      return pending.promise;
    },
    renderGoalsList: async () => {},
    state: { lastReporter: "Ethan" },
    toast: (message, kind) => toasts.push({ message, kind }),
    window,
  });
  vm.runInContext(source, context);
  vm.runInContext(`
    globalThis.newGoalTest = {
      guardNavigation: guardNewGoalNavigation,
      isOpen: () => _newGoalModalOpen,
      open: openNewGoalModal,
    };
  `, context);

  return {
    ...dom,
    advanceNodeGeneration() { nodeContext.generation += 1; },
    confirmations,
    context,
    location,
    requests,
    runtime: context.newGoalTest,
    setApiHandler(next) { handler = next; },
    toasts,
    window,
  };
}

function fields(browser) {
  return {
    cancel: browser.document.querySelector("[data-cancel]"),
    modal: browser.document.querySelector("[data-testid='new-goal-modal']"),
    priority: browser.document.querySelector("[name='priority']"),
    prompt: browser.document.querySelector("[name='prompt']"),
    submit: browser.document.querySelector("[data-ok]"),
    backdrop: browser.document.querySelector(".modal-backdrop"),
  };
}

function openBrowser(options) {
  const browser = newGoalRuntime(options);
  browser.runtime.open();
  return browser;
}

async function settle() {
  for (let index = 0; index < 4; index += 1) await Promise.resolve();
  await new Promise((resolve) => setImmediate(resolve));
}

function dirty(browser, prompt = "Keep this carefully written Goal") {
  const current = fields(browser);
  current.prompt.value = prompt;
  current.priority.value = "high";
  return current;
}

test("new goal modal provides a large responsive prompt editor and accessible discard alert", () => {
  assert.match(source, /class="modal new-goal-modal"/);
  assert.doesNotMatch(source, /data-testid="new-goal-modal"[^>]*style=/);
  assert.match(
    styles,
    /\.new-goal-modal\s*\{[^}]*width:\s*60vw;[^}]*max-width:\s*60vw;/s,
  );
  assert.match(
    styles,
    /\.new-goal-modal textarea\[name="prompt"\]\s*\{[^}]*height:\s*clamp\(180px, 32vh, 320px\);/s,
  );
  assert.match(
    styles,
    /@media \(max-width: 760px\)[\s\S]*?\.new-goal-modal\s*\{[^}]*width:\s*calc\(100% - 24px\);[^}]*max-width:\s*calc\(100% - 24px\);/,
  );
  assert.match(commonSource, /\{ role: "alertdialog", ariaLabel: title \|\| message \}/);
  assert.match(source, /Your unsaved Goal text will be discarded\./);
  assert.match(source, /focusCancel: true/);
});

test("pristine Escape, backdrop, and Cancel close directly", async (t) => {
  const cases = {
    Escape(browser) {
      browser.document.dispatchEvent(new BrowserEvent("keydown", { key: "Escape" }));
    },
    backdrop(browser) { fields(browser).backdrop.click(); },
    Cancel(browser) { fields(browser).cancel.click(); },
  };

  for (const [name, dismiss] of Object.entries(cases)) {
    await t.test(name, async () => {
      const browser = openBrowser();
      dismiss(browser);
      await settle();
      assert.equal(browser.confirmations.length, 0);
      assert.equal(browser.runtime.isOpen(), false);
      assert.equal(fields(browser).modal, null);
      assert.equal(browser.location.hash, "#/goals");
    });
  }
});

test("dirty Escape, backdrop, and Cancel share one discard decision", async (t) => {
  const cases = {
    Escape(browser) {
      const current = fields(browser);
      current.prompt.focus();
      browser.document.dispatchEvent(new BrowserEvent("keydown", { key: "Escape" }));
      return current.prompt;
    },
    backdrop(browser) {
      const current = fields(browser);
      current.prompt.focus();
      current.backdrop.click();
      return current.prompt;
    },
    Cancel(browser) {
      const current = fields(browser);
      current.cancel.focus();
      current.cancel.click();
      return current.cancel;
    },
  };

  for (const [name, dismiss] of Object.entries(cases)) {
    await t.test(name, async () => {
      const browser = openBrowser();
      const original = dirty(browser);
      const expectedFocus = dismiss(browser);
      dismiss(browser);
      assert.equal(browser.confirmations.length, 1, "dismissals must not stack confirmations");
      assert.deepEqual(JSON.parse(JSON.stringify(browser.confirmations[0].options)), {
        title: "Discard unsaved Goal?",
        okLabel: "Discard Goal",
        cancelLabel: "Keep editing",
        danger: true,
        focusCancel: true,
      });
      browser.confirmations[0].resolve(false);
      await settle();
      assert.equal(browser.runtime.isOpen(), true);
      assert.equal(fields(browser).prompt.value, original.prompt.value);
      assert.equal(fields(browser).priority.value, "high");
      assert.equal(browser.document.activeElement, expectedFocus);

      dismiss(browser);
      assert.equal(browser.confirmations.length, 2);
      browser.confirmations[1].resolve(true);
      await settle();
      assert.equal(browser.runtime.isOpen(), false);
      assert.equal(browser.location.hash, "#/goals");
    });
  }
});

test("priority alone is dirty while whitespace-only prompt remains pristine", async () => {
  const priorityBrowser = openBrowser();
  fields(priorityBrowser).priority.value = "medium";
  fields(priorityBrowser).cancel.click();
  assert.equal(priorityBrowser.confirmations.length, 1);
  priorityBrowser.confirmations[0].resolve(false);
  await settle();
  fields(priorityBrowser).priority.value = "low";
  fields(priorityBrowser).cancel.click();
  await settle();
  assert.equal(priorityBrowser.confirmations.length, 1);
  assert.equal(priorityBrowser.runtime.isOpen(), false);

  const whitespaceBrowser = openBrowser();
  fields(whitespaceBrowser).prompt.value = "  \n  ";
  fields(whitespaceBrowser).cancel.click();
  await settle();
  assert.equal(whitespaceBrowser.confirmations.length, 0);
  assert.equal(whitespaceBrowser.runtime.isOpen(), false);
});

test("navigation waits for dirty confirmation and restores the original route on refusal", async () => {
  const browser = openBrowser({ hash: "#/goals/new" });
  dirty(browser, "Draft survives navigation");
  let continuations = 0;
  browser.location.hash = "#/features";
  assert.equal(browser.runtime.guardNavigation({
    destinationHash: "#/features",
    continueNavigation: () => { continuations += 1; },
  }), true);
  assert.equal(browser.confirmations.length, 1);
  browser.confirmations[0].resolve(false);
  await settle();
  assert.equal(browser.location.hash, "#/goals/new");
  assert.equal(continuations, 0);
  assert.equal(fields(browser).prompt.value, "Draft survives navigation");
  assert.equal(browser.document.activeElement, fields(browser).prompt);

  browser.location.hash = "#/features";
  browser.runtime.guardNavigation({
    destinationHash: "#/features",
    continueNavigation: () => { continuations += 1; },
  });
  browser.confirmations[1].resolve(true);
  await settle();
  assert.equal(continuations, 1);
  assert.equal(browser.runtime.isOpen(), false);
  assert.equal(browser.location.hash, "#/features");
});

test("pristine navigation closes without confirmation and continues", async () => {
  const browser = openBrowser({ hash: "#/goals/new" });
  browser.location.hash = "#/dashboard";
  let continued = false;
  assert.equal(browser.runtime.guardNavigation({
    destinationHash: "#/dashboard",
    continueNavigation: () => { continued = true; },
  }), true);
  await settle();
  assert.equal(browser.confirmations.length, 0);
  assert.equal(browser.runtime.isOpen(), false);
  assert.equal(continued, true);
});

test("successful submit closes directly and reopening starts pristine", async () => {
  const browser = openBrowser();
  dirty(browser, "Create this Goal");
  fields(browser).submit.click();
  await settle();
  assert.equal(browser.requests.length, 1);
  assert.deepEqual(JSON.parse(JSON.stringify(browser.requests[0].body)), {
    reporter: "Ethan",
    prompt: "Create this Goal",
    priority: "high",
    duplicate_decision: "",
  });
  assert.equal(browser.confirmations.length, 0);
  assert.equal(browser.runtime.isOpen(), false);

  browser.location.hash = "#/goals/new";
  browser.runtime.open();
  assert.equal(fields(browser).prompt.value, "");
  assert.equal(fields(browser).priority.value, "low");
  assert.equal(browser.document.querySelector("[data-testid='new-goal-duplicate']"), null);
  fields(browser).cancel.click();
  await settle();
  assert.equal(browser.confirmations.length, 0);
});

test("durable creation closes even when an optional saved callback fails", async () => {
  const browser = newGoalRuntime();
  browser.runtime.open({
    onSaved: async () => { throw new Error("Refresh failed"); },
  });
  fields(browser).prompt.value = "Creation already succeeded";
  fields(browser).submit.click();
  await settle();
  assert.equal(browser.requests.length, 1);
  assert.equal(browser.runtime.isOpen(), false);
  assert.equal(browser.confirmations.length, 0);
  assert.deepEqual(browser.toasts.at(-1), {
    message: "Goal created, but refresh failed: Refresh failed",
    kind: "error",
  });
});

test("failed submit retains the complete draft and remains protected", async () => {
  const browser = openBrowser({
    apiHandler: async () => { throw new Error("Service unavailable"); },
  });
  dirty(browser, "Do not lose this failed submission");
  fields(browser).submit.click();
  await settle();
  assert.equal(browser.runtime.isOpen(), true);
  assert.equal(fields(browser).prompt.value, "Do not lose this failed submission");
  assert.equal(fields(browser).priority.value, "high");
  assert.deepEqual(browser.toasts.at(-1), { message: "Service unavailable", kind: "error" });
  fields(browser).cancel.click();
  assert.equal(browser.confirmations.length, 1);
});

test("a late New Goal response cannot close the preserved previous-Node draft", async () => {
  const pendingCreate = deferred();
  const browser = openBrowser({ apiHandler: () => pendingCreate.promise });
  dirty(browser, "Goal submitted from the previous Node");
  fields(browser).submit.click();
  assert.equal(browser.requests.length, 1);

  browser.advanceNodeGeneration();
  pendingCreate.resolve({ created: true, goal: { id: "OLD-NODE-GOAL" } });
  await settle();

  assert.equal(browser.runtime.isOpen(), true);
  assert.equal(fields(browser).prompt.value, "Goal submitted from the previous Node");
  assert.equal(fields(browser).priority.value, "high");
  assert.deepEqual(browser.toasts, []);
});

test("duplicate handling and a declined dismissal retain the draft and decision", async () => {
  let attempts = 0;
  const browser = openBrowser({
    apiHandler: async () => {
      attempts += 1;
      if (attempts === 1) throw duplicateError();
      return { created: true, goal: { id: "GOAL2" } };
    },
  });
  dirty(browser, "Potential duplicate draft");
  fields(browser).submit.click();
  await settle();
  assert.equal(browser.runtime.isOpen(), true);
  assert.ok(browser.document.querySelector("[data-testid='new-goal-duplicate']"));
  browser.document.querySelector("[data-testid='new-goal-duplicate-import']").click();
  fields(browser).cancel.focus();
  fields(browser).cancel.click();
  browser.confirmations[0].resolve(false);
  await settle();
  assert.equal(fields(browser).prompt.value, "Potential duplicate draft");
  assert.equal(fields(browser).priority.value, "high");
  assert.ok(browser.document.querySelector("[data-testid='new-goal-duplicate']"));
  assert.equal(browser.document.activeElement, fields(browser).cancel);

  fields(browser).submit.click();
  await settle();
  assert.equal(browser.requests.length, 2);
  assert.equal(browser.requests[1].body.duplicate_decision, "original");
  assert.equal(browser.confirmations.length, 1);
  assert.equal(browser.runtime.isOpen(), false);
});

test("actual page unload is protected only while the form is dirty", () => {
  const browser = openBrowser();
  const pristine = new BrowserEvent("beforeunload");
  browser.window.dispatchEvent(pristine);
  assert.equal(pristine.defaultPrevented, false);

  fields(browser).prompt.value = "Unsaved page draft";
  const dirtyUnload = new BrowserEvent("beforeunload");
  browser.window.dispatchEvent(dirtyUnload);
  assert.equal(dirtyUnload.defaultPrevented, true);
  assert.equal(dirtyUnload.returnValue, "");
  assert.equal(browser.window.listenerCount("beforeunload"), 1);
});
