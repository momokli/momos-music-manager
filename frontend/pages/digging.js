/**
 * digging.js — Digging Curator page module.
 *
 * A tool for DJs to prepare digging sessions by building harmonic chains
 * of tracks. Select a seed track, configure Camelot wheel jumps and BPM
 * range, then browse suggestions to build a harmonic chain. Save the
 * chain as a tag-linked playlist.
 *
 * State machine:
 *   SEED_SELECTION → CHAIN_BUILDING (when seed chosen)
 *   CHAIN_BUILDING → SEED_SELECTION (when seed cleared or new seed clicked)
 */

import { fetchJSON } from "../shared/api.js";
import {
  renderLoading,
  renderEmpty,
  renderErrorBlock,
  showToast,
} from "../shared/components.js";

/* ================================================================== */
/*  Constants                                                          */
/* ================================================================== */

/** Page panel modes */
const MODE = {
  SEED_SELECTION: "seed-selection",
  CHAIN_BUILDING: "chain-building",
};

/** Default seed fetch parameters */
const DEFAULT_SEED_PARAMS = {
  sortBy: "play_count",
  sortOrder: "asc",
  playCountMax: 5,
  limit: 20,
};

/** Available sort fields for seed browsing */
const SORT_OPTIONS = [
  { value: "play_count", label: "Play Count" },
  { value: "last_played", label: "Last Played" },
  { value: "bpm", label: "BPM" },
  { value: "title", label: "Title" },
  { value: "rating", label: "Rating" },
  { value: "random", label: "Random" },
];

/** Camelot jump labels and their API values */
const JUMP_OPTIONS = [
  { label: "+1", value: "+1" },
  { label: "-1", value: "-1" },
  { label: "+2", value: "+2" },
  { label: "-2", value: "-2" },
  { label: "+7", value: "+7" },
  { label: "-7", value: "-7" },
  { label: "A↔B", value: "a_to_b" },
  { label: "d↔m", value: "relative" },
  { label: "±0", value: "same" },
];

/** Colours for compatibility badges */
const COMPAT_COLORS = {
  perfect: "var(--green)",
  good: "var(--accent)",
  ok: "var(--yellow)",
  unknown: "var(--text-subtle)",
};

/** Compatibility indicator dots */
const COMPAT_DOTS = {
  perfect: "●",
  good: "●",
  ok: "●",
  unknown: "○",
};

/* ================================================================== */
/*  Helpers                                                            */
/* ================================================================== */

function escapeHtml(str) {
  if (typeof str !== "string") return str;
  return str
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

/**
 * Parse the PMV comment format: [{phase}{mood}{vibe}] tags source_id
 * Returns { phase: string|null, mood: string|null, vibe: string|null }
 */
function parsePmv(comment) {
  const result = { phase: null, mood: null, vibe: null };
  if (!comment) return result;
  const match = comment.match(/^\[([^\]]*)\]/);
  if (!match) return result;
  const chars = match[1];
  if (chars.length >= 1) result.phase = chars[0];
  if (chars.length >= 2) result.mood = chars[1];
  if (chars.length >= 3) result.vibe = chars[2];
  return result;
}

/**
 * Render PMV indicators: [P _ _], [P M _], [P M V], etc.
 */
function renderPmv(comment) {
  const pmv = parsePmv(comment);
  const p = pmv.phase ? pmv.phase : "_";
  const m = pmv.mood ? pmv.mood : "_";
  const v = pmv.vibe ? pmv.vibe : "_";
  return `<span class="font-mono text-xs" style="color:var(--text-muted)">[${escapeHtml(p)}${escapeHtml(m)}${escapeHtml(v)}]</span>`;
}

/**
 * Format BPM as integer string.
 */
function fmtBpm(bpm) {
  if (bpm == null || bpm === 0) return "\u2014";
  return String(Math.round(bpm));
}

/**
 * Format play count.
 */
function fmtPlays(count) {
  if (count == null) return "0";
  return String(count);
}

/**
 * Render a Camelot key badge.
 */
function renderKeyBadge(key, compatColor) {
  if (!key) {
    return `<span class="font-mono text-xs" style="color:var(--text-subtle)">\u2014</span>`;
  }
  const bg = compatColor || "var(--border)";
  const color = compatColor ? "#fff" : "var(--text-muted)";
  return `<span class="font-mono text-xs" style="background:${bg};color:${color};padding:2px 6px;border-radius:var(--radius-sm);font-weight:600">${escapeHtml(key)}</span>`;
}

/**
 * Build query string from params object, skipping null/undefined/empty.
 */
