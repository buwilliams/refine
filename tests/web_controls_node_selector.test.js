"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const source = fs.readFileSync(
  path.join(__dirname, "../src/surfaces/web/static/js/node-context.js"),
  "utf8",
);

function nodeContextRuntime(backend) {
  const calls = [];
  const toasts = [];
  const selector = {
    id: "global-node",
    children: [],
    value: "",
    disabled: false,
    appendChild(child) { this.children.push(child); },
    set innerHTML(_value) { this.children = []; },
  };
  const document = {
    getElementById(id) { return id === "global-node" ? selector : null; },
    createElement() { return { value: "", textContent: "", disabled: false, dataset: {} }; },
    querySelector(query) { return backend.querySelector?.(query) || null; },
    addEventListener() {},
  };
  const state = {
    project: null,
    currentRoute: null,
    currentGoal: null,
    screenDataCache: new Map(),
  };
  const context = vm.createContext({
    console,
    document,
    _featureModalRoot: backend.featureRoot || null,
    state,
    setTimeout,
    clearTimeout,
    modalConfirm: async (...args) => backend.confirm ? backend.confirm(...args) : true,
    invalidateScreenDataCache: () => state.screenDataCache.clear(),
    refreshNodeScopedState: async () => {},
    updateActiveNodeLabel: () => {},
    navigate: () => {},
    toast: (message, kind) => toasts.push({ message, kind }),
    api: async (method, requestPath, body, options) => {
      calls.push({ method, path: requestPath, body, options });
      if (backend.handle) return backend.handle(method, requestPath, body, options);
      if (method === "POST" && requestPath === "/api/nodes/activate") {
        backend.activeId = body.node_id;
        return { active_node_id: backend.activeId };
      }
      if (requestPath === "/api/project/status") {
        return {
          attached: backend.attached !== false,
          target_root: backend.targetRoot || "/tmp/app-a",
          active_node_id: backend.attached === false ? "" : backend.activeId,
          active_node: backend.nodes.find((node) => node.id === backend.activeId)?.display_name || "",
        };
      }
      if (requestPath === "/api/nodes") {
        return { nodes: backend.nodes.map((node) => ({ ...node })), active_node_id: backend.activeId };
      }
      throw new Error(`Unexpected request ${method} ${requestPath}`);
    },
  });
  vm.runInContext(source, context);
  vm.runInContext(`
    globalThis.nodeContextTest = {
      reconcileNodeContext,
      activateNodeContext,
      handleNodeContextMutationEvent,
      captureNodeContextGeneration,
      isNodeContextGenerationCurrent,
    };
  `, context);
  return { api: context.nodeContextTest, calls, selector, state, toasts };
}

function optionSnapshot(selector) {
  return selector.children.map((option) => ({
    value: option.value,
    text: option.textContent,
    disabled: option.disabled,
  }));
}

test("Controls Node selector maps authoritative IDs to display names and excludes archived Nodes", async () => {
  const backend = {
    activeId: "node-b",
    nodes: [
      { id: "node-a", display_name: "Alpha" },
      { id: "node-b", display_name: "Beta" },
      { id: "node-old", display_name: "Old", archived: true },
    ],
  };
  const runtime = nodeContextRuntime(backend);

  await runtime.api.reconcileNodeContext();

  assert.deepEqual(optionSnapshot(runtime.selector), [
    { value: "node-a", text: "Alpha", disabled: false },
    { value: "node-b", text: "Beta", disabled: false },
  ]);
  assert.equal(runtime.selector.value, "node-b");
  assert.equal(runtime.state.project.active_node, "Beta");
  const reads = runtime.calls.filter((call) => call.method === "GET");
  assert.deepEqual(reads.map((call) => call.path), ["/api/project/status", "/api/nodes"]);
  assert.ok(reads.every((call) => call.options.cache === false));
});

test("Controls Node selector is disabled and non-actionable while detached", async () => {
  const runtime = nodeContextRuntime({ attached: false, activeId: "", nodes: [] });
  await runtime.api.reconcileNodeContext();
  assert.equal(runtime.selector.disabled, true);
  assert.equal(runtime.selector.value, "");
  assert.deepEqual(optionSnapshot(runtime.selector), [
    { value: "", text: "No node", disabled: false },
  ]);
  assert.equal(await runtime.api.activateNodeContext(""), false);
  assert.equal(runtime.calls.filter((call) => call.method === "POST").length, 0);
});

