import { test, expect } from "@playwright/test";

/**
 * Rediscovery page — the resurfacing queue (Issue #84).
 *
 * Facet form, stats bar and a live preview table with server-side reasons.
 * All filtering, sorting, `total` and pagination are server-side over
 * `/api/rediscovery/candidates`; the stats bar reads `/api/rediscovery/stats`
 * over the identical facet set (`buildFacetParams` / `buildCandidatesParams`).
 *
 * The page must never filter client-side after pagination, and a facet change
 * must not trigger a full reload.
 *
 * ── Seed contract (source of truth: `src/db/testing.rs:719`,
 *    `seed_rediscovery_scenario`) ─────────────────────────────────────────
 *
 * `beforeEach` seeds `POST /api/testing/seed {"scenario":"rediscovery"}`.
 *
 * Universe of `v_track_forgotten_facts` = {1,2,3,10,11,12,13,14,15,16,17,18}.
 * Seed anchor `REDISCOVERY_SEED_EPOCH = 1_790_000_000` (~2026-09). The numbers
 * below stay stable while the real server `now` lies within
 * `[anchor, anchor + ~275 days]` (until ~2027-06): they depend both on the seed
 * and on the `touchedBeforeDays` / `excludePushedSinceDays` facets. If the seed
 * changes or `now` moves past that window, these expectations must be updated.
 *
 * Default facets of the page:
 *   touchedBeforeDays=365 · excludeBackpack=true ·
 *   excludePushedSinceDays=180 · sort=oldest-touched · limit=50 · offset=0
 *
 * Exclusions (Default):
 *   last_touched too young (RECENT 90d): 10, 15
 *   Backpack (excludeBackpack=true):     16
 *   Push cooldown 180d (PUSH_FRESH):     17
 *   Survivors (Default):                 1,2,3,11,12,13,14,18 → total = 8
 *
 * Empirically verified against the seed:
 *   candidates?…default              total=8  ids=[2,1,12,13,14,18,3,11]
 *   candidates?…sort=bpm             ids=[11,1,12,18,2,13,14,3]  (first = 11)
 *   candidates?…likedOnly=true       total=7
 *   candidates?…excludeBackpack=false total=9 (contains 16)
 *   candidates?…limit=3&offset=0     ids=[2,1,12]  (total 8 → 3 pages)
 *   candidates?…limit=3&offset=3     ids=[13,14,18]
 *   stats: matching=8 · withBpmAndKey=7 · needsAnalysis=0 ·
 *          notOwned=2 · pushedRecently=1
 *   track 2 reasons = [last-touched-2014-05-13, only-in-1-playlist,
 *                      never-pushed, liked, not-in-backpack]
 *   (`last_touched_at = 1_400_000_000` → fmt_date = 2014-05-13)
 *
 * Harness rules (DoD):
 *   - register `pageerror` BEFORE `page.goto` in every test, end with
 *     `expect(errors).toEqual([])`;
 *   - no `waitForTimeout` — wait on `waitForResponse` + Playwright auto-wait;
 *   - selectors only `#id` / `[data-*]`.
 */

const CANDIDATES = /\/api\/rediscovery\/candidates/;
const STATS = /\/api\/rediscovery\/stats/;

async function gotoRediscovery(page) {
  // Register the response wait BEFORE goto so the initial load cannot be missed.
  const first = page.waitForResponse((r) => CANDIDATES.test(r.url()));
  await page.goto("/#rediscovery");
  await first;
  await expect(page.locator("#rd-preview tr[data-track-id]").first()).toBeVisible();
}