function buildQueryString(params) {
  const sp = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v != null && v !== "" && v !== false) {
      sp.set(k, String(v));
    }
  }
  return sp.toString();
}

/**
 * Compute a human-readable jump label between two tracks by comparing
 * their Camelot keys.
 */
function computeJumpLabel(prev, next) {
  if (!prev || !next) return "";
  if (!prev.musicalKey || !next.musicalKey) return "";
  if (prev.musicalKey === next.musicalKey) return "\u00B10";
  const prevMatch = prev.musicalKey.match(/^(\d+)([AB])$/);
  const nextMatch = next.musicalKey.match(/^(\d+)([AB])$/);
  if (!prevMatch || !nextMatch) return "";
  const prevNum = parseInt(prevMatch[1], 10);
  const nextNum = parseInt(nextMatch[1], 10);
  const prevLetter = prevMatch[2];
  const nextLetter = nextMatch[2];
  const diff = nextNum - prevNum;
  const wrappedDiff = ((diff % 12) + 12) % 12;
  if (prevLetter !== nextLetter && wrappedDiff === 0) return "A\u2194B";
  if (wrappedDiff <= 6) return `+${wrappedDiff}`;
  return `-${12 - wrappedDiff}`;
}

/* ================================================================== */
/*  Default state factory                                              */
/* ================================================================== */

function createInitialState() {
  return {
    mode: MODE.SEED_SELECTION,

    // Seed browsing
    seedTab: "browse",
    sortBy: DEFAULT_SEED_PARAMS.sortBy,
    sortOrder: DEFAULT_SEED_PARAMS.sortOrder,
    browseBpmMin: "",
    browseBpmMax: "",
    playCountMax: DEFAULT_SEED_PARAMS.playCountMax,
    searchQuery: "",
    browseResults: [],

    // Chain building
    seedTrack: null,
    chain: [],
    chainTrackIds: new Set(),
    suggestions: [],
    activeJumps: new Set(["+1", "-1", "a_to_b", "same"]),
    bpmRange: 8,

    // Save
    tagName: "",
    tagNameCounter: 1,

    // Loading flags
    loadingSeeds: false,
    loadingSuggestions: false,
    saving: false,
  };
}

/* ================================================================== */
/*  API calls                                                          */
/* ================================================================== */

async function fetchSeeds(params, signal) {
  return fetchJSON(`/api/digging/seeds?${buildQueryString(params)}`, { signal });
}

async function fetchFile(id, signal) {
  return fetchJSON(`/api/files/${id}`, { signal });
}

async function fetchSuggestions(seedTrackId, activeJumps, bpmRange, limit, signal) {
  return fetchJSON("/api/digging/suggestions", {
    method: "POST",
    body: JSON.stringify({
      seedTrackId,
      activeJumps: Array.from(activeJumps),
      bpmRange,
      limit: limit || 20,
    }),
    signal,
  });
}

async function saveChain(tagName, trackIds, signal) {
  return fetchJSON("/api/digging/save-chain", {
    method: "POST",
    body: JSON.stringify({ tagName, trackIds, commentUpdates: true }),
    signal,
  });
}

/* ================================================================== */
/*  Render helpers — each returns an HTML string for a panel           */
/* ================================================================== */

/**
 * Seed selection panel — Browse or Search mode.
 */
