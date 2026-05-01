/**
 * search-filter.js — Unified search & filter toolbar builder.
 *
 * Provides consistent render helpers and event wiring for all pages.
 * Search inputs use `data-sf-search="true"`, filter elements use `data-sf-filter="{key}"`.
 *
 * Usage:
 *   const html = renderSearchInput("files", state.search)
 *              + renderFilterSelect("key", keyOptions, state.key);
 *   toolbar.innerHTML = html;
 *   wireSearchFilter(toolbar, state, () => fetchAndRender(...), 300);
 */

import { escapeHtml } from "./components.js";

/* ------------------------------------------------------------------ */
/*  Render helpers                                                     */
/* ------------------------------------------------------------------ */

/**
 * Render a search input with magnifying-glass icon.
 * @param {string} entity — lower-case label for placeholder (e.g. "files", "tracks")
 * @param {string} [value=""] — current search value
 * @returns {string} HTML string
 */
export function renderSearchInput(entity, value = "") {
  const val = value ? ` value="${escapeHtml(value)}"` : "";
  return `<div class="search-wrap">
    <i class="fas fa-search"></i>
    <input type="text" class="input-text input-search" data-sf-search="true"
           placeholder="Search ${escapeHtml(entity)}…"${val}>
  </div>`;
}

/**
 * Render a filter <select> element.
 * @param {string} key — filter key, set as data-sf-filter attribute
 * @param {Array<{value:string, label:string}>} options — dropdown items
 * @param {string} [currentValue=""] — currently selected value
 * @returns {string} HTML string
 */
export function renderFilterSelect(key, options, currentValue = "") {
  const opts = options
    .map(
      (o) =>
        `<option value="${escapeHtml(o.value)}"${o.value === currentValue ? " selected" : ""}>${escapeHtml(o.label)}</option>`,
    )
    .join("");
  return `<select class="input-text" data-sf-filter="${escapeHtml(key)}" style="width:auto;min-width:100px">
    ${opts}
  </select>`;
}

/**
 * Render a group of filter buttons.
 * @param {string} key — filter key, set as data-sf-filter-group on the wrapper
 * @param {Array<{value:string, label:string}>} options — button items
 * @param {string} [currentValue=""] — currently active value
 * @returns {string} HTML string
 */
export function renderFilterGroup(key, options, currentValue = "") {
  const btns = options
    .map(
      (o) =>
        `<button class="filter-btn${o.value === currentValue ? " active" : ""}" data-value="${escapeHtml(o.value)}">${escapeHtml(o.label)}</button>`,
    )
    .join("");
  return `<div class="filter-group" data-sf-filter-group="${escapeHtml(key)}">
    ${btns}
  </div>`;
}

/* ------------------------------------------------------------------ */
/*  State tracking (module-level)                                      */
/* ------------------------------------------------------------------ */

/**
 * Tracks whether the search input had focus before a re-render.
 * Set on mousedown (before focus is lost), restored after onChange completes.
 */
let _sfFocusPending = false;

/* ------------------------------------------------------------------ */
/*  Event wiring                                                       */
/* ------------------------------------------------------------------ */

/**
 * Wire up search input + all filter elements inside a toolbar.
 *
 * - `[data-sf-search]` → debounced text input, sets `state.search`
 * - `[data-sf-filter]` on <select> / checkbox → immediate on `change`
 * - `[data-sf-filter]` on <input> (text, number) → debounced on `input`
 * - `[data-sf-filter-group]` → click delegation on contained `.filter-btn`
 *
 * After ANY onChange completes, tries to restore focus to the search input
 * if it had focus before the change.
 *
 * @param {HTMLElement} toolbarEl — the .toolbar element containing the controls
 * @param {object} state — mutable state object (mutated in-place, page reset to 0)
 * @param {Function} onChange — called with (state) when any filter changes
 * @param {number} [debounceMs=300] — debounce delay for text inputs
 */
export function wireSearchFilter(toolbarEl, state, onChange, debounceMs = 300) {
  // Search input
  const searchInput = toolbarEl.querySelector("[data-sf-search]");
  if (searchInput) {
    searchInput.value = state.search || "";
    let timer;

    // Track focus via mousedown — fires BEFORE the browser moves focus,
    // so we know the user intended to interact with the search input.
    searchInput.addEventListener("mousedown", () => {
      _sfFocusPending = true;
    });

    // Also track focus via focusin for keyboard navigation (Tab)
    searchInput.addEventListener("focusin", () => {
      _sfFocusPending = true;
    });

    searchInput.addEventListener("input", () => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        state.search = searchInput.value;
        state.page = 0;
        runAndRefocus(onChange, state);
      }, debounceMs);
    });
  }

  // Individual filter controls
  toolbarEl.querySelectorAll("[data-sf-filter]").forEach((el) => {
    const key = el.dataset.sfFilter;

    // Set initial value from state
    if (state[key] !== undefined) {
      if (el.tagName === "SELECT") el.value = state[key];
      else if (el.type === "checkbox") el.checked = state[key];
      else el.value = state[key];
    }

    if (el.tagName === "SELECT" || el.type === "checkbox") {
      // Immediate on change
      el.addEventListener("change", () => {
        state[key] = el.type === "checkbox" ? el.checked : el.value;
        state.page = 0;
        runAndRefocus(onChange, state);
      });
    } else {
      // Debounced text/number inputs
      let timer;
      el.addEventListener("input", () => {
        clearTimeout(timer);
        timer = setTimeout(() => {
          state[key] = el.value;
          state.page = 0;
          runAndRefocus(onChange, state);
        }, debounceMs);
      });
    }
  });

  // Filter button groups
  toolbarEl.querySelectorAll("[data-sf-filter-group]").forEach((wrapper) => {
    const key = wrapper.dataset.sfFilterGroup;
    wrapper.addEventListener("click", (e) => {
      const btn = e.target.closest(".filter-btn[data-value]");
      if (!btn) return;

      // Toggle active class
      wrapper
        .querySelectorAll(".filter-btn")
        .forEach((b) => b.classList.remove("active"));
      btn.classList.add("active");

      state[key] = btn.dataset.value;
      state.page = 0;
      runAndRefocus(onChange, state);
    });
  });
}

/* ------------------------------------------------------------------ */
/*  Refocus helper                                                     */
/* ------------------------------------------------------------------ */

/**
 * Call onChange(state), then restore focus to the search input if
 * _sfFocusPending was set (meaning it had focus before the change).
 *
 * This is extracted so EVERY filter change path goes through the same
 * refocus logic, not just the debounced text-input path.
 */
async function runAndRefocus(onChange, state) {
  try {
    await Promise.resolve(onChange(state));
  } catch (err) {
    console.error("search-filter onChange error:", err);
  }

  // Restore focus if the search input had focus before the change
  if (_sfFocusPending) {
    const el = document.querySelector("[data-sf-search]");
    if (el && el !== document.activeElement) {
      el.focus();
      el.setSelectionRange(el.value.length, el.value.length);
    }
    _sfFocusPending = false;
  }
}
