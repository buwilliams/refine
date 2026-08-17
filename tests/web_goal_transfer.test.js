"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const source = fs.readFileSync(
  path.join(__dirname, "../src/surfaces/web/static/js/features/goals-detail.js"),
  "utf8",
);

function transferRuntime({ activeNodeId = "local", apiHandler } = {}) {
  const requests = [];
  const refreshed = [];
  const errors = [];
  const toasts = [];
  let handler = apiHandler || (async () => ({ updated: 1 }));
  const context = vm.createContext({
    __refreshGoalDetail: async (goalId) => refreshed.push(goalId),
    api: async (method, requestPath, body) => {
      requests.push({ method, path: requestPath, body });
      return handler(method, requestPath, body);
    },
    loadGoalDetail: async (goalId) => refreshed.push(goalId),
    nodeContextActiveNodeId: () => activeNodeId,
    showActionError: async (error, title) => errors.push({ error, title }),
    toast: (message, kind) => toasts.push({ message, kind }),
  });
  vm.runInContext(source, context);
  vm.runInContext(`
    loadGoalDetail = globalThis.__refreshGoalDetail;
    globalThis.goalTransferTest = {
      render: renderGoalTransferToActiveNodeAction,
      run: transferGoalToActiveNode,
      target: goalTransferToActiveNodeTarget,
    };
  `, context);
  return {
    errors,
    refreshed,
    requests,
    runtime: context.goalTransferTest,
    setApiHandler(next) { handler = next; },
    toasts,
  };
}

test("Goal modal offers transfer only for a Goal owned by another node", () => {
  const { runtime } = transferRuntime({ activeNodeId: "LOCAL" });

  assert.equal(runtime.target({ node_id: "remote" }), "LOCAL");
  assert.match(runtime.render({ node_id: "remote" }), /Transfer to my node/);
  assert.match(runtime.render({ node_id: "remote" }), /goal-action-transfer-to-my-node/);
  assert.equal(runtime.target({ node_id: "local" }), "");
  assert.equal(runtime.render({ node_id: "local" }), "");
  assert.match(runtime.render({}), /Transfer to my node/);
  assert.equal(transferRuntime({ activeNodeId: "default" }).runtime.render({}), "");
  assert.equal(transferRuntime({ activeNodeId: "" }).runtime.render({ node_id: "remote" }), "");
});

test("Goal transfer uses the single-item server contract and refreshes after success", async () => {
  const browser = transferRuntime({ activeNodeId: "local" });

  await browser.runtime.run({ id: "GOAL1", node_id: "remote" });

  assert.deepEqual(JSON.parse(JSON.stringify(browser.requests)), [{
    method: "POST",
    path: "/api/nodes/transfer-goals",
    body: { item_id: "GOAL1", target_node_id: "local" },
  }]);
  assert.deepEqual(browser.toasts, [{ message: "Transferred to my node", kind: "info" }]);
  assert.deepEqual(browser.refreshed, ["GOAL1"]);
  assert.deepEqual(browser.errors, []);
});

test("Goal transfer surfaces server-owned eligibility failures without refreshing", async () => {
  const rejection = new Error("Goal GOAL1 is assigned to Feature FEA1; transfer the Feature instead");
  const browser = transferRuntime({
    apiHandler: async () => { throw rejection; },
  });

  await browser.runtime.run({ id: "GOAL1", node_id: "remote", feature_id: "FEA1" });

  assert.equal(browser.requests.length, 1);
  assert.deepEqual(browser.refreshed, []);
  assert.deepEqual(browser.toasts, []);
  assert.equal(browser.errors.length, 1);
  assert.equal(browser.errors[0].error, rejection);
  assert.equal(browser.errors[0].title, "Transfer failed");
});
