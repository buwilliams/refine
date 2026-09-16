const assert = require("node:assert/strict");
const test = require("node:test");
const { openApp, apiFixture, GOAL, SKIP } = require("./support/web_app");

const steps = ["plan", "implement", "quality", "governance"];
const time = second => `2026-09-13T12:00:${String(second).padStart(2, "0")}Z`;
const event = (step, generation, summary = `${step} output`) => ({
  source: `workflow.${step}.enter`, generation, state: "succeeded",
  results: { skill: { binding_id: `${step}-skill`, role: step, outcome: "success", summary,
    evidence: [`${step} verification`], artifacts: { document: `${step}.md`, text: "<script>unsafe()</script>" } } },
});
function fixtureGoal() {
  return { ...structuredClone(GOAL), status: "governance", rounds: [{
    ...structuredClone(GOAL.rounds[0]),
    implementation_report: "Full implementation report", implementation_reported_at: time(20),
    event_results: Object.fromEntries(steps.map((step, index) => [step, event(step, index + 1)])),
    logs: [
      { datetime: time(0), category: "agent", message: "Unattributed history" },
      ...steps.flatMap((step, index) => [
        { datetime: time(index * 10 + 1), category: "state", message: `Workflow status changed: ${index ? steps[index - 1] : "todo"} -> ${step}` },
        { datetime: time(index * 10 + 2), category: "agent", message: `${step} agent activity` },
      ]),
      { datetime: time(35), category: "quality", message: "Governance candidate refresh validation" },
      { datetime: time(36), category: "agent", message: "Late Plan output", details: { workflow_state: "plan" } },
    ],
  }] };
}
async function openGoal(goal) {
  const app = await openApp({ fixture: path => path === "/api/goals/GOAL1" ? { goal } : apiFixture(path) });
  await app.page.goto(`${app.origin}/#/goals/GOAL1`);
  await app.page.waitForSelector('[data-testid="goal-round"]');
  return app;
}

