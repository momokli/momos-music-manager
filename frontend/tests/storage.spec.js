import { test, expect } from "@playwright/test";

test.describe("Storage Page Backfill", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("/api/testing/seed", {
      data: { scenario: "basic" },
    });
  });

  test("shows backfill backup sizes section", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await page.goto("/#storage");
    await page.waitForSelector("#backfill-section", { timeout: 8000 });
    await expect(page.locator("#backfill-section")).toBeVisible();
    await expect(page.locator("#btn-backfill")).toBeVisible();

    expect(errors).toEqual([]);
  });

  test("backfill button triggers API call and shows result", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await page.goto("/#storage");
    await page.waitForSelector("#btn-backfill", { timeout: 8000 });

    // Click the backfill button
    await page.click("#btn-backfill");

    // Wait for the status to be non-empty (task started, no records, or error)
    await expect(page.locator("#backfill-status")).not.toBeEmpty({
      timeout: 10000,
    });

    // Button should be re-enabled after completion
    await expect(page.locator("#btn-backfill")).toBeEnabled({ timeout: 10000 });

    // Either shows "No records need backfill" or task started or error
    const statusText = await page.locator("#backfill-status").textContent();
    const validStatuses = ["No records need backfill", "Task started", "Error", "Failed"];
    const hasValidStatus = validStatuses.some((s) => statusText.includes(s));
    expect(hasValidStatus).toBeTruthy();

    expect(errors).toEqual([]);
  });
});

test.describe("Storage Page - Ghost Records", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("/api/testing/seed", {
      data: { scenario: "basic" },
    });
  });

  test("ghost card hidden when no orphans", async ({ page }) => {
    // Intercept status to return 0 orphans
    await page.route("**/api/storage/status", async (route) => {
      const response = await route.fetch();
      const json = await response.json();
      json.data.orphanedFileCount = 0;
      await route.fulfill({ response, json });
    });

    const errors = [];
    page.on("pageerror", (err) => errors.push(err));
    await page.goto("/#storage");
    // Wait for status cards to render
    await page.waitForSelector(".storage-card", { timeout: 8000 });
    // With 0 orphans, card should not exist
    await expect(page.locator("#orphan-card")).toHaveCount(0);
    expect(errors).toEqual([]);
  });

  test("ghost card visible with orphans", async ({ page }) => {
    // Intercept status to return > 0 orphans
    await page.route("**/api/storage/status", async (route) => {
      const response = await route.fetch();
      const json = await response.json();
      json.data.orphanedFileCount = 3;
      await route.fulfill({ response, json });
    });

    const errors = [];
    page.on("pageerror", (err) => errors.push(err));
    await page.goto("/#storage");
    await page.waitForSelector("#orphan-card", { timeout: 8000 });
    await expect(page.locator("#orphan-card")).toBeVisible();
    // Should show count > 0
    const countEl = page.locator("#orphan-card .metric-value").first();
    await expect(countEl).toHaveText("3");
    expect(errors).toEqual([]);
  });

  test("purge button shows confirmation dialog", async ({ page }) => {
    // Intercept status to return > 0 orphans
    await page.route("**/api/storage/status", async (route) => {
      const response = await route.fetch();
      const json = await response.json();
      json.data.orphanedFileCount = 3;
      await route.fulfill({ response, json });
    });

    await page.goto("/#storage");
    await page.waitForSelector("#purge-orphans-btn", { timeout: 8000 });

    // Listen for dialog
    let dialogAccepted = false;
    page.on("dialog", (dialog) => {
      dialogAccepted = true;
      dialog.accept();
    });

    await page.locator("#purge-orphans-btn").click();
    // Should trigger a confirm() dialog
    expect(dialogAccepted).toBeTruthy();
  });

  test("purge succeeds and card disappears", async ({ page }) => {
    // Intercept status to return > 0 orphans
    await page.route("**/api/storage/status", async (route) => {
      const response = await route.fetch();
      const json = await response.json();
      json.data.orphanedFileCount = 3;
      await route.fulfill({ response, json });
    });

    // Intercept purge API to return success
    let purgeCalled = false;
    await page.route("**/api/storage/purge-orphans", async (route) => {
      // Only intercept POST requests
      if (route.request().method() === "POST") {
        purgeCalled = true;
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({ data: { purged: 3 } }),
        });
      } else {
        await route.fallback();
      }
    });

    await page.goto("/#storage");
    await page.waitForSelector("#orphan-card", { timeout: 8000 });
    await expect(page.locator("#orphan-card")).toBeVisible();

    // Click purge, accept confirmation
    page.on("dialog", (dialog) => dialog.accept());
    await page.locator("#purge-orphans-btn").click();

    // Wait for the card to be removed (via JS: card.remove())
    await page.waitForSelector("#orphan-card", { state: "detached", timeout: 8000 });
    expect(purgeCalled).toBeTruthy();
  });
});
