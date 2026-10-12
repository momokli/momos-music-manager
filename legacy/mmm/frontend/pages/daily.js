/**
 * daily.js — Daily Tagging Queue page.
 *
 * Generates narrowed Spotify playlists for on-the-go tagging.
 * Listen on your phone and tag by adding tracks to tag-named playlists
 * in Spotify.
 *
 * Layout:
 *   ┌── Header + description ───────────────────────────────────┐
 *   ┌── FORM CARD ──────────────────────────────────────────────┐
 *   │  Source Tags: [typeahead search] [chip ×] [chip ×]        │
 *   │  BPM Range:   [Min] – [Max]  [preset buttons]            │
 *   │  Tracks/batch: [N]                                        │
 *   │  ☑ Exclude already fully tagged (P+M+V)                   │
 *   │  [Generate Playlist]                                      │
 *   └───────────────────────────────────────────────────────────┘
 *   ┌── RESULT ─────────────────────────────────────────────────┐
 *   │  ✅ PlaylistName · N tracks · BPM range                   │
 *   │  [Open in Spotify]                                        │
 *   └───────────────────────────────────────────────────────────┘
 *   ┌── HISTORY ────────────────────────────────────────────────┐
 *   │  PlaylistName · N tracks · [▶] · date                     │
 *   └───────────────────────────────────────────────────────────┘
 */

import { escapeHtml, showToast } from "../shared/components.js";
import { fetchJSON } from "../shared/api.js";

/* ------------------------------------------------------------------ */
/*  State                                                              */
/* ------------------------------------------------------------------ */

const HISTORY_KEY = "dailyHistory";
const MAX_HISTORY = 20;

const state = {
  selectedTags: [], // [{ id, name }]
  bpmMin: 145,
  bpmMax: 155,
  limit: 20,
  excludeFullyTagged: true,
  loading: false,
  result: null, // latest API response
  history: [], // array of { playlistName, trackCount, spotifyUrl, generatedAt }
};

let _container = null;

/* ------------------------------------------------------------------ */
/*  Persistence                                                        */
/* ------------------------------------------------------------------ */

function loadHistory() {
  try {
    const raw = localStorage.getItem(HISTORY_KEY);
    state.history = raw ? JSON.parse(raw) : [];
  } catch {
    state.history = [];
  }
}

function saveHistory() {
  try {
    localStorage.setItem(
      HISTORY_KEY,
      JSON.stringify(state.history.slice(0, MAX_HISTORY)),
    );
  } catch {
    /* quota exceeded — silently drop oldest */
  }
}

/* ------------------------------------------------------------------ */
/*  Initialization                                                     */
/* ------------------------------------------------------------------ */

export async function init(container, _signal, _hashParams) {
  _container = container;
  loadHistory();
  render();
  wireEvents();
}

/* ------------------------------------------------------------------ */
/*  Render                                                             */
/* ------------------------------------------------------------------ */

function render() {
  _container.innerHTML = `
    <div class="daily-page">
      <div class="page-header">
        <h1><i class="fa-solid fa-calendar-day"></i> Daily Tagging Queue</h1>
      </div>
      <p class="daily-intro">
        Generate a narrowed Spotify playlist for on-the-go tagging.
        Listen on your phone and tag by adding tracks to tag-named playlists
        in Spotify.
      </p>

      <div class="card daily-form">
        ${renderSourceTagsRow()}
        ${renderBpmRow()}
        ${renderLimitRow()}
        ${renderExcludeRow()}
        <div class="daily-form-row">
          <button id="daily-generate" class="btn btn-primary" ${state.loading ? "disabled" : ""}>
            ${
              state.loading
                ? '<i class="fa-solid fa-spinner fa-spin"></i> Generating...'
                : '<i class="fa-solid fa-bolt"></i> Generate Playlist'
            }
          </button>
        </div>
      </div>

      <div id="daily-result">${renderResult()}</div>

      <div class="card daily-history">
        <h3><i class="fa-solid fa-history"></i> History</h3>
        <div id="daily-history-list">${renderHistory()}</div>
      </div>
    </div>
  `;
}

