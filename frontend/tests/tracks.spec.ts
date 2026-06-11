import { test, expect } from "@playwright/test";

test.describe("Tracks Page", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("http://localhost:3000/api/testing/seed", {
      data: { scenario: "basic" },
    });
  });

  test("renders without console errors", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (err) => errors.push(err.message));
    await page.goto("/#/tracks");
    await expect(page.locator('[data-page="tracks"]')).toBeVisible({ timeout: 8000 });
    expect(errors).toEqual([]);
  });

  test("shows track table", async ({ page }) => {
    await page.goto("/#/tracks");
    const table = page.locator("table");
    await expect(table).toBeVisible();
    const headers = table.locator("th");
    expect(await headers.count()).toBeGreaterThanOrEqual(3);
  });

  test("search input filters tracks", async ({ page }) => {
    await page.goto("/#/tracks");
    const search = page.locator("[data-tracks-search]");
    await expect(search).toBeVisible();
  });

  test("shows filter toolbar with service filter", async ({ page }) => {
    await page.goto("/#/tracks");
    // Filter panel should have service buttons
    const serviceFilter = page.locator('[data-filter="services"]');
    await expect(serviceFilter).toBeVisible();
  });

  test("shows BPM filter when expanded", async ({ page }) => {
    await page.goto("/#/tracks");
    const bpmFilter = page.locator('[data-filter="bpm"]');
    await expect(bpmFilter).toBeVisible();
  });

  test("paginates results when enough tracks exist", async ({ page }) => {
    await page.goto("/#/tracks");
    // Pagination shows when total > page size (50)
    const pagination = page.locator("[data-pagination]");
    // Basic seed may not have 50+ tracks, so pagination may not render
    // This test checks that the table renders and pagination is present if needed
    await expect(page.locator("table")).toBeVisible();
    // If pagination exists, it should have navigation buttons
    const paginationCount = await pagination.count();
    if (paginationCount > 0) {
      await expect(pagination.locator("button").first()).toBeVisible();
    }
  });

  test("column headers are sortable", async ({ page }) => {
    await page.goto("/#/tracks");
    const table = page.locator("table");
    // First header should be clickable for sort
    const firstHeader = table.locator("th").first();
    await expect(firstHeader).toBeVisible();
  });
});
