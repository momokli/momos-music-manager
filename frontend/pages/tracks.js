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
import { escapeHtml, renderLoading, renderErrorBlock } from "../shared/components.js";
import { formatDuration } from "../shared/format.js";
import {
  renderSearchInput,
  renderFilterGroup,
  wireSearchFilter,
} from "../shared/search-filter.js";
import {
  getPageSize,
  renderPageSizeSelector,
  sortableTh,
  wireSortableHeaders,
  wirePageSizeSelector,
  updateHash,
} from "../shared/crud.js";
import {
  loadColumnConfig,
  renderColumnConfigTrigger,
  renderColumnHeaders,
  renderColumnCells,
  wireColumnResize,
  wireColumnDragReorder,
  wireConfigTrigger,
} from "../shared/column-config.js";

/* ------------------------------------------------------------------ */
/*  Constants                                                          */
/* ------------------------------------------------------------------ */

const SERVICE_OPTIONS = [
  { value: "all", label: "All Services" },
  { value: "spotify", label: "Spotify" },
  { value: "soundcloud", label: "SoundCloud" },
  { value: "youtube", label: "YouTube" },
];

/**
 * Column model for the tracks page.
 * Each entry: { id, label, sortable?, sortKey?, defaultWidth }.
 * User customisations (visibility, width, order) are stored per-page in localStorage.
 */
const TRACKS_COLUMNS = [
  { id: "title", label: "Title", sortable: true, sortKey: "title", defaultWidth: 22 },
  { id: "artist", label: "Artist", sortable: true, sortKey: "artist", defaultWidth: 16 },
  {
    id: "service",
    label: "Service",
    sortable: true,
    sortKey: "service",
    defaultWidth: 8,
  },
  { id: "album", label: "Album", sortable: true, sortKey: "album", defaultWidth: 14 },
  { id: "playlists", label: "Playlists", sortable: false, defaultWidth: 18 },
  { id: "localFiles", label: "Local Files", sortable: false, defaultWidth: 10 },
  {
    id: "duration",
    label: "Duration",
    sortable: true,
    sortKey: "duration_ms",
    defaultWidth: 8,
  },
  { id: "isrc", label: "ISRC", sortable: true, sortKey: "isrc", defaultWidth: 4 },
  {
    id: "imported",
    label: "Imported",
    sortable: true,
    sortKey: "imported_at",
    defaultWidth: 7,
  },
];

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

function formatTimestamp(ts) {
  if (!ts) return '<span class="text-muted">—</span>';
  const d = new Date(ts * 1000);
  return `<span class="font-mono text-xs">${d.toLocaleDateString()}</span>`;
}

/**
 * Cell renderer map for columns.
 * Each function receives a track object and returns HTML for the <td>.
 */
const TRACKS_CELL_RENDERERS = {
  title: (t) => escapeHtml(t.title),
  artist: (t) => escapeHtml(t.artist),
  service: (t) =>
    `<span class="service-badge ${t.service}">${t.service.charAt(0).toUpperCase() + t.service.slice(1)}</span>`,
  album: (t) => (t.album ? escapeHtml(t.album) : '<span class="text-muted">—</span>'),
  playlists: (t) => renderPlaylistBadges(t),
  localFiles: (t) => renderLocalFiles(t),
  duration: (t) =>
    `<span class="font-mono">${escapeHtml(formatDuration(t.duration))}</span>`,
  isrc: (t) =>
    t.isrc
      ? `<span class="font-mono text-sm">${escapeHtml(t.isrc)}</span>`
      : '<span class="text-muted">—</span>',
  imported: (t) =>
    t.importedAt ? formatTimestamp(t.importedAt) : '<span class="text-muted">—</span>',
};

/**
 * Render playlist name badges for a track.
 * @param {object} t — adapted track object with playlistNames array
 * @returns {string} HTML
 */
function renderPlaylistBadges(t) {
  if (!t.playlistNames || t.playlistNames.length === 0) {
    return '<span class="text-muted">—</span>';
  }
  return t.playlistNames
    .map(
      (n) =>
        `<span class="tag-badge font-mono" style="font-size:0.75rem">${escapeHtml(n)}</span>`,
    )
    .join(" ");
}

/**
 * Render local file code badges for a track.
 * @param {object} t — adapted track object with files string (space-joined)
 * @returns {string} HTML
 */