function renderSeedPanel(state) {
  const isBrowse = state.seedTab === "browse";

  const sortOptsHtml = SORT_OPTIONS.map(
    (o) =>
      `<option value="${o.value}"${state.sortBy === o.value ? " selected" : ""}>${escapeHtml(o.label)}</option>`,
  ).join("");

  const orderIcon =
    state.sortOrder === "asc" ? "fa-arrow-up-short-wide" : "fa-arrow-down-wide-short";
  const orderTitle = state.sortOrder === "asc" ? "Ascending" : "Descending";

  const tabBrowseCls = isBrowse ? "digging-tab-active" : "";
  const tabSearchCls = !isBrowse ? "digging-tab-active" : "";

  // Active tab style override
  const tabActiveStyle = "color:var(--text);border-bottom-color:var(--accent)";

  // Seed results
  let resultsHtml;
  if (state.loadingSeeds) {
    resultsHtml =
      '<div class="loading" style="padding:var(--space-8) 0"><div class="spinner"></div><p>Loading seeds...</p></div>';
  } else if (state.browseResults.length === 0) {
    resultsHtml =
      '<div class="empty-state" style="padding:var(--space-8) 0"><div class="empty-icon"><i class="fas fa-music"></i></div><h3>No tracks found</h3><p>Try different filters or search criteria.</p></div>';
  } else {
    resultsHtml =
      '<div style="max-height:400px;overflow-y:auto">' +
      state.browseResults
        .map(
          (t) =>
            `<div style="display:flex;align-items:center;gap:var(--space-3);padding:var(--space-2) var(--space-3);border-bottom:1px solid var(--border);transition:background var(--transition)">
              <div style="flex:1;min-width:0">
                <div style="font-weight:600;font-size:0.9rem;white-space:nowrap;overflow:hidden;text-overflow:ellipsis">${escapeHtml(t.title)}</div>
                <span class="text-xs" style="color:var(--text-muted)">${escapeHtml(t.artist || "Unknown")}</span>
              </div>
              <div style="display:flex;align-items:center;gap:6px;flex-shrink:0">
                <span class="font-mono text-sm" style="color:var(--text-secondary);font-weight:600">${fmtBpm(t.bpm)} BPM</span>
                ${renderKeyBadge(t.musicalKey)}
                <span class="text-xs" style="color:var(--text-subtle)">${fmtPlays(t.playCount)} plays</span>
                ${renderPmv(t.comment)}
              </div>
              <button class="btn btn-sm btn-primary digging-select-seed" data-id="${t.id}" style="flex-shrink:0">
                <i class="fas fa-plus"></i> Select
              </button>
            </div>`,
        )
        .join("") +
      "</div>";
  }

  const baseTabStyle =
    "background:none;border:none;padding:var(--space-2) var(--space-3);cursor:pointer;font-size:0.85rem;color:var(--text-muted);border-bottom:2px solid transparent;transition:color var(--transition),border-color var(--transition)";

  return `
    <div class="card" style="overflow:hidden;margin-top:var(--space-4)">
      <div style="display:flex;align-items:center;justify-content:space-between;border-bottom:1px solid var(--border);flex-wrap:wrap">
        <div style="display:flex">
          <button class="digging-tab ${tabBrowseCls}" data-tab="browse" style="${baseTabStyle};${isBrowse ? tabActiveStyle : ""}">
            <i class="fas fa-list"></i> Browse
          </button>
          <button class="digging-tab ${tabSearchCls}" data-tab="search" style="${baseTabStyle};${!isBrowse ? tabActiveStyle : ""}">
            <i class="fas fa-search"></i> Search
          </button>
        </div>
        <div style="display:flex;align-items:center;gap:var(--space-2);padding:var(--space-2) var(--space-3)">
          ${
            isBrowse
              ? `
            <div style="display:flex;align-items:center;gap:4px">
              <select class="input-text digging-sort-select" style="width:auto;font-size:0.8rem;padding:4px 8px">
                ${sortOptsHtml}
              </select>
              <button class="btn btn-xs digging-order-toggle" title="${orderTitle}">
                <i class="fas ${orderIcon}"></i>
              </button>
            </div>
            <div style="display:flex;align-items:center;gap:4px">
              <input type="number" class="input-text digging-bpm-min" placeholder="BPM min"
                value="${escapeHtml(state.browseBpmMin)}"
                style="width:62px;font-size:0.8rem;padding:4px 8px">
              <span class="text-xs" style="color:var(--text-subtle)">\u2013</span>
              <input type="number" class="input-text digging-bpm-max" placeholder="BPM max"
                value="${escapeHtml(state.browseBpmMax)}"
                style="width:62px;font-size:0.8rem;padding:4px 8px">
            </div>`
              : `
            <div class="search-wrap" style="margin:0">
              <i class="fas fa-search"></i>
              <input type="text" class="input-text digging-search-input"
                placeholder="Search title or artist..."
                value="${escapeHtml(state.searchQuery)}"
                style="font-size:0.8rem;padding:4px 8px 4px 28px">
            </div>`
          }
        </div>
      </div>
      <div>${resultsHtml}</div>
      <div style="padding:var(--space-3);text-align:center;border-top:1px solid var(--border)">
        <button class="btn btn-sm digging-random-seed">
          <i class="fas fa-dice"></i> Random Seed
        </button>
      </div>
    </div>`;
}

/**
 * Chain panel (left side of the workspace).
 */
