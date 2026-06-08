import { expect, test } from "@playwright/test";
import { ensureAttachedProject, jsonObject } from "./helpers";

test("generates governance rules through Smoke AI", async ({ page, request }) => {
  test.setTimeout(60_000);
  await ensureAttachedProject(request);

  try {
    await page.goto("/#/project/governance");
    await expect(page.getByRole("heading", { name: "Governance", level: 2 })).toBeVisible();

    await page.getByTestId("s-governance-product-edit").click();
    await page.getByTestId("s-governance-product").fill(
      "Refine helps teams turn product gaps into verifiable implementation work.",
    );
    await page.getByTestId("s-governance-product-edit").click();

    await page.getByTestId("s-governance-constitution-edit").click();
    await page.getByTestId("s-governance-constitution").fill(
      "Prefer deterministic tests, reversible changes, and clear ownership.",
    );
    await page.getByTestId("s-governance-constitution-edit").click();

    const generated = page.waitForResponse((response) =>
      response.url().includes("/api/governance/generate-rules") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    const savedGeneratedRules = page.waitForResponse((response) =>
      response.url().includes("/api/governance") &&
      response.request().method() === "PATCH" &&
      response.status() === 200
    );
    await page.getByTestId("governance-generate").click();
    const payload = await (await generated).json();
    expect(payload.provider).toBe("smoke-ai");
    expect(payload.source).toBe("provider");
    expect(String(payload.raw ?? "")).toContain("smoke-ai governance response");

    await expect(page.getByTestId("governance-rule-input").first()).toHaveValue(/smoke-ai governance response/);
    await expect(page.getByTestId("governance-rule-input").nth(1)).toHaveValue(/clear ownership/);
    await savedGeneratedRules;

    const saved = await jsonObject(await request.get("/api/governance"));
    const rules = (saved.rules as Array<{ text?: string; source?: string }> | undefined) ?? [];
    expect(rules.some((rule) => rule.text?.includes("smoke-ai governance response"))).toBeTruthy();
    expect(rules.every((rule) => rule.source === "generated")).toBeTruthy();
  } finally {
    await request.patch("/api/governance", {
      data: {
        product: "",
        constitution: "",
        rules: [],
      },
    });
  }
});

test("autosaves manual governance edits and rule removal", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const suffix = Date.now();
  const product = `Manual governance product ${suffix}`;
  const constitution = `Manual governance constitution ${suffix}`;
  const rule = `Manual governance rule ${suffix}`;

  try {
    await page.goto("/#/project/governance");
    await expect(page.getByRole("heading", { name: "Governance", level: 2 })).toBeVisible();

    const productSaved = page.waitForResponse((response) =>
      response.url().includes("/api/governance") &&
      response.request().method() === "PATCH" &&
      response.status() === 200
    );
    await page.getByTestId("s-governance-product-edit").click();
    await page.getByTestId("s-governance-product").fill(product);
    await page.getByTestId("s-governance-product").blur();
    await productSaved;

    const constitutionSaved = page.waitForResponse((response) =>
      response.url().includes("/api/governance") &&
      response.request().method() === "PATCH" &&
      response.status() === 200
    );
    await page.getByTestId("s-governance-constitution-edit").click();
    await page.getByTestId("s-governance-constitution").fill(constitution);
    await page.getByTestId("s-governance-constitution").blur();
    await constitutionSaved;

    await expect.poll(async () => {
      const saved = await jsonObject(await request.get("/api/governance"));
      return {
        product: saved.product,
        constitution: saved.constitution,
      };
    }).toEqual({ product, constitution });

    await page.getByTestId("governance-add-rule").click();
    const ruleInput = page.getByTestId("governance-rule-input").last();
    await expect(ruleInput).toBeFocused();
    const ruleSaved = page.waitForResponse((response) =>
      response.url().includes("/api/governance") &&
      response.request().method() === "PATCH" &&
      response.status() === 200
    );
    await ruleInput.fill(rule);
    await ruleInput.blur();
    await ruleSaved;
    await expect.poll(async () => {
      const saved = await jsonObject(await request.get("/api/governance"));
      const rules = (saved.rules as Array<{ text?: string; source?: string }> | undefined) ?? [];
      return rules.find((item) => item.text === rule)?.source ?? "";
    }).toBe("manual");

    const removed = page.waitForResponse((response) =>
      response.url().includes("/api/governance") &&
      response.request().method() === "PATCH" &&
      response.status() === 200
    );
    await page.getByTestId("governance-remove-rule").last().click();
    await removed;
    await expect.poll(async () => {
      const saved = await jsonObject(await request.get("/api/governance"));
      const rules = (saved.rules as Array<{ text?: string }> | undefined) ?? [];
      return rules.some((item) => item.text === rule);
    }).toBe(false);
  } finally {
    await request.patch("/api/governance", {
      data: {
        product: "",
        constitution: "",
        rules: [],
      },
    });
  }
});
