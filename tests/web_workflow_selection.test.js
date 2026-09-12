const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const statuses = ["backlog", "todo", "plan", "implement", "quality", "governance", "review", "done", "failed", "cancelled"];
function render(selectedStatuses) {
  const context = vm.createContext({ workflowStatuses: () => statuses,
    workflowStatusLabel: s => s, fmtCount: String, htmlEscape: String });
  vm.runInContext(fs.readFileSync(path.join(__dirname, "../src/surfaces/web/static/js/workflow-visualization.js"), "utf8"), context);
  return context.renderWorkflowVisualization({ selectedStatuses, hrefForStatus: s => `#/goals?status=${s}` });
}
test("workflow selection highlights one or several filtered steps and clears for all", () => {
  const single = render(["failed"]);
  assert.equal((single.match(/workflow-status-selected/g) || []).length, 1);
  assert.match(single, /failed workflow-status-selected" aria-current="true"/);
  assert.equal((render(["failed", "plan"]).match(/workflow-status-selected/g) || []).length, 2);
  assert.doesNotMatch(render(statuses), /workflow-status-selected|aria-current/);
  assert.doesNotMatch(render([]), /workflow-status-selected|aria-current/);
});
