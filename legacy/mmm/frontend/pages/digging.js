/**
 * digging.js — Flat Ladder Digging page.
 *
 * PANE 1 (left, 55%): TRACK BROWSER — search, filter toggles, tag chips,
 *                  track cards with waveform audio, pagination.
 * PANE 2 (right, 45%): LADDER — flat ordered list of identical track cards,
 *                  drag to reorder, session persistence, save as playlist.
 *
 * Filters derive from ALL ladder tracks, not selected steps.
 *
 * API: GET /api/digging/tracks — paginated, server-side filtering
 *      by energy, key, BPM, tags, and text search.
 */

import { fetchJSON } from "../shared/api.js";
import { escapeHtml, showToast } from "../shared/components.js";

/* ── Constants ──────────────────────────────────────────── */

const ENERGY_COLORS = [
  "var(--text-subtle)",
  "var(--green)",
  "var(--accent)",
  "var(--yellow)",
  "var(--orange)",
  "var(--red)",
];

const PHASE_ENERGY = {
  end: 0,
  start: 1,
  release: 2,
  sustain: 3,
  build: 4,
  peak: 5,
};

const KEY_RANGE_OPTIONS = [
  { value: "+1,-1,same,a_to_b", label: "\u00B11 (same, A\u2194B)" },
  { value: "+2,-2,+1,-1,same,a_to_b", label: "\u00B12" },
  { value: "same,a_to_b", label: "Same key / A\u2194B" },
  { value: "+7,-7,+1,-1,same,a_to_b", label: "\u00B17 (energy boost)" },
  { value: "same,a_to_b,+1,-1,+2,-2,+7,-7", label: "All compatible" },
];

const SESSION_KEY = "diggingSession_v2";
let _saveDebounce = null;

/* ── State ──────────────────────────────────────────────── */

const state = {
  // Ladder — flat array of track objects
  ladder: [],

  // PMV filter (manual, independent of ladder)
  pmvCategories: [], // ['p','m','v'] — active prefix buttons
  pmvAggregate: "", // 'full'|'partial'|'none'|''

  // Key filter (24 Camelot keys, independent of ladder)
  selectedKeys: [], // ['1m','3d','4m',...]

  // Phase filter (appends phase tag names to tags param)
  selectedPhases: [], // ['start','build','peak',...]

  // Filters (ladder-derived toggles)
  filterEnergyEnabled: true,
  filterKeyEnabled: false,
  filterBpmEnabled: false,
  filterLadderTagsEnabled: true,
  keyRange: "+1,-1,same,a_to_b",

  // Tag chips (user-added)
  selectedTagChips: [], // [{ id, name }]

  // BPM
  bpmRange: 5,
  bpmFrom: null,
  bpmTo: null,

  // Sort
  sortBy: "rating",
  sortOrder: "desc",

  // Search
  searchTerm: "",

  // Track browser
  tracks: [],
  total: 0,
  page: 0,
  pageSize: 20,
  loading: false,

  // Audio
  activeAudio: null,

  // Save
  showSaveDialog: false,
  playlistName: "",

  // Session persistence
  sessionName: "default",
  sessions: {},
};

// Waveform cache & audio helpers
const waveformCache = new Map();
let progressInterval = null;
let sharedAudioContext = null;

function getAudioContext() {
  if (!sharedAudioContext) {
    sharedAudioContext = new (window.AudioContext || window.webkitAudioContext)();
  }
  if (sharedAudioContext.state === "suspended") {
    sharedAudioContext.resume();
  }
  return sharedAudioContext;
}

/* ── Helpers ────────────────────────────────────────────── */

function formatTime(seconds) {
  if (!seconds || isNaN(seconds)) return "0:00";
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}

function formatDuration(ms) {
  if (!ms) return "--:--";
  const totalSec = Math.floor(ms / 1000);
  const min = Math.floor(totalSec / 60);
  const sec = totalSec % 60;
  return `${min}:${sec.toString().padStart(2, "0")}`;
}

function daysAgo(epochSec) {
  if (!epochSec) return "";
  const days = Math.floor((Date.now() / 1000 - epochSec) / 86400);
  return days <= 0 ? "today" : `${days}d ago`;
}

function pickAudioFile(files) {
  if (!files || files.length === 0) return null;
  // Prefer FLAC (any location), then stem.m4a, then any
  const flac = files.find((f) => f.fileType === "flac");
  if (flac) return flac;
  const stem = files.find((f) => f.fileType === "stem.m4a");
  if (stem) return stem;
  return files[0];
}

function energyColor(energy) {
  return ENERGY_COLORS[Math.round(energy)] || "var(--text-muted)";
}

function loadConfig() {
  try {
    const saved = JSON.parse(localStorage.getItem("digging2Config") || "{}");
    if (saved.bpmRange != null) state.bpmRange = saved.bpmRange;
    if (saved.filterEnergyEnabled != null)
      state.filterEnergyEnabled = saved.filterEnergyEnabled;
    if (saved.filterKeyEnabled != null) state.filterKeyEnabled = saved.filterKeyEnabled;
    if (saved.filterBpmEnabled != null) state.filterBpmEnabled = saved.filterBpmEnabled;
    if (saved.filterLadderTagsEnabled != null)
      state.filterLadderTagsEnabled = saved.filterLadderTagsEnabled;
    if (saved.keyRange) state.keyRange = saved.keyRange;
  } catch {
    /* ignore */
  }
}

function saveConfig() {
  try {
    localStorage.setItem(
      "digging2Config",
      JSON.stringify({
        bpmRange: state.bpmRange,
        filterEnergyEnabled: state.filterEnergyEnabled,
        filterKeyEnabled: state.filterKeyEnabled,
        filterBpmEnabled: state.filterBpmEnabled,
        filterLadderTagsEnabled: state.filterLadderTagsEnabled,
        keyRange: state.keyRange,
      }),
    );
  } catch {
    /* ignore */
  }
}

/* ── Session persistence ────────────────────────────────── */

function saveSession() {
  const session = {
    ladder: state.ladder.map((t) => ({
      id: t.id,
      title: t.title,
      artist: t.artist,
      bpm: t.bpm,
      musicalKey: t.musicalKey,
      energyLevel: t.energyLevel,
      tags: t.tags,
      files: t.files,
      playlists: t.playlists,
      playCount: t.playCount,
      rating: t.rating,
      lastPlayed: t.lastPlayed,
      durationMs: t.durationMs,
      service: t.service,
      genre: t.genre,
    })),
    filters: {
      energy: state.filterEnergyEnabled,
      key: state.filterKeyEnabled,
      bpm: state.filterBpmEnabled,
      tags: state.filterLadderTagsEnabled,
      keyRange: state.keyRange,
      chips: state.selectedTagChips,
      pmvCategories: state.pmvCategories,
      pmvAggregate: state.pmvAggregate,
      selectedKeys: state.selectedKeys,
      selectedPhases: state.selectedPhases,
    },
    bpmRange: state.bpmRange,
    sortBy: state.sortBy,
    sortOrder: state.sortOrder,
    savedAt: Date.now(),
  };
  try {
    localStorage.setItem(SESSION_KEY, JSON.stringify(session));
  } catch {
    /* ignore */
  }
}

/** Debounced auto-save. Call after every state change. */
function autoSaveSession() {
  clearTimeout(_saveDebounce);
  _saveDebounce = setTimeout(saveSession, 2000);
}

