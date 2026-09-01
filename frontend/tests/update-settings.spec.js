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
      channel: "dev",
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
    await expect(page.locator("#settings-updates-content")).toContainText("dev");
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
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    const settingsCalls = [];
    await page.route("**/api/update/settings", (route) => {
      settingsCalls.push(route.request().postDataJSON());
      // Continue to the real backend — the toggle must really persist in
      // test-playwright.db so the reload assertion is meaningful.
      return route.continue();
    });

    await page.goto("/#settings");
    await page.waitForSelector("#autoupdate-toggle", { state: "attached", timeout: 8000 });

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
    await page.waitForSelector("#autoupdate-toggle", { state: "attached", timeout: 8000 });
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
    await expect(page.locator("#settings-updates-content")).toContainText(
      "never auto-update across channels",
    );
    await expect(page.locator("#update-apply-now-btn")).not.toBeVisible();

    expect(errors).toEqual([]);
  });
});
