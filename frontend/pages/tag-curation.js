/**
 * tag-curation.js — Curation workflow for assigning parent tags to Setlist tags.
 *
 * Layout:
 *   ┌── Top Bar (prev/next + progress) ───────────────────────────┐
 *   │  ◀ Prev    Tag 3 of 87    Next ▶     ████░░░░  3%          │
 *   ├── Tag Card ─────────────────────────────────────────────────┤
 *   │  Dark Techno/2026/Hardtechno/Germantechno/...               │
 *   │  #54 · Setlist · 9 files                                    │
 *   ├── Parent Tags ──────────────────────────────────────────────┤
 *   │  [Mood dark ×]  [Vibe techno ×]  [Merkmal hard ×]           │
 *   │  [Search & add...        ]  [Add]  [New]                    │
 *   │  ┌─ dropdown (positioned) ──────────────┐                  │
 *   │  │ Vibe  techno               → Add     │                  │
 *   │  │ Mood  dark                 → Add     │                  │
 *   │  │ ── → Create & Add "xyz"              │                  │
 *   │  └──────────────────────────────────────┘                  │
 *   ├── Browse All Tags (collapsible) ──────────────── [▼] ──────┤
 *   │  [Search...]  Sort: [Name ▾]  Has parents: [All ▾]         │
 *   │  ┌────────────────────────────────────────────────────────┐ │
 *   │  │ Dark Techno/2026/...         9 files  [3]              │ │
 *   │  │ Beatport Top 100 Techno      5 files  [0]              │ │
 *   │  └────────────────────────────────────────────────────────┘ │
 *   └─────────────────────────────────────────────────────────────┘
 *
 * Keyboard:
 *   ← / p — previous tag
 *   → / n — next tag
 *   /     — focus search input
 *   Esc   — close any open dropdown
 */

import {
  escapeHtml,
  renderLoading,
  renderErrorBlock,
  showToast,
} from "../shared/components.js";
import { fetchJSON } from "../shared/api.js";

/* ------------------------------------------------------------------ */
/*  Constants                                                         */
/* ------------------------------------------------------------------ */

const CATEGORY_INFO = {
  Phase: { color: "var(--purple)", icon: "fa-activity" },
  Mood: { color: "var(--pink)", icon: "fa-heart" },
  Vibe: { color: "var(--yellow)", icon: "fa-sparkles" },
  Merkmal: { color: "var(--green)", icon: "fa-hash" },
  Setlist: { color: "var(--text-muted)", icon: "fa-list-music" },
};

/* ------------------------------------------------------------------ */
/*  State                                                             */
/* ------------------------------------------------------------------ */

/** All tags loaded once for client-side search */
let allTags = [];

/** Inline category picker state */
let pickerMode = false;
let pickerSelectedIndex = -1;

const state = {
  queue: [],
  currentIndex: 0,
  currentTag: null,
  parents: [],
  search: "",
  sort: "length",
  order: "desc",
  hasParents: "any",
  browseCollapsed: true,
  saveInFlight: null,
  browseSearch: "",
  browseSort: "length",
  browseOrder: "desc",
  browseHasParents: "any",
};

/* ------------------------------------------------------------------ */
/*  Rendering helpers                                                  */
/* ------------------------------------------------------------------ */

/**
 * Render the top navigation bar with prev/next buttons and progress bar.
 */
function renderTopBar(total, index) {
  const pct = total > 0 ? Math.round(((index + 1) / total) * 100) : 0;
  const prevDisabled = index <= 0;
  const nextDisabled = index >= total - 1;

  return `
    <div style="display:flex;align-items:center;gap:var(--space-4);margin-bottom:var(--space-4);flex-wrap:wrap;">
      <button class="btn btn-sm" id="cur-prev-btn"${prevDisabled ? " disabled" : ""}>
        <i class="fas fa-chevron-left"></i> Prev
      </button>
      <span style="font-size:0.85rem;color:var(--text-muted);font-weight:500;white-space:nowrap;">
        Tag ${index + 1} of ${total}
      </span>
      <button class="btn btn-sm" id="cur-next-btn"${nextDisabled ? " disabled" : ""}>
        Next <i class="fas fa-chevron-right"></i>
      </button>
      <div class="progress-bar" style="flex:1;min-width:120px;max-width:300px;">
        <div class="progress-bar-fill" style="width:${pct}%;"></div>
      </div>
      <span style="font-size:0.75rem;color:var(--text-subtle);font-family:var(--font-mono);">${pct}%</span>
    </div>
  `;
}

/**
 * Render the tag card showing the current tag's name, ID, category, file count.
 */
