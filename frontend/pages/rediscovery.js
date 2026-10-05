/**
 * rediscovery.js — Rediscovery (resurfacing queue) page.
 *
 * Browse forgotten tracks: facet form on the left, live preview table with
 * the server-side *reasons* for every row. All filtering, sorting, `total`
 * and pagination run server-side over `/api/rediscovery/candidates`; the
 * stats bar reads `/api/rediscovery/stats` over the identical facet set.
 *
 * Layout:
 *   ┌── Header + description ───────────────────────────────────┐
 *   ┌── FACET FORM (#rd-*) ─────────────────────────────────────┐
 *   └───────────────────────────────────────────────────────────┘
 *   ┌── STATS BAR (#rd-stats) ──────────────────────────────────┐
 *   │  matching · withBpmAndKey · needsAnalysis · notOwned      │
 *   │  · pushedRecently                                         │
 *   └───────────────────────────────────────────────────────────┘
 *   ┌── PREVIEW (#rd-preview) ──────────────────────────────────┐
 *   │  rows + reason chips                                      │
 *   └───────────────────────────────────────────────────────────┘
 *   ┌── PAGINATION (#rd-prev/#rd-next/#rd-page-info/#rd-total) ─┐
 *   └───────────────────────────────────────────────────────────┘
 */

import {
  escapeHtml,
  showToast,
  renderBadge,
  renderTable,
  td,
  renderEmpty,
  renderErrorBlock,
  Pagination,
} from "../shared/components.js";
import { fetchJSON } from "../shared/api.js";
import {
  formatBPM,
  formatNumber,
  formatDate,
  placeholderUnset,
} from "../shared/format.js";

/* ------------------------------------------------------------------ */
/*  State                                                              */
/* ------------------------------------------------------------------ */

const SORTS = ["oldest-touched", "forgotten", "bpm", "artist", "random"];

const state = {
  facets: {},
  page: 0,
  limit: 50,
  total: 0,
  loading: false,
  rows: [],
  stats: null,
  sort: "oldest-touched",
  seed: "",
};

let _container = null;
let _pageSignal = null;
let _inflight = null;
let _debounceTimer = null;
let _pagination = null;

/* ------------------------------------------------------------------ */
/*  Initialization                                                     */
/* ------------------------------------------------------------------ */

