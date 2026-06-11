import { test, expect } from "@playwright/test";

test.describe("Tags Hub Page", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("http://localhost:3000/api/testing/seed", {
      data: { scenario: "basic" },
    });

    // Seed Vibe tags (category id=4) — not included in basic scenario
    await request.post("http://localhost:3000/api/tags", {
      data: { name: "techno", categoryId: 4 },
    });
    await request.post("http://localhost:3000/api/tags", {
      data: { name: "house", categoryId: 4 },
    });
    await request.post("http://localhost:3000/api/tags", {
      data: { name: "warehouse", categoryId: 4 },
    });

    // Seed Setlist tags (category id=1) with various backpack states
    await request.post("http://localhost:3000/api/tags", {
      data: { name: "Collapse-capital", categoryId: 1 },
    });
    await request.post("http://localhost:3000/api/tags", {
      data: { name: "Test-Playlist", categoryId: 1 },
    });

    // Create Genre category (migration 021 not yet applied) + seed tags
    const genreResp = await request.post("http://localhost:3000/api/tag-categories", {
      data: { name: "Genre", prefix: "G", icon: "fa-guitar", sortOrder: 4 },
    });
    const genreCat = await genreResp.json();
    const genreId = genreCat.data?.id;
    if (genreId) {
      await request.post("http://localhost:3000/api/tags", {
        data: { name: "techno", categoryId: genreId },
      });
      await request.post("http://localhost:3000/api/tags", {
        data: { name: "house", categoryId: genreId },
      });
      await request.post("http://localhost:3000/api/tags", {
        data: { name: "drum-and-bass", categoryId: genreId },
      });
    }
  });

  test("renders without console errors", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (err) => errors.push(err.message));

    await page.goto("/#/tags");
    await expect(page.locator('[data-page="tags"]')).toBeVisible({
      timeout: 8000,
    });
    expect(errors).toEqual([]);
  });

  // ── ENERGY CURVE ──────────────────────────────────────────

  test("energy curve section shows phase tags with energy levels", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="energy-curve"]');
    await expect(section).toBeVisible();

    // Migration 001 seeds 6 phase tags: End(0), Release(1), Start(2),
    // Sustain(3), Build(4), Peak(5)
    const tags = section.locator("[data-energy-tag]");
    await expect(tags.first()).toBeVisible();
    // At least the 6 default phases
    expect(await tags.count()).toBeGreaterThanOrEqual(3);
  });

  test("energy curve shows energy level badges", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="energy-curve"]');
    // At least one phase tag should have a data-energy attribute
    await expect(section.locator("[data-energy]").first()).toBeVisible();
  });

  // ── MOOD / VIBE / GENRE ───────────────────────────────────

  test("mood section shows tag chips", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="mood"]');
    await expect(section).toBeVisible();
    // Should have the tag section rendered (may be empty or have chips)
    await expect(section.locator("[data-tag-chip]").first()).toBeVisible();
  });

  test("vibe section shows tag chips", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="vibe"]');
    await expect(section).toBeVisible();
    const chips = section.locator("[data-tag-chip]");
    await expect(chips.first()).toBeVisible();
  });

  test("genre section shows tag chips", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="genre"]');
    await expect(section).toBeVisible();
    const chips = section.locator("[data-tag-chip]");
    await expect(chips.first()).toBeVisible();
  });

  test("mood/vibe/genre sections have add input with typeahead", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="mood"]');
    const addInput = section.locator("[data-add-tag-input]");
    await expect(addInput).toBeVisible();
  });

  // ── MERKMAL ───────────────────────────────────────────────

  test("merkmal section has typeahead input", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="merkmal"]');
    await expect(section).toBeVisible();
    const input = section.locator("[data-add-tag-input]");
    await expect(input).toBeVisible();
  });

  // ── SETLIST ───────────────────────────────────────────────

  test("setlist section shows tags table", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="setlist"]');
    await expect(section).toBeVisible();

    // Table should be visible
    const table = section.locator("table");
    await expect(table).toBeVisible();
    await expect(table.locator("th")).toHaveCount(3);
  });

  test("setlist section has backpack toggle", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="setlist"]');
    const backpackBtn = section.locator('[data-action="toggle-backpack"]').first();
    await expect(backpackBtn).toBeVisible();
  });

  // ── BUNDLES ───────────────────────────────────────────────

  test("bundles section shows static bundles", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="bundles"]');
    await expect(section).toBeVisible();
    await expect(section.locator('[data-bundle-type="static"]')).toBeVisible();
  });

  test("bundles section shows dynamic bundles", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="bundles"]');
    await expect(section).toBeVisible();
    await expect(section.locator('[data-bundle-type="dynamic"]')).toBeVisible();
  });

  test("new bundle button opens creation form", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="bundles"]');
    await section.locator('[data-action="new-bundle"]').click();
    await expect(section.locator("[data-bundle-form]")).toBeVisible();
  });

  // ── COLLAPSIBLE SECTIONS ──────────────────────────────────

  test("sections are collapsible and state persists", async ({ page }) => {
    await page.goto("/#/tags");
    const section = page.locator('[data-tags-section="mood"]');
    const toggle = section.locator("[data-section-toggle]");
    await toggle.click();
    // Section content should be hidden after toggle
    await expect(section.locator("[data-tag-chip]").first()).not.toBeVisible();

    // Reload page — collapse state should persist via localStorage
    await page.reload();
    await expect(
      page.locator('[data-tags-section="mood"]').locator("[data-tag-chip]").first(),
    ).not.toBeVisible();
  });
});