function renderTagCard(tag) {
  if (!tag) return "";
  const catInfo = CATEGORY_INFO[tag.category] || CATEGORY_INFO.Setlist;
  return `
    <div style="background:var(--surface);border:1px solid var(--border);border-radius:var(--radius-xl);padding:var(--space-5) var(--space-6);margin-bottom:var(--space-4);">
      <div style="font-size:1.25rem;font-weight:700;color:var(--text);word-break:break-word;line-height:1.3;margin-bottom:var(--space-2);">
        ${escapeHtml(tag.name)}
      </div>
      <div style="display:flex;align-items:center;gap:var(--space-3);font-size:0.8rem;color:var(--text-muted);">
        <span style="background:var(--border);padding:1px 8px;border-radius:var(--radius-md);color:var(--text-subtle);font-family:var(--font-mono);">#${tag.id}</span>
        <span style="color:${catInfo.color};display:inline-flex;align-items:center;gap:4px;">
          <i class="${catInfo.icon}"></i> ${escapeHtml(tag.category)}
        </span>
        <span><i class="fas fa-file"></i> ${tag.fileCount} files</span>
      </div>
    </div>
  `;
}

/**
 * Render a parent tag chip (category badge + name + × remove button).
 */
function renderParentChip(parent) {
  const catInfo = CATEGORY_INFO[parent.category] || CATEGORY_INFO.Setlist;
  return `
    <span class="tag-chip" data-parent-id="${parent.id}" style="display:inline-flex;align-items:center;gap:var(--space-1);background:var(--accent-bg);color:var(--accent);border:1px solid var(--border);border-radius:var(--radius-lg);padding:2px 10px 2px 12px;font-size:0.8rem;font-weight:500;margin:2px;">
      <span style="color:${catInfo.color};font-size:0.7rem;font-weight:700;text-transform:uppercase;">${parent.category}</span>
      ${escapeHtml(parent.name)}
      <span class="parent-remove-btn" data-parent-id="${parent.id}" style="cursor:pointer;font-size:0.7rem;opacity:0.6;transition:opacity 0.15s;margin-left:2px;" title="Remove parent">
        <i class="fas fa-times"></i>
      </span>
    </span>
  `;
}

/**
 * Render the parent tags section: current chips + search typeahead + Add/New buttons.
 */
function renderParentArea(tagId, parents) {
  const chipsHtml =
    parents.length > 0
      ? parents.map(renderParentChip).join("")
      : '<span style="font-size:0.85rem;color:var(--text-subtle);">No parent tags set. This long tag name will appear in comments as-is.</span>';

  return `
    <div style="background:var(--surface);border:1px solid var(--border);border-radius:var(--radius-xl);padding:var(--space-5) var(--space-6);margin-bottom:var(--space-4);">
      <div style="display:flex;align-items:center;gap:var(--space-2);margin-bottom:var(--space-3);">
        <i class="fas fa-sitemap" style="color:var(--text-muted);font-size:0.85rem;"></i>
        <span style="font-weight:600;font-size:0.85rem;color:var(--text-secondary);">Parent Tags</span>
        <span style="font-size:0.7rem;color:var(--text-subtle);font-weight:400;">(aliases used in comments)</span>
      </div>
      <div id="cur-parent-chips" style="display:flex;flex-wrap:wrap;gap:2px;margin-bottom:var(--space-3);min-height:28px;">
        ${chipsHtml}
      </div>
      <div style="display:flex;gap:6px;position:relative;">
        <div class="typeahead-wrap" style="flex:1;position:relative;">
          <input type="text" class="input-text w-full" id="cur-parent-search" placeholder="Search &amp; add parent tags…" autocomplete="off" style="font-size:0.85rem;">
          <div id="cur-parent-dropdown" class="typeahead-dropdown" style="display:none;position:absolute;top:100%;left:0;right:0;max-height:220px;overflow-y:auto;background:var(--bg);border:1px solid var(--border);border-top:none;border-radius:0 0 var(--radius-md) var(--radius-md);z-index:100;box-shadow:0 8px 24px rgba(0,0,0,0.3);"></div>
        </div>
        <button class="btn btn-sm btn-primary" id="cur-add-btn" disabled style="white-space:nowrap;"><i class="fas fa-plus"></i> Add</button>
        <button class="btn btn-sm" id="cur-new-btn" style="white-space:nowrap;"><i class="fas fa-tag"></i> New</button>
      </div>
      <div style="font-size:0.75rem;color:var(--text-subtle);margin-top:var(--space-2);">
        Parent tags replace this tag in file comments. Each parent contributes its own category (P/M/V/E).
      </div>
    </div>
  `;
}

/**
 * Render the collapsible Browse All section with filter inputs and mini table.
 */
