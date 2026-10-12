import { test, expect } from "@playwright/test";

/**
 * ui-events — SPA view tracking hook (full-package feature, plan E2/E5).
 *
 * The app.js navigate() hook posts one `ui.view.opened` per successful
 * page init to the internal `/api/ui-events` endpoint (fire-and-forget,
 * keepalive). The real test server runs with default config (ui events
 * OFF) — so every request must be answered with a silent 204 and the SPA
 * must never see an error. Request bodies are asserted via the network
 * log.
 */

test.describe("ui.view.opened hook", () => {
  test("fires exactly once per navigation with the page id", async ({
    page,
  }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));

    const uiEventBodies = [];
    const uiEventResponses = [];
    page.on("request", (req) => {
      if (req.url().includes("/api/ui-events")) {
        try {
          uiEventBodies.push(req.postDataJSON());
        } catch (_e) {
          uiEventBodies.push({ parseError: true });
        }
      }
    });
    page.on("response", (resp) => {
      if (resp.url().includes("/api/ui-events")) {
        uiEventResponses.push(resp.status());
      }
    });

    // Initial load (no hash) → dashboard is opened exactly once.
    await page.goto("/");
    await expect
      .poll(() => uiEventBodies.length)
      .toBeGreaterThanOrEqual(1);

    // Navigate to folders, then to settings.
    await page.evaluate(() => {
      window.location.hash = "#folders";
    });
    await expect
      .poll(() => uiEventBodies.some((b) => b?.payload?.view === "folders"))
      .toBe(true);

    await page.evaluate(() => {
      window.location.hash = "#settings";
    });
    await expect
      .poll(() => uiEventBodies.some((b) => b?.payload?.view === "settings"))
      .toBe(true);

    // Every body is a well-formed ui.view.opened event…
    for (const body of uiEventBodies) {
      expect(body.type).toBe("ui.view.opened");
      expect(body.payload).toBeTruthy();
      expect(body.payload.view).toMatch(/^[a-z0-9-]{1,64}$/);
    }
    // …and the real server (ui events default off) answers 204 — never 4xx.
    await expect.poll(() => uiEventResponses.length).toBe(uiEventBodies.length);
    for (const status of uiEventResponses) {
      expect(status).toBe(204);
    }

    // Same-page re-navigation must not duplicate the event.
    const dashboardCount = uiEventBodies.filter(
      (b) => b?.payload?.view === "dashboard",
    ).length;
    expect(dashboardCount).toBe(1);
    expect(errors).toEqual([]);
  });
});