function renderChainPanel(state) {
  let entriesHtml;

  if (!state.seedTrack) {
    entriesHtml =
      '<div class="empty-state" style="padding:var(--space-8) 0">' +
      '<div class="empty-icon"><i class="fas fa-link"></i></div>' +
      "<h3>No seed selected</h3>" +
      "<p>Browse or search for a seed track to start.</p>" +
      '<button class="btn btn-sm digging-new-seed"><i class="fas fa-search"></i> Select Seed</button>' +
      "</div>";
  } else if (state.chain.length === 0) {
    entriesHtml =
      '<div class="empty-state" style="padding:var(--space-8) 0">' +
      '<div class="empty-icon"><i class="fas fa-plus-circle" style="color:var(--text-subtle)"></i></div>' +
      '<p class="text-sm" style="color:var(--text-muted)">Add suggestions to build your chain.</p>' +
      "</div>";
  } else {
    entriesHtml = state.chain
      .map((track, i) => {
        const isSeed = i === 0;
        const num = String(i + 1).padStart(2, "0");
        const jumpLabel = i > 0 ? computeJumpLabel(state.chain[i - 1], track) : null;

        return (
          (jumpLabel
            ? `<div style="display:flex;flex-direction:column;align-items:center;padding:2px 0">
                <span class="font-mono text-xs" style="color:var(--text-subtle);background:var(--bg);padding:0 6px;border-radius:var(--radius-sm);z-index:1">${escapeHtml(jumpLabel)}</span>
                <div style="width:1px;height:12px;background:var(--border);margin-top:-1px"></div>
              </div>`
            : "") +
          `<div style="display:flex;align-items:center;gap:var(--space-2);padding:var(--space-2);border-radius:var(--radius-md);background:${isSeed ? "var(--accent-bg)" : "transparent"};border:1px solid ${isSeed ? "var(--accent)" : "var(--border)"}">
            <div style="width:28px;height:28px;display:flex;align-items:center;justify-content:center;border-radius:var(--radius-sm);background:${isSeed ? "var(--accent)" : "var(--border)"};color:${isSeed ? "#fff" : "var(--text-muted)"};font-size:0.75rem;font-weight:700;font-family:var(--font-mono);flex-shrink:0">${num}</div>
            <div style="flex:1;min-width:0">
              <div style="display:flex;align-items:center;gap:6px;margin-bottom:2px">
                ${renderKeyBadge(track.musicalKey)}
                <span class="font-mono text-sm" style="color:var(--text-secondary);font-weight:600">${fmtBpm(track.bpm)}</span>
                ${renderPmv(track.comment)}
              </div>
              <div style="font-size:0.85rem;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis">${escapeHtml(track.title)}</div>
              <div class="text-xs" style="color:var(--text-muted)">${escapeHtml(track.artist || "Unknown")}</div>
            </div>
            ${
              isSeed
                ? '<span class="text-xs" style="color:var(--accent);font-weight:600;white-space:nowrap">SEED</span>'
                : `<button class="digging-remove-chain" data-index="${i}" title="Remove" style="background:none;border:none;cursor:pointer;padding:4px;border-radius:var(--radius-sm);opacity:0.6;transition:opacity var(--transition);flex-shrink:0">
                     <i class="fas fa-times" style="color:var(--red);font-size:0.8rem"></i>
                   </button>`
            }
          </div>`
        );
      })
      .join("");
  }

  let saveHtml = "";
  if (state.seedTrack && state.chain.length > 0) {
    const tagVal =
      state.tagName || `digging-${String(state.tagNameCounter).padStart(2, "0")}`;
    saveHtml = `
      <div style="padding:var(--space-3);border-top:1px solid var(--border)">
        <div style="display:flex;align-items:center;gap:var(--space-2)">
          <input type="text" class="input-text digging-tag-name" value="${escapeHtml(tagVal)}"
            placeholder="digging-01" style="flex:1;font-size:0.85rem;padding:6px 10px">
          <button class="btn btn-sm btn-green digging-save-chain" ${state.saving ? "disabled" : ""}>
            <i class="fas fa-save"></i> ${state.saving ? "Saving..." : "Save Chain"}
          </button>
        </div>
        <p class="text-xs" style="color:var(--text-subtle);margin-top:4px">
          <i class="fas fa-info-circle"></i> Saves as a tag with linked tracks and comment updates.
        </p>
      </div>`;
  }

  return `
    <div style="display:flex;align-items:center;justify-content:space-between;padding:var(--space-3);border-bottom:1px solid var(--border)">
      <h3 style="margin:0;font-size:0.95rem"><i class="fas fa-link" style="color:var(--accent);margin-right:6px"></i>Chain</h3>
      <div style="display:flex;gap:var(--space-2)">
        ${state.seedTrack ? '<button class="btn btn-xs digging-clear-chain"><i class="fas fa-times"></i> Clear</button>' : ""}
        ${state.seedTrack ? '<button class="btn btn-xs digging-new-seed"><i class="fas fa-search"></i> New Seed</button>' : ""}
      </div>
    </div>
    <div style="flex:1;overflow-y:auto;padding:var(--space-2);display:flex;flex-direction:column;gap:2px">
      ${entriesHtml}
    </div>
    ${saveHtml}`;
}

