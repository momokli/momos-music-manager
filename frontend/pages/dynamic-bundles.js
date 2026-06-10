/**
 * dynamic-bundles.js — Dynamic Tag Bundles management page.
 *
 * A dynamic bundle defines filter criteria (base tags, BPM range, PMV, file types)
 * that are evaluated to determine which files belong. Creates a Setlist tag
 * that can be toggled as backpack.
 *
 * Layout:
 *   Left panel: searchable list of dynamic bundles with file count + backpack badge
 *   Right panel: edit form with filter criteria + file preview
 */

import {
  escapeHtml,
  renderLoading,
  renderErrorBlock,
  renderEmpty,
  showToast,
  showModal,
} from "../shared/components.js";
import { fetchJSON } from "../shared/api.js";

/* ------------------------------------------------------------------ */
/*  Constants                                                         */
/* ------------------------------------------------------------------ */

const MINOR_KEYS = Array.from({ length: 12 }, (_, i) => `${i + 1}m`);
const MAJOR_KEYS = Array.from({ length: 12 }, (_, i) => `${i + 1}d`);

/* ------------------------------------------------------------------ */
/*  State                                                             */
/* ------------------------------------------------------------------ */

const state = {
  bundles: [],
  selectedId: null,

  // Edit form fields
  editName: "",
  editAllTracks: false,
  editBaseTags: [],
  editBpmMin: null,
  editBpmMax: null,
  editPmvCategories: [],
  editKeys: [],
  editRatingMin: null,
  editPlayCountMin: null,

  // Tag typeahead
  tagSearch: "",
  typeaheadResults: [],
  typeaheadOpen: false,
  typeaheadIndex: -1,

  // Preview
  previewTracks: [],
  previewLoading: false,

  // General
  loading: false,
  saving: false,
};

/* ------------------------------------------------------------------ */
/*  Page Init                                                         */
/* ------------------------------------------------------------------ */

/**
 * Page init — called by the SPA router on #dynamic-bundles.
 * @param {HTMLElement} container
 * @param {AbortSignal} signal
 * @param {Object} hashParams
 */
export async function init(container, signal, hashParams) {
  container.innerHTML = renderLoading("Loading dynamic bundles…");
  if (signal.aborted) return;

  try {
    const resp = await fetchJSON("/api/dynamic-bundles?limit=200", { signal });
    if (signal.aborted) return;
    state.bundles = resp.data || [];
    state.selectedId = null;
    resetForm();

    renderFullPage(container);
    wireEvents(container, signal);
  } catch (err) {
    if (err.name === "AbortError") return;
    container.innerHTML = renderErrorBlock({
      title: "Failed to load dynamic bundles",
      detail: err.message || "Unknown error",
      retryFn: "window.location.hash='#dynamic-bundles'",
    });
  }
}

/* ------------------------------------------------------------------ */
/*  Render Functions                                                  */
/* ------------------------------------------------------------------ */

function renderFullPage(container) {
  const hasSelected = state.selectedId != null;
  container.innerHTML = `
    <div class="page-header-row">
      <h1><i class="fas fa-filter-list"></i> Dynamic Bundles</h1>
      <button class="btn btn-sm btn-primary" id="db-new-btn">
        <i class="fas fa-plus"></i> New Dynamic Bundle
      </button>
    </div>
    <div class="db-layout">
      <div class="db-list" id="db-list-panel">
        <div style="margin-bottom:var(--space-3);">
          <div class="tag-search-wrap" style="position:relative;">
            <i class="fas fa-search" style="position:absolute;left:10px;top:50%;transform:translateY(-50%);color:var(--text-muted);font-size:0.8rem;z-index:1;"></i>
            <input type="text" class="input-text input-search" id="db-search" placeholder="Filter bundles…" autocomplete="off">
          </div>
        </div>
        <div id="db-bundle-list">
          ${renderBundleList()}
        </div>
      </div>
      <div class="db-edit" id="db-edit-panel">
        ${
          hasSelected
            ? renderBundleDetail()
            : renderEmpty({
                icon: "filter-list",
                title: "Select or create a bundle",
                message:
                  "Choose a dynamic bundle from the left panel or create a new one to configure its filter criteria.",
              })
        }
      </div>
    </div>
  `;
}

