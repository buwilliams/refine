const { selectMain } = require("./support/web_app");
const assert = require("node:assert/strict");
const test = require("node:test");
const {
  openApp,
  apiFixture,
  GOAL,
  FEATURE,
  SKIP,
} = require("./support/web_app");

async function planningApp() {
  const requests = [];
  const goal = (id, name, status = "draft") => ({
    ...GOAL,
    id,
    name,
    status,
    description: "Explore the next improvement",
    rounds: [],
    workflow_revision: 1,
  });
  const snapshot = {
    boards: [
      {
        id: "board",
        name: "Product ideas",
        revision: 1,
        lanes: [
          { id: "ideas", name: "Ideas", action: "none" },
          { id: "ready", name: "Ready", action: "accept_into_backlog" },
          { id: "done", name: "Done", action: "none" },
        ],
      },
      {
        id: "personal",
        name: "Personal",
        revision: 1,
        lanes: [{ id: "open", name: "Open", action: "none" }],
      },
    ],
    cards: [
      {
        goal: goal("DRAFT1", "Research onboarding"),
        placement: {
          goal_id: "DRAFT1",
          board_id: "board",
          lane_id: "ideas",
          position: 1024,
          revision: 1,
        },
      },
      {
        goal: goal("DRAFT2", "Improve documentation"),
        placement: {
          goal_id: "DRAFT2",
          board_id: "board",
          lane_id: "ready",
          position: 2048,
          revision: 1,
        },
      },
    ],
    actions: [],
    migration: true,
    nodes: [{ id: "node-a", display_name: "Development", enabled: true }],
  };
  const available = goal("EXISTING", "Existing workflow Goal", "backlog");
  const app = await openApp({
    fixture(path, request) {
      if (path === "/api/planning") return structuredClone(snapshot);
      if (path === "/api/skills/catalog")
        return { sources: ["custom", "planning.lane.enter"] };
      if (path === "/api/features/FEAT1")
        return {
          feature: {
            ...FEATURE,
            goals: [
              ...snapshot.cards.map((card) => ({
                ...card.goal,
                feature_id: "FEAT1",
              })),
              available,
            ],
          },
        };
      if (path === "/api/goals") {
        const url = new URL(request.url());
        const goals =
          url.searchParams.get("exclude_draft") === "1"
            ? [available]
            : [...snapshot.cards.map((c) => c.goal), available];
        const query = (url.searchParams.get("q") || "").toLowerCase();
        const matches = goals.filter((goal) =>
          goal.name.toLowerCase().includes(query),
        );
        return {
          goals: matches,
          total: matches.length,
          matching_ids: matches.map((g) => g.id),
          facets: { status_counts: { draft: 2, backlog: 1 } },
        };
      }
      if (path === "/api/planning/commands") {
        const body = request.postDataJSON();
        requests.push(body);
        let result = {};
        if (body.operation === "card.delete") {
          const card = snapshot.cards.find(
            (card) => card.placement.goal_id === body.goal_id,
          );
          if (
            card.goal.status !== "draft" ||
            card.goal.workflow_revision !== body.data.expected_goal_revision ||
            card.placement.revision !== body.expected_revision
          ) {
            return {
              state: "failed",
              message: "Goal changed; refresh before deleting",
            };
          }
          snapshot.cards = snapshot.cards.filter(
            (card) => card.placement.goal_id !== body.goal_id,
          );
          result = { goal_id: body.goal_id, deleted: true };
        }
        if (
          body.operation === "lane.update" ||
          body.operation === "lane.delete"
        ) {
          const board = snapshot.boards.find(
            (board) => board.id === body.board_id,
          );
          if (body.expected_revision !== board.revision)
            return {
              state: "failed",
              message: "Board changed. Reopen lane settings.",
            };
          const index = board.lanes.findIndex(
            (lane) => lane.id === body.lane_id,
          );
          const [lane] = board.lanes.splice(index, 1);
          if (body.operation === "lane.update") {
            const { position = index, ...settings } = body.data;
            Object.assign(lane, settings);
            board.lanes.splice(position, 0, lane);
          }
          board.revision++;
          result = structuredClone(board);
        }
        if (body.operation === "board.delete") {
          snapshot.boards = snapshot.boards.filter(
            (board) => board.id !== body.board_id,
          );
          snapshot.cards = snapshot.cards.filter(
            (card) => card.placement.board_id !== body.board_id,
          );
        }
        if (body.operation === "board.create") {
          result = {
            id: "new-board",
            name: body.data.name,
            revision: 1,
            lanes: [{ id: "new-lane", name: "Ideas", action: "none" }],
          };
          snapshot.boards.push(result);
        }
        if (
          body.operation === "card.create" ||
          body.operation === "card.attach"
        ) {
          const item =
            body.operation === "card.attach"
              ? available
              : goal("NEW", body.data.name);
          snapshot.cards.push({
            goal: item,
            placement: {
              goal_id: item.id,
              board_id: body.board_id,
              lane_id: body.lane_id,
              position: 4096,
              revision: 1,
            },
          });
        }
        if (body.operation === "card.move") {
          const card = snapshot.cards.find(
            (c) => c.placement.goal_id === body.goal_id,
          );
          Object.assign(card.placement, {
            board_id: body.board_id,
            lane_id: body.lane_id,
            position: body.data.position ?? 4096,
          });
        }
        return { state: "complete", result };
      }
      if (path === "/api/goals/DRAFT1") return { goal: snapshot.cards[0].goal };
      return apiFixture(path, request);
    },
  });
  await app.page.goto(`${app.origin}/#/planning?board=board`);
  await app.page.getByTestId("planning-page").waitFor();
  return { ...app, requests, snapshot };
}