function renderBrowseSection() {
  const collapsed = state.browseCollapsed;
  const arrowIcon = collapsed ? "fa-chevron-down" : "fa-chevron-up";

  return `
    <div style="background:var(--surface);border:1px solid var(--border);border-radius:var(--radius-xl);overflow:hidden;">
      <div id="cur-browse-toggle" style="display:flex;align-items:center;gap:var(--space-3);padding:var(--space-3) var(--space-5);cursor:pointer;user-select:none;background:var(--surface);border-bottom:${collapsed ? "none" : "1px solid var(--border)"};">
        <i class="fas fa-list" style="color:var(--text-muted);font-size:0.85rem;"></i>
        <span style="font-weight:600;font-size:0.85rem;color:var(--text-secondary);flex:1;">Browse All Tags</span>
        <span style="color:var(--text-subtle);font-size:0.8rem;" id="cur-browse-count"></span>
        <i class="fas ${arrowIcon}" style="color:var(--text-subtle);font-size:0.75rem;"></i>
      </div>
      <div id="cur-browse-body" style="display:${collapsed ? "none" : "block"};">
        <div style="display:flex;gap:var(--space-3);padding:var(--space-3) var(--space-5);border-bottom:1px solid var(--border);flex-wrap:wrap;align-items:center;">
          <div style="flex:1;min-width:120px;position:relative;">
            <i class="fas fa-search" style="position:absolute;left:10px;top:50%;transform:translateY(-50%);color:var(--text-subtle);font-size:0.75rem;pointer-events:none;"></i>
            <input type="text" class="input-text" id="cur-browse-search" placeholder="Filter tags…" autocomplete="off" style="padding-left:28px;font-size:0.85rem;" value="${escapeHtml(state.browseSearch)}">
          </div>
          <div style="display:flex;align-items:center;gap:var(--space-2);">
            <label style="font-size:0.75rem;color:var(--text-subtle);white-space:nowrap;">Sort:</label>
            <select id="cur-browse-sort" class="input-text" style="width:auto;font-size:0.85rem;padding:var(--space-1) var(--space-2);">
              <option value="name"${state.browseSort === "name" ? " selected" : ""}>Name</option>
              <option value="length"${state.browseSort === "length" ? " selected" : ""}>Length</option>
              <option value="files"${state.browseSort === "files" ? " selected" : ""}>Files</option>
              <option value="parents"${state.browseSort === "parents" ? " selected" : ""}>Parents</option>
            </select>
            <button class="btn btn-sm btn-icon" id="cur-browse-order" title="Toggle sort order" style="font-size:0.75rem;">
              <i class="fas fa-arrow-${state.browseOrder === "asc" ? "up" : "down"}"></i>
            </button>
          </div>
          <div style="display:flex;align-items:center;gap:var(--space-2);">
            <label style="font-size:0.75rem;color:var(--text-subtle);white-space:nowrap;">Parents:</label>
            <select id="cur-browse-has-parents" class="input-text" style="width:auto;font-size:0.85rem;padding:var(--space-1) var(--space-2);">
              <option value="any"${state.browseHasParents === "any" ? " selected" : ""}>All</option>
              <option value="yes"${state.browseHasParents === "yes" ? " selected" : ""}>Yes</option>
              <option value="no"${state.browseHasParents === "no" ? " selected" : ""}>No</option>
            </select>
          </div>
        </div>
        <div id="cur-browse-table-wrap" style="overflow-x:auto;">
          <div id="cur-browse-loading" style="text-align:center;padding:var(--space-6);color:var(--text-muted);font-size:0.85rem;">
            <i class="fas fa-spinner fa-spin"></i> Loading…
          </div>
        </div>
      </div>
    </div>
  `;
}

/** Clear the dropdown */
/* ------------------------------------------------------------------ */
/*  API helpers                                                       */
/* ------------------------------------------------------------------ */

async function fetchQueue(signal) {
  const params = new URLSearchParams();
  if (state.search) params.set("search", state.search);
  if (state.sort) params.set("sort", state.sort);
  if (state.order) params.set("order", state.order);
  if (state.hasParents && state.hasParents !== "any")
    params.set("has_parents", state.hasParents);
  params.set("limit", "500");

  const resp = await fetchJSON(`/api/tags/curation-queue?${params.toString()}`, {
    signal,
  });
  return resp.data || [];
}

async function saveParents(tagId, parentTagIds) {
  const promise = fetchJSON(`/api/tags/${tagId}/parents`, {
    method: "PUT",
    body: JSON.stringify({ parentTagIds }),
  });
  state.saveInFlight = promise;
  try {
    await promise;
  } finally {
    if (state.saveInFlight === promise) {
      state.saveInFlight = null;
    }
  }
}

/** Filter cached allTags by search term (client-side, no API call) */
function filterTags(query) {
  const q = query.toLowerCase().trim();
  if (!q) return allTags;
  return allTags.filter((t) => t.name.toLowerCase().includes(q));
}

