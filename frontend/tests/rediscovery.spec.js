import { test, expect } from "@playwright/test";

/**
 * Rediscovery page — the resurfacing queue.
 *
 * Facet form, stats bar and a live preview table with server-side reasons.
 * Filtering, sorting, total and pagination are all server-side; the page must
 * never filter client-side after pagination and must not reload while
 * filtering (the `window.__rdNavMarker` guard would change on a reload).
 */

const CANDIDATES = /\/api\/rediscovery\/candidates/;

async function gotoRediscovery(page) {
  await page.goto("/#rediscovery");
  await page.waitForSelector(".rediscovery-page", { timeout: 8000 });
  // Wait for the first server-side load to render at least the table shell.
  await page.waitForSelector("#rd-preview", { timeout: 8000 });
}

test.describe("Rediscovery page (resurfacing queue)", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("/api/testing/seed", { data: { scenario: "rediscovery" } });
  });

  test("loads via hash router and appears in the Tools menu", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await gotoRediscovery(page);

    await expect(page.locator("h1")).toContainText("Rediscovery");

    // Tools menu entry exists and becomes visible once the dropdown opens.
    const toolItem = page.locator('[data-page="rediscovery"]');
    await expect(toolItem).toHaveCount(1);
    await page.locator('[data-dropdown-trigger="tools"]').click();
    await expect(toolItem).toBeVisible();

    expect(errors).toEqual([]);
  });

  test("counts bar shows the stats values", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await gotoRediscovery(page);
    await page.waitForSelector('#rd-stats [data-stat="matching"]', { timeout: 8000 });

    const matching = await page
      .locator('#rd-stats [data-stat="matching"]')
      .textContent();
    expect(Number(matching.replace(/[^\d]/g, ""))).toBeGreaterThan(0);

    for (const stat of [
      "withBpmAndKey",
      "needsAnalysis",
      "notOwned",
      "pushedRecently",
    ]) {
      await expect(page.locator(`#rd-stats [data-stat="${stat}"]`)).toBeVisible();
    }

    expect(errors).toEqual([]);
  });

  test("preview renders candidate rows with reason chips", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await gotoRediscovery(page);
    await page.waitForSelector("#rd-preview tbody tr", { timeout: 8000 });

    const rowCount = await page.locator("#rd-preview tbody tr").count();
    expect(rowCount).toBeGreaterThan(0);

    // Every row carries a data-track-id and at least one verbatim reason chip.
    const chips = page.locator("#rd-preview .reason-chip");
    expect(await chips.count()).toBeGreaterThan(0);

    // Seed: all rediscovery tracks are liked in playlist 5.
    await expect(
      page.locator('#rd-preview .reason-chip[data-reason="liked"]').first(),
    ).toBeVisible();

    expect(errors).toEqual([]);
  });

  test("facet change is server-side and does not reload the page", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await gotoRediscovery(page);
    await page.waitForSelector("#rd-preview tbody tr", { timeout: 8000 });

    const marker = await page.evaluate(() => window.__rdNavMarker);

    const responsePromise = page.waitForResponse(
      (r) =>
        CANDIDATES.test(r.url()) &&
        r.url().includes("likedOnly=true") &&
        r.url().includes("bpmMin=120"),
      { timeout: 8000 },
    );

    await page.locator("#rd-liked-only").check();
    await page.locator("#rd-bpm-min").fill("120");

    const response = await responsePromise;
    expect(response.url()).toContain("likedOnly=true");
    expect(response.url()).toContain("bpmMin=120");

    // No reload: the init-time marker is unchanged.
    const markerAfter = await page.evaluate(() => window.__rdNavMarker);
    expect(markerAfter).toBe(marker);

    expect(errors).toEqual([]);
  });

  test("pagination is server-side and not re-filtered client-side", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await gotoRediscovery(page);
    await page.waitForSelector("#rd-preview tbody tr", { timeout: 8000 });

    // Widen the facets so more than one page exists, then ask for 5/page.
    await page.locator("#rd-exclude-pushed-since-days").fill("0");
    await page.locator("#rd-exclude-backpack").uncheck();
    await page.locator("#rd-touched-before-days").fill("1");

    const firstPromise = page.waitForResponse(
      (r) =>
        CANDIDATES.test(r.url()) &&
        r.url().includes("limit=5") &&
        r.url().includes("offset=0"),
      { timeout: 8000 },
    );
    await page.locator("#rd-limit").fill("5");

    const firstResp = await firstPromise;
    const firstBody = await firstResp.json();
    const firstPage = firstBody.data;
    expect(firstPage.limit).toBe(5);
    expect(firstPage.offset).toBe(0);
    expect(firstPage.total).toBeGreaterThan(5);

    // Client-side is not filtering: DOM rows == returned candidates (== limit).
    const firstRows = await page.locator("#rd-preview tbody tr").count();
    expect(firstRows).toBe(firstPage.candidates.length);
    expect(firstRows).toBe(5);

    // Next page → server-side offset=5.
    const nextPromise = page.waitForResponse(
      (r) =>
        CANDIDATES.test(r.url()) &&
        r.url().includes("limit=5") &&
        r.url().includes("offset=5"),
      { timeout: 8000 },
    );
    await page.locator("#rd-next").click();

    const nextResp = await nextPromise;
    const nextBody = await nextResp.json();
    const secondPage = nextBody.data;
    expect(secondPage.offset).toBe(5);

    await expect(page.locator("#rd-page-info")).toContainText("Page 2 of");

    // Rows == server-page size, i.e. no client-side filtering after pagination.
    const secondRows = await page.locator("#rd-preview tbody tr").count();
    expect(secondRows).toBe(secondPage.candidates.length);
    expect(secondRows).toBe(
      Math.min(secondPage.limit, secondPage.total - secondPage.offset),
    );

    expect(errors).toEqual([]);
  });

  test("no pageerror after filtering and paging", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await gotoRediscovery(page);
    await page.waitForSelector("#rd-preview tbody tr", { timeout: 8000 });

    await page.locator("#rd-max-playlists").fill("1");
    await page.locator("#rd-require-bpm").check();
    await page.locator("#rd-limit").fill("5");
    await page.waitForTimeout(600);
    await page.locator("#rd-next").click();
    await page.waitForTimeout(600);

    expect(errors).toEqual([]);
  });
});
