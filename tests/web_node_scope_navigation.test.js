const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

function navigationRuntime(hash) {
  const links = ["dashboard", "goals"].map((destination) => ({
    dataset: { nodeScopeDestination: destination },
    href: "",
    setAttribute(name, value) {
      if (name === "href") this.href = value;
    },
  }));
  const context = vm.createContext({
    URLSearchParams,
    location: { hash },
    $$: () => links,
  });
  vm.runInContext(
    fs.readFileSync(
      path.join(__dirname, "../src/surfaces/web/static/js/node-scope-navigation.js"),
      "utf8",
    ),
    context,
  );
  vm.runInContext(`
    syncNodeScopeNavigation();
    globalThis.nodeScopeNavigationTest = {
      links: () => $$("unused").map((link) => link.href),
      navigate: (destination, source) => nodeScopeNavigationHash(destination, source),
    };
  `, context);
  return context.nodeScopeNavigationTest;
}

test("Current node survives repeated Dashboard and Goals navigation", () => {
  const dashboard = navigationRuntime("#/");
  assert.deepEqual(Array.from(dashboard.links()), ["#/", "#/goals?node=current"]);
  assert.equal(dashboard.navigate("#/", "#/goals?node=current"), "#/");
  assert.equal(dashboard.navigate("#/goals", "#/"), "#/goals?node=current");
});

test("explicit All nodes survives repeated Dashboard and Goals navigation", () => {
  const dashboard = navigationRuntime("#/?node=all");
  assert.deepEqual(Array.from(dashboard.links()), ["#/?node=all", "#/goals?node=all"]);
  assert.equal(dashboard.navigate("#/", "#/goals?node=all"), "#/?node=all");
  assert.equal(dashboard.navigate("#/goals", "#/?node=all"), "#/goals?node=all");
  assert.equal(dashboard.navigate("#/", "#/goals"), "#/?node=all");
});

test("a named Goals node is not projected onto Dashboard", () => {
  const goals = navigationRuntime("#/goals?node=node-a");
  assert.deepEqual(Array.from(goals.links()), ["#/", "#/goals"]);
  assert.equal(goals.navigate("#/", "#/goals?node=node-a"), "#/");
});