test("attach and detach reconciliation advances context without inventing a Node selection", async () => {
  const backend = {
    activeId: "node-a",
    nodes: [{ id: "node-a", display_name: "Alpha" }],
  };
  const runtime = nodeContextRuntime(backend);
  await runtime.api.reconcileNodeContext();
  const attachedGeneration = runtime.api.captureNodeContextGeneration();
  backend.attached = false;
  await runtime.api.reconcileNodeContext({ external: true });
  assert.equal(runtime.selector.value, "");
  assert.equal(runtime.selector.disabled, true);
  assert.equal(runtime.api.isNodeContextGenerationCurrent(attachedGeneration), false);
  backend.attached = true;
  await runtime.api.reconcileNodeContext({ external: true });
  assert.equal(runtime.selector.value, "node-a");
  assert.equal(runtime.selector.disabled, false);
});

test("switching apps advances context even when both apps use the same Node ID", async () => {
  const backend = {
    activeId: "node-a",
    targetRoot: "/tmp/app-a",
    nodes: [{ id: "node-a", display_name: "Alpha" }],
  };
  const runtime = nodeContextRuntime(backend);
  await runtime.api.reconcileNodeContext();
  const appAGeneration = runtime.api.captureNodeContextGeneration();
  backend.targetRoot = "/tmp/app-b";
  await runtime.api.reconcileNodeContext({ external: true });
  assert.equal(runtime.api.isNodeContextGenerationCurrent(appAGeneration), false);
  assert.equal(runtime.state.project.target_root, "/tmp/app-b");
  assert.equal(runtime.selector.value, "node-a");
});

test("activation sends the exact Node ID, coalesces concurrent selection, and rereads authority", async () => {
  let releaseActivation;
  const backend = {
    activeId: "node-a",
    nodes: [
      { id: "node-a", display_name: "Alpha" },
      { id: "node-b", display_name: "Beta" },
    ],
  };
  backend.handle = async (method, requestPath, body) => {
    if (method === "POST") {
      await new Promise((resolve) => { releaseActivation = resolve; });
      backend.activeId = body.node_id;
      return { active_node_id: backend.activeId };
    }
    if (requestPath === "/api/project/status") {
      return { attached: true, active_node_id: backend.activeId, active_node: backend.activeId === "node-b" ? "Beta" : "Alpha" };
    }
    return { nodes: backend.nodes, active_node_id: backend.activeId };
  };
  const runtime = nodeContextRuntime(backend);
  await runtime.api.reconcileNodeContext();
  const first = runtime.api.activateNodeContext("node-b");
  const second = runtime.api.activateNodeContext("node-b");
  while (!releaseActivation) await new Promise((resolve) => setImmediate(resolve));
  releaseActivation();
  assert.equal(await first, true);
  assert.equal(await second, true);

  const posts = runtime.calls.filter((call) => call.method === "POST");
  assert.equal(posts.length, 1);
  assert.equal(JSON.stringify(posts[0].body), JSON.stringify({ node_id: "node-b" }));
  assert.equal(runtime.selector.value, "node-b");
  assert.ok(runtime.calls.slice(runtime.calls.indexOf(posts[0]) + 1)
    .some((call) => call.path === "/api/project/status" && call.options.cache === false));
});

test("activation failure rolls the selector back to authoritative identity", async () => {
  const backend = {
    activeId: "node-a",
    nodes: [
      { id: "node-a", display_name: "Alpha" },
      { id: "node-b", display_name: "Beta" },
    ],
    async handle(method, requestPath) {
      if (method === "POST") throw new Error("activation refused");
      if (requestPath === "/api/project/status") {
        return { attached: true, active_node_id: "node-a", active_node: "Alpha" };
      }
      return { nodes: this.nodes, active_node_id: "node-a" };
    },
  };
  const runtime = nodeContextRuntime(backend);
  await runtime.api.reconcileNodeContext();
  assert.equal(await runtime.api.activateNodeContext("node-b"), false);
  assert.equal(runtime.selector.value, "node-a");
  assert.match(runtime.toasts.at(-1).message, /activation refused/);
});

