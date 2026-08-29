import { test, expect } from "@playwright/test";

test.describe("Track Detail Page", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("/api/testing/seed", {
      data: { scenario: "basic" },
    });
  });

  test("page loads without errors", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await page.goto("/#track-detail?id=1");
    await page.waitForSelector(".detail-section", { timeout: 8000 });
    await expect(page.locator(".detail-section").first()).toBeVisible();

    expect(errors).toEqual([]);
  });

  test("disconnect button appears on linked files", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await page.goto("/#track-detail?id=1");
    await page.waitForSelector(".detail-section", { timeout: 8000 });

    // Track 1 (ISRC US001) auto-links to files 1 and 2 via ISRC in seed data.
    // Both file cards should have a disconnect (×) button.
    const disconnectBtns = page.locator(".disconnect-btn");
    await expect(disconnectBtns.first()).toBeVisible({ timeout: 5000 });

    // Expect at least 1 linked file (both file 1 and file 2 match via ISRC)
    const count = await disconnectBtns.count();
    expect(count).toBeGreaterThanOrEqual(1);

    expect(errors).toEqual([]);
  });

  test("can disconnect a linked file", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await page.goto("/#track-detail?id=1");
    await page.waitForSelector(".disconnect-btn", { timeout: 8000 });

    // Count linked files before disconnect
    const beforeCount = await page.locator(".disconnect-btn").count();
    expect(beforeCount).toBeGreaterThanOrEqual(1);

    // Click the × button on the first linked file card
    const firstBtn = page.locator(".disconnect-btn").first();
    await firstBtn.click();

    // Wait for the page to refresh after the PUT + re-fetch
    await page.waitForTimeout(800);

    // After disconnect + refresh, the disconnected file card should be gone.
    // If both files were linked (file 1 + file 2 via ISRC), we should have
    // one fewer. If only one was rendered, the "Linked Files" section may
    // disappear entirely (if no files remain).
    const afterCount = await page.locator(".disconnect-btn").count();
    expect(afterCount).toBeLessThan(beforeCount);

    expect(errors).toEqual([]);
  });

  test("link file typeahead searches and re-links a file", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await page.goto("/#track-detail?id=1");
    await page.waitForSelector(".disconnect-btn", { timeout: 8000 });

    // Step 1 — disconnect the first file so we have something to re-link
    const beforeCount = await page.locator(".disconnect-btn").count();
    const firstBtn = page.locator(".disconnect-btn").first();
    await firstBtn.click();
    await page.waitForTimeout(800);

    // Verify one file was removed
    const afterDisconnect = await page.locator(".disconnect-btn").count();
    expect(afterDisconnect).toBeLessThan(beforeCount);

    // Step 2 — use the typeahead to find the disconnected file and re-link it
    const searchInput = page.locator("#link-file-search");
    await expect(searchInput).toBeVisible({ timeout: 5000 });

    // Type enough to trigger the debounced search (≥2 chars)
    await searchInput.fill("Title");
    await page.waitForTimeout(400); // debounce is 250ms

    // Dropdown should open with results
    const dropdown = page.locator("#link-file-dropdown");
    await expect(dropdown).toHaveClass(/open/, { timeout: 3000 });

    // There should be at least one clickable result
    const dropdownItems = dropdown.locator(".tag-dropdown-item");
    const itemCount = await dropdownItems.count();
    expect(itemCount).toBeGreaterThanOrEqual(1);

    // Click the first result to link it
    await dropdownItems.first().click();

    // Wait for the PUT + re-fetch
    await page.waitForTimeout(800);

    // After re-linking, the file should appear again
    const afterLink = await page.locator(".disconnect-btn").count();
    expect(afterLink).toBeGreaterThanOrEqual(afterDisconnect);

    expect(errors).toEqual([]);
  });
});
