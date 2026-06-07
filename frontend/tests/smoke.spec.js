import { test, expect } from "@playwright/test";

test.describe("App Shell", () => {
  test("health endpoint responds", async ({ request }) => {
    const resp = await request.get("/api/health");
    expect(resp.status()).toBe(200);
    const body = await resp.json();
    expect(body.status).toBe("ok");
  });

  test("seed endpoint works (basic scenario)", async ({ request }) => {
    const resp = await request.post("/api/testing/seed", {
      data: { scenario: "basic" },
    });
    expect(resp.status()).toBe(200);
    const body = await resp.json();
    expect(body.ok).toBe(true);
    expect(body.scenario).toBe("basic");
    expect(body.rows.files).toBeGreaterThan(0);
  });

  test("dashboard loads without errors", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await page.goto("/");
    await page
      .waitForSelector("#main-content .loading", { state: "hidden" })
      .catch(() => {});
    await expect(page.locator(".topnav")).toBeVisible({ timeout: 8000 });
    expect(errors).toEqual([]);
  });

  test("files page loads and shows table", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await page.goto("/#files");
    await page.waitForSelector("#files-content table", { timeout: 8000 });
    await expect(page.locator("#files-content table")).toBeVisible();
    expect(errors).toEqual([]);
  });

  test("tracks page loads with toolbar", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await page.goto("/#tracks");
    await page.waitForSelector("#tracks-filter-panel", { timeout: 8000 });
    await expect(page.locator("#tracks-filter-panel")).toBeVisible();
    expect(errors).toEqual([]);
  });

  test("playlists page loads", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await page.goto("/#playlists");
    await page.waitForSelector("#playlists-content", { timeout: 8000 });
    await expect(page.locator("#playlists-content")).toBeVisible();
    expect(errors).toEqual([]);
  });

  test("tags page loads", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    await page.goto("/#tags");
    await page.waitForSelector("#tags-content", { timeout: 8000 });
    await expect(page.locator("#tags-content")).toBeVisible();
    expect(errors).toEqual([]);
  });
});
