import { defineConfig, devices } from "@playwright/test";

const baseURL = process.env.REFINE_TEST_BASE_URL || "http://127.0.0.1:18080";

export default defineConfig({
  testDir: "./tests/ui",
  globalSetup: "./tests/ui/global-setup.ts",
  globalTeardown: "./tests/ui/global-teardown.ts",
  timeout: 30_000,
  workers: 1,
  expect: {
    timeout: 10_000
  },
  fullyParallel: false,
  reporter: [["list"], ["html", { open: "never", outputFolder: "target/refine-integration/artifacts/playwright-report" }]],
  use: {
    baseURL,
    trace: "retain-on-failure",
    screenshot: "only-on-failure"
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] }
    }
  ]
});
