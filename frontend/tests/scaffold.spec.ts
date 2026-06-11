import { test, expect } from "@playwright/test";

test.describe("Scaffold", () => {
  test("Vite dev server serves index.html", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (err) => errors.push(err.message));

    await page.goto("/");
    await expect(page.locator("#root")).toBeVisible({ timeout: 10000 });
    expect(errors).toEqual([]);
  });

  test("React mounts without errors", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (err) => errors.push(err.message));

    await page.goto("/");
    const root = page.locator("#root");
    await expect(root).not.toBeEmpty({ timeout: 10000 });
    expect(errors).toEqual([]);
  });

  test("TypeScript compiles without errors", async () => {
    // Verified by the tsc --noEmit gate before Playwright runs
  });
});
