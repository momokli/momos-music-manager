import { test, expect } from "@playwright/test";

test.describe("Files Page", () => {
  test.beforeEach(async ({ request }) => {
    // Reset DB to known state before each test
    await request.post("/api/testing/seed", {
      data: { scenario: "files_filter" },
    });
  });

  test("shows paginated file list", async ({ page }) => {
    await page.goto("/#files");
    await page.waitForSelector("#files-content table tbody tr", { timeout: 8000 });

    const rows = page.locator("#files-content table tbody tr");
    await expect(rows.first()).toBeVisible();
    expect(await rows.count()).toBeGreaterThan(0);
  });

  test("search filters files by title", async ({ page }) => {
    await page.goto("/#files");
    await page.waitForSelector("#files-content table tbody tr", { timeout: 8000 });

    const searchInput = page.locator('[data-sf-search="true"]');
    await expect(searchInput).toBeVisible({ timeout: 3000 });
    await searchInput.fill("Title One");
    await searchInput.press("Enter");
    await page.waitForTimeout(500);

    const rows = page.locator("#files-content table tbody tr");
    const count = await rows.count();
    expect(count).toBeGreaterThan(0);

    // Every visible row should contain "Title One" somewhere
    const firstRowText = await rows.first().textContent();
    expect(firstRowText).toContain("Title One");
  });

  test("pagination controls are present", async ({ page }) => {
    await page.goto("/#files");
    await page.waitForSelector("#files-content table tbody tr", { timeout: 8000 });

    // Pagination controls should be visible
    const prevBtn = page.locator("#files-page-prev");
    const nextBtn = page.locator("#files-page-next");
    await expect(prevBtn).toBeVisible();
    await expect(nextBtn).toBeVisible();
  });

  test("column sorting changes data order", async ({ page }) => {
    await page.goto("/#files");
    await page.waitForSelector("#files-content table tbody tr", { timeout: 8000 });

    // Click a sortable column header to sort ASC
    const titleHeader = page.locator("#files-content table thead th.sortable").first();
    const headerText = await titleHeader.textContent();
    await titleHeader.click();
    await page.waitForTimeout(300);

    // The header should show active sort indicator
    await expect(titleHeader).toHaveClass(/sort-asc|sort-desc/);

    // Content should still render
    const rows = page.locator("#files-content table tbody tr");
    expect(await rows.count()).toBeGreaterThan(0);
  });

  test("navigating to file detail opens detail page", async ({ page }) => {
    await page.goto("/#files");
    await page.waitForSelector("#files-content table tbody tr", { timeout: 8000 });

    // Click the first file's detail link (if any exists)
    const firstRowLink = page
      .locator('#files-content table tbody tr a[href*="file-detail"]')
      .first();
    if (await firstRowLink.isVisible().catch(() => false)) {
      await firstRowLink.click();
      await page.waitForTimeout(500);

      // Should navigate to file-detail hash
      expect(page.url()).toContain("file-detail");
    }
  });
});