function renderBundleList() {
  if (state.bundles.length === 0) {
    return `<div style="padding:1rem 0;color:var(--text-muted);font-size:0.85rem;">No dynamic bundles yet. Create one to get started.</div>`;
  }

  const search = state.tagSearch.toLowerCase();
  const filtered = search
    ? state.bundles.filter((b) => b.name.toLowerCase().includes(search))
    : state.bundles;

  return filtered
    .map((b) => {
      const active = b.id === state.selectedId ? " active" : "";
      const fileCount = b.matchingFileCount ?? 0;
      return `
        <div class="db-card${active}" data-bundle-id="${b.id}">
          <div style="display:flex;align-items:center;gap:0.5rem;min-width:0;">
            <div style="flex:1;min-width:0;">
              <div class="db-card-name">${escapeHtml(b.name)}</div>
              <div class="db-card-meta">
                <span><i class="fas fa-file-audio"></i> ${fileCount} file${fileCount !== 1 ? "s" : ""}</span>
                ${b.tagBackpack ? '<span><i class="fas fa-backpack" style="color:var(--accent);" title="In backpack"></i></span>' : ""}
              </div>
            </div>
            ${b.tagBackpack ? '<i class="fas fa-backpack" style="color:var(--accent);font-size:0.85rem;"></i>' : ""}
          </div>
        </div>
      `;
    })
    .join("");
}

