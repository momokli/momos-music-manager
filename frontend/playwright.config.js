import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  timeout: 15000,
  expect: { timeout: 5000 },
  retries: 1,
  workers: 1,
  reporter: [["html"], ["list"]],
  // Global shared webServer: Rust backend (API + seed endpoint)
  webServer: [
    {
      command: "cargo run -- serve --host 127.0.0.1 --port 3000",
      cwd: "..",
      url: "http://localhost:3000/api/health",
      reuseExistingServer: false,
      timeout: 90000,
      env: {
        DATABASE_URL: "sqlite:test-playwright.db",
      },
    },
    {
      command: "npx vite --port 5173",
      cwd: ".",
      url: "http://localhost:5173",
      reuseExistingServer: true,
      timeout: 30000,
    },
  ],
  projects: [
    {
      name: "react",
      testMatch: /.*\.spec\.ts/,
      use: { baseURL: "http://localhost:5173" },
    },
    {
      name: "vanilla",
      testMatch: /.*\.spec\.js/,
      use: { baseURL: "http://localhost:3001" },
    },
  ],
});
