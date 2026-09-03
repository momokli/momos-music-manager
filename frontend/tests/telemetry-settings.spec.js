import { test, expect } from "@playwright/test";

/**
 * Telemetry-settings view: status card (effective values + sources), save
 * into config.toml, "Push now" flow with inline result, CLI-access card.
 *
 * API responses are stubbed (no writes to the real ~/.config happen in
 * these tests); the dirty-tracking and request payloads are asserted
 * against the real fetch calls the page makes.
 *
 * Conventions: the toggle's native input is visually hidden by the switch
 * CSS (state: "attached", click the label). Every settings page has TWO
 * switches now (Updates + Telemetry) — selectors are scoped per card.
 */

function statusStub(overrides = {}) {
  return {
    currentVersion: "1.3.0",
    enabled: false,
    enabledSource: "default",
    baseUrl: null,
    baseUrlSource: "default",
    token: null,
    tokenSource: "default",
    instance: "macbook",
    instanceSource: "default",
    fullDbIntervalSecs: 0,
    fullDbIntervalSource: "default",
    eventsEndpoint: null,
    periodicPushActive: false,
    lastPushAt: null,
    lastPushStatus: null,
    lastPushError: null,
    cli: {
      supported: true,
      reason: null,
      linkPath: "/usr/local/bin/momos-music-manager",
      targetPath:
        "/Applications/Momo's Music Manager.app/Contents/MacOS/momos-music-manager",
    },
    ...overrides,
  };
}

function updateStatusStub() {
  return {
    currentVersion: "1.3.0",
    channel: "release",
    channelSource: "default",
    availableChannels: ["release", "rolling"],
    baseUrl: "https://example.invalid",
    enabled: false,
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
  };
}

/** Stub the update card (kept out of the way; only the Updates toggle test
 * exercises it against the real backend). */
async function stubUpdateStatus(page) {
  await page.route("**/api/update/status", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ data: updateStatusStub() }),
    }),
  );
}

async function openSettings(page) {
  await stubUpdateStatus(page);
  await page.goto("/#settings");
  await page.waitForSelector("#settings-telemetry-card", { timeout: 8000 });
}

test.describe("Settings page — telemetry + CLI", () => {
  test("telemetry card shows defaults (off) and CLI link", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await page.route("**/api/telemetry-settings/status", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ data: statusStub() }),
      }),
    );
    await openSettings(page);
    await page.waitForSelector("#telemetry-enabled-toggle", {
      state: "attached",
      timeout: 8000,
    });

    // Default: OFF, save disabled (nothing dirty yet), push disabled.
    await expect(page.locator("#telemetry-enabled-toggle")).not.toBeChecked();
    await expect(page.locator("#telemetry-save-btn")).toBeDisabled();
    await expect(page.locator("#telemetry-push-btn")).toBeDisabled();
    // Last push: never.
    await expect(page.locator("#settings-telemetry-content")).toContainText("never");
    // CLI card shows the link + command.
    await expect(page.locator("#settings-cli-content")).toContainText(
      "/usr/local/bin/momos-music-manager",
    );
    await expect(page.locator("#settings-cli-content")).toContainText(
      "momos-music-manager --version",
    );
    expect(errors).toEqual([]);
  });

  test("enabling telemetry saves {enabled:true} and enables push", async ({
    page,
  }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    let savePayload = null;
    let pushCalled = false;
    let statusCalls = 0;

    // One status route: call 1 = initial (off), call 2 = after save
    // (enabled via toml), call 3 = after push (last-push ok state).
    await page.route("**/api/telemetry-settings/status", (route) => {
      statusCalls += 1;
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          data: statusStub(
            statusCalls === 1
              ? {}
              : {
                  enabled: true,
                  enabledSource: "toml",
                  baseUrl: "https://collector.example",
                  baseUrlSource: "toml",
                  lastPushAt: statusCalls >= 3 ? 1760000000 : null,
                  lastPushStatus: statusCalls >= 3 ? "ok" : null,
                },
          ),
        }),
      });
    });
    await page.route("**/api/telemetry-settings/settings", (route) => {
      savePayload = route.request().postDataJSON();
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ data: statusStub() }),
      });
    });
    await page.route("**/api/telemetry-settings/push", (route) => {
      pushCalled = true;
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          data: { ok: true, message: "Push succeeded", pushedAt: 1760000000 },
        }),
      });
    });

    await openSettings(page);
    await page.waitForSelector("#telemetry-enabled-toggle", {
      state: "attached",
      timeout: 8000,
    });

    // Toggle on via the visible switch label, then save.
    await page.locator("#settings-telemetry-content .switch").click();
    await expect(page.locator("#telemetry-enabled-toggle")).toBeChecked();
    await expect(page.locator("#telemetry-save-btn")).toBeEnabled();
    await page.locator("#telemetry-save-btn").click();
    await expect.poll(() => savePayload).toEqual({ enabled: true });

    // After save the refetched status says enabled → push button active.
    await expect(page.locator("#telemetry-push-btn")).toBeEnabled();
    await page.locator("#telemetry-push-btn").click();
    await expect.poll(() => pushCalled).toBe(true);
    // Success inline; the refetched last-push state shows the timestamp.
    await expect(page.locator("#settings-telemetry-content")).toContainText(
      "Push succeeded",
    );
    await expect(page.locator("#settings-telemetry-content")).not.toContainText("never");
    expect(errors).toEqual([]);
  });

  test("changed interval is sent as fullDbIntervalSecs on save", async ({
    page,
  }) => {
    let savePayload = null;
    await page.route("**/api/telemetry-settings/status", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          data: statusStub({
            fullDbIntervalSecs: 3600,
            fullDbIntervalSource: "toml",
          }),
        }),
      }),
    );
    await page.route("**/api/telemetry-settings/settings", (route) => {
      savePayload = route.request().postDataJSON();
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ data: statusStub() }),
      });
    });

    await openSettings(page);
    await page.waitForSelector("#telemetry-interval", { timeout: 8000 });
    await page.locator("#telemetry-interval").fill("7200");
    await page.locator("#telemetry-save-btn").click();
    await expect.poll(() => savePayload).toEqual({ fullDbIntervalSecs: 7200 });
  });

  test("env-pinned fields are disabled with a hint", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await page.route("**/api/telemetry-settings/status", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          data: statusStub({
            enabled: true,
            enabledSource: "env",
            baseUrl: "https://env.example",
            baseUrlSource: "env",
          }),
        }),
      }),
    );
    await openSettings(page);
    await page.waitForSelector("#telemetry-enabled-toggle", {
      state: "attached",
      timeout: 8000,
    });

    await expect(page.locator("#telemetry-enabled-toggle")).toBeDisabled();
    await expect(page.locator("#telemetry-base-url")).toBeDisabled();
    await expect(page.locator("#settings-telemetry-content")).toContainText(
      "MOMOS_TELEMETRY_ENABLED",
    );
    // Nothing editable → save stays disabled.
    await expect(page.locator("#telemetry-save-btn")).toBeDisabled();
    expect(errors).toEqual([]);
  });
});
