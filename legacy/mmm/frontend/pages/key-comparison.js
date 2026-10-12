/**
 * key-comparison.js — Spotify vs Traktor Key/BPM Comparison page.
 *
 * Pick a tag, see side-by-side Traktor vs Spotify BPM/Key for all linked files.
 * Summary shows match/mismatch counts. Table is sortable by column.
 *
 * API: GET /api/files/key-comparison?tag=X&limit=500
 */

import { fetchJSON } from "../shared/api.js";
import { showToast } from "../shared/components.js";

/* ------------------------------------------------------------------ */
/*  State                                                              */
/* ------------------------------------------------------------------ */

const state = {
  /** @type {string|null} */
  selectedTag: null,
  /** @type {Array} */
  rows: [],
  /** @type {Object|null} */
  summary: null,
  loading: false,
  /** Current sort column and direction */
  sortCol: null,
  sortDir: "asc",
};

/* ------------------------------------------------------------------ */
/*  Initialization                                                     */
/* ------------------------------------------------------------------ */

/**
 * @param {HTMLElement} container
 * @param {AbortSignal} [signal]
 */
export async function init(container, signal) {
  container.innerHTML = renderLayout();
  wireEvents(container, signal);

  // Try to load a tag from the hash
  const hashTag = getTagFromHash();
  if (hashTag) {
    state.selectedTag = hashTag;
    updateTagInput(container);
    await loadComparison(container, signal);
  }
}

/* ------------------------------------------------------------------ */
/*  Layout                                                             */
/* ------------------------------------------------------------------ */

function renderLayout() {
  return /* html */ `
    <div class="page-header">
      <h1><i class="fa-solid fa-scale-balanced"></i> Key Comparison</h1>
      <span class="page-subtitle">Traktor vs Spotify — BPM &amp; Camelot Key</span>
    </div>

    <div class="kc-controls">
      <div class="kc-tag-search-wrap">
        <i class="fa-solid fa-tag kc-search-icon"></i>
        <input
          type="text"
          class="input-text input-search"
          id="kc-tag-search"
          placeholder="Filter by tag name…"
          autocomplete="off"
        />
        <div class="tag-dropdown" id="kc-tag-dropdown"></div>
      </div>
      <span id="kc-tag-selected" class="kc-tag-pill"></span>
      <button class="btn btn-primary" id="kc-load-btn" disabled>
        <i class="fa-solid fa-magnifying-glass"></i> Compare
      </button>
    </div>

    <div id="kc-summary"></div>
    <div id="kc-table-wrap" class="kc-table-wrap"></div>
  `;
}

/* ------------------------------------------------------------------ */
/*  Events                                                             */
/* ------------------------------------------------------------------ */

function wireEvents(container, signal) {
  // Tag search typeahead
  const tagInput = container.querySelector("#kc-tag-search");
  let debounceTimer = null;
  let dropdownOpen = false;

  tagInput.addEventListener("input", () => {
    clearTimeout(debounceTimer);
    const q = tagInput.value.trim();
    if (q.length < 1) {
      closeDropdown(container);
      return;
    }
    debounceTimer = setTimeout(() => searchTags(container, q, signal), 200);
  });

  tagInput.addEventListener("focus", () => {
    const q = tagInput.value.trim();
    if (q.length >= 1) searchTags(container, q, signal);
  });

  tagInput.addEventListener("keydown", (e) => {
    const items = container.querySelectorAll("#kc-tag-dropdown .tag-dropdown-item");
    if (!items.length) return;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      navigateDropdown(items, 1);
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      navigateDropdown(items, -1);
    }
    if (e.key === "Enter") {
      e.preventDefault();
      const active = container.querySelector(
        "#kc-tag-dropdown .tag-dropdown-item.active",
      );
      if (active) selectTag(container, active.dataset.name, signal);
    }
    if (e.key === "Escape") closeDropdown(container);
  });

  // Click outside closes dropdown
  document.addEventListener("click", (e) => {
    if (!container.contains(e.target)) closeDropdown(container);
  });

  // Load button
  container.querySelector("#kc-load-btn").addEventListener("click", () => {
    loadComparison(container, signal);
  });

  // Clear tag
  container.addEventListener("click", (e) => {
    if (e.target.classList.contains("kc-tag-pill-x")) {
      state.selectedTag = null;
      updateTagInput(container);
      container.querySelector("#kc-load-btn").disabled = true;
      container.querySelector("#kc-summary").innerHTML = "";
      container.querySelector("#kc-table-wrap").innerHTML = "";
    }
  });

  // Sort clicks
  container.addEventListener("click", (e) => {
    const th = e.target.closest("th.sortable");
    if (!th) return;
    const col = th.dataset.sort;
    if (state.sortCol === col) {
      state.sortDir = state.sortDir === "asc" ? "desc" : "asc";
    } else {
      state.sortCol = col;
      state.sortDir = "asc";
    }
    renderTable(container);
  });
}

