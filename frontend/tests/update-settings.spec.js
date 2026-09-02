import { test, expect } from "@playwright/test";

/**
 * Update-settings view (Phase A+B): status card, check now, toggle
 * persistence, override hint, update now flows.
 *
 * The Playwright server boots `cargo run serve` against test-playwright.db
 * (fresh per run unless the file persists — tests must not assume the
 * toggle's default state).
 */

function statusStub(overrides = {}) {
  return {
    currentVersion: "1.1.0",
    channel: "release",
    channelSource: "default",
    availableChannels: ["release", "rolling"],
    baseUrl: "https://github.com/momokli/momos-music-manager/releases/latest",
    enabled: true,
    enabledSource: "default",
    artifact: { osArch: "linux-x64", ext: "tar.gz" },
    lastCheckAt: null,
    lastCheckStatus: null,
    lastCheckError: null,
    lastCheckResult: null,
    updateAvailable: false,
    pendingUpdate: null,
    pendingUpdateError: null,
    platformSelfInstall: true,
    autoApplyIntervalSecs: 14400,
    autoApplyIntervalSource: "default",
    ...overrides,
  };
}

async function stubStatus(page, overrides) {
  await page.route("**/api/update/status", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ data: statusStub(overrides) }),
    }),
  );
}

test.describe("Settings page — update controls", () => {
  test("page loads without JS errors and shows the update card", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await page.goto("/#settings");
    await page.waitForSelector("#settings-updates-card", { timeout: 8000 });

    // Nav entry exists and is active
    await expect(page.locator('.topnav-link[data-page="settings"]')).toBeVisible();
    await expect(page.locator('.topnav-link[data-page="settings"]')).toHaveClass(/active/);

    // Real status endpoint renders version + toggle (the switch label is
    // visible; the native input is visually hidden by the switch CSS)
    await page.waitForSelector("#settings-updates-content code", { timeout: 8000 });
    await expect(page.locator(".switch")).toBeVisible();
    await expect(page.locator("#autoupdate-toggle")).toBeAttached();

    expect(errors).toEqual([]);
  });

  test("status card shows version, channel and last-check state", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await stubStatus(page, {
      currentVersion: "1.1.0-dev+abc1234",
      channel: "rolling",
      lastCheckAt: 1756742400,
      lastCheckStatus: "ok",
      lastCheckResult: {
        state: "upToDate",
        availableVersion: null,
        currentVersion: "1.1.0-dev+abc1234",
        artifactName: null,
      },
    });

    await page.goto("/#settings");
    await page.waitForSelector("#settings-updates-card", { timeout: 8000 });
    await expect(page.locator("#settings-updates-content")).toContainText("1.1.0-dev+abc1234");
    await expect(page.locator("#settings-updates-content")).toContainText("rolling");
    await expect(page.locator("#settings-updates-content")).toContainText("Up to date");
    // lastCheckAt 1756742400 = 2025-09-01 — locale-dependent, so assert the
    // row exists and does not say "never"
    await expect(page.locator("#settings-updates-content")).not.toContainText("never");

    expect(errors).toEqual([]);
  });

  test("check now triggers POST /api/update/check and shows result inline", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    let checkCalled = false;
    await page.route("**/api/update/check", (route) => {
      checkCalled = true;
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          data: statusStub({
            lastCheckAt: 1756742400,
            lastCheckStatus: "ok",
            lastCheckResult: {
              state: "updateAvailable",
              availableVersion: "1.2.0",
              currentVersion: "1.1.0",
              artifactName: "momos-music-manager-1.2.0-linux-x64.tar.gz",
            },
            updateAvailable: true,
          }),
        }),
      });
    });

    await page.goto("/#settings");
    await page.waitForSelector("#update-check-now-btn", { timeout: 8000 });
    await page.click("#update-check-now-btn");

    await expect(page.locator("#settings-update-inline")).toContainText(
      "Update available: v1.2.0",
      { timeout: 8000 },
    );
    expect(checkCalled).toBe(true);
    expect(errors).toEqual([]);
  });

  test("toggle persists across reload via real API", async ({ page }) => {
    // Real-API roundtrips can be slow on cold/slow filesystems (SQLite
    // fsync per write) — give this test room beyond the 15 s default.
    test.setTimeout(60_000);

    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    const settingsCalls = [];
    await page.route("**/api/update/settings", (route) => {
      settingsCalls.push(route.request().postDataJSON());
      // Continue to the real backend — the toggle must really persist in
      // test-playwright.db so the reload assertion is meaningful.
      return route.continue();
    });

    // The real backend on this machine can be slow under load — generous
    // explicit waits (the 15 s test default is too tight for real-API flows).
    const SLOW_WAIT = 30_000;

    await page.goto("/#settings");
    await page.waitForSelector("#autoupdate-toggle", {
      state: "attached",
      timeout: SLOW_WAIT,
    });

    // Normalize to "on" first (DB may persist a previous run's state).
    // The native input is visually hidden by the switch CSS — click the
    // label (the visible slider) to toggle it.
    const toggle = page.locator("#autoupdate-toggle");
    if (!(await toggle.isChecked())) {
      await page.locator(".switch").click();
      await expect.poll(() => settingsCalls.length).toBeGreaterThan(0);
    }
    await expect(toggle).toBeChecked();

    // Turn off — real POST, then verify the request body
    await page.locator(".switch").click();
    await expect.poll(() => settingsCalls.length).toBeGreaterThan(0);
    expect(settingsCalls[settingsCalls.length - 1]).toEqual({ autoUpdateEnabled: false });
    await expect(page.locator("#settings-updates-content")).toContainText("Disabled");

    // Reload: the toggle must stay off (persisted via the real settings API
    // and reflected by GET /api/update/status)
    await page.reload();
    await page.waitForSelector("#autoupdate-toggle", {
      state: "attached",
      timeout: SLOW_WAIT,
    });
    await expect(page.locator("#autoupdate-toggle")).not.toBeChecked();
    await expect(page.locator("#settings-updates-content")).toContainText("Disabled");

    expect(errors).toEqual([]);
  });

  test("toggle disabled with override hint when enabledSource is toml", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await stubStatus(page, { enabledSource: "toml" });
    await page.goto("/#settings");
    await page.waitForSelector("#autoupdate-toggle", { state: "attached", timeout: 8000 });

    await expect(page.locator("#autoupdate-toggle")).toBeDisabled();
    await expect(page.locator("#settings-updates-content")).toContainText("config.toml");

    expect(errors).toEqual([]);
  });

  test("toggle disabled with override hint when enabledSource is env", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await stubStatus(page, { enabledSource: "env" });
    await page.goto("/#settings");
    await page.waitForSelector("#autoupdate-toggle", { state: "attached", timeout: 8000 });

    await expect(page.locator("#autoupdate-toggle")).toBeDisabled();
    await expect(page.locator("#settings-updates-content")).toContainText(
      "MOMOS_AUTOUPDATE_ENABLED",
    );

    expect(errors).toEqual([]);
  });

  test("update now: installed outcome shows restart hint", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    let applyCalled = false;
    await stubStatus(page, {
      lastCheckStatus: "ok",
      lastCheckResult: {
        state: "updateAvailable",
        availableVersion: "1.2.0",
        currentVersion: "1.1.0",
        artifactName: "momos-music-manager-1.2.0-linux-x64.tar.gz",
      },
      updateAvailable: true,
    });
    await page.route("**/api/update/apply", (route) => {
      applyCalled = true;
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          data: { outcome: "installed", newVersion: "1.2.0", oldVersion: "1.1.0", restartNeeded: true },
        }),
      });
    });

    await page.goto("/#settings");
    await page.waitForSelector("#update-apply-now-btn", { timeout: 8000 });
    await expect(page.locator("#update-apply-now-btn")).toBeVisible();
    await expect(page.locator("#update-apply-now-btn")).toBeEnabled();

    await page.click("#update-apply-now-btn");
    // Confirm modal
    await page.click('[data-modal-action="confirm"]');

    await expect(page.locator("#settings-update-inline")).toContainText("Restart the server", {
      timeout: 8000,
    });
    expect(applyCalled).toBe(true);
    expect(errors).toEqual([]);
  });

  test("update now: downloaded outcome shows DMG path + instructions", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await stubStatus(page, {
      artifact: { osArch: "macos-universal", ext: "dmg" },
      platformSelfInstall: false,
      lastCheckStatus: "ok",
      lastCheckResult: {
        state: "updateAvailable",
        availableVersion: "1.2.0",
        currentVersion: "1.1.0",
        artifactName: "momos-music-manager-1.2.0-macos-universal.dmg",
      },
      updateAvailable: true,
    });
    await page.route("**/api/update/apply", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          data: {
            outcome: "downloaded",
            path: "/Users/x/Downloads/momos-music-manager-1.2.0-macos-universal.dmg",
            version: "1.2.0",
          },
        }),
      }),
    );

    await page.goto("/#settings");
    await page.waitForSelector("#update-apply-now-btn", { timeout: 8000 });
    await page.click("#update-apply-now-btn");
    await page.click('[data-modal-action="confirm"]');

    await expect(page.locator("#settings-update-inline")).toContainText("Downloads", {
      timeout: 8000,
    });
    await expect(page.locator("#settings-update-inline")).toContainText(
      "momos-music-manager-1.2.0-macos-universal.dmg",
    );
    expect(errors).toEqual([]);
  });

  test("channel mismatch: explain text, no apply button", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await stubStatus(page, {
      channel: "rolling",
      lastCheckStatus: "ok",
      lastCheckResult: {
        state: "channelMismatch",
        availableVersion: "2.0.0",
        currentVersion: "1.1.0-dev+abc1234",
        artifactName: null,
      },
      updateAvailable: false,
    });

    await page.goto("/#settings");
    await page.waitForSelector("#settings-updates-content", { timeout: 8000 });

    await expect(page.locator("#settings-updates-content")).toContainText("Channel mismatch");
    // The source serves the other channel than selected.
    await expect(page.locator("#settings-updates-content")).toContainText(
      "serves a stable release",
    );
    await expect(page.locator("#update-apply-now-btn")).not.toBeVisible();

    expect(errors).toEqual([]);
  });
});