function loadSession() {
  try {
    const raw = localStorage.getItem(SESSION_KEY);
    if (!raw) return false;
    const s = JSON.parse(raw);
    state.ladder = s.ladder || [];
    if (s.filters) {
      state.filterEnergyEnabled = s.filters.energy ?? true;
      state.filterKeyEnabled = s.filters.key ?? false;
      state.filterBpmEnabled = s.filters.bpm ?? false;
      state.filterLadderTagsEnabled = s.filters.tags ?? true;
      state.keyRange = s.filters.keyRange || "+1,-1,same,a_to_b";
      state.selectedTagChips = s.filters.chips || [];
      state.pmvCategories = s.filters.pmvCategories || [];
      state.pmvAggregate = s.filters.pmvAggregate || "";
      state.selectedKeys = s.filters.selectedKeys || [];
      state.selectedPhases = s.filters.selectedPhases || [];
    }
    state.bpmRange = s.bpmRange ?? 5;
    state.sortBy = s.sortBy || "rating";
    state.sortOrder = s.sortOrder || "desc";
    return true;
  } catch {
    return false;
  }
}

/* ── Entry point ────────────────────────────────────────── */

export async function init(container, _signal, _hashParams) {
  loadConfig();
  const hadSession = loadSession();
  renderLayout(container);
  wireEvents(container);
  if (!hadSession) {
    loadTracks();
  } else {
    renderLadderPane();
    loadTracks();
  }
}

/* ── Render: layout ─────────────────────────────────────── */

function renderLayout(container) {
  container.innerHTML = `
    <div class="page-header">
      <h1><i class="fas fa-magnifying-glass"></i> DIGGING
        <span class="text-muted" style="font-size:0.7rem;margin-left:0.5rem" id="digging-total-count"></span>
      </h1>
    </div>
    <div class="digging-2pane">
      <div class="pane pane-browser" id="pane-browser">
        ${renderBrowserHeader()}
        ${renderFilterRows()}
        ${renderFilterBar()}
        <div id="browser-content"></div>
      </div>
      <div class="pane pane-ladder" id="pane-ladder">
        <div id="ladder-content"></div>
      </div>
    </div>
  `;

  renderLadderPane();
  renderBrowserContent();
}

/* ── Render: Ladder ─────────────────────────────────────── */

function renderLadderPane() {
  const container = document.getElementById("ladder-content");
  if (!container) return;

  if (!state.ladder.length) {
    container.innerHTML = `<div class="empty-state">
      <i class="fas fa-layer-group empty-icon"></i>
      <p>Drop tracks from the browser to build your ladder</p>
      <button class="btn btn-sm btn-secondary" id="ladder-load-session" style="margin-top:0.5rem">
        <i class="fas fa-folder-open"></i> Load Session
      </button>
    </div>`;
    return;
  }

  // Compute summary stats
  const bpms = state.ladder.filter((t) => t.bpm).map((t) => t.bpm);
  const bpmMin = bpms.length ? Math.min(...bpms) : null;
  const bpmMax = bpms.length ? Math.max(...bpms) : null;
  const keys = [
    ...new Set(state.ladder.filter((t) => t.musicalKey).map((t) => t.musicalKey)),
  ];

  const allTags = new Set();
  state.ladder.forEach((t) => {
    (t.tags || []).forEach((tag) => {
      const n = typeof tag === "string" ? tag : tag.name || "";
      if (n && n.length < 40) allTags.add(n);
    });
  });

  const cardsHtml = state.ladder
    .map((track, idx) => renderTrackCard(track, idx, true))
    .join("");

  const saveSection = state.showSaveDialog
    ? `<div class="ladder-save-form">
        <div class="form-group">
          <label for="ladder-playlist-name">Playlist name</label>
          <input type="text" class="input-text" id="ladder-playlist-name"
            placeholder="my-set" value="${escapeHtml(state.playlistName)}" />
        </div>
        <div style="display:flex;gap:0.5rem;margin-top:0.5rem;">
          <button class="btn btn-primary" id="ladder-save-confirm">
            <i class="fas fa-save"></i> Save
          </button>
          <button class="btn btn-secondary" id="ladder-save-cancel">
            <i class="fas fa-times"></i> Cancel
          </button>
        </div>
      </div>`
    : "";

  container.innerHTML = `
    <div class="ladder-header">
      <h3 class="ladder-header-title">LADDER <span class="ladder-header-count">(${state.ladder.length} track${state.ladder.length !== 1 ? "s" : ""})</span></h3>
      <div class="ladder-header-actions">
        <button class="btn btn-sm" id="ladder-save-session" title="Save session">
          <i class="fas fa-save"></i>
        </button>
        <button class="btn btn-sm" id="ladder-load-session" title="Load session">
          <i class="fas fa-folder-open"></i>
        </button>
      </div>
    </div>
    <div class="ladder-summary">
      ${bpmMin != null ? `<span class="ladder-summary-item">BPM: <strong>${bpmMin}\u2013${bpmMax}</strong></span>` : ""}
      ${keys.length ? `<span class="ladder-summary-item">Keys: <strong>${keys.join(", ")}</strong></span>` : ""}
      ${allTags.size ? `<span class="ladder-summary-item">Tags: <strong>${allTags.size}</strong></span>` : ""}
    </div>
    <div id="ladder-tracks">
      ${cardsHtml}
    </div>
    <div class="ladder-dropzone" id="ladder-dropzone">Drop tracks here</div>
    <div class="ladder-footer">
      <button class="btn btn-secondary btn-save-playlist" id="ladder-save-show" style="width:100%;">
        <i class="fas fa-save"></i> Save as Playlist
      </button>
      ${saveSection}
    </div>
  `;

  // Wire audio + waveform
  setupAudioPlayers();
  wireWaveformSeek();
  preloadWaveforms();
}

function preloadWaveforms() {
  const canvases = document.querySelectorAll(".waveform-canvas");
  let delay = 0;
  canvases.forEach((canvas) => {
    const fileId = +canvas.dataset.fileId;
    if (!fileId) return;
    const peaks = waveformCache.get(String(fileId));
    if (peaks) {
      drawWaveform(fileId, peaks, 0);
    } else {
      setTimeout(() => loadWaveform(fileId), delay);
      delay += 300;
    }
  });
}

/* ── Render: Browser ────────────────────────────────────── */

function renderBrowserHeader() {
  return `
    <div class="browser-search-bar">
      <div class="tag-chips-wrap" id="browser-tag-chips-wrap">
        <i class="fas fa-search"></i>
        <div class="tag-chips" id="browser-tag-chips"></div>
        <input
          type="text"
          class="input-text tag-chip-input"
          id="browser-search-input"
          placeholder="search tracks, tags..."
          autocomplete="off"
        />
      </div>
      <div class="browser-bpm-display">
        <span>BPM: <span id="bpm-range-value" class="bpm-range-num">${state.bpmRange}</span> \u00B1</span>
        <input type="range" id="bpm-range-slider" min="1" max="15" value="${state.bpmRange}" class="config-slider" />
      </div>
      <div class="bpm-abs-filter">
        <input type="number" id="bpm-filter-from" class="input-text" style="width:70px;font-size:0.75rem"
          placeholder="BPM from" value="${state.bpmFrom || ""}" />
        <span style="color:var(--text-muted)">\u2013</span>
        <input type="number" id="bpm-filter-to" class="input-text" style="width:70px;font-size:0.75rem"
          placeholder="to" value="${state.bpmTo || ""}" />
      </div>
      <div class="sort-bar">
        <span style="font-size:0.75rem;color:var(--text-muted)">Sort:</span>
        <select id="sort-select" class="input-text" style="width:110px;font-size:0.8rem">
          <option value="relevance" ${state.sortBy === "relevance" ? "selected" : ""}>Relevance</option>
          <option value="playCount" ${state.sortBy === "playCount" ? "selected" : ""}>Plays</option>
          <option value="rating" ${state.sortBy === "rating" ? "selected" : ""}>Rating</option>
          <option value="bpm" ${state.sortBy === "bpm" ? "selected" : ""}>BPM</option>
          <option value="energy" ${state.sortBy === "energy" ? "selected" : ""}>Energy</option>
          <option value="lastPlayed" ${state.sortBy === "lastPlayed" ? "selected" : ""}>Recent</option>
          <option value="tagCount" ${state.sortBy === "tagCount" ? "selected" : ""}>Tags</option>
        </select>
        <button id="sort-dir-btn" class="btn btn-sm toggle-chip" title="Toggle sort direction">
          ${state.sortOrder === "asc" ? "\u2191" : "\u2193"}
        </button>
      </div>
    </div>`;
}

