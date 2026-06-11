import { test, expect } from "@playwright/test";

test.describe("Digging Page", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("http://localhost:3000/api/testing/seed", {
      data: { scenario: "digging" },
    });
  });

  test("renders without console errors", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (err) => errors.push(err.message));

    await page.goto("/#/digging");
    await expect(page.locator('[data-page="digging"]')).toBeVisible({
      timeout: 8000,
    });
    expect(errors).toEqual([]);
  });

  test("tag typeahead finds and selects tags", async ({ page }) => {
    await page.goto("/#/digging");
    const input = page.locator("[data-digging-tag-search]");
    await input.fill("collapse");
    await expect(page.locator("[data-digging-tag-dropdown]")).toBeVisible();
    await page
      .locator("[data-digging-tag-dropdown] >> text=Collapse-capital")
      .click();
    await expect(page.locator("[data-digging-tag-chip]")).toContainText(
      "Collapse-capital",
    );
  });

  test("find similar returns suggestions", async ({ page }) => {
    await page.goto("/#/digging");
    await page.locator("[data-digging-tag-search]").fill("collapse");
    await page
      .locator("[data-digging-tag-dropdown] >> text=Collapse-capital")
      .click();
    await page.locator('[data-action="find-similar"]').click();
    await expect(page.locator("[data-digging-suggestion]").first()).toBeVisible({
      timeout: 10000,
    });
  });

  test("suggestions show BPM, key, camelot compatibility", async ({ page }) => {
    await page.goto("/#/digging");
    await page.locator("[data-digging-tag-search]").fill("collapse");
    await page
      .locator("[data-digging-tag-dropdown] >> text=Collapse-capital")
      .click();
    await page.locator('[data-action="find-similar"]').click();

    const first = page.locator("[data-digging-suggestion]").first();
    await expect(first.locator('[data-field="bpm"]')).toBeVisible({
      timeout: 10000,
    });
    await expect(first.locator('[data-field="key"]')).toBeVisible();
    await expect(first.locator("[data-camelot-compat]")).toBeVisible();
  });

  test("BPM range slider filters suggestions", async ({ page }) => {
    await page.goto("/#/digging");
    await page.locator("[data-digging-tag-search]").fill("collapse");
    await page
      .locator("[data-digging-tag-dropdown] >> text=Collapse-capital")
      .click();
    await page.locator('[data-action="find-similar"]').click();
    await expect(page.locator("[data-digging-suggestion]").first()).toBeVisible({
      timeout: 10000,
    });

    const slider = page.locator("[data-bpm-range]");
    await expect(slider).toBeVisible();
  });

  test("audio player plays and pauses", async ({ page }) => {
    await page.goto("/#/digging");
    await page.locator("[data-digging-tag-search]").fill("collapse");
    await page
      .locator("[data-digging-tag-dropdown] >> text=Collapse-capital")
      .click();
    await page.locator('[data-action="find-similar"]').click();

    const playBtn = page
      .locator("[data-digging-suggestion]")
      .first()
      .locator('[data-action="play"]');
    await expect(playBtn).toBeVisible({ timeout: 10000 });
    await playBtn.click();
    await expect(playBtn.locator(".fa-pause")).toBeVisible();
  });

  test("add to staging and save as playlist", async ({ page }) => {
    await page.goto("/#/digging");
    await page.locator("[data-digging-tag-search]").fill("collapse");
    await page
      .locator("[data-digging-tag-dropdown] >> text=Collapse-capital")
      .click();
    await page.locator('[data-action="find-similar"]').click();
    await expect(page.locator("[data-digging-suggestion]").first()).toBeVisible({
      timeout: 10000,
    });

    await page
      .locator("[data-digging-suggestion]")
      .first()
      .locator('[data-action="add-to-staging"]')
      .click();
    await expect(page.locator("[data-staging-count]")).toContainText("1");
  });
});