function renderBundleDetail() {
  return `
    <!-- Name -->
    <div class="db-edit-section">
      <div class="db-edit-label">Name</div>
      <input type="text" class="input-text w-full" id="db-edit-name" value="${escapeHtml(state.editName)}" placeholder="e.g. Hard Techno 140-160">
    </div>

    <!-- Base Tags -->
    <div class="db-edit-section">
      <div class="db-edit-label">Base</div>
      <div style="display:flex;gap:var(--space-4);margin-bottom:var(--space-3);">
        <label style="display:flex;align-items:center;gap:6px;font-size:0.85rem;cursor:pointer;">
          <input type="radio" name="db-base-mode" value="all" ${state.editAllTracks ? "checked" : ""}>
          All tracks
        </label>
        <label style="display:flex;align-items:center;gap:6px;font-size:0.85rem;cursor:pointer;">
          <input type="radio" name="db-base-mode" value="tags" ${!state.editAllTracks ? "checked" : ""}>
          Specific tags
        </label>
      </div>
      <div id="db-base-tags-section" style="${state.editAllTracks ? "display:none;" : ""}">
        <div id="db-base-tag-chips" style="display:flex;flex-wrap:wrap;gap:0.25rem;margin-bottom:var(--space-3);min-height:28px;">
          ${
            state.editBaseTags.length > 0
              ? state.editBaseTags.map((t) => renderBaseTagChip(t)).join("")
              : '<span style="font-size:0.85rem;color:var(--text-subtle);">No tags selected. Type to search and add tags below.</span>'
          }
        </div>
        <div style="display:flex;gap:6px;position:relative;">
          <div class="typeahead-wrap" style="flex:1;position:relative;">
            <input type="text" class="input-text w-full" id="db-base-tag-search" placeholder="Search tags to add…" autocomplete="off" style="font-size:0.85rem;">
            <div id="db-base-tag-dropdown" class="typeahead-dropdown" style="display:none;position:absolute;top:100%;left:0;right:0;max-height:220px;overflow-y:auto;background:var(--bg);border:1px solid var(--border);border-top:none;border-radius:0 0 var(--radius-md) var(--radius-md);z-index:100;box-shadow:0 8px 24px rgba(0,0,0,0.3);"></div>
          </div>
          <button class="btn btn-sm btn-primary" id="db-add-base-tag-btn" disabled style="white-space:nowrap;"><i class="fas fa-plus"></i> Add</button>
        </div>
      </div>
    </div>

    <!-- BPM Range -->
    <div class="db-edit-section">
      <div class="db-edit-label">BPM Range</div>
      <div style="display:flex;gap:var(--space-3);align-items:center;">
        <input type="number" class="input-text" id="db-edit-bpm-min" value="${state.editBpmMin != null ? state.editBpmMin : ""}" placeholder="Min" min="0" max="300" style="width:100px;">
        <span style="color:var(--text-muted);">to</span>
        <input type="number" class="input-text" id="db-edit-bpm-max" value="${state.editBpmMax != null ? state.editBpmMax : ""}" placeholder="Max" min="0" max="300" style="width:100px;">
      </div>
    </div>

    <!-- Key Filter -->
    <div class="db-edit-section">
      <div class="db-edit-label">Key</div>
      <div style="font-size:0.85rem;">
        <div class="key-grid" data-key-row="minor" style="display:flex;flex-wrap:wrap;gap:4px;margin-bottom:6px;">
          ${MINOR_KEYS.map((k) => `<button class="key-btn minor${(state.editKeys || []).includes(k) ? " active" : ""}" data-key="${k}">${k}</button>`).join("")}
          <button class="key-btn action" data-key-action="minor-all">ALL m</button>
          <button class="key-btn action" data-key-action="minor-none">NONE m</button>
        </div>
        <div class="key-grid" data-key-row="major" style="display:flex;flex-wrap:wrap;gap:4px;">
          ${MAJOR_KEYS.map((k) => `<button class="key-btn major${(state.editKeys || []).includes(k) ? " active" : ""}" data-key="${k}">${k}</button>`).join("")}
          <button class="key-btn action" data-key-action="major-all">ALL d</button>
          <button class="key-btn action" data-key-action="major-none">NONE d</button>
        </div>
      </div>
    </div>

    <!-- PMV Categories -->
    <div class="db-edit-section">
      <div class="db-edit-label">
        Tag Categories
        <span style="font-size:0.75rem;color:var(--text-muted);font-weight:400;text-transform:none;letter-spacing:0;">
          (P=Phase &middot; M=Mood &middot; V=Vibe)
        </span>
      </div>
      <div style="display:flex;gap:var(--space-4);flex-wrap:wrap;">
        ${["P", "M", "V"]
          .map((cat) => {
            const checked = state.editPmvCategories.includes(cat.toLowerCase());
            return `
            <label style="display:flex;align-items:center;gap:6px;font-size:0.85rem;cursor:pointer;">
              <input type="checkbox" class="db-pmv-checkbox" data-pmv="${cat.toLowerCase()}" ${checked ? "checked" : ""}>
              ${cat}
            </label>
          `;
          })
          .join("")}
      </div>
    </div>

    <!-- Rating / Play Count -->
    <div style="display:flex;gap:var(--space-4);">
      <div class="db-edit-section" style="flex:1;">
        <div class="db-edit-label">Min Rating</div>
        <input type="number" class="input-text" id="db-edit-rating-min" value="${state.editRatingMin ?? ""}" placeholder="0" min="0" max="5" style="width:80px;">
      </div>
      <div class="db-edit-section" style="flex:1;">
        <div class="db-edit-label">Min Plays</div>
        <input type="number" class="input-text" id="db-edit-play-count-min" value="${state.editPlayCountMin ?? ""}" placeholder="0" min="0" style="width:80px;">
      </div>
    </div>

    <!-- Track Preview -->
    <div class="db-edit-section">
      <div class="db-edit-label" style="display:flex;align-items:center;gap:var(--space-3);">
        <span>Track Preview</span>
        <button class="btn btn-sm btn-ghost" id="db-refresh-preview-btn" ${state.previewLoading ? "disabled" : ""}>
          <i class="fas fa-sync${state.previewLoading ? " fa-spin" : ""}"></i> Refresh
        </button>
      </div>
      <div id="db-preview-container">
        ${renderPreview()}
      </div>
    </div>

    <!-- Action Buttons -->
    <div style="display:flex;gap:var(--space-3);padding-top:var(--space-4);border-top:1px solid var(--border);">
      <button class="btn btn-primary" id="db-save-btn" ${state.saving ? "disabled" : ""}>
        ${state.saving ? '<i class="fas fa-spinner fa-spin"></i> Saving…' : '<i class="fas fa-save"></i> Save'}
      </button>
      <button class="btn btn-danger" id="db-delete-btn" ${state.saving ? "disabled" : ""}>
        <i class="fas fa-trash"></i> Delete
      </button>
    </div>
  `;
}

