import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const repoRoot = process.cwd();
const artifactRoot = path.join(repoRoot, "target/refine-integration/artifacts/ui");
const metadataPath = path.join(artifactRoot, "daemon.json");

export default async function globalTeardown() {
  const metadata = readMetadata();
  if (!metadata) return;
  copyRuntime(metadata, "teardown");
  spawnSync(metadata.binary, ["system", "stop", "--port", String(metadata.port), "--runtime-root", metadata.runtimeRoot], {
    cwd: repoRoot,
    encoding: "utf-8",
    env: {
      ...process.env,
      REFINE_DAEMON_PORT: String(metadata.port),
      REFINE_TEST_PORT: String(metadata.port)
    }
  });
  if (metadata.pid) {
    try {
      process.kill(metadata.pid, "SIGTERM");
    } catch (_error) {
      // Process may already have exited after graceful stop.
    }
  }
  copyRuntime(metadata, "after-stop");
  fs.rmSync(metadata.runtimeRoot, { recursive: true, force: true });
  fs.rmSync(metadata.appRoot, { recursive: true, force: true });
}

function readMetadata(): any | null {
  try {
    return JSON.parse(fs.readFileSync(metadataPath, "utf-8"));
  } catch (_error) {
    return null;
  }
}

function copyRuntime(metadata: any, label: string) {
  const source = path.join(metadata.runtimeRoot, String(metadata.port));
  if (!fs.existsSync(source)) return;
  const dest = path.join(metadata.artifactRoot, `runtime-${label}-${Date.now()}`);
  fs.cpSync(source, dest, { recursive: true, force: true });
}
