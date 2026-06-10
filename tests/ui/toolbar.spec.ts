import fs from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { attachProject, ensureAttachedProject, jsonObject, projectStatus } from "./helpers";

function testAppRoot(): string {
  return process.env.REFINE_TEST_APP_ROOT ||
    path.join(process.cwd(), "target/refine-integration/apps/rust-test-app");
}

test("browses target app files from the toolbar", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const searchPrefix = `toolbar-search-${Date.now()}`;
  const filePrefix = `toolbar-file-${Date.now()}`;
  const firstSearchFile = `${searchPrefix}-a.txt`;
  const secondSearchFile = `${searchPrefix}-b.txt`;
  const largeFile = `${filePrefix}-large.txt`;
  const imageFile = `${filePrefix}-pixel.png`;
  const binaryFile = `${filePrefix}-artifact.bin`;
  const depthDir = `${filePrefix}-depth`;
  const wideDir = `${filePrefix}-wide`;
  const largeTail = "toolbar chunk tail marker";
  const largeContent = `${Array.from({ length: 2600 }, (_, index) =>
    `toolbar chunk ${String(index).padStart(4, "0")} ${"x".repeat(48)}`,
  ).join("\n")}\n${largeTail}\n`;
  const pngPixel = Buffer.from(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p9sAAAAASUVORK5CYII=",
    "base64",
  );
  fs.writeFileSync(path.join(testAppRoot(), firstSearchFile), "first toolbar search fixture\n");
  fs.writeFileSync(path.join(testAppRoot(), secondSearchFile), "second toolbar search fixture\n");
  fs.writeFileSync(path.join(testAppRoot(), largeFile), largeContent);
  fs.writeFileSync(path.join(testAppRoot(), imageFile), pngPixel);
  fs.writeFileSync(path.join(testAppRoot(), binaryFile), Buffer.from([0, 1, 2, 3]));
  fs.mkdirSync(path.join(testAppRoot(), depthDir, "one", "two", "three", "four"), { recursive: true });
  fs.writeFileSync(path.join(testAppRoot(), depthDir, "one", "two", "three", "four", "leaf.txt"), "depth leaf\n");
  fs.mkdirSync(path.join(testAppRoot(), wideDir), { recursive: true });
  for (let index = 0; index < 205; index += 1) {
    fs.writeFileSync(
      path.join(testAppRoot(), wideDir, `entry-${String(index).padStart(3, "0")}.txt`),
      `wide entry ${index}\n`,
    );
  }

  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: async (text: string) => {
          (window as any).__refineClipboardText = text;
        },
      },
    });
  });

  await page.goto("/");

  await page.getByTestId("toolbar-tab-files").click();
  await expect(page.getByTestId("toolbar-dock")).toHaveClass(/open/);
  await expect(page.getByTestId("toolbar-files-panel")).toBeVisible();
  await expect(page.getByTestId("files-tree")).toContainText("README.md");
  await expect(page.getByTestId("files-tree")).toContainText("app.py");

  await page.getByTestId("files-expand-all").click();
  await expect(page.getByTestId("files-tree")).toContainText("README.md");
  await page.getByTestId("files-collapse-all").click();
  await expect(page.getByTestId("files-tree")).toContainText("app.py");
  await page.getByTestId("files-clear-tree").click();
  await expect(page.getByTestId("files-tree")).toContainText("README.md");

  await page.getByTestId("files-path-input").fill(depthDir);
  await page.getByTestId("files-go-path").click();
  await expect(page.getByTestId("files-tree")).toContainText("one");
  await page.getByTestId("files-expand-all").click();
  await expect(page.getByTestId("files-tree")).toContainText("three");
  await expect(page.getByTestId("files-tree")).toContainText("Tree depth limit reached.");

  await page.getByTestId("files-path-input").fill(wideDir);
  await page.getByTestId("files-go-path").click();
  await page.getByTestId("files-expand-all").click();
  await expect(page.getByTestId("files-tree")).toContainText("entry-199.txt");
  await expect(page.getByTestId("files-tree")).toContainText("Showing first 200 entries.");

  await page.getByTestId("files-path-input").fill("README.md");
  await page.getByTestId("files-go-path").click();
  await expect(page.getByTestId("files-status")).toHaveText("README.md");
  await expect(page.getByTestId("files-source")).toContainText("Disposable target app");
  await expect(page.getByTestId("files-source-line").first()).toContainText("# Refine rust smoke target app");
  await page.getByTestId("files-copy-path").click();
  await expect(page.getByTestId("toast").filter({ hasText: "Path copied" })).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as any).__refineClipboardText)).toBe("README.md");
  await page.getByTestId("files-copy-content").click();
  await expect(page.getByTestId("toast").filter({ hasText: "File contents copied" })).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as any).__refineClipboardText)).toContain("Disposable target app");

  await page.getByTestId("files-refresh").click();
  await expect(page.getByTestId("files-source")).toContainText("Disposable target app");

  await page.getByTestId("files-search-input").fill("app.py");
  await expect(page.getByTestId("files-search-result").filter({ hasText: "app.py" })).toBeVisible();
  await page.getByTestId("files-search-input").press("Enter");
  await expect(page.getByTestId("files-status")).toHaveText("app.py");
  await expect(page.getByTestId("files-source")).toContainText("def health()");

  await page.getByTestId("files-search-input").fill(searchPrefix);
  await expect(page.getByTestId("files-search-result")).toHaveCount(2);
  await expect(page.getByTestId("files-search-result").nth(0)).toHaveAttribute("aria-selected", "true");
  await page.getByTestId("files-search-input").focus();
  await page.getByTestId("files-search-input").press("ArrowDown");
  await expect(page.getByTestId("files-search-result").nth(1)).toHaveAttribute("aria-selected", "true");
  await page.getByTestId("files-search-input").focus();
  await page.getByTestId("files-search-input").press("ArrowUp");
  await expect(page.getByTestId("files-search-result").nth(0)).toHaveAttribute("aria-selected", "true");
  await page.getByTestId("files-search-input").focus();
  await page.getByTestId("files-search-input").press("ArrowDown");
  await expect(page.getByTestId("files-search-result").nth(1)).toHaveAttribute("aria-selected", "true");
  const selectedSearchFile = await page.getByTestId("files-search-result").nth(1).getAttribute("data-files-path");
  expect(selectedSearchFile).toBeTruthy();
  await page.getByTestId("files-search-input").press("Enter");
  await expect(page.getByTestId("files-status")).toHaveText(selectedSearchFile!);
  await expect(page.getByTestId("files-source")).toContainText(
    selectedSearchFile === secondSearchFile ? "second toolbar search fixture" : "first toolbar search fixture",
  );

  await page.getByTestId("files-path-input").fill(largeFile);
  await page.getByTestId("files-go-path").click();
  await expect(page.getByTestId("files-source")).toContainText("toolbar chunk 0000");
  await expect(page.getByTestId("files-source")).not.toContainText(largeTail);
  await expect(page.locator("[data-files-load-more]")).toContainText("Scroll to load more");
  await page.getByTestId("files-source").evaluate((element) => {
    const source = element as HTMLElement;
    source.scrollTop = source.scrollHeight;
    source.dispatchEvent(new Event("scroll"));
  });
  await expect(page.getByTestId("files-source")).toContainText(largeTail);

  await page.getByTestId("files-path-input").fill(imageFile);
  await page.getByTestId("files-go-path").click();
  await expect(page.getByTestId("files-status")).toHaveText(imageFile);
  await expect(page.getByTestId("files-image-preview")).toBeVisible();
  await expect(page.getByTestId("files-image-preview").locator("img")).toHaveAttribute(
    "src",
    /^data:image\/png;base64,/,
  );

  await page.getByTestId("files-path-input").fill(binaryFile);
  await page.getByTestId("files-go-path").click();
  await expect(page.getByTestId("files-status")).toHaveText(binaryFile);
  await expect(page.getByTestId("files-message")).toContainText("Binary preview is not available yet.");
  await expect(page.getByTestId("files-copy-content")).toHaveCount(0);

  await page.getByTestId("files-clear-path").click();
  await expect(page.getByTestId("files-path-input")).toHaveValue("");
  await expect(page.getByTestId("files-search-result").filter({ hasText: secondSearchFile })).toBeVisible();

  const resizeBox = await page.getByTestId("toolbar-resize").boundingBox();
  expect(resizeBox).toBeTruthy();
  const bodyHeightBefore = await page.getByTestId("toolbar-body").evaluate((element) =>
    Math.round((element as HTMLElement).getBoundingClientRect().height),
  );
  await page.mouse.move(resizeBox!.x + resizeBox!.width / 2, resizeBox!.y + resizeBox!.height / 2);
  await page.mouse.down();
  await page.mouse.move(resizeBox!.x + resizeBox!.width / 2, resizeBox!.y + resizeBox!.height / 2 - 90);
  await page.mouse.up();
  const bodyHeightAfter = await page.getByTestId("toolbar-body").evaluate((element) =>
    Math.round((element as HTMLElement).getBoundingClientRect().height),
  );
  expect(bodyHeightAfter).toBeGreaterThan(bodyHeightBefore + 40);
  await expect.poll(() => page.evaluate(() => {
    const stored = JSON.parse(localStorage.getItem("refine_chat_tabs") || "{}");
    return Math.round(Number(stored.bodyHeight || 0));
  })).toBe(bodyHeightAfter);

  await page.reload();
  await expect(page.getByTestId("toolbar-files-panel")).toBeVisible();
  await expect(page.getByTestId("toolbar-body")).toBeVisible();
  await expect.poll(() => page.getByTestId("toolbar-body").evaluate((element) =>
    Math.round((element as HTMLElement).getBoundingClientRect().height),
  )).toBe(bodyHeightAfter);

  await page.getByTestId("toolbar-fullscreen").click();
  await expect(page.getByTestId("toolbar-fullscreen")).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("toolbar-dock")).toHaveClass(/fullscreen/);
  await page.getByTestId("toolbar-fullscreen").click();
  await expect(page.getByTestId("toolbar-fullscreen")).toHaveAttribute("aria-pressed", "false");

  await page.getByTestId("toolbar-collapse").click();
  await expect(page.getByTestId("toolbar-dock")).not.toHaveClass(/open/);
});

