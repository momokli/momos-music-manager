import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  timeout: 15000,
  expect: { timeout: 5000 },
  retries: 1,
  workers: 1, // one at a time — SQLite is single-writer
  reporter: [["html"], ["list"]],
  use: {
    baseURL: "http://localhost:3001",
    screenshot: "only-on-failure",
    trace: "on-first-retry",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    // macOS provenance attr prevents binary from creating files; touch first.
    // --no-autoupdate: the serve() startup check would write its result into
    // the same SQLite ~10 s after boot (and hit GitHub); on slow filesystems
    // that write collides with the real-API specs (SQLite BUSY → HTTP 500).
    command:
      "touch test-playwright.db && cargo run -- serve --no-autoupdate --host 127.0.0.1 --port 3001",
    cwd: "..", // run from project root
    url: "http://localhost:3001/api/health",
    reuseExistingServer: false,
    timeout: 90000,
    env: {
      DATABASE_URL: "sqlite:test-playwright.db",
    },
  },
});
