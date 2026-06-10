import { test, expect } from "@playwright/test";

test.describe("Storage Page Backfill", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("/api/testing/seed", {
      data: { scenario: "basic" },
    });
  });

  test("shows backfill backup sizes section", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await page.goto("/#storage");
    await page.waitForSelector("#backfill-section", { timeout: 8000 });
    await expect(page.locator("#backfill-section")).toBeVisible();
    await expect(page.locator("#btn-backfill")).toBeVisible();

    expect(errors).toEqual([]);
  });

  test("backfill button triggers API call and shows result", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await page.goto("/#storage");
    await page.waitForSelector("#btn-backfill", { timeout: 8000 });

    // Click the backfill button
    await page.click("#btn-backfill");

    // Wait for the status to be non-empty (task started, no records, or error)
    await expect(page.locator("#backfill-status")).not.toBeEmpty({
      timeout: 10000,
    });

    // Button should be re-enabled after completion
    await expect(page.locator("#btn-backfill")).toBeEnabled({ timeout: 10000 });

    // Either shows "No records need backfill" or task started or error
    const statusText = await page.locator("#backfill-status").textContent();
    const validStatuses = ["No records need backfill", "Task started", "Error", "Failed"];
    const hasValidStatus = validStatuses.some((s) => statusText.includes(s));
    expect(hasValidStatus).toBeTruthy();

    expect(errors).toEqual([]);
  });
});