test("filters system operations in the toolbar", async ({ page, request }) => {
  await ensureAttachedProject(request);
  await page.goto("/");

  await page.getByTestId("toolbar-tab-files").click();
  await page.getByTestId("files-path-input").fill("missing-toolbar-file.txt");
  await page.getByTestId("files-go-path").click();
  await expect(page.getByTestId("files-message")).toContainText(/not found|No such file|missing/i);

  await page.getByTestId("toolbar-tab-system").click();
  await expect(page.getByTestId("toolbar-system-panel")).toBeVisible();
  await expect(page.getByTestId("system-log-line").filter({ hasText: "missing-toolbar-file.txt" }).first()).toBeVisible();
  await expect(page.getByTestId("system-log-count")).toContainText("/");

  await page.getByTestId("system-log-filter-error").check({ force: true });
  await expect(page.getByTestId("system-log-line").filter({ hasText: "missing-toolbar-file.txt" }).first()).toBeVisible();
  await expect(page.getByTestId("system-log-count")).toContainText("of");
  await expect.poll(() => page.evaluate(() => {
    const stored = JSON.parse(localStorage.getItem("refine_chat_tabs") || "{}");
    return stored.systemFilters || [];
  })).toEqual(["error"]);

  await page.reload();
  await expect(page.getByTestId("toolbar-system-panel")).toBeVisible();
  await expect(page.getByTestId("system-log-filter-error")).toBeChecked();
  await expect(page.getByTestId("system-log-empty")).toContainText("No system activity matches this filter.");

  await page.getByTestId("system-log-filter-all").check({ force: true });
  await page.getByTestId("system-log-filter-queued").check({ force: true });
  await expect(page.getByTestId("system-log-empty")).toContainText("No system activity matches this filter.");

  await page.getByTestId("system-log-filter-all").check({ force: true });
  await expect.poll(() => page.evaluate(() => {
    const stored = JSON.parse(localStorage.getItem("refine_chat_tabs") || "{}");
    return stored.systemFilters || [];
  })).toEqual([]);

  await page.evaluate(() => {
    for (let index = 0; index < 260; index += 1) {
      (window as any).recordUiNotice(`toolbar limit operation ${index}`, {
        kind: "info",
        source: "toolbar-test",
      });
    }
  });
  await expect(page.getByTestId("system-log-count")).toHaveText("250 / 250");
  await expect(page.getByTestId("system-log-line")).toHaveCount(250);
  await expect(page.getByTestId("system-log-line").filter({ hasText: "toolbar limit operation 0" })).toHaveCount(0);
  await expect(page.getByTestId("system-log-line").filter({ hasText: "toolbar limit operation 259" })).toBeVisible();
});