test("flat workflow tabs put each step's artifacts before its own activity and preserve selection", { skip: SKIP }, async () => {
  const goal = fixtureGoal();
  goal.rounds[0].event_results.governance.results.skill.role = "quality";
  const app = await openGoal(goal);
  try {
    const { page } = app;
    const tabs = page.locator('.round-tabs [role="tab"]');
    assert.deepEqual(await tabs.allTextContents(), ["Request", "Plan", "Implement", "Quality", "Governance", "Failed", "Prompts", "Activity"]);
    assert.equal(await page.locator('[data-round-tab="plan"]').getAttribute("aria-selected"), "true");
    for (const step of steps) {
      await page.locator(`[data-round-tab="${step}"]`).click();
      const panel = page.locator(`[data-round-panel="${step}"]`);
      assert.equal(await panel.isVisible(), true);
      const text = await panel.innerText();
      assert.ok(text.indexOf(`${step} output`) < text.indexOf(`${step} agent activity`));
      assert.match(text, new RegExp(`${step}\\.md`));
      assert.match(text, /<script>unsafe\(\)<\/script>/);
      for (const other of steps.filter(other => other !== step)) assert.ok(!text.includes(`${other} agent activity`));
      assert.equal(await panel.locator('[role="tablist"]').count(), 0);
    }
    assert.match(await page.locator('[data-round-panel="implement"]').innerText(), /Full implementation report/);
    assert.match(await page.locator('[data-round-panel="governance"]').innerText(), /Governance candidate refresh validation/);
    assert.doesNotMatch(await page.locator('[data-round-panel="quality"]').innerText(), /Governance candidate refresh validation/);
    assert.match(await page.locator('[data-round-panel="plan"]').innerText(), /Late Plan output/);
    assert.equal(await page.locator('.round-step script').count(), 0);
    await page.locator('[data-round-tab="quality"]').click();
    await page.evaluate(goal => drawGoalDetail(goal), goal);
    assert.equal(await page.locator('[data-round-panel="quality"]').isVisible(), true);
    await page.locator('[data-round-tab="quality"]').focus();
    await page.keyboard.press("ArrowRight");
    assert.equal(await page.locator('[data-round-tab="governance"]').getAttribute("aria-selected"), "true");
    await page.keyboard.press("End");
    assert.equal(await page.locator('[data-round-panel="activity"]').isVisible(), true);
    assert.match(await page.locator('[data-round-panel="activity"]').innerText(), /Unattributed history/);
    await page.keyboard.press("Home");
    assert.equal(await page.locator('[data-round-panel="request"]').isVisible(), true);
    for (const width of [375, 1280]) {
      await page.setViewportSize({ width, height: 900 });
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    }
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test("retained artifacts stay labelled and deduplicated; empty steps and manual redirects do not borrow current state", { skip: SKIP }, async () => {
  const goal = fixtureGoal();
  const old = event("plan", 1, "Retained plan");
  goal.rounds[0].event_results = {};
  goal.rounds[0].workflow_candidate_refresh = { previous_gate_evidence: { event_results: { old } } };
  goal.rounds[0].prior_attempts = [{ event_results: { old } }];
  delete goal.rounds[0].implementation_report;
  goal.workflow_controls = [{ source_round: 1, at: time(13), from: "implement", to: "todo" },
    { source_round: 2, at: time(14), from: "todo", to: "quality" }];
  goal.rounds[0].logs.push({ datetime: time(15), category: "agent", message: "After manual parking" });
  const app = await openGoal(goal);
  try {
    const { page } = app;
    const plan = page.locator('[data-round-panel="plan"]');
    assert.match(await plan.innerText(), /Before candidate refresh/);
    assert.equal((await plan.innerText()).split("Retained plan").length - 1, 1);
    await page.locator('[data-round-tab="quality"]').click();
    assert.match(await page.locator('[data-round-panel="quality"]').innerText(), /No Quality artifacts/);
    for (const step of steps) assert.doesNotMatch(await page.locator(`[data-round-panel="${step}"]`).innerText(), /After manual parking/);
    assert.match(await page.locator('[data-round-panel="activity"]').innerText(), /After manual parking/);
    await page.evaluate(goal => drawGoalDetail({ ...goal, round_edit_revision: 1,
      rounds: [{ prompt: "Different Round", created: "later", logs: [] }] }), goal);
    assert.equal(await page.locator('[data-round-panel="request"]').isVisible(), true);
    for (const step of steps) {
      assert.match(await page.locator(`[data-round-panel="${step}"]`).innerText(), new RegExp(`No ${step[0].toUpperCase() + step.slice(1)} activity`));
    }
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test("Failed tab supports investigation, keyboard navigation, refreshes, and narrow screens", { skip: SKIP }, async () => {
  const goal = fixtureGoal();
  goal.status = "failed";
  Object.assign(goal.rounds[0], { failure_category: "integration", failure_message: "Branch tip moved <candidate>",
    failure_at: time(40), quality_state: "passed", rule_state: "passed" });
  goal.rounds[0].logs.push({ datetime: time(40), severity: "error", message: "Integration details",
    details: { stderr: "<script>unsafe()</script>\n".repeat(60), candidate_commit: "a".repeat(256) } });
  const app = await openGoal(goal);
  try {
    const { page } = app;
    await page.locator('[data-round-tab="governance"]').click();
    await page.keyboard.press("ArrowRight");
    const tab = page.locator('[data-round-tab="failed"]');
    const panel = page.locator('[data-round-panel="failed"]');
    assert.equal(await tab.getAttribute("aria-selected"), "true");
    assert.equal(await tab.getAttribute("aria-controls"), await panel.getAttribute("id"));
    assert.equal(await panel.getAttribute("aria-labelledby"), await tab.getAttribute("id"));
    assert.equal(await panel.isVisible(), true);
    assert.match(await panel.innerText(), /Branch tip moved <candidate>/);
    assert.match(await panel.innerText(), /Integration details/);
    assert.match(await panel.innerText(), /<script>unsafe\(\)<\/script>/);
    assert.equal(await panel.locator('script, details, [role="tablist"]').count(), 0);
    for (const width of [375, 1280]) {
      await page.setViewportSize({ width, height: 900 });
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
      for (const element of await panel.locator(".round-failures, pre").all()) {
        const style = await element.evaluate(el => ({ overflow: getComputedStyle(el).overflowY,
          maxHeight: getComputedStyle(el).maxHeight, fits: el.scrollWidth <= el.clientWidth + 1 }));
        assert.equal(style.overflow, "visible");
        assert.equal(style.maxHeight, "none");
        assert.equal(style.fits, true);
      }
    }
    await page.evaluate(goal => drawGoalDetail(goal), goal);
    assert.equal(await panel.isVisible(), true);
    await tab.focus();
    await page.keyboard.press("ArrowRight");
    assert.equal(await page.locator('[data-round-panel="prompts"]').isVisible(), true);
    await page.keyboard.press("ArrowLeft");
    assert.equal(await panel.isVisible(), true);
    const failed = structuredClone(goal.rounds[0]);
    goal.status = "implement";
    Object.assign(goal.rounds[0], { failure_message: "", failure_category: "", failure_at: "", prior_attempts: [failed] });
    await page.evaluate(goal => drawGoalDetail(goal), goal);
    assert.equal(await panel.isVisible(), true);
    assert.match(await panel.innerText(), /No current failure is recorded/);
    assert.match(await panel.innerText(), /Historical failures and diagnostics/);
    assert.match(await panel.innerText(), /Previous execution/);
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test("Failed is always visible with an empty state and keeps evidence within each Round", { skip: SKIP }, async () => {
  const goal = fixtureGoal();
  goal.status = "failed";
  goal.rounds.unshift({ prompt: "Earlier request", created: "2026-09-12T12:00:00Z", logs: [] });
  goal.rounds[1].failure_message = "Latest Round failure";
  const app = await openGoal(goal);
  try {
    const { page } = app;
    const rounds = page.getByTestId("goal-round");
    await rounds.first().getByTestId("goal-round-summary").click();
    assert.equal(await rounds.first().locator('[data-round-tab="request"]').getAttribute("aria-selected"), "true");
    await rounds.first().locator('[data-round-tab="failed"]').click();
    assert.match(await rounds.first().getByRole("tabpanel").innerText(), /No failure evidence/);
    assert.doesNotMatch(await rounds.first().getByRole("tabpanel").innerText(), /Latest Round failure/);
    await rounds.last().locator('[data-round-tab="failed"]').click();
    assert.match(await rounds.last().getByRole("tabpanel").innerText(), /Latest Round failure/);
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});