/* ------------------------------------------------------------------ */
/*  Tag Search                                                         */
/* ------------------------------------------------------------------ */

async function searchTags(container, query, signal) {
  try {
    const data = await fetchJSON(
      `/api/tags?search=${encodeURIComponent(query)}&page_size=10`,
      { signal },
    );
    const tags = data.data || [];
    renderTagDropdown(container, tags);
  } catch (err) {
    if (signal?.aborted) return;
    console.error("Tag search failed:", err);
  }
}

function renderTagDropdown(container, tags) {
  const dropdown = container.querySelector("#kc-tag-dropdown");
  if (!tags.length) {
    dropdown.innerHTML = '<div class="tag-dropdown-empty">No tags found</div>';
    dropdown.classList.add("open");
    return;
  }
  dropdown.innerHTML = tags
    .map(
      (t) =>
        `<div class="tag-dropdown-item" data-name="${escHtml(t.name)}">${escHtml(t.name)}</div>`,
    )
    .join("");
  dropdown.classList.add("open");

  // Click to select
  dropdown.querySelectorAll(".tag-dropdown-item").forEach((item) => {
    item.addEventListener("click", () => selectTag(container, item.dataset.name));
  });
}

function selectTag(container, tagName, signal) {
  state.selectedTag = tagName;
  updateTagInput(container);
  closeDropdown(container);
  container.querySelector("#kc-load-btn").disabled = false;
  loadComparison(container, signal);
}

function updateTagInput(container) {
  const input = container.querySelector("#kc-tag-search");
  const pill = container.querySelector("#kc-tag-selected");
  if (state.selectedTag) {
    input.value = "";
    input.placeholder = "";
    pill.innerHTML = `<span class="tag-chip">${escHtml(state.selectedTag)} <i class="fa-solid fa-xmark kc-tag-pill-x"></i></span>`;
  } else {
    input.placeholder = "Filter by tag name…";
    pill.innerHTML = "";
  }
}

function navigateDropdown(items, dir) {
  let idx = -1;
  items.forEach((item, i) => {
    if (item.classList.contains("active")) idx = i;
  });
  if (idx >= 0) items[idx].classList.remove("active");
  idx = idx + dir;
  if (idx < 0) idx = items.length - 1;
  if (idx >= items.length) idx = 0;
  items[idx].classList.add("active");
  items[idx].scrollIntoView({ block: "nearest" });
}

function closeDropdown(container) {
  const dd = container.querySelector("#kc-tag-dropdown");
  dd.classList.remove("open");
  dd.innerHTML = "";
}

/* ------------------------------------------------------------------ */
/*  Load Comparison                                                    */
/* ------------------------------------------------------------------ */

async function loadComparison(container, signal) {
  if (!state.selectedTag) return;

  state.loading = true;
  container.querySelector("#kc-summary").innerHTML =
    '<div class="kc-loading"><i class="fa-solid fa-spinner fa-spin"></i> Loading comparison…</div>';
  container.querySelector("#kc-table-wrap").innerHTML = "";

  const tag = encodeURIComponent(state.selectedTag);
  try {
    const data = await fetchJSON(`/api/files/key-comparison?tag=${tag}&limit=500`, {
      signal,
    });
    const payload = data.data || data;
    state.rows = payload.files || [];
    state.summary = payload.summary || null;
    renderSummary(container);
    renderTable(container);
  } catch (err) {
    if (signal?.aborted) return;
    showToast(`Failed to load comparison: ${err.message}`, "error");
    container.querySelector("#kc-summary").innerHTML = "";
  } finally {
    state.loading = false;
  }
}

/* ------------------------------------------------------------------ */
/*  Summary                                                            */
/* ------------------------------------------------------------------ */

function renderSummary(container) {
  const s = state.summary;
  if (!s) {
    container.querySelector("#kc-summary").innerHTML = "";
    return;
  }

  const bpmPct =
    s.totalCompared > 0 ? ((s.bpmMatchCount / s.totalCompared) * 100).toFixed(1) : "0";
  const keyPct =
    s.totalCompared > 0 ? ((s.keyMatchCount / s.totalCompared) * 100).toFixed(1) : "0";

  container.querySelector("#kc-summary").innerHTML = /* html */ `
    <div class="kc-summary-cards">
      <div class="kc-summary-card">
        <div class="kc-summary-label">Total Compared</div>
        <div class="kc-summary-value">${s.totalCompared}</div>
      </div>
      <div class="kc-summary-card ${s.bpmMismatchCount === 0 ? "kc-success" : ""}">
        <div class="kc-summary-label">BPM Match</div>
        <div class="kc-summary-value">${s.bpmMatchCount} <span class="kc-pct">(${bpmPct}%)</span></div>
      </div>
      <div class="kc-summary-card ${s.bpmMismatchCount > 0 ? "kc-warn" : ""}">
        <div class="kc-summary-label">BPM Mismatch</div>
        <div class="kc-summary-value">${s.bpmMismatchCount}</div>
      </div>
      <div class="kc-summary-card ${s.keyMismatchCount === 0 ? "kc-success" : ""}">
        <div class="kc-summary-label">Key Match</div>
        <div class="kc-summary-value">${s.keyMatchCount} <span class="kc-pct">(${keyPct}%)</span></div>
      </div>
      <div class="kc-summary-card ${s.keyMismatchCount > 0 ? "kc-warn" : ""}">
        <div class="kc-summary-label">Key Mismatch</div>
        <div class="kc-summary-value">${s.keyMismatchCount}</div>
      </div>
    </div>
  `;
}

