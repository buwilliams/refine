import { expect, test, type Page } from "@playwright/test";
import { ensureAttachedProject, jsonObject } from "./helpers";

async function fillChatInputStable(page: Page, text: string) {
  const input = page.getByTestId("chat-input");
  await expect.poll(async () => {
    await input.fill(text);
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => {
      requestAnimationFrame(resolve);
    })));
    return input.inputValue();
  }).toBe(text);
}

test("runs standalone chat controls through Smoke AI", async ({ page, request }) => {
  test.setTimeout(60_000);
  await ensureAttachedProject(request);
  await page.addInitScript(() => {
    localStorage.removeItem("refine_chat_tabs");
  });
  await page.goto("/");

  await page.getByTestId("toolbar-tab-standalone").click();
  await expect(page.getByTestId("chat-input")).toBeVisible();
  await expect(page.getByTestId("chat-status")).toContainText("No active session");

  await page.getByTestId("chat-toggle").click();
  await expect(page.getByTestId("chat-status")).toContainText("active");
  await expect(page.getByTestId("chat-toggle")).toHaveText("Stop standalone");

  await fillChatInputStable(page, "Start a standalone chat conversation for Smoke AI.");
  const chatInputQueued = page.waitForRequest((request) =>
    /\/api\/chat\/[^/]+\/input$/.test(new URL(request.url()).pathname) &&
    request.method() === "POST"
  );
  await page.getByTestId("chat-send").click();
  await chatInputQueued;
  await expect(page.getByTestId("chat-output")).toContainText("smoke-ai chat response", { timeout: 45_000 });
  await expect(page.getByTestId("chat-activity-toggle")).toBeVisible();

  await page.getByTestId("chat-activity-toggle").click();
  await expect(page.getByTestId("chat-activity-toggle")).toHaveAttribute("aria-expanded", "false");

  await page.getByTestId("chat-toggle").click();
  await expect(page.getByTestId("chat-status")).toContainText("No active session");

  await page.getByTestId("chat-clear").click();
  await page.getByRole("button", { name: "Clear", exact: true }).click();
  await expect(page.getByTestId("chat-output")).toHaveText("");
  await expect(page.getByTestId("chat-status")).toContainText("No active session");
});

test("edits and removes standalone queued chat messages", async ({ page, request }) => {
  test.setTimeout(60_000);
  await ensureAttachedProject(request);
  await page.addInitScript(() => {
    localStorage.removeItem("refine_chat_tabs");
  });
  await page.goto("/");

  await page.getByTestId("toolbar-tab-standalone").click();
  await expect(page.getByTestId("chat-input")).toBeVisible();
  await page.getByTestId("chat-toggle").click();
  await expect(page.getByTestId("chat-status")).toContainText("active");

  await fillChatInputStable(page, "Start a smoke-ai queue delay chat turn.");
  const firstInput = page.waitForResponse((response) =>
    /\/api\/chat\/[^/]+\/input$/.test(new URL(response.url()).pathname) &&
    response.request().method() === "POST" &&
    response.status() === 200
  );
  await page.getByTestId("chat-send").click();
  await firstInput;
  await expect(page.getByTestId("chat-queue")).toHaveCount(0);

  await fillChatInputStable(page, "Queued standalone message before edit.");
  const queuedInput = page.waitForResponse((response) =>
    /\/api\/chat\/[^/]+\/input$/.test(new URL(response.url()).pathname) &&
    response.request().method() === "POST" &&
    response.status() === 200
  );
  await page.getByTestId("chat-send").click();
  const queuedPayload = await (await queuedInput).json();
  expect(queuedPayload.queued_messages).toEqual(expect.arrayContaining([
    expect.objectContaining({ text: "Queued standalone message before edit." }),
  ]));

  const queueItem = page.getByTestId("chat-queue-item").filter({ hasText: "Queued standalone message before edit." });
  await expect(queueItem).toBeVisible();
  await expect(page.getByTestId("chat-queue-count")).toHaveText("1");

  await queueItem.getByTestId("chat-queue-text").fill("Edited standalone queued message.");
  const savedQueue = page.waitForResponse((response) =>
    /\/api\/chat\/[^/]+\/queue\/[^/]+$/.test(new URL(response.url()).pathname) &&
    response.request().method() === "PATCH" &&
    response.status() === 200
  );
  await queueItem.getByTestId("chat-queue-save").click();
  const savedPayload = await (await savedQueue).json();
  expect(savedPayload.queued_messages).toEqual(expect.arrayContaining([
    expect.objectContaining({ text: "Edited standalone queued message." }),
  ]));
  await expect(page.getByTestId("chat-queue-item")).toContainText("Edited standalone queued message.");

  const removedQueue = page.waitForResponse((response) =>
    /\/api\/chat\/[^/]+\/queue\/[^/]+$/.test(new URL(response.url()).pathname) &&
    response.request().method() === "DELETE" &&
    response.status() === 200
  );
  await page.getByTestId("chat-queue-remove").click();
  const removedPayload = await (await removedQueue).json();
  expect(removedPayload.queued_messages).toEqual([]);
  await expect(page.getByTestId("chat-queue")).toHaveCount(0);

  await expect(page.getByTestId("chat-output")).toContainText("smoke-ai chat response", { timeout: 45_000 });
  await page.getByTestId("chat-clear").click();
  await expect(page.getByTestId("modal-dialog")).toContainText("Clear chat history");
  await page.getByTestId("modal-ok").click();
  await expect(page.getByTestId("chat-output")).toHaveText("");
  await expect(page.getByTestId("chat-status")).toContainText("No active session");
});

