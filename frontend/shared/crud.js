/**
 * crud.js — Shared CRUD page building blocks.
 *
 * Provides render helpers and event wiring for sortable tables,
 * page size selector, and hash-based URL state.
 */

import { escapeHtml } from "./components.js";

/* ------------------------------------------------------------------ */
/*  Page Size (global via localStorage)                                */
/* ------------------------------------------------------------------ */

/**
 * Get the global page size preference from localStorage.
 * @param {number} fallback — default if nothing stored (default: 25)
 * @returns {number}
 */
export function getPageSize(fallback = 25) {
  const saved = localStorage.getItem("crudPageSize");
  return saved ? parseInt(saved, 10) : fallback;
}

/**
 * Render a page size selector dropdown.
 * Lives in the stats row, NOT in the toolbar (re-renders with body).
 * @param {number} currentSize — currently selected size
 * @param {number[]} [available] — available sizes (default: [10, 25, 50, 100])
 * @returns {string} HTML string
 */
export function renderPageSizeSelector(currentSize, available = [10, 25, 50, 100]) {
  const opts = available
    .map(
      (s) =>
        `<option value="${s}"${s === currentSize ? " selected" : ""}>${s} per page</option>`,
    )
    .join("");
  return `<select class="page-size-select" data-page-size="true">${opts}</select>`;
}

/**
 * Wire page size selector changes.
 * Updates localStorage globally, resets to page 0, calls onChange.
 * @param {HTMLElement} container — element containing [data-page-size]
 * @param {object} state — mutable state object (mutates pageSize, page)
 * @param {Function} onChange — called after state is updated
 */
export function wirePageSizeSelector(container, state, onChange) {
  const sel = container.querySelector("[data-page-size]");
  if (!sel) return;
  sel.addEventListener("change", () => {
    const val = parseInt(sel.value, 10);
    localStorage.setItem("crudPageSize", String(val));
    state.pageSize = val;
    state.page = 0;
    onChange();
  });
}

/* ------------------------------------------------------------------ */
/*  Sortable Column Headers                                            */
/* ------------------------------------------------------------------ */

/**
 * Render a sortable table header with current state indicator.
 *
 * Sort cycle (click handler uses this): none → asc → desc → none
 *
 * CSS classes used:
 *   .sortable          — cursor pointer, hover effect
 *   .sort-asc          — active ascending sort
 *   .sort-desc         — active descending sort
 *
 * Icon priority: at rest shows fa-sort, active shows fa-sort-up or fa-sort-down,
 * the inactive direction icon is hidden via CSS.
 *
 * @param {string} label — display label text
 * @param {string} column — sort key sent to the API (e.g. "title", "bpm")
 * @param {object} state — current CRUD state { sort, order }
 * @param {object} [opts] — options { style, width }
 * @returns {string} HTML
 */
export function sortableTh(label, column, state, opts = {}) {
  let icon = "fa-sort";
  let cls = "sortable";
  if (state.sort === column) {
    cls += state.order === "asc" ? " sort-asc" : " sort-desc";
    icon = state.order === "asc" ? "fa-sort-up" : "fa-sort-down";
  }
  const style = opts.style ? ` style="${opts.style}"` : "";
  return `<th class="${cls}" data-sort="${column}"${style}>${escapeHtml(label)} <i class="fas ${icon}"></i></th>`;
}

/**
 * Wire sortable header clicks on a table element.
 * Three-state cycle: none → asc → desc → none (allows resetting to default).
 * Mutates state.sort, state.order, state.page (resets to 0), then calls onChange.
 *
 * @param {HTMLElement} tableEl — the <table> or container with sortable <th>
 * @param {object} state — mutable state object { sort, order, page }
 * @param {Function} onChange — called after state is updated (no arguments)
 */
export function wireSortableHeaders(tableEl, state, onChange) {
  tableEl.querySelectorAll("th.sortable[data-sort]").forEach((th) => {
    th.addEventListener("click", () => {
      const col = th.dataset.sort;
      if (state.sort === col) {
        if (state.order === "asc") {
          state.order = "desc";
        } else {
          state.sort = "";
          state.order = "asc";
        }
      } else {
        state.sort = col;
        state.order = "asc";
      }
      state.page = 0;
      onChange();
    });
  });
}

/* ------------------------------------------------------------------ */
/*  URL Hash State (Linkable Views)                                    */
/* ------------------------------------------------------------------ */

/**
 * Update window.location.hash from canonical CRUD state.
 * Uses history.replaceState so no hashchange event fires.
 * The page module handles re-fetching itself after state changes.
 *
 * Serialises all state keys EXCEPT pageSize (global via localStorage).
 * Skips keys whose value matches the provided defaults.
 * Array values are joined with comma.
 *
 * @param {string} pageId — URL page identifier (e.g. "files", "tracks")
 * @param {object} state — the current CRUD state object
 * @param {object} [defaults] — default values to skip (e.g. { sort: "", order: "asc" })
 */
export function updateHash(pageId, state, defaults = {}) {
  const params = new URLSearchParams();
  for (const [key, val] of Object.entries(state)) {
    if (key === "pageSize") continue; // global, not in hash
    if (val instanceof Set) continue; // Sets are not hash-serializable
    if (val === defaults[key] || val === undefined || val === null) continue;
    if (Array.isArray(val) && val.length === 0) continue;
    params.set(key, Array.isArray(val) ? val.join(",") : String(val));
  }
  const qs = params.toString();
  const hash = qs ? `#${pageId}?${qs}` : `#${pageId}`;
  if (window.location.hash !== hash) {
    history.replaceState(null, "", hash);
  }
}

/**
 * Parse common CRUD params from a hashParams object.
 * Returns a partial state object (spread into your state on init).
 *
 * @param {object} hashParams — from app.js getHashParams()
 * @param {object} schema — mapping of key → { type: "string"|"number"|"boolean"|"array", default }
 * @returns {object} parsed state values
 */
export function parseHash(hashParams, schema) {
  const result = {};
  for (const [key, opts] of Object.entries(schema)) {
    const raw = hashParams[key];
    if (raw === undefined || raw === null) {
      result[key] = opts.default;
      continue;
    }
    switch (opts.type) {
      case "number":
        result[key] = parseInt(raw, 10) || opts.default;
        break;
      case "boolean":
        result[key] = raw === "true";
        break;
      case "array":
        result[key] = raw.split(",").filter(Boolean);
        break;
      case "string":
      default:
        result[key] = raw;
        break;
    }
  }
  return result;
}
