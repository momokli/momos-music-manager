import { test, expect } from "@playwright/test";

test.describe("File Detail Page", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("/api/testing/seed", {
      data: { scenario: "basic" },
    });
  });

  test("page loads without errors", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await page.goto("/#file-detail?id=1");
    await page.waitForSelector(".detail-section", { timeout: 8000 });
    await expect(page.locator(".detail-section").first()).toBeVisible();

    expect(errors).toEqual([]);
  });

  test("can disconnect a linked track", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await page.goto("/#file-detail?id=1");
    await page.waitForSelector(".detail-section", { timeout: 8000 });

    // File 1 (ISRC US001) auto-links to track 1 (ISRC US001).
    // The "Linked Tracks" section should contain disconnect buttons.
    const disconnectBtns = page.locator(".disconnect-btn");
    await expect(disconnectBtns.first()).toBeVisible({ timeout: 5000 });

    // Count before disconnect
    const beforeCount = await disconnectBtns.count();
    expect(beforeCount).toBeGreaterThanOrEqual(1);

    // Click the × button to exclude the track→file link
    await disconnectBtns.first().click();

    // Wait for the PUT + re-fetch
    await page.waitForTimeout(800);

    // After disconnect + refresh, there should be fewer linked tracks.
    // If this was the only linked track, the entire "Linked Tracks"
    // section disappears (no .disconnect-btn elements).
    const afterCount = await page.locator(".disconnect-btn").count();
    expect(afterCount).toBeLessThan(beforeCount);

    expect(errors).toEqual([]);
  });

  test("shows file metadata without errors", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await page.goto("/#file-detail?id=2");
    await page.waitForSelector(".detail-section", { timeout: 8000 });

    // File 2 is a stem.m4a — verify the page title reflects the file
    const heading = page.locator(".page-header h1");
    await expect(heading).toBeVisible();
    const headingText = await heading.textContent();
    expect(headingText).toContain("Title One");

    // Should show file type badge (use .first() — there may be multiple badges)
    await expect(page.locator(".service-badge").first()).toBeVisible();

    expect(errors).toEqual([]);
  });
});