test("a reconciliation started before activation cannot repaint the confirmed Node", async () => {
  const backend = {
    activeId: "node-a",
    nodes: [
      { id: "node-a", display_name: "Alpha" },
      { id: "node-b", display_name: "Beta" },
    ],
  };
  const runtime = nodeContextRuntime(backend);
  await runtime.api.reconcileNodeContext();

  const staleResolvers = [];
  let holdOldReads = true;
  backend.handle = async (method, requestPath, body) => {
    if (method === "POST") {
      backend.activeId = body.node_id;
      holdOldReads = false;
      return { active_node_id: backend.activeId };
    }
    if (holdOldReads) {
      const snapshot = requestPath === "/api/project/status"
        ? { attached: true, target_root: "/tmp/app-a", active_node_id: "node-a", active_node: "Alpha" }
        : { nodes: backend.nodes, active_node_id: "node-a" };
      return new Promise((resolve) => staleResolvers.push(() => resolve(snapshot)));
    }
    if (requestPath === "/api/project/status") {
      return { attached: true, target_root: "/tmp/app-a", active_node_id: backend.activeId, active_node: "Beta" };
    }
    return { nodes: backend.nodes, active_node_id: backend.activeId };
  };

  const staleReconciliation = runtime.api.reconcileNodeContext({ external: true });
  while (staleResolvers.length < 2) await Promise.resolve();
  assert.equal(await runtime.api.activateNodeContext("node-b"), true);
  staleResolvers.forEach((resolve) => resolve());
  await staleReconciliation;

  assert.equal(runtime.selector.value, "node-b");
  assert.equal(runtime.state.project.active_node_id, "node-b");
  assert.equal(runtime.state.project.active_node, "Beta");
});

test("a local dirty draft can veto Node activation without losing its values", async () => {
  const prompt = { value: "keep this draft" };
  const priority = { value: "high" };
  const backdrop = { dataset: {} };
  const modal = {
    querySelector(query) {
      if (query.includes("new-goal-prompt")) return prompt;
      if (query.includes("new-goal-priority")) return priority;
      return null;
    },
    closest() { return backdrop; },
  };
  const backend = {
    activeId: "node-a",
    nodes: [
      { id: "node-a", display_name: "Alpha" },
      { id: "node-b", display_name: "Beta" },
    ],
    querySelector(query) {
      return query === "[data-testid='new-goal-modal']" ? modal : null;
    },
    confirm: async () => false,
  };
  const runtime = nodeContextRuntime(backend);
  await runtime.api.reconcileNodeContext();
  assert.equal(await runtime.api.activateNodeContext("node-b"), false);
  assert.equal(prompt.value, "keep this draft");
  assert.equal(priority.value, "high");
  assert.equal(runtime.calls.filter((call) => call.method === "POST").length, 0);
  assert.equal(runtime.selector.value, "node-a");
});

test("a local Feature composer draft can veto Node activation", async () => {
  const featureRoot = {
    dataset: {},
    _featureComposerHasDraft: () => true,
  };
  const backend = {
    activeId: "node-a",
    nodes: [
      { id: "node-a", display_name: "Alpha" },
      { id: "node-b", display_name: "Beta" },
    ],
    featureRoot,
    confirm: async (message) => {
      assert.match(message, /Feature/);
      return false;
    },
  };
  const runtime = nodeContextRuntime(backend);
  await runtime.api.reconcileNodeContext();

  assert.equal(await runtime.api.activateNodeContext("node-b"), false);
  assert.equal(runtime.calls.filter((call) => call.method === "POST").length, 0);
  assert.equal(runtime.selector.value, "node-a");
});

