import { test, expect } from "@playwright/test";

/**
 * Verify the Rust backend serves the built frontend correctly.
 * This catches the "blank screen" bug where the Rust server embeds
 * raw .tsx files instead of the Vite build output.
 */
test.describe("Rust server serves built frontend", () => {
  test("serves index.html and React mounts", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (err) => errors.push(err.message));

    // Hit the Rust server directly (not Vite proxy)
    await page.goto("http://localhost:3000/");
    await expect(page.locator("#root")).toBeVisible({ timeout: 10000 });
    await expect(page.locator("#root")).not.toBeEmpty({ timeout: 10000 });

    // Must have zero JS errors (no MIME type errors, no import failures)
    expect(errors).toEqual([]);
  });

  test("sidebar navigation works", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (err) => errors.push(err.message));

    await page.goto("http://localhost:3000/");
    const nav = page.locator("[data-sidebar]");
    await expect(nav).toBeVisible({ timeout: 8000 });

    const links = nav.locator("[data-nav-item]");
    await expect(links).toHaveCount(7);
    expect(errors).toEqual([]);
  });
});
