"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const staticRoot = path.join(__dirname, "../src/surfaces/web/static/js");
const bulkSource = fs.readFileSync(
  path.join(staticRoot, "features/goals-bulk.js"),
  "utf8",
);
const commandsSource = fs.readFileSync(path.join(staticRoot, "commands.js"), "utf8");

function deferred() {
  let resolve;
  const promise = new Promise((next) => { resolve = next; });
  return { promise, resolve };
}

async function settle() {
  await new Promise((resolve) => setImmediate(resolve));
}

function bulkFeedbackRuntime({ commands = false } = {}) {
  const apiResponse = deferred();
  const requests = [];
  const resolverCalls = [];
  const toasts = [];
  const context = vm.createContext({
    SETTINGS_SURFACES: {},
    URL,
    URLSearchParams,
    api: async (method, requestPath, payload) => {
      requests.push({ method, path: requestPath, payload });
      return await apiResponse.promise;
    },
    describeGoalsFilter: () => "all goals",
    goalsExcludedIds: new Set(),
    goalsFilterFromHash: () => ({
      status: "", q: "", reporter: "", assignee: "", feature: "", node: "",
      rounds_gte: "", rounds_lte: "",
    }),
    goalsIncludedIds: new Set(),
    goalsSelectAllMatching: true,
    htmlEscape: String,
    modalConfirm: async () => true,
    refreshDashboard: async () => {},
    refreshGoalsListIfCurrent: async () => {},
    refreshReporters: async () => {},
    registerCommand: () => {},
    renderGoalsList: async () => {},
    resolveBackgroundOperationResponse: async (response, message = "") => {
      resolverCalls.push({ hasOperation: !!response?.operation, message });
      if (message) context.toast(message, "info");
      return response?.operation?.result || response;
    },
    showActionError: async (error) => { throw error; },
    state: { currentRoute: "other", reporters: [] },
    toast: (message, kind, options = {}) => toasts.push({ message, kind, options }),
    window: {},
    withButtonBusy: async (_button, _label, action) => await action(),
    _lastGoalsRender: null,
    _openModal: async () => "todo",
  });
  vm.runInContext(bulkSource, context);
  if (commands) vm.runInContext(commandsSource, context);
  vm.runInContext(`
    globalThis.bulkFeedbackTest = {
      panel: () => openBulkModal("status"),
      command: () => runBulkStatusCommand({ source: "backlog", dest: "todo" }),
    };
  `, context);
  return {
    apiResponse,
    requests,
    resolverCalls,
    run: commands ? context.bulkFeedbackTest.command : context.bulkFeedbackTest.panel,
    toasts,
  };
}

async function assertImmediateSingleAcknowledgement(browser) {
  const completion = browser.run();
  await settle();

  assert.equal(browser.requests.length, 1);
  assert.deepEqual(browser.toasts.map(({ message, kind }) => ({ message, kind })), [{
    message: "Refine is working on it asynchronously.",
    kind: "info",
  }]);

  browser.apiResponse.resolve({
    operation: { id: "bulk-1", result: { updated: 2 } },
  });
  await completion;

  assert.deepEqual(browser.resolverCalls, [{ hasOperation: true, message: "" }]);
  assert.equal(
    browser.toasts.filter(({ message }) => message === "Refine is working on it asynchronously.").length,
    1,
  );
  assert.deepEqual(browser.toasts.map(({ message, kind }) => ({ message, kind })), [
    { message: "Refine is working on it asynchronously.", kind: "info" },
    { message: "Updated 2 goals", kind: "info" },
  ]);
}

test("Goals bulk panel acknowledges asynchronous work before the API settles", async () => {
  await assertImmediateSingleAcknowledgement(bulkFeedbackRuntime());
});

test("Goals bulk command shares immediate acknowledgement without an operation duplicate", async () => {
  await assertImmediateSingleAcknowledgement(bulkFeedbackRuntime({ commands: true }));
});
