/**
 * tracks.js — Service tracks page.
 *
 * Lists tracks imported from connected streaming services with
 * pagination, filtering by service, and search.
 * Toolbar is rendered once and preserved across re-renders to
 * keep the search input stable (no focus loss on reload).
 */

import { fetchJSON } from "../shared/api.js";
import {
  renderLoading,
  renderErrorBlock,
  renderTable,
  td,
} from "../shared/components.js";
import { formatDuration } from "../shared/format.js";
import {
  renderSearchInput,
  renderFilterGroup,
  wireSearchFilter,
} from "../shared/search-filter.js";

/* ------------------------------------------------------------------ */
/*  Constants                                                          */
/* ------------------------------------------------------------------ */

const PAGE_SIZE = 10;

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

function escapeHtml(str) {
  if (typeof str !== "string") return str;
  return str
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

/* ------------------------------------------------------------------ */
/*  Adapter                                                            */
/* ------------------------------------------------------------------ */

/**
 * Transform an API service track to the shape expected by the render function.
 * API: durationMs in milliseconds, localFiles as string[].
 * Render: duration in seconds, files as space-joined string.
 */
function adaptTrack(t) {
  return {
    id: t.id,
    title: t.title,
    artist: t.artist,
    service: t.service,
    album: t.album,
    files: t.localFiles && t.localFiles.length > 0 ? t.localFiles.join(" ") : null,
    duration: t.durationMs ? Math.round(t.durationMs / 1000) : 0,
    isrc: t.isrc,
  };
}

/* ------------------------------------------------------------------ */
/*  Render helpers (content area only, toolbar is stable)              */
/* ------------------------------------------------------------------ */

const SERVICE_OPTIONS = [
  { value: "all", label: "All Services" },
  { value: "spotify", label: "Spotify" },
  { value: "soundcloud", label: "SoundCloud" },
  { value: "youtube", label: "YouTube" },
];

const TABLE_HEADERS = [
  { label: "Title", style: "width:25%" },
  { label: "Artist", style: "width:20%" },
  { label: "Service", style: "width:10%" },
  { label: "Album", style: "width:15%" },
  { label: "Local Files", style: "width:15%" },
  { label: "Duration", style: "width:8%;text-align:right" },
  { label: "ISRC", style: "width:7%" },
];

/**
 * Render the toolbar HTML (called once on init).
 */
function renderToolbar(search) {
  return `
    <div class="toolbar">
      ${renderSearchInput("tracks", search)}
      ${renderFilterGroup("service", SERVICE_OPTIONS, "all")}
    </div>`;
}

/**
 * Render the body content (stats, table, pagination).
 */
function renderBody(data, state) {
  const { tracks } = data;
  const totalCount = data._total ?? tracks.length;
  const totalPages = Math.ceil(totalCount / PAGE_SIZE) || 1;
  const pageId = "tracks";

  const rowsHtml = tracks
    .map(
      (t) =>
        `<tr>
          ${td(escapeHtml(t.title), { style: "width:25%" })}
          ${td(escapeHtml(t.artist), { style: "width:20%" })}
          ${td(`<span class="service-badge ${t.service}">${t.service.charAt(0).toUpperCase() + t.service.slice(1)}</span>`, { style: "width:10%" })}
          ${td(t.album ? escapeHtml(t.album) : '<span class="text-muted">—</span>', { style: "width:15%" })}
          ${td(
            t.files
              ? t.files
                  .split(" ")
                  .map((f) => `<code class="font-mono">${escapeHtml(f)}</code>`)
                  .join(" ")
              : '<span class="text-muted">—</span>',
            { style: "width:15%" },
          )}
          ${td(`<span class="font-mono">${escapeHtml(formatDuration(t.duration))}</span>`, { style: "width:8%;text-align:right" })}
          ${td(t.isrc ? `<span class="font-mono text-sm">${escapeHtml(t.isrc)}</span>` : '<span class="text-muted">—</span>', { style: "width:7%" })}
        </tr>`,
    )
    .join("");

  const stats = `<div class="stats-row">
    <div class="stats-group">
      <button class="btn btn-sm btn-icon" id="tracks-refresh" title="Refresh"><i class="fa-solid fa-rotate"></i></button>
      <strong>${totalCount.toLocaleString()}</strong> tracks
    </div>
  </div>`;

  const table = renderTable(TABLE_HEADERS, rowsHtml);

  const pagination = `<div class="pagination" id="${pageId}-pagination">
    <button class="pagination-btn" id="${pageId}-prev" disabled><i class="fa-solid fa-chevron-left"></i></button>
    <span class="pagination-info" id="${pageId}-info">Page ${state.page + 1} of ${totalPages}</span>
    <button class="pagination-btn" id="${pageId}-next" ${totalPages <= 1 ? "disabled" : ""}><i class="fa-solid fa-chevron-right"></i></button>
  </div>`;

  return `${stats}\n${table}\n${pagination}`;
}

/**
 * Render an empty-state body (no tracks at all, not just filtered to zero).
 */
function renderEmptyBody(search) {
  return `
    <div class="stats-row">
      <div class="stats-group">
        <button class="btn btn-sm btn-icon" id="tracks-refresh" title="Refresh"><i class="fa-solid fa-rotate"></i></button>
        <strong>0</strong> tracks
      </div>
    </div>
    <div class="table-wrap"><table class="data-table">
      <thead><tr><th style="width:25%">Title</th><th style="width:20%">Artist</th><th style="width:10%">Service</th><th style="width:15%">Album</th><th style="width:15%">Local Files</th><th style="width:8%;text-align:right">Duration</th><th style="width:7%">ISRC</th></tr></thead>
      <tbody><tr><td colspan="7"><div class="text-center text-muted" style="padding:32px">No tracks found. Import tracks from your connected services to get started.</div></td></tr></tbody>
    </table></div>`;
}

/* ------------------------------------------------------------------ */
/*  Fetch + Render cycle                                               */
/* ------------------------------------------------------------------ */

/**
 * Build query string for the tracks endpoint from the given state.
 */
function buildParams(state) {
  const params = new URLSearchParams();
  params.set("limit", String(PAGE_SIZE));
  params.set("offset", String(state.page * PAGE_SIZE));
  if (state.service !== "all") params.set("service", state.service);
  if (state.search) params.set("search", state.search);
  return params;
}

/**
 * Replace the content area (#tracks-content) with the given HTML.
 */
function setContent(html) {
  const el = document.getElementById("tracks-content");
  if (el) el.innerHTML = html;
}

/**
 * Fetch /api/tracks and /api/tracks/count in parallel, then render.
 * Only replaces #tracks-content — the toolbar stays untouched.
 */
async function fetchAndRender(container, signal, state) {
  setContent(renderLoading("Loading tracks…"));

  try {
    const [tracksResp, countResp] = await Promise.all([
      fetchJSON(`/api/tracks?${buildParams(state)}`, { signal }),
      fetchJSON(`/api/tracks/count?${buildParams(state)}`, { signal }),
    ]);
    if (signal.aborted) return;

    const data = {
      _total: countResp.data,
      tracks: tracksResp.data.map(adaptTrack),
    };

    // Empty state (no tracks in DB at all)
    if (data.tracks.length === 0 && data._total === 0) {
      setContent(renderEmptyBody(state.search));
      wireContentEvents(container, signal, state);
      return;
    }

    setContent(renderBody(data, state));
    wireContentEvents(container, signal, state);
  } catch (err) {
    if (err.name === "AbortError") return;
    try {
      if (signal.aborted) return;
    } catch {
      return;
    }
    setContent(
      renderErrorBlock({
        title: "Failed to load tracks",
        detail: err.message,
        retryFn: "window.location.hash='#tracks'",
      }),
    );
  }
}

/* ------------------------------------------------------------------ */
/*  Event wiring                                                       */
/* ------------------------------------------------------------------ */

function wireContentEvents(container, signal, state) {
  // Refresh button
  const refreshBtn = container.querySelector("#tracks-refresh");
  if (refreshBtn) {
    refreshBtn.onclick = () => fetchAndRender(container, signal, state);
  }

  // Pagination: Prev button
  const prevBtn = container.querySelector("#tracks-prev");
  if (prevBtn) {
    prevBtn.disabled = state.page === 0;
    prevBtn.onclick = () => {
      if (state.page > 0) {
        state.page--;
        fetchAndRender(container, signal, state);
      }
    };
  }

  // Pagination: Next button
  const nextBtn = container.querySelector("#tracks-next");
  if (nextBtn) {
    nextBtn.onclick = () => {
      state.page++;
      fetchAndRender(container, signal, state);
    };
  }
}

/* ------------------------------------------------------------------ */
/*  Initialisation                                                     */
/* ------------------------------------------------------------------ */

export async function init(container, signal) {
  // State for pagination and filters — mutable, lives across renders
  const state = { page: 0, service: "all", search: "" };

  // Render stable toolbar + content wrapper ONCE
  container.innerHTML = `
    ${renderToolbar(state.search)}
    <div id="tracks-content">${renderLoading("Loading tracks…")}</div>
  `;

  // Wire search + filter once (toolbar is stable)
  const toolbar = container.querySelector(".toolbar");
  if (toolbar) {
    wireSearchFilter(toolbar, state, () => fetchAndRender(container, signal, state));
  }

  // Fetch initial data
  await fetchAndRender(container, signal, state);
}