async function createTag(name, categoryName, signal) {
  // Find the category by name — need to fetch categories first
  const catsResp = await fetchJSON("/api/tag-categories", { signal });
  const categories = catsResp.data || [];
  const cat = categories.find((c) => c.name === categoryName);
  if (!cat) throw new Error(`Category "${categoryName}" not found`);

  const resp = await fetchJSON("/api/tags", {
    method: "POST",
    body: JSON.stringify({ name: name.trim(), categoryId: cat.id }),
    signal,
  });
  // POST /api/tags usually returns the created tag with .data wrapper
  return resp.data || resp;
}

/* ------------------------------------------------------------------ */
/*  DOM helpers                                                       */
/* ------------------------------------------------------------------ */

function getEl(id, container) {
  return container.querySelector(`#${id}`);
}

/* ------------------------------------------------------------------ */
/*  Browse section logic                                              */
/* ------------------------------------------------------------------ */

let browseDebounceTimer = null;

async function renderBrowseTable(container) {
  const wrap = getEl("cur-browse-table-wrap", container);
  if (!wrap) return;
  const loading = getEl("cur-browse-loading", container);
  if (loading) loading.style.display = "block";

  try {
    const params = new URLSearchParams();
    if (state.browseSearch) params.set("search", state.browseSearch);
    if (state.browseSort) params.set("sort", state.browseSort);
    if (state.browseOrder) params.set("order", state.browseOrder);
    if (state.browseHasParents && state.browseHasParents !== "any") {
      params.set("has_parents", state.browseHasParents);
    }
    params.set("limit", "200");

    const resp = await fetchJSON(`/api/tags/curation-queue?${params.toString()}`);
    const items = resp.data || [];

    // Update count
    const countEl = getEl("cur-browse-count", container);
    if (countEl) countEl.textContent = `${items.length} tags`;

    if (items.length === 0) {
      wrap.innerHTML = `<div style="text-align:center;padding:var(--space-6);color:var(--text-muted);font-size:0.85rem;">No matching tags</div>`;
      return;
    }

    // Sort locally for length sort
    let sorted = [...items];
    if (state.browseSort === "length") {
      sorted.sort((a, b) => {
        const diff = a.name.length - b.name.length;
        return state.browseOrder === "asc" ? diff : -diff;
      });
    }

    const rowsHtml = sorted
      .map(
        (item) => `
      <tr class="browse-tag-row" data-tag-id="${item.id}" style="cursor:pointer;">
        <td style="padding:var(--space-2) var(--space-3);font-size:0.85rem;color:var(--text);word-break:break-word;">${escapeHtml(item.name)}</td>
        <td style="padding:var(--space-2) var(--space-3);font-size:0.8rem;color:var(--text-muted);text-align:center;white-space:nowrap;">${item.fileCount}</td>
        <td style="padding:var(--space-2) var(--space-3);text-align:center;white-space:nowrap;">
          <span class="badge" style="background:${item.parentCount > 0 ? "var(--accent-bg)" : "var(--border)"};color:${item.parentCount > 0 ? "var(--accent)" : "var(--text-subtle)"};font-size:0.75rem;">
            ${item.parentCount}
          </span>
        </td>
      </tr>`,
      )
      .join("");

    wrap.innerHTML = `
      <table class="data-table">
        <thead>
          <tr>
            <th style="text-align:left;padding:var(--space-2) var(--space-3);">Name</th>
            <th style="text-align:center;padding:var(--space-2) var(--space-3);width:80px;">Files</th>
            <th style="text-align:center;padding:var(--space-2) var(--space-3);width:80px;">Parents</th>
          </tr>
        </thead>
        <tbody>${rowsHtml}</tbody>
      </table>
    `;

    // Wire row click to jump to tag
    wrap.querySelectorAll(".browse-tag-row").forEach((row) => {
      row.addEventListener("click", () => {
        const tagId = parseInt(row.dataset.tagId, 10);
        jumpToTag(tagId, container);
      });
    });
  } catch (err) {
    if (err.name === "AbortError") return;
    if (wrap) {
      wrap.innerHTML = `<div style="text-align:center;padding:var(--space-6);color:var(--red);font-size:0.85rem;">Failed to load: ${escapeHtml(err.message)}</div>`;
    }
  }
}

function jumpToTag(tagId, container) {
  const idx = state.queue.findIndex((t) => t.id === tagId);
  if (idx >= 0) {
    state.currentIndex = idx;
    renderCurrentTag(container);
  }
}

/* ------------------------------------------------------------------ */
/*  Current tag rendering                                              */
/* ------------------------------------------------------------------ */

function clearDropdown() {
  const dd = document.getElementById("cur-parent-dropdown");
  if (dd) dd.style.display = "none";
  // Also remove category picker if open
  const picker = document.getElementById("cur-cat-picker");
  if (picker) picker.remove();
}

function getCurrentParentIds() {
  return state.parents.map((p) => p.id);
}

async function addParent(tag, container) {
  if (state.parents.some((p) => p.id === tag.id)) return;
  state.parents.push(tag);
  await persistParents(container);
  renderParentChips(container);
  clearDropdown();
}