function renderFilterBar() {
  const keyRangeOptions = KEY_RANGE_OPTIONS.map(
    (o) =>
      `<option value="${o.value}" ${state.keyRange === o.value ? "selected" : ""}>${escapeHtml(o.label)}</option>`,
  ).join("");

  // Count auto tags from all ladder tracks
  const autoTags = new Set();
  state.ladder.forEach((t) => {
    (t.tags || []).forEach((tag) => {
      const p = typeof tag === "string" ? "" : tag.prefix || "";
      const n = typeof tag === "string" ? tag : tag.name || "";
      if (p !== "P" && n && n.length < 40) autoTags.add(n);
    });
  });

  return `
    <div class="filter-bar" id="filter-bar">
      <label class="toggle-chip${state.filterEnergyEnabled ? " active" : ""}" data-filter="energy">
        \u26A1Energy <span id="filter-energy-info">${state.ladder.length > 0 ? [...new Set(state.ladder.filter((t) => t.energyLevel != null).map((t) => Math.round(t.energyLevel)))].join(",") : "\u2014"}</span>
      </label>
      <label class="toggle-chip${state.filterKeyEnabled ? " active" : ""}" data-filter="key">
        \uD83D\uDD11Key \u00B1
      </label>
      <label class="toggle-chip${state.filterBpmEnabled ? " active" : ""}" data-filter="bpm">
        \uD83C\uDFB5Ladder BPM
      </label>
      <label class="toggle-chip${state.filterLadderTagsEnabled ? " active" : ""}" data-filter="ladderTags">
        \uD83C\uDFF7\uFE0FLadder tags: <span id="filter-tags-count">${autoTags.size + state.selectedTagChips.length}</span>
      </label>
      <div class="filter-bar-right">
        <select id="key-range-select" class="input-text key-range-select" ${!state.filterKeyEnabled ? 'style="display:none"' : ""}>
          ${keyRangeOptions}
        </select>
        <div class="tag-chips filter-bar-chips" id="filter-bar-chips"></div>
      </div>
    </div>`;
}

function renderFilterRows() {
  const allKeys = [];
  for (let i = 1; i <= 12; i++) allKeys.push(i + "m");
  for (let i = 1; i <= 12; i++) allKeys.push(i + "d");

  const phases = ["End", "Start", "Release", "Build", "Sustain", "Peak"];

  return `
    <div class="digging-filter-rows">
      <div class="dfr-row">
        <span class="dfr-label">PMV:</span>
        <div class="dfr-group">
          <button class="dfr-btn ${state.pmvCategories.includes("p") ? "active" : ""}" data-filter="pmv-cat" data-val="p">P</button>
          <button class="dfr-btn ${state.pmvCategories.includes("m") ? "active" : ""}" data-filter="pmv-cat" data-val="m">M</button>
          <button class="dfr-btn ${state.pmvCategories.includes("v") ? "active" : ""}" data-filter="pmv-cat" data-val="v">V</button>
        </div>
        <span class="dfr-sep">|</span>
        <div class="dfr-group">
          <button class="dfr-btn ${state.pmvAggregate === "full" ? "active" : ""}" data-filter="pmv-agg" data-val="full">Full</button>
          <button class="dfr-btn ${state.pmvAggregate === "partial" ? "active" : ""}" data-filter="pmv-agg" data-val="partial">Partial</button>
          <button class="dfr-btn ${state.pmvAggregate === "none" ? "active" : ""}" data-filter="pmv-agg" data-val="none">None</button>
        </div>
      </div>
      <div class="dfr-row dfr-row-key">
        <span class="dfr-label">KEY:</span>
        <div class="dfr-group dfr-key-group">
          ${allKeys
            .filter((k) => k.endsWith("m"))
            .map(
              (k) =>
                `<button class="dfr-btn dfr-key-btn ${state.selectedKeys.includes(k) ? "active" : ""}" data-filter="key" data-val="${k}">${k}</button>`,
            )
            .join("")}
        </div>
        <span class="dfr-sep">|</span>
        <button class="dfr-btn dfr-action-btn" data-filter="key-all" data-mode="m">ALL m</button>
        <button class="dfr-btn dfr-action-btn" data-filter="key-none" data-mode="m">NONE m</button>
      </div>
      <div class="dfr-row dfr-row-key">
        <span class="dfr-label" style="visibility:hidden">KEY:</span>
        <div class="dfr-group dfr-key-group">
          ${allKeys
            .filter((k) => k.endsWith("d"))
            .map(
              (k) =>
                `<button class="dfr-btn dfr-key-btn ${state.selectedKeys.includes(k) ? "active" : ""}" data-filter="key" data-val="${k}">${k}</button>`,
            )
            .join("")}
        </div>
        <span class="dfr-sep">|</span>
        <button class="dfr-btn dfr-action-btn" data-filter="key-all" data-mode="d">ALL d</button>
        <button class="dfr-btn dfr-action-btn" data-filter="key-none" data-mode="d">NONE d</button>
      </div>
      <div class="dfr-row">
        <span class="dfr-label">Phase:</span>
        <div class="dfr-group">
          ${phases.map((p) => `<button class="dfr-btn ${state.selectedPhases.includes(p.toLowerCase()) ? "active" : ""}" data-filter="phase" data-val="${p.toLowerCase()}">${p}</button>`).join("")}
        </div>
      </div>
    </div>
  `;
}

function updateFilterRows() {
  const el = document.querySelector(".digging-filter-rows");
  if (el) el.outerHTML = renderFilterRows();
}

function renderTagChipsInBar() {
  const container = document.getElementById("filter-bar-chips");
  if (!container) return;

  if (state.selectedTagChips.length === 0) {
    container.innerHTML = "";
    return;
  }

  container.innerHTML = state.selectedTagChips
    .map(
      (c) =>
        `<span class="tag-chip tag-chip-sm">${escapeHtml(c.name)}<button class="tag-chip-x" data-tag-name="${escapeHtml(c.name)}">&times;</button></span>`,
    )
    .join("");
}