function renderBaseTagChip(tag) {
  return `
    <span class="tag-chip" data-base-tag-name="${escapeHtml(tag)}">
      ${escapeHtml(tag)}
      <span class="tag-chip-x" data-base-tag-name="${escapeHtml(tag)}" title="Remove">×</span>
    </span>
  `;
}

function renderTypeaheadDropdown(results, selectedIndex) {
  if (results.length === 0) {
    return `<div style="padding:var(--space-3);color:var(--text-subtle);font-size:0.85rem;text-align:center;">No tags found</div>`;
  }
  return results
    .map((tag, idx) => {
      const highlighted = idx === selectedIndex ? " highlighted" : "";
      return `
        <div class="tag-dropdown-item${highlighted}" data-index="${idx}" data-tag-name="${escapeHtml(tag.name)}">
          <span style="display:flex;align-items:center;gap:0.5rem;">
            <span style="font-size:0.75rem;color:var(--text-muted);white-space:nowrap;">${escapeHtml(tag.categoryName || "")}</span>
            <span>${escapeHtml(tag.name)}</span>
          </span>
          <span style="font-size:0.75rem;color:var(--accent);white-space:nowrap;">→ Add</span>
        </div>
      `;
    })
    .join("");
}

function renderPreview() {
  if (state.previewLoading) {
    return renderLoading("Loading preview…");
  }
  if (!state.previewTracks || state.previewTracks.length === 0) {
    return '<span style="font-size:0.85rem;color:var(--text-subtle);">No matching tracks yet. Save the bundle then refresh preview.</span>';
  }
  return `
    <table class="db-preview-table">
      <thead>
        <tr>
          <th>#</th>
          <th>Track</th>
          <th>Artist</th>
          <th>BPM</th>
          <th>Key</th>
          <th>File</th>
        </tr>
      </thead>
      <tbody>
        ${state.previewTracks
          .slice(0, 20)
          .map(
            (t, i) => `
              <tr>
                <td style="color:var(--text-muted);text-align:center;">${i + 1}</td>
                <td>${escapeHtml(t.title || "—")}</td>
                <td>${escapeHtml(t.artist || "—")}</td>
                <td>${t.bpm != null ? t.bpm : "—"}</td>
                <td>${escapeHtml(t.musicalKey || "—")}</td>
                <td style="font-size:0.8rem;color:var(--text-muted);">${escapeHtml(t.fileType || "—")}</td>
              </tr>
            `,
          )
          .join("")}
      </tbody>
    </table>
  `;
}

/* ------------------------------------------------------------------ */
/*  Wire Events                                                       */
/* ------------------------------------------------------------------ */

function wireEvents(container, signal) {
  // ── Search bundles ──
  const searchInput = container.querySelector("#db-search");
  if (searchInput) {
    let timer = null;
    searchInput.addEventListener("input", () => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        state.tagSearch = searchInput.value.trim();
        const list = container.querySelector("#db-bundle-list");
        if (list) {
          list.innerHTML = renderBundleList();
        }
      }, 150);
    });
  }

  // ── Bundle list item click (delegated) ──
  const listPanel = container.querySelector("#db-bundle-list");
  if (listPanel) {
    listPanel.addEventListener("click", (e) => {
      const card = e.target.closest(".db-card");
      if (!card) return;
      const id = parseInt(card.dataset.bundleId, 10);
      if (id !== state.selectedId) {
        selectBundle(container, id);
      }
    });
  }

  // ── New bundle button ──
  const newBtn = container.querySelector("#db-new-btn");
  if (newBtn) {
    newBtn.addEventListener("click", () => {
      resetForm();
      state.selectedId = "new";
      const detail = container.querySelector("#db-edit-panel");
      if (detail) {
        detail.innerHTML = renderBundleDetail();
      }
      wireEditFormEvents(container, signal);
      // Clear any active selection in the list
      const list = container.querySelector("#db-bundle-list");
      if (list) {
        list
          .querySelectorAll(".db-card.active")
          .forEach((el) => el.classList.remove("active"));
      }
    });
  }

  // ── Edit form events ──
  wireEditFormEvents(container, signal);

  if (signal) {
    signal.addEventListener("abort", () => {
      // Cleanup handled by re-init on page change
    });
  }
}