export async function init(container, signal, _hashParams) {
  _container = container;
  _pageSignal = signal;

  // Reload marker: the page must never be re-initialized by a full reload
  // while filtering — tests compare this value across interactions.
  window.__rdNavMarker = `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  window.__rdRetry = () => loadPreview({ includeStats: true });

  render();
  wireEvents();

  if (signal) {
    signal.addEventListener("abort", () => {
      if (_inflight) _inflight.abort();
    });
  }

  await loadPreview({ includeStats: true });
}

/* ------------------------------------------------------------------ */
/*  Render                                                             */
/* ------------------------------------------------------------------ */

function render() {
  _container.innerHTML = `
    <div class="rediscovery-page">
      <div class="page-header">
        <h1><i class="fa-solid fa-rotate-left"></i> Rediscovery</h1>
      </div>
      <p class="rediscovery-intro">
        Resurface tracks you have not touched in a long time. Filter the
        queue, inspect why each track surfaced, then generate a playlist.
      </p>

      <div class="card rediscovery-form">
        ${renderFacetForm()}
      </div>

      <div class="rediscovery-stats card" id="rd-stats"></div>

      <div id="rd-preview"></div>

      <div class="pagination-bar">
        <button class="btn btn-sm" id="rd-prev">
          <i class="fa-solid fa-chevron-left"></i> Prev
        </button>
        <span id="rd-page-info" class="pagination-info">Page 1 of 1</span>
        <button class="btn btn-sm" id="rd-next">
          Next <i class="fa-solid fa-chevron-right"></i>
        </button>
        <span id="rd-showing" class="pagination-showing"></span>
        <span id="rd-total" class="pagination-total">Total: 0</span>
      </div>
    </div>
  `;
}

function numberInput(id, value, opts = {}) {
  const { min = "", max = "", step = "", placeholder = "" } = opts;
  return `<input type="number" id="${id}" class="input-text rediscovery-input"
    value="${value}" ${min !== "" ? `min="${min}"` : ""} ${
      max !== "" ? `max="${max}"` : ""
    } ${step !== "" ? `step="${step}"` : ""} ${
      placeholder ? `placeholder="${placeholder}"` : ""
    } style="width:90px" />`;
}

function checkbox(id, checked, label) {
  return `<label class="checkbox-label rediscovery-check">
    <input type="checkbox" id="${id}" ${checked ? "checked" : ""} />
    ${escapeHtml(label)}
  </label>`;
}

function renderFacetForm() {
  const sortOptions = SORTS.map(
    (s) =>
      `<option value="${s}" ${state.sort === s ? "selected" : ""}>${escapeHtml(s)}</option>`,
  ).join("");

  return `
    <div class="rediscovery-form-grid">
      <div class="rediscovery-field">
        <label class="rediscovery-label" for="rd-touched-before-days">Touched before (days)</label>
        ${numberInput("rd-touched-before-days", 365, { min: 1 })}
      </div>
      <div class="rediscovery-field">
        <label class="rediscovery-label" for="rd-max-playlists">Max playlists</label>
        ${numberInput("rd-max-playlists", "", { min: 0, placeholder: "any" })}
      </div>
      <div class="rediscovery-field">
        <label class="rediscovery-label" for="rd-exclude-pushed-since-days">Exclude pushed since (days)</label>
        ${numberInput("rd-exclude-pushed-since-days", 180, { min: 0 })}
      </div>
      <div class="rediscovery-field">
        <label class="rediscovery-label" for="rd-play-count-max">Play count max</label>
        ${numberInput("rd-play-count-max", "", { min: 0, placeholder: "any" })}
      </div>
      <div class="rediscovery-field">
        <label class="rediscovery-label" for="rd-not-played-since-days">Not played since (days)</label>
        ${numberInput("rd-not-played-since-days", "", { min: 0, placeholder: "any" })}
      </div>
      <div class="rediscovery-field">
        <label class="rediscovery-label" for="rd-bpm-min">BPM min</label>
        ${numberInput("rd-bpm-min", "", { min: 0, step: 0.1, placeholder: "any" })}
      </div>
      <div class="rediscovery-field">
        <label class="rediscovery-label" for="rd-bpm-max">BPM max</label>
        ${numberInput("rd-bpm-max", "", { min: 0, step: 0.1, placeholder: "any" })}
      </div>
      <div class="rediscovery-field">
        <label class="rediscovery-label" for="rd-keys">Keys (csv)</label>
        <input type="text" id="rd-keys" class="input-text rediscovery-input"
          placeholder="8m,4m" autocomplete="off" />
      </div>
      <div class="rediscovery-field">
        <label class="rediscovery-label" for="rd-genres">Genres (csv)</label>
        <input type="text" id="rd-genres" class="input-text rediscovery-input"
          placeholder="House,Techno" autocomplete="off" />
      </div>
      <div class="rediscovery-field">
        <label class="rediscovery-label" for="rd-sort">Sort</label>
        <select id="rd-sort" class="input-text rediscovery-input">${sortOptions}</select>
      </div>
      <div class="rediscovery-field">
        <label class="rediscovery-label" for="rd-seed">Seed (random)</label>
        ${numberInput("rd-seed", "", { min: 0, placeholder: "optional" })}
      </div>
      <div class="rediscovery-field">
        <label class="rediscovery-label" for="rd-limit">Per page</label>
        ${numberInput("rd-limit", 50, { min: 1, max: 200 })}
      </div>
    </div>

    <div class="rediscovery-toggles">
      ${checkbox("rd-liked-only", false, "Liked only")}
      ${checkbox("rd-exclude-backpack", true, "Exclude backpack")}
      ${checkbox("rd-require-bpm", false, "Require BPM")}
      ${checkbox("rd-require-key", false, "Require key")}
    </div>

    <div class="rediscovery-form-actions">
      <button type="button" id="rd-generate" class="btn btn-primary">
        <i class="fa-solid fa-bolt"></i> Generate Playlist
      </button>
    </div>
  `;
}

/* ── Stats bar ──────────────────────────────────────────────────── */

const STAT_ITEMS = [
  ["matching", "Matching"],
  ["withBpmAndKey", "With BPM+Key"],
  ["needsAnalysis", "Needs analysis"],
  ["notOwned", "Not owned"],
  ["pushedRecently", "Pushed recently"],
];

function renderStats() {
  const el = _container.querySelector("#rd-stats");
  if (!el) return;
  const stats = state.stats || {};
  el.innerHTML = STAT_ITEMS.map(([key, label]) => {
    const value = stats[key] == null ? 0 : stats[key];
    return `<div class="rediscovery-stat">
      <span class="rediscovery-stat-label">${escapeHtml(label)}</span>
      <span class="rediscovery-stat-value" data-stat="${key}">${escapeHtml(
        formatNumber(value),
      )}</span>
    </div>`;
  }).join("");
}

/* ── Preview table ──────────────────────────────────────────────── */

const PREVIEW_HEADERS = [
  "Title",
  "Artist",
  "Playlists",
  "Last added",
  "Touched",
  "BPM",
  "Key",
  "Genre",
  "Liked",
  "Backpack",
  "Reasons",
];

function renderReasons(reasons) {
  if (!Array.isArray(reasons) || reasons.length === 0) return "";
  return reasons
    .map(
      (r) =>
        `<span class="reason-chip" data-reason="${escapeHtml(r)}">${escapeHtml(r)}</span>`,
    )
    .join("");
}

function renderRow(row) {
  const bpm = row.bpm == null ? placeholderUnset("No BPM") : escapeHtml(formatBPM(row.bpm));
  const key = row.musicalKey ? escapeHtml(row.musicalKey) : placeholderUnset("No key");
  const genre = row.genre ? escapeHtml(row.genre) : placeholderUnset("No genre");
  const lastAdded =
    row.lastAddedAt == null ? placeholderUnset("Never added") : escapeHtml(formatDate(row.lastAddedAt));
  const touched =
    row.touchedYearsAgo == null
      ? placeholderUnset("Unknown")
      : `${Number(row.touchedYearsAgo).toFixed(1)}y`;
  const liked = row.likedAt != null ? renderBadge("Liked", "var(--green)") : placeholderUnset("Not liked");
  const backpack = row.inBackpack
    ? renderBadge("Backpack", "var(--accent, #6366f1)")
    : placeholderUnset("Not in backpack");

  return `<tr data-track-id="${row.trackId}">
    ${td(escapeHtml(row.title || "—"))}
    ${td(escapeHtml(row.artist || "—"))}
    ${td(String(row.playlistCount ?? 0))}
    ${td(lastAdded)}
    ${td(touched)}
    ${td(bpm)}
    ${td(key)}
    ${td(genre)}
    ${td(liked)}
    ${td(backpack)}
    ${td(renderReasons(row.reasons))}
  </tr>`;
}

function renderPreview() {
  const el = _container.querySelector("#rd-preview");
  if (!el) return;

  if (state.error) {
    el.innerHTML = renderErrorBlock({
      title: "Failed to load rediscovery candidates",
      detail: state.error,
      retryFn: "window.__rdRetry && window.__rdRetry()",
    });
    return;
  }

  if (!state.rows || state.rows.length === 0) {
    el.innerHTML = renderEmpty({
      icon: "rotate-left",
      title: "No candidates",
      message: "No tracks match the current facets. Try widening the filters.",
    });
    return;
  }

  el.innerHTML = renderTable(PREVIEW_HEADERS, state.rows.map(renderRow).join(""));
}

/* ------------------------------------------------------------------ */
/*  Facet collection / query building                                  */
/* ------------------------------------------------------------------ */

function val(id) {
  const el = _container.querySelector(`#${id}`);
  return el ? el.value.trim() : "";
}

