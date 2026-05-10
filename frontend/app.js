/**
 * app.js — SPA Hash Router
 *
 * Maps hash fragments to page modules.
 * Each page module must export: init(container, signal) => void
 *
 * Usage:
 *   import { init } from './app.js';
 *   init();
 */

import { renderNav, setActiveNav } from "./shared/nav.js";

const PAGE_MAP = {
  "": "dashboard",
  dashboard: "dashboard",
  files: "files",
  tracks: "tracks",
  playlists: "playlists",
  tags: "tags",
  "tag-categories": "tag-categories",
  services: "services",
  tasks: "tasks",
  folders: "folders",
  "auto-categorize": "auto-categorize",
  digging: "digging",
  "traktor-import": "traktor-import",
  "deemix-queue": "deemix-queue",
  data: "data",
  "tag-curation": "tag-curation",
};

let currentPageId = null;
let currentAbortController = null;

/**
 * Parse the hash and return the normalized page id (without query params).
 */
function getPageIdFromHash() {
  const raw = window.location.hash.replace(/^#/, "").trim().toLowerCase();
  // Strip query params — e.g. "files?unlinked=true" → "files"
  const pageKey = raw.split("?")[0];
  return PAGE_MAP[pageKey] || "dashboard";
}

/**
 * Parse query params from the hash fragment.
 * e.g. "#files?unlinked=true" → { unlinked: "true" }
 */
function getHashParams() {
  const raw = window.location.hash.replace(/^#/, "").trim();
  const qIndex = raw.indexOf("?");
  if (qIndex === -1) return {};
  const params = new URLSearchParams(raw.slice(qIndex));
  const obj = {};
  for (const [key, val] of params.entries()) {
    obj[key] = val;
  }
  return obj;
}

/**
 * Load and render a page.
 */
async function navigate(pageId) {
  // If same page, skip (prevents aborting/restarting an ongoing page load)
  if (currentPageId === pageId) {
    return;
  }

  // Abort any ongoing page work
  if (currentAbortController) {
    currentAbortController.abort();
  }
  currentAbortController = new AbortController();

  // Update nav highlight
  setActiveNav(pageId);

  currentPageId = pageId;

  const container = document.getElementById("main-content");
  if (!container) return;

  // Show loading state
  container.innerHTML = `<div class="loading"><div class="spinner"></div><p>Loading ${pageId}...</p></div>`;

  try {
    // Dynamic import
    const mod = await import(`./pages/${pageId}.js`);
    if (typeof mod.init === "function") {
      container.innerHTML = ""; // Clear loading
      mod.init(container, currentAbortController.signal, getHashParams());
    } else {
      container.innerHTML = `<div class="error-block">
        <div class="error-icon"><i class="fas fa-exclamation-triangle"></i></div>
        <h3>Module Error</h3>
        <p>Page "${pageId}.js" does not export an init function.</p>
      </div>`;
    }
  } catch (err) {
    if (err.name === "AbortError" || err.name === "ChunkLoadError") return;
    console.error(`Failed to load page "${pageId}":`, err);
    container.innerHTML = `<div class="error-block">
      <div class="error-icon"><i class="fas fa-exclamation-triangle"></i></div>
      <h3>Failed to load page</h3>
      <p>${err.message || "Unknown error"}</p>
      <a href="#dashboard" class="btn btn-primary">
        <i class="fas fa-redo"></i> Go to Dashboard
      </a>
    </div>`;
  }
}

/**
 * Handle hash changes.
 */
function onHashChange() {
  const pageId = getPageIdFromHash();
  navigate(pageId);
}

/**
 * Initialize the router and render sidebar nav.
 */
export function init() {
  // Render sidebar navigation
  const initialPage = getPageIdFromHash();
  renderNav(initialPage);

  // Listen for hash changes
  window.addEventListener("hashchange", onHashChange);

  // Set hash for first-time visitors (triggers hashchange → navigate).
  // Always call navigate() directly in case hashchange fires before
  // our listener is registered. The page-id guard inside navigate()
  // prevents double-init.
  if (!window.location.hash || window.location.hash === "#") {
    window.location.hash = `#${initialPage}`;
  }
  navigate(initialPage);
}

/**
 * Programmatic navigation helper.
 */
export function goTo(pageId) {
  if (PAGE_MAP[pageId]) {
    window.location.hash = `#${pageId}`;
  }
}

// Auto-start when this module is loaded
init();
