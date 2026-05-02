/**
 * tracks.js — Service tracks page.
 *
 * Lists tracks imported from connected streaming services with
 * pagination, filtering by service, search, and optional playlist
 * scoping. Supports playlist-context badge in the toolbar.
 *
 * Toolbar is rendered once and preserved across re-renders to
 * keep the search input stable (no focus loss on reload).
 *
 * URL hash params:
 *   #tracks?search=foo&service=spotify&page=0&playlistId=42&playlistName=Summer
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

const SERVICE_OPTIONS = [
  { value: "all", label: "All Services" },
  { value: "spotify", label: "Spotify" },
  { value: "soundcloud", label: "SoundCloud" },
  { value: "youtube", label: "YouTube" },
];

const TABLE_HEADERS = [
  { label: "Title", style: "width:22%" },
  { label: "Artist", style: "width:16%" },
  { label: "Service", style: "width:8%" },
  { label: "Album", style: "width:14%" },
  { label: "Playlists", style: "width:18%" },
  { label: "Local Files", style: "width:10%" },
  { label: "Duration", style: "width:8%;text-align:right" },
  { label: "ISRC", style: "width:4%" },
];

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
 * API: durationMs in milliseconds, localFiles as string[], playlistNames as string[].
 * Render: duration in seconds, files as space-joined string, playlistNames as array.
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
    playlistNames: t.playlistNames || [],
  };
}

/* ------------------------------------------------------------------ */
/*  Render helpers (content area only, toolbar is stable)              */
/* ------------------------------------------------------------------ */

/**
 * Render the toolbar HTML (called once on init).
 * Includes search input, service filter group, and optional playlist
 * context badge when scoped to a playlist.
 *
 * @param {string} search  — current search value
 * @param {object} state   — current state (used for playlistName)
 * @returns {string} HTML
 */
function renderToolbar(search, state) {
  let playlistBadge = "";
  if (state.playlistName) {
    playlistBadge = `
      <div class="playlist-context-badge">
        <i class="fa-solid fa-list"></i>
        <span>Playlist: ${escapeHtml(state.playlistName)}</span>
        <button class="playlist-context-clear" title="Clear playlist filter">&times;</button>
      </div>`;
  }

  return `
    <div class="toolbar">
      ${renderSearchInput("tracks", search)}
      ${renderFilterGroup("service", SERVICE_OPTIONS, state.service)}
      ${playlistBadge}
    </div>`;
}

/**
 * Render the body content (stats, table, pagination).
 * When scoped to a playlist (state.playlistId is set), the Playlists
 * column is hidden as it is redundant.
 */
function renderBody(data, state) {
  const { tracks } = data;
  const totalCount = data._total ?? tracks.length;
  const totalPages = Math.ceil(totalCount / PAGE_SIZE) || 1;
  const pageId = "tracks";
  const isScoped = !!state.playlistId;

  // When scoped, filter out the Playlists column header
  const headers = isScoped
    ? TABLE_HEADERS.filter((h) => h.label !== "Playlists")
    : TABLE_HEADERS;

  const rowsHtml = tracks
    .map((t) => {
      const playlistsHtml =
        t.playlistNames && t.playlistNames.length > 0
          ? t.playlistNames
              .map(
                (n) =>
                  `<span class="tag-badge font-mono" style="font-size:0.75rem">${escapeHtml(n)}</span>`,
              )
              .join(" ")
          : '<span class="text-muted">—</span>';

      return `<tr>
          ${td(escapeHtml(t.title), { style: "width:22%" })}
          ${td(escapeHtml(t.artist), { style: "width:16%" })}
          ${td(
            `<span class="service-badge ${t.service}">${t.service.charAt(0).toUpperCase() + t.service.slice(1)}</span>`,
            { style: "width:8%" },
          )}
          ${td(t.album ? escapeHtml(t.album) : '<span class="text-muted">—</span>', {
            style: "width:14%",
          })}
          ${td(playlistsHtml, {
            style: `width:18%${isScoped ? ";display:none" : ""}`,
          })}
          ${td(
            t.files
              ? t.files
                  .split(" ")
                  .map((f) => `<code class="font-mono">${escapeHtml(f)}</code>`)
                  .join(" ")
              : '<span class="text-muted">—</span>',
            { style: "width:10%" },
          )}
          ${td(
            `<span class="font-mono">${escapeHtml(formatDuration(t.duration))}</span>`,
            { style: "width:8%;text-align:right" },
          )}
          ${td(
            t.isrc
              ? `<span class="font-mono text-sm">${escapeHtml(t.isrc)}</span>`
              : '<span class="text-muted">—</span>',
            { style: "width:4%" },
          )}
        </tr>`;
    })
    .join("");

  const stats = `<div class="stats-row">
    <div class="stats-group">
      <button class="btn btn-sm btn-icon" id="tracks-refresh" title="Refresh"><i class="fa-solid fa-rotate"></i></button>
      <strong>${totalCount.toLocaleString()}</strong> tracks
      ${
        isScoped
          ? `<span style="margin-left:8px;color:var(--text-subtle)">in playlist <strong>${escapeHtml(state.playlistName)}</strong></span>`
          : ""
      }
    </div>
  </div>`;

  const table = renderTable(headers, rowsHtml);

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
  const headers = [
    { label: "Title", style: "width:22%" },
    { label: "Artist", style: "width:16%" },
    { label: "Service", style: "width:8%" },
    { label: "Album", style: "width:14%" },
    { label: "Playlists", style: "width:18%" },
    { label: "Local Files", style: "width:10%" },
    { label: "Duration", style: "width:8%;text-align:right" },
    { label: "ISRC", style: "width:4%" },
  ];

  const theadHtml = headers
    .map((h) => `<th${h.style ? ` style="${h.style}"` : ""}>${escapeHtml(h.label)}</th>`)
    .join("");

  return `
    <div class="stats-row">
      <div class="stats-group">
        <button class="btn btn-sm btn-icon" id="tracks-refresh" title="Refresh"><i class="fa-solid fa-rotate"></i></button>
        <strong>0</strong> tracks
      </div>
    </div>
    <div class="table-wrap"><table class="data-table">
      <thead><tr>${theadHtml}</tr></thead>
      <tbody><tr><td colspan="8"><div class="text-center text-muted" style="padding:32px">No tracks found. Import tracks from your connected services to get started.</div></td></tr></tbody>
    </table></div>`;
}