async function removeParent(parentId, container) {
  state.parents = state.parents.filter((p) => p.id !== parentId);
  await persistParents(container);
  renderParentChips(container);
}

async function persistParents(container) {
  if (!state.currentTag) return;
  const parentIds = getCurrentParentIds();
  try {
    await saveParents(state.currentTag.id, parentIds);
    // No toast on success — silent is fine
  } catch (err) {
    showToast(`Failed to save parents: ${err.message}`, "error");
  }
}

function renderParentChips(container) {
  const chipsContainer = getEl("cur-parent-chips", container);
  if (!chipsContainer) return;
  if (state.parents.length === 0) {
    chipsContainer.innerHTML =
      '<span style="font-size:0.85rem;color:var(--text-subtle);">No parent tags set. This long tag name will appear in comments as-is.</span>';
  } else {
    chipsContainer.innerHTML = state.parents.map(renderParentChip).join("");
  }
  // Wire remove buttons
  chipsContainer.querySelectorAll(".parent-remove-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      const pid = parseInt(btn.dataset.parentId, 10);
      removeParent(pid, container);
    });
  });
}

async function renderCurrentTag(container) {
  if (state.queue.length === 0) {
    container.innerHTML = `
      <div class="empty-state">
        <div class="empty-icon"><i class="fas fa-check-circle" style="color:var(--green);font-size:3rem;"></i></div>
        <h3>No tags to curate</h3>
        <p>No Setlist tags found. Create some tags first.</p>
        <a href="#tags" class="btn btn-primary"><i class="fas fa-tags"></i> View Tags</a>
      </div>`;
    return;
  }

  if (state.currentIndex >= state.queue.length) {
    state.currentIndex = state.queue.length - 1;
  }
  if (state.currentIndex < 0) {
    state.currentIndex = 0;
  }

  const tag = state.queue[state.currentIndex];
  state.currentTag = tag;

  // Fetch current parents for this tag
  try {
    const resp = await fetchJSON(`/api/tags/${tag.id}/parents`);
    state.parents = resp.data || [];
  } catch (_) {
    state.parents = tag.parents || [];
  }

  // Wait for any in-flight save from previous navigation
  if (state.saveInFlight) {
    try {
      await state.saveInFlight;
    } catch (_) {
      // Ignore previous save errors
    }
  }

  container.innerHTML = `
    <div id="cur-main" style="max-width:800px;margin:0 auto;">
      ${renderTopBar(state.queue.length, state.currentIndex)}
      ${renderTagCard(state.currentTag)}
      ${renderParentArea(state.currentTag.id, state.parents)}
      ${renderBrowseSection()}
    </div>
  `;

  wireEvents(container);
  renderBrowseTable(container);
}

/* ------------------------------------------------------------------ */
/*  Event wiring                                                      */
/* ------------------------------------------------------------------ */

async function navigate(delta, container) {
  // Wait for in-flight save
  if (state.saveInFlight) {
    try {
      await state.saveInFlight;
    } catch (_) {}
  }
  const newIdx = state.currentIndex + delta;
  if (newIdx < 0 || newIdx >= state.queue.length) return;
  state.currentIndex = newIdx;
  await renderCurrentTag(container);
}

/**
 * Wire all events for the page.
 */
