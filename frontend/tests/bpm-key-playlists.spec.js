import { test, expect } from "@playwright/test";

/**
 * BPM // Key Playlists page (#bpm-key-playlists).
 *
 * Seeds the dedicated `bpm_key_playlists` scenario (added in the backend wave)
 * so the preview has real (BPM, key) groups to render.
 *
 * DOM contract exposed by frontend/pages/bpm-key-playlists.js:
 *   #bpmkey-tbl                    preview table
 *   #bpmkey-tbl tbody tr           one row per group
 *     [data-system-key]            e.g. "bpm_key:124:12A"
 *     [data-bpm] / [data-key]      the combo
 *   #bpmkey-refresh-preview        refresh button
 *   #bpmkey-sync                   "Sync to Spotify" button
 *   #bpmkey-task-status            task progress card (hidden until a sync runs)
 *   #bpmkey-setting-*              settings inputs
 *   #bpmkey-save-settings          settings save button
 */
test.describe("BPM // Key Playlists Page", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("/api/testing/seed", {
      data: { scenario: "bpm_key_playlists" },
    });
  });

  test("page loads, preview renders groups, no page errors", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await page.goto("/#bpm-key-playlists");
    await page.waitForSelector("#bpmkey-tbl", { timeout: 10000 });
    await expect(page.locator("#bpmkey-tbl")).toBeVisible();

    // At least one (BPM, key) group derived from the seed.
    const rows = page.locator("#bpmkey-tbl tbody tr");
    expect(await rows.count()).toBeGreaterThan(0);

    // Rows carry the stable combo key (independent of the display template).
    const firstSystemKey = await rows.first().getAttribute("data-system-key");
    expect(firstSystemKey).toMatch(/^bpm_key:/);

    // A numeric BPM and a key are rendered per row.
    expect(await rows.first().getAttribute("data-bpm")).toMatch(/^\d+$/);
    expect(await rows.first().getAttribute("data-key")).not.toBeNull();

    // Actions + settings controls are present.
    await expect(page.locator("#bpmkey-sync")).toBeVisible();
    await expect(page.locator("#bpmkey-refresh-preview")).toBeVisible();
    await expect(page.locator("#bpmkey-setting-name-template")).toBeVisible();
    await expect(page.locator("#bpmkey-setting-enabled")).toBeVisible();
    await expect(page.locator("#bpmkey-save-settings")).toBeVisible();

    expect(errors).toEqual([]);
  });

  test("refresh preview re-renders without errors", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await page.goto("/#bpm-key-playlists");
    await page.waitForSelector("#bpmkey-tbl", { timeout: 10000 });

    await page.locator("#bpmkey-refresh-preview").click();
    await expect(page.locator("#bpmkey-tbl")).toBeVisible({ timeout: 8000 });
    await expect(page.locator("#bpmkey-refresh-preview")).toBeEnabled();

    expect(errors).toEqual([]);
  });

  test("sync button starts a task or surfaces a clear error", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await page.goto("/#bpm-key-playlists");
    await page.waitForSelector("#bpmkey-sync", { timeout: 10000 });

    await page.locator("#bpmkey-sync").click();
    await page.waitForTimeout(2000);

    const taskStatusVisible = await page
      .locator("#bpmkey-task-status")
      .isVisible()
      .catch(() => false);
    const toastVisible = await page
      .locator(".toast-notification")
      .isVisible()
      .catch(() => false);

    // Either the sync started (progress card appears + is polled) or a clear
    // error toast was shown (e.g. Spotify not configured in the test env).
    expect(taskStatusVisible || toastVisible).toBeTruthy();

    // The button is always restored to an enabled state.
    await expect(page.locator("#bpmkey-sync")).toBeEnabled();

    expect(errors).toEqual([]);
  });

  test("settings form round-trips through PUT", async ({ page, request }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    // Snapshot current settings so the test leaves no residue
    // (the `settings` table is not cleared by the seed).
    const before = await (await request.get("/api/bpm-key-playlists/settings")).json();
    const original = before.data.settings;

    await page.goto("/#bpm-key-playlists");
    await page.waitForSelector("#bpmkey-setting-name-template", { timeout: 10000 });

    const uniqueTemplate = "E2E {bpm} // {key}";
    await page.locator("#bpmkey-setting-name-template").fill(uniqueTemplate);
    await page.locator("#bpmkey-save-settings").click();
    await expect(page.locator(".toast-notification")).toBeVisible({ timeout: 5000 });

    const after = await (await request.get("/api/bpm-key-playlists/settings")).json();
    expect(after.data.settings.nameTemplate).toBe(uniqueTemplate);

    // Restore the original settings.
    await request.put("/api/bpm-key-playlists/settings", { data: original });

    expect(errors).toEqual([]);
  });
});