test("Planning composer and Dashboard retain their state through Back and Forward", { skip: SKIP }, async () => {
  const app = await planningApp();
  try {
    const { page } = app;
    await page.locator('[data-planning-add="ideas"]').click();
    const input = page.getByRole("combobox", { name: "Card title or existing Goal" });
    await input.fill("Unsubmitted planning idea");
    await input.evaluate(el => { window.retainedComposer = el; });
    await selectMain(page, "dashboard");
    await page.locator(".dashboard-status-grid").waitFor();
    await page.getByTestId("dashboard-contributor-rankings-panel").locator("summary").click();
    await page.evaluate(() => { window.retainedDashboard = document.getElementById("dash"); });
    await page.goBack();
    await input.waitFor();
    assert.equal(await input.inputValue(), "Unsubmitted planning idea");
    assert.equal(await input.evaluate(el => el === retainedComposer), true);
    await page.goForward();
    await page.locator(".dashboard-status-grid").waitFor();
    assert.equal(await page.evaluate(() => document.getElementById("dash") === retainedDashboard), true);
    assert.equal(await page.getByTestId("dashboard-contributor-rankings-panel").evaluate(el => el.open), true);
    assert.deepEqual(app.requests, []);
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test(
  "Planning has its own board navigation, aligned controls, distinct surfaces, and shared input styles",
  { skip: SKIP },
  async () => {
    const app = await planningApp();
    try {
      const { page } = app;
      assert.deepEqual(
        await page
          .locator(".rail-section")
          .evaluateAll((els) => els.map((el) => el.id)),
        [
          "rail-main-section",
          "rail-windows-section",
          "rail-planning-section",
          "rail-skills-section",
          "rail-hubs-section",
        ],
      );
      assert.equal(
        await page.locator('.rail-main [data-route="planning"]').count(),
        0,
      );
      assert.equal(
        await page
          .getByRole("switch", { name: "Show archived" })
          .getAttribute("aria-checked"),
        "false",
      );
      await page.getByRole("switch", { name: "Show archived" }).click();
      assert.equal(
        await page
          .getByRole("switch", { name: "Show archived" })
          .getAttribute("aria-checked"),
        "true",
      );
      assert.equal(await page.locator("[data-planning-attach]").count(), 0);
      const geometry = await page.evaluate(() => {
        const center = (selector) => {
          const b = document.querySelector(selector).getBoundingClientRect();
          return b.y + b.height / 2;
        };
        return [
          center("[data-planning-archived]"),
          center("[data-planning-board-settings]"),
          center("[data-planning-new-lane]"),
        ];
      });
      assert.ok(Math.max(...geometry) - Math.min(...geometry) < 2);
      for (const theme of ["light", "dark"]) {
        await page.evaluate(
          (theme) => (document.documentElement.dataset.theme = theme),
          theme,
        );
        const colors = await page.evaluate(() =>
          ["body", ".planning-lane", ".planning-card"].map(
            (s) => getComputedStyle(document.querySelector(s)).backgroundColor,
          ),
        );
        assert.equal(
          new Set(colors).size,
          3,
          `${theme}: page, lane, and card surfaces must differ`,
        );
        await page.locator('[data-planning-add="ideas"]').click();
        const styles = await page.evaluate(() => {
          const input = document.querySelector(".planning-composer input"),
            standard = document.createElement("input");
          standard.type = "text";
          input.after(standard);
          const props = [
            "height",
            "padding",
            "borderRadius",
            "backgroundColor",
            "color",
            "borderWidth",
          ];
          const read = (e) => props.map((p) => getComputedStyle(e)[p]);
          const result = [read(input), read(standard)];
          standard.remove();
          return result;
        });
        assert.deepEqual(styles[0], styles[1]);
        await page.keyboard.press("Escape");
      }
      await page.locator("#rail-toggle").click();
      await page.getByTestId("planning-menu").click();
      assert.equal(
        await page
          .locator(
            '#planning-board-options [data-planning-nav-board="personal"] span',
          )
          .isVisible(),
        true,
      );
      await page.keyboard.press("Escape");
      await page.locator("#rail-toggle").click();
      await page.screenshot({
        path: "/tmp/refine-planning-polish-desktop.png",
        fullPage: true,
      });
      await page.evaluate(
        () => (document.documentElement.dataset.theme = "light"),
      );
      await page.screenshot({
        path: "/tmp/refine-planning-polish-light.png",
        fullPage: true,
      });
      await page.setViewportSize({ width: 390, height: 844 });
      assert.equal(
        await page.evaluate(() => document.body.scrollWidth <= innerWidth),
        true,
      );
      await page.screenshot({
        path: "/tmp/refine-planning-polish-mobile.png",
        fullPage: true,
      });
      const scrollBefore = await page.evaluate(() => {
        const lanes = document.querySelector(".planning-lanes");
        lanes.scrollLeft = 320;
        document
          .querySelector('[data-card="DRAFT2"]')
          .focus({ preventScroll: true });
        return lanes.scrollLeft;
      });
      await page.evaluate(() => refreshPlanning());
      assert.equal(
        await page.locator(".planning-lanes").evaluate((el) => el.scrollLeft),
        scrollBefore,
      );
      assert.equal(
        await page.evaluate(() => document.activeElement.dataset.card),
        "DRAFT2",
      );
      await page.locator("#mobile-rail-toggle").click();
      await page.getByTestId("planning-menu").click();
      await page
        .locator('#planning-board-options [data-planning-nav-board="personal"]')
        .click();
      await page.waitForURL(/board=personal/);
      await page.waitForFunction(
        () =>
          document.querySelector(".planning-board-heading h2")?.textContent ===
          "Personal",
      );
      assert.equal(await page.locator("#rail-scrim").isVisible(), false);
      assert.deepEqual(app.pageErrors, []);
    } finally {
      await app.close();
    }
  },
);

test(
  "Add card creates on Enter and offers existing Goals through keyboard typeahead",
  { skip: SKIP },
  async () => {
    const app = await planningApp();
    try {
      const { page } = app;
      await page.locator('[data-planning-add="ideas"]').click();
      const input = page.getByRole("combobox", {
        name: "Card title or existing Goal",
      });
      await input.fill("A new idea");
      await input.press("Enter");
      await page.waitForFunction(
        () => !document.querySelector(".planning-composer"),
      );
      assert.equal(app.requests[0].operation, "card.create");
      assert.equal(app.requests[0].data.name, "A new idea");
      await page.locator('[data-planning-add="ideas"]').click();
      await input.fill("Existing");
      await page
        .getByRole("option")
        .filter({ hasText: "Existing workflow Goal" })
        .waitFor();
      await input.press("ArrowDown");
      await input.press("Enter");
      await page.waitForFunction(
        () => !document.querySelector(".planning-composer"),
      );
      assert.equal(app.requests[1].operation, "card.attach");
      assert.equal(app.requests[1].goal_id, "EXISTING");
      assert.equal(app.requests[1].lane_id, "ideas");
      assert.deepEqual(app.pageErrors, []);
    } finally {
      await app.close();
    }
  },
);

test(
  "Drag and drop preserves identity and inserts before the destination card",
  { skip: SKIP },
  async () => {
    const app = await planningApp();
    try {
      const { page } = app;
      const target = page.locator('[data-card="DRAFT2"]');
      await page
        .locator('[data-card="DRAFT1"]')
        .dragTo(target, { targetPosition: { x: 30, y: 5 } });
      await page.waitForFunction(() =>
        document.querySelector('[data-lane="ready"] [data-card="DRAFT1"]'),
      );
      const move = app.requests.find((r) => r.operation === "card.move");
      assert.equal(move.goal_id, "DRAFT1");
      assert.equal(move.lane_id, "ready");
      assert.equal(move.expected_revision, 1);
      assert.ok(move.data.position < 2048);
      assert.deepEqual(app.pageErrors, []);
    } finally {
      await app.close();
    }
  },
);

test(
  "Draft stays out of workflow navigation and creation while board creation works from the rail",
  { skip: SKIP },
  async () => {
    const app = await planningApp();
    try {
      const { page } = app;
      await selectMain(page, "dashboard");
      await page.locator(".dashboard-status-grid").waitFor();
      assert.equal(await page.getByTestId("workflow-status-draft").count(), 0);
      const dashboardLeft = await page
        .locator(".dashboard-title-row")
        .evaluate((el) => el.getBoundingClientRect().left);
      await page.goto(`${app.origin}/#/features/FEAT1`);
      await page.locator('[data-testid="feature-goal-row"]').waitFor();
      assert.equal(
        await page.locator('[data-feature-goal-row="DRAFT1"]').count(),
        0,
      );
      assert.equal(
        await page.locator('[data-feature-goal-row="EXISTING"]').count(),
        1,
      );
      await page.keyboard.press("Escape");
      await selectMain(page, "goals");
      await page.locator("#goals-table").waitFor();
      assert.equal(
        await page.locator('#goals-status option[value="draft"]').count(),
        0,
      );
      assert.equal(
        await page.getByText("Research onboarding", { exact: true }).count(),
        0,
      );
      await page.getByTestId("planning-menu").click();
      await page.locator("[data-planning-create-board]").click();
      await page
        .getByTestId("hub-modal")
        .locator('input[name="name"]')
        .fill("My new board");
      await page.getByTestId("hub-modal").locator('[type="submit"]').click();
      await page.waitForFunction(
        () =>
          document.querySelector(".planning-board-heading h2")?.textContent ===
          "My new board",
      );
      assert.equal(
        await page
          .locator(".planning-heading")
          .evaluate((el) => el.getBoundingClientRect().left),
        dashboardLeft,
      );
      assert.match(page.url(), /board=new-board/);
      assert.deepEqual(app.pageErrors, []);
    } finally {
      await app.close();
    }
  },
);

test(
  "Existing cards move between lanes from search and stale search replies cannot replace current choices",
  { skip: SKIP },
  async () => {
    const app = await planningApp();
    try {
      const { page } = app;
      await page.locator('[data-planning-add="ready"]').click();
      const input = page.getByRole("combobox", {
        name: "Card title or existing Goal",
      });
      await input.fill("Research");
      await page
        .getByRole("option")
        .filter({ hasText: "Research onboarding" })
        .waitFor();
      await input.press("ArrowDown");
      await input.press("Enter");
      await page.waitForFunction(
        () => !document.querySelector(".planning-composer"),
      );
      assert.equal(app.requests[0].operation, "card.move");
      assert.equal(app.requests[0].goal_id, "DRAFT1");
      assert.equal(app.requests[0].expected_revision, 1);
      await page.locator('[data-planning-add="ideas"]').click();
      await page.evaluate(() => {
        const original = api;
        window.api = async (method, path, ...args) => {
          if (path.includes("q=Old")) {
            await new Promise((resolve) => setTimeout(resolve, 500));
            return {
              goals: [{ id: "OLD", name: "Old reply", status: "backlog" }],
            };
          }
          return original(method, path, ...args);
        };
      });
      await input.fill("Old");
      await page.waitForTimeout(220);
      await input.fill("Existing");
      await page
        .getByRole("option")
        .filter({ hasText: "Existing workflow Goal" })
        .waitFor();
      await page.waitForTimeout(550);
      assert.equal(
        await page.getByRole("option").filter({ hasText: "Old reply" }).count(),
        0,
      );
      await page.evaluate(() => {
        const original = api;
        window.api = async (method, ...args) => {
          if (method === "POST") throw new Error("Node unavailable; try again");
          return original(method, ...args);
        };
      });
      await input.press("Enter");
      await page
        .locator("[data-composer-status]")
        .filter({ hasText: "Node unavailable" })
        .waitFor();
      assert.equal(await input.inputValue(), "Existing");
      assert.equal(await input.isEnabled(), true);
      assert.deepEqual(app.pageErrors, []);
    } finally {
      await app.close();
    }
  },
);

test(
  "Archived boards remain reachable from the Planning dropdown",
  { skip: SKIP },
  async () => {
    const app = await planningApp();
    try {
      app.snapshot.boards[1].archived = true;
      await app.page.locator("[data-planning-refresh]").click();
      await app.page.waitForFunction(
        () =>
          !document.querySelector(
            '#planning-board-options [data-planning-nav-board="personal"]',
          ),
      );
      await app.page.getByRole("switch", { name: "Show archived" }).click();
      await app.page
        .locator('#planning-board-options [data-planning-nav-board="personal"]')
        .waitFor({ state: "attached" });
      await app.page.getByTestId("planning-menu").click();
      await app.page
        .locator('#planning-board-options [data-planning-nav-board="personal"]')
        .click();
      await app.page.waitForURL(/board=personal/);
      await app.page.waitForFunction(
        () =>
          document.querySelector(".planning-board-heading h2")?.textContent ===
          "Personal",
      );
      assert.deepEqual(app.pageErrors, []);
    } finally {
      await app.close();
    }
  },
);

test(
  "Board views close without deleting data and reopen from the Planning menu",
  { skip: SKIP },
  async () => {
    const app = await planningApp();
    try {
      const { page } = app;
      await page.locator('[data-close-planning-board="board"]').click();
      await page.waitForURL(/#\/$/);
      assert.equal(app.requests.length, 0);
      assert.equal(app.snapshot.boards.length, 2);
      await page.reload();
      await page.locator("#dash").waitFor();
      assert.equal(
        await page.locator('[data-close-planning-board="board"]').count(),
        0,
      );
      await page.getByTestId("planning-menu").click();
      await page
        .locator('#planning-board-options [data-planning-nav-board="board"]')
        .click();
      await page.locator('[data-close-planning-board="board"]').waitFor();
      assert.equal(await page.locator('[data-card="DRAFT1"]').count(), 1);
      await page.getByTestId("toolbar-add").click();
      await page.locator('[data-add-toolbar-tab="files"]').click();
      await page.waitForURL(/#\/windows\//);
      await page.locator('[data-close-planning-board="board"]').click();
      await page.getByTestId("toolbar-tab-close").click();
      await page.waitForURL(/#\/$/);
      await page.locator("#dash").waitFor();
      assert.equal(
        await page.locator('[data-close-planning-board="board"]').count(),
        0,
      );
      assert.deepEqual(app.pageErrors, []);
    } finally {
      await app.close();
    }
  },
);

test(
  "Board deletion confirms retained Goals and removes the board and open view",
  { skip: SKIP },
  async () => {
    const app = await planningApp();
    try {
      const { page } = app;
      await page.locator("[data-planning-board-settings]").click();
      await page.locator("[data-delete-board]").click();
      const confirm = page.locator(".modal-backdrop").last();
      assert.match(await confirm.innerText(), /underlying Goals will be kept/);
      await confirm
        .getByRole("button", { name: "Cancel", exact: true })
        .click();
      assert.equal(app.requests.length, 0);
      await page.locator("[data-delete-board]").click();
      await page
        .locator(".modal-backdrop")
        .last()
        .getByRole("button", { name: "Delete board", exact: true })
        .click();
      await page.waitForURL(/#\/$/);
      assert.equal(app.requests[0].operation, "board.delete");
      assert.equal(app.requests[0].expected_revision, 1);
      assert.equal(
        app.snapshot.boards.some((board) => board.id === "board"),
        false,
      );
      assert.equal(
        await page.locator('[data-close-planning-board="board"]').count(),
        0,
      );
      assert.deepEqual(app.pageErrors, []);
    } finally {
      await app.close();
    }
  },
);

for (const theme of ["light", "dark"]) {
  test(
    `Planning card titles and shared menus keep white text on blue hover and focus (${theme})`,
    { skip: SKIP },
    async () => {
      const app = await planningApp();
      try {
        const { page } = app;
        app.snapshot.cards[1].goal.status = "backlog";
        await page.reload();
        await page.locator(".planning-card-title[href]").waitFor();
        await page.evaluate(
          (theme) => (document.documentElement.dataset.theme = theme),
          theme,
        );
        const assertHighlight = async (locator, label) => {
          const colors = await locator.evaluate((el) => {
            const style = getComputedStyle(el);
            const probe = document.createElement("span");
            probe.style.color = "var(--color-primary-hover)";
            el.append(probe);
            const blue = getComputedStyle(probe).color;
            probe.remove();
            return {
              background: style.backgroundColor,
              foreground: style.color,
              blue,
            };
          });
          assert.equal(
            colors.background,
            colors.blue,
            `${label}: blue background`,
          );
          assert.equal(
            colors.foreground,
            "rgb(255, 255, 255)",
            `${label}: white foreground`,
          );
        };
        const checkStates = async (locator, label) => {
          await locator.hover();
          await assertHighlight(locator, `${label} hover`);
          await page.mouse.move(0, 0);
          await locator.focus();
          await page.keyboard.press("Tab");
          await page.keyboard.press("Shift+Tab");
          assert.equal(
            await locator.evaluate((el) => el.matches(":focus-visible")),
            true,
          );
          await assertHighlight(locator, `${label} keyboard focus`);
          await locator.evaluate((el) => el.blur());
        };
        await checkStates(
          page.locator("button.planning-card-title"),
          "Draft title",
        );
        await checkStates(page.locator("a.planning-card-title"), "Goal title");
        await page.getByTestId("planning-menu").click();
        await checkStates(
          page.locator("[data-planning-create-board]"),
          "Create board",
        );
        await checkStates(
          page.locator("#planning-board-options a").first(),
          "Board menu item",
        );
        await page.keyboard.press("Escape");
        await page.getByTestId("toolbar-add").click();
        await checkStates(
          page.locator('[data-add-toolbar-tab="files"]'),
          "Tool menu item",
        );
        assert.deepEqual(app.pageErrors, []);
      } finally {
        await app.close();
      }
    },
  );
}

for (const mobile of [false, true]) {
  test(
    `Lane settings use one footer and save position with settings (${mobile ? "mobile" : "desktop"})`,
    { skip: SKIP },
    async () => {
      const app = await planningApp();
      try {
        const { page } = app;
        await page.setViewportSize(
          mobile ? { width: 390, height: 844 } : { width: 1280, height: 900 },
        );
        await page.locator('[data-lane-settings="ideas"]').click();
        const editor = page.locator(".planning-lane-editor");
        assert.equal(
          await editor.locator('.modal-body button[type="submit"]').count(),
          0,
        );
        assert.equal(await editor.locator("[data-delete]").isDisabled(), true);
        assert.deepEqual(
          await editor
            .locator(".modal-actions button:visible")
            .allTextContents(),
          ["Delete lane", "Cancel", "Save"],
        );
        await editor.getByLabel("Name", { exact: true }).fill("Discovery");
        await editor
          .getByLabel("Position", { exact: true })
          .selectOption({ value: "2" });
        // Opening a related Skill editor must not discard unsaved lane fields.
        await editor
          .getByRole("button", { name: "Add Skill", exact: true })
          .click();
        const skill = page.getByRole("dialog", {
          name: "New Skill",
          exact: true,
        });
        await skill.waitFor();
        assert.equal(
          await skill.locator("[data-trigger-source]").inputValue(),
          "planning.lane.enter",
        );
        assert.equal(
          await skill.locator("[data-planning-lane-filter]").inputValue(),
          "ideas",
        );
        await skill.locator("[data-close]").click();
        assert.equal(
          await editor.getByLabel("Name", { exact: true }).inputValue(),
          "Discovery",
        );
        assert.equal(
          await editor.getByLabel("Position", { exact: true }).inputValue(),
          "2",
        );
        assert.equal(app.requests.length, 0);
        const geometry = await editor
          .locator(".modal-actions button:visible")
          .evaluateAll((buttons) =>
            buttons.map((button) => {
              const box = button.getBoundingClientRect();
              return { top: box.top, right: box.right, left: box.left };
            }),
          );
        assert.equal(new Set(geometry.map((box) => box.top)).size, 1);
        assert.ok(
          geometry.every(
            (box) => box.left >= 0 && box.right <= (mobile ? 390 : 1280),
          ),
        );
        await page.screenshot({
          path: `/tmp/refine-lane-settings-${mobile ? "mobile" : "desktop"}.png`,
        });
        await editor.getByRole("button", { name: "Save", exact: true }).click();
        await editor.waitFor({ state: "detached" });
        assert.equal(app.requests.length, 1);
        assert.equal(app.requests[0].operation, "lane.update");
        assert.equal(app.requests[0].data.position, 2);
        assert.equal(app.requests[0].data.name, "Discovery");
        await page.waitForFunction(
          () =>
            document.querySelector(".planning-lanes")?.lastElementChild?.dataset
              .lane === "ideas",
        );
        assert.deepEqual(
          app.snapshot.boards[0].lanes.map((lane) => lane.id),
          ["ready", "done", "ideas"],
        );
        await page.locator('[data-lane-settings="done"]').click();
        await editor
          .getByRole("button", { name: "Delete lane", exact: true })
          .click();
        const confirmation = page.getByRole("alertdialog", {
          name: "Delete lane",
          exact: true,
        });
        await confirmation
          .getByRole("button", { name: "Cancel", exact: true })
          .click();
        assert.equal(app.requests.length, 1);
        await editor
          .getByRole("button", { name: "Delete lane", exact: true })
          .click();
        await confirmation
          .getByRole("button", { name: "Delete lane", exact: true })
          .click();
        await editor.waitFor({ state: "detached" });
        assert.equal(app.requests.at(-1).operation, "lane.delete");
        assert.deepEqual(app.pageErrors, []);
      } finally {
        await app.close();
      }
    },
  );
}

test(
  "Lane settings keep unsaved fields when a board revision conflicts",
  { skip: SKIP },
  async () => {
    const app = await planningApp();
    try {
      const { page } = app;
      await page.locator('[data-lane-settings="ideas"]').click();
      const editor = page.locator(".planning-lane-editor");
      await editor.getByLabel("Name", { exact: true }).fill("Keep this draft");
      await editor
        .getByLabel("Position", { exact: true })
        .selectOption({ value: "2" });
      app.snapshot.boards[0].revision++;
      await editor.getByRole("button", { name: "Save", exact: true }).click();
      await editor
        .getByRole("alert")
        .filter({ hasText: "Board changed" })
        .waitFor();
      assert.equal(
        await editor.getByLabel("Name", { exact: true }).inputValue(),
        "Keep this draft",
      );
      assert.equal(
        await editor.getByLabel("Position", { exact: true }).inputValue(),
        "2",
      );
      assert.equal(
        await editor
          .getByRole("button", { name: "Save", exact: true })
          .isEnabled(),
        true,
      );
      assert.equal(app.snapshot.boards[0].lanes[0].id, "ideas");
      assert.deepEqual(app.pageErrors, []);
    } finally {
      await app.close();
    }
  },
);

test(
  "Deleting a Draft card confirms removal, preserves other cards, and hides Delete for workflow Goals",
  { skip: SKIP },
  async () => {
    const app = await planningApp();
    try {
      const { page } = app;
      app.snapshot.cards[1].goal.status = "backlog";
      await page.reload();
      await page.locator('[data-card="DRAFT1"]').waitFor();
      assert.equal(
        await page.locator('[data-card="DRAFT2"] [data-card-delete]').count(),
        0,
      );
      const remove = page.locator('[data-card-delete="DRAFT1"]');
      await remove.click();
      const confirm = page.getByRole("alertdialog", {
        name: "Delete Draft Goal",
        exact: true,
      });
      assert.match(
        await confirm.textContent(),
        /Research onboarding.*cannot be undone/s,
      );
      assert.equal(
        await confirm
          .getByRole("button", { name: "Cancel", exact: true })
          .evaluate((el) => el === document.activeElement),
        true,
      );
      await confirm
        .getByRole("button", { name: "Cancel", exact: true })
        .click();
      assert.equal(app.requests.length, 0);
      assert.equal(await page.locator('[data-card="DRAFT1"]').count(), 1);
      await remove.click();
      await confirm
        .getByRole("button", { name: "Delete", exact: true })
        .click();
      await page.locator('[data-card="DRAFT1"]').waitFor({ state: "detached" });
      assert.equal(app.requests.length, 1);
      assert.equal(app.requests[0].operation, "card.delete");
      assert.equal(app.requests[0].expected_revision, 1);
      assert.equal(app.requests[0].data.expected_goal_revision, 1);
      assert.equal(await page.locator('[data-card="DRAFT2"]').count(), 1);
      assert.deepEqual(app.pageErrors, []);
    } finally {
      await app.close();
    }
  },
);

test(
  "A rejected Draft deletion leaves the card visible and reports the conflict",
  { skip: SKIP },
  async () => {
    const app = await planningApp();
    try {
      const { page } = app;
      await page.locator('[data-card-delete="DRAFT1"]').click();
      app.snapshot.cards[0].goal.status = "backlog";
      await page
        .getByRole("alertdialog", { name: "Delete Draft Goal", exact: true })
        .getByRole("button", { name: "Delete", exact: true })
        .click();
      await page
        .getByText("Goal changed; refresh before deleting", { exact: true })
        .waitFor();
      assert.equal(app.snapshot.cards.length, 2);
      assert.equal(await page.locator('[data-card="DRAFT1"]').count(), 1);
      assert.deepEqual(app.pageErrors, []);
    } finally {
      await app.close();
    }
  },
);
