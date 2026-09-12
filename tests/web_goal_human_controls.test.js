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
    for (const file of ["base.css", "common.css", "modals.css", "goals.css", "theme.css"]) {
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
      window.diagnosticDetailsText = value => JSON.stringify(value, null, 2);
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
    await page.addScriptTag({ path: path.join(__dirname, "../src/surfaces/web/static/js/features/workflow-controls.js") });
    await page.evaluate(() => {
      window.realComputeFailureBanner = computeFailureBanner;
      for (const name of ["computeFailureBanner", "computeGovernanceBanner", "computeFeatureBlockingNotice"]) window[name] = () => null;
      goalDetailContainer = () => document.querySelector("#body");
      loadGoalDetail = async () => {};
      window.realBindRoundFormSubmit = bindRoundFormSubmit;
      bindRoundFormSubmit = () => {};
      window.goal = { id: "GOAL1", name: "Repair", status: "failed", workflow_revision: 42, workflow_controls: [{ source_round: 2, at: "today", from: "failed", to: "todo", request: { reason: "Retry after repair" } }],
        rounds: [
          { prompt: "Original", created: "first", failure_message: "Original attempt error", logs: [{ severity: "error", message: "First Round log" }] },
          { prompt: "Failed retry", created: "second", event_results: { plan: { generation: 3, results: { planner: { role: "plan", outcome: "success", summary: "Task #1 - Restore the navigation plan." } } } }, failure_message: "Retry attempt error", logs: [{ severity: "error", message: "Second Round log", details: { evidence: "Full diagnostic evidence\n".repeat(60), invocation: "a".repeat(256) } }] }
        ] };
      drawGoalDetail(goal);
    });
    const latestRound = page.getByTestId("goal-round").last();
    assert.equal(await latestRound.getByRole("tab", { name: "Plan", exact: true }).getAttribute("aria-selected"), "true");
    assert.equal(await latestRound.getByRole("tabpanel").count(), 1);
    assert.equal(await page.locator(".round .card").count(), 0);
    assert.equal(await page.getByText("Status and history", { exact: true }).count(), 0);
    await latestRound.getByRole("tab", { name: "Activity", exact: true }).click();
    assert.match(await latestRound.getByRole("tabpanel").textContent(), /Second Round log/);
    const activity = latestRound.getByRole("tabpanel");
    assert.equal(await activity.locator("details").count(), 0);
    assert.match(await activity.textContent(), /Retry after repair/);
    assert.match(await activity.textContent(), /Full diagnostic evidence/);
    assert.equal(await activity.locator("h3").first().textContent(), "Why this Round failed");
    for (const element of await activity.locator(".round-history, .round-log, pre").all()) {
      const style = await element.evaluate(el => ({ overflow: getComputedStyle(el).overflowY, maxHeight: getComputedStyle(el).maxHeight, fits: el.scrollWidth <= el.clientWidth + 1 }));
      assert.equal(style.overflow, "visible");
      assert.equal(style.maxHeight, "none");
      assert.equal(style.fits, true, "long diagnostics should wrap without horizontal scrolling");
    }
    await page.evaluate(() => drawGoalDetail(goal));
    assert.equal(await latestRound.getByRole("tab", { name: "Activity", exact: true }).getAttribute("aria-selected"), "true");
    await latestRound.getByRole("tab", { name: "Activity", exact: true }).focus();
    await page.keyboard.press("Home");
    assert.equal(await latestRound.getByRole("tab", { name: "Request", exact: true }).getAttribute("aria-selected"), "true");
    await page.keyboard.press("ArrowRight");
    assert.equal(await latestRound.getByRole("tab", { name: "Plan", exact: true }).getAttribute("aria-selected"), "true");
    await page.keyboard.press("ArrowRight");
    assert.match(await latestRound.getByRole("tabpanel").textContent(), /No implementation report/);
    await latestRound.getByTestId("goal-round-summary").click();
    await page.evaluate(() => drawGoalDetail(goal));
    assert.equal(await latestRound.getAttribute("open"), null);
    await latestRound.getByTestId("goal-round-summary").click();
    assert.equal(await latestRound.getByRole("tab", { name: "Report", exact: true }).getAttribute("aria-selected"), "true");
    await page.evaluate(() => drawGoalDetail({ ...goal, id: "OTHER" }));
    assert.equal(await latestRound.getByRole("tab", { name: "Plan", exact: true }).getAttribute("aria-selected"), "true");
    await page.evaluate(() => drawGoalDetail(goal));
    assert.equal(await latestRound.getByRole("tab", { name: "Report", exact: true }).getAttribute("aria-selected"), "true");
    assert.equal(await page.locator('[data-testid="goal-round-delete"] svg').count(), 2);
    assert.equal(await page.locator("#btn-workflow-control").count(), 0);
    assert.equal(await page.getByTestId("goal-failure-banner").count(), 0);
    assert.match(await page.getByTestId("goal-implementation-plan-summary").textContent(), /Task #1 - Restore the navigation plan/);
    assert.equal(await page.locator('.goal-detail > [data-testid="goal-failure-summary"]').count(), 0);
    assert.equal(await page.locator('[data-testid="goal-round"] [data-testid="goal-failure-summary"]').count(), 2);
    const firstLog = await page.getByTestId("goal-round-log").first().textContent();
    assert.match(firstLog, /First Round log/);
    assert.doesNotMatch(firstLog, /Second Round log/);
    assert.equal(await page.getByTestId("goal-step-toggle").getAttribute("class"),
      await page.getByTestId("goal-action-menu-toggle").getAttribute("class"));
    await page.getByTestId("goal-step-primary").click();
    assert.equal(await page.evaluate(() => requests.at(-1).body.to), "todo");
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
    await page.evaluate(() => {
      bindRoundFormSubmit = realBindRoundFormSubmit;
      computeFailureBanner = realComputeFailureBanner;
      goal = { ...goal, status: "failed", rounds: [], workflow_controls: [
        { request: { reason: "Explicit Round deletion" } }
      ] };
      drawGoalDetail(goal);
    });
    assert.equal(await page.getByTestId("goal-failure-banner").count(), 0);
    assert.equal(await page.getByText(/Workflow decisions/).count(), 0);
    assert.equal(await page.getByTestId("goal-round-form").getAttribute("data-kind"), "submit");
    await page.getByTestId("goal-round-prompt").fill("Start again after deleting the last Round");
    await page.getByTestId("goal-round-submit").click();
    const firstRound = await page.evaluate(() => requests.at(-1));
    assert.equal(firstRound.method, "POST");
    assert.equal(firstRound.path, "/api/goals/GOAL1/rounds");
    assert.equal(firstRound.body.prompt, "Start again after deleting the last Round");
    assert.deepEqual(await page.evaluate(() => errors), []);
  } finally { await browser.close(); }
});
