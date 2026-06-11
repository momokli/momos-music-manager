import { test, expect } from "@playwright/test";

test.describe("Backpack Page", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("http://localhost:3000/api/testing/seed", {
      data: { scenario: "basic" },
    });
  });

  test("renders without console errors", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (err) => errors.push(err.message));

    await page.goto("/#/backpack");
    await expect(page.locator('[data-page="backpack"]')).toBeVisible({ timeout: 8000 });
    expect(errors).toEqual([]);
  });

  test("shows backpack tags", async ({ page }) => {
    await page.goto("/#/backpack");
    // Tags with backpack=true should be listed
    await expect(page.locator('[data-backpack-tags]')).toBeVisible();
  });

  test("shows track status cards", async ({ page }) => {
    await page.goto("/#/backpack");
    // Track cards should show file status
    await expect(page.locator('[data-track-status]')).toBeVisible();
  });

  test("sync button exists", async ({ page }) => {
    await page.goto("/#/backpack");
    await expect(page.locator('[data-action="sync-backpack"]')).toBeVisible();
  });

  test("pull missing button exists", async ({ page }) => {
    await page.goto("/#/backpack");
    await expect(page.locator('[data-action="pull-missing"]')).toBeVisible();
  });
});
