/**
 * digging.js — Digging Curator page.
 *
 * Split-view: left panel for unified tag/file search + staging,
 * right panel for scored & ranked suggestions with embedded audio players.
 *
 * Flow: Search → click tag (adds all its files to staging)
 *                    or click file (adds just that file)
 *        → Refine (searches suggestions from staging seeds)
 *        → Save as Playlist (persists staging)
 */

import { fetchJSON } from "../shared/api.js";
import { escapeHtml, showToast } from "../shared/components.js";

const ALL_CAMELOT_KEYS = [
  "1m",
  "2m",
  "3m",
  "4m",
  "5m",
  "6m",
  "7m",
  "8m",
  "9m",
  "10m",
  "11m",
  "12m",
  "1d",
  "2d",
  "3d",
  "4d",
  "5d",
  "6d",
  "7d",
  "8d",
  "9d",
  "10d",
  "11d",
  "12d",
];

const state = {
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
  staging: [], // DiggingSuggestion[] — accumulated tracks
  showSaveDialog: false,
  playlistName: "",
  preferTagRichness: false,
};

// Audio waveform cache (fileId -> Float32Array of 200 normalized peaks)
const waveformCache = new Map();
let progressInterval = null;

// Shared AudioContext — creating one per file hits browser limits (~6)
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

/* ================================================================== */
/*  Page initialisation                                                */
/* ================================================================== */

