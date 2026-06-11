import { expect, test, type APIRequestContext, type Page } from "@playwright/test";
import { ensureAttachedProject, jsonObject, waitForOperationResult } from "./helpers";

const EMPTY_TARGET_APP_SETTINGS = {
  target_app_url: "",
  target_app_start_command: "",
  target_app_stop_command: "",
  target_app_build_command: "",
  target_app_status_command: "",
  target_app_cwd: "",
  target_app_env_json: "{}",
  target_app_start_timeout_seconds: "120",
  target_app_stop_timeout_seconds: "60",
  target_app_build_timeout_seconds: "300",
  target_app_status_timeout_seconds: "10",
  target_app_log_path: "",
  target_app_http_check_url: "",
  target_app_tcp_check_host: "",
  target_app_tcp_check_port: "",
  target_app_process_check_command: "",
};

async function resetTargetAppSettings(request: APIRequestContext) {
  await request.patch("/api/settings", { data: EMPTY_TARGET_APP_SETTINGS });
}

async function fillAndChange(page: Page, testId: string, value: string) {
  const field = page.getByTestId(testId);
  await field.fill(value);
  await field.dispatchEvent("change");
}

test("generates target-app config through Smoke AI", async ({ page, request }) => {
  await ensureAttachedProject(request);

  try {
    await page.goto("/#/node/target-app");
    await expect(page.getByRole("heading", { name: "Node", level: 2 })).toBeVisible();
    await expect(page.getByTestId("target-app-generate-ai")).toBeVisible();

    const generated = page.waitForResponse((response) =>
      response.url().includes("/api/target-app/generate-instructions") &&
      response.request().method() === "POST" &&
      response.status() === 202
    );
    await page.getByTestId("target-app-generate-ai").click();
    await page.getByRole("button", { name: "Generate", exact: true }).click();
    const payload = await (await generated).json();
    const operationId = String(payload.operation?.id ?? "");
    expect(payload.operation?.owner).toBe("target-app:generate");
    expect(operationId).toBeTruthy();
    const result = await waitForOperationResult(request, operationId);

    expect(result.provider).toBe("smoke-ai");
    expect(result.source).toBe("provider");
    expect(String(result.raw ?? "")).toContain("refine-smoke target app check passed");
    const config = (result.config as Record<string, unknown> | undefined) ?? {};
    expect(config.start_command).toBe("./.refine/manage-app.sh start");

    await expect(page.getByTestId("target-app-start-command")).toHaveValue("./.refine/manage-app.sh start");
    await expect(page.getByTestId("target-app-stop-command")).toHaveValue("./.refine/manage-app.sh stop");
    await expect(page.getByTestId("target-app-build-command")).toHaveValue("./.refine/manage-app.sh build");
    await expect(page.getByTestId("target-app-status-command")).toHaveValue("./.refine/manage-app.sh status");
    await expect(page.getByTestId("target-app-env")).toHaveValue(/REFINE_SMOKE_TARGET/);
    await expect(page.getByTestId("target-app-http-url")).toHaveValue("http://127.0.0.1:3456/health");
    await expect(page.getByTestId("target-app-process-command")).toHaveValue("printf smoke-ai-target-process");

    await expect.poll(async () => {
      const saved = await jsonObject(await request.get("/api/settings"));
      const settings = (saved.settings as Record<string, unknown> | undefined) ?? {};
      return String(settings.target_app_start_command ?? "");
    }).toBe("./.refine/manage-app.sh start");

    await page.getByTestId("toolbar-tab-system").click();
    await expect(page.getByTestId("system-log-line").filter({ hasText: "Generate with AI started" }).first()).toBeVisible();
    await expect(page.getByTestId("system-log-line").filter({ hasText: "Generate with AI completed" }).first()).toBeVisible();
  } finally {
    await resetTargetAppSettings(request);
  }
});

