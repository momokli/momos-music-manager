import { test, expect } from "@playwright/test";

/**
 * Backpack Spotify transport playlist.
 *
 * The page renders a status card from `GET /api/backpack` and a
 * "Push to Spotify" button that POSTs `/api/backpack/push`. Spotify is not
 * configured in the Playwright environment, so the push endpoints are stubbed
 * at the network layer — these tests cover the UI wiring, not the Spotify calls
 * (those are covered by the Rust integration tests).
 */

const STATUS_OK = {
  data: {
    trackCount: 12,
    fileCount: 9,
    playlistUrl: "https://open.spotify.com/playlist/bp-id-1",
    signature: "abc123",
    dirty: false,
    dirtyAt: null,
    lastPushAt: 1700000000,
    lastPushStatus: "ok",
    lastPushError: null,
    pushPending: false,
  },
};

async function gotoBackpack(page) {
  await page.goto("/#backpack");
  await page.waitForSelector("#backpack-playlist-card", { timeout: 8000 });
}

test.describe("Backpack Spotify playlist", () => {
  test.beforeEach(async ({ request, page }) => {
    await request.post("/api/testing/seed", { data: { scenario: "basic" } });
    await page.route("**/api/backpack", async (route) => {
      if (route.request().method() === "GET") {
        await route.fulfill({ json: STATUS_OK });
      } else {
        await route.continue();
      }
    });
  });

  test("renders status card with set size and playlist link", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await gotoBackpack(page);

    const card = page.locator(".backpack-playlist-card");
    await expect(card).toBeVisible();
    await expect(card).toContainText("12");
    await expect(card).toContainText("9");
    await expect(
      page.locator('.backpack-playlist-link a[href*="spotify.com"]'),
    ).toBeVisible();
    await expect(page.locator("#backpack-push")).toBeVisible();

    expect(errors).toEqual([]);
  });

  test("push button reports the outcome and keeps the link visible", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await page.route("**/api/backpack/push", async (route) => {
      await route.fulfill({
        json: {
          data: {
            trackCount: 12,
            created: false,
            updated: true,
            spotifyUrl: "https://open.spotify.com/playlist/bp-id-1",
            deemixSubmitted: true,
            dryRun: false,
            verificationFailed: false,
          },
        },
      });
    });

    await gotoBackpack(page);
    await page.click("#backpack-push");

    await expect(page.locator(".toast-notification")).toContainText("12", {
      timeout: 6000,
    });
    await expect(
      page.locator('.backpack-playlist-link a[href*="spotify.com"]'),
    ).toBeVisible();

    expect(errors).toEqual([]);
  });

  test("cleared deemix checkbox sends submitToDeemix=false", async ({ page }) => {
    let sentBody = null;
    await page.route("**/api/backpack/push", async (route) => {
      sentBody = route.request().postDataJSON();
      await route.fulfill({
        json: {
          data: {
            trackCount: 12,
            created: false,
            updated: true,
            spotifyUrl: "https://open.spotify.com/playlist/bp-id-1",
            deemixSubmitted: false,
            dryRun: false,
            verificationFailed: false,
          },
        },
      });
    });

    await gotoBackpack(page);
    await page.uncheck("#backpack-submit-deemix");
    await page.click("#backpack-push");

    await expect(page.locator(".toast-notification")).toBeVisible({ timeout: 6000 });
    expect(sentBody).not.toBeNull();
    expect(sentBody.submitToDeemix).toBe(false);
  });

  test("a backpack tag can be removed from the backpack page", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    // The `basic` seed has tag 8 ('Deep') with backpack = 1.
    let sentMethod = null;
    let sentBody = null;
    await page.route("**/api/tags/*/backpack", async (route) => {
      sentMethod = route.request().method();
      sentBody = route.request().postDataJSON();
      await route.fulfill({ json: { data: { id: 8, backpack: false } } });
    });

    await gotoBackpack(page);

    const removeBtn = page.locator(".backpack-tag-remove").first();
    await expect(removeBtn).toBeVisible();
    await removeBtn.click();

    await expect(page.locator(".toast-notification")).toContainText("Backpack", {
      timeout: 6000,
    });
    expect(sentMethod).toBe("PUT");
    expect(sentBody).toEqual({ backpack: false });

    expect(errors).toEqual([]);
  });
});
