/**
 * digging.js — Digging Curator page.
 *
 * Split-view: left panel for tag-based seed selection + config,
 * right panel for scored & ranked suggestions with embedded audio players.
 */

import { fetchJSON } from "../shared/api.js";
import { escapeHtml, showToast } from "../shared/components.js";

const state = {
  selectedTag: null, // { id, name }
  seeds: [],
  bpmRange: 8,
  camelotJumps: {
    "+1": true,
    "-1": true,
    "+2": true,
    "-2": true,
    "+7": true,
    "-7": true,
    a_to_b: true,
    same: true,
  },
  limit: 10,
  suggestions: [],
  bpmMin: null,
  bpmMax: null,
  candidatesConsidered: 0,
  loading: false,
  activeAudio: null, // HTMLAudioElement currently playing
};

/* ================================================================== */
/*  Page initialisation                                                */
/* ================================================================== */

export async function init(container, _signal, _hashParams) {
  loadConfig();
  renderLayout(container);
  wireEvents(container);
}

/* ================================================================== */
/*  Render functions                                                   */
/* ================================================================== */

function renderLayout(container) {
  container.innerHTML = `
    <div class="page-header">
      <h1><i class="fas fa-magnifying-glass"></i> DIGGING</h1>
    </div>
    <div class="digging-layout">
      <div class="digging-seeds">
        <div class="digging-tag-select">
          <div class="typeahead-wrap">
            <div class="tag-search-wrap">
              <i class="fas fa-tag"></i>
              <input
                type="text"
                class="input-text input-search"
                id="digging-tag-search"
                placeholder="search tag..."
                autocomplete="off"
              />
              <div class="tag-dropdown hidden" id="digging-tag-dropdown"></div>
            </div>
          </div>
          <div class="tag-chips" id="digging-tag-chips"></div>
          <button class="btn btn-primary" id="digging-find-similar" disabled>
            <i class="fas fa-search"></i> Find Similar
          </button>
        </div>

        <div class="digging-config" id="digging-config">
          <div class="config-row">
            <label>BPM Range: ±<span id="bpm-range-value">${state.bpmRange}</span></label>
            <input type="range" id="bpm-range-slider" min="2" max="20" value="${state.bpmRange}" />
          </div>
          <div class="config-row">
            <label>Camelot Jumps:</label>
            <div class="jump-toggles" id="jump-toggles"></div>
          </div>
        </div>

        <div class="digging-result-stats" id="digging-result-stats"></div>
        <div id="digging-seeds-list"></div>
      </div>

      <div class="digging-suggestions">
        <div class="digging-loading hidden" id="digging-loading">
          <i class="fas fa-spinner fa-spin"></i> Loading suggestions...
        </div>
        <div id="digging-empty-state">
          <p class="empty-text">Select a tag and click "Find Similar"</p>
        </div>
        <div id="digging-suggestions-list" class="hidden"></div>
      </div>
    </div>
  `;

  renderJumpToggles();
  updateTagChip();
}

function renderJumpToggles() {
  const container = document.getElementById("jump-toggles");
  if (!container) return;
  const labels = {
    "+1": "+1",
    "-1": "-1",
    "+2": "+2",
    "-2": "-2",
    "+7": "+7",
    "-7": "-7",
    a_to_b: "A\u2194B",
    same: "same",
  };
  container.innerHTML = Object.entries(state.camelotJumps)
    .map(
      ([jump, active]) =>
        `<button class="jump-toggle ${active ? "active" : ""}" data-jump="${jump}">${labels[jump] || jump}</button>`,
    )
    .join("");
}

function updateTagChip() {
  const chips = document.getElementById("digging-tag-chips");
  const btn = document.getElementById("digging-find-similar");
  if (!chips || !btn) return;
  if (state.selectedTag) {
    chips.innerHTML = `<span class="tag-chip">${escapeHtml(state.selectedTag.name)} <span class="tag-chip-x" data-action="remove-tag">&times;</span></span>`;
    btn.disabled = false;
  } else {
    chips.innerHTML = "";
    btn.disabled = true;
  }
}

