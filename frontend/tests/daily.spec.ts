import { test, expect } from "@playwright/test";

test.describe("Daily Page", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("http://localhost:3000/api/testing/seed", {
      data: { scenario: "basic" },
    });
  });

  test("renders without console errors", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (err) => errors.push(err.message));
    await page.goto("/#/daily");
    await expect(page.locator('[data-page="daily"]')).toBeVisible({ timeout: 8000 });
    expect(errors).toEqual([]);
  });

  test("shows tag selector form", async ({ page }) => {
    await page.goto("/#/daily");
    await expect(page.locator('[data-daily-tag-search]')).toBeVisible();
  });

  test("shows BPM presets", async ({ page }) => {
    await page.goto("/#/daily");
    await expect(page.locator('[data-daily-bpm-presets]')).toBeVisible();
  });

  test("tag typeahead works", async ({ page }) => {
    await page.goto("/#/daily");
    const input = page.locator('[data-daily-tag-search]');
    await input.fill("gro");
    // Typeahead should appear
    await expect(page.locator('[data-daily-tag-dropdown]')).toBeVisible();
  });

  test("generate button calls API", async ({ page }) => {
    await page.goto("/#/daily");
    const input = page.locator('[data-daily-tag-search]');
    await input.fill("Groovy");
    await page.locator('[data-daily-tag-dropdown] >> text=Groovy').click();

    const generateBtn = page.locator('[data-action="generate"]');
    await expect(generateBtn).toBeEnabled();
    await generateBtn.click();
    // Should show result card with playlist name
    await expect(page.locator('[data-daily-result]')).toBeVisible({ timeout: 10000 });
    await expect(page.locator('[data-daily-result]')).toContainText("Daily");
  });
});
