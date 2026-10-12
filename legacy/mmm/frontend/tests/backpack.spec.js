import { test, expect } from "@playwright/test";

/**
 * Backpack page — the "what I want on my Mac" control.
 *
 * Covers the keep set (backpack tags + subscribed playlists), the file-sync
 * switch and the Sync All trigger. The Spotify playlist transport was removed
 * (ADR-067); these tests assert it stays gone.
 */

async function gotoBackpack(page) {
  await page.goto("/#backpack");
  await page.waitForSelector("#backpack-sync-enabled", { timeout: 8000 });
}

test.describe("Backpack page (keep / best-format control)", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("/api/testing/seed", { data: { scenario: "basic" } });
  });

  test("renders the set and no longer shows the Spotify transport", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await gotoBackpack(page);

    await expect(page.locator("h1")).toContainText("Backpack");
    await expect(page.locator(".backpack-section")).toHaveCount(2);
    await expect(page.locator("#backpack-sync-all")).toBeVisible();
    await expect(page.locator("#backpack-sync-enabled")).toBeChecked();

    // Transport is gone: no playlist card, no push button, no music-api card.
    await expect(page.locator("#backpack-playlist-card")).toHaveCount(0);
    await expect(page.locator("#backpack-push")).toHaveCount(0);
    await expect(page.locator("#backpack-music-api-card")).toHaveCount(0);

    expect(errors).toEqual([]);
  });

  test("a backpack tag shows as a source and can be removed", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await gotoBackpack(page);

    const cards = page.locator(".backpack-tag-card");
    await expect(cards).toHaveCount(1);
    await expect(cards.first()).toContainText("Deep");

    await cards.first().locator(".backpack-tag-remove").click();

    // The tag stays in the DB but leaves the Backpack, so the list empties.
    await expect(page.locator(".backpack-tag-card")).toHaveCount(0);
    expect(errors).toEqual([]);
  });

  test("the file-sync switch persists via the settings endpoint", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await gotoBackpack(page);

    let putBody = null;
    await page.route("**/api/storage/settings/backpack-sync", async (route) => {
      if (route.request().method() === "PUT") {
        putBody = route.request().postDataJSON();
        await route.fulfill({ json: { data: { enabled: putBody.enabled } } });
      } else {
        await route.continue();
      }
    });

    await page.locator("#backpack-sync-enabled").uncheck();

    await expect.poll(() => putBody && putBody.enabled).toBe(false);
    // Sync All is disabled while the switch is off.
    await expect(page.locator("#backpack-sync-all")).toBeDisabled();
    expect(errors).toEqual([]);
  });

  test("Sync All starts a backpack sync task", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await gotoBackpack(page);

    let posted = false;
    await page.route("**/api/storage/sync-backpack", async (route) => {
      posted = true;
      await route.fulfill({ json: { data: { taskId: "task-1" } } });
    });
    await page.route("**/api/tasks/task-1", async (route) => {
      await route.fulfill({ json: { data: { status: "completed" } } });
    });

    await page.locator("#backpack-sync-all").click();

    await expect.poll(() => posted).toBe(true);
    expect(errors).toEqual([]);
  });
});