test("runs terminal commands and submits gaps for merge from the toolbar", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const created = await jsonObject(await request.post("/api/gaps", {
    data: { name: `Terminal merge ${Date.now()}` },
  }));
  const gap = created.gap as Record<string, unknown>;
  const gapId = String(gap.id ?? "");
  expect(gapId).toBeTruthy();
  await jsonObject(await request.post(`/api/gaps/${encodeURIComponent(gapId)}/start`, { data: {} }));

  await page.goto("/");

  const tabs = page.locator(".toolbar-tab");
  await expect(tabs.nth(0)).toHaveText(/System/);
  await expect(tabs.nth(1)).toHaveText(/Files/);
  await expect(tabs.nth(2)).toHaveText(/Terminal/);
  await expect(tabs.nth(3)).toHaveText(/Standalone/);

  await page.getByTestId("toolbar-tab-terminal").click();
  await expect(page.getByTestId("toolbar-terminal-panel")).toBeVisible();
  await expect(page.getByTestId("terminal-worktree-select")).toContainText("rust-test-app");

  await page.getByTestId("terminal-command-input").fill("printf toolbar-terminal-ok");
  await page.getByTestId("terminal-run").click();
  await expect(page.getByTestId("terminal-output")).toContainText("toolbar-terminal-ok");

  await page.getByTestId("terminal-gap-id").fill(gapId);
  await page.getByTestId("terminal-submit-merge").click();
  await expect(page.getByTestId("terminal-output")).toContainText(`Gap ${gapId} -> ready-merge`);
  await expect.poll(async () => {
    const detail = await jsonObject(await request.get(`/api/gaps/${encodeURIComponent(gapId)}`));
    const current = detail.gap as Record<string, unknown>;
    return String(current.status ?? "");
  }).toBe("ready-merge");
});

