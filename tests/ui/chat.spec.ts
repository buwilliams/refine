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

async function selectReporter(page: Page) {
  await page.getByTestId("context-menu-toggle").click();
  await expect(page.getByTestId("global-reporter").locator("option", { hasText: "refine-smoke" })).toHaveCount(1);
  await page.getByTestId("global-reporter").selectOption("refine-smoke");
}

async function startStandaloneChat(page: Page): Promise<string> {
  await page.getByTestId("chat-toggle").click();
  const status = page.getByTestId("chat-status");
  await expect(status).toContainText(/Session .+ active/);
  const statusText = await status.textContent();
  const sessionId = statusText?.match(/Session\s+(\S+)\s+active/)?.[1] ?? "";
  expect(sessionId).toBeTruthy();
  return sessionId;
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

  await startStandaloneChat(page);
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

test("drafts a Gap from standalone chat context through Smoke AI", async ({ page, request }) => {
  test.setTimeout(120_000);
  await ensureAttachedProject(request);
  let gapId = "";

  try {
    await page.addInitScript(() => {
      localStorage.removeItem("refine_chat_tabs");
    });
    await page.goto("/");
    await selectReporter(page);

    await page.getByTestId("toolbar-tab-standalone").click();
    await expect(page.getByTestId("standalone-draft-gap")).toBeVisible();
    await expect(page.getByTestId("standalone-draft-gap")).toBeDisabled();
    const sessionId = await startStandaloneChat(page);
    await jsonObject(await request.post(`/api/chat/${encodeURIComponent(sessionId)}/input`, {
      data: { text: "Start a standalone chat conversation before drafting a Gap." },
    }));
    await expect(page.getByTestId("chat-output")).toContainText("smoke-ai chat response", { timeout: 45_000 });
    await expect(page.getByTestId("standalone-draft-gap")).toBeEnabled();

    const extractResponse = page.waitForResponse((response) =>
      response.url().includes("/api/import/extract") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("standalone-draft-gap").click();
    await expect(page.getByTestId("standalone-gap-draft-modal")).toBeVisible();
    const extractPayload = await (await extractResponse).json();
    expect(extractPayload.purpose).toBe("standalone_gap");
    expect(extractPayload.provider).toBe("smoke-ai");
    await expect(page.getByTestId("standalone-gap-draft-actual")).toHaveValue(/smoke-ai standalone actual behavior/);
    await expect(page.getByTestId("standalone-gap-draft-target")).toHaveValue(/smoke-ai standalone target behavior/);
    await expect(page.getByTestId("standalone-gap-draft-priority")).toHaveValue("low");

    const createResponse = page.waitForResponse((response) =>
      response.url().includes("/api/gaps") &&
      response.request().method() === "POST" &&
      response.status() === 201
    );
    await page.getByTestId("standalone-gap-draft-submit").click();
    const createPayload = await (await createResponse).json();
    gapId = String(createPayload.gap?.id ?? "");
    expect(gapId).toBeTruthy();
    await expect(page.getByTestId("standalone-gap-draft-modal")).toHaveCount(0);
    await expect(page).toHaveURL(new RegExp(`#\\/gaps\\/${gapId}`));

    const gap = await jsonObject(await request.get(`/api/gaps/${gapId}`));
    const rounds = (gap.gap as { rounds?: Array<{ actual?: string; target?: string }> } | undefined)?.rounds ?? [];
    expect(rounds.some((round) =>
      (round.actual ?? "").includes("smoke-ai standalone actual behavior") &&
      (round.target ?? "").includes("smoke-ai standalone target behavior")
    )).toBeTruthy();
    expect((gap.gap as { feature_id?: string | null } | undefined)?.feature_id ?? null).toBeNull();
  } finally {
    if (gapId) await request.delete(`/api/gaps/${gapId}`);
  }
});

test("does not duplicate transcript lines when SSE chat events race redraws", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const sessionId = "ui-sse-race";
  const prompt = "Unique duplicate guard prompt.";
  const response = "Unique duplicate guard response.";
  let inputPosts = 0;
  let readRequests = 0;

  await page.route("**/api/chat/start", async (route) => {
    await route.fulfill({
      status: 201,
      contentType: "application/json",
      body: JSON.stringify({
        session_id: sessionId,
        mode: "standalone",
        provider: "mock",
      }),
    });
  });
  await page.route(`**/api/chat/${sessionId}/input`, async (route) => {
    inputPosts += 1;
    const body = route.request().postDataJSON() as { text?: string };
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        alive: true,
        session_id: sessionId,
        queued_messages: [{
          id: "queued-one",
          text: body.text ?? "",
          created_at: new Date().toISOString(),
          updated_at: new Date().toISOString(),
        }],
        importable_artifacts: [],
        in_flight: true,
        provider_session_id: null,
      }),
    });
  });
  await page.route(`**/api/chat/${sessionId}/read`, async (route) => {
    readRequests += 1;
    await route.fulfill({
      status: 500,
      contentType: "application/json",
      body: JSON.stringify({ error: { message: "chat read should not be called" } }),
    });
  });

  await page.addInitScript(() => {
    localStorage.removeItem("refine_chat_tabs");
  });
  await page.goto("/");

  await page.getByTestId("toolbar-tab-standalone").click();
  await startStandaloneChat(page);

  await fillChatInputStable(page, prompt);
  await page.getByTestId("chat-send").click();
  await page.evaluate(() => {
    (window as any).drawToolbar?.();
    (window as any).drawToolbar?.();
  });
  await page.evaluate(({ id, promptText, responseText }) => {
    const emit = (event: Record<string, unknown>, inFlight: boolean) => {
      (window as any).handleChatSseEvent?.({
        session_id: id,
        mode: "standalone",
        provider: "mock",
        in_flight: inFlight,
        closed: false,
        event,
      });
    };
    emit({
      id: "event-user",
      role: "user",
      text: promptText,
      progress: false,
      created_at: new Date().toISOString(),
    }, true);
    (window as any).drawToolbar?.();
    emit({
      id: "event-assistant",
      role: "assistant",
      text: responseText,
      progress: false,
      created_at: new Date().toISOString(),
    }, false);
    emit({
      id: "event-assistant",
      role: "assistant",
      text: responseText,
      progress: false,
      created_at: new Date().toISOString(),
    }, false);
  }, { id: sessionId, promptText: prompt, responseText: response });

  await expect(page.getByTestId("chat-output")).toContainText(response);
  await page.waitForTimeout(250);
  const transcript = await page.getByTestId("chat-output").textContent();
  expect(transcript?.match(/Unique duplicate guard prompt\./g) ?? []).toHaveLength(1);
  expect(transcript?.match(/Unique duplicate guard response\./g) ?? []).toHaveLength(1);
  expect(inputPosts).toBe(1);
  expect(readRequests).toBe(0);
});

