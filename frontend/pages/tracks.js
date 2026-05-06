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
 *   #tracks?search=foo&selectedServices=spotify,soundcloud&pmvCategories=p,m&pmvAggregate=full&page=0&playlistId=42&playlistName=Summer
 */

import { fetchJSON } from "../shared/api.js";
import { escapeHtml, renderLoading, renderErrorBlock } from "../shared/components.js";
import { formatDuration } from "../shared/format.js";
import { renderSearchInput, wireSearchFilter } from "../shared/search-filter.js";
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
import { renderActionsPanel } from "../shared/actions-panel.js";

/* ------------------------------------------------------------------ */
/*  Constants                                                          */
/* ------------------------------------------------------------------ */

/**
 * Column model for the tracks page.
 * Each entry: { id, label, sortable?, sortKey?, defaultWidth }.
 * User customisations (visibility, width, order) are stored per-page in localStorage.
 */
const TRACKS_COLUMNS = [
  { id: "title", label: "Title", sortable: true, sortKey: "title", defaultWidth: 220 },
  { id: "artist", label: "Artist", sortable: true, sortKey: "artist", defaultWidth: 160 },
  {
    id: "service",
    label: "Service",
    sortable: true,
    sortKey: "service",
    defaultWidth: 80,
  },
  { id: "album", label: "Album", sortable: true, sortKey: "album", defaultWidth: 140 },
  { id: "playlists", label: "Playlists", sortable: false, defaultWidth: 180 },
  { id: "localFiles", label: "Local Files", sortable: false, defaultWidth: 100 },
  {
    id: "duration",
    label: "Duration",
    sortable: true,
    sortKey: "duration_ms",
    defaultWidth: 80,
  },
  { id: "isrc", label: "ISRC", sortable: true, sortKey: "isrc", defaultWidth: 60 },
  {
    id: "imported",
    label: "Imported",
    sortable: true,
    sortKey: "imported_at",
    defaultWidth: 80,
  },
  {
    id: "latestAdded",
    label: "Latest Added",
    sortable: false,
    defaultWidth: 80,
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
    `<span class="service-badge ${t.service}"><i class="fab fa-${t.service}"></i> ${t.service.charAt(0).toUpperCase() + t.service.slice(1)}</span>`,
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
  latestAdded: (t) =>
    t.maxAddedAt ? formatTimestamp(t.maxAddedAt) : '<span class="text-muted">—</span>',
};

/**
 * Category colors for tag chips.
 */
const CATEGORY_COLORS = {
  Phase: "#f59e0b",
  Mood: "#ef4444",
  Vibe: "#8b5cf6",
  Merkmal: "#06b6d4",
  Setlist: "#10b981",
};

/**
 * Render playlist name badges for a track.
 * Uses enriched playlistTags (with category/prefix/icon) when available,
 * falls back to plain playlistNames otherwise.
 * @param {object} t — adapted track object with playlistTags and playlistNames
 * @returns {string} HTML
 */
function renderPlaylistBadges(t) {
  if (
    (!t.playlistTags || t.playlistTags.length === 0) &&
    (!t.playlistNames || t.playlistNames.length === 0)
  ) {
    return '<span class="text-muted">—</span>';
  }

  // Use enriched tag data if available (has category info)
  if (t.playlistTags && t.playlistTags.length > 0) {
    return t.playlistTags
      .map((pt) => {
        const color = CATEGORY_COLORS[pt.category] || "var(--accent)";
        return `<span class="tag-chip" style="background:${color}18;border:1px solid ${color}44;color:${color}">
          <span class="tag-prefix" style="font-weight:700;opacity:0.7">${escapeHtml(pt.prefix)}</span>
          ${escapeHtml(pt.tagName)}
        </span>`;
      })
      .join(" ");
  }

  // Fallback to plain name badges
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

/**
 * Extract PMV categories present in a comment bracket.
 * Returns { p: bool, m: bool, v: bool }.
 */
function pmvFromComment(comment) {
  if (!comment) return { p: false, m: false, v: false };
  const m = comment.match(/^\[([PMV_]+)\]/);
  if (!m) return { p: false, m: false, v: false };
  return {
    p: m[1].includes("P"),
    m: m[1].includes("M"),
    v: m[1].includes("V"),
  };
}

/**
 * Apply client-side filters that cannot be handled server-side:
 * - Service filter (multi-select OR from icon buttons)
 * - PMV filter (categories or aggregate from comment bracket)
 */
function applyClientFilters(tracks, state) {
  let result = tracks;

  // Service filter (multi-select OR)
  if (state.selectedServices && state.selectedServices.length > 0) {
    result = result.filter(
      (t) => t.service && state.selectedServices.includes(t.service),
    );
  }

  // PMV filter
  if (state.pmvCategories && state.pmvCategories.length > 0) {
    result = result.filter((t) => {
      const pmv = pmvFromComment(t.commentTarget || t.comment || "");
      return state.pmvCategories.some((c) => pmv[c]);
    });
  } else if (state.pmvAggregate) {
    result = result.filter((t) => {
      const pmv = pmvFromComment(t.commentTarget || t.comment || "");
      const hasAny = pmv.p || pmv.m || pmv.v;
      const hasAll = pmv.p && pmv.m && pmv.v;
      switch (state.pmvAggregate) {
        case "full":
          return hasAll;
        case "partial":
          return hasAny;
        case "none":
          return !hasAny;
        default:
          return true;
      }
    });
  }

  return result;
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
    maxAddedAt: t.maxAddedAt || null,
    playlistNames: t.playlistNames || [],
    playlistTags: t.playlistTags || [],
    comment: t.comment || null,
    commentTarget: t.commentTarget || null,
  };
}

/* ------------------------------------------------------------------ */
/*  Render helpers (content area only, toolbar is stable)              */
/* ------------------------------------------------------------------ */

/**
 * Render the toolbar HTML (called once on init).
 * Includes search input, service icon buttons, PMV filter row, and
 * optional playlist context badge when scoped to a playlist.
 *
 * @param {string} search  — current search value
 * @param {object} state   — current state (used for playlistName, selectedServices, pmvCategories, pmvAggregate)
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

  const selServices = state.selectedServices || [];
  const pmvCats = state.pmvCategories || [];

  return `<div class="filter-panel" id="tracks-filter-panel">
    <div class="filter-panel-header">
      ${renderSearchInput("tracks", search)}
      ${playlistBadge}
      <button class="filter-panel-toggle" id="tracks-filter-toggle" title="Toggle filters">
        <i class="fas fa-chevron-up chevron"></i>
      </button>
    </div>
    <div class="filter-panel-body">
      <!-- Service filter -->
      <div class="filter-row">
        <span class="filter-row-label toggleable" data-filter="service">Service</span>
        <div class="filter-group service-filter-group" style="flex-wrap:wrap">
          <button class="filter-btn${selServices.includes("spotify") ? " active" : ""}" data-value="spotify" title="Spotify"><i class="fab fa-spotify"></i></button>
          <button class="filter-btn${selServices.includes("soundcloud") ? " active" : ""}" data-value="soundcloud" title="SoundCloud"><i class="fab fa-soundcloud"></i></button>
          <button class="filter-btn${selServices.includes("youtube") ? " active" : ""}" data-value="youtube" title="YouTube"><i class="fab fa-youtube"></i></button>
        </div>
      </div>
      <!-- PMV filter -->
      <div class="filter-row">
        <span class="filter-row-label toggleable" data-filter="pmv">PMV</span>
        <div class="filter-group" id="track-pmv-cat-btns" style="flex-wrap:wrap">
          <button class="filter-btn${pmvCats.includes("p") ? " active" : ""}" data-value="p" title="Has Phase tags">P</button>
          <button class="filter-btn${pmvCats.includes("m") ? " active" : ""}" data-value="m" title="Has Mood tags">M</button>
          <button class="filter-btn${pmvCats.includes("v") ? " active" : ""}" data-value="v" title="Has Vibe tags">V</button>
        </div>
        <span class="pmv-sep">|</span>
        <div class="filter-group" id="track-pmv-agg-btns" style="flex-wrap:wrap">
          <button class="filter-btn${state.pmvAggregate === "full" ? " active" : ""}" data-value="full" title="Has all three categories">Full</button>
          <button class="filter-btn${state.pmvAggregate === "partial" ? " active" : ""}" data-value="partial" title="Has at least one category">Partial</button>
          <button class="filter-btn${state.pmvAggregate === "none" ? " active" : ""}" data-value="none" title="Has no PMV categories">None</button>
        </div>
      </div>
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
      state.layoutMode
        ? '<button class="btn btn-sm btn-primary" id="tracks-layout-btn" style="margin-left:8px"><i class="fas fa-check"></i> Done</button>'
        : '<button class="btn btn-sm" id="tracks-layout-btn" style="margin-left:8px"><i class="fas fa-arrows-alt"></i> Modify Column Layout</button>'
    }
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
    selectedServices: [],
    pmvCategories: [],
    pmvAggregate: "",
    page: 0,
  });
  setContent(renderLoading("Loading tracks…"));

  try {
    const [tracksResp, countResp] = await Promise.all([
      fetchJSON(`/api/tracks?${buildParams(state)}`, { signal }),
      fetchJSON(`/api/tracks/count?${buildParams(state)}`, { signal }),
    ]);
    if (signal.aborted) return;

    let clientTracks = tracksResp.data.map(adaptTrack);
    // Apply client-side filters (service multi-select, PMV)
    clientTracks = applyClientFilters(clientTracks, state);

    const data = {
      _total: countResp.data,
      tracks: clientTracks,
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

/**
 * Wire toolbar filter events (called once after toolbar is mounted).
 * Handles multi-select service icon buttons and PMV category/aggregate buttons.
 */
function wireToolbarEvents(container, signal, state) {
  // ── Multi-select service filter ──
  const serviceGroup = container.querySelector(".service-filter-group");
  if (serviceGroup) {
    serviceGroup.addEventListener(
      "click",
      (e) => {
        const btn = e.target.closest(".filter-btn");
        if (!btn) return;
        const value = btn.dataset.value;
        const idx = state.selectedServices.indexOf(value);
        if (idx >= 0) {
          state.selectedServices.splice(idx, 1);
        } else {
          state.selectedServices.push(value);
        }
        state.page = 0;
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  }

  // ── PMV category buttons (multi-select: P, M, V) ──
  const pmvCatBtns = container.querySelector("#track-pmv-cat-btns");
  if (pmvCatBtns) {
    pmvCatBtns.addEventListener(
      "click",
      (e) => {
        const btn = e.target.closest(".filter-btn");
        if (!btn) return;
        const val = btn.dataset.value;
        const idx = state.pmvCategories.indexOf(val);
        if (idx >= 0) {
          state.pmvCategories.splice(idx, 1);
        } else {
          // Clear aggregate group when picking categories
          state.pmvAggregate = "";
          container
            .querySelectorAll("#track-pmv-agg-btns .filter-btn")
            .forEach((b) => b.classList.remove("active"));
          state.pmvCategories.push(val);
        }
        state.page = 0;
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  }

  // ── PMV aggregate buttons (single-select: Full, Partial, None) ──
  const pmvAggBtns = container.querySelector("#track-pmv-agg-btns");
  if (pmvAggBtns) {
    pmvAggBtns.addEventListener(
      "click",
      (e) => {
        const btn = e.target.closest(".filter-btn");
        if (!btn) return;
        const val = btn.dataset.value;
        if (state.pmvAggregate === val) {
          state.pmvAggregate = "";
        } else {
          // Clear category group when picking aggregate
          state.pmvCategories = [];
          container
            .querySelectorAll("#track-pmv-cat-btns .filter-btn")
            .forEach((b) => b.classList.remove("active"));
          state.pmvAggregate = val;
        }
        state.page = 0;
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  }
}

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
  if (state.layoutMode) {
    wireColumnResize(container, "tracks", TRACKS_COLUMNS, colConfig);
    wireColumnDragReorder(container, "tracks", TRACKS_COLUMNS, colConfig, () => {
      fetchAndRender(container, signal, state);
    });
  }
  wireConfigTrigger(container, "tracks", TRACKS_COLUMNS, colConfig, () => {
    fetchAndRender(container, signal, state);
  });

  // Layout mode toggle
  const layoutBtn = container.querySelector("#tracks-layout-btn");
  if (layoutBtn) {
    layoutBtn.onclick = () => {
      state.layoutMode = !state.layoutMode;
      document.body.classList.toggle("layout-mode", state.layoutMode);
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
  const parseCSV = (val) => (val ? val.split(",").filter(Boolean) : []);

  const state = {
    page: parseInt(hashParams?.page) || 0,
    pageSize: getPageSize(),
    search: hashParams?.search || "",
    sort: hashParams?.sort || "",
    order: hashParams?.order || "asc",
    selectedServices: parseCSV(hashParams?.selectedServices),
    pmvCategories: parseCSV(hashParams?.pmvCategories),
    pmvAggregate: hashParams?.pmvAggregate || "",
    playlistId: hashParams?.playlistId ? parseInt(hashParams.playlistId) : null,
    playlistName: hashParams?.playlistName || null,
    layoutMode: false,
  };

  // Reset layout mode on page entry
  document.body.classList.remove("layout-mode");

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

  // Render stable toolbar + actions panel + content wrapper ONCE
  container.innerHTML = `
    <div style="display:flex;flex-direction:column;gap:var(--space-4);">
      <div style="display:flex;gap:var(--space-4);align-items:flex-start;">
        <div style="flex:4;min-width:0;">${renderToolbar(state.search, state)}</div>
        <div class="actions-panel" style="flex:1;min-width:180px;max-width:220px;">
          <div class="actions-panel-header">
            <span><i class="fas fa-bolt"></i> Actions</span>
            <span class="actions-sel-count" id="tracks-sel-count">0</span>
          </div>
          <button class="btn btn-sm" id="tracks-actions-refresh"><i class="fas fa-rotate"></i> Refresh</button>
        </div>
      </div>
      <div id="tracks-content" style="min-height:200px;">${renderLoading("Loading tracks…")}</div>
    </div>`;

  // Wire search + filter once (toolbar is stable)
  const filterPanel = container.querySelector("#tracks-filter-panel");
  if (filterPanel) {
    wireSearchFilter(filterPanel, state, () => fetchAndRender(container, signal, state));
  }

  // Wire toolbar filter events (service icons, PMV)
  wireToolbarEvents(container, signal, state);

  // Wire actions panel refresh
  import("../shared/actions-panel.js").then(({ wireActionsRefresh }) => {
    wireActionsRefresh(container, "tracks", () => {
      state.page = 0;
      return fetchAndRender(container, signal, state);
    });
  });

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