test("resets project-scoped toolbar state when switching apps", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const original = await projectStatus(request);
  const originalPath = String(original.client_repo ?? testAppRoot());
  const disposablePath = path.join(
    process.cwd(),
    `target/refine-integration/apps/toolbar-reset-app-${Date.now()}`,
  );
  await request.delete("/api/apps", { data: { path: disposablePath } }).catch(() => undefined);
  fs.rmSync(disposablePath, { recursive: true, force: true });

  try {
    await page.goto("/");
    await page.evaluate(() => {
      localStorage.setItem("refine_chat_tabs", JSON.stringify({
        tabs: {
          system: { label: "System", mode: "system" },
          files: { label: "Files", mode: "files" },
          standalone: { label: "Standalone", mode: "standalone" },
          "GAP-OLD": { label: "Old Gap", mode: "gap", gapId: "GAP-OLD", sessionId: "stale-session" },
        },
        activeTabId: "GAP-OLD",
        open: true,
        fullscreen: true,
        bodyHeight: 444,
        systemFilters: ["error"],
      }));
    });
    await page.reload();
    await expect(page.getByTestId("toolbar-dock")).toHaveClass(/open/);
    await expect(page.getByTestId("toolbar-dock")).toHaveClass(/fullscreen/);
    await expect(page.getByTestId("toolbar-tab-GAP-OLD")).toHaveCount(1);

    const attachedPath = await page.evaluate(async (targetPath) => {
      const result = await (window as any).api("POST", "/api/project/attach", { path: targetPath });
      await (window as any).applyProjectAttachResult(result);
      return result.client_repo || "";
    }, disposablePath);
    expect(attachedPath).toBe(disposablePath);
    await expect.poll(async () =>
      String((await projectStatus(request)).client_repo ?? "")
    ).toBe(disposablePath);

    await expect(page.getByTestId("toolbar-dock")).not.toHaveClass(/open/);
    await expect(page.getByTestId("toolbar-dock")).not.toHaveClass(/fullscreen/);
    await expect(page.getByTestId("toolbar-tab-GAP-OLD")).toHaveCount(0);
    await expect(page.getByTestId("toolbar-tab-standalone")).toHaveCount(1);
    await expect.poll(async () => page.evaluate(() => {
      const stored = JSON.parse(localStorage.getItem("refine_chat_tabs") || "{}");
      return {
        activeTabId: stored.activeTabId,
        open: stored.open,
        fullscreen: stored.fullscreen,
        bodyHeight: stored.bodyHeight ?? null,
        systemFilters: stored.systemFilters || [],
        tabIds: Object.keys(stored.tabs || {}),
      };
    })).toEqual({
      activeTabId: "standalone",
      open: false,
      fullscreen: false,
      bodyHeight: null,
      systemFilters: [],
      tabIds: ["system", "files", "terminal", "standalone"],
    });
  } finally {
    await attachProject(request, originalPath);
    await request.delete("/api/apps", { data: { path: disposablePath } }).catch(() => undefined);
    fs.rmSync(disposablePath, { recursive: true, force: true });
  }
});
