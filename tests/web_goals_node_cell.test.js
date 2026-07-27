const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

function browserRuntime() {
  const context = vm.createContext({
    URLSearchParams,
    htmlEscape: (value) => String(value)
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;"),
  });
  vm.runInContext(
    fs.readFileSync(
      path.join(__dirname, "../src/surfaces/web/static/js/features/goals-list.js"),
      "utf8",
    ),
    context,
  );
  vm.runInContext(`
    globalThis.goalsNodeCellTest = {
      render: (goal) => renderGoalNodeCell(goal),
    };
  `, context);
  return context.goalsNodeCellTest;
}

test("Goal node cells preserve the full escaped name for the hover tooltip", () => {
  const runtime = browserRuntime();

  assert.equal(
    runtime.render({
      node_display_name: 'A very long <remote> node named "Pine"',
      node_id: "fallback-node",
    }),
    '<span class="goals-node-value" title="A very long &lt;remote&gt; node named &quot;Pine&quot;">A very long &lt;remote&gt; node named &quot;Pine&quot;</span>',
  );
  assert.equal(
    runtime.render({ node_id: "fallback-node" }),
    '<span class="goals-node-value" title="fallback-node">fallback-node</span>',
  );
});
