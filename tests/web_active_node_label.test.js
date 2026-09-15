"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const source = fs.readFileSync(
  path.join(__dirname, "../src/surfaces/web/static/js/common.js"),
  "utf8",
);
const activeNodeFunctions = source.slice(
  source.indexOf("function updateActiveNodeLabel()"),
  source.indexOf("function hasAttachedProject()"),
);

test("authoritative project status labels the browser with the new Goal owner", () => {
  const document = {
    title: "refine",
  };
  const context = vm.createContext({
    document,
    state: {
      project: {
        attached: true,
        active_node_id: "port-owner",
        active_node: "Port Owner",
        nodes: [
          { id: "stale-base", display_name: "Stale Base Node" },
          { id: "port-owner", display_name: "Port Owner" },
        ],
      },
    },
  });
  vm.runInContext(`${activeNodeFunctions}\nupdateActiveNodeLabel();`, context);

  const newlyCreatedGoal = {
    node_id: "port-owner",
    node_display_name: "Port Owner",
  };
  assert.equal(newlyCreatedGoal.node_id, context.state.project.active_node_id);
  assert.equal(newlyCreatedGoal.node_display_name, context.state.project.active_node);
  assert.equal(document.title, "Port Owner - refine");
  assert.doesNotMatch(document.title, /Stale Base Node/);
});
