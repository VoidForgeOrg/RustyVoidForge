import { defineConfig, devices } from "@playwright/test";

// The walk starts nothing: web-walk.py serves the built app on localhost:8081, and e2e.sh runs the API
// and dex.
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
    // web-walk.py's own child serves on 127.0.0.1; `localhost` resolves only there, so no other app
    // on ::1 can answer, while the URL, dex's redirect and the origin stay `localhost`.
    launchOptions: { args: ["--host-resolver-rules=MAP localhost 127.0.0.1"] },
    trace: "retain-on-failure",
  },
});
