const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

function browserRuntime() {
  const context = vm.createContext({
    URLSearchParams,
    location: { hash: "#/" },
  });
  for (const file of [
    "../src/surfaces/web/static/js/features/goals-list.js",
    "../src/surfaces/web/static/js/features/dashboard.js",
  ]) {
    vm.runInContext(fs.readFileSync(path.join(__dirname, file), "utf8"), context);
  }
  vm.runInContext(`
    globalThis.dashboardAttentionTest = {
      goalsHash: (item, reporter, scope) =>
        dashboardAttentionGoalsHash(item, reporter, scope),
    };
  `, context);
  return context.dashboardAttentionTest;
}

test("failed Goal recovery attention links to the selected reporter and node scope", () => {
  const runtime = browserRuntime();

  const hash = runtime.goalsHash(
    { filter: { status: "failed" } },
    "Buddy Williams",
    "current",
  );

  assert.equal(
    hash,
    "#/goals?status=failed&reporter=Buddy+Williams&node=current",
  );
});
