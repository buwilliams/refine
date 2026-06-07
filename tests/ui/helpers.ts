import { expect, type APIRequestContext, type APIResponse } from "@playwright/test";

export async function jsonObject(response: APIResponse): Promise<Record<string, unknown>> {
  expect(response.ok(), await response.text()).toBeTruthy();
  const data = await response.json();
  expect(typeof data).toBe("object");
  expect(data).not.toBeNull();
  return data as Record<string, unknown>;
}

export async function ensureAttachedProject(request: APIRequestContext): Promise<void> {
  const status = await jsonObject(await request.get("/api/project/status"));
  expect(status.attached).toBe(true);
  expect(String(status.client_repo ?? "")).toMatch(/rust-test-app$/);
}