function wireEditFormEvents(container, signal) {
  if (!container.querySelector("#db-edit-panel")) return;

  // ── Base mode radio buttons ──
  const radios = container.querySelectorAll('input[name="db-base-mode"]');
  radios.forEach((radio) => {
    radio.addEventListener("change", () => {
      state.editAllTracks = radio.value === "all";
      const tagsSection = container.querySelector("#db-base-tags-section");
      if (tagsSection) {
        tagsSection.style.display = state.editAllTracks ? "none" : "";
      }
    });
  });

  // ── Name input ──
  const nameInput = container.querySelector("#db-edit-name");
  if (nameInput) {
    nameInput.addEventListener("input", () => {
      state.editName = nameInput.value.trim();
    });
  }

  // ── BPM inputs ──
  const bpmMin = container.querySelector("#db-edit-bpm-min");
  if (bpmMin) {
    bpmMin.addEventListener("input", () => {
      state.editBpmMin = bpmMin.value !== "" ? parseFloat(bpmMin.value) : null;
    });
  }
  const bpmMax = container.querySelector("#db-edit-bpm-max");
  if (bpmMax) {
    bpmMax.addEventListener("input", () => {
      state.editBpmMax = bpmMax.value !== "" ? parseFloat(bpmMax.value) : null;
    });
  }

  // ── PMV checkboxes ──
  const pmvBoxes = container.querySelectorAll(".db-pmv-checkbox");
  pmvBoxes.forEach((box) => {
    box.addEventListener("change", () => {
      const pmv = box.dataset.pmv;
      if (box.checked) {
        if (!state.editPmvCategories.includes(pmv)) {
          state.editPmvCategories.push(pmv);
        }
      } else {
        state.editPmvCategories = state.editPmvCategories.filter((p) => p !== pmv);
      }
    });
  });

  // ── Key buttons ──
  const keyGrids = container.querySelectorAll(".key-grid");
  keyGrids.forEach((grid) => {
    grid.addEventListener("click", (e) => {
      const btn = e.target.closest(".key-btn");
      if (!btn) return;

      const action = btn.dataset.keyAction;
      if (action === "minor-all") {
        state.editKeys = [
          ...state.editKeys.filter((k) => !k.endsWith("m")),
          ...MINOR_KEYS,
        ];
      } else if (action === "minor-none") {
        state.editKeys = state.editKeys.filter((k) => !k.endsWith("m"));
      } else if (action === "major-all") {
        state.editKeys = [
          ...state.editKeys.filter((k) => !k.endsWith("d")),
          ...MAJOR_KEYS,
        ];
      } else if (action === "major-none") {
        state.editKeys = state.editKeys.filter((k) => !k.endsWith("d"));
      } else {
        const key = btn.dataset.key;
        if (!key) return;
        const idx = state.editKeys.indexOf(key);
        if (idx >= 0) {
          state.editKeys.splice(idx, 1);
        } else {
          state.editKeys.push(key);
        }
      }

      // Update all key button active classes
      container.querySelectorAll(".key-btn[data-key]").forEach((kb) => {
        kb.classList.toggle("active", state.editKeys.includes(kb.dataset.key));
      });
    });
  });

  // ── Rating min input ──
  const ratingMin = container.querySelector("#db-edit-rating-min");
  if (ratingMin) {
    ratingMin.addEventListener("input", () => {
      const v = ratingMin.value.trim();
      state.editRatingMin = v !== "" ? parseInt(v, 10) : null;
    });
  }

  // ── Play count min input ──
  const playCountMin = container.querySelector("#db-edit-play-count-min");
  if (playCountMin) {
    playCountMin.addEventListener("input", () => {
      const v = playCountMin.value.trim();
      state.editPlayCountMin = v !== "" ? parseInt(v, 10) : null;
    });
  }

  // ── Base tag typeahead ──
  wireBaseTagTypeahead(container, signal);

  // ── Base tag chip removal (delegated) ──
  const chipsContainer = container.querySelector("#db-base-tag-chips");
  if (chipsContainer) {
    chipsContainer.addEventListener("click", (e) => {
      const xBtn = e.target.closest(".tag-chip-x");
      if (!xBtn) return;
      const tagName = xBtn.dataset.baseTagName;
      if (!tagName) return;
      state.editBaseTags = state.editBaseTags.filter((t) => t !== tagName);
      renderBaseTagChips(container);
    });
  }

  // ── Refresh preview button ──
  const refreshBtn = container.querySelector("#db-refresh-preview-btn");
  if (refreshBtn) {
    refreshBtn.addEventListener("click", () => {
      fetchPreview(container);
    });
  }

  // ── Save button ──
  const saveBtn = container.querySelector("#db-save-btn");
  if (saveBtn) {
    saveBtn.addEventListener("click", () => {
      saveBundle(container);
    });
  }

  // ── Delete button ──
  const deleteBtn = container.querySelector("#db-delete-btn");
  if (deleteBtn) {
    deleteBtn.addEventListener("click", () => {
      deleteBundle(container);
    });
  }
}