/* ── Form rows ──────────────────────────────────────────────────── */

function renderSourceTagsRow() {
  return `
    <div class="daily-form-row">
      <label class="daily-label">Source Tags</label>
      <div class="daily-tag-input-row">
        <div class="typeahead-wrap" style="position:relative;flex:1">
          <input
            type="text"
            id="daily-tag-search"
            class="input-text"
            placeholder="add tag..."
            autocomplete="off"
          />
          <div class="tag-dropdown" id="daily-tag-dropdown" style="display:none"></div>
        </div>
      </div>
      <div class="tag-chips" id="daily-tag-chips">${renderTagChips()}</div>
    </div>
  `;
}

function renderBpmRow() {
  return `
    <div class="daily-form-row">
      <label class="daily-label">BPM Range</label>
      <div class="daily-bpm-row">
        <input type="number" id="daily-bpm-min" class="input-text"
          value="${state.bpmMin}" min="0" max="300" step="1" style="width:80px" />
        <span class="daily-bpm-sep">&ndash;</span>
        <input type="number" id="daily-bpm-max" class="input-text"
          value="${state.bpmMax}" min="0" max="300" step="1" style="width:80px" />
        <div class="daily-bpm-presets">
          <button class="btn btn-sm" data-bpm-min="120" data-bpm-max="130">120&ndash;130</button>
          <button class="btn btn-sm" data-bpm-min="130" data-bpm-max="140">130&ndash;140</button>
          <button class="btn btn-sm" data-bpm-min="140" data-bpm-max="150">140&ndash;150</button>
          <button class="btn btn-sm" data-bpm-min="145" data-bpm-max="155">145&ndash;155</button>
          <button class="btn btn-sm" data-bpm-min="150" data-bpm-max="160">150&ndash;160</button>
        </div>
      </div>
    </div>
  `;
}

function renderLimitRow() {
  return `
    <div class="daily-form-row">
      <label class="daily-label">Tracks per batch</label>
      <input type="number" id="daily-limit" class="input-text"
        value="${state.limit}" min="5" max="50" step="5" style="width:80px" />
    </div>
  `;
}

function renderExcludeRow() {
  return `
    <div class="daily-form-row">
      <label class="checkbox-label">
        <input type="checkbox" id="daily-exclude-tagged" ${state.excludeFullyTagged ? "checked" : ""} />
        Exclude already fully tagged (has P+M+V tags)
      </label>
    </div>
  `;
}

function renderTagChips() {
  if (state.selectedTags.length === 0) return "";
  return state.selectedTags
    .map(
      (t) =>
        `<span class="tag-chip" data-daily-tag="${escapeHtml(t.name)}">${escapeHtml(t.name)}<span class="tag-chip-x">&times;</span></span>`,
    )
    .join("");
}

/* ── Result ─────────────────────────────────────────────────────── */

function spotifyStatusLabel(status) {
  switch (status) {
    case "not_configured":
      return "(Spotify not configured)";
    case "no_tracks":
      return "(No tracks to push)";
    case "failed":
      return "(Spotify push failed — check server logs)";
    default:
      return "(Spotify push skipped)";
  }
}

function renderResult() {
  if (!state.result) return "";
  const r = state.result;
  const spotifyBtn = r.spotifyUrl
    ? `<a href="${r.spotifyUrl}" target="_blank" rel="noopener"
	          class="btn btn-sm daily-spotify-btn">
	        <i class="fa-brands fa-spotify"></i> Open in Spotify</a>`
    : `<span class="text-muted">${spotifyStatusLabel(r.spotifyPushStatus)}</span>`;
  const bpmStr =
    state.bpmMin > 0 || state.bpmMax < 999
      ? ` &middot; ${state.bpmMin}&ndash;${state.bpmMax} BPM`
      : "";

  return `
    <div class="card daily-result-card">
      <h4><i class="fa-solid fa-check-circle" style="color:var(--green)"></i> ${escapeHtml(r.playlistName)}</h4>
      <p>${r.trackCount} tracks${bpmStr}</p>
      <div class="daily-result-actions">${spotifyBtn}</div>
    </div>
  `;
}

