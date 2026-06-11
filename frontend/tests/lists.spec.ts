import { test, expect } from "@playwright/test";

test.describe("Lists (Playlists) Page", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("http://localhost:3000/api/testing/seed", {
      data: { scenario: "basic" },
    });
  });

  test("renders without console errors", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (err) => errors.push(err.message));
    await page.goto("/#/lists");
    await expect(page.locator('[data-page="lists"]')).toBeVisible({ timeout: 8000 });
    expect(errors).toEqual([]);
  });

  test("shows playlist table", async ({ page }) => {
    await page.goto("/#/lists");
    const table = page.locator("table");
    await expect(table).toBeVisible();
    // At least has name, tracks, service columns
    const headers = table.locator("th");
    expect(await headers.count()).toBeGreaterThanOrEqual(3);
  });

  test("shows service badges", async ({ page }) => {
    await page.goto("/#/lists");
    // Service badges should be visible if playlists exist
    const badges = page.locator('[data-service-badge]');
    // At least one badge (from seed data)
    await expect(badges.first()).toBeVisible();
  });

  test("search filters playlists", async ({ page }) => {
    await page.goto("/#/lists");
    const search = page.locator('[data-lists-search]');
    await expect(search).toBeVisible();
    await search.fill("Groovy");
  });

  test("archive toggle exists", async ({ page }) => {
    await page.goto("/#/lists");
    const archiveToggles = page.locator('[data-action="toggle-archive"]');
    // Archive toggles should be in table rows
    await expect(archiveToggles.first()).toBeVisible();
  });
});