/* ------------------------------------------------------------------ */
/*  Base Tag Typeahead                                                */
/* ------------------------------------------------------------------ */

function wireBaseTagTypeahead(container, signal) {
  const input = container.querySelector("#db-base-tag-search");
  const dropdown = container.querySelector("#db-base-tag-dropdown");
  const addBtn = container.querySelector("#db-add-base-tag-btn");
  if (!input || !dropdown) return;

  let debounceTimer = null;

  const doSearch = async () => {
    const q = input.value.trim();
    if (q.length < 1) {
      closeBaseTagTypeahead(container);
      if (addBtn) addBtn.disabled = true;
      return;
    }

    try {
      const resp = await fetchJSON(
        `/api/tags?search=${encodeURIComponent(q)}&page_size=20`,
        { signal },
      );
      const results = (resp.data || []).filter(
        (t) =>
          !state.editBaseTags.some((bt) => bt.toLowerCase() === t.name.toLowerCase()),
      );
      state.typeaheadResults = results;
      state.typeaheadIndex = results.length > 0 ? 0 : -1;
      dropdown.innerHTML = renderTypeaheadDropdown(results, state.typeaheadIndex);
      dropdown.style.display = results.length > 0 ? "block" : "none";
      if (addBtn) addBtn.disabled = results.length === 0;
    } catch (err) {
      if (err.name === "AbortError") return;
    }
  };

  input.addEventListener("input", () => {
    clearTimeout(debounceTimer);
    if (input.value.trim().length === 0) {
      closeBaseTagTypeahead(container);
      if (addBtn) addBtn.disabled = true;
      return;
    }
    debounceTimer = setTimeout(doSearch, 200);
  });

  // Click on dropdown item
  dropdown.addEventListener("click", (e) => {
    const item = e.target.closest(".tag-dropdown-item");
    if (item) {
      selectBaseTagTypeaheadItem(container, item);
    }
  });

  // Keyboard navigation
  input.addEventListener("keydown", (e) => {
    if (!dropdown || dropdown.style.display === "none") return;
    const items = dropdown.querySelectorAll(".tag-dropdown-item");

    if (e.key === "ArrowDown") {
      e.preventDefault();
      state.typeaheadIndex = Math.min(state.typeaheadIndex + 1, items.length - 1);
      highlightTypeaheadItem(dropdown, state.typeaheadIndex);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      state.typeaheadIndex = Math.max(state.typeaheadIndex - 1, 0);
      highlightTypeaheadItem(dropdown, state.typeaheadIndex);
    } else if (e.key === "Enter") {
      e.preventDefault();
      const item = items[state.typeaheadIndex];
      if (item) {
        selectBaseTagTypeaheadItem(container, item);
      }
    } else if (e.key === "Escape") {
      closeBaseTagTypeahead(container);
      input.blur();
    }
  });

  // Close on outside click
  document.addEventListener("click", (e) => {
    if (
      !e.target.closest("#db-base-tag-search") &&
      !e.target.closest("#db-base-tag-dropdown")
    ) {
      closeBaseTagTypeahead(container);
    }
  });

  // Add button
  if (addBtn) {
    addBtn.addEventListener("click", () => {
      const highlighted = dropdown.querySelector(".tag-dropdown-item.highlighted");
      if (highlighted) {
        selectBaseTagTypeaheadItem(container, highlighted);
      }
    });
  }
}

function highlightTypeaheadItem(dropdown, idx) {
  const items = dropdown.querySelectorAll(".tag-dropdown-item");
  items.forEach((el, i) => {
    el.classList.toggle("highlighted", i === idx);
  });
}