export async function init(container, _signal, _hashParams) {
  loadConfig();
  renderLayout(container);
  wireEvents(container);
  // Preload any waveforms from cached state
  requestAnimationFrame(() => preloadWaveforms());
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
              <i class="fas fa-search"></i>
              <input
                type="text"
                class="input-text input-search"
                id="digging-tag-search"
                placeholder="search tags & tracks..."
                autocomplete="off"
              />
              <div class="tag-dropdown" id="digging-tag-dropdown"></div>
            </div>
          </div>
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
          <div class="config-row">
            <label class="checkbox-label">
              <input type="checkbox" id="tag-richness-toggle" />
              <span>Prefer well-tagged tracks (Phase + Mood + Vibe)</span>
            </label>
          </div>
        </div>

        <div class="digging-result-stats" id="digging-result-stats"></div>
        <div id="digging-staging-area"></div>
      </div>

      <div class="digging-suggestions">
        <div class="digging-loading hidden" id="digging-loading">
          <i class="fas fa-spinner fa-spin"></i> Loading suggestions...
        </div>
        <div id="digging-empty-state">
          <p class="empty-text">Search for a tag or track to add to staging, then click Refine</p>
        </div>
        <div id="digging-suggestions-list" class="hidden"></div>
      </div>
    </div>
  `;

  renderJumpToggles();
  renderStaging();
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
          ${s.genre ? `<span class="badge badge-genre">${escapeHtml(s.genre)}</span>` : ""}
          ${s.fileType ? `<span class="badge badge-filetype">${s.fileType}</span>` : ""}
        </div>
        <div class="sugg-tags">
          ${shared.map((t) => `<span class="tag-chip tag-chip-sm">${escapeHtml(t)}</span>`).join("")}
        </div>
        <div class="audio-player" data-file-id="${s.fileId}">
          <button class="btn-play" data-file-id="${s.fileId}"><i class="fas fa-play"></i></button>
          <div class="waveform-wrap" data-file-id="${s.fileId}">
            <canvas class="waveform-canvas" data-file-id="${s.fileId}" width="200" height="40"></canvas>
            <div class="waveform-progress" data-file-id="${s.fileId}"></div>
          </div>
          <span class="time-display" data-file-id="${s.fileId}">0:00 / ${formatTime(s.durationMs ? Math.floor(s.durationMs / 1000) : 0)}</span>
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
            tag:${sb.tagMatchBonus?.toFixed(0) ?? "0"}
            rich:${sb.tagRichnessBonus?.toFixed(0) ?? "0"}
            ovrl:${sb.categoryOverlapBonus?.toFixed(0) ?? "0"}
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

/* ================================================================== */
/*  Staging area                                                        */
/* ================================================================== */

function getStagingKeys() {
  const keys = state.staging.filter((s) => s.musicalKey).map((s) => s.musicalKey);
  return [...new Set(keys)].sort();
}

function renderStaging() {
  const container = document.getElementById("digging-staging-area");
  if (!container) return;

  // Always render — even when empty, show the empty state
  if (!state.staging.length) {
    container.innerHTML = `
      <div class="staging-section staging-empty">
        <div class="staging-header">
          <h3><i class="fas fa-layer-group"></i> STAGING (0)</h3>
        </div>
        <p class="empty-text">No tracks yet. Search for a tag or track above.</p>
      </div>
    `;
    return;
  }

  const coveredKeys = getStagingKeys();
  const coverageHtml = ALL_CAMELOT_KEYS.map((k) => {
    const covered = coveredKeys.includes(k);
    return `<span class="camelot-dot ${covered ? "covered" : "missing"}" title="${k}">${k}</span>`;
  }).join("");

  container.innerHTML = `
    <div class="staging-section">
      <div class="staging-header">
        <h3><i class="fas fa-layer-group"></i> STAGING (${state.staging.length})</h3>
        <div class="staging-coverage">${coverageHtml}</div>
      </div>
      ${state.staging
        .map(
          (s) => `
        <div class="seed-card">
          <div class="seed-title">${escapeHtml(s.title)}</div>
          <div class="seed-artist">${escapeHtml(s.artist)}</div>
          <div class="seed-badges">
            ${s.bpm ? `<span class="badge">${s.bpm} BPM</span>` : ""}
            ${s.musicalKey ? `<span class="badge badge-key">${escapeHtml(s.musicalKey)}</span>` : ""}
            ${s.playCount > 0 ? `<span class="badge">&#11088;${s.playCount}</span>` : '<span class="badge badge-fresh">new</span>'}
            ${s.genre ? `<span class="badge badge-genre">${escapeHtml(s.genre)}</span>` : ""}
            ${s.fileType ? `<span class="badge badge-filetype">${s.fileType}</span>` : ""}
          </div>
          <div class="audio-player" data-file-id="${s.fileId}">
            <button class="btn-play btn-play-sm" data-file-id="${s.fileId}"><i class="fas fa-play"></i></button>
            <div class="waveform-wrap" data-file-id="${s.fileId}">
              <canvas class="waveform-canvas" data-file-id="${s.fileId}" width="150" height="30"></canvas>
              <div class="waveform-progress" data-file-id="${s.fileId}"></div>
            </div>
            <span class="time-display" data-file-id="${s.fileId}">0:00</span>
            <audio class="audio-el" data-file-id="${s.fileId}" preload="none">
              <source src="/api/files/${s.fileId}/stream" />
            </audio>
          </div>
          <button class="btn btn-sm btn-remove" data-action="remove-staging" data-file-id="${s.fileId}">
            <i class="fas fa-times"></i> remove
          </button>
          ${
            s.sharedTags && s.sharedTags.length
              ? `<div class="seed-tags">${s.sharedTags.map((t) => `<span class="tag-chip tag-chip-sm">${escapeHtml(t)}</span>`).join("")}</div>`
              : ""
          }
        </div>
      `,
        )
        .join("")}
      <div class="staging-actions">
        <button class="btn btn-primary" id="digging-refine">
          <i class="fas fa-search"></i> Refine (${state.staging.length})
        </button>
        ${
          state.showSaveDialog
            ? `
          <div class="staging-save">
            <input type="text" class="input-text" id="staging-playlist-name"
              placeholder="playlist name..." value="${escapeHtml(state.playlistName)}" />
            <button class="btn btn-primary" id="staging-save-confirm">
              <i class="fas fa-save"></i> Save
            </button>
            <button class="btn btn-secondary" id="staging-save-cancel">
              <i class="fas fa-times"></i>
            </button>
          </div>
        `
            : `
          <button class="btn btn-secondary" id="staging-save-show">
            <i class="fas fa-save"></i> Save as Playlist
          </button>
        `
        }
      </div>
    </div>
  `;

  // Preload waveforms after DOM update
  requestAnimationFrame(() => preloadWaveforms());
}

function updateStats() {
  const stats = document.getElementById("digging-result-stats");
  if (!stats) return;
  if (!state.suggestions.length) {
    stats.innerHTML = "";
    return;
  }
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
  // --- Unified search: tags AND files ---
  const searchInput = document.getElementById("digging-tag-search");
  const searchDropdown = document.getElementById("digging-tag-dropdown");
  let debounceTimer;

  searchInput.addEventListener("input", () => {
    clearTimeout(debounceTimer);
    const q = searchInput.value.trim();
    if (q.length < 1) {
      searchDropdown.classList.remove("open");
      return;
    }
    debounceTimer = setTimeout(async () => {
      try {
        const [tagRes, fileRes] = await Promise.all([
          fetchJSON(`/api/tags?search=${encodeURIComponent(q)}&page_size=8`),
          fetchJSON(
            `/api/files?search=${encodeURIComponent(q)}&page_size=8&bpmNotEmpty=true&keyNotEmpty=true`,
          ),
        ]);

        const tags = tagRes?.data || [];
        const files = fileRes?.data || [];

        const tagItems = tags.map((t) => ({
          type: "tag",
          id: t.id,
          name: t.name,
          label: `🏷️ ${escapeHtml(t.name)} (${t.fileCount || "?"} files)`,
        }));

        const fileItems = files.map((f) => ({
          type: "file",
          id: f.id,
          name: `${f.title} — ${f.artist || "?"}`,
          label: `📁 ${escapeHtml(f.title)} — ${escapeHtml(f.artist || "?")} · ${f.bpm ? f.bpm + "BPM" : ""} ${f.musicalKey || ""}`,
          file: f,
        }));

        const items = [...tagItems, ...fileItems];

        if (items.length) {
          searchDropdown.innerHTML = items
            .map(
              (item) =>
                `<div class="tag-dropdown-item" data-type="${item.type}" data-id="${item.id}" data-name="${escapeHtml(item.name)}">${item.label}</div>`,
            )
            .join("");
          searchDropdown.classList.add("open");
        } else {
          searchDropdown.classList.remove("open");
        }
      } catch {
        searchDropdown.classList.remove("open");
      }
    }, 200);
  });

  // --- Select dropdown item ---
  searchDropdown.addEventListener("click", async (e) => {
    const item = e.target.closest(".tag-dropdown-item");
    if (!item) return;
    const itemType = item.dataset.type;
    const itemId = +item.dataset.id;
    const itemName = item.dataset.name;

    searchInput.value = "";
    searchDropdown.classList.remove("open");

    if (itemType === "tag") {
      await addTagToStaging(itemName, container);
    } else if (itemType === "file") {
      await addFileToStaging(itemId, container);
    }
  });

  // --- Keyboard: Enter to select first result ---
  searchInput.addEventListener("keydown", async (e) => {
    if (e.key === "Enter") {
      const firstItem = searchDropdown.querySelector(".tag-dropdown-item");
      if (firstItem && searchDropdown.classList.contains("open")) {
        const itemType = firstItem.dataset.type;
        const itemId = +firstItem.dataset.id;
        const itemName = firstItem.dataset.name;

        searchInput.value = "";
        searchDropdown.classList.remove("open");

        if (itemType === "tag") {
          await addTagToStaging(itemName, container);
        } else if (itemType === "file") {
          await addFileToStaging(itemId, container);
        }
      }
    }
  });

  // Close dropdown on outside click
  container.addEventListener("click", (e) => {
    if (!e.target.closest(".typeahead-wrap")) {
      searchDropdown.classList.remove("open");
    }
  });

  // --- BPM range slider ---
  const slider = document.getElementById("bpm-range-slider");
  const bpmValue = document.getElementById("bpm-range-value");
  slider.addEventListener("input", () => {
    state.bpmRange = +slider.value;
    bpmValue.textContent = state.bpmRange;
    saveConfig();
    if (state.staging.length > 0) doSearch(container);
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
    if (state.staging.length > 0) doSearch(container);
  });

  // --- Tag richness toggle ---
  const richnessToggle = document.getElementById("tag-richness-toggle");
  if (richnessToggle) {
    richnessToggle.checked = state.preferTagRichness;
    richnessToggle.addEventListener("change", (e) => {
      state.preferTagRichness = e.target.checked;
      saveConfig();
      if (state.staging.length > 0) doSearch(container);
    });
  }

  // --- Delegate: Refine, Load More, Add-to-Staging, Remove-from-Staging, Save dialog ---
  container.addEventListener("click", async (e) => {
    const target = e.target;

    // Refine button (in staging area)
    if (target.id === "digging-refine" || target.closest("#digging-refine")) {
      await doSearch(container);
      return;
    }

    // Load More
    if (target.id === "digging-load-more" || target.closest("#digging-load-more")) {
      state.limit += 10;
      saveConfig();
      await doSearch(container);
      return;
    }

    // Add to Staging (moves suggestion from suggestions list to staging)
    const addBtn = target.closest('[data-action="add-to-tag"]');
    if (addBtn) {
      const fileId = +addBtn.dataset.fileId;
      const suggestion = state.suggestions.find((s) => s.fileId === fileId);
      if (suggestion) {
        // Skip if already in staging
        if (state.staging.some((s) => s.fileId === fileId)) return;
        state.staging.push(suggestion);
        state.suggestions = state.suggestions.filter((s) => s.fileId !== fileId);
        renderStaging();
        renderSuggestions();
        setupAudioPlayers();
        wireWaveformSeek();
      }
      return;
    }

    // Remove from staging
    const removeBtn = target.closest('[data-action="remove-staging"]');
    if (removeBtn) {
      const fileId = +removeBtn.dataset.fileId;
      const removed = state.staging.find((s) => s.fileId === fileId);
      state.staging = state.staging.filter((s) => s.fileId !== fileId);
      if (removed) {
        // Return to suggestions list (at top)
        state.suggestions.unshift(removed);
      }
      renderStaging();
      renderSuggestions();
      setupAudioPlayers();
      wireWaveformSeek();
      return;
    }

    // Save dialog: show input
    if (e.target.id === "staging-save-show") {
      state.showSaveDialog = true;
      if (!state.playlistName) {
        // Default playlist name: first tag name from staging files
        state.playlistName = "digging-" + new Date().toISOString().slice(0, 10);
      }
      renderStaging();
      setupAudioPlayers();
      wireWaveformSeek();
      return;
    }

    // Save dialog: cancel
    if (e.target.id === "staging-save-cancel") {
      state.showSaveDialog = false;
      state.playlistName = "";
      renderStaging();
      setupAudioPlayers();
      wireWaveformSeek();
      return;
    }

    // Save dialog: confirm
    if (e.target.id === "staging-save-confirm") {
      saveStagingAsPlaylist();
      return;
    }
  });

  // --- Staging: Playlist name input ---
  container.addEventListener("input", (e) => {
    if (e.target.id === "staging-playlist-name") {
      state.playlistName = e.target.value;
    }
  });
}

/* ================================================================== */
/*  Adding to staging                                                   */
/* ================================================================== */

/**
 * Fetch all files for a tag and add them to staging (deduplicating by fileId).
 */
async function addTagToStaging(tagName, container) {
  try {
    const response = await fetchJSON("/api/digging/suggest", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        seedTag: tagName,
        limit: 1,
      }),
    });
    const data = response.data || response;
    const seeds = (data.seeds || []).map((s) => ({ ...s, fileId: s.id }));

    if (!seeds.length) {
      showToast(`Tag "${tagName}" has no files with BPM+Key`, "error");
      return;
    }

    // Add seeds to staging, skipping duplicates
    for (const seed of seeds) {
      if (!state.staging.some((s) => s.fileId === seed.fileId)) {
        state.staging.push(seed);
      }
    }

    renderStaging();
    setupAudioPlayers();
    wireWaveformSeek();
    showToast(`Added ${seeds.length} tracks from "${tagName}" to staging`, "success");

    // Auto-search since user picked a tag
    await doSearch(container);
  } catch (err) {
    showToast("Failed to load tag files: " + err.message, "error");
  }
}