function renderBrowserContent() {
  const container = document.getElementById("browser-content");
  if (!container) return;

  if (state.loading) {
    container.innerHTML = `<div class="digging-loading"><div class="spinner"></div><p>Loading tracks...</p></div>`;
    return;
  }

  if (state.tracks.length === 0) {
    const hasLadder = state.ladder.length > 0;
    container.innerHTML = `<div class="empty-state">
      <i class="fas fa-music empty-icon"></i>
      <p>${hasLadder ? "No tracks match these filters" : "Drop tracks into the ladder to browse suggestions"}</p>
    </div>`;
    return;
  }

  const cardsHtml = state.tracks
    .map((track, idx) => renderTrackCard(track, idx, false))
    .join("");
  const paginationHtml = renderPagination();

  container.innerHTML = `${cardsHtml}${paginationHtml}`;

  setupAudioPlayers();
  wireWaveformSeek();
  preloadWaveforms();
}

/* ── Unified track card (used in both browser and ladder) ─ */

function renderTrackCard(track, index, inLadder) {
  const audioFile = pickAudioFile(track.files);
  const fileId = audioFile ? audioFile.id : null;
  const hasPlayback = audioFile != null;
  const dateAgo = track.lastPlayed ? daysAgo(track.lastPlayed) : "";
  const isInLadder = inLadder || state.ladder.some((t) => t.id === track.id);

  // Split tags by category
  const allTags = (track.tags || []).map((t) =>
    typeof t === "string"
      ? { name: t, categoryName: "", prefix: "", energyLevel: null }
      : { ...t, energyLevel: null },
  );
  const phaseTags = allTags.filter((t) => t.prefix === "P");
  const moodTags = allTags.filter((t) => t.prefix === "M");
  const vibeTags = allTags.filter((t) => t.prefix === "V");
  const otherTags = allTags.filter(
    (t) => t.prefix !== "P" && t.prefix !== "M" && t.prefix !== "V",
  );

  // Tag row helper
  const tagRow = (label, tags, showEnergy) => {
    if (!tags.length) return "";
    const chips = tags
      .map((t) => {
        const en =
          showEnergy && PHASE_ENERGY[t.name.toLowerCase()] != null
            ? ` \u26A1${PHASE_ENERGY[t.name.toLowerCase()]}`
            : "";
        return `<span class="tag-chip-sm" title="${escapeHtml(t.categoryName)}">${escapeHtml(t.name)}${en}</span>`;
      })
      .join("");
    return `<div class="tc-tag-row"><span class="tc-tag-label">${label}</span>${chips}</div>`;
  };

  return `
    <div class="track-card${isInLadder ? " in-ladder" : ""}" draggable="true" data-track-id="${track.id}"
         ${inLadder ? `data-ladder-idx="${index}"` : ""}>
      <div class="tc-drag-handle" title="${inLadder ? "Reorder" : "Drag to ladder"}" draggable="true">\u281F</div>
      <div class="tc-main">
        <div class="tc-info">
          <div class="tc-title">
            ${inLadder ? `<span class="tc-rank">#${index + 1}</span>` : ""}
            ${escapeHtml(track.title)}
          </div>
          <div class="tc-artist">${escapeHtml(track.artist)}</div>
          <div class="tc-meta">
            ${track.bpm ? `<span class="badge badge-bpm">${track.bpm}</span>` : ""}
            ${track.musicalKey ? `<span class="badge badge-key">${escapeHtml(track.musicalKey)}</span>` : ""}
            ${track.playCount > 0 ? `<span class="badge badge-plays">\u25B6${track.playCount}</span>` : '<span class="badge badge-fresh">new</span>'}
            ${track.rating > 0 ? `<span class="badge badge-rating">${"\u2605".repeat(Math.min(5, Math.round(track.rating / 20)))}</span>` : ""}
            ${dateAgo ? `<span class="badge badge-last-played">${dateAgo}</span>` : ""}
          </div>
          ${tagRow("PHASE", phaseTags, true)}
          ${tagRow("MOOD", moodTags, false)}
          ${tagRow("VIBE", vibeTags, false)}
          ${tagRow("TAGS", otherTags, false)}
        </div>
        <div class="tc-audio">
          ${
            hasPlayback
              ? `<button class="btn-play btn-play-sm" data-file-id="${fileId}" data-track-id="${track.id}"><i class="fas fa-play"></i></button>
          <div class="waveform-wrap" data-file-id="${fileId}">
            <canvas class="waveform-canvas" data-file-id="${fileId}" width="120" height="30"></canvas>
            <div class="waveform-progress" data-file-id="${fileId}"></div>
          </div>
          <audio class="audio-el" data-file-id="${fileId}" preload="none">
            <source src="/api/files/${fileId}/stream" />
          </audio>
          <span class="time-display" data-file-id="${fileId}" style="min-width:55px;font-size:0.65rem;">${formatDuration(track.durationMs)}</span>`
              : `<span class="no-playback">No playback</span>`
          }
        </div>
      </div>
      <div class="tc-formats">
        ${(track.files || [])
          .map(
            (f) =>
              `<span class="format-badge ${f.location}">${(f.fileType || "").replace("stem.", "")} ${f.location === "local" ? "\uD83D\uDCBB" : "\uD83D\uDCBE"}</span>`,
          )
          .join("")}
        ${track.service ? `<span class="badge badge-service">${escapeHtml(track.service)}</span>` : ""}
      </div>
      ${inLadder ? `<button class="btn-remove-card" data-action="remove-ladder" data-idx="${index}"><i class="fas fa-times"></i></button>` : ""}
    </div>`;
}

function renderPagination() {
  const totalPages = Math.ceil(state.total / state.pageSize) || 1;
  const current = state.page + 1;

  if (state.total <= state.pageSize) return "";

  return `
    <div class="browser-pagination">
      <span class="pagination-info">Page ${current} of ${totalPages} (${state.total} total)</span>
      <button class="btn btn-sm" id="browser-prev-page" ${state.page <= 0 ? "disabled" : ""}>
        <i class="fas fa-chevron-left"></i> Prev
      </button>
      <button class="btn btn-sm" id="browser-next-page" ${current >= totalPages ? "disabled" : ""}>
        Next <i class="fas fa-chevron-right"></i>
      </button>
    </div>`;
}

/* ── Audio player ───────────────────────────────────────── */

function setupAudioPlayers() {
  document.querySelectorAll(".tc-audio .btn-play, .btn-play-sm").forEach((btn) => {
    const newBtn = btn.cloneNode(true);
    btn.parentNode.replaceChild(newBtn, btn);

    newBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      const fileId = +newBtn.dataset.fileId;
      const audio = document.querySelector(`audio[data-file-id="${fileId}"]`);
      if (!audio) return;

      if (state.activeAudio && state.activeAudio !== audio) {
        state.activeAudio.pause();
        state.activeAudio.currentTime = 0;
        document
          .querySelectorAll(
            `.btn-play[data-file-id="${state.activeAudio.dataset.fileId}"]`,
          )
          .forEach((pb) => {
            pb.innerHTML = '<i class="fas fa-play"></i>';
          });
      }

      if (audio.paused) {
        audio.play().catch(() => {});
        document.querySelectorAll(`.btn-play[data-file-id="${fileId}"]`).forEach((pb) => {
          pb.innerHTML = '<i class="fas fa-pause"></i>';
        });
        state.activeAudio = audio;

        if (waveformCache.has(String(fileId))) {
          drawWaveform(fileId, waveformCache.get(String(fileId)), 0);
        } else {
          loadWaveform(fileId);
        }

        if (progressInterval) clearInterval(progressInterval);
        progressInterval = setInterval(() => updateProgress(fileId), 100);
      } else {
        audio.pause();
        document.querySelectorAll(`.btn-play[data-file-id="${fileId}"]`).forEach((pb) => {
          pb.innerHTML = '<i class="fas fa-play"></i>';
        });
        state.activeAudio = null;
        if (progressInterval) clearInterval(progressInterval);
      }

      audio.onended = () => {
        document.querySelectorAll(`.btn-play[data-file-id="${fileId}"]`).forEach((pb) => {
          pb.innerHTML = '<i class="fas fa-play"></i>';
        });
        state.activeAudio = null;
        if (progressInterval) clearInterval(progressInterval);
        const peaks = waveformCache.get(String(fileId));
        if (peaks) drawWaveform(fileId, peaks, 0);
        document
          .querySelectorAll(`.time-display[data-file-id="${fileId}"]`)
          .forEach((td) => {
            if (audio.duration) td.textContent = formatTime(audio.duration);
          });
      };
    });
  });
}

async function loadWaveform(fileId) {
  if (waveformCache.has(String(fileId))) {
    drawWaveform(fileId, waveformCache.get(String(fileId)), 0);
    return;
  }
  try {
    const canvas = document.querySelector(`.waveform-canvas[data-file-id="${fileId}"]`);
    if (!canvas) return;

    const res = await fetch(`/api/files/${fileId}/stream`);
    if (!res.ok) throw new Error("stream failed");
    const arrayBuffer = await res.arrayBuffer();

    const ctx = getAudioContext();
    const audioBuffer = await ctx.decodeAudioData(arrayBuffer);

    const channelData = audioBuffer.getChannelData(0);
    const samples = Math.min(200, canvas.width);
    const peaks = new Float32Array(samples);
    const blockSize = Math.floor(channelData.length / samples);

    for (let i = 0; i < samples; i++) {
      let peak = 0;
      const start = i * blockSize;
      const end = Math.min(start + blockSize, channelData.length);
      for (let j = start; j < end; j++) {
        const val = Math.abs(channelData[j]);
        if (val > peak) peak = val;
      }
      peaks[i] = peak;
    }

    let maxPeak = 0;
    for (let i = 0; i < samples; i++) if (peaks[i] > maxPeak) maxPeak = peaks[i];
    if (maxPeak > 0) for (let i = 0; i < samples; i++) peaks[i] /= maxPeak;

    waveformCache.set(String(fileId), peaks);
    drawWaveform(fileId, peaks, 0);
  } catch (err) {
    console.warn("Waveform failed for", fileId, err);
    drawFallbackWaveform(fileId);
  }
}

function drawFallbackWaveform(fileId) {
  const canvas = document.querySelector(`.waveform-canvas[data-file-id="${fileId}"]`);
  if (!canvas) return;
  const ctx = canvas.getContext("2d");
  const w = canvas.width;
  const h = canvas.height;
  ctx.clearRect(0, 0, w, h);
  const style = getComputedStyle(document.body);
  const mutedColor = style.getPropertyValue("--text-subtle").trim() || "#666";
  const barCount = 15;
  const barWidth = Math.max(2, (w - (barCount - 1) * 2) / barCount);
  for (let i = 0; i < barCount; i++) {
    const x = i * (barWidth + 2);
    const barHeight = h * 0.3;
    const y = (h - barHeight) / 2;
    ctx.fillStyle = mutedColor;
    ctx.fillRect(x, y, barWidth, barHeight);
  }
}

function drawWaveform(fileId, peaks, progress) {
  document
    .querySelectorAll(`.waveform-canvas[data-file-id="${fileId}"]`)
    .forEach((canvas) => {
      const ctx = canvas.getContext("2d");
      const w = canvas.width;
      const h = canvas.height;
      ctx.clearRect(0, 0, w, h);

      const barCount = Math.min(peaks.length, 200);
      const gap = 1;
      const drawWidth = Math.max(1, (w - (barCount - 1) * gap) / barCount);
      const progressIndex = Math.floor((progress || 0) * barCount);

      const style = getComputedStyle(document.body);
      const primaryColor = style.getPropertyValue("--accent").trim() || "#6366f1";
      const mutedColor = style.getPropertyValue("--text-subtle").trim() || "#666";

      for (let i = 0; i < barCount; i++) {
        const x = i * (drawWidth + gap);
        const barHeight = Math.max(1, peaks[i] * h * 0.9);
        const y = (h - barHeight) / 2;
        ctx.fillStyle = i <= progressIndex ? primaryColor : mutedColor;
        ctx.fillRect(x, y, drawWidth, barHeight);
      }
    });
}

function updateProgress(fileId) {
  const audio = document.querySelector(`.audio-el[data-file-id="${fileId}"]`);
  if (!audio || !audio.duration) return;
  const progress = audio.currentTime / audio.duration;

  document
    .querySelectorAll(`.waveform-progress[data-file-id="${fileId}"]`)
    .forEach((overlay) => {
      overlay.style.width = `${progress * 100}%`;
    });

  document.querySelectorAll(`.time-display[data-file-id="${fileId}"]`).forEach((td) => {
    td.textContent = formatTime(audio.currentTime);
  });

  const peaks = waveformCache.get(String(fileId));
  if (peaks) drawWaveform(fileId, peaks, progress);
}

function wireWaveformSeek() {
  document.querySelectorAll(".waveform-wrap").forEach((wrap) => {
    const newWrap = wrap.cloneNode(true);
    wrap.parentNode.replaceChild(newWrap, wrap);
    newWrap.addEventListener("click", (e) => {
      const fileId = +newWrap.dataset.fileId;
      const audio = document.querySelector(`audio[data-file-id="${fileId}"]`);
      if (!audio || !audio.duration) return;
      const rect = newWrap.getBoundingClientRect();
      const x = e.clientX - rect.left;
      const ratio = Math.max(0, Math.min(1, x / rect.width));
      audio.currentTime = ratio * audio.duration;
      if (state.activeAudio === audio) {
        updateProgress(fileId);
      }
    });
  });
}

/* ── Data loading ───────────────────────────────────────── */

async function loadTracks() {
  const params = new URLSearchParams();
  params.set("pageSize", String(state.pageSize));
  params.set("page", String(state.page));

  // Text search
  if (state.searchTerm.trim()) {
    params.set("q", state.searchTerm.trim());
  }

  // ── Filters derived from ALL ladder tracks ──

  // Energy: each track's energy ±1, unioned from all ladder tracks
  if (state.ladder.length > 0 && state.filterEnergyEnabled) {
    const energySet = new Set();
    state.ladder
      .filter((t) => t.energyLevel != null)
      .forEach((t) => {
        const e = Math.round(t.energyLevel);
        for (let d = -1; d <= 1; d++) {
          const v = e + d;
          if (v >= 1 && v <= 5) energySet.add(v);
        }
      });
    const energies = [...energySet];
    if (energies.length) params.set("energyLevels", energies.join(","));
  }

  // Keys: all unique keys from ladder tracks
  if (state.ladder.length > 0 && state.filterKeyEnabled) {
    const keys = [
      ...new Set(state.ladder.filter((t) => t.musicalKey).map((t) => t.musicalKey)),
    ];
    if (keys.length) {
      params.set("keyList", keys.join(","));
      params.set("keyRange", state.keyRange);
    }
  }

  // BPM: median of all ladder BPMs ± slider
  if (state.ladder.length > 0 && state.filterBpmEnabled) {
    const bpms = state.ladder.filter((t) => t.bpm).map((t) => t.bpm);
    if (bpms.length > 0) {
      const sorted = [...bpms].sort((a, b) => a - b);
      const median = sorted[Math.floor(sorted.length / 2)];
      params.set("bpmMin", String(median - state.bpmRange));
      params.set("bpmMax", String(median + state.bpmRange));
    }
  }

  // Tags: auto from all ladder tracks (non-Phase) + user chips
  if (state.ladder.length > 0 && state.filterLadderTagsEnabled) {
    const autoTags = new Set();
    state.ladder.forEach((t) => {
      (t.tags || []).forEach((tag) => {
        const p = typeof tag === "string" ? "" : tag.prefix || "";
        const n = typeof tag === "string" ? tag : tag.name || "";
        if (p !== "P" && n && n.length < 40) autoTags.add(n);
      });
    });
    const allTags = [...autoTags, ...state.selectedTagChips.map((c) => c.name)];
    if (allTags.length > 0) params.set("tags", allTags.join(","));
  } else if (state.selectedTagChips.length > 0) {
    params.set("tags", state.selectedTagChips.map((c) => c.name).join(","));
  }

  // Sort
  if (state.sortBy !== "relevance" || state.ladder.length === 0) {
    params.set("sortBy", state.sortBy);
    params.set("sortOrder", state.sortOrder);
  }

  // Absolute BPM filter (overrides ladder BPM when set)
  if (state.bpmFrom != null) {
    params.set("bpmMin", state.bpmFrom);
  }
  if (state.bpmTo != null) {
    params.set("bpmMax", state.bpmTo);
  }

  // ── Persistent filter rows (independent of ladder) ──

  // PMV filter
  if (state.pmvCategories.length > 0) {
    params.set("pmvCategories", state.pmvCategories.join(","));
  }
  if (state.pmvAggregate) {
    params.set("pmvAggregate", state.pmvAggregate);
  }

  // Key filter (manual, independent of ladder keys)
  if (state.selectedKeys.length > 0) {
    params.set("keyList", state.selectedKeys.join(","));
  }

  // Phase filter (appends to tags param — ANDs with ladder tags / chips)
  if (state.selectedPhases.length > 0) {
    const currentTags = params.get("tags") || "";
    const phaseTags = state.selectedPhases.join(",");
    params.set("tags", currentTags ? currentTags + "," + phaseTags : phaseTags);
  }

  state.loading = true;
  renderBrowserContent();

  try {
    const resp = await fetchJSON(`/api/digging/tracks?${params.toString()}`);
    const data = resp.data || resp;
    state.tracks = data.tracks || [];
    state.total = data.total || 0;
    state.page = data.page || 0;
  } catch (err) {
    showToast("Error loading tracks: " + err.message, "error");
    state.tracks = [];
    state.total = 0;
  } finally {
    state.loading = false;
    renderBrowserContent();
    renderTagChipsInBar();
    renderLadderPane();
    // Update energy info in filter bar
    updateFilterInfo();
    const totalEl = document.getElementById("digging-total-count");
    if (totalEl) totalEl.textContent = state.total ? `${state.total} tracks` : "";
  }
}

function updateFilterInfo() {
  const energyInfo = document.getElementById("filter-energy-info");
  if (energyInfo) {
    if (state.ladder.length > 0) {
      const energies = [
        ...new Set(
          state.ladder
            .filter((t) => t.energyLevel != null)
            .map((t) => Math.round(t.energyLevel)),
        ),
      ];
      energyInfo.textContent = energies.length ? energies.join(",") : "\u2014";
    } else {
      energyInfo.textContent = "\u2014";
    }
  }

  // Update tag count
  const autoTags = new Set();
  state.ladder.forEach((t) => {
    (t.tags || []).forEach((tag) => {
      const p = typeof tag === "string" ? "" : tag.prefix || "";
      const n = typeof tag === "string" ? tag : tag.name || "";
      if (p !== "P" && n && n.length < 40) autoTags.add(n);
    });
  });
  const tagsCount = document.getElementById("filter-tags-count");
  if (tagsCount) tagsCount.textContent = autoTags.size + state.selectedTagChips.length;
}

/* ── Save as playlist ───────────────────────────────────── */

async function saveAsPlaylist() {
  const name = state.playlistName.trim();
  if (!name) {
    showToast("Please enter a playlist name", "error");
    document.getElementById("ladder-playlist-name")?.focus();
    return;
  }

  const trackIds = [...new Set(state.ladder.map((t) => t.id))];

  if (!trackIds.length) {
    showToast("No tracks to save", "error");
    return;
  }

  try {
    await fetchJSON("/api/playlists/local", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name, trackIds }),
    });
    showToast(`Playlist "${name}" created with ${trackIds.length} tracks`, "success");
    state.showSaveDialog = false;
    state.playlistName = "";
    renderLadderPane();
  } catch (err) {
    showToast("Failed to save: " + err.message, "error");
  }
}

/* ── Ladder helpers ─────────────────────────────────────── */

function addToLadder(track, index) {
  // Check not already in ladder
  if (state.ladder.some((t) => t.id === track.id)) {
    showToast("Track already in ladder", "info");
    return;
  }
  if (index != null && index >= 0 && index <= state.ladder.length) {
    state.ladder.splice(index, 0, { ...track });
  } else {
    state.ladder.push({ ...track });
  }
  renderLadderPane();
  renderBrowserContent();
  autoSaveSession();
  state.page = 0;
  loadTracks();
}

function removeFromLadder(index) {
  if (index < 0 || index >= state.ladder.length) return;
  state.ladder.splice(index, 1);
  renderLadderPane();
  renderBrowserContent();
  autoSaveSession();
  state.page = 0;
  loadTracks();
}

function moveWithinLadder(fromIdx, toIdx) {
  if (fromIdx < 0 || fromIdx >= state.ladder.length) return;
  if (toIdx < 0 || toIdx > state.ladder.length) return;
  const [track] = state.ladder.splice(fromIdx, 1);
  if (toIdx > fromIdx) toIdx--; // adjust after removal
  state.ladder.splice(toIdx, 0, track);
  renderLadderPane();
  autoSaveSession();
}

/* ── Event wiring ───────────────────────────────────────── */

function wireEvents(container) {
  // ─── Search input + tag chip autocomplete ───
  const searchInput = document.getElementById("browser-search-input");
  let chipDebounce;
  let debounceTimer;
  let chipDropdown = null;

  function ensureDropdown() {
    if (!chipDropdown) {
      chipDropdown = document.createElement("div");
      chipDropdown.id = "browser-chip-dropdown";
      chipDropdown.className = "tag-dropdown";
      chipDropdown.style.display = "none";
      document.getElementById("browser-tag-chips-wrap")?.appendChild(chipDropdown);
    }
    return chipDropdown;
  }

  if (searchInput) {
    searchInput.addEventListener("input", () => {
      clearTimeout(chipDebounce);
      clearTimeout(debounceTimer);

      const val = searchInput.value;
      state.searchTerm = val;

      if (val.length >= 1) {
        chipDebounce = setTimeout(async () => {
          try {
            const resp = await fetchJSON(
              `/api/tags?search=${encodeURIComponent(val)}&page_size=8`,
            );
            const tags = (resp.data || resp || []).filter(
              (t) =>
                !state.selectedTagChips.some(
                  (c) => c.name.toLowerCase() === t.name.toLowerCase(),
                ),
            );
            const dd = ensureDropdown();
            if (tags.length > 0) {
              dd.innerHTML = tags
                .map(
                  (t) =>
                    `<div class="tag-dropdown-item" data-tag-id="${t.id}" data-tag-name="${escapeHtml(t.name)}">
                      <i class="fas fa-tag"></i> ${escapeHtml(t.name)}
                      <span class="text-muted" style="font-size:0.7rem">(${escapeHtml(t.categoryName || t.category_name || "")})</span>
                    </div>`,
                )
                .join("");
              dd.style.display = "block";
            } else {
              dd.style.display = "none";
            }
          } catch {
            const dd = ensureDropdown();
            dd.style.display = "none";
          }
        }, 200);
      } else {
        const dd = ensureDropdown();
        dd.style.display = "none";
      }

      debounceTimer = setTimeout(() => {
        state.page = 0;
        loadTracks();
      }, 300);
    });

    searchInput.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        clearTimeout(chipDebounce);
        clearTimeout(debounceTimer);
        state.searchTerm = searchInput.value;

        const dd = ensureDropdown();
        if (dd.style.display === "block") {
          const first = dd.querySelector(".tag-dropdown-item");
          if (first) {
            const tagName = first.dataset.tagName;
            const tagId = +first.dataset.tagId;
            if (
              !state.selectedTagChips.some(
                (c) => c.name.toLowerCase() === tagName.toLowerCase(),
              )
            ) {
              state.selectedTagChips.push({ id: tagId, name: tagName });
            }
            searchInput.value = "";
            dd.style.display = "none";
            renderTagChipsInBar();
            state.page = 0;
            loadTracks();
            return;
          }
        }

        state.page = 0;
        loadTracks();
      }

      if (e.key === "Escape") {
        const dd = ensureDropdown();
        dd.style.display = "none";
      }

      if (e.key === "ArrowDown") {
        const dd = ensureDropdown();
        if (dd.style.display === "block") {
          e.preventDefault();
          const items = dd.querySelectorAll(".tag-dropdown-item");
          const sel = dd.querySelector(".tag-dropdown-item.selected");
          let nextIdx = 0;
          if (sel) {
            sel.classList.remove("selected");
            nextIdx = Array.from(items).indexOf(sel) + 1;
          }
          if (nextIdx < items.length) {
            items[nextIdx].classList.add("selected");
          }
        }
      }

      if (e.key === "ArrowUp") {
        const dd = ensureDropdown();
        if (dd.style.display === "block") {
          e.preventDefault();
          const items = dd.querySelectorAll(".tag-dropdown-item");
          const sel = dd.querySelector(".tag-dropdown-item.selected");
          let prevIdx = items.length - 1;
          if (sel) {
            sel.classList.remove("selected");
            prevIdx = Array.from(items).indexOf(sel) - 1;
          }
          if (prevIdx >= 0) {
            items[prevIdx].classList.add("selected");
          }
        }
      }
    });
  }

  // ─── Tag chip dropdown: select ───
  container.addEventListener("click", (e) => {
    const item = e.target.closest(".tag-dropdown-item");
    if (!item) return;
    const tagName = item.dataset.tagName;
    const tagId = +item.dataset.tagId;

    if (
      !state.selectedTagChips.some((c) => c.name.toLowerCase() === tagName.toLowerCase())
    ) {
      state.selectedTagChips.push({ id: tagId, name: tagName });
    }

    if (searchInput) searchInput.value = "";
    const dd = document.getElementById("browser-chip-dropdown");
    if (dd) dd.style.display = "none";
    renderTagChipsInBar();
    state.page = 0;
    loadTracks();
  });

  // ─── Close dropdown on outside click ───
  document.addEventListener("click", (e) => {
    if (
      !e.target.closest("#browser-tag-chips-wrap") &&
      !e.target.closest("#browser-chip-dropdown")
    ) {
      const dd = document.getElementById("browser-chip-dropdown");
      if (dd) dd.style.display = "none";
    }
  });

  // ─── Filter row: PMV category buttons ───
  container.addEventListener("click", (e) => {
    const btn = e.target.closest('[data-filter="pmv-cat"]');
    if (!btn) return;
    const val = btn.dataset.val;
    const idx = state.pmvCategories.indexOf(val);
    if (idx >= 0) state.pmvCategories.splice(idx, 1);
    else state.pmvCategories.push(val);
    // Clear aggregate when picking categories
    state.pmvAggregate = "";
    updateFilterRows();
    state.page = 0;
    loadTracks();
    autoSaveSession();
  });

  // ─── Filter row: PMV aggregate buttons ───
  container.addEventListener("click", (e) => {
    const btn = e.target.closest('[data-filter="pmv-agg"]');
    if (!btn) return;
    state.pmvAggregate = btn.dataset.val;
    // Clear categories when picking aggregate
    state.pmvCategories = [];
    updateFilterRows();
    state.page = 0;
    loadTracks();
    autoSaveSession();
  });

  // ─── Filter row: Key buttons ───
  container.addEventListener("click", (e) => {
    const btn = e.target.closest('[data-filter="key"]');
    if (!btn) return;
    const val = btn.dataset.val;
    const idx = state.selectedKeys.indexOf(val);
    if (idx >= 0) state.selectedKeys.splice(idx, 1);
    else state.selectedKeys.push(val);
    updateFilterRows();
    state.page = 0;
    loadTracks();
    autoSaveSession();
  });

  // ─── Filter row: Key ALL / NONE buttons ───
  container.addEventListener("click", (e) => {
    const btnAll = e.target.closest('[data-filter="key-all"]');
    if (btnAll) {
      const mode = btnAll.dataset.mode;
      const modeKeys = [];
      for (let i = 1; i <= 12; i++) modeKeys.push(i + mode);
      state.selectedKeys = [...new Set([...state.selectedKeys, ...modeKeys])];
      updateFilterRows();
      state.page = 0;
      loadTracks();
      autoSaveSession();
      return;
    }
    const btnNone = e.target.closest('[data-filter="key-none"]');
    if (btnNone) {
      const mode = btnNone.dataset.mode;
      state.selectedKeys = state.selectedKeys.filter((k) => !k.endsWith(mode));
      updateFilterRows();
      state.page = 0;
      loadTracks();
      autoSaveSession();
    }
  });

  // ─── Filter row: Phase buttons ───
  container.addEventListener("click", (e) => {
    const btn = e.target.closest('[data-filter="phase"]');
    if (!btn) return;
    const val = btn.dataset.val;
    const idx = state.selectedPhases.indexOf(val);
    if (idx >= 0) state.selectedPhases.splice(idx, 1);
    else state.selectedPhases.push(val);
    updateFilterRows();
    state.page = 0;
    loadTracks();
    autoSaveSession();
  });

  // ─── Filter toggle chips ───
  container.addEventListener("click", (e) => {
    const chip = e.target.closest(".toggle-chip[data-filter]");
    if (!chip) return;

    const filter = chip.dataset.filter;
    switch (filter) {
      case "energy":
        state.filterEnergyEnabled = !state.filterEnergyEnabled;
        chip.classList.toggle("active");
        saveConfig();
        autoSaveSession();
        state.page = 0;
        loadTracks();
        break;
      case "key":
        state.filterKeyEnabled = !state.filterKeyEnabled;
        chip.classList.toggle("active");
        const keySelect = document.getElementById("key-range-select");
        if (keySelect) keySelect.style.display = state.filterKeyEnabled ? "" : "none";
        saveConfig();
        autoSaveSession();
        state.page = 0;
        loadTracks();
        break;
      case "bpm":
        state.filterBpmEnabled = !state.filterBpmEnabled;
        chip.classList.toggle("active");
        saveConfig();
        autoSaveSession();
        state.page = 0;
        loadTracks();
        break;
      case "ladderTags":
        state.filterLadderTagsEnabled = !state.filterLadderTagsEnabled;
        chip.classList.toggle("active");
        saveConfig();
        autoSaveSession();
        state.page = 0;
        loadTracks();
        break;
    }
  });

  // ─── Key range select ───
  container.addEventListener("change", (e) => {
    if (e.target.id === "key-range-select") {
      state.keyRange = e.target.value;
      saveConfig();
      autoSaveSession();
      state.page = 0;
      loadTracks();
    }
  });

  // ─── BPM slider ───
  const slider = document.getElementById("bpm-range-slider");
  const bpmValue = document.getElementById("bpm-range-value");
  if (slider && bpmValue) {
    slider.addEventListener("input", () => {
      state.bpmRange = +slider.value;
      bpmValue.textContent = state.bpmRange;
      saveConfig();
      autoSaveSession();
      if (state.filterBpmEnabled && state.ladder.length > 0) {
        state.page = 0;
        loadTracks();
      }
    });
  }

  // ─── Sort select ───
  const sortSelect = document.getElementById("sort-select");
  if (sortSelect) {
    sortSelect.addEventListener("change", () => {
      state.sortBy = sortSelect.value;
      state.page = 0;
      loadTracks();
    });
  }

  // ─── Sort direction toggle ───
  const sortDirBtn = document.getElementById("sort-dir-btn");
  if (sortDirBtn) {
    sortDirBtn.addEventListener("click", () => {
      state.sortOrder = state.sortOrder === "asc" ? "desc" : "asc";
      sortDirBtn.textContent = state.sortOrder === "asc" ? "\u2191" : "\u2193";
      state.page = 0;
      loadTracks();
    });
  }

  // ─── Absolute BPM from/to inputs ───
  let bpmFromTimer;
  const bpmFromInput = document.getElementById("bpm-filter-from");
  if (bpmFromInput) {
    bpmFromInput.addEventListener("input", (e) => {
      clearTimeout(bpmFromTimer);
      bpmFromTimer = setTimeout(() => {
        const val = e.target.value.trim();
        state.bpmFrom = val ? +val : null;
        state.page = 0;
        loadTracks();
      }, 500);
    });
  }
  let bpmToTimer;
  const bpmToInput = document.getElementById("bpm-filter-to");
  if (bpmToInput) {
    bpmToInput.addEventListener("input", (e) => {
      clearTimeout(bpmToTimer);
      bpmToTimer = setTimeout(() => {
        const val = e.target.value.trim();
        state.bpmTo = val ? +val : null;
        state.page = 0;
        loadTracks();
      }, 500);
    });
  }

  // ─── Pagination ───
  container.addEventListener("click", (e) => {
    if (e.target.closest("#browser-prev-page")) {
      if (state.page > 0) {
        state.page--;
        loadTracks();
      }
    }
    if (e.target.closest("#browser-next-page")) {
      const totalPages = Math.ceil(state.total / state.pageSize) || 1;
      if (state.page < totalPages - 1) {
        state.page++;
        loadTracks();
      }
    }
  });

  // ─── Drag & Drop: Browser ───
  container.addEventListener("dragstart", (e) => {
    const card = e.target.closest(".track-card");
    if (!card) return;
    const handle = e.target.closest(".tc-drag-handle");
    if (!handle) {
      e.preventDefault();
      return;
    }
    const trackId = +card.dataset.trackId;
    e.dataTransfer.setData("text/plain", String(trackId));
    e.dataTransfer.effectAllowed = "move";
    card.classList.add("dragging");
  });

  container.addEventListener("dragend", (e) => {
    const card = e.target.closest(".track-card");
    if (card) card.classList.remove("dragging");
    document
      .querySelectorAll(".drag-over")
      .forEach((el) => el.classList.remove("drag-over"));
  });

  // ─── Drop: Browser → Ladder ───
  const ladderPane = document.getElementById("pane-ladder");
  if (ladderPane) {
    ladderPane.addEventListener("dragover", (e) => {
      e.preventDefault();
      e.dataTransfer.dropEffect = "move";
      const dropzone = document.getElementById("ladder-dropzone");
      if (dropzone) dropzone.classList.add("drag-over");
    });

    ladderPane.addEventListener("dragleave", (e) => {
      // Only remove if actually leaving the ladder pane
      if (!ladderPane.contains(e.relatedTarget)) {
        const dropzone = document.getElementById("ladder-dropzone");
        if (dropzone) dropzone.classList.remove("drag-over");
      }
    });

    ladderPane.addEventListener("drop", (e) => {
      e.preventDefault();
      const dropzone = document.getElementById("ladder-dropzone");
      if (dropzone) dropzone.classList.remove("drag-over");

      const trackId = parseInt(e.dataTransfer.getData("text/plain"), 10);
      if (!trackId) return;

      // Check if dropped on a specific ladder card (for reorder positioning)
      const targetCard = e.target.closest(".track-card.in-ladder");
      let insertIdx = state.ladder.length; // default: end
      if (targetCard) {
        insertIdx = +targetCard.dataset.ladderIdx;
        if (isNaN(insertIdx)) insertIdx = state.ladder.length;
      }

      // Find the track in browser results or ladder
      const existingIdx = state.ladder.findIndex((t) => t.id === trackId);
      if (existingIdx >= 0) {
        // Reorder within ladder
        moveWithinLadder(existingIdx, insertIdx);
        return;
      }

      const track = state.tracks.find((t) => t.id === trackId);
      if (!track) return;

      addToLadder(track, insertIdx);
    });
  }

  // ─── Remove from ladder ───
  container.addEventListener("click", (e) => {
    const removeBtn = e.target.closest('[data-action="remove-ladder"]');
    if (!removeBtn) return;
    const idx = +removeBtn.dataset.idx;
    removeFromLadder(idx);
  });

  // ─── Remove tag chip × ───
  container.addEventListener("click", (e) => {
    const chipX = e.target.closest(".tag-chip-x");
    if (!chipX) return;
    const tagName = chipX.dataset.tagName;
    state.selectedTagChips = state.selectedTagChips.filter(
      (c) => c.name.toLowerCase() !== tagName.toLowerCase(),
    );
    renderTagChipsInBar();
    state.page = 0;
    loadTracks();
  });

  // ─── Session save / load ───
  container.addEventListener("click", (e) => {
    if (e.target.closest("#ladder-save-session")) {
      saveSession();
      showToast("Session saved", "success");
      return;
    }
    if (e.target.closest("#ladder-load-session")) {
      if (loadSession()) {
        renderLadderPane();
        renderBrowserContent();
        state.page = 0;
        loadTracks();
        showToast("Session loaded", "success");
      } else {
        showToast("No saved session found", "info");
      }
      return;
    }
  });

  // ─── Save dialog ───
  container.addEventListener("click", (e) => {
    if (e.target.id === "ladder-save-show") {
      state.showSaveDialog = true;
      if (!state.playlistName)
        state.playlistName = "digging-" + new Date().toISOString().slice(0, 10);
      renderLadderPane();
      return;
    }
    if (e.target.id === "ladder-save-cancel") {
      state.showSaveDialog = false;
      state.playlistName = "";
      renderLadderPane();
      return;
    }
    if (e.target.id === "ladder-save-confirm") {
      saveAsPlaylist();
      return;
    }
  });

  // ─── Playlist name input ───
  container.addEventListener("input", (e) => {
    if (e.target.id === "ladder-playlist-name") {
      state.playlistName = e.target.value;
    }
  });
}