function checked(id) {
  const el = _container.querySelector(`#${id}`);
  return !!(el && el.checked);
}

/** Build the shared facet params (no pagination/sort/seed). */
function buildFacetParams() {
  const p = new URLSearchParams();

  const touched = val("rd-touched-before-days");
  p.set("touchedBeforeDays", touched === "" ? "365" : touched);

  const maxPlaylists = val("rd-max-playlists");
  if (maxPlaylists !== "") p.set("maxPlaylists", maxPlaylists);

  if (checked("rd-liked-only")) p.set("likedOnly", "true");

  p.set("excludeBackpack", checked("rd-exclude-backpack") ? "true" : "false");

  const excludePushed = val("rd-exclude-pushed-since-days");
  if (excludePushed !== "") p.set("excludePushedSinceDays", excludePushed);

  const playCountMax = val("rd-play-count-max");
  if (playCountMax !== "") p.set("playCountMax", playCountMax);

  const notPlayed = val("rd-not-played-since-days");
  if (notPlayed !== "") p.set("notPlayedSinceDays", notPlayed);

  if (checked("rd-require-bpm")) p.set("requireBpm", "true");
  if (checked("rd-require-key")) p.set("requireKey", "true");

  const bpmMin = val("rd-bpm-min");
  if (bpmMin !== "") p.set("bpmMin", bpmMin);

  const bpmMax = val("rd-bpm-max");
  if (bpmMax !== "") p.set("bpmMax", bpmMax);

  const keys = val("rd-keys");
  if (keys !== "") p.set("keys", keys);

  const genres = val("rd-genres");
  if (genres !== "") p.set("genres", genres);

  return p;
}

