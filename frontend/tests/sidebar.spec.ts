import { test, expect } from "@playwright/test";

test.describe("Sidebar Navigation", () => {
  test("renders 7 nav items", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (err) => errors.push(err.message));

    await page.goto("/");
    const nav = page.locator("[data-sidebar]");
    await expect(nav).toBeVisible({ timeout: 8000 });

    const links = nav.locator("[data-nav-item]");
    await expect(links).toHaveCount(7);
    expect(errors).toEqual([]);
  });

  test("has three section headers", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator('[data-nav-section="workflows"]')).toBeVisible();
    await expect(page.locator('[data-nav-section="library"]')).toBeVisible();
    await expect(page.locator('[data-nav-section="setup"]')).toBeVisible();
  });

  test("workflow items are Dig, Daily, Pack", async ({ page }) => {
    await page.goto("/");
    const workflowSection = page.locator('[data-nav-section="workflows"]');
    const items = workflowSection.locator("[data-nav-item]");
    await expect(items).toHaveCount(3);
    await expect(items.nth(0)).toContainText("Dig");
    await expect(items.nth(1)).toContainText("Daily");
    await expect(items.nth(2)).toContainText("Pack");
  });

  test("library items are Tracks, Lists, Tags", async ({ page }) => {
    await page.goto("/");
    const libSection = page.locator('[data-nav-section="library"]');
    const items = libSection.locator("[data-nav-item]");
    await expect(items).toHaveCount(3);
    await expect(items.nth(0)).toContainText("Tracks");
    await expect(items.nth(1)).toContainText("Lists");
    await expect(items.nth(2)).toContainText("Tags");
  });

  test("setup item exists", async ({ page }) => {
    await page.goto("/");
    const setupSection = page.locator('[data-nav-section="setup"]');
    await expect(setupSection.locator("[data-nav-item]")).toContainText("Setup");
  });

  test("active nav item is highlighted", async ({ page }) => {
    await page.goto("/");
    await page.click('[data-nav-item="dig"]');
    await expect(page.locator('[data-nav-item="dig"]')).toHaveAttribute(
      "data-active",
      "true",
    );
  });

  test("clicking nav item navigates to correct page", async ({ page }) => {
    await page.goto("/");
    await page.click('[data-nav-item="tags"]');
    await expect(page).toHaveURL(/.*tags.*/);
    await expect(page.locator('[data-page="tags"]')).toBeVisible();
  });

  test("hash-based navigation still works for backward compat", async ({ page }) => {
    await page.goto("/#digging");
    await expect(page.locator('[data-page="digging"]')).toBeVisible({ timeout: 8000 });
  });
});