/* ── History ────────────────────────────────────────────────────── */

function renderHistory() {
  if (state.history.length === 0) {
    return '<p class="text-muted">No playlists generated yet.</p>';
  }
  return state.history
    .map(
      (h) => `
    <div class="daily-history-item">
      <span class="daily-history-name">${escapeHtml(h.playlistName)}</span>
      <span class="daily-history-count">${h.trackCount} tracks</span>
      ${
        h.spotifyUrl
          ? `<a href="${h.spotifyUrl}" target="_blank" rel="noopener"
              class="btn btn-xs daily-spotify-btn" title="Open in Spotify">
            <i class="fa-brands fa-spotify"></i></a>`
          : ""
      }
      <span class="daily-history-date">${formatDate(h.generatedAt)}</span>
    </div>`,
    )
    .join("");
}

function formatDate(epochSecs) {
  if (!epochSecs) return "";
  const d = new Date(epochSecs * 1000);
  const now = new Date();
  const yesterday = new Date(now);
  yesterday.setDate(yesterday.getDate() - 1);

  if (d.toDateString() === now.toDateString()) return "Today";
  if (d.toDateString() === yesterday.toDateString()) return "Yesterday";
  return d.toLocaleDateString();
}

/* ------------------------------------------------------------------ */
/*  Event Wiring                                                       */
/* ------------------------------------------------------------------ */

function wireEvents() {
  wireTagTypeahead();
  wireChipRemoval();
  wireBpmPresets();
  wireBpmInputs();
  wireLimitInput();
  wireExcludeToggle();
  wireGenerateButton();
}

/* ── Tag typeahead ──────────────────────────────────────────────── */

function wireTagTypeahead() {
  const search = _container.querySelector("#daily-tag-search");
  const dropdown = _container.querySelector("#daily-tag-dropdown");
  let timer = null;

  search.addEventListener("input", () => {
    clearTimeout(timer);
    const q = search.value.trim();
    if (!q) {
      dropdown.style.display = "none";
      return;
    }
    timer = setTimeout(async () => {
      try {
        const data = await fetchJSON(
          `/api/tags?search=${encodeURIComponent(q)}&page_size=10`,
        );
        const tags = data.data || [];
        if (tags.length === 0) {
          dropdown.style.display = "none";
          return;
        }
        dropdown.innerHTML = tags
          .map(
            (t) =>
              `<div class="tag-dropdown-item" data-tag-id="${t.id}" data-tag-name="${escapeHtml(t.name)}">
                ${escapeHtml(t.name)}
                <span class="text-muted">${escapeHtml(t.categoryName || "")}</span>
              </div>`,
          )
          .join("");
        dropdown.style.display = "block";
      } catch {
        /* typeahead failure is non-critical */
      }
    }, 300);
  });

  // Select tag from dropdown
  dropdown.addEventListener("click", (e) => {
    const item = e.target.closest(".tag-dropdown-item");
    if (!item) return;
    const name = item.dataset.tagName;
    const id = parseInt(item.dataset.tagId, 10);
    if (!state.selectedTags.find((t) => t.id === id)) {
      state.selectedTags.push({ id, name });
      refreshTagChips();
    }
    search.value = "";
    dropdown.style.display = "none";
  });

  // Close dropdown on outside click
  document.addEventListener("click", (e) => {
    if (!_container.contains(e.target)) {
      dropdown.style.display = "none";
    }
  });
}

/* ── Chip removal ───────────────────────────────────────────────── */