/** Full candidates query: shared facets + sort/seed + limit/offset. */
function buildCandidatesParams() {
  const limit = parseInt(val("rd-limit"), 10) || 50;
  state.limit = limit;

  const p = buildFacetParams();
  p.set("sort", val("rd-sort") || "oldest-touched");
  p.set("limit", String(limit));
  p.set("offset", String(state.page * limit));

  const seed = val("rd-seed");
  if (seed !== "") p.set("seed", seed);

  return p;
}

/* ------------------------------------------------------------------ */
/*  Data loading                                                       */
/* ------------------------------------------------------------------ */

async function loadPreview({ includeStats = true } = {}) {
  if (_inflight) _inflight.abort();
  _inflight = new AbortController();
  const signal = _inflight.signal;

  const candParams = buildCandidatesParams();
  const statsParams = buildFacetParams();

  try {
    const candReq = fetchJSON(
      `/api/rediscovery/candidates?${candParams.toString()}`,
      { signal },
    );
    const statsReq = includeStats
      ? fetchJSON(`/api/rediscovery/stats?${statsParams.toString()}`, { signal })
      : null;

    const [candRes, statsRes] = await Promise.all([candReq, statsReq]);
    if (signal.aborted) return;

    state.error = null;
    state.rows = (candRes.data && candRes.data.candidates) || [];
    state.total = (candRes.data && candRes.data.total) || 0;
    if (statsRes) state.stats = statsRes.data;

    renderPreview();
    if (statsRes) renderStats();
    updatePagination();
  } catch (e) {
    if (e && e.name === "AbortError") return;
    if (signal.aborted) return;
    state.error = e && e.message ? e.message : "Unknown error";
    state.rows = [];
    renderPreview();
  }
}

/* ------------------------------------------------------------------ */
/*  Pagination (server-side)                                           */
/* ------------------------------------------------------------------ */

function setupPagination() {
  _pagination = new Pagination({
    itemsPerPage: state.limit,
    initialPage: 0,
    bindings: {
      prev: "rd-prev",
      next: "rd-next",
      info: "rd-page-info",
      total: "rd-total",
      showing: "rd-showing",
    },
    onPageChange: (page) => {
      state.page = page;
      loadPreview({ includeStats: false });
    },
  });
}

function updatePagination() {
  if (!_pagination) return;
  _pagination.itemsPerPage = state.limit;
  _pagination.page = state.page;
  _pagination.update(state.total, state.rows.length);
}

/* ------------------------------------------------------------------ */
/*  Event Wiring                                                       */
/* ------------------------------------------------------------------ */

function wireEvents() {
  setupPagination();

  const form = _container.querySelector(".rediscovery-form");
  if (form) {
    const onFacetChange = () => {
      state.page = 0;
      clearTimeout(_debounceTimer);
      _debounceTimer = setTimeout(() => {
        loadPreview({ includeStats: true });
      }, 300);
    };
    form.addEventListener("input", onFacetChange);
    form.addEventListener("change", onFacetChange);
  }

  const generate = _container.querySelector("#rd-generate");
  if (generate) {
    generate.addEventListener("click", () => {
      showToast("Not implemented yet", "info");
    });
  }
}

/* ------------------------------------------------------------------ */
/*  Placeholder safety                                                 */
/* ------------------------------------------------------------------ */

// `state.error` is used by renderPreview but declared lazily on state.
// Assigning here keeps the shape explicit without a separate field above.
state.error = null;
