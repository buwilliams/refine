import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";

const repoRoot = process.cwd();
const port = Number(process.env.REFINE_TEST_PORT || "18080");
const baseURL = process.env.REFINE_TEST_BASE_URL || `http://127.0.0.1:${port}`;
const binary = process.env.REFINE_TEST_REFINE_BIN;
const runtimeRoot = process.env.REFINE_TEST_RUNTIME_ROOT || path.join(repoRoot, "target/refine-integration/run");
const appRoot = process.env.REFINE_TEST_APP_ROOT || path.join(repoRoot, "target/refine-integration/apps/rust-test-app");
const artifactRoot = path.join(repoRoot, "target/refine-integration/artifacts/ui");
const metadataPath = path.join(artifactRoot, "daemon.json");

export default async function globalSetup() {
  if (!binary) {
    throw new Error("REFINE_TEST_REFINE_BIN is required. Run through `cargo run --manifest-path xtask/Cargo.toml -- test-ui`.");
  }
  if (!fs.existsSync(binary)) {
    throw new Error(`REFINE_TEST_REFINE_BIN does not exist: ${binary}`);
  }

  fs.rmSync(artifactRoot, { recursive: true, force: true });
  fs.mkdirSync(artifactRoot, { recursive: true });
  stopDaemon();
  fs.rmSync(runtimeRoot, { recursive: true, force: true });
  fs.rmSync(appRoot, { recursive: true, force: true });
  ensureTestApp();

  const stdout = fs.openSync(path.join(artifactRoot, "daemon.stdout.log"), "w");
  const stderr = fs.openSync(path.join(artifactRoot, "daemon.stderr.log"), "w");
  const child = spawn(binary, [
    "system", "start",
    "--foreground",
    "--port", String(port),
    "--runtime-root", runtimeRoot,
    "--static-root", path.join(repoRoot, "src/surfaces/web/static")
  ], {
    cwd: repoRoot,
    detached: true,
    env: refineEnv(),
    stdio: ["ignore", stdout, stderr]
  });
  child.unref();
  fs.writeFileSync(metadataPath, JSON.stringify({
    pid: child.pid,
    port,
    baseURL,
    runtimeRoot,
    appRoot,
    artifactRoot,
    binary
  }, null, 2));

  await waitForDaemon();
  runRefine(["project", "attach", appRoot], "project attach");
  await waitForAttachedProject();
  await configureSmokeAiProvider();
  await createReporter();
}

function refineEnv(): NodeJS.ProcessEnv {
  return {
    ...process.env,
    REFINE_TEST_PORT: String(port),
    REFINE_DAEMON_PORT: String(port),
    REFINE_TEST_BASE_URL: baseURL,
    REFINE_TEST_RUNTIME_ROOT: runtimeRoot,
    REFINE_TEST_APP_ROOT: appRoot
  };
}

function ensureTestApp() {
  fs.mkdirSync(appRoot, { recursive: true });
  fs.writeFileSync(path.join(appRoot, "README.md"), "# Refine rust smoke target app\n\nDisposable target app for the Rust UI suite.\n");
  fs.writeFileSync(path.join(appRoot, "app.py"), "def health() -> str:\n    return \"ok\"\n");
  fs.writeFileSync(path.join(appRoot, ".gitignore"), "__pycache__/\n*.py[cod]\n");
  if (!fs.existsSync(path.join(appRoot, ".git"))) runGit(["init", "-q"]);
  runGit(["config", "user.email", "refine-smoke@example.invalid"]);
  runGit(["config", "user.name", "Refine Rust Smoke"]);
  runGit(["add", "README.md", "app.py", ".gitignore"]);
  const diff = spawnSync("git", ["diff", "--cached", "--quiet", "--exit-code"], { cwd: appRoot, encoding: "utf-8" });
  if (diff.status === 1) runGit(["commit", "-q", "-m", "Initialize refine rust smoke target app"]);
  if (diff.status !== 0 && diff.status !== 1) {
    throw new Error(`git diff --cached failed\nstdout:\n${diff.stdout}\nstderr:\n${diff.stderr}`);
  }
}

function runGit(args: string[]) {
  const result = spawnSync("git", args, { cwd: appRoot, encoding: "utf-8" });
  if (result.status !== 0) throw new Error(`git ${args.join(" ")} failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`);
}

function runRefine(args: string[], label: string) {
  const result = spawnSync(binary!, args, { cwd: repoRoot, encoding: "utf-8", env: refineEnv() });
  fs.appendFileSync(path.join(artifactRoot, "cli-transcript.log"), `$ refine ${args.join(" ")}\nstatus: ${result.status}\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}\n\n`);
  if (result.status !== 0) throw new Error(`${label} failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`);
}

function stopDaemon() {
  if (!binary || !fs.existsSync(binary)) return;
  spawnSync(binary, ["system", "stop", "--port", String(port), "--runtime-root", runtimeRoot], {
    cwd: repoRoot,
    encoding: "utf-8",
    env: refineEnv()
  });
}

async function waitForDaemon() {
  const deadline = Date.now() + 60_000;
  let lastError = "";
  while (Date.now() < deadline) {
    try {
      await httpGet("/system/version");
      return;
    } catch (error) {
      lastError = String(error);
      await delay(100);
    }
  }
  throw new Error(`daemon did not become ready at ${baseURL}: ${lastError}`);
}

async function waitForAttachedProject() {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    try {
      const status = JSON.parse(await httpGet("/api/project/status"));
      if (status.attached === true && String(status.client_repo || "").endsWith("rust-test-app")) return;
    } catch (_error) {
      // keep polling
    }
    await delay(100);
  }
  throw new Error(`project did not report attached at ${baseURL}`);
}

async function createReporter() {
  await httpRequest("POST", "/api/reporters", JSON.stringify({ name: "refine-smoke" }));
}

async function configureSmokeAiProvider() {
  await httpRequest("PATCH", "/api/settings", JSON.stringify({ agent_cli: "smoke-ai" }));
}

function httpGet(route: string): Promise<string> {
  return httpRequest("GET", route);
}

function httpRequest(method: string, route: string, body = ""): Promise<string> {
  return new Promise((resolve, reject) => {
    const url = new URL(`${baseURL}${route}`);
    const request = http.request({
      hostname: url.hostname,
      port: url.port,
      path: `${url.pathname}${url.search}`,
      method,
      headers: {
        "Content-Type": "application/json",
        "Content-Length": Buffer.byteLength(body),
        "X-Refine-API-Version": "1",
        "Idempotency-Key": `ui-setup-${Date.now()}-${Math.random().toString(16).slice(2)}`
      }
    }, (response) => {
      let body = "";
      response.setEncoding("utf8");
      response.on("data", (chunk) => body += chunk);
      response.on("end", () => {
        if (response.statusCode && response.statusCode >= 200 && response.statusCode < 300) resolve(body);
        else reject(new Error(`HTTP ${response.statusCode}: ${body}`));
      });
    });
    request.on("error", reject);
    request.setTimeout(2000, () => {
      request.destroy(new Error("timeout"));
    });
    request.write(body);
    request.end();
  });
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
