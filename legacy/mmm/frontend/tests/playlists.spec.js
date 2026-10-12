import { test, expect } from "@playwright/test";

test.describe("Playlists Page", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("/api/testing/seed", {
      data: { scenario: "basic" },
    });
  });

  test("shows push-to-spotify button for local playlists", async ({ page, request }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    // Create a local playlist via API so it appears in the list
    await request.post("/api/playlists/local", {
      data: { name: "Test Local Push", trackIds: [1] },
    });

    await page.goto("/#playlists");
    await page.waitForSelector("#pl-tbl", { timeout: 8000 });
    await expect(page.locator("#pl-tbl")).toBeVisible();

    // The local playlist should have a "Push" button
    const pushBtn = page.locator('[data-act="push-spotify"]');
    await expect(pushBtn.first()).toBeVisible({ timeout: 5000 });

    expect(errors).toEqual([]);
  });

  test("page loads without errors", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await page.goto("/#playlists");
    await page.waitForSelector("#pl-tbl", { timeout: 8000 });
    await expect(page.locator("#pl-tbl")).toBeVisible();

    expect(errors).toEqual([]);
  });

  test("generated system playlists are hidden by default and revealed via the filter", async ({
    page,
    request,
  }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    // The BPM//key scenario creates generated ('system') playlists, i.e. rows
    // with playlist_kind = 'generated'.
    await request.post("/api/testing/seed", {
      data: { scenario: "bpm_key_playlists" },
    });

    // Sanity check: the seed really produced at least one generated playlist.
    const onlyResp = await request.get("/api/playlists?system=only&limit=100");
    const onlyBody = await onlyResp.json();
    expect((onlyBody.data.playlists || []).length).toBeGreaterThan(0);

    await page.goto("/#playlists");
    await page.waitForSelector("#pl-tbl", { timeout: 8000 });

    // Default = exclude: the "Hide system" button is active and no generated
    // row is present in the table.
    await expect(
      page.locator('[data-sf-filter-group="system"] .filter-btn[data-value="exclude"]'),
    ).toHaveClass(/active/);
    await expect(page.locator('#pl-tbl [data-kind="generated"]')).toHaveCount(0);

    // Switch to "Show all" (system=include) → generated rows appear.
    await page
      .locator('[data-sf-filter-group="system"] .filter-btn[data-value="include"]')
      .click();
    await expect(
      page.locator('[data-sf-filter-group="system"] .filter-btn[data-value="include"]'),
    ).toHaveClass(/active/);
    await expect(page.locator('#pl-tbl [data-kind="generated"]').first()).toBeVisible({
      timeout: 8000,
    });

    expect(errors).toEqual([]);
  });
});