function selectBaseTagTypeaheadItem(container, item) {
  const name = item.dataset.tagName;
  if (!name) return;

  // Add to base tags if not already present
  if (!state.editBaseTags.some((t) => t.toLowerCase() === name.toLowerCase())) {
    state.editBaseTags.push(name);
  }

  const input = container.querySelector("#db-base-tag-search");
  if (input) {
    input.value = "";
  }
  closeBaseTagTypeahead(container);
  renderBaseTagChips(container);
}

function closeBaseTagTypeahead(container) {
  const dropdown = container.querySelector("#db-base-tag-dropdown");
  const addBtn = container.querySelector("#db-add-base-tag-btn");
  if (dropdown) dropdown.style.display = "none";
  if (addBtn) addBtn.disabled = true;
  state.typeaheadResults = [];
  state.typeaheadIndex = -1;
  state.typeaheadOpen = false;
}

function renderBaseTagChips(container) {
  const chipsEl = container.querySelector("#db-base-tag-chips");
  if (!chipsEl) return;
  if (state.editBaseTags.length > 0) {
    chipsEl.innerHTML = `<div style="display:flex;flex-wrap:wrap;gap:0.25rem;">${state.editBaseTags.map(renderBaseTagChip).join("")}</div>`;
  } else {
    chipsEl.innerHTML =
      '<span style="font-size:0.85rem;color:var(--text-subtle);">No tags selected. Type to search and add tags below.</span>';
  }
}

/* ------------------------------------------------------------------ */
/*  Data Operations                                                   */
/* ------------------------------------------------------------------ */

async function selectBundle(container, bundleId) {
  state.selectedId = bundleId;
  state.saving = false;

  // Update list active highlight
  const list = container.querySelector("#db-bundle-list");
  if (list) {
    list.querySelectorAll(".db-card").forEach((el) => {
      el.classList.toggle("active", parseInt(el.dataset.bundleId, 10) === bundleId);
    });
  }

  // Show edit panel with loading
  const detail = container.querySelector("#db-edit-panel");
  if (detail) {
    detail.innerHTML = renderLoading("Loading bundle…");
  }

  try {
    const resp = await fetchJSON(`/api/dynamic-bundles/${bundleId}`);
    const b = resp.data;
    if (!b) throw new Error("Bundle not found");

    state.editName = b.name || "";
    state.editAllTracks = b.includeAllTracks || false;
    state.editBaseTags = b.baseTags
      ? Array.isArray(b.baseTags)
        ? b.baseTags
        : JSON.parse(b.baseTags)
      : [];
    state.editBpmMin = b.bpmMin ?? null;
    state.editBpmMax = b.bpmMax ?? null;
    state.editPmvCategories = b.pmvCategories
      ? Array.isArray(b.pmvCategories)
        ? b.pmvCategories
        : JSON.parse(b.pmvCategories)
      : [];
    state.editKeys = b.keys ? (Array.isArray(b.keys) ? b.keys : JSON.parse(b.keys)) : [];
    state.editRatingMin = b.ratingMin ?? null;
    state.editPlayCountMin = b.playCountMin ?? null;

    // Re-render detail
    if (detail) {
      detail.innerHTML = renderBundleDetail();
    }
    wireEditFormEvents(container, null);

    // Fetch preview
    fetchPreview(container);
  } catch (err) {
    showToast(`Failed to load bundle: ${err.message}`, "error");
    if (detail) {
      detail.innerHTML = renderErrorBlock({
        title: "Failed to load bundle",
        detail: err.message,
        retryFn: `window.location.hash='#dynamic-bundles'`,
      });
    }
  }
}

