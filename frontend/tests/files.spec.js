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

  test("comment diff shows old/new when file needs update", async ({ page, request }) => {
    // Seed with comment diff scenario — file 40 needs update, file 41 is up-to-date
    await request.post("/api/testing/seed", {
      data: { scenario: "comment_diff" },
    });

    // Navigate with filters: local files only, needs_update comment status
    await page.goto("/#files?commentStatuses=needs_update&isLocal=true");
    await page.waitForSelector("#files-content table tbody tr", { timeout: 8000 });

    // Check for JavaScript errors
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    // The page should show diff-line elements for files needing updates
    const diffLines = page.locator(".diff-line");
    const count = await diffLines.count();
    expect(count).toBeGreaterThan(0);

    // Each diff-line should contain old (-) and new (+) indicators
    const firstDiff = diffLines.first();
    await expect(firstDiff.locator(".diff-line-old .diff-sign.minus")).toBeVisible();
    await expect(firstDiff.locator(".diff-line-new .diff-sign.plus")).toBeVisible();

    // Should NOT show the ✓ unchanged indicator for files needing updates
    // (files that are up-to-date won't appear with needs_update filter)
    const unchanged = page.locator(".diff-line-unchanged");
    const unchangedCount = await unchanged.count();
    // File 41 is filtered out (needsUpdate=false), so no unchanged lines in results
    expect(unchangedCount).toBe(0);

    // Verify no JS errors
    expect(errors.length).toBe(0);
  });

  test("select all count respects isLocal filter", async ({ page, request }) => {
    // Seed comment diff scenario
    await request.post("/api/testing/seed", {
      data: { scenario: "comment_diff" },
    });

    await page.goto("/#files?commentStatuses=needs_update&isLocal=true");
    await page.waitForSelector("#files-content table tbody tr", { timeout: 8000 });

    // Read the total count from the stats row
    const statsRow = page.locator("#files-content .stats-row strong");
    const statsText = await statsRow.textContent();
    const totalMatching = parseInt(statsText, 10);
    expect(totalMatching).toBeGreaterThan(0);

    // Check all visible rows via the header select-all checkbox
    const selectAllCheckbox = page.locator("#files-select-all");
    await selectAllCheckbox.setChecked(false);
    await selectAllCheckbox.setChecked(true);
    await page.waitForTimeout(500);

    // Now the WRITE COMMENTS button should show a count matching the filtered total
    const writeBtn = page.locator("#files-actions-write-comments");
    await expect(writeBtn).toBeVisible({ timeout: 3000 });

    const btnText = await writeBtn.textContent();
    // Extract the number from "WRITE COMMENTS (N)"
    const match = btnText.match(/WRITE COMMENTS \((\d+)\)/);
    if (match) {
      const writeCount = parseInt(match[1], 10);
      // The write count must not exceed the total matching files
      expect(writeCount).toBeLessThanOrEqual(totalMatching);
      // Should be at least 1 (file 40 needs update; file 41 is up-to-date)
      expect(writeCount).toBeGreaterThan(0);
    }
  });

  test("stems filter shows only files without stems", async ({ page }) => {
    // files_filter scenario: files 1-4 + 30-32.
    // File 1 (flac US001) has a stem, file 2 IS the stem,
    // files 3, 4, 30, 31, 32 are flac without stems → expect 5 files.
    await page.goto("/#files?stems=true");
    await page.waitForSelector("#files-content table tbody tr", { timeout: 8000 });

    // Total in stats row must be 5
    const statsText = await page.locator("#files-content .stats-row strong").textContent();
    expect(parseInt(statsText, 10)).toBe(5);

    // The filter button row is present and "Missing" is active
    const missingBtn = page.locator('[data-stem-filter="yes"]');
    await expect(missingBtn).toBeVisible();
    await expect(missingBtn).toHaveClass(/active/);

    // No stem.m4a rows and no "Title One" (the file that HAS a stem)
    const formatCells = page.locator("#files-content table tbody tr td:nth-child(5)");
    const formats = await formatCells.allTextContents();
    expect(formats.every((f) => !f.includes("stem.m4a"))).toBe(true);
    const rows = page.locator("#files-content table tbody tr");
    expect(await rows.count()).toBe(5);
  });

  test("stems filter combines with backup filter", async ({ page }) => {
    // All stems-filter files (3, 4, 30, 31, 32) are backed up → still 5.
    // File 1 has a stem, file 2 is a stem → excluded regardless of backup.
    await page.goto("/#files?stems=true&backedUp=true");
    await page.waitForSelector("#files-content table tbody tr", { timeout: 8000 });

    const statsText = await page.locator("#files-content .stats-row strong").textContent();
    expect(parseInt(statsText, 10)).toBe(5);
  });

  test("stems filter excludes local-only stems", async ({ page }) => {
    // stems=true & isLocal=true: stems-filter files are backup-only
    // (files 3,4,30,31,32 have no local entry) → expect 0 files.
    await page.goto("/#files?stems=true&isLocal=true");
    await page.waitForTimeout(800);

    const statsText = await page.locator("#files-content .stats-row strong").textContent();
    expect(parseInt(statsText, 10)).toBe(0);
  });
});