function wireEvents(container) {
  // ── Prev / Next buttons ──
  const prevBtn = getEl("cur-prev-btn", container);
  const nextBtn = getEl("cur-next-btn", container);
  if (prevBtn) prevBtn.addEventListener("click", () => navigate(-1, container));
  if (nextBtn) nextBtn.addEventListener("click", () => navigate(1, container));

  // ── Browse section toggle ──
  const browseToggle = getEl("cur-browse-toggle", container);
  if (browseToggle) {
    browseToggle.addEventListener("click", () => {
      state.browseCollapsed = !state.browseCollapsed;
      localStorage.setItem("browseCollapsed_tagCuration", state.browseCollapsed);
      // Re-render just the browse section
      const body = getEl("cur-browse-body", container);
      const arrow = browseToggle.querySelector(".fa-chevron-down, .fa-chevron-up");
      if (body) {
        body.style.display = state.browseCollapsed ? "none" : "block";
      }
      if (arrow) {
        arrow.className = `fas ${state.browseCollapsed ? "fa-chevron-down" : "fa-chevron-up"}`;
      }
      if (!state.browseCollapsed) {
        renderBrowseTable(container);
      }
    });
  }

  // ── Browse search ──
  const browseSearch = getEl("cur-browse-search", container);
  if (browseSearch) {
    browseSearch.addEventListener("input", () => {
      clearTimeout(browseDebounceTimer);
      browseDebounceTimer = setTimeout(() => {
        state.browseSearch = browseSearch.value.trim();
        renderBrowseTable(container);
      }, 250);
    });
  }

  // ── Browse sort ──
  const browseSort = getEl("cur-browse-sort", container);
  if (browseSort) {
    browseSort.addEventListener("change", () => {
      state.browseSort = browseSort.value;
      renderBrowseTable(container);
    });
  }

  // ── Browse order ──
  const browseOrder = getEl("cur-browse-order", container);
  if (browseOrder) {
    browseOrder.addEventListener("click", () => {
      state.browseOrder = state.browseOrder === "asc" ? "desc" : "asc";
      const icon = browseOrder.querySelector("i");
      if (icon) {
        icon.className = `fas fa-arrow-${state.browseOrder === "asc" ? "up" : "down"}`;
      }
      renderBrowseTable(container);
    });
  }

  // ── Browse has-parents filter ──
  const browseHasParents = getEl("cur-browse-has-parents", container);
  if (browseHasParents) {
    browseHasParents.addEventListener("change", () => {
      state.browseHasParents = browseHasParents.value;
      renderBrowseTable(container);
    });
  }

  // ── Parent search typeahead ──
  const searchInput = getEl("cur-parent-search", container);
  const dropdown = getEl("cur-parent-dropdown", container);
  const addBtn = getEl("cur-add-btn", container);
  const newBtn = getEl("cur-new-btn", container);

  if (searchInput && dropdown) {
    let updateTimer = null;

    function updateDropdown() {
      const q = searchInput.value.trim();
      if (q.length < 1) {
        dropdown.style.display = "none";
        if (addBtn) addBtn.disabled = true;
        return;
      }

      // Filter locally from the cached tag list (no API call)
      const matches = filterTags(q).filter(
        (t) => t.id !== state.currentTag?.id && !state.parents.some((p) => p.id === t.id),
      );

      // Build items: existing tags + always the "Create" entry at the end
      const items = matches.map((t) => ({
        type: "tag",
        id: t.id,
        name: t.name,
        category: t.category,
      }));
      items.push({ type: "create", search: q });

      // Clamp pickerSelectedIndex
      let idx = pickerSelectedIndex;
      if (idx >= items.length) idx = items.length - 1;
      if (idx < 0) idx = 0;
      pickerSelectedIndex = idx;

      let html = items
        .map((item, idx2) => {
          const sel =
            idx2 === pickerSelectedIndex
              ? "background:var(--accent-bg);outline:1px solid var(--accent);"
              : "";
          if (item.type === "tag") {
            const catInfo = CATEGORY_INFO[item.category] || CATEGORY_INFO.Setlist;
            return `<div class="typeahead-item" data-index="${idx}" data-id="${item.id}" data-name="${escapeHtml(item.name)}" data-category="${escapeHtml(item.category || "")}" style="padding:8px 12px;cursor:pointer;display:flex;align-items:center;gap:8px;font-size:0.85rem;border-bottom:1px solid var(--border);transition:background 0.1s;${sel}">
              <span style="color:${catInfo.color};font-size:0.7rem;font-weight:700;text-transform:uppercase;min-width:40px;">${item.category || "?"}</span>
              <span style="flex:1;color:var(--text);">${escapeHtml(item.name)}</span>
              <span style="font-size:0.75rem;color:var(--accent);white-space:nowrap;">→ Add</span>
            </div>`;
          }
          return `<div class="typeahead-item typeahead-create" data-index="${idx}" data-search="${escapeHtml(item.search)}" style="padding:8px 12px;cursor:pointer;display:flex;align-items:center;gap:8px;font-size:0.85rem;border-top:${matches.length > 0 ? "1px solid var(--border-strong)" : "none"};transition:background 0.1s;${sel}">
            <i class="fas fa-plus-circle" style="color:var(--green);font-size:0.9rem;"></i>
            <span style="flex:1;color:var(--text-muted);">→ Create &amp; Add <strong>"${escapeHtml(item.search)}"</strong></span>
          </div>`;
        })
        .join("");

      dropdown.innerHTML = html;
      dropdown.style.display = "block";
      if (addBtn) addBtn.disabled = false;

      // Wire clicks
      dropdown.querySelectorAll(".typeahead-item").forEach((el) => {
        el.addEventListener("click", () => {
          const idx = parseInt(el.dataset.index, 10);
          selectItem(idx, searchInput, dropdown, container);
        });
      });
    }

    async function selectItem(idx, searchInput, dropdown, container) {
      const items = dropdown.querySelectorAll(".typeahead-item");
      if (idx < 0 || idx >= items.length) return;
      const el = items[idx];

      if (el.classList.contains("typeahead-create")) {
        // Switch to inline category picker in the dropdown
        showCategoryPicker(el.dataset.search, searchInput, dropdown, container);
        return;
      }

      const id = parseInt(el.dataset.id, 10);
      const name = el.dataset.name;
      const category = el.dataset.category;
      await addParent({ id, name, category }, container);
      searchInput.value = "";
      dropdown.style.display = "none";
    }

    function highlightItem(idx) {
      dropdown.querySelectorAll(".typeahead-item, .cat-pick-btn").forEach((el, i) => {
        el.style.background = i === idx ? "var(--accent-bg)" : "";
        el.style.outline = i === idx ? "1px solid var(--accent)" : "";
      });
    }

    searchInput.addEventListener("input", () => {
      clearTimeout(updateTimer);
      pickerSelectedIndex = -1;
      pickerMode = false;
      if (searchInput.value.trim().length === 0) {
        dropdown.style.display = "none";
        if (addBtn) addBtn.disabled = true;
        return;
      }
      updateTimer = setTimeout(() => {
        pickerSelectedIndex = 0;
        updateDropdown();
      }, 100);
    });

    // Keyboard navigation: arrows + enter + escape
    searchInput.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        if (pickerMode) {
          // Go back to search mode
          pickerMode = false;
          pickerSelectedIndex = 0;
          updateDropdown();
          searchInput.focus();
          return;
        }
        dropdown.style.display = "none";
        searchInput.blur();
        return;
      }

      const items = dropdown.querySelectorAll(pickerMode ? ".cat-pick-btn" : ".typeahead-item");

      if (e.key === "ArrowDown") {
        e.preventDefault();
        if (items.length === 0) return;
        pickerSelectedIndex = (pickerSelectedIndex + 1) % items.length;
        highlightItem(pickerSelectedIndex);
        return;
      }

      if (e.key === "ArrowUp") {
        e.preventDefault();
        if (items.length === 0) return;
        pickerSelectedIndex = (pickerSelectedIndex - 1 + items.length) % items.length;
        highlightItem(pickerSelectedIndex);
        return;
      }

      if (e.key === "Enter") {
        e.preventDefault();
        if (dropdown.style.display === "none" || items.length === 0 || pickerSelectedIndex < 0) {
          return;
        }
        if (pickerMode) {
          // In picker mode: create the tag with the selected category
          const el = items[pickerSelectedIndex];
          const categoryName = el.dataset.category;
          const tagName = el.dataset.tagname;
          try {
            const newTag = await createTag(tagName, categoryName);
            await addParent(newTag, container);
            showToast(`Created "${tagName}" and added as parent`, "success");
          } catch (err) {
            showToast(`Failed: ${err.message}`, "error");
          }
          dropdown.style.display = "none";
          searchInput.value = "";
          pickerMode = false;
        } else {
          selectItem(pickerSelectedIndex, searchInput, dropdown, container);
        }
      }
    });

    // Close dropdown on outside click
    document.addEventListener("click", (e) => {
      if (searchInput && dropdown && !searchInput.contains(e.target) && !dropdown.contains(e.target)) {
        pickerMode = false;
        dropdown.style.display = "none";
      }
    });
  }

  // ── Add button: selects first non-create item, or shows category picker ──
  if (addBtn && searchInput && dropdown) {
    addBtn.addEventListener("click", async () => {
      const first = dropdown.querySelector(".typeahead-item:not(.typeahead-create)");
      if (first) {
        const idx = parseInt(first.dataset.index, 10);
        await selectItem(idx, searchInput, dropdown, container);
      } else if (searchInput.value.trim()) {
        showCategoryPicker(searchInput.value.trim(), searchInput, dropdown, container);
        searchInput.value = "";
      }
    });
  }

  // ── New button: shows category picker inline ──
  if (newBtn) {
    newBtn.addEventListener("click", () => {
      const q = searchInput?.value.trim() || "";
      showCategoryPicker(q || "new tag", searchInput, dropdown, container);
      if (searchInput) searchInput.value = "";
    });
  }
}

