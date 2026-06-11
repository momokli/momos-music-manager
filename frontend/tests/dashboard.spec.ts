import { test, expect } from "@playwright/test";

test.describe("Dashboard Page", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("http://localhost:3000/api/testing/seed", {
      data: { scenario: "basic" },
    });
  });

  test("renders without console errors", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (err) => errors.push(err.message));

    await page.goto("/");
    await expect(page.locator('[data-page="dashboard"]')).toBeVisible({
      timeout: 8000,
    });
    expect(errors).toEqual([]);
  });

  test("shows stats cards", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator('[data-stat="files"]')).toBeVisible();
    await expect(page.locator('[data-stat="tracks"]')).toBeVisible();
    await expect(page.locator('[data-stat="playlists"]')).toBeVisible();
    await expect(page.locator('[data-stat="tags"]')).toBeVisible();
  });

  test("shows service status", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator("[data-service-status]")).toBeVisible();
  });

  test("shows recent activity", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator("[data-recent-activity]")).toBeVisible();
  });

  test("quick action buttons work", async ({ page }) => {
    await page.goto("/");
    const syncBtn = page.locator('[data-action="sync-all"]');
    await expect(syncBtn).toBeVisible();
    await expect(page.locator('[data-action="go-to-files"]')).toBeVisible();
  });
});