function renderSeeds() {
  const list = document.getElementById("digging-seeds-list");
  if (!list) return;
  if (!state.seeds.length) {
    list.innerHTML = '<p class="empty-text">No seeds found</p>';
    return;
  }
  list.innerHTML = state.seeds
    .map(
      (s) => `
    <div class="seed-card ${s.excludedAsOutlier ? "outlier" : ""}">
      <div class="seed-title">${escapeHtml(s.title)}</div>
      <div class="seed-artist">${escapeHtml(s.artist)}</div>
      <div class="seed-badges">
        ${s.bpm ? `<span class="badge">${s.bpm} BPM</span>` : ""}
        ${s.musicalKey ? `<span class="badge badge-key">${escapeHtml(s.musicalKey)}</span>` : ""}
        ${s.playCount > 0 ? `<span class="badge">&#11088;${s.playCount}</span>` : '<span class="badge badge-fresh">new</span>'}
      </div>
      <div class="seed-tags">
        ${(s.tags || []).map((t) => `<span class="tag-chip tag-chip-sm">${escapeHtml(t.name)}</span>`).join("")}
      </div>
      ${s.excludedAsOutlier ? '<div class="outlier-warning"><i class="fas fa-triangle-exclamation"></i> BPM outlier \u2014 excluded</div>' : ""}
    </div>
  `,
    )
    .join("");
}

function renderSuggestions() {
  const list = document.getElementById("digging-suggestions-list");
  const empty = document.getElementById("digging-empty-state");
  if (!list || !empty) return;

  if (!state.suggestions.length) {
    empty.classList.remove("hidden");
    list.classList.add("hidden");
    list.innerHTML = "";
    return;
  }

  empty.classList.add("hidden");
  list.classList.remove("hidden");

  const hasMore = state.suggestions.length >= state.limit;

  list.innerHTML = state.suggestions
    .map((s, i) => {
      const sb = s.scoreBreakdown;
      const compatClass = s.camelotCompatibility;
      const shared = s.sharedTags || [];
      return `
    <div class="suggestion-card" data-file-id="${s.fileId}">
      <div class="sugg-rank">#${i + 1}</div>
      <div class="sugg-body">
        <div class="sugg-title">${escapeHtml(s.title)}</div>
        <div class="sugg-artist">${escapeHtml(s.artist)}</div>
        <div class="sugg-badges">
          ${s.bpm ? `<span class="badge">${s.bpm} BPM</span>` : ""}
          ${s.musicalKey ? `<span class="badge badge-key">${escapeHtml(s.musicalKey)}</span>` : ""}
          <span class="badge badge-camelot ${compatClass}">${compatClass}</span>
          ${s.playCount > 0 ? `<span class="badge">&#11088;${s.playCount}</span>` : '<span class="badge badge-fresh">new</span>'}
        </div>
        <div class="sugg-tags">
          ${shared.map((t) => `<span class="tag-chip tag-chip-sm">${escapeHtml(t)}</span>`).join("")}
        </div>
        <div class="sugg-player">
          <button class="btn-play" data-file-id="${s.fileId}"><i class="fas fa-play"></i></button>
          <span class="duration">${formatDuration(s.durationMs)}</span>
          <audio class="audio-el" data-file-id="${s.fileId}" preload="none">
            <source src="/api/files/${s.fileId}/stream" />
          </audio>
        </div>
        <div class="sugg-score">
          <span class="score-total">Score: ${s.score.toFixed(1)}</span>
          <span class="score-detail" title="playCount / recency / bpmDiff / camelot / sharedTags">
            pc:${sb.playCountScore.toFixed(0)}
            rec:${sb.recencyScore.toFixed(0)}
            bpm:${sb.bpmScore.toFixed(0)}
            cam:${sb.camelotBonus.toFixed(0)}
            tag:${sb.tagMatchBonus.toFixed(0)}
          </span>
        </div>
      </div>
      <div class="sugg-actions">
        <button class="btn btn-sm btn-outline" data-action="add-to-tag" data-file-id="${s.fileId}">
          <i class="fas fa-tag"></i> Add
        </button>
      </div>
    </div>
  `;
    })
    .join("");

  if (hasMore) {
    list.insertAdjacentHTML(
      "beforeend",
      '<button class="btn btn-secondary" id="digging-load-more" style="width:100%;margin-top:0.5rem">Load More</button>',
    );
  }
}

function updateStats() {
  const stats = document.getElementById("digging-result-stats");
  if (!stats) return;
  stats.innerHTML = `
    <div class="stats-text">
      ${state.suggestions.length} suggestions from ${state.candidatesConsidered} candidates
      (BPM: ${state.bpmMin}\u2013${state.bpmMax})
    </div>
  `;
}

/* ================================================================== */
/*  Event wiring                                                       */
/* ================================================================== */

function wireEvents(container) {
  // --- Tag typeahead ---
  const tagInput = document.getElementById("digging-tag-search");
  const tagDropdown = document.getElementById("digging-tag-dropdown");
  let debounceTimer;

  tagInput.addEventListener("input", () => {
    clearTimeout(debounceTimer);
    const q = tagInput.value.trim();
    if (q.length < 1) {
      tagDropdown.classList.add("hidden");
      return;
    }
    debounceTimer = setTimeout(async () => {
      try {
        const data = await fetchJSON(
          `/api/tags?search=${encodeURIComponent(q)}&page_size=20`,
        );
        const tags = data?.data?.tags || data?.tags || [];
        if (tags.length) {
          tagDropdown.innerHTML = tags
            .map(
              (t) =>
                `<div class="tag-dropdown-item" data-id="${t.id}" data-name="${escapeHtml(t.name)}">${escapeHtml(t.name)}</div>`,
            )
            .join("");
          tagDropdown.classList.remove("hidden");
        } else {
          tagDropdown.classList.add("hidden");
        }
      } catch {
        tagDropdown.classList.add("hidden");
      }
    }, 200);
  });

  tagDropdown.addEventListener("click", (e) => {
    const item = e.target.closest(".tag-dropdown-item");
    if (!item) return;
    const name = item.dataset.name;
    state.selectedTag = { id: +item.dataset.id, name: name };
    tagInput.value = "";
    tagDropdown.classList.add("hidden");
    updateTagChip();
    clearSuggestions();
  });

  // Close dropdown on outside click
  container.addEventListener("click", (e) => {
    if (!e.target.closest(".typeahead-wrap")) {
      tagDropdown.classList.add("hidden");
    }
  });

  // --- Remove tag chip ---
  const chips = document.getElementById("digging-tag-chips");
  chips.addEventListener("click", (e) => {
    if (e.target.dataset.action === "remove-tag") {
      state.selectedTag = null;
      tagInput.value = "";
      updateTagChip();
      clearSuggestions();
    }
  });

  // --- BPM range slider ---
  const slider = document.getElementById("bpm-range-slider");
  const bpmValue = document.getElementById("bpm-range-value");
  slider.addEventListener("input", () => {
    state.bpmRange = +slider.value;
    bpmValue.textContent = state.bpmRange;
    saveConfig();
    if (state.suggestions.length > 0) doSearch(container);
  });

  // --- Camelot jump toggles ---
  const jumpsContainer = document.getElementById("jump-toggles");
  jumpsContainer.addEventListener("click", (e) => {
    const btn = e.target.closest(".jump-toggle");
    if (!btn) return;
    const jump = btn.dataset.jump;
    state.camelotJumps[jump] = !state.camelotJumps[jump];
    btn.classList.toggle("active");
    saveConfig();
    if (state.suggestions.length > 0) doSearch(container);
  });

  // --- Find Similar ---
  document.getElementById("digging-find-similar").addEventListener("click", () => {
    if (state.selectedTag) doSearch(container);
  });

  // --- Keyboard: Enter in tag search ---
  tagInput.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && state.selectedTag) {
      doSearch(container);
    }
  });

  // --- Delegate: Load More + Add-to-Tag + Play buttons ---
  container.addEventListener("click", (e) => {
    const target = e.target;

    // Load More
    if (target.id === "digging-load-more" || target.closest("#digging-load-more")) {
      state.limit += 10;
      saveConfig();
      doSearch(container);
      return;
    }

    // Add to Tag
    const addBtn = target.closest('[data-action="add-to-tag"]');
    if (addBtn) {
      const fileId = +addBtn.dataset.fileId;
      const suggestion = state.suggestions.find((s) => s.fileId === fileId);
      if (suggestion && state.selectedTag) {
        showToast(
          `Add "${suggestion.title}" to "${state.selectedTag.name}" \u2014 coming soon`,
          "info",
        );
      }
      return;
    }

    // Play button
    const playBtn = target.closest(".btn-play");
    if (playBtn) {
      handlePlayClick(playBtn);
      return;
    }
  });
}

/* ================================================================== */
/*  Audio player                                                       */
/* ================================================================== */

function handlePlayClick(btn) {
  const fileId = btn.dataset.fileId;
  const audio = document.querySelector(`audio[data-file-id="${fileId}"]`);
  if (!audio) return;

  // Stop any currently playing audio
  if (state.activeAudio && state.activeAudio !== audio) {
    state.activeAudio.pause();
    state.activeAudio.currentTime = 0;
    const prevBtn = document.querySelector(
      `.btn-play[data-file-id="${state.activeAudio.dataset.fileId}"]`,
    );
    if (prevBtn) prevBtn.innerHTML = '<i class="fas fa-play"></i>';
  }

  if (audio.paused) {
    audio.play().catch(() => {});
    btn.innerHTML = '<i class="fas fa-pause"></i>';
    state.activeAudio = audio;
  } else {
    audio.pause();
    btn.innerHTML = '<i class="fas fa-play"></i>';
    state.activeAudio = null;
  }

  audio.onended = () => {
    btn.innerHTML = '<i class="fas fa-play"></i>';
    state.activeAudio = null;
  };
}

/* ================================================================== */
/*  API calls                                                          */
/* ================================================================== */

function buildRequest() {
  const activeJumps = Object.entries(state.camelotJumps)
    .filter(([, active]) => active)
    .map(([jump]) => jump);

  return {
    seedTag: state.selectedTag.name,
    bpmRange: state.bpmRange,
    limit: state.limit,
    camelotJumps: activeJumps,
  };
}

async function doSearch(container) {
  if (!state.selectedTag) return;

  state.loading = true;
  document.getElementById("digging-loading").classList.remove("hidden");
  document.getElementById("digging-empty-state").classList.add("hidden");
  document.getElementById("digging-suggestions-list").classList.add("hidden");

  try {
    const response = await fetchJSON("/api/digging/suggest", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(buildRequest()),
    });
    const data = response.data || response;
    state.seeds = data.seeds || [];
    state.suggestions = data.suggestions || [];
    state.bpmMin = data.bpmMin;
    state.bpmMax = data.bpmMax;
    state.candidatesConsidered = data.candidatesConsidered || 0;

    renderSeeds();
    renderSuggestions();
    updateStats();
  } catch (err) {
    showToast("Error fetching suggestions: " + err.message, "error");
  } finally {
    state.loading = false;
    document.getElementById("digging-loading").classList.add("hidden");
  }
}

function clearSuggestions() {
  state.seeds = [];
  state.suggestions = [];
  state.bpmMin = null;
  state.bpmMax = null;
  state.candidatesConsidered = 0;
  document.getElementById("digging-seeds-list").innerHTML = "";
  document.getElementById("digging-suggestions-list").innerHTML = "";
  document.getElementById("digging-suggestions-list").classList.add("hidden");
  document.getElementById("digging-empty-state").classList.remove("hidden");
  document.getElementById("digging-result-stats").innerHTML = "";
}

/* ================================================================== */
/*  Helpers                                                            */
/* ================================================================== */

function formatDuration(ms) {
  if (!ms) return "--:--";
  const totalSec = Math.floor(ms / 1000);
  const min = Math.floor(totalSec / 60);
  const sec = totalSec % 60;
  return `${min}:${sec.toString().padStart(2, "0")}`;
}

function loadConfig() {
  try {
    const saved = JSON.parse(localStorage.getItem("diggingConfig") || "{}");
    if (saved.bpmRange != null) state.bpmRange = saved.bpmRange;
    if (saved.camelotJumps) Object.assign(state.camelotJumps, saved.camelotJumps);
    if (saved.limit != null) state.limit = saved.limit;
  } catch {
    /* ignore */
  }
}

function saveConfig() {
  try {
    localStorage.setItem(
      "diggingConfig",
      JSON.stringify({
        bpmRange: state.bpmRange,
        camelotJumps: state.camelotJumps,
        limit: state.limit,
      }),
    );
  } catch {
    /* ignore */
  }
}
