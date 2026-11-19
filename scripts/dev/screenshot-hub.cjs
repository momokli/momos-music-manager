#!/usr/bin/env node
/**
 * Screenshots of the hub's album pages, using the session cookie handed in by
 * `scripts/dev/screenshot.sh` (bcrypt login against the local dev DB — no
 * Keycloak, no Tonies account).
 *
 *   HUB_BASE=http://127.0.0.1:18080 HUB_COOKIE=hub_session=… OUT_DIR=/tmp/shots \
 *     node scripts/dev/screenshot-hub.mjs
 *
 * Uses the system Chrome (`channel: "chrome"`) so no browser download is needed.
 */
const path = require("path");
const fs = require("fs");

const { chromium } = require("playwright");

const base = process.env.HUB_BASE;
const cookie = process.env.HUB_COOKIE;
const outDir = process.env.OUT_DIR || path.resolve("screenshots");
const tonieId = process.env.TONIE_ID || "ct1";

if (!base || !cookie) {
  console.error("HUB_BASE and HUB_COOKIE are required");
  process.exit(2);
}

const shots = [
  {
    name: "hub-tonies-albums-desktop.png",
    url: `/tonies/${tonieId}/albums`,
    width: 1280,
    height: 900,
  },
  {
    name: "hub-tonies-albums-mobile.png",
    url: `/tonies/${tonieId}/albums`,
    width: 390,
    height: 844,
  },
];

(async () => {
  fs.mkdirSync(outDir, { recursive: true });

  const browser = await chromium.launch({ channel: "chrome" });
  const errors = [];

  for (const shot of shots) {
    const context = await browser.newContext({
      viewport: { width: shot.width, height: shot.height },
      deviceScaleFactor: 2,
    });
    const [name, value] = cookie.split("=");
    await context.addCookies([
      { name, value, domain: "127.0.0.1", path: "/", sameSite: "Lax" },
    ]);

    const page = await context.newPage();
    page.on("pageerror", (e) => errors.push(`${shot.name}: ${e.message}`));
    page.on("console", (m) => {
      // Every browser probes /favicon.ico; the hub serves none and that 404 says
      // nothing about the page. The URL lives in `location()`, not in the text.
      const where = (m.location() && m.location().url) || "";
      if (m.type() === "error" && !where.endsWith("/favicon.ico")) {
        errors.push(`${shot.name} console: ${m.text()} (${where})`);
      }
    });

    const resp = await page.goto(base + shot.url, { waitUntil: "networkidle" });
    if (!resp || !resp.ok()) {
      throw new Error(`${shot.url} -> ${resp ? resp.status() : "no response"}`);
    }

    // The album page is the whole point: assert the marker the PR adds is there,
    // so a screenshot of an empty grid never silently passes.
    if (shot.url.includes("/albums")) {
      const tiles = await page.locator(".tonies-album-grid > *").count();
      const basket = await page.locator(".tonies-list[data-cart] li").count();
      if (tiles === 0) throw new Error(`${shot.url}: album grid is empty`);
      console.log(`${shot.name}: ${tiles} tiles, ${basket} cart rows`);
    }

    const file = path.join(outDir, shot.name);
    await page.screenshot({ path: file, fullPage: true });
    console.log(`wrote ${file}`);
    await context.close();
  }

  await browser.close();

  if (errors.length) {
    console.error("page errors:\n" + errors.join("\n"));
    process.exit(1);
  }
})().catch((e) => {
  console.error(String(e && e.stack ? e.stack : e));
  process.exit(1);
});