// ── Inline Category Picker ─────────────────────────────────────────

/** Show category buttons inline in the dropdown */
function showCategoryPicker(tagName, searchInput, dropdown, container) {
  const cats = [
    { name: "Phase",   color: "var(--purple)", icon: "fa-activity" },
    { name: "Mood",    color: "var(--pink)",   icon: "fa-heart" },
    { name: "Vibe",    color: "var(--yellow)", icon: "fa-sparkles" },
    { name: "Merkmal", color: "var(--green)",  icon: "fa-hash" },
  ];

  pickerMode = true;
  pickerSelectedIndex = 0;

  const html = cats
    .map(
      (c, idx) =>
        `<div class="cat-pick-btn" data-index="${idx}" data-category="${c.name}" data-tagname="${escapeHtml(tagName)}" style="padding:10px 14px;cursor:pointer;display:flex;align-items:center;gap:10px;font-size:0.9rem;${idx === 0 ? "background:var(--accent-bg);outline:1px solid var(--accent);" : ""}border-bottom:1px solid var(--border);transition:background 0.1s;">
          <span style="color:${c.color};font-size:1.1rem;width:24px;text-align:center;"><i class="fas ${c.icon}"></i></span>
          <span style="flex:1;color:var(--text-secondary);">Create as <strong>${c.name}</strong></span>
          <span style="color:var(--text-subtle);font-size:0.75rem;white-space:nowrap;">${tagName}</span>
        </div>`,
    )
    .join("");

  dropdown.innerHTML = `
    <div style="padding:8px 12px;font-size:0.8rem;color:var(--text-muted);border-bottom:1px solid var(--border);display:flex;align-items:center;gap:6px;">
      <i class="fas fa-plus-circle" style="color:var(--green);"></i>
      <span>Pick category for <strong>"${escapeHtml(tagName)}"</strong> (Esc to go back)</span>
    </div>
    ${html}
  `;
  dropdown.style.display = "block";

  // Wire clicks on category buttons
  dropdown.querySelectorAll(".cat-pick-btn").forEach((el) => {
    el.addEventListener("click", () => {
      const categoryName = el.dataset.category;
      const tagName = el.dataset.tagname;
      doCreateTag(tagName, categoryName, searchInput, dropdown, container);
    });
  });
}