function renderLocalFiles(t) {
  if (!t.files) return '<span class="text-muted">—</span>';
  return t.files
    .split(" ")
    .map((f) => `<code class="font-mono">${escapeHtml(f)}</code>`)
    .join(" ");
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
    importedAt: t.importedAt || null,
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

  return `<div class="filter-panel" id="tracks-filter-panel">
    <div class="filter-panel-header">
      ${renderSearchInput("tracks", search)}
      ${renderFilterGroup("service", SERVICE_OPTIONS, state.service)}
      ${playlistBadge}
      <button class="filter-panel-toggle" id="tracks-filter-toggle" title="Toggle filters">
        <i class="fas fa-chevron-up chevron"></i>
      </button>
    </div>
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
  const totalPages = Math.ceil(totalCount / state.pageSize) || 1;
  const pageId = "tracks";
  const isScoped = !!state.playlistId;

  // Load column config, hide Playlists when scoped to a playlist
  const config = loadColumnConfig("tracks", TRACKS_COLUMNS);
  if (isScoped) {
    const plEntry = config.find((c) => c.id === "playlists");
    if (plEntry) plEntry.visible = false;
  }

  const headers = renderColumnHeaders(config, TRACKS_COLUMNS, state, sortableTh);

  const rowsHtml = tracks
    .map(
      (t) =>
        `<tr>${renderColumnCells(config, TRACKS_COLUMNS, TRACKS_CELL_RENDERERS, t)}</tr>`,
    )
    .join("");

  const stats = `<div class="stats-row">
    <div class="stats-group">
      <button class="btn btn-sm btn-icon" id="tracks-refresh" title="Refresh"><i class="fa-solid fa-rotate"></i></button>
      <strong>${totalCount.toLocaleString()}</strong> tracks
      ${renderPageSizeSelector(state.pageSize)}
      ${renderColumnConfigTrigger()}
      ${
        isScoped
          ? `<span style="margin-left:8px;color:var(--text-subtle)">in playlist <strong>${escapeHtml(state.playlistName)}</strong></span>`
          : ""
      }
    </div>
  </div>`;

  const table = `<div class="table-wrap"><table class="data-table"><thead><tr>${headers}</tr></thead><tbody>${rowsHtml}</tbody></table></div>`;

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
function renderEmptyBody(state) {
  const config = loadColumnConfig("tracks", TRACKS_COLUMNS);
  const isScoped = !!state.playlistId;
  if (isScoped) {
    const plEntry = config.find((c) => c.id === "playlists");
    if (plEntry) plEntry.visible = false;
  }
  const headers = renderColumnHeaders(
    config,
    TRACKS_COLUMNS,
    { sort: "", order: "" },
    sortableTh,
  );
  const visibleCount = config.filter((c) => c.visible).length;

  return `
    <div class="stats-row">
      <div class="stats-group">
        <button class="btn btn-sm btn-icon" id="tracks-refresh" title="Refresh"><i class="fa-solid fa-rotate"></i></button>
        <strong>0</strong> tracks
        ${renderColumnConfigTrigger()}
      </div>
    </div>
    <div class="table-wrap"><table class="data-table">
      <thead><tr>${headers}</tr></thead>
      <tbody><tr><td colspan="${visibleCount}"><div class="text-center text-muted" style="padding:32px">No tracks found. Import tracks from your connected services to get started.</div></td></tr></tbody>
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
  params.set("limit", String(state.pageSize));
  params.set("offset", String(state.page * state.pageSize));
  if (state.sort) params.set("sort", state.sort);
  if (state.order) params.set("order", state.order);
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
  // Sync hash before fetching so URL reflects current state
  updateHash("tracks", state, {
    sort: "",
    order: "asc",
    search: "",
    service: "all",
    page: 0,
  });
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
      setContent(renderEmptyBody(state));
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

  // Sortable column headers (three-state cycle)
  const tableEl = container.querySelector(".data-table");
  if (tableEl) {
    wireSortableHeaders(tableEl, state, () => {
      fetchAndRender(container, signal, state);
    });
  }

  // Page size selector (global via localStorage)
  wirePageSizeSelector(container, state, () => {
    fetchAndRender(container, signal, state);
  });

  // Column resize, reorder, config modal
  const colConfig = loadColumnConfig("tracks", TRACKS_COLUMNS);
  wireColumnResize(container, "tracks", TRACKS_COLUMNS, colConfig);
  wireColumnDragReorder(container, "tracks", TRACKS_COLUMNS, colConfig, () => {
    fetchAndRender(container, signal, state);
  });
  wireConfigTrigger(container, "tracks", TRACKS_COLUMNS, colConfig, () => {
    fetchAndRender(container, signal, state);
  });
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
    pageSize: getPageSize(),
    search: hashParams?.search || "",
    sort: hashParams?.sort || "",
    order: hashParams?.order || "asc",
    service: hashParams?.service || "all",
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
  const filterPanel = container.querySelector("#tracks-filter-panel");
  if (filterPanel) {
    wireSearchFilter(filterPanel, state, () => fetchAndRender(container, signal, state));
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

  // Wire filter panel toggle
  const toggleBtn = container.querySelector("#tracks-filter-toggle");
  if (toggleBtn && filterPanel) {
    // Restore saved state from localStorage
    const saved = localStorage.getItem("filterPanelCollapsed_tracks");
    if (saved === "true") filterPanel.classList.add("collapsed");

    toggleBtn.addEventListener("click", () => {
      filterPanel.classList.toggle("collapsed");
      localStorage.setItem(
        "filterPanelCollapsed_tracks",
        filterPanel.classList.contains("collapsed"),
      );
    });
  }

  // Fetch initial data
  await fetchAndRender(container, signal, state);
}
