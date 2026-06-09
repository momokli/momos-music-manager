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
});