/**
 * Suggestions panel (right side of the workspace).
 */
function renderSuggestionsPanel(state) {
  // Jump chips
  const jumpsHtml = JUMP_OPTIONS.map((j) => {
    const active = state.activeJumps.has(j.value);
    return `<button class="digging-jump-chip" data-jump="${j.value}"
      style="background:${active ? "var(--accent)" : "var(--bg)"};color:${active ? "#fff" : "var(--text-muted)"};border:1px solid ${active ? "var(--accent)" : "var(--border)"};border-radius:var(--radius-md);padding:3px 10px;font-size:0.75rem;cursor:pointer;font-family:var(--font-mono);transition:background var(--transition),color var(--transition),border-color var(--transition);font-weight:${active ? "600" : "400"}">${escapeHtml(j.label)}</button>`;
  }).join("");

  const bpmRangeVal = state.bpmRange;

  // Suggestions body
  let bodyHtml;
  if (!state.seedTrack) {
    bodyHtml =
      '<div class="empty-state" style="padding:var(--space-10) 0">' +
      '<div class="empty-icon"><i class="fas fa-arrow-left"></i></div>' +
      "<h3>Select a seed first</h3>" +
      "<p>Choose a seed track to get harmonic suggestions.</p>" +
      "</div>";
  } else if (state.loadingSuggestions) {
    bodyHtml =
      '<div class="loading" style="padding:var(--space-10) 0"><div class="spinner"></div><p>Finding suggestions...</p></div>';
  } else if (state.suggestions.length === 0) {
    bodyHtml =
      '<div class="empty-state" style="padding:var(--space-10) 0">' +
      '<div class="empty-icon"><i class="fas fa-search"></i></div>' +
      "<h3>No suggestions found</h3>" +
      "<p>Try adjusting your harmonic jumps or BPM range.</p>" +
      "</div>";
  } else {
    bodyHtml =
      '<div style="padding:var(--space-1)">' +
      state.suggestions
        .map((t, i) => {
          const inChain = state.chainTrackIds.has(t.id);
          const compatColor =
            COMPAT_COLORS[t.camelotCompatibility] || COMPAT_COLORS.unknown;
          const compatDot = COMPAT_DOTS[t.camelotCompatibility] || "?";
          const scoreStr =
            t.score != null ? (t.score > 0 ? "+" : "") + t.score.toFixed(0) : "";
          return `<div style="display:flex;align-items:center;gap:var(--space-2);padding:var(--space-2);border-radius:var(--radius-md);border:1px solid var(--border);margin-bottom:var(--space-1);transition:background var(--transition)">
            <div style="font-size:1.1rem;flex-shrink:0;width:16px;text-align:center" title="${escapeHtml(t.camelotCompatibility || "unknown")}">${compatDot}</div>
            <div style="flex:1;min-width:0">
              <div style="font-size:0.85rem;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis">${escapeHtml(t.title)}</div>
              <div class="text-xs" style="color:var(--text-muted)">${escapeHtml(t.artist || "Unknown")}</div>
              <div style="display:flex;align-items:center;gap:6px;margin-top:3px">
                ${renderKeyBadge(t.musicalKey)}
                <span class="font-mono text-xs" style="color:var(--text-secondary);font-weight:600">${fmtBpm(t.bpm)} BPM</span>
                <span class="text-xs" style="color:var(--text-subtle)">${fmtPlays(t.playCount)} plays</span>
                ${
                  t.bpmDiff != null
                    ? `<span class="text-xs font-mono" style="color:var(--text-subtle)">${t.bpmDiff >= 0 ? "+" : ""}${t.bpmDiff.toFixed(1)}</span>`
                    : ""
                }
              </div>
            </div>
            <div style="font-size:0.75rem;font-weight:700;min-width:24px;text-align:right;color:${t.score < 0 ? "var(--green)" : t.score > 0 ? "var(--red)" : "var(--text-muted)"};flex-shrink:0">
              ${scoreStr}
            </div>
            ${
              inChain
                ? '<span class="text-xs" style="color:var(--green);font-weight:600;white-space:nowrap;flex-shrink:0"><i class="fas fa-check"></i> In Chain</span>'
                : `<button class="btn btn-xs digging-add-to-chain" data-id="${t.id}" data-index="${i}" style="flex-shrink:0">
                     <i class="fas fa-plus"></i> Add
                   </button>`
            }
          </div>`;
        })
        .join("") +
      "</div>";
  }

  return `
    <div style="display:flex;align-items:center;justify-content:space-between;padding:var(--space-3);border-bottom:1px solid var(--border)">
      <h3 style="margin:0;font-size:0.95rem"><i class="fas fa-lightbulb" style="color:var(--yellow);margin-right:6px"></i>Suggestions</h3>
      ${state.seedTrack ? '<button class="btn btn-xs digging-refresh-suggestions" title="Refresh"><i class="fas fa-rotate"></i></button>' : ""}
    </div>
    <div style="padding:var(--space-2) var(--space-3);border-bottom:1px solid var(--border)">
      <div style="display:flex;flex-wrap:wrap;gap:4px;margin-bottom:var(--space-2)">${jumpsHtml}</div>
      <div>
        <div style="display:flex;align-items:center;justify-content:space-between">
          <label class="text-xs" style="color:var(--text-subtle)">BPM Range:</label>
          <span class="font-mono text-xs" style="color:var(--text-secondary);font-weight:600">\u00B1${bpmRangeVal}</span>
        </div>
        <input type="range" class="digging-bpm-range-slider" min="1" max="24" value="${bpmRangeVal}" step="1"
          style="width:100%;accent-color:var(--accent);margin:4px 0">
        <div style="display:flex;justify-content:space-between" class="text-xs" style="color:var(--text-subtle)">
          <span>\u00B11</span>
          <span>\u00B124</span>
        </div>
      </div>
    </div>
    <div style="flex:1;overflow-y:auto">${bodyHtml}</div>`;
}