/* ------------------------------------------------------------------ */
/*  Fetch + Render cycle                                               */
/* ------------------------------------------------------------------ */

/**
 * Build query string for the tracks endpoint from the given state.
 * Includes playlistId filter when scoped to a playlist.
 */
function buildParams(state) {
  const params = new URLSearchParams();
  params.set("limit", String(PAGE_SIZE));
  params.set("offset", String(state.page * PAGE_SIZE));
  if (state.service !== "all") params.set("service", state.service);
  if (state.search) params.set("search", state.search);
  if (state.playlistId) params.set("playlistId", String(state.playlistId));
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

/**
 * Initialise the tracks page.
 *
 * @param {HTMLElement} container — the #main-content element
 * @param {AbortSignal} signal    — abort signal for ongoing requests
 * @param {object}       hashParams — parsed hash params from app.js
 *   Supported keys:
 *     page         — current page (0-based)
 *     service      — service filter ("all", "spotify", "soundcloud", "youtube")
 *     search       — search query string
 *     playlistId   — playlist ID for scoped view
 *     playlistName — playlist name for scoped view (optional, fetched if missing)
 */
export async function init(container, signal, hashParams) {
  // Parse hash params for initial state
  const state = {
    page: parseInt(hashParams?.page) || 0,
    service: hashParams?.service || "all",
    search: hashParams?.search || "",
    playlistId: hashParams?.playlistId ? parseInt(hashParams.playlistId) : null,
    playlistName: hashParams?.playlistName || null,
  };

  // If playlistId is set but no playlistName provided, fetch it
  if (state.playlistId && !state.playlistName) {
    try {
      const resp = await fetchJSON(`/api/playlists/${state.playlistId}`, { signal });
      if (resp.data) {
        state.playlistName = resp.data.name;
      }
    } catch (err) {
      if (err.name === "AbortError") return;
      console.warn("Failed to fetch playlist name:", err);
    }
  }

  // Render stable toolbar + content wrapper ONCE
  container.innerHTML = `
    ${renderToolbar(state.search, state)}
    <div id="tracks-content">${renderLoading("Loading tracks…")}</div>
  `;

  // Wire search + filter once (toolbar is stable)
  const toolbar = container.querySelector(".toolbar");
  if (toolbar) {
    wireSearchFilter(toolbar, state, () => fetchAndRender(container, signal, state));
  }

  // Wire playlist context clear button
  const clearBtn = container.querySelector(".playlist-context-clear");
  if (clearBtn) {
    clearBtn.onclick = () => {
      state.playlistId = null;
      state.playlistName = null;
      window.location.hash = "#tracks";
      fetchAndRender(container, signal, state);
    };
  }

  // Fetch initial data
  await fetchAndRender(container, signal, state);
}