test.describe("Rediscovery page (resurfacing queue)", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("/api/testing/seed", { data: { scenario: "rediscovery" } });
  });

  test("1. page loads via hash router and appears in the Tools menu", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await gotoRediscovery(page);

    await expect(page.locator("#main-content h1")).toContainText("Rediscovery");
    await expect(page.locator("#rd-preview")).toBeVisible();

    // Tools menu entry exists; becomes visible once the dropdown opens.
    const toolItem = page.locator('[data-page="rediscovery"]');
    await expect(toolItem).toHaveCount(1);
    await page.locator('[data-dropdown-trigger="tools"]').click();
    await expect(toolItem).toBeVisible();

    expect(errors).toEqual([]);
  });

  test("2. default filters — expected candidate count and server sort", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    const stats = page.waitForResponse((r) => STATS.test(r.url()));
    await gotoRediscovery(page);
    await stats;

    // Default facets survive (excludeBackpack=true → track 16 excluded).
    await expect(page.locator("#rd-preview tr[data-track-id]")).toHaveCount(8);
    await expect(page.locator("#rd-total")).toContainText("8");

    // Server-side `oldest-touched` sort: the oldest survivor is track 2.
    await expect(page.locator("#rd-preview tr[data-track-id]").first()).toHaveAttribute(
      "data-track-id",
      "2",
    );

    expect(errors).toEqual([]);
  });

  test("3. likedOnly reduces to liked candidates", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await gotoRediscovery(page);

    const resp = page.waitForResponse(
      (r) => CANDIDATES.test(r.url()) && r.url().includes("likedOnly=true"),
    );
    await page.locator("#rd-liked-only").check();
    expect((await (await resp).json()).data.total).toBe(7);

    await expect(page.locator("#rd-preview tr[data-track-id]")).toHaveCount(7);
    // Track 3 is not liked → gone.
    await expect(page.locator('#rd-preview tr[data-track-id="3"]')).toHaveCount(0);

    expect(errors).toEqual([]);
  });

  test("4. excludeBackpack off brings the backpack track back", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await gotoRediscovery(page);
    await expect(page.locator("#rd-preview tr[data-track-id]")).toHaveCount(8);
    await expect(page.locator('#rd-preview tr[data-track-id="16"]')).toHaveCount(0);

    const resp = page.waitForResponse(
      (r) => CANDIDATES.test(r.url()) && r.url().includes("excludeBackpack=false"),
    );
    await page.locator("#rd-exclude-backpack").uncheck();
    expect((await (await resp).json()).data.total).toBe(9);

    await expect(page.locator("#rd-preview tr[data-track-id]")).toHaveCount(9);
    await expect(page.locator('#rd-preview tr[data-track-id="16"]')).toHaveCount(1);

    expect(errors).toEqual([]);
  });

  test("5. sort=oldest-touched vs bpm — first row changes", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await gotoRediscovery(page);
    await expect(page.locator("#rd-preview tr[data-track-id]").first()).toHaveAttribute(
      "data-track-id",
      "2",
    );

    const resp = page.waitForResponse(
      (r) => CANDIDATES.test(r.url()) && r.url().includes("sort=bpm"),
    );
    await page.locator("#rd-sort").selectOption("bpm");
    await resp;

    // bpm ascending (NULL last): 124 BPM track 11 leads.
    await expect(page.locator("#rd-preview tr[data-track-id]").first()).toHaveAttribute(
      "data-track-id",
      "11",
    );

    expect(errors).toEqual([]);
  });

  test("6. reason chips — oldest track carries last-touched-*", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await gotoRediscovery(page);

    // Default sort puts the oldest survivor (track 2) on the first row.
    await expect(page.locator("#rd-preview tr[data-track-id]").first()).toHaveAttribute(
      "data-track-id",
      "2",
    );
    const row = page.locator('#rd-preview tr[data-track-id="2"]');

    await expect(row.locator('[data-reason="last-touched-2014-05-13"]')).toBeVisible();
    for (const reason of [
      "only-in-1-playlist",
      "never-pushed",
      "liked",
      "not-in-backpack",
    ]) {
      await expect(row.locator(`[data-reason="${reason}"]`)).toBeVisible();
    }

    // Exactly one last-touched-* chip on that row.
    await expect(row.locator('[data-reason^="last-touched-"]')).toHaveCount(1);

    expect(errors).toEqual([]);
  });

  test("7. pagination — smaller limit increases pages, rows stay server-side", async ({
    page,
  }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await gotoRediscovery(page);

    const first = page.waitForResponse(
      (r) =>
        CANDIDATES.test(r.url()) &&
        r.url().includes("limit=3") &&
        r.url().includes("offset=0"),
    );
    await page.locator("#rd-limit").fill("3");
    const data = (await (await first).json()).data;
    expect(data.limit).toBe(3);
    expect(data.offset).toBe(0);
    expect(data.total).toBe(8);

    await expect(page.locator("#rd-preview tr[data-track-id]")).toHaveCount(3);
    await expect(page.locator("#rd-page-info")).toHaveText(/Page 1 of 3/);

    const next = page.waitForResponse(
      (r) =>
        CANDIDATES.test(r.url()) &&
        r.url().includes("limit=3") &&
        r.url().includes("offset=3"),
    );
    await page.locator("#rd-next").click();
    const second = (await (await next).json()).data;
    expect(second.offset).toBe(3);

    // Rows == server page size, i.e. no client-side filtering after pagination.
    await expect(page.locator("#rd-preview tr[data-track-id]")).toHaveCount(3);
    await expect(page.locator("#rd-page-info")).toHaveText(/Page 2 of 3/);

    expect(errors).toEqual([]);
  });

  test("8. stats bar matches the seed", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    const stats = page.waitForResponse((r) => STATS.test(r.url()));
    await gotoRediscovery(page);
    await stats;

    await expect(page.locator('#rd-stats [data-stat="matching"]')).toHaveText("8");
    await expect(page.locator('#rd-stats [data-stat="withBpmAndKey"]')).toHaveText("7");
    await expect(page.locator('#rd-stats [data-stat="needsAnalysis"]')).toHaveText("0");
    await expect(page.locator('#rd-stats [data-stat="notOwned"]')).toHaveText("2");
    await expect(page.locator('#rd-stats [data-stat="pushedRecently"]')).toHaveText("1");

    expect(errors).toEqual([]);
  });
});