async function saveBundle(container) {
  if (state.saving) return;
  if (!state.editName) {
    showToast("Bundle name is required", "error");
    return;
  }

  state.saving = true;
  const saveBtn = container.querySelector("#db-save-btn");
  if (saveBtn) {
    saveBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Saving…';
    saveBtn.disabled = true;
  }

  const isNew = state.selectedId === "new";
  const body = {
    name: state.editName,
    includeAllTracks: state.editAllTracks,
    baseTags: state.editAllTracks
      ? null
      : state.editBaseTags.length > 0
        ? state.editBaseTags
        : null,
    bpmMin: state.editBpmMin,
    bpmMax: state.editBpmMax,
    pmvCategories: state.editPmvCategories.length > 0 ? state.editPmvCategories : null,
    keys: state.editKeys.length > 0 ? state.editKeys : null,
    ratingMin: state.editRatingMin,
    playCountMin: state.editPlayCountMin,
  };

  try {
    let resp;
    if (isNew) {
      resp = await fetchJSON("/api/dynamic-bundles", {
        method: "POST",
        body: JSON.stringify(body),
      });
    } else {
      resp = await fetchJSON(`/api/dynamic-bundles/${state.selectedId}`, {
        method: "PUT",
        body: JSON.stringify(body),
      });
    }

    showToast(`Bundle "${state.editName}" ${isNew ? "created" : "updated"}`, "success");

    // Refresh bundle list
    await refreshBundleList(container);

    // Select the saved bundle
    const savedBundle = resp.data;
    if (savedBundle && savedBundle.id) {
      await selectBundle(container, savedBundle.id);
    }
  } catch (err) {
    showToast(`Failed to save bundle: ${err.message}`, "error");
  } finally {
    state.saving = false;
    if (saveBtn) {
      saveBtn.innerHTML = '<i class="fas fa-save"></i> Save';
      saveBtn.disabled = false;
    }
  }
}

async function deleteBundle(container) {
  if (state.saving) return;
  if (state.selectedId === "new") return;

  showModal({
    title: "Delete Dynamic Bundle",
    bodyHtml: `
      <div style="padding:var(--space-6);">
        <p style="margin-bottom:var(--space-4);">Are you sure you want to delete the bundle <strong>${escapeHtml(state.editName)}</strong>?</p>
        <p style="font-size:0.85rem;color:var(--text-subtle);">This will also delete the associated tag and all its file entries.</p>
      </div>
      <div class="modal-actions" style="padding:0 var(--space-6) var(--space-6);">
        <button class="btn" data-modal-action="close">Cancel</button>
        <button class="btn btn-danger" data-modal-action="confirm-delete">Delete</button>
      </div>
    `,
    onAction: async (action, close) => {
      if (action !== "confirm-delete") return;
      try {
        await fetchJSON(`/api/dynamic-bundles/${state.selectedId}`, {
          method: "DELETE",
        });
        showToast(`Bundle "${state.editName}" deleted`, "info");
        close();

        // Remove from bundles list and reset
        state.bundles = state.bundles.filter((b) => b.id !== state.selectedId);
        state.selectedId = null;
        resetForm();
        renderFullPage(container);
        wireEvents(container, null);
      } catch (err) {
        showToast(`Failed to delete bundle: ${err.message}`, "error");
      }
    },
  });
}

async function fetchPreview(container) {
  if (state.selectedId == null || state.selectedId === "new") return;
  state.previewLoading = true;
  const previewContainer = container.querySelector("#db-preview-container");
  if (previewContainer) {
    previewContainer.innerHTML = renderPreview();
  }

  try {
    const resp = await fetchJSON(`/api/dynamic-bundles/${state.selectedId}/preview`);
    state.previewTracks = resp.data?.tracks || [];
    state.previewLoading = false;
    if (previewContainer) {
      previewContainer.innerHTML = renderPreview();
    }
  } catch (err) {
    state.previewLoading = false;
    if (previewContainer) {
      previewContainer.innerHTML =
        '<span style="font-size:0.85rem;color:var(--text-subtle);">Failed to load preview.</span>';
    }
    showToast(`Failed to fetch preview: ${err.message}`, "error");
  }
}

async function refreshBundleList(container) {
  try {
    const resp = await fetchJSON("/api/dynamic-bundles?limit=200");
    state.bundles = resp.data || [];
    const list = container.querySelector("#db-bundle-list");
    if (list) {
      list.innerHTML = renderBundleList();
    }
  } catch {
    // Non-critical — list will refresh on next page load
  }
}

function resetForm() {
  state.editName = "";
  state.editAllTracks = false;
  state.editBaseTags = [];
  state.editBpmMin = null;
  state.editBpmMax = null;
  state.editPmvCategories = [];
  state.editKeys = [];
  state.editRatingMin = null;
  state.editPlayCountMin = null;
  state.previewTracks = [];
  state.previewLoading = false;
}