test("renders persisted active chat from SSE without read requests", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const sessionId = "ui-sse-only-chat";
  let readRequests = 0;

  await page.addInitScript((id) => {
    localStorage.setItem("refine_chat_tabs", JSON.stringify({
      activeTabId: "standalone",
      open: true,
      tabs: {
        standalone: {
          gapId: null,
          label: "Standalone",
          mode: "standalone",
          sessionId: id,
          output: "",
          progress: "",
          showProgress: true,
          closedReason: null,
          agentResponded: false,
          sentUserInput: false,
          queuedMessages: [],
          localQueuedMessages: [],
          starting: false,
        },
      },
    }));
    class MockEventSource {
      url: string;
      onerror: (() => void) | null = null;
      constructor(url: string) {
        this.url = url;
      }
      addEventListener() {}
      close() {}
    }
    (window as any).EventSource = MockEventSource;
  }, sessionId);

  await page.route(`**/api/chat/${sessionId}/read`, async (route) => {
    readRequests += 1;
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        alive: true,
        session_id: sessionId,
        lines: readRequests === 1 ? ["SSE delivered chat line."] : [],
        progress_lines: [],
        queued_messages: [],
        importable_artifacts: [],
        in_flight: false,
        provider_session_id: null,
      }),
    });
  });

  await page.goto("/");
  await expect(page.getByTestId("chat-status")).toContainText(`Session ${sessionId} active`);
  await page.waitForTimeout(1200);
  expect(readRequests).toBe(0);

  await page.evaluate((id) => {
    (window as any).handleChatSseEvent?.({
      session_id: id,
      mode: "standalone",
      provider: "mock",
      in_flight: false,
      closed: false,
      event: {
        id: "sse-line-one",
        role: "assistant",
        text: "SSE delivered chat line.",
        progress: false,
        created_at: new Date().toISOString(),
      },
    });
  }, sessionId);
  await expect(page.getByTestId("chat-output")).toContainText("SSE delivered chat line.");
  expect(readRequests).toBe(0);
});

test("edits and removes standalone queued chat messages", async ({ page, request }) => {
  test.setTimeout(120_000);
  await ensureAttachedProject(request);
  await page.addInitScript(() => {
    localStorage.removeItem("refine_chat_tabs");
  });
  await page.goto("/");

  await page.getByTestId("toolbar-tab-standalone").click();
  await expect(page.getByTestId("chat-input")).toBeVisible();
  const sessionId = await startStandaloneChat(page);
  await jsonObject(await request.post(`/api/chat/${encodeURIComponent(sessionId)}/input`, {
    data: { text: "Start a smoke-ai queue delay chat turn." },
  }));
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
