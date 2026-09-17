const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const { BROWSER, SKIP } = require("./support/web_app");
const ROOT = path.join(__dirname, "../src/surfaces/web/static");
async function fixture() {
  const browser = await BROWSER.chromium.launch({
    headless: true,
    executablePath: BROWSER.executablePath,
  });
  const page = await browser.newPage();
  await page.setContent('<main id="main"></main>');
  await page.evaluate(() => {
    window.state = { currentRoute: "planning", lastReporter: "Another person" };
    window.htmlEscape = (s) =>
      String(s ?? "")
        .replaceAll("&", "&amp;")
        .replaceAll("<", "&lt;")
        .replaceAll('"', "&quot;");
    window.workflowStatusLabel = (s) => s;
    window.captureNodeContextGeneration = () => 1;
    window.isNodeContextGenerationCurrent = (g) => g === 1;
    window.errors = [];
    window.showActionError = (e) => errors.push(e.message);
    window.requests = [];
    window.snapshot = {
      boards: [
        {
          id: "board",
          name: "Shared",
          revision: 1,
          lanes: [
            { id: "ideas", name: "Ideas", action: "none" },
            { id: "done", name: "Done", action: "none" },
          ],
        },
      ],
      cards: [
        {
          placement: {
            goal_id: "GOAL1",
            board_id: "board",
            lane_id: "ideas",
            position: 1,
            revision: 1,
          },
          goal: {
            id: "GOAL1",
            name: "Read paper",
            status: "draft",
            reporter: "Buddy",
            node_id: "other-node",
            priority: "low",
            workflow_revision: 3,
          },
        },
      ],
      actions: [],
      migration: true,
    };
    window.api = async (method, url, body) => {
      if (method === "GET") return structuredClone(snapshot);
      requests.push({ method, url, body });
      return { id: body.request_id, state: "queued" };
    };
    window.hubModal = (title, content) => {
      const r = document.createElement("div");
      r.className = "modal-backdrop";
      r.innerHTML = `<h2>${title}</h2>${content}`;
      r._close = () => r.remove();
      r._nodeGeneration = 1;
      document.body.append(r);
      return r;
    };
    window.hubAction = async (root, fn) => {
      try {
        await fn();
      } catch (e) {
        errors.push(e.message);
      }
    };
  });
  await page.addScriptTag({ path: path.join(ROOT, "js/features/planning.js") });
  await page.evaluate(() => renderPlanning());
  return { browser, page };
}
test(
  "shared cards remain visible across Reporter selection and keyboard move preserves Goal identity",
  { skip: SKIP },
  async () => {
    const { browser, page } = await fixture();
    try {
      assert.match(
        await page.locator(".planning-page").innerText(),
        /Read paper/,
      );
      assert.match(await page.locator(".planning-card").innerText(), /Buddy/);
      await page.locator("[data-card='GOAL1']").focus();
      await page.keyboard.press("m");
      await page.locator(".modal-backdrop [data-lane]").selectOption("done");
      await page.locator("[data-planning-save]").click();
      const requests = await page.evaluate(() => requests);
      assert.equal(requests.length, 1);
      assert.equal(requests[0].body.operation, "card.move");
      assert.equal(requests[0].body.goal_id, "GOAL1");
      assert.equal(requests[0].body.lane_id, "done");
      assert.equal(requests[0].body.expected_revision, 1);
      assert.equal(requests[0].body.data.status, undefined);
      assert.deepEqual(await page.evaluate(() => errors), []);
    } finally {
      await browser.close();
    }
  },
);
test(
  "card edits preserve input when a revision conflict is returned",
  { skip: SKIP },
  async () => {
    const { browser, page } = await fixture();
    try {
      await page.getByRole("button", { name: "Edit", exact: true }).click();
      await page.locator("input[name=name]").fill("Keep my draft");
      await page.evaluate(() => {
        window.api = async () => {
          throw new Error("Planning record changed; refresh its revision");
        };
      });
      await page.locator(".modal-backdrop button[type=submit]").click();
      assert.equal(
        await page.locator("input[name=name]").inputValue(),
        "Keep my draft",
      );
      assert.match(await page.locator("[data-error]").innerText(), /changed/);
    } finally {
      await browser.close();
    }
  },
);
test("Project Planning replaces Todo navigation and exposes Draft", () => {
  const index = fs.readFileSync(path.join(ROOT, "index.html"), "utf8");
  assert.match(index, /data-testid="nav-planning"/);
  assert.doesNotMatch(index, /data-add-toolbar-tab="todo"/);
  assert.match(index, /features\/planning.js/);
  const common = fs.readFileSync(path.join(ROOT, "js/common.js"), "utf8");
  assert.match(common, /draft: "Draft"/);
});

test(
  "Draft Goal links return to Project Planning",
  { skip: SKIP },
  async () => {
    const { openApp, apiFixture, GOAL } = require("./support/web_app");
    const app = await openApp({
      fixture(pathname) {
        if (pathname === "/api/goals/GOAL1")
          return {
            goal: {
              ...GOAL,
              status: "draft",
              rounds: [],
              description: "Research the onboarding flow",
              planning: { board_id: "shared", lane_id: "ideas" },
            },
          };
        return apiFixture(pathname);
      },
    });
    try {
      await app.page.goto(`${app.origin}/#/goals/GOAL1`);
      await app.page.waitForURL(/#\/planning\?card=GOAL1$/);
      assert.equal(await app.page.getByTestId("goal-detail").count(), 0);
      assert.deepEqual(app.pageErrors, []);
    } finally {
      await app.close();
    }
  },
);