test("keeps Generate with AI loading state after reload until the background operation finishes", async ({ page, request }) => {
  await ensureAttachedProject(request);
  await resetTargetAppSettings(request);

  let complete = false;
  await page.route("**/api/operations/operation-target-reload", async (route) => {
    await route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        operation: complete
          ? {
              id: "operation-target-reload",
              owner: "target-app:generate",
              status: "complete",
              result: {
                http_status: 200,
                ok: true,
                config: {
                  start_command: "./.refine/manage-app.sh start",
                  stop_command: "./.refine/manage-app.sh stop",
                  build_command: "./.refine/manage-app.sh build",
                  status_command: "./.refine/manage-app.sh status",
                  cwd: ".",
                  env: {},
                  start_timeout_seconds: 120,
                  stop_timeout_seconds: 60,
                  build_timeout_seconds: 300,
                  status_timeout_seconds: 10,
                  log_path: ".refine/manage-app.log",
                  http_check_url: "",
                  tcp_check_host: "",
                  tcp_check_port: "",
                  process_check_command: "",
                  notes: "reload result",
                },
              },
            }
          : {
              id: "operation-target-reload",
              owner: "target-app:generate",
              status: "running",
              progress: { message: "Generating target-app config with AI" },
              result: {},
            },
      }),
    });
  });

  try {
    await page.goto("/#/node/target-app");
    await page.evaluate(() => {
      localStorage.setItem("refine_target_app_generate_operation", JSON.stringify({
        operationId: "operation-target-reload",
        startedAt: Date.now(),
      }));
    });
    await page.reload();

    await expect(page.getByTestId("target-app-generate-ai")).toBeDisabled();
    await expect(page.getByTestId("target-app-generate-ai")).toHaveText("Generating...");
    await page.getByTestId("toolbar-tab-system").click();
    await expect(page.getByTestId("system-log-line").filter({ hasText: "Generating target-app config with AI" }).first()).toBeVisible();

    complete = true;
    await expect(page.getByTestId("target-app-start-command")).toHaveValue("./.refine/manage-app.sh start");
    await expect(page.getByTestId("target-app-generate-ai")).toBeEnabled();
    await expect(page.getByTestId("target-app-generate-ai")).toHaveText("Generate with AI");
    await expect.poll(() => page.evaluate(() => localStorage.getItem("refine_target_app_generate_operation"))).toBeNull();
  } finally {
    await page.unroute("**/api/operations/operation-target-reload");
    await resetTargetAppSettings(request);
  }
});

test("autosaves target-app config fields", async ({ page, request }) => {
  await ensureAttachedProject(request);
  await resetTargetAppSettings(request);

  try {
    await page.goto("/#/node/target-app");
    await expect(page.getByTestId("target-app-start-command")).toBeVisible();

    await fillAndChange(page, "target-app-url", "http://127.0.0.1:4321");
    await fillAndChange(page, "target-app-start-command", "printf manual-start");
    await fillAndChange(page, "target-app-stop-command", "printf manual-stop");
    await fillAndChange(page, "target-app-build-command", "printf manual-build");
    await fillAndChange(page, "target-app-status-command", "printf manual-status");
    await fillAndChange(page, "target-app-cwd", "demo-app");
    await fillAndChange(page, "target-app-env", "{\"MANUAL_TARGET_APP\":\"1\"}");
    await fillAndChange(page, "target-app-start-timeout", "21");
    await fillAndChange(page, "target-app-stop-timeout", "22");
    await fillAndChange(page, "target-app-build-timeout", "23");
    await fillAndChange(page, "target-app-status-timeout", "24");
    await fillAndChange(page, "target-app-log-path", "target/manual-target-app.log");
    await fillAndChange(page, "target-app-http-url", "http://127.0.0.1:4321/health");
    await fillAndChange(page, "target-app-tcp-host", "127.0.0.1");
    await fillAndChange(page, "target-app-tcp-port", "4321");
    await fillAndChange(page, "target-app-process-command", "printf manual-process");

    await expect.poll(async () => {
      const saved = await jsonObject(await request.get("/api/settings"));
      const settings = (saved.settings as Record<string, unknown> | undefined) ?? {};
      return [
        settings.target_app_url,
        settings.target_app_start_command,
        settings.target_app_stop_command,
        settings.target_app_build_command,
        settings.target_app_status_command,
        settings.target_app_cwd,
        settings.target_app_env_json,
        settings.target_app_start_timeout_seconds,
        settings.target_app_stop_timeout_seconds,
        settings.target_app_build_timeout_seconds,
        settings.target_app_status_timeout_seconds,
        settings.target_app_log_path,
        settings.target_app_http_check_url,
        settings.target_app_tcp_check_host,
        settings.target_app_tcp_check_port,
        settings.target_app_process_check_command,
      ].join("|");
    }).toBe([
      "http://127.0.0.1:4321",
      "printf manual-start",
      "printf manual-stop",
      "printf manual-build",
      "printf manual-status",
      "demo-app",
      "{\"MANUAL_TARGET_APP\":\"1\"}",
      "21",
      "22",
      "23",
      "24",
      "target/manual-target-app.log",
      "http://127.0.0.1:4321/health",
      "127.0.0.1",
      "4321",
      "printf manual-process",
    ].join("|"));
  } finally {
    await resetTargetAppSettings(request);
  }
});
