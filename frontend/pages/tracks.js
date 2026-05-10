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
 *   #tracks?search=foo&selectedServices=spotify,soundcloud&page=0&playlistId=42&playlistName=Summer
 */

import { fetchJSON } from "../shared/api.js";
import {
  escapeHtml,
  renderLoading,
  renderErrorBlock,
  showToast,
} from "../shared/components.js";
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
import { renderActionsPanel, updateSelectionCount } from "../shared/actions-panel.js";

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
 * Includes search input, service icon buttons, and
 * optional playlist context badge when scoped to a playlist.
 *
 * @param {string} search  — current search value
 * @param {object} state   — current state (used for playlistName, selectedServices)
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
  const chipsHtml = (state.selectedTags || [])
    .map(
      (t) =>
        `<span class="tag-chip" data-tag="${t}">${escapeHtml(t)} <i class="fas fa-times tag-chip-x"></i></span>`,
    )
    .join("");

  return `<div class="filter-panel" id="tracks-filter-panel">
    <div class="filter-panel-header">
      ${renderSearchInput("tracks", search)}
      ${playlistBadge}
      <button class="filter-panel-toggle" id="tracks-filter-toggle" title="Toggle filters">
        <i class="fas fa-chevron-up chevron"></i>
      </button>
    </div>
    <div class="filter-panel-body">
      <div class="filter-panel-scroll" style="display:grid;grid-template-columns:1fr 1fr;gap:var(--space-2) var(--space-4);">
        <!-- LEFT COLUMN: Track Info -->
        <div>
          <div class="filter-section-header" style="margin-top:0"><i class="fas fa-music"></i> Track Info</div>

          <!-- Tags filter (typeahead + chips) -->
          <div class="filter-row">
            <span class="filter-row-label toggleable" data-filter="tag">Tags</span>
            <div class="typeahead-wrap" style="flex:1">
              <div class="tag-search-wrap">
                <i class="fas fa-tag"></i>
                <input type="text" class="input-text input-search" id="tracks-tag-search"
                       placeholder="filter by TAG" autocomplete="off">
                <div class="tag-dropdown" id="tracks-tag-dropdown"></div>
              </div>
            </div>
            <div class="tag-chips" id="tracks-tag-chips">${chipsHtml}</div>
          </div>

          <!-- Date filter -->
          <div class="filter-row">
            <span class="filter-row-label toggleable" data-filter="date">Date</span>
            <div style="display:flex;flex-direction:column;gap:4px;flex:1">
              <!-- Imported row -->
              <div style="display:flex;align-items:center;gap:4px;flex-wrap:wrap">
                <span style="font-size:0.7rem;color:var(--text-subtle);min-width:55px">Imported</span>
                <select class="input-select" id="tracks-imported-mode" style="width:70px;font-size:0.7rem;padding:2px 4px;">
                  <option value="">—</option>
                  <option value="since" ${state.importedMode === "since" ? "selected" : ""}>Since</option>
                  <option value="before" ${state.importedMode === "before" ? "selected" : ""}>Before</option>
                </select>
                <input type="number" class="input-text" id="tracks-imported-num"
                       value="${state.importedNum ?? ""}" placeholder="#"
                       style="width:50px;font-size:0.7rem;padding:2px 4px;" min="1">
                <select class="input-select" id="tracks-imported-unit" style="width:75px;font-size:0.7rem;padding:2px 4px;">
                  <option value="days" ${state.importedUnit === "days" ? "selected" : ""}>Days</option>
                  <option value="weeks" ${state.importedUnit === "weeks" ? "selected" : ""}>Weeks</option>
                  <option value="months" ${state.importedUnit === "months" ? "selected" : ""}>Months</option>
                </select>
              </div>
              <!-- Latest Added row -->
              <div style="display:flex;align-items:center;gap:4px;flex-wrap:wrap">
                <span style="font-size:0.7rem;color:var(--text-subtle);min-width:55px">Added</span>
                <select class="input-select" id="tracks-added-mode" style="width:70px;font-size:0.7rem;padding:2px 4px;">
                  <option value="">—</option>
                  <option value="since" ${state.addedMode === "since" ? "selected" : ""}>Since</option>
                  <option value="before" ${state.addedMode === "before" ? "selected" : ""}>Before</option>
                </select>
                <input type="number" class="input-text" id="tracks-added-num"
                       value="${state.addedNum ?? ""}" placeholder="#"
                       style="width:50px;font-size:0.7rem;padding:2px 4px;" min="1">
                <select class="input-select" id="tracks-added-unit" style="width:75px;font-size:0.7rem;padding:2px 4px;">
                  <option value="days" ${state.addedUnit === "days" ? "selected" : ""}>Days</option>
                  <option value="weeks" ${state.addedUnit === "weeks" ? "selected" : ""}>Weeks</option>
                  <option value="months" ${state.addedUnit === "months" ? "selected" : ""}>Months</option>
                </select>
              </div>
            </div>
          </div>
        </div>

        <!-- RIGHT COLUMN: Classification -->
        <div>
          <div class="filter-section-header" style="margin-top:0"><i class="fas fa-tag"></i> Classification</div>

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
            <div class="filter-group" id="pmv-cat-btns" style="flex-wrap:wrap">
              <button class="filter-btn${(state.pmvCategories || []).includes("p") ? " active" : ""}" data-value="p" title="Has Phase tags">P</button>
              <button class="filter-btn${(state.pmvCategories || []).includes("m") ? " active" : ""}" data-value="m" title="Has Mood tags">M</button>
              <button class="filter-btn${(state.pmvCategories || []).includes("v") ? " active" : ""}" data-value="v" title="Has Vibe tags">V</button>
            </div>
            <span class="pmv-sep">|</span>
            <div class="filter-group" id="pmv-agg-btns" style="flex-wrap:wrap">
              <button class="filter-btn${state.pmvAggregate === "full" ? " active" : ""}" data-value="full" title="Has all three categories">Full</button>
              <button class="filter-btn${state.pmvAggregate === "partial" ? " active" : ""}" data-value="partial" title="Has at least one category">Partial</button>
              <button class="filter-btn${state.pmvAggregate === "none" ? " active" : ""}" data-value="none" title="Has no PMV categories">None</button>
            </div>
          </div>

          <!-- Type filter -->
          <div class="filter-row">
            <span class="filter-row-label toggleable" data-filter="type">Type</span>
            <div class="filter-group" id="tracks-filetype-filter" style="flex-wrap:wrap">
              <button class="filter-btn${(state.fileTypes || []).includes("flac") ? " active" : ""}" data-value="flac">FLAC</button>
              <button class="filter-btn${(state.fileTypes || []).includes("mp3") ? " active" : ""}" data-value="mp3">MP3</button>
              <button class="filter-btn${(state.fileTypes || []).includes("stem.m4a") ? " active" : ""}" data-value="stem.m4a">Stem</button>
              <button class="filter-btn${(state.fileTypes || []).includes("wav") ? " active" : ""}" data-value="wav">WAV</button>
            </div>
            <span class="pmv-sep">|</span>
            <div class="filter-group" id="tracks-filetype-agg-btns" style="flex-wrap:wrap">
              <button class="filter-btn${state.fileTypeAgg === "any" ? " active" : ""}" data-value="any" title="Has at least one local file">Some</button>
              <button class="filter-btn${state.fileTypeAgg === "none" ? " active" : ""}" data-value="none" title="Has no local file">None</button>
            </div>
          </div>
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
  const selectedSet = state.selectedTrackIds || new Set();

  // Load column config, hide Playlists when scoped to a playlist
  const config = loadColumnConfig("tracks", TRACKS_COLUMNS);
  if (isScoped) {
    const plEntry = config.find((c) => c.id === "playlists");
    if (plEntry) plEntry.visible = false;
  }

  const dataHeaders = renderColumnHeaders(config, TRACKS_COLUMNS, state, sortableTh);

  // Checkbox column header (select-all, outside column config)
  const allOnPageSelected =
    tracks.length > 0 && tracks.every((t) => selectedSet.has(t.id));
  const checkboxHeader =
    '<th class="col-checkbox"><input type="checkbox" class="tracks-select-all" id="tracks-select-all"' +
    (allOnPageSelected ? " checked" : "") +
    "></th>";
  const headers = checkboxHeader + dataHeaders;

  const rowsHtml = tracks
    .map((t) => {
      const checked = selectedSet.has(t.id);
      const cb =
        '<td class="col-checkbox"><input type="checkbox" class="tracks-row-checkbox" data-track-id="' +
        t.id +
        '"' +
        (checked ? " checked" : "") +
        "></td>";
      return `<tr>${cb}${renderColumnCells(config, TRACKS_COLUMNS, TRACKS_CELL_RENDERERS, t)}</tr>`;
    })
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
  const dataHeaders = renderColumnHeaders(
    config,
    TRACKS_COLUMNS,
    { sort: "", order: "" },
    sortableTh,
  );
  const checkboxHeader = '<th class="col-checkbox"></th>';
  const headers = checkboxHeader + dataHeaders;
  const visibleCount = config.filter((c) => c.visible).length + 1;

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
  // Server-side filters
  if (state.selectedServices && state.selectedServices.length > 0) {
    params.set("services", state.selectedServices.join(","));
  }
  if (state.selectedTags && state.selectedTags.length > 0) {
    params.set("tags", state.selectedTags.join(","));
  }
  if (state.pmvCategories && state.pmvCategories.length > 0) {
    params.set("pmvCategories", state.pmvCategories.join(","));
  }
  if (state.pmvAggregate) {
    params.set("pmvAggregate", state.pmvAggregate);
  }
  if (state.fileTypes && state.fileTypes.length > 0) {
    params.set("fileTypes", state.fileTypes.join(","));
  }
  if (state.fileTypeAgg) {
    params.set("fileTypeAgg", state.fileTypeAgg);
  }
  // Date filters — convert weeks/months to days
  function toDays(num, unit) {
    if (!num || num <= 0) return null;
    switch (unit) {
      case "weeks":
        return num * 7;
      case "months":
        return num * 30;
      default:
        return num;
    }
  }
  const importedDays = toDays(state.importedNum, state.importedUnit);
  const addedDays = toDays(state.addedNum, state.addedUnit);
  if (state.importedMode === "since" && importedDays) {
    params.set("importedAfterDays", String(importedDays));
  } else if (state.importedMode === "before" && importedDays) {
    params.set("importedBeforeDays", String(importedDays));
  }
  if (state.addedMode === "since" && addedDays) {
    params.set("addedAfterDays", String(addedDays));
  } else if (state.addedMode === "before" && addedDays) {
    params.set("addedBeforeDays", String(addedDays));
  }
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
    selectedTags: [],
    pmvCategories: [],
    pmvAggregate: "",
    fileTypes: [],
    fileTypeAgg: "",
    importedMode: "",
    importedNum: null,
    importedUnit: "days",
    addedMode: "",
    addedNum: null,
    addedUnit: "days",
    page: 0,
  });
  setContent(renderLoading("Loading tracks…"));

  try {
    const [tracksResp, countResp] = await Promise.all([
      fetchJSON(`/api/tracks?${buildParams(state)}`, { signal }),
      fetchJSON(`/api/tracks/count?${buildParams(state)}`, { signal }),
    ]);
    if (signal && signal.aborted) return;

    const clientTracks = tracksResp.data.map(adaptTrack);

    const data = {
      _total: countResp.data,
      tracks: clientTracks,
    };

    // Empty state (no tracks in DB at all)
    if (data.tracks.length === 0 && data._total === 0) {
      setContent(renderEmptyBody(state));
      wireContentEvents(container, signal, state);
      updateSelectionUI(container, state);
      return;
    }

    setContent(renderBody(data, state));
    wireContentEvents(container, signal, state);
    updateSelectionUI(container, state);
  } catch (err) {
    if (err.name === "AbortError") return;
    try {
      if (signal && signal.aborted) return;
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
 * Handles all filter interactions: service, tags, PMV, type, date,
 * and generic toggleable labels.
 */
function wireToolbarEvents(container, signal, state) {
  const filterPanel = container.querySelector("#tracks-filter-panel");

  // ── Multi-select service filter (fixes active class bug) ──
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
          btn.classList.remove("active");
        } else {
          state.selectedServices.push(value);
          btn.classList.add("active");
        }
        state.page = 0;
        updateHash("tracks", state, {
          sort: "",
          order: "asc",
          search: "",
          selectedServices: [],
          selectedTags: [],
          pmvCategories: [],
          pmvAggregate: "",
          fileTypes: [],
          fileTypeAgg: "",
          importedMode: "",
          importedNum: null,
          importedUnit: "days",
          addedMode: "",
          addedNum: null,
          addedUnit: "days",
          page: 0,
        });
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  }

  // ── Tag search input with keyboard navigation (like Files page) ──
  const tagSearch = container.querySelector("#tracks-tag-search");
  const tagDropdown = container.querySelector("#tracks-tag-dropdown");
  if (tagSearch && tagDropdown) {
    let timer;
    let selectedIndex = -1;

    function updateSelection() {
      const items = tagDropdown.querySelectorAll(".tag-dropdown-item");
      items.forEach((item, i) => {
        item.classList.toggle("selected", i === selectedIndex);
      });
      const selected = items[selectedIndex];
      if (selected) {
        selected.scrollIntoView({ block: "nearest" });
      }
    }

    function addSelectedTag() {
      const items = tagDropdown.querySelectorAll(".tag-dropdown-item");
      const selected = items[selectedIndex];
      if (!selected) return;
      const tag = selected.dataset.tag;
      if (!tag) return;
      if (!state.selectedTags.includes(tag)) {
        state.selectedTags.push(tag);
        state.page = 0;
      }
      tagSearch.value = "";
      tagDropdown.classList.remove("open");
      tagDropdown.innerHTML = "";
      selectedIndex = -1;
      renderTagChips();
      updateHash("tracks", state, {
        sort: "",
        order: "asc",
        search: "",
        selectedServices: [],
        selectedTags: [],
        pmvCategories: [],
        pmvAggregate: "",
        fileTypes: [],
        fileTypeAgg: "",
        importedMode: "",
        importedNum: null,
        importedUnit: "days",
        addedMode: "",
        addedNum: null,
        addedUnit: "days",
        page: 0,
      });
      fetchAndRender(container, signal, state);
    }

    tagSearch.addEventListener(
      "input",
      () => {
        clearTimeout(timer);
        selectedIndex = -1;
        const q = tagSearch.value.trim();
        if (!q) {
          tagDropdown.classList.remove("open");
          tagDropdown.innerHTML = "";
          return;
        }
        timer = setTimeout(async () => {
          try {
            const resp = await fetchJSON(`/api/tags?search=${encodeURIComponent(q)}`);
            const tags = resp.data || [];
            if (tags.length === 0) {
              tagDropdown.innerHTML = `<div class="tag-dropdown-empty">No tags found</div>`;
              selectedIndex = -1;
            } else {
              tagDropdown.innerHTML = tags
                .map(
                  (t, i) =>
                    `<div class="tag-dropdown-item${i === 0 ? " selected" : ""}" data-tag="${t.name}">
                      <span class="tag-dropdown-name">${t.name}</span>
                      ${t.category ? `<span class="tag-dropdown-cat">${t.category}</span>` : ""}
                    </div>`,
                )
                .join("");
              selectedIndex = 0;
            }
            tagDropdown.classList.add("open");
          } catch {
            // ignore errors during search
          }
        }, 150);
      },
      { signal },
    );

    tagDropdown.addEventListener(
      "click",
      (e) => {
        const item = e.target.closest(".tag-dropdown-item");
        if (!item) return;
        const tag = item.dataset.tag;
        if (!tag) return;
        if (!state.selectedTags.includes(tag)) {
          state.selectedTags.push(tag);
          state.page = 0;
        }
        tagSearch.value = "";
        tagDropdown.classList.remove("open");
        tagDropdown.innerHTML = "";
        selectedIndex = -1;
        renderTagChips();
        updateHash("tracks", state, {
          sort: "",
          order: "asc",
          search: "",
          selectedServices: [],
          selectedTags: [],
          pmvCategories: [],
          pmvAggregate: "",
          fileTypes: [],
          fileTypeAgg: "",
          importedMode: "",
          importedNum: null,
          importedUnit: "days",
          addedMode: "",
          addedNum: null,
          addedUnit: "days",
          page: 0,
        });
        fetchAndRender(container, signal, state);
      },
      { signal },
    );

    tagSearch.addEventListener(
      "keydown",
      (e) => {
        if (!tagDropdown.classList.contains("open")) return;
        const items = tagDropdown.querySelectorAll(".tag-dropdown-item");
        switch (e.key) {
          case "ArrowDown":
            e.preventDefault();
            if (items.length === 0) return;
            selectedIndex = Math.min(selectedIndex + 1, items.length - 1);
            updateSelection();
            break;
          case "ArrowUp":
            e.preventDefault();
            if (items.length === 0) return;
            selectedIndex = Math.max(selectedIndex - 1, 0);
            updateSelection();
            break;
          case "Enter":
            e.preventDefault();
            addSelectedTag();
            break;
          case "Escape":
            tagDropdown.classList.remove("open");
            tagDropdown.innerHTML = "";
            selectedIndex = -1;
            tagSearch.blur();
            break;
        }
      },
      { signal },
    );

    // Close dropdown on outside click
    document.addEventListener(
      "click",
      (e) => {
        const wrap = container.querySelector(".tag-search-wrap");
        if (!wrap || wrap.contains(e.target)) return;
        if (tagDropdown) {
          tagDropdown.classList.remove("open");
          tagDropdown.innerHTML = "";
          selectedIndex = -1;
        }
      },
      { signal },
    );
  }

  // ── Tag chip rendering helper ──
  function renderTagChips() {
    const chipsContainer = container.querySelector("#tracks-tag-chips");
    if (!chipsContainer) return;
    chipsContainer.innerHTML = state.selectedTags
      .map(
        (t) =>
          `<span class="tag-chip" data-tag="${t}">${escapeHtml(t)} <i class="fas fa-times tag-chip-x"></i></span>`,
      )
      .join("");
  }

  // ── Tag chip removal (delegated) ──
  const chipsContainer = container.querySelector("#tracks-tag-chips");
  if (chipsContainer) {
    chipsContainer.addEventListener(
      "click",
      (e) => {
        const x = e.target.closest(".tag-chip-x");
        if (!x) return;
        const chip = x.closest(".tag-chip");
        if (!chip) return;
        const tag = chip.dataset.tag;
        state.selectedTags = state.selectedTags.filter((t) => t !== tag);
        state.page = 0;
        updateHash("tracks", state, {
          sort: "",
          order: "asc",
          search: "",
          selectedServices: [],
          selectedTags: [],
          pmvCategories: [],
          pmvAggregate: "",
          fileTypes: [],
          fileTypeAgg: "",
          importedMode: "",
          importedNum: null,
          importedUnit: "days",
          addedMode: "",
          addedNum: null,
          addedUnit: "days",
          page: 0,
        });
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  }

  // ── PMV category buttons (multi-select: P, M, V) ──
  const pmvCatBtns = container.querySelector("#pmv-cat-btns");
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
          btn.classList.remove("active");
        } else {
          // Clear aggregate group when picking categories
          state.pmvAggregate = "";
          container
            .querySelectorAll("#pmv-agg-btns .filter-btn")
            .forEach((b) => b.classList.remove("active"));
          state.pmvCategories.push(val);
          btn.classList.add("active");
        }
        state.page = 0;
        updateHash("tracks", state, {
          sort: "",
          order: "asc",
          search: "",
          selectedServices: [],
          selectedTags: [],
          pmvCategories: [],
          pmvAggregate: "",
          fileTypes: [],
          fileTypeAgg: "",
          importedMode: "",
          importedNum: null,
          importedUnit: "days",
          addedMode: "",
          addedNum: null,
          addedUnit: "days",
          page: 0,
        });
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  }

  // ── PMV aggregate buttons (single-select: Full, Partial, None) ──
  const pmvAggBtns = container.querySelector("#pmv-agg-btns");
  if (pmvAggBtns) {
    pmvAggBtns.addEventListener(
      "click",
      (e) => {
        const btn = e.target.closest(".filter-btn");
        if (!btn) return;
        const val = btn.dataset.value;
        if (state.pmvAggregate === val) {
          state.pmvAggregate = "";
          btn.classList.remove("active");
        } else {
          // Clear category group when picking aggregate
          state.pmvCategories = [];
          container
            .querySelectorAll("#pmv-cat-btns .filter-btn")
            .forEach((b) => b.classList.remove("active"));
          state.pmvAggregate = val;
          container
            .querySelectorAll("#pmv-agg-btns .filter-btn")
            .forEach((b) => b.classList.remove("active"));
          btn.classList.add("active");
        }
        state.page = 0;
        updateHash("tracks", state, {
          sort: "",
          order: "asc",
          search: "",
          selectedServices: [],
          selectedTags: [],
          pmvCategories: [],
          pmvAggregate: "",
          fileTypes: [],
          fileTypeAgg: "",
          importedMode: "",
          importedNum: null,
          importedUnit: "days",
          addedMode: "",
          addedNum: null,
          addedUnit: "days",
          page: 0,
        });
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  }

  // ── Multi-select file type filter ──
  const filetypeBtns = container.querySelector("#tracks-filetype-filter");
  if (filetypeBtns) {
    filetypeBtns.addEventListener(
      "click",
      (e) => {
        const btn = e.target.closest(".filter-btn");
        if (!btn) return;
        const value = btn.dataset.value;
        const idx = state.fileTypes.indexOf(value);
        if (idx >= 0) {
          state.fileTypes.splice(idx, 1);
          btn.classList.remove("active");
        } else {
          // Clear aggregate group when picking specific types
          state.fileTypeAgg = "";
          container
            .querySelectorAll("#tracks-filetype-agg-btns .filter-btn")
            .forEach((b) => b.classList.remove("active"));
          state.fileTypes.push(value);
          btn.classList.add("active");
        }
        state.page = 0;
        updateHash("tracks", state, {
          sort: "",
          order: "asc",
          search: "",
          selectedServices: [],
          selectedTags: [],
          pmvCategories: [],
          pmvAggregate: "",
          fileTypes: [],
          fileTypeAgg: "",
          importedMode: "",
          importedNum: null,
          importedUnit: "days",
          addedMode: "",
          addedNum: null,
          addedUnit: "days",
          page: 0,
        });
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  }

  // ── File type aggregate buttons (single-select: Some, None) ──
  const filetypeAggBtns = container.querySelector("#tracks-filetype-agg-btns");
  if (filetypeAggBtns) {
    filetypeAggBtns.addEventListener(
      "click",
      (e) => {
        const btn = e.target.closest(".filter-btn");
        if (!btn) return;
        const val = btn.dataset.value;
        if (state.fileTypeAgg === val) {
          state.fileTypeAgg = "";
          btn.classList.remove("active");
        } else {
          // Clear specific types when picking aggregate
          state.fileTypes = [];
          container
            .querySelectorAll("#tracks-filetype-filter .filter-btn")
            .forEach((b) => b.classList.remove("active"));
          state.fileTypeAgg = val;
          container
            .querySelectorAll("#tracks-filetype-agg-btns .filter-btn")
            .forEach((b) => b.classList.remove("active"));
          btn.classList.add("active");
        }
        state.page = 0;
        updateHash("tracks", state, {
          sort: "",
          order: "asc",
          search: "",
          selectedServices: [],
          selectedTags: [],
          pmvCategories: [],
          pmvAggregate: "",
          fileTypes: [],
          fileTypeAgg: "",
          importedMode: "",
          importedNum: null,
          importedUnit: "days",
          addedMode: "",
          addedNum: null,
          addedUnit: "days",
          page: 0,
        });
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  }

  // ── Date filter wiring ──
  const importedMode = container.querySelector("#tracks-imported-mode");
  const importedNum = container.querySelector("#tracks-imported-num");
  const importedUnit = container.querySelector("#tracks-imported-unit");
  const addedMode = container.querySelector("#tracks-added-mode");
  const addedNum = container.querySelector("#tracks-added-num");
  const addedUnit = container.querySelector("#tracks-added-unit");

  function wireDateFilter() {
    if (importedMode) {
      importedMode.addEventListener(
        "change",
        () => {
          state.importedMode = importedMode.value;
          state.page = 0;
          updateHash("tracks", state, {
            sort: "",
            order: "asc",
            search: "",
            selectedServices: [],
            selectedTags: [],
            pmvCategories: [],
            pmvAggregate: "",
            fileTypes: [],
            fileTypeAgg: "",
            importedMode: "",
            importedNum: null,
            importedUnit: "days",
            addedMode: "",
            addedNum: null,
            addedUnit: "days",
            page: 0,
          });
          fetchAndRender(container, signal, state);
        },
        { signal },
      );
    }
    if (importedNum) {
      importedNum.addEventListener(
        "input",
        () => {
          const val = importedNum.value.trim();
          state.importedNum = val ? parseInt(val, 10) : null;
          state.page = 0;
          updateHash("tracks", state, {
            sort: "",
            order: "asc",
            search: "",
            selectedServices: [],
            selectedTags: [],
            pmvCategories: [],
            pmvAggregate: "",
            fileTypes: [],
            fileTypeAgg: "",
            importedMode: "",
            importedNum: null,
            importedUnit: "days",
            addedMode: "",
            addedNum: null,
            addedUnit: "days",
            page: 0,
          });
          fetchAndRender(container, signal, state);
        },
        { signal },
      );
    }
    if (importedUnit) {
      importedUnit.addEventListener(
        "change",
        () => {
          state.importedUnit = importedUnit.value;
          state.page = 0;
          updateHash("tracks", state, {
            sort: "",
            order: "asc",
            search: "",
            selectedServices: [],
            selectedTags: [],
            pmvCategories: [],
            pmvAggregate: "",
            fileTypes: [],
            fileTypeAgg: "",
            importedMode: "",
            importedNum: null,
            importedUnit: "days",
            addedMode: "",
            addedNum: null,
            addedUnit: "days",
            page: 0,
          });
          fetchAndRender(container, signal, state);
        },
        { signal },
      );
    }
    if (addedMode) {
      addedMode.addEventListener(
        "change",
        () => {
          state.addedMode = addedMode.value;
          state.page = 0;
          updateHash("tracks", state, {
            sort: "",
            order: "asc",
            search: "",
            selectedServices: [],
            selectedTags: [],
            pmvCategories: [],
            pmvAggregate: "",
            fileTypes: [],
            fileTypeAgg: "",
            importedMode: "",
            importedNum: null,
            importedUnit: "days",
            addedMode: "",
            addedNum: null,
            addedUnit: "days",
            page: 0,
          });
          fetchAndRender(container, signal, state);
        },
        { signal },
      );
    }
    if (addedNum) {
      addedNum.addEventListener(
        "input",
        () => {
          const val = addedNum.value.trim();
          state.addedNum = val ? parseInt(val, 10) : null;
          state.page = 0;
          updateHash("tracks", state, {
            sort: "",
            order: "asc",
            search: "",
            selectedServices: [],
            selectedTags: [],
            pmvCategories: [],
            pmvAggregate: "",
            fileTypes: [],
            fileTypeAgg: "",
            importedMode: "",
            importedNum: null,
            importedUnit: "days",
            addedMode: "",
            addedNum: null,
            addedUnit: "days",
            page: 0,
          });
          fetchAndRender(container, signal, state);
        },
        { signal },
      );
    }
    if (addedUnit) {
      addedUnit.addEventListener(
        "change",
        () => {
          state.addedUnit = addedUnit.value;
          state.page = 0;
          updateHash("tracks", state, {
            sort: "",
            order: "asc",
            search: "",
            selectedServices: [],
            selectedTags: [],
            pmvCategories: [],
            pmvAggregate: "",
            fileTypes: [],
            fileTypeAgg: "",
            importedMode: "",
            importedNum: null,
            importedUnit: "days",
            addedMode: "",
            addedNum: null,
            addedUnit: "days",
            page: 0,
          });
          fetchAndRender(container, signal, state);
        },
        { signal },
      );
    }
  }
  wireDateFilter();

  // ── Generic toggle for data-filter labels ──
  filterPanel?.querySelectorAll("[data-filter]").forEach((label) => {
    function updateFilterUI() {
      const key = label.dataset.filter + "Enabled";
      const isActive = state[key] !== false;
      label.classList.toggle("active", isActive);
      label.classList.toggle("off", !isActive);
      const row = label.closest(".filter-row");
      if (row) {
        const inputs = row.querySelectorAll(
          "select, input, button, .filter-group, .tag-chips, .typeahead-wrap",
        );
        inputs.forEach((el) => el.classList.toggle("filter-disabled", !isActive));
      }
    }
    label.addEventListener(
      "click",
      () => {
        const key = label.dataset.filter + "Enabled";
        state[key] = state[key] === false ? true : false;
        state.page = 0;
        updateFilterUI();
        updateHash("tracks", state, {
          sort: "",
          order: "asc",
          search: "",
          selectedServices: [],
          selectedTags: [],
          pmvCategories: [],
          pmvAggregate: "",
          fileTypes: [],
          fileTypeAgg: "",
          importedMode: "",
          importedNum: null,
          importedUnit: "days",
          addedMode: "",
          addedNum: null,
          addedUnit: "days",
          page: 0,
        });
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
    updateFilterUI();
  });

  // ── Auto-enable disabled filter sections on click ──
  filterPanel?.addEventListener(
    "click",
    (e) => {
      const row = e.target.closest(".filter-row");
      if (!row) return;
      const label = row.querySelector("[data-filter]");
      if (!label) return;
      const key = label.dataset.filter + "Enabled";
      if (state[key] !== false) return;
      if (e.target.closest("[data-filter]")) return;
      state[key] = true;
      state.page = 0;
      label.classList.add("active");
      label.classList.remove("off");
      row
        .querySelectorAll(
          "select, input, button, .filter-group, .tag-chips, .typeahead-wrap",
        )
        .forEach((el) => el.classList.remove("filter-disabled"));
      updateHash("tracks", state, {
        sort: "",
        order: "asc",
        search: "",
        selectedServices: [],
        selectedTags: [],
        pmvCategories: [],
        pmvAggregate: "",
        fileTypes: [],
        fileTypeAgg: "",
        importedMode: "",
        importedNum: null,
        importedUnit: "days",
        addedMode: "",
        addedNum: null,
        addedUnit: "days",
        page: 0,
      });
      fetchAndRender(container, signal, state);
    },
    { signal },
  );
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

  // ── Checkbox selection ──
  // Select-all checkbox
  const selectAllCb = container.querySelector("#tracks-select-all");
  if (selectAllCb) {
    selectAllCb.onclick = () => {
      const checked = selectAllCb.checked;
      // Get all track IDs currently visible on the page
      const rowCbs = container.querySelectorAll(".tracks-row-checkbox");
      rowCbs.forEach((cb) => {
        const trackId = parseInt(cb.dataset.trackId, 10);
        if (checked) {
          state.selectedTrackIds.add(trackId);
        } else {
          state.selectedTrackIds.delete(trackId);
        }
        cb.checked = checked;
      });
      updateSelectionUI(container, state);
    };
  }

  // Individual row checkboxes
  const rowCbs = container.querySelectorAll(".tracks-row-checkbox");
  rowCbs.forEach((cb) => {
    cb.onclick = () => {
      const trackId = parseInt(cb.dataset.trackId, 10);
      if (cb.checked) {
        state.selectedTrackIds.add(trackId);
      } else {
        state.selectedTrackIds.delete(trackId);
      }
      // Update select-all checkbox
      const allCb = container.querySelector("#tracks-select-all");
      if (allCb) {
        const allRowCbs = container.querySelectorAll(".tracks-row-checkbox");
        allCb.checked =
          allRowCbs.length > 0 && Array.from(allRowCbs).every((rc) => rc.checked);
      }
      updateSelectionUI(container, state);
    };
  });
}

/* ------------------------------------------------------------------ */
/*  Selection + Bulk Actions                                           */
/* ------------------------------------------------------------------ */

/**
 * Update the selection count badge and needs-comment count in the actions panel.
 */
function updateSelectionUI(container, state) {
  const count = state.selectedTrackIds.size;
  updateSelectionCount(container, "tracks", count);

  // Compute needs-comment count from backend
  computeNeedsCount(container, state);
}

/**
 * Query /api/tracks/needs-comment-count for the currently selected tracks.
 * Updates the WRITE COMMENTS button label with the count.
 */
async function computeNeedsCount(container, state) {
  const btn = container.querySelector("#tracks-actions-write-comments");
  if (!btn) return;

  const selectedIds = Array.from(state.selectedTrackIds);
  if (selectedIds.length === 0) {
    btn.innerHTML = '<i class="fas fa-pen"></i> WRITE COMMENTS';
    state.needsCommentCount = 0;
    return;
  }

  // Show loading indicator
  btn.innerHTML = '<i class="fas fa-pen"></i> WRITE COMMENTS (...)';
  btn.disabled = true;

  try {
    const resp = await fetchJSON("/api/tracks/needs-comment-count", {
      method: "POST",
      body: JSON.stringify({ trackIds: selectedIds }),
    });
    const data = resp.data;
    state.needsCommentCount = data.tracksNeedingUpdate || 0;
    btn.innerHTML = `<i class="fas fa-pen"></i> WRITE COMMENTS (${state.needsCommentCount})`;
  } catch (err) {
    console.warn("Failed to compute needs-comment count:", err);
    btn.innerHTML = '<i class="fas fa-pen"></i> WRITE COMMENTS';
  } finally {
    btn.disabled = state.selectedTrackIds.size === 0;
  }
}

/**
 * Write comments for files linked to the currently selected tracks.
 */
async function writeCommentsForSelected(container, state) {
  const selectedIds = Array.from(state.selectedTrackIds);
  if (selectedIds.length === 0) {
    showToast("No tracks selected.", "warning");
    return;
  }

  const btn = container.querySelector("#tracks-actions-write-comments");
  if (btn) {
    btn.disabled = true;
    btn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Writing...';
  }

  try {
    const resp = await fetchJSON("/api/tracks/write-comments", {
      method: "POST",
      body: JSON.stringify({ trackIds: selectedIds }),
    });
    const data = resp.data;
    if (data.fileCount > 0) {
      showToast(
        `Comment write queued (task #${data.taskId}, ${data.fileCount} file(s))`,
        "success",
      );
    } else {
      showToast("All comments are up to date", "info");
    }
    // Reset selection after successful write
    state.selectedTrackIds.clear();
    state.needsCommentCount = 0;
    updateSelectionUI(container, state);
    // Re-render to clear checkboxes
    fetchAndRender(container, null, state);
  } catch (err) {
    showToast(`Failed to queue comment write: ${err.message}`, "error");
  } finally {
    if (btn) {
      btn.disabled = false;
      btn.innerHTML = '<i class="fas fa-pen"></i> WRITE COMMENTS';
    }
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
    selectedTags: parseCSV(hashParams?.selectedTags),
    pmvCategories: parseCSV(hashParams?.pmvCategories),
    pmvAggregate: hashParams?.pmvAggregate || "",
    fileTypes: parseCSV(hashParams?.fileTypes),
    fileTypeAgg: hashParams?.fileTypeAgg || "",
    importedMode: hashParams?.importedMode || "",
    importedNum: hashParams?.importedNum ? parseInt(hashParams.importedNum) : null,
    importedUnit: hashParams?.importedUnit || "days",
    addedMode: hashParams?.addedMode || "",
    addedNum: hashParams?.addedNum ? parseInt(hashParams.addedNum) : null,
    addedUnit: hashParams?.addedUnit || "days",
    playlistId: hashParams?.playlistId ? parseInt(hashParams.playlistId) : null,
    playlistName: hashParams?.playlistName || null,
    tagEnabled: true,
    serviceEnabled: true,
    pmvEnabled: true,
    typeEnabled: true,
    dateEnabled: true,
    layoutMode: false,
    selectedTrackIds: new Set(),
    needsCommentCount: 0,
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
  const actionsHtml = renderActionsPanel("tracks", [
    {
      id: "write-comments",
      label: "WRITE COMMENTS",
      icon: "fas fa-pen",
      cls: "btn-primary btn-write-comments",
      action: "write-comments",
    },
  ]);

  container.innerHTML = `
    <div style="display:flex;flex-direction:column;gap:var(--space-4);">
      <div style="display:flex;gap:var(--space-4);align-items:flex-start;">
        <div style="flex:4;min-width:0;">${renderToolbar(state.search, state)}</div>
        ${actionsHtml}
      </div>
      <div id="tracks-content" style="min-height:200px;">${renderLoading("Loading tracks…")}</div>
    </div>`;

  // Wire search + filter once (toolbar is stable)
  const filterPanel = container.querySelector("#tracks-filter-panel");
  if (filterPanel) {
    wireSearchFilter(filterPanel, state, () => fetchAndRender(container, signal, state));
  }

  // Wire toolbar filter events (service icons)
  wireToolbarEvents(container, signal, state);

  // Wire actions panel refresh
  import("../shared/actions-panel.js").then(({ wireActionsRefresh }) => {
    wireActionsRefresh(container, "tracks", () => {
      state.page = 0;
      return fetchAndRender(container, signal, state);
    });
  });

  // Wire WRITE COMMENTS button in actions panel
  const writeBtn = container.querySelector("#tracks-actions-write-comments");
  if (writeBtn) {
    writeBtn.onclick = () => writeCommentsForSelected(container, state);
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