/**
 * Full workspace view (chain + suggestions side by side).
 */
function renderWorkspace(state) {
  return `
    <div style="display:flex;gap:var(--space-4);align-items:stretch;margin-top:var(--space-4);min-height:500px">
      <div style="flex:1;display:flex;flex-direction:column;background:var(--surface);border:1px solid var(--border);border-radius:var(--radius-lg);overflow:hidden;min-width:0">${renderChainPanel(state)}</div>
      <div style="flex:2;display:flex;flex-direction:column;background:var(--surface);border:1px solid var(--border);border-radius:var(--radius-lg);overflow:hidden;min-width:0">${renderSuggestionsPanel(state)}</div>
    </div>`;
}

/**
 * Render the full page content based on current state.
 */
function renderContent(state) {
  if (state.mode === MODE.SEED_SELECTION) {
    return renderSeedPanel(state);
  }
  return renderWorkspace(state);
}

/**
 * Build the page shell and full content, returns HTML.
 */
function renderPage(state) {
  return `<div id="digging-content">${renderContent(state)}</div>`;
}

/* ================================================================== */
/*  Data loading functions — also re-render after fetch                */
/* ================================================================== */

/**
 * Fetch browse seeds and re-render.
 */
async function loadBrowseSeeds(contentEl, state) {
  if (state.signal.aborted) return;
  state.loadingSeeds = true;

  const params = {
    sortBy: state.sortBy,
    sortOrder: state.sortOrder,
    playCountMax: state.playCountMax,
    limit: DEFAULT_SEED_PARAMS.limit,
  };
  if (state.browseBpmMin) params.bpmMin = state.browseBpmMin;
  if (state.browseBpmMax) params.bpmMax = state.browseBpmMax;

  try {
    const data = await fetchSeeds(params, state.signal);
    if (state.signal.aborted) return;
    state.browseResults = Array.isArray(data) ? data : data.data || data.tracks || [];
  } catch (err) {
    if (err.name === "AbortError") return;
    state.browseResults = [];
    showToast(`Failed to load seeds: ${err.message}`, "error");
  } finally {
    state.loadingSeeds = false;
  }

  contentEl.innerHTML = renderContent(state);
}

/**
 * Fetch search results and re-render.
 */
async function loadSearchResults(contentEl, state) {
  if (state.signal.aborted || !state.searchQuery) return;
  state.loadingSeeds = true;

  try {
    const data = await fetchSeeds(
      { search: state.searchQuery, limit: DEFAULT_SEED_PARAMS.limit },
      state.signal,
    );
    if (state.signal.aborted) return;
    state.browseResults = Array.isArray(data) ? data : data.data || data.tracks || [];
  } catch (err) {
    if (err.name === "AbortError") return;
    state.browseResults = [];
    showToast(`Search failed: ${err.message}`, "error");
  } finally {
    state.loadingSeeds = false;
  }

  contentEl.innerHTML = renderContent(state);
}

/**
 * Select a seed track and transition to chain-building mode.
 */