test.describe("Settings page — auto-apply interval (Phase C)", () => {
  test("interval select shows the effective value and auto-restart hint", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await stubStatus(page, { enabled: true });
    await page.goto("/#settings");
    await page.waitForSelector("#autoupdate-interval-select", {
      state: "attached",
      timeout: 8000,
    });

    const select = page.locator("#autoupdate-interval-select");
    await expect(select).toBeVisible();
    await expect(select).not.toBeDisabled();
    await expect(select).toHaveValue("14400");
    // Default preset labelled + auto-restart semantics visible.
    await expect(page.locator("#settings-updates-content")).toContainText(
      "Every 4 hours (default)",
    );
    await expect(page.locator("#settings-updates-content")).toContainText(
      "server restarts itself",
    );

    expect(errors).toEqual([]);
  });

  test("interval select shows custom value when pinned by env", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await stubStatus(page, {
      autoApplyIntervalSecs: 7200,
      autoApplyIntervalSource: "env",
    });
    await page.goto("/#settings");
    await page.waitForSelector("#autoupdate-interval-select", {
      state: "attached",
      timeout: 8000,
    });

    const select = page.locator("#autoupdate-interval-select");
    await expect(select).toBeDisabled();
    await expect(select).toHaveValue("7200");
    await expect(page.locator("#settings-updates-content")).toContainText(
      "MOMOS_AUTOUPDATE_INTERVAL_SECS",
    );

    expect(errors).toEqual([]);
  });

  test("interval select disabled with override hint when intervalSource is toml", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await stubStatus(page, { autoApplyIntervalSource: "toml" });
    await page.goto("/#settings");
    await page.waitForSelector("#autoupdate-interval-select", {
      state: "attached",
      timeout: 8000,
    });

    await expect(page.locator("#autoupdate-interval-select")).toBeDisabled();
    await expect(page.locator("#settings-updates-content")).toContainText(
      "interval_secs",
    );
    await expect(page.locator("#settings-updates-content")).toContainText("config.toml");

    expect(errors).toEqual([]);
  });

  test("interval persists via the real settings API across reload", async ({ page }) => {
    test.setTimeout(60_000);
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    const settingsCalls = [];
    await page.route("**/api/update/settings", (route) => {
      settingsCalls.push(route.request().postDataJSON());
      return route.continue();
    });
    const SLOW_WAIT = 30_000;

    await page.goto("/#settings");
    await page.waitForSelector("#autoupdate-interval-select", {
      state: "attached",
      timeout: SLOW_WAIT,
    });

    // Normalize to the default first (a previous run may have persisted a
    // custom interval in test-playwright.db).
    const select = page.locator("#autoupdate-interval-select");
    const current = await select.inputValue();
    if (current !== "14400") {
      await select.selectOption("14400");
      await expect.poll(() => settingsCalls.length).toBeGreaterThan(0);
    }

    // Switch to "every hour" — real POST, verify the request body.
    await select.selectOption("3600");
    await expect.poll(() => settingsCalls.length).toBeGreaterThan(0);
    expect(settingsCalls[settingsCalls.length - 1]).toEqual({
      autoApplyIntervalSecs: 3600,
    });
    await expect(select).toHaveValue("3600");

    // Reload: the interval must stay (persisted via the real settings API
    // and reflected by GET /api/update/status).
    await page.reload();
    await page.waitForSelector("#autoupdate-interval-select", {
      state: "attached",
      timeout: SLOW_WAIT,
    });
    await expect(page.locator("#autoupdate-interval-select")).toHaveValue("3600");
    await expect(page.locator("#settings-updates-content")).toContainText("Every hour");

    // Restore the default so later runs start clean.
    await page.locator("#autoupdate-interval-select").selectOption("14400");
    await expect.poll(() => settingsCalls.length).toBeGreaterThan(0);

    expect(errors).toEqual([]);
  });

  test("interval change to off disables the periodic loop label", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await stubStatus(page, { enabled: true });
    await page.route("**/api/update/settings", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          data: {
            autoApplyIntervalSecs: 0,
            autoApplyIntervalSource: "ui",
          },
        }),
      }),
    );

    await page.goto("/#settings");
    await page.waitForSelector("#autoupdate-interval-select", {
      state: "attached",
      timeout: 8000,
    });
    const select = page.locator("#autoupdate-interval-select");
    await select.selectOption("0");

    await expect(select).toHaveValue("0");
    await expect(page.locator("#settings-updates-content")).toContainText(
      "Off (manual updates only)",
    );
    await expect(page.locator("#settings-updates-content")).toContainText(
      "applied manually",
    );
    await expect(page.locator("#settings-updates-content")).toContainText(
      "automatic applying is off",
    );

    expect(errors).toEqual([]);
  });
});
