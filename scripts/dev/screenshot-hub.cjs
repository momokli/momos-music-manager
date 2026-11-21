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
const os = require("os");

const { chromium } = require("playwright");

/**
 * System Chrome ("channel: chrome") is the intended browser — no download. On a
 * host without it, fall back to a Chromium already in the Playwright cache so
 * the dev screenshots still work offline.
 */
function findBrowser() {
  if (process.env.CHROME_PATH && fs.existsSync(process.env.CHROME_PATH)) {
    return { executablePath: process.env.CHROME_PATH };
  }
  for (const p of [
    "/opt/google/chrome/chrome",
    "/usr/bin/google-chrome",
    "/usr/bin/google-chrome-stable",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
  ]) {
    if (fs.existsSync(p)) return { channel: "chrome" };
  }
  const cache = path.join(os.homedir(), ".cache", "ms-playwright");
  if (fs.existsSync(cache)) {
    const dirs = fs
      .readdirSync(cache)
      .filter((d) => /^chromium-\d+$/.test(d))
      .sort((a, b) => Number(b.split("-")[1]) - Number(a.split("-")[1]));
    for (const d of dirs) {
      for (const rel of ["chrome-linux64/chrome", "chrome-linux/chrome"]) {
        const p = path.join(cache, d, rel);
        if (fs.existsSync(p)) return { executablePath: p };
      }
    }
  }
  return { channel: "chrome" };
}

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
  {
    name: "hub-backpacks-quellen-desktop.png",
    url: "/backpacks",
    width: 1280,
    height: 900,
  },
  {
    name: "hub-backpacks-quellen-mobile.png",
    url: "/backpacks",
    width: 390,
    height: 844,
  },
  {
    name: "hub-comment-templates-desktop.png",
    url: "/comment-templates",
    width: 1280,
    height: 900,
  },
  {
    name: "hub-kellner-template-desktop.png",
    url: "/kellner",
    width: 1280,
    height: 900,
  },
  {
    name: "hub-kellner-template-mobile.png",
    url: "/kellner",
    width: 390,
    height: 844,
  },
];

(async () => {
  fs.mkdirSync(outDir, { recursive: true });

  const browser = await chromium.launch(findBrowser());
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

    // The backpack view (#651) is only worth a screenshot when it actually
    // rendered cards and all four seeded source kinds: an empty grid or a card
    // without sources would otherwise pass silently.
    if (shot.url.includes("/backpacks")) {
      const cards = await page.locator(".hub-backpack-card").count();
      const sources = await page.locator(".hub-backpack-sources li").count();
      if (cards === 0) throw new Error(`${shot.url}: backpack grid is empty`);
      if (sources < 4) {
        throw new Error(`${shot.url}: expected >=4 backpack sources, got ${sources}`);
      }
      console.log(`${shot.name}: ${cards} backpack cards, ${sources} source rows`);
    }

    // Comment-template preview (#670): /kellner renders the fragment on the
    // server, /comment-templates loads it via htmx on click. Either way the
    // shot is only meaningful with rows, `10 von N` and the `n/512` counter.
    if (shot.url.includes("/comment-templates")) {
      const btn = page.locator('button[hx-post$="/preview"]').first();
      if ((await btn.count()) === 0) {
        throw new Error(`${shot.url}: no preview trigger button`);
      }
      await btn.click();
      await page.waitForSelector(".hub-preview-list li", { timeout: 15000 });
      const rows = await page.locator(".hub-preview-list li").count();
      if (rows === 0) throw new Error(`${shot.url}: preview list is empty`);
      const counter = (await page.locator(".hub-preview p.hub-muted").last().textContent()) || "";
      if (!/\d+ von \d+/.test(counter)) {
        throw new Error(`${shot.url}: missing "10 von N" counter (got "${counter.trim()}")`);
      }
      console.log(`${shot.name}: ${rows} preview rows, counter "${counter.trim()}"`);
    }

    if (shot.url.includes("/kellner")) {
      const rows = await page.locator(".hub-preview-list li").count();
      if (rows < 10) {
        throw new Error(`${shot.url}: expected >=10 preview rows, got ${rows}`);
      }
      const counter = (await page.locator(".hub-preview p.hub-muted").last().textContent()) || "";
      if (!/\d+ von \d+/.test(counter)) {
        throw new Error(`${shot.url}: missing "10 von N" counter (got "${counter.trim()}")`);
      }
      console.log(`${shot.name}: ${rows} preview rows, counter "${counter.trim()}"`);
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