async function selectSeed(contentEl, state, trackId) {
  if (state.signal.aborted) return;

  try {
    const data = await fetchFile(trackId, state.signal);
    if (state.signal.aborted) return;
    const track = data.data || data;

    state.seedTrack = track;
    state.chain = [track];
    state.chainTrackIds = new Set([track.id]);
    state.suggestions = [];
    state.tagName = "";
    state.mode = MODE.CHAIN_BUILDING;

    // Render workspace immediately, then fetch suggestions
    contentEl.innerHTML = renderContent(state);
    await refreshSuggestions(contentEl, state);
  } catch (err) {
    if (err.name === "AbortError") return;
    showToast(`Failed to select seed: ${err.message}`, "error");
  }
}

/**
 * Refresh suggestions for the current seed track.
 */
async function refreshSuggestions(contentEl, state) {
  if (state.signal.aborted) return;
  if (!state.seedTrack) return;

  state.loadingSuggestions = true;
  contentEl.innerHTML = renderContent(state);

  try {
    const data = await fetchSuggestions(
      state.seedTrack.id,
      state.activeJumps,
      state.bpmRange,
      20,
      state.signal,
    );
    if (state.signal.aborted) return;
    state.suggestions = Array.isArray(data) ? data : data.data || data.tracks || [];
  } catch (err) {
    if (err.name === "AbortError") return;
    state.suggestions = [];
    showToast(`Failed to get suggestions: ${err.message}`, "error");
  } finally {
    state.loadingSuggestions = false;
  }

  contentEl.innerHTML = renderContent(state);
}

/**
 * Re-render the workspace (chain + suggestions) without re-fetching.
 */
function rerenderWorkspace(contentEl, state) {
  if (state.mode !== MODE.CHAIN_BUILDING) return;
  contentEl.innerHTML = renderContent(state);
}

/* ================================================================== */
/*  Event wiring — delegation on the content element                   */
/* ================================================================== */