/* ------------------------------------------------------------------ */
/*  Table                                                              */
/* ------------------------------------------------------------------ */

function renderTable(container) {
  const wrap = container.querySelector("#kc-table-wrap");
  if (!state.rows.length) {
    wrap.innerHTML = '<div class="kc-empty">No linked files found for this tag.</div>';
    return;
  }

  // Sort
  const sorted = [...state.rows];
  if (state.sortCol) {
    sorted.sort((a, b) => {
      const va = sortVal(a, state.sortCol);
      const vb = sortVal(b, state.sortCol);
      return state.sortDir === "asc" ? cmp(va, vb) : cmp(vb, va);
    });
  }

  const header = (col, label, sortable = true) => {
    const cls = sortable ? "sortable" : "";
    const arrow =
      state.sortCol === col
        ? ` <i class="fa-solid fa-sort-${state.sortDir === "asc" ? "up" : "down"}"></i>`
        : "";
    return `<th class="${cls}" data-sort="${col}">${label}${arrow}</th>`;
  };

  wrap.innerHTML = /* html */ `
    <table class="table kc-table">
      <thead>
        <tr>
          ${header("title", "Title")}
          ${header("artist", "Artist")}
          ${header("traktorBpm", "Traktor BPM")}
          ${header("spotifyBpm", "Spotify BPM")}
          ${header("traktorKey", "Traktor Key")}
          ${header("spotifyKey", "Spotify Key")}
          ${header("bpmMatch", "BPM OK")}
          ${header("keyMatch", "Key OK")}
          ${header("spotifyDanceability", "Dance")}
          ${header("spotifyEnergy", "Energy")}
          ${header("spotifyValence", "Valence")}
        </tr>
      </thead>
      <tbody>
        ${sorted.map(renderRow).join("")}
      </tbody>
    </table>
  `;
}

function renderRow(r) {
  const bpmOk =
    r.bpmMatch === true
      ? '<span class="kc-match">✓</span>'
      : r.bpmMatch === false
        ? '<span class="kc-mismatch">✗</span>'
        : '<span class="kc-na">—</span>';
  const keyOk =
    r.keyMatch === true
      ? '<span class="kc-match">✓</span>'
      : r.keyMatch === false
        ? '<span class="kc-mismatch">✗</span>'
        : '<span class="kc-na">—</span>';

  const fmtVal = (v) => (v != null ? v.toFixed(2) : "—");

  return /* html */ `
    <tr>
      <td><a href="#file-detail?id=${r.fileId}" class="kc-track-link" title="${escHtml(r.title)}">${escHtml(trunc(r.title, 40))}</a></td>
      <td>${escHtml(r.artist || "—")}</td>
      <td>${r.traktorBpm != null ? r.traktorBpm.toFixed(1) : "—"}</td>
      <td>${r.spotifyBpm != null ? r.spotifyBpm.toFixed(1) : "—"}</td>
      <td>${escHtml(r.traktorKey || "—")}</td>
      <td>${escHtml(r.spotifyKey || "—")}</td>
      <td class="kc-icon-cell">${bpmOk}</td>
      <td class="kc-icon-cell">${keyOk}</td>
      <td>${fmtVal(r.spotifyDanceability)}</td>
      <td>${fmtVal(r.spotifyEnergy)}</td>
      <td>${fmtVal(r.spotifyValence)}</td>
    </tr>
  `;
}

/* ------------------------------------------------------------------ */
/*  Sort helpers                                                       */
/* ------------------------------------------------------------------ */

function sortVal(row, col) {
  const v = row[col];
  if (v == null) return "";
  if (typeof v === "boolean") return v ? "a" : "z";
  if (typeof v === "number") return v;
  return String(v).toLowerCase();
}

function cmp(a, b) {
  if (a < b) return -1;
  if (a > b) return 1;
  return 0;
}

/* ------------------------------------------------------------------ */
/*  Utils                                                              */
/* ------------------------------------------------------------------ */

function escHtml(s) {
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function trunc(s, max) {
  return s.length > max ? s.slice(0, max - 1) + "…" : s;
}

function getTagFromHash() {
  const raw = window.location.hash.replace(/^#/, "");
  const [page, query] = raw.split("?");
  if (page !== "key-comparison" || !query) return null;
  const params = new URLSearchParams(query);
  return params.get("tag");
}