async function doCreateTag(tagName, categoryName, searchInput, dropdown, container) {
  try {
    const newTag = await createTag(tagName, categoryName);
    await addParent(newTag, container);
    showToast(`Created "${tagName}" and added as parent`, "success");
  } catch (err) {
    showToast(`Failed: ${err.message}`, "error");
  }
  dropdown.style.display = "none";
  searchInput.value = "";
  pickerMode = false;
}

/* ------------------------------------------------------------------ */
/*  Keyboard shortcuts                                                 */
/* ------------------------------------------------------------------ */

/**
 * Sets up global keyboard shortcuts for the page.
 */
function setupKeyboardShortcuts(container, signal) {
  const handler = (e) => {
    // Don't intercept if typing in an input
    const tag = e.target.tagName;
    if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") {
      // Allow Escape even in inputs
      if (e.key === "Escape") {
        clearDropdown();
        e.target.blur();
        return;
      }
      // Allow / in inputs to do nothing (already focused)
      return;
    }

    switch (e.key) {
      case "ArrowLeft":
      case "p":
      case "P":
        e.preventDefault();
        navigate(-1, container);
        break;
      case "ArrowRight":
      case "n":
      case "N":
        e.preventDefault();
        navigate(1, container);
        break;
      case "/":
        e.preventDefault();
        const searchInput = document.getElementById("cur-parent-search");
        if (searchInput) searchInput.focus();
        break;
      case "Escape":
        clearDropdown();
        break;
    }
  };

  document.addEventListener("keydown", handler);
  signal.addEventListener("abort", () => {
    document.removeEventListener("keydown", handler);
  });
}

/* ------------------------------------------------------------------ */
/*  Page initialisation                                                */
/* ------------------------------------------------------------------ */

/**
 * Page init — called by the SPA router when navigating to #tag-curation.
 *
 * @param {HTMLElement} container
 * @param {AbortSignal} signal
 * @param {object} hashParams
 */
export async function init(container, signal, hashParams) {
  container.tabIndex = -1;
  container.style.outline = "none";
  container.innerHTML = renderLoading("Loading tag curation…");
  if (signal.aborted) return;

  // Restore browse collapsed state from localStorage
  const saved = localStorage.getItem("browseCollapsed_tagCuration");
  if (saved !== null) {
    state.browseCollapsed = saved === "true";
  }

  try {
    // Fetch all tags for client-side search, then the curation queue
    const tagsResp = await fetchJSON("/api/tags?limit=5000", { signal });
    if (signal.aborted) return;
    allTags = tagsResp.data || [];

    state.queue = await fetchQueue(signal);
    if (signal.aborted) return;

    if (state.queue.length === 0) {
      container.innerHTML = `
        <div class="empty-state">
          <div class="empty-icon"><i class="fas fa-check-circle" style="color:var(--green);font-size:3rem;"></i></div>
          <h3>No tags to curate</h3>
          <p>No Setlist tags found. Create some tags with long names first, then assign parent tags to control how they appear in comments.</p>
          <a href="#tags" class="btn btn-primary"><i class="fas fa-tags"></i> View Tags</a>
        </div>`;
      return;
    }

    // Start at the first tag
    state.currentIndex = 0;
    await renderCurrentTag(container);
    setupKeyboardShortcuts(container, signal);

    // Focus the container for keyboard events
    container.focus();
  } catch (err) {
    if (err.name === "AbortError") return;
    container.innerHTML = renderErrorBlock({
      title: "Failed to load tag curation",
      detail: err.message || "Unknown error",
      retryFn: "window.location.hash='#tag-curation'",
    });
  }
}