function setupEvents(contentEl, state) {
  // ---------- click ----------
  contentEl.addEventListener("click", async (e) => {
    if (state.saving) return;

    // ---- Tab switch ----
    const tabBtn = e.target.closest("[data-tab]");
    if (tabBtn) {
      state.seedTab = tabBtn.dataset.tab;
      state.browseResults = [];
      if (state.seedTab === "browse") {
        await loadBrowseSeeds(contentEl, state);
      } else {
        contentEl.innerHTML = renderContent(state);
      }
      return;
    }

    // ---- Sort order toggle ----
    const orderBtn = e.target.closest(".digging-order-toggle");
    if (orderBtn) {
      state.sortOrder = state.sortOrder === "asc" ? "desc" : "asc";
      await loadBrowseSeeds(contentEl, state);
      return;
    }

    // ---- Select seed ----
    const selectBtn = e.target.closest(".digging-select-seed");
    if (selectBtn) {
      const id = parseInt(selectBtn.dataset.id, 10);
      if (!isNaN(id)) await selectSeed(contentEl, state, id);
      return;
    }

    // ---- Random seed ----
    const randomBtn = e.target.closest(".digging-random-seed");
    if (randomBtn) {
      state.sortBy = "random";
      await loadBrowseSeeds(contentEl, state);
      return;
    }

    // ---- New seed (return to seed selection) ----
    const newSeedBtn = e.target.closest(".digging-new-seed");
    if (newSeedBtn) {
      state.mode = MODE.SEED_SELECTION;
      state.seedTrack = null;
      state.chain = [];
      state.chainTrackIds.clear();
      state.suggestions = [];
      state.seedTab = "browse";
      await loadBrowseSeeds(contentEl, state);
      return;
    }

    // ---- Clear chain ----
    const clearBtn = e.target.closest(".digging-clear-chain");
    if (clearBtn) {
      state.chain = [];
      state.chainTrackIds.clear();
      state.tagName = "";
      await refreshSuggestions(contentEl, state);
      return;
    }

    // ---- Jump chip toggle ----
    const jumpChip = e.target.closest(".digging-jump-chip");
    if (jumpChip) {
      const val = jumpChip.dataset.jump;
      if (state.activeJumps.has(val)) {
        if (state.activeJumps.size > 1) {
          state.activeJumps.delete(val);
        }
      } else {
        state.activeJumps.add(val);
      }
      await refreshSuggestions(contentEl, state);
      return;
    }

    // ---- Add to chain ----
    const addBtn = e.target.closest(".digging-add-to-chain");
    if (addBtn) {
      const id = parseInt(addBtn.dataset.id, 10);
      const index = parseInt(addBtn.dataset.index, 10);
      const track = state.suggestions[index];
      if (track && id === track.id && !state.chainTrackIds.has(id)) {
        state.chain.push(track);
        state.chainTrackIds.add(id);
        if (!state.tagName) {
          state.tagNameCounter = state.chain.length;
        }
        rerenderWorkspace(contentEl, state);
      }
      return;
    }

    // ---- Remove from chain ----
    const removeBtn = e.target.closest(".digging-remove-chain");
    if (removeBtn) {
      const idx = parseInt(removeBtn.dataset.index, 10);
      if (idx >= 0 && idx < state.chain.length) {
        const removed = state.chain.splice(idx, 1)[0];
        state.chainTrackIds.delete(removed.id);
        rerenderWorkspace(contentEl, state);
      }
      return;
    }

    // ---- Save chain ----
    const saveBtn = e.target.closest(".digging-save-chain");
    if (saveBtn && !state.saving) {
      const input = contentEl.querySelector(".digging-tag-name");
      const tagName = input ? input.value.trim() : state.tagName;
      if (!tagName) {
        showToast("Please enter a tag name", "error");
        return;
      }
      if (state.chain.length === 0) {
        showToast("Chain is empty \u2014 add tracks first", "error");
        return;
      }
      state.saving = true;
      state.tagName = tagName;
      try {
        await saveChain(
          tagName,
          state.chain.map((t) => t.id),
          state.signal,
        );
        if (state.signal.aborted) return;
        showToast(`Chain "${tagName}" saved successfully!`, "success");
        state.tagNameCounter++;
      } catch (err) {
        if (err.name === "AbortError") return;
        showToast(`Failed to save chain: ${err.message}`, "error");
      } finally {
        state.saving = false;
      }
      rerenderWorkspace(contentEl, state);
      return;
    }

    // ---- Refresh suggestions ----
    const refreshBtn = e.target.closest(".digging-refresh-suggestions");
    if (refreshBtn) {
      await refreshSuggestions(contentEl, state);
      return;
    }
  });

  // ---------- change ----------
  let debounceTimer;
  contentEl.addEventListener("change", (e) => {
    const sortSelect = e.target.closest(".digging-sort-select");
    if (sortSelect) {
      state.sortBy = sortSelect.value;
      loadBrowseSeeds(contentEl, state);
      return;
    }
  });

  // ---------- input (debounced) ----------
  let inputTimer;
  contentEl.addEventListener("input", (e) => {
    // BPM min/max browse
    const bpmMinInput = e.target.closest(".digging-bpm-min");
    const bpmMaxInput = e.target.closest(".digging-bpm-max");
    if (bpmMinInput || bpmMaxInput) {
      clearTimeout(inputTimer);
      inputTimer = setTimeout(() => {
        state.browseBpmMin = contentEl.querySelector(".digging-bpm-min")?.value || "";
        state.browseBpmMax = contentEl.querySelector(".digging-bpm-max")?.value || "";
        loadBrowseSeeds(contentEl, state);
      }, 500);
      return;
    }

    // Search input
    const searchInput = e.target.closest(".digging-search-input");
    if (searchInput) {
      clearTimeout(inputTimer);
      inputTimer = setTimeout(() => {
        state.searchQuery = searchInput.value.trim();
        if (state.searchQuery.length > 0) {
          loadSearchResults(contentEl, state);
        } else {
          state.browseResults = [];
          contentEl.innerHTML = renderContent(state);
        }
      }, 350);
      return;
    }

    // BPM range slider
    const slider = e.target.closest(".digging-bpm-range-slider");
    if (slider) {
      state.bpmRange = parseInt(slider.value, 10);
      const labelSpan = contentEl.querySelector(".digging-bpm-slider-area .font-mono");
      if (labelSpan) labelSpan.textContent = String(state.bpmRange);
      clearTimeout(debounceTimer);
      debounceTimer = setTimeout(() => {
        refreshSuggestions(contentEl, state);
      }, 400);
      return;
    }

    // Tag name input
    const tagInput = e.target.closest(".digging-tag-name");
    if (tagInput) {
      state.tagName = tagInput.value;
      return;
    }
  });
}

/* ================================================================== */
/*  Page initialisation                                                */
/* ================================================================== */

export async function init(container, signal) {
  const state = createInitialState();
  state.signal = signal;

  // Render full page
  container.innerHTML = renderPage(state);
  if (signal.aborted) return;

  const contentEl = container.querySelector("#digging-content");
  if (!contentEl) return;

  // Set up event delegation (persists across innerHTML replacements)
  setupEvents(contentEl, state);

  // Initial load of browse seeds
  await loadBrowseSeeds(contentEl, state);

  // Cleanup on abort
  signal.addEventListener("abort", () => {
    // Event listeners are on the container which will be removed on navigation.
  });
}
