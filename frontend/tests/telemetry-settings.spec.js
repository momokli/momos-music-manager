import { test, expect } from "@playwright/test";

/**
 * Telemetry-settings view: status card (effective values + sources), save
 * into config.toml, "Push now" flow with inline result, CLI-access card.
 *
 * API responses are stubbed (no writes to the real ~/.config happen in
 * these tests); the dirty-tracking and request payloads are asserted
 * against the real fetch calls the page makes.
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

async function stubRoutes(page, overrides) {
  await page.route("**/api/update/status", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        data: {
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
        },
      }),
    }),
  );
  await page.route("**/api/telemetry-settings/status", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ data: statusStub(overrides) }),
    }),
  );
}

test.describe("Settings page — telemetry + CLI", () => {
  test("telemetry card shows defaults (off) and CLI link", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await stubRoutes(page);
    await page.goto("/#settings");
    await page.waitForSelector("#settings-telemetry-card", { timeout: 8000 });
    await page.waitForSelector("#telemetry-enabled-toggle", { timeout: 8000 });

    // Default: OFF, save disabled (nothing dirty yet), push disabled.
    await expect(page.locator("#telemetry-enabled-toggle")).not.toBeChecked();
    await expect(page.locator("#telemetry-save-btn")).toBeDisabled();
    await expect(page.locator("#telemetry-push-btn")).toBeDisabled();
    // Last push: never.
    await expect(page.locator("#settings-telemetry-content")).toContainText("never");
    // CLI card shows the link + command.
    await expect(page.locator("#settings-cli-content")).toContainText("/usr/local/bin/momos-music-manager");
    await expect(page.locator("#settings-cli-content")).toContainText("momos-music-manager --version");
    expect(errors).toEqual([]);
  });

  test("enabling telemetry saves {enabled:true} and enables push", async ({
    page,
  }) => {
    await stubRoutes(page, {
      // After the save the server responds with the new effective state.
    });
    let savePayload = null;
    let pushCalled = false;
    let statusCalls = 0;

    await page.route("**/api/telemetry-settings/status", (route) => {
      statusCalls += 1;
      // First call: current state (off). Later calls (after save/push):
      // enabled.
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          data: statusStub({
            enabled: statusCalls > 1,
            enabledSource: statusCalls > 1 ? "toml" : "default",
            baseUrl: statusCalls > 1 ? "https://collector.example" : null,
            baseUrlSource: statusCalls > 1 ? "toml" : "default",
            lastPushAt: statusCalls > 2 ? 1760000000 : null,
            lastPushStatus: statusCalls > 2 ? "ok" : null,
          }),
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

    await page.goto("/#settings");
    await page.waitForSelector("#telemetry-enabled-toggle", { timeout: 8000 });

    await page.locator("#telemetry-enabled-toggle").check();
    await expect(page.locator("#telemetry-save-btn")).toBeEnabled();
    await page.locator("#telemetry-save-btn").click();
    await expect
      .poll(() => savePayload)
      .toEqual({ enabled: true });

    // After save the refetched status says enabled → push button active.
    await expect(page.locator("#telemetry-push-btn")).toBeEnabled();
    await page.locator("#telemetry-push-btn").click();
    await expect.poll(() => pushCalled).toBe(true);
    // Success inline + refetched last-push state shows the timestamp.
    await expect(page.locator("#settings-telemetry-content")).toContainText("Push succeeded");
  });

  test("changed interval is sent as fullDbIntervalSecs on save", async ({
    page,
  }) => {
    let savePayload = null;
    await stubRoutes(page, {
      fullDbIntervalSecs: 3600,
      fullDbIntervalSource: "toml",
    });
    await page.route("**/api/telemetry-settings/settings", (route) => {
      savePayload = route.request().postDataJSON();
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ data: statusStub() }),
      });
    });

    await page.goto("/#settings");
    await page.waitForSelector("#telemetry-interval", { timeout: 8000 });
    await page.locator("#telemetry-interval").fill("7200");
    await page.locator("#telemetry-save-btn").click();
    await expect.poll(() => savePayload).toEqual({ fullDbIntervalSecs: 7200 });
  });

  test("env-pinned fields are disabled with a hint", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await stubRoutes(page, {
      enabled: true,
      enabledSource: "env",
      baseUrl: "https://env.example",
      baseUrlSource: "env",
    });
    await page.goto("/#settings");
    await page.waitForSelector("#telemetry-enabled-toggle", { timeout: 8000 });

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