/**
 * Add a single file to staging by fetching its details.
 */
async function addFileToStaging(fileId, container) {
  // Skip if already in staging
  if (state.staging.some((s) => s.fileId === fileId)) {
    showToast("Track already in staging", "info");
    return;
  }

  try {
    // Use the digging/suggest endpoint to resolve the file as a seed
    const suggestRes = await fetchJSON("/api/digging/suggest", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        seedFileIds: [fileId],
        limit: 1,
      }),
    });
    const data = suggestRes.data || suggestRes;
    const seeds = (data.seeds || []).map((s) => ({ ...s, fileId: s.id }));

    if (!seeds.length) {
      showToast("File not found or missing BPM/Key", "error");
      return;
    }

    const seed = seeds[0];
    state.staging.push(seed);

    renderStaging();
    setupAudioPlayers();
    wireWaveformSeek();
    showToast(`Added "${seed.title}" to staging`, "success");

    // Auto-search
    await doSearch(container);
  } catch (err) {
    showToast("Failed to add file: " + err.message, "error");
  }
}

/* ================================================================== */
/*  Audio player                                                       */
/* ================================================================== */

function setupAudioPlayers() {
  document.querySelectorAll(".btn-play").forEach((btn) => {
    // Remove old listener by cloning
    const newBtn = btn.cloneNode(true);
    btn.parentNode.replaceChild(newBtn, btn);

    newBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      const fileId = +newBtn.dataset.fileId;
      const audio = document.querySelector(`audio[data-file-id="${fileId}"]`);
      if (!audio) return;

      // Stop currently playing audio
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
        newBtn.innerHTML = '<i class="fas fa-pause"></i>';
        state.activeAudio = audio;

        // Draw waveform from cache (preloaded by preloadWaveforms)
        if (waveformCache.has(String(fileId))) {
          drawWaveform(fileId, waveformCache.get(String(fileId)), 0);
        }

        // Start progress updates
        if (progressInterval) clearInterval(progressInterval);
        progressInterval = setInterval(() => updateProgress(fileId), 100);
      } else {
        audio.pause();
        newBtn.innerHTML = '<i class="fas fa-play"></i>';
        state.activeAudio = null;
        if (progressInterval) clearInterval(progressInterval);
      }

      audio.onended = () => {
        newBtn.innerHTML = '<i class="fas fa-play"></i>';
        state.activeAudio = null;
        if (progressInterval) clearInterval(progressInterval);
        // Reset waveform
        const peaks = waveformCache.get(String(fileId));
        if (peaks) drawWaveform(fileId, peaks, 0);
        // Reset time display
        const td = document.querySelector(`.time-display[data-file-id="${fileId}"]`);
        if (td) {
          td.textContent = `0:00 / ${formatTime(audio.duration)}`;
        }
      };
    });
  });
}

