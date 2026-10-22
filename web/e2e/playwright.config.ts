import { defineConfig, devices } from "@playwright/test";

// The walk starts nothing: e2e.sh serves the built app on localhost:8081 and runs the API and dex.
export default defineConfig({
  testDir: ".",
  timeout: 10 * 60_000,
  workers: 1,
  retries: 0,
  reporter: [["list"]],
  use: {
    ...devices["Desktop Chrome"],
    baseURL: "http://localhost:8081",
    actionTimeout: 30_000,
    trace: "retain-on-failure",
  },
});