function wireChipRemoval() {
  _container.addEventListener("click", (e) => {
    const chip = e.target.closest(".tag-chip");
    if (!chip) return;
    const xBtn = e.target.closest(".tag-chip-x");
    if (!xBtn) return;
    const name = chip.dataset.dailyTag;
    state.selectedTags = state.selectedTags.filter((t) => t.name !== name);
    refreshTagChips();
  });
}

function refreshTagChips() {
  const chips = _container.querySelector("#daily-tag-chips");
  if (chips) chips.innerHTML = renderTagChips();
}

/* ── BPM presets ────────────────────────────────────────────────── */

function wireBpmPresets() {
  _container.querySelectorAll("[data-bpm-min]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const min = parseInt(btn.dataset.bpmMin, 10);
      const max = parseInt(btn.dataset.bpmMax, 10);
      state.bpmMin = min;
      state.bpmMax = max;
      _container.querySelector("#daily-bpm-min").value = min;
      _container.querySelector("#daily-bpm-max").value = max;
    });
  });
}

/* ── BPM inputs ─────────────────────────────────────────────────── */

function wireBpmInputs() {
  _container.querySelector("#daily-bpm-min").addEventListener("change", (e) => {
    state.bpmMin = parseInt(e.target.value, 10) || 0;
  });
  _container.querySelector("#daily-bpm-max").addEventListener("change", (e) => {
    state.bpmMax = parseInt(e.target.value, 10) || 999;
  });
}

/* ── Limit ──────────────────────────────────────────────────────── */

function wireLimitInput() {
  _container.querySelector("#daily-limit").addEventListener("change", (e) => {
    state.limit = parseInt(e.target.value, 10) || 20;
  });
}

/* ── Exclude toggle ─────────────────────────────────────────────── */

function wireExcludeToggle() {
  _container.querySelector("#daily-exclude-tagged").addEventListener("change", (e) => {
    state.excludeFullyTagged = e.target.checked;
  });
}

/* ── Generate button ────────────────────────────────────────────── */

function wireGenerateButton() {
  _container.querySelector("#daily-generate").addEventListener("click", async () => {
    if (state.selectedTags.length === 0) {
      showToast("Please add at least one source tag", "warning");
      return;
    }

    state.loading = true;
    refreshGenerateButton();

    try {
      const body = {
        tags: state.selectedTags.map((t) => t.name),
        bpmMin: state.bpmMin,
        bpmMax: state.bpmMax,
        limit: state.limit,
        excludeFullyTagged: state.excludeFullyTagged,
      };
      const resp = await fetchJSON("/api/daily/generate", {
        method: "POST",
        body: JSON.stringify(body),
      });

      state.result = resp.data;
      state.history.unshift({
        playlistName: resp.data.playlistName,
        trackCount: resp.data.trackCount,
        spotifyUrl: resp.data.spotifyUrl,
        generatedAt: Math.floor(Date.now() / 1000),
      });
      saveHistory();
      refreshResultAndHistory();

      if (resp.data.trackCount === 0) {
        showToast("No tracks match your criteria", "warning");
      } else {
        const name = resp.data.playlistName;
        const count = resp.data.trackCount;
        showToast(`Generated "${name}" with ${count} tracks`, "success");
      }
    } catch (e) {
      showToast(e.message || "Generation failed", "error");
    } finally {
      state.loading = false;
      refreshGenerateButton();
    }
  });
}

/* ── Micro-refresh helpers ──────────────────────────────────────── */

function refreshGenerateButton() {
  const btn = _container.querySelector("#daily-generate");
  if (!btn) return;
  btn.disabled = state.loading;
  btn.innerHTML = state.loading
    ? '<i class="fa-solid fa-spinner fa-spin"></i> Generating...'
    : '<i class="fa-solid fa-bolt"></i> Generate Playlist';
}

function refreshResultAndHistory() {
  const resultEl = _container.querySelector("#daily-result");
  if (resultEl) resultEl.innerHTML = renderResult();

  const historyEl = _container.querySelector("#daily-history-list");
  if (historyEl) historyEl.innerHTML = renderHistory();
}