async function loadWaveform(fileId) {
  // Skip if already cached
  if (waveformCache.has(String(fileId))) {
    drawWaveform(fileId, waveformCache.get(String(fileId)), 0);
    return;
  }

  try {
    const canvas = document.querySelector(`.waveform-canvas[data-file-id="${fileId}"]`);
    if (!canvas) return;

    // Fetch the full audio file
    const res = await fetch(`/api/files/${fileId}/stream`);
    const arrayBuffer = await res.arrayBuffer();

    // Decode via Web Audio API (shared context — avoids browser limit of ~6)
    const ctx = getAudioContext();
    const audioBuffer = await ctx.decodeAudioData(arrayBuffer);

    // Get PCM data from first channel, downsample to ~200 peaks
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

    // Normalize to 0..1
    let maxPeak = 0;
    for (let i = 0; i < samples; i++) {
      if (peaks[i] > maxPeak) maxPeak = peaks[i];
    }
    if (maxPeak > 0) {
      for (let i = 0; i < samples; i++) peaks[i] /= maxPeak;
    }

    waveformCache.set(String(fileId), peaks);
    drawWaveform(fileId, peaks, 0);
  } catch (err) {
    console.warn("Waveform failed for", fileId, err);
    // Draw fallback: flat bars so the UI isn't blank
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
  const mutedColor = style.getPropertyValue("--muted").trim() || "#999";

  // Draw uniform low bars to show "waveform unavailable"
  const barCount = 40;
  const barWidth = w / barCount;
  for (let i = 0; i < barCount; i++) {
    const barHeight = (Math.random() * 0.3 + 0.1) * h; // 10-40% height
    const x = i * barWidth;
    const y = (h - barHeight) / 2;
    ctx.fillStyle = mutedColor;
    ctx.globalAlpha = 0.3;
    ctx.fillRect(x, y, barWidth - 1, barHeight);
  }
  ctx.globalAlpha = 1.0;
}

function drawWaveform(fileId, peaks, progress) {
  const canvas = document.querySelector(`.waveform-canvas[data-file-id="${fileId}"]`);
  if (!canvas || !peaks) return;
  const ctx = canvas.getContext("2d");
  const w = canvas.width;
  const h = canvas.height;

  ctx.clearRect(0, 0, w, h);

  const barCount = peaks.length;
  const barWidth = w / barCount;
  const gap = 1;
  const drawWidth = Math.max(1, barWidth - gap);
  const progressIndex = Math.floor(progress * barCount);

  // Get computed style colors
  const style = getComputedStyle(document.body);
  const primaryColor = style.getPropertyValue("--primary").trim() || "#4a9eff";
  const mutedColor = style.getPropertyValue("--muted").trim() || "#666";

  for (let i = 0; i < barCount; i++) {
    const x = i * barWidth;
    const barHeight = Math.max(2, peaks[i] * h * 0.9);
    const y = (h - barHeight) / 2;

    ctx.fillStyle = i <= progressIndex ? primaryColor : mutedColor;
    ctx.fillRect(x, y, drawWidth, barHeight);
  }
}

function updateProgress(fileId) {
  const audio = document.querySelector(`audio[data-file-id="${fileId}"]`);
  if (!audio || !audio.duration || audio.paused) return;

  const progress = audio.currentTime / audio.duration;

  // Update progress overlay
  const overlay = document.querySelector(`.waveform-progress[data-file-id="${fileId}"]`);
  if (overlay) overlay.style.width = `${progress * 100}%`;

  // Update time display
  const td = document.querySelector(`.time-display[data-file-id="${fileId}"]`);
  if (td) {
    td.textContent = `${formatTime(audio.currentTime)} / ${formatTime(audio.duration)}`;
  }

  // Redraw waveform with progress highlight
  const peaks = waveformCache.get(String(fileId));
  if (peaks) drawWaveform(fileId, peaks, progress);
}

function wireWaveformSeek() {
  document.querySelectorAll(".waveform-wrap").forEach((wrap) => {
    wrap.addEventListener("click", (e) => {
      // Don't seek if clicking the play button inside the wrap
      if (e.target.closest(".btn-play")) return;

      const fileId = +wrap.dataset.fileId;
      const audio = document.querySelector(`audio[data-file-id="${fileId}"]`);
      if (!audio || !audio.duration) return;

      const rect = wrap.getBoundingClientRect();
      const x = e.clientX - rect.left;
      const ratio = Math.max(0, Math.min(1, x / rect.width));
      audio.currentTime = ratio * audio.duration;
    });
  });
}

/* ================================================================== */
/*  API calls                                                          */
/* ================================================================== */

function buildRequest() {
  if (!state.staging.length) return null;

  const activeJumps = Object.entries(state.camelotJumps)
    .filter(([, active]) => active)
    .map(([jump]) => jump);

  const body = {
    seedFileIds: state.staging.map((s) => s.fileId),
    bpmRange: state.bpmRange,
    limit: state.limit,
    camelotJumps: activeJumps,
  };

  if (state.preferTagRichness) {
    body.preferTagRichness = true;
  }

  return body;
}

async function saveStagingAsPlaylist() {
  const name = state.playlistName.trim();
  if (!name) {
    showToast("Please enter a playlist name", "error");
    document.getElementById("staging-playlist-name")?.focus();
    return;
  }

  const fileIds = state.staging.map((s) => s.fileId);
  try {
    await fetchJSON("/api/playlists/local", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name, fileIds }),
    });
    showToast(`Playlist "${name}" created with ${fileIds.length} tracks`, "success");
    state.staging = [];
    state.showSaveDialog = false;
    state.playlistName = "";
    clearSuggestions();
    renderStaging();
    setupAudioPlayers();
    wireWaveformSeek();
  } catch (err) {
    showToast("Failed to save: " + err.message, "error");
  }
}

