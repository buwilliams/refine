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
            '#planning-board-options [data-planning-nav-board="personal"] .rail-copy',
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
      await page.locator('[data-testid="nav-dashboard"]').click();
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
      await page.locator('[data-testid="nav-goals"]').click();
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
