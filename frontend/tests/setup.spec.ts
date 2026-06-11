import { test, expect } from "@playwright/test";

test.describe("Setup Hub Page", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("http://localhost:3000/api/testing/seed", {
      data: { scenario: "basic" },
    });
  });

  test("renders without console errors", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (err) => errors.push(err.message));
    await page.goto("/#/setup");
    await expect(page.locator('[data-page="setup"]')).toBeVisible({
      timeout: 8000,
    });
    expect(errors).toEqual([]);
  });

  test("renders setup cards", async ({ page }) => {
    await page.goto("/#/setup");
    const cards = page.locator("[data-setup-card]");
    await expect(cards.first()).toBeVisible();
    expect(await cards.count()).toBeGreaterThanOrEqual(4);
  });

  test("services card shows connection status", async ({ page }) => {
    await page.goto("/#/setup");
    const servicesCard = page.locator('[data-setup-card="services"]');
    await expect(servicesCard).toBeVisible();
    await expect(
      servicesCard.locator('[data-service="spotify"]'),
    ).toBeVisible();
    await expect(
      servicesCard.locator('[data-service="soundcloud"]'),
    ).toBeVisible();
  });

  test("folders card shows folder status", async ({ page }) => {
    await page.goto("/#/setup");
    const foldersCard = page.locator('[data-setup-card="folders"]');
    await expect(foldersCard).toBeVisible();
    await expect(
      foldersCard.locator('[data-action="scan-folder"]').first(),
    ).toBeVisible();
  });

  test("storage card shows stats", async ({ page }) => {
    await page.goto("/#/setup");
    const storageCard = page.locator('[data-setup-card="storage"]');
    await expect(storageCard).toBeVisible();
    await expect(
      storageCard.locator('[data-stat="local-files"]'),
    ).toBeVisible();
  });

  test("tasks card shows tasks", async ({ page }) => {
    await page.goto("/#/setup");
    const tasksCard = page.locator('[data-setup-card="tasks"]');
    await expect(tasksCard).toBeVisible();
  });

  test("import/export card has action buttons", async ({ page }) => {
    await page.goto("/#/setup");
    const dataCard = page.locator('[data-setup-card="data"]');
    await expect(dataCard).toBeVisible();
    await expect(dataCard.locator('[data-action="export"]')).toBeVisible();
    await expect(dataCard.locator('[data-action="import"]')).toBeVisible();
  });

  test("key comparison card exists", async ({ page }) => {
    await page.goto("/#/setup");
    await expect(
      page.locator('[data-setup-card="key-comparison"]'),
    ).toBeVisible();
  });
});