async function doSearch(container) {
  if (!state.staging.length) return;

  const requestBody = buildRequest();
  if (!requestBody) return;

  state.loading = true;
  const loadingEl = document.getElementById("digging-loading");
  if (loadingEl) loadingEl.classList.remove("hidden");

  try {
    const response = await fetchJSON("/api/digging/suggest", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(requestBody),
    });
    const data = response.data || response;
    state.suggestions = data.suggestions || [];
    state.bpmMin = data.bpmMin;
    state.bpmMax = data.bpmMax;
    state.candidatesConsidered = data.candidatesConsidered || 0;

    renderSuggestions();
    updateStats();
    setupAudioPlayers();
    wireWaveformSeek();
    preloadWaveforms();
  } catch (err) {
    showToast("Error fetching suggestions: " + err.message, "error");
  } finally {
    state.loading = false;
    if (loadingEl) loadingEl.classList.add("hidden");
  }
}

function preloadWaveforms() {
  const canvases = document.querySelectorAll(".waveform-canvas");
  let delay = 0;
  canvases.forEach((canvas) => {
    const fileId = canvas.dataset.fileId;
    if (!fileId || waveformCache.has(String(fileId))) return;
    setTimeout(() => loadWaveform(fileId), delay);
    delay += 100; // stagger 100ms apart to avoid concurrent fetch flood
  });
}

function clearSuggestions() {
  state.suggestions = [];
  state.bpmMin = null;
  state.bpmMax = null;
  state.candidatesConsidered = 0;
  const list = document.getElementById("digging-suggestions-list");
  const empty = document.getElementById("digging-empty-state");
  if (list) {
    list.innerHTML = "";
    list.classList.add("hidden");
  }
  if (empty) empty.classList.remove("hidden");
  const stats = document.getElementById("digging-result-stats");
  if (stats) stats.innerHTML = "";
}

/* ================================================================== */
/*  Helpers                                                            */
/* ================================================================== */

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

function loadConfig() {
  try {
    const saved = JSON.parse(localStorage.getItem("diggingConfig") || "{}");
    if (saved.bpmRange != null) state.bpmRange = saved.bpmRange;
    if (saved.camelotJumps) Object.assign(state.camelotJumps, saved.camelotJumps);
    if (saved.limit != null) state.limit = saved.limit;
    if (saved.preferTagRichness != null)
      state.preferTagRichness = saved.preferTagRichness;
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
        preferTagRichness: state.preferTagRichness,
      }),
    );
  } catch {
    /* ignore */
  }
}
