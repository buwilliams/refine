const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

function goalDetailSource() {
  return fs.readFileSync(
    path.join(
      __dirname,
      "../src/surfaces/web/static/js/features/goals-detail.js",
    ),
    "utf8",
  );
}

test("Goal Cancel uses the public cancellation API and promises retention", () => {
  const source = goalDetailSource();
  const handler = source
    .split('bindOnce($("#btn-cancel"), "click"')[1]
    .split('bindOnce($("#btn-delete"), "click"')[0];

  assert.match(
    handler,
    /api\("POST", `\/api\/goals\/\$\{liveGoal\(\)\.id\}\/cancel`\)/,
  );
  assert.match(handler, /Workflow worktrees and branches will be retained/);
  assert.doesNotMatch(handler, /worktree \+ branch cleaned up/);
  assert.match(handler, /await loadGoalDetail\(liveGoal\(\)\.id\)/);
  assert.match(handler, /toast\("Cancelled", "info"\)/);
});
