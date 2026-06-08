import { expect, type APIRequestContext, type APIResponse } from "@playwright/test";

export async function jsonObject(response: APIResponse): Promise<Record<string, unknown>> {
  expect(response.ok(), await response.text()).toBeTruthy();
  const data = await response.json();
  expect(typeof data).toBe("object");
  expect(data).not.toBeNull();
  return data as Record<string, unknown>;
}

export async function waitForJobResult(
  request: APIRequestContext,
  jobId: string,
): Promise<Record<string, unknown>> {
  await expect.poll(async () => {
    const payload = await jsonObject(await request.get(`/api/jobs/${jobId}`));
    const job = (payload.job as Record<string, unknown> | undefined) ?? {};
    return String(job.status ?? "");
  }).toBe("complete");
  const payload = await jsonObject(await request.get(`/api/jobs/${jobId}`));
  const job = (payload.job as Record<string, unknown> | undefined) ?? {};
  return (job.result as Record<string, unknown> | undefined) ?? {};
}

export async function ensureAttachedProject(request: APIRequestContext): Promise<void> {
  const status = await jsonObject(await request.get("/api/project/status"));
  expect(status.attached).toBe(true);
  expect(String(status.client_repo ?? "")).toMatch(/rust-test-app$/);
}

export async function projectStatus(request: APIRequestContext): Promise<Record<string, unknown>> {
  return jsonObject(await request.get("/api/project/status"));
}

export async function detachProject(request: APIRequestContext): Promise<Record<string, unknown>> {
  return jsonObject(await request.post("/api/project/detach"));
}

export async function attachProject(
  request: APIRequestContext,
  path: string,
): Promise<Record<string, unknown>> {
  return jsonObject(await request.post("/api/project/attach", { data: { path } }));
}