test("opens Gap chat and drafts a round from a Smoke AI turn", async ({ page, request }) => {
  test.setTimeout(60_000);
  await ensureAttachedProject(request);
  const created = await jsonObject(await request.post("/api/gaps", {
    data: {
      reporter: "refine-smoke",
      actual: "Gap chat actual behavior",
      target: "Gap chat target behavior",
      priority: "low",
    },
  }));
  const gapId = String((created.gap as { id?: string } | undefined)?.id ?? "");
  expect(gapId).toBeTruthy();

  try {
    await page.goto("/");
    await page.getByTestId("context-menu-toggle").click();
    await page.getByTestId("global-reporter").selectOption("refine-smoke");
    await page.goto(`/#/gaps/${gapId}`);
    await expect(page.getByTestId("gap-detail")).toBeVisible();

    await page.getByTestId("gap-open-chat").click();
    await expect(page.getByTestId(`toolbar-tab-${gapId}`)).toHaveClass(/active/);
    await expect(page.getByTestId(`toolbar-tab-${gapId}`).getByTestId("toolbar-tab-dot")).toBeVisible();
    await expect(page.getByTestId(`toolbar-tab-${gapId}`).getByTestId("toolbar-tab-close")).toBeVisible();
    await expect(page.getByTestId("chat-gap-link")).toContainText(gapId.slice(0, 10));
    await expect(page.getByTestId("chat-status")).toContainText("active");

    await page.getByTestId("chat-input").fill("Run a Gap chat draft round conversation for this deterministic defect.");
    await page.getByTestId("chat-send").click();
    await expect(page.getByTestId("chat-output")).toContainText("smoke-ai round actual behavior", { timeout: 45_000 });
    await expect(page.getByTestId("chat-activity-toggle")).toBeVisible();
    await expect(page.getByTestId("chat-activity-toggle")).toHaveAttribute("aria-expanded", "true");
    await expect(page.getByTestId("chat-progress-panel")).toBeVisible();
    await page.getByTestId("chat-activity-toggle").click();
    await expect(page.getByTestId("chat-activity-toggle")).toHaveAttribute("aria-expanded", "false");
    await expect(page.getByTestId("chat-progress-panel")).toBeHidden();
    await page.getByTestId("chat-activity-toggle").click();
    await expect(page.getByTestId("chat-activity-toggle")).toHaveAttribute("aria-expanded", "true");
    await expect(page.getByTestId("chat-progress-panel")).toBeVisible();
    await expect(page.getByTestId("gap-draft-round")).toBeEnabled();

    await page.getByTestId("gap-draft-round").click();
    await expect(page.getByTestId("gap-round-extract-modal")).toBeVisible();
    await expect(page.getByTestId("gap-round-extract-actual")).toHaveValue(/smoke-ai round actual behavior/);
    await expect(page.getByTestId("gap-round-extract-target")).toHaveValue(/smoke-ai round target behavior/);
    await expect(page.getByTestId("gap-round-extract-submit")).toBeEnabled();
    await page.getByTestId("gap-round-extract-submit").click();
    await expect(page.getByTestId("gap-round-extract-modal")).toHaveCount(0);

    await expect.poll(async () => {
      const gap = await jsonObject(await request.get(`/api/gaps/${gapId}`));
      return (gap.gap as { round_count?: number } | undefined)?.round_count ?? 0;
    }).toBe(2);
    const gap = await jsonObject(await request.get(`/api/gaps/${gapId}`));
    const rounds = (gap.gap as { rounds?: Array<{ actual?: string }> } | undefined)?.rounds ?? [];
    expect(rounds.some((round) => (round.actual ?? "").includes("smoke-ai round actual behavior"))).toBeTruthy();

    if ((await page.getByTestId(`toolbar-tab-${gapId}`).count()) > 0) {
      await page.getByTestId(`toolbar-tab-${gapId}`).getByTestId("toolbar-tab-close").click();
      await expect(page.getByTestId(`toolbar-tab-${gapId}`)).toHaveCount(0);
      await expect(page.getByTestId("toolbar-tab-standalone")).toHaveClass(/active/);
    }
  } finally {
    await request.delete(`/api/gaps/${gapId}`);
  }
});
