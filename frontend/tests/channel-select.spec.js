import { test, expect } from "@playwright/test";

/**
 * Channel select (channel-select feature): the Settings page offers a
 * channel dropdown (`release` | `rolling`) next to the auto-update toggle.
 * Switching channels is confirmed via modal and persisted through
 * POST /api/update/settings ({ channel }); the server clears the stale
 * last-check cache so the card falls back to "Never checked" until the
 * next Check now on the new channel.
 *
 * The Playwright server boots `cargo run serve` against test-playwright.db
 * (fresh per run unless the file persists — tests must not assume the
 * channel's default state and restore it at the end).
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

test.describe("Settings page — channel select", () => {
  test("channel dropdown renders next to the toggle with both channels", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await stubStatus(page, { channel: "rolling", channelSource: "default" });
    await page.goto("/#settings");
    await page.waitForSelector("#update-channel-select", { state: "attached", timeout: 8000 });

    const select = page.locator("#update-channel-select");
    await expect(select).toBeVisible();
    await expect(select).toBeEnabled();
    await expect(select).toHaveValue("rolling");
    const options = page.locator("#update-channel-select option");
    await expect(options).toHaveCount(2);
    await expect(options.nth(0)).toHaveText("Release (stable)");
    await expect(options.nth(1)).toHaveText("Rolling (dev builds of main)");

    // Dropdown sits next to the auto-update toggle (same row).
    await expect(page.locator("#settings-updates-content")).toContainText("Channel");
    await expect(page.locator("#autoupdate-toggle")).toBeAttached();

    expect(errors).toEqual([]);
  });

  test("switching channel requires confirm modal and posts { channel }", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    const settingsCalls = [];
    await stubStatus(page, { channel: "release", channelSource: "default" });
    await page.route("**/api/update/settings", (route) => {
      settingsCalls.push(route.request().postDataJSON());
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          data: {
            autoUpdateEnabled: true,
            enabled: true,
            enabledSource: "default",
            channel: "rolling",
            channelSource: "ui",
          },
        }),
      });
    });

    await page.goto("/#settings");
    await page.waitForSelector("#update-channel-select", { timeout: 8000 });

    // From here on the status endpoint reports the *new* channel (persisted
    // server-side) — the page reloads it after the switch.
    await stubStatus(page, {
      channel: "rolling",
      channelSource: "ui",
      lastCheckAt: null,
      lastCheckStatus: null,
      lastCheckResult: null,
    });

    await page.selectOption("#update-channel-select", "rolling");
    // Confirm modal explains the cross-channel consequence.
    await expect(page.locator("#shared-modal")).toContainText("Switch update channel");
    await expect(page.locator("#shared-modal")).toContainText("rolling");
    await page.click('[data-modal-action="confirm"]');

    await expect
      .poll(() => settingsCalls.length)
      .toBeGreaterThan(0);
    expect(settingsCalls[0]).toEqual({ channel: "rolling" });
    await expect(page.locator("#settings-update-inline")).toContainText(
      "Channel switched to rolling",
      { timeout: 8000 },
    );
    await expect(page.locator("#update-channel-select")).toHaveValue("rolling");

    expect(errors).toEqual([]);
  });

  test("cancelling the modal reverts the dropdown and posts nothing", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    let settingsCalls = 0;
    await stubStatus(page, { channel: "release", channelSource: "default" });
    await page.route("**/api/update/settings", (route) => {
      settingsCalls += 1;
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ data: statusStub() }),
      });
    });

    await page.goto("/#settings");
    await page.waitForSelector("#update-channel-select", { timeout: 8000 });

    await page.selectOption("#update-channel-select", "rolling");
    await page.click('[data-modal-action="cancel"]');

    await expect(page.locator("#update-channel-select")).toHaveValue("release");
    expect(settingsCalls).toBe(0);
    expect(errors).toEqual([]);
  });

  test("dropdown disabled with override hint when channelSource is toml", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await stubStatus(page, { channelSource: "toml" });
    await page.goto("/#settings");
    await page.waitForSelector("#update-channel-select", { state: "attached", timeout: 8000 });

    await expect(page.locator("#update-channel-select")).toBeDisabled();
    await expect(page.locator("#settings-updates-content")).toContainText("[autoupdate] channel");
    await expect(page.locator("#settings-updates-content")).toContainText("config.toml");

    expect(errors).toEqual([]);
  });

  test("dropdown disabled with override hint when channelSource is env", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await stubStatus(page, { channelSource: "env" });
    await page.goto("/#settings");
    await page.waitForSelector("#update-channel-select", { state: "attached", timeout: 8000 });

    await expect(page.locator("#update-channel-select")).toBeDisabled();
    await expect(page.locator("#settings-updates-content")).toContainText(
      "MOMOS_AUTOUPDATE_CHANNEL",
    );

    expect(errors).toEqual([]);
  });

  test("channel mismatch text names the selected channel (explicit switch is not an error)", async ({
    page,
  }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await stubStatus(page, {
      channel: "rolling",
      channelSource: "ui",
      lastCheckStatus: "ok",
      lastCheckResult: {
        state: "updateAvailable",
        availableVersion: "1.1.0-dev+def5678",
        currentVersion: "1.1.0",
        artifactName: "momos-music-manager-1.1.0-dev+def5678-linux-x64.tar.gz",
      },
      updateAvailable: true,
    });

    await page.goto("/#settings");
    await page.waitForSelector("#settings-updates-content", { timeout: 8000 });

    // A release build switched to rolling may apply the dev update: no
    // mismatch state, apply button is offered.
    await expect(page.locator("#settings-updates-content")).toContainText("Update available");
    await expect(page.locator("#update-apply-now-btn")).toBeVisible();
    await expect(page.locator("#update-apply-now-btn")).toBeEnabled();

    expect(errors).toEqual([]);
  });

  test("channel persists across reload via real API and restores the default", async ({
    page,
  }) => {
    // Real-API roundtrips on the sandbox filesystem are slow (SQLite writes
    // take seconds) — give this test room beyond the 15 s default.
    test.setTimeout(60_000);

    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    // The real backend on this machine can be slow under load — generous
    // explicit waits (the 15 s test default is too tight for real-API flows).
    const SLOW_WAIT = 30_000;

    await page.goto("/#settings");
    await page.waitForSelector("#update-channel-select", {
      state: "attached",
      timeout: SLOW_WAIT,
    });

    const select = page.locator("#update-channel-select");
    const setChannel = async (value) => {
      const current = await select.inputValue();
      if (current === value) return;
      await page.selectOption("#update-channel-select", value);
      await page.click('[data-modal-action="confirm"]');
      await expect(page.locator("#settings-update-inline")).toContainText(
        `Channel switched to ${value}`,
        { timeout: SLOW_WAIT },
      );
    };

    // Normalize: the real DB may carry a previous run's value.
    await setChannel("release");
    // Switch to rolling via the real settings API…
    await setChannel("rolling");
    // …and reload: the dropdown must stay on rolling (persisted).
    await page.reload();
    await page.waitForSelector("#update-channel-select", {
      state: "attached",
      timeout: SLOW_WAIT,
    });
    await expect(select).toHaveValue("rolling");

    // Restore the default (release) so later runs stay deterministic.
    await setChannel("release");
    await page.reload();
    await page.waitForSelector("#update-channel-select", {
      state: "attached",
      timeout: SLOW_WAIT,
    });
    await expect(select).toHaveValue("release");

    expect(errors).toEqual([]);
  });
});
