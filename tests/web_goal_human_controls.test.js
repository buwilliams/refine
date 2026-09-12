const assert = require("node:assert/strict");
const path = require("node:path");
const test = require("node:test");
const { chromium } = require("playwright");

// Real DOM clicks exercise the production renderer and handlers. Only unrelated
// modal services and HTTP transport are stubbed; no live Goals are mutated.
test("every Goal step is selectable and Round deletion uses its inspected revision", async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    const page = await browser.newPage();
    await page.setContent('<div id="body" class="goal-detail-modal-body"></div>');
    for (const file of ["base.css", "goals.css"]) {
      await page.addStyleTag({ path: path.join(__dirname, "../src/surfaces/web/static/css", file) });
    }
    await page.evaluate(() => {
      window.$ = s => document.querySelector(s);
      window.$$ = s => Array.from(document.querySelectorAll(s));
      window.state = { lastReporter: "User", reporters: [], project: {} };
      window.htmlEscape = s => String(s ?? "").replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll('"', "&quot;");
      window.governanceReviewStatus = () => ({ visible: false });
      window.nodeContextActiveNodeId = () => "default";
      window.workflowStatusLabel = s => s[0].toUpperCase() + s.slice(1);
      window.fmtTime = s => s || "";
      window.bindOnce = (el, event, handler) => el?.addEventListener(event, handler);
      window.renderInto = (container, html, bind) => { container.innerHTML = html; bind(); };
      for (const name of ["renderBanners", "recordFeatureBlockingNotice", "bindFailureBannerActions"]) window[name] = () => {};
      for (const name of ["computeFailureBanner", "computeGovernanceBanner", "computeFeatureBlockingNotice"]) window[name] = () => null;
      window.renderWorkflowOutcome = () => "";
      window.hubId = () => "test-request";
      window.toast = () => {};
      window.errors = [];
      window.requests = [];
      window.api = async (method, path, body) => { requests.push({ method, path, body }); return {}; };
      window.showActionError = e => errors.push(e.message);
      window.confirmDeletion = true;
      window.modalConfirm = async () => confirmDeletion;
    });
    await page.addScriptTag({ path: path.join(__dirname, "../src/surfaces/web/static/js/features/goals-detail.js") });
    await page.evaluate(() => {
      for (const name of ["computeFailureBanner", "computeGovernanceBanner", "computeFeatureBlockingNotice"]) window[name] = () => null;
      goalDetailContainer = () => document.querySelector("#body");
      loadGoalDetail = async () => {};
      bindRoundFormSubmit = () => {};
      window.goal = { id: "GOAL1", name: "Repair", status: "failed", workflow_revision: 42,
        rounds: [{ prompt: "Original", created: "first" }, { prompt: "Failed retry", created: "second" }] };
      drawGoalDetail(goal);
    });
    assert.equal(await page.locator('[data-testid="goal-round-delete"] svg').count(), 2);
    for (const step of ["backlog", "todo", "plan", "implement", "quality", "governance", "review", "done", "failed", "cancelled"]) {
      await page.getByTestId("goal-step-toggle").click();
      const panel = await page.locator(".goal-step-menu .nav-menu-panel").boundingBox();
      assert.ok(panel.x >= 0, "step choices must stay inside the viewport");
      await page.getByTestId(`goal-step-${step}`).click();
      const request = await page.evaluate(() => requests.at(-1));
      assert.equal(request.body.to, step);
      assert.equal(request.body.force, true);
      assert.equal(request.body.expected_revision, 42);
      assert.equal(request.path, "/api/workflow/goals/GOAL1/move");
    }
    await page.getByRole("button", { name: "Delete Round 2", exact: true }).click();
    const deletion = await page.evaluate(() => requests.at(-1));
    assert.equal(deletion.method, "DELETE");
    assert.equal(deletion.path, "/api/goals/GOAL1/rounds/1");
    assert.equal(deletion.body.expected_revision, 42);
    const count = await page.evaluate(() => { confirmDeletion = false; return requests.length; });
    await page.getByRole("button", { name: "Delete Round 1", exact: true }).click();
    assert.equal(await page.evaluate(() => requests.length), count);
    assert.deepEqual(await page.evaluate(() => errors), []);
  } finally { await browser.close(); }
});