test("an external activation preserves a dirty draft but marks it stale and disables submission", async () => {
  const prompt = { value: "preserved text", disabled: false };
  const priority = { value: "medium", disabled: false };
  const submit = { disabled: false };
  const panel = { warnings: [], prepend(node) { this.warnings.unshift(node); } };
  const backdrop = {
    dataset: {},
    matches() { return false; },
    querySelector(query) { return query === ".modal" ? panel : null; },
    querySelectorAll() { return [prompt, priority, submit]; },
  };
  const modal = {
    querySelector(query) {
      if (query.includes("new-goal-prompt")) return prompt;
      if (query.includes("new-goal-priority")) return priority;
      return null;
    },
    closest() { return backdrop; },
  };
  const backend = {
    activeId: "node-a",
    nodes: [
      { id: "node-a", display_name: "Alpha" },
      { id: "node-b", display_name: "Beta" },
    ],
    querySelector(query) {
      return query === "[data-testid='new-goal-modal']" ? modal : null;
    },
  };
  const runtime = nodeContextRuntime(backend);
  await runtime.api.reconcileNodeContext();
  backend.activeId = "node-b";
  await runtime.api.reconcileNodeContext({ external: true });
  assert.equal(prompt.value, "preserved text");
  assert.equal(priority.value, "medium");
  assert.equal(prompt.disabled, true);
  assert.equal(priority.disabled, true);
  assert.equal(submit.disabled, true);
  assert.equal(backdrop.dataset.nodeContextStale, "true");
  assert.match(panel.warnings[0].textContent, /previous Node/);
  assert.equal(runtime.selector.value, "node-b");
});

test("local create, rename, and archive reconciliation redraws registry truth", async () => {
  const backend = {
    activeId: "node-a",
    nodes: [{ id: "node-a", display_name: "Alpha" }],
  };
  const runtime = nodeContextRuntime(backend);
  await runtime.api.reconcileNodeContext();
  backend.nodes.push({ id: "node-b", display_name: "Beta" });
  await runtime.api.reconcileNodeContext();
  assert.deepEqual(optionSnapshot(runtime.selector).map((option) => option.text), ["Alpha", "Beta"]);
  backend.nodes[1].display_name = "Beta renamed";
  await runtime.api.reconcileNodeContext();
  assert.equal(optionSnapshot(runtime.selector)[1].text, "Beta renamed");
  backend.nodes[1].archived = true;
  await runtime.api.reconcileNodeContext();
  assert.deepEqual(optionSnapshot(runtime.selector).map((option) => option.value), ["node-a"]);
});

test("two browser runtimes converge after a filtered api_mutation activation notice", async () => {
  const backend = {
    activeId: "node-a",
    nodes: [
      { id: "node-a", display_name: "Alpha" },
      { id: "node-b", display_name: "Beta" },
    ],
  };
  const first = nodeContextRuntime(backend);
  const second = nodeContextRuntime(backend);
  await Promise.all([first.api.reconcileNodeContext(), second.api.reconcileNodeContext()]);
  const readsBeforeUnrelatedMutation = first.calls.length;
  first.api.handleNodeContextMutationEvent({
    data: JSON.stringify({ method: "PATCH", path: "/api/goals/GOAL1", status: 200 }),
  });
  await new Promise((resolve) => setTimeout(resolve, 90));
  assert.equal(first.calls.length, readsBeforeUnrelatedMutation);
  first.api.handleNodeContextMutationEvent({
    data: JSON.stringify({ method: "POST", path: "/api/nodes/activate", status: 409 }),
  });
  await new Promise((resolve) => setTimeout(resolve, 90));
  assert.equal(first.calls.length, readsBeforeUnrelatedMutation);
  backend.activeId = "node-b";
  const event = { data: JSON.stringify({ method: "POST", path: "/api/nodes/activate", status: 200 }) };
  first.api.handleNodeContextMutationEvent(event);
  second.api.handleNodeContextMutationEvent(event);
  await new Promise((resolve) => setTimeout(resolve, 120));
  assert.equal(first.selector.value, "node-b");
  assert.equal(second.selector.value, "node-b");
  assert.ok(first.api.captureNodeContextGeneration() > 0);
  assert.ok(second.api.captureNodeContextGeneration() > 0);
});

test("a prior-generation read is rejected after authoritative Node transition", async () => {
  const backend = {
    activeId: "node-a",
    nodes: [
      { id: "node-a", display_name: "Alpha" },
      { id: "node-b", display_name: "Beta" },
    ],
  };
  const runtime = nodeContextRuntime(backend);
  await runtime.api.reconcileNodeContext();
  const oldGeneration = runtime.api.captureNodeContextGeneration();
  backend.activeId = "node-b";
  await runtime.api.reconcileNodeContext({ external: true });
  assert.equal(runtime.api.isNodeContextGenerationCurrent(oldGeneration), false);
});
