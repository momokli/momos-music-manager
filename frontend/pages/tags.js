/**
 * tags.js — Tags page module.
 *
 * Canonical CRUD blueprint: stable toolbar + server-side paginated table
 * with sort, search, category filter, page size selector, and hash sync.
 *
 * API shape:
 *   GET /api/tags?limit=25&offset=0&search=...&category=...&sort=name&order=asc
 *   GET /api/tags/count?search=...&category=...
 *   GET /api/tag-categories
 *   POST /api/tags
 *   PUT  /api/tags/{id}
 *   DELETE /api/tags/{id}
 */

import {
  escapeHtml,
  renderLoading,
  renderErrorBlock,
  showToast,
  showModal,
} from "../shared/components.js";
import { fetchJSON } from "../shared/api.js";
import { renderSearchInput, wireSearchFilter } from "../shared/search-filter.js";
import {
  getPageSize,
  renderPageSizeSelector,
  sortableTh,
  wireSortableHeaders,
  wirePageSizeSelector,
  updateHash,
  parseHash,
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
/*  Constants                                                         */
/* ------------------------------------------------------------------ */

const CATEGORY_INFO = {
  Phase: { color: "var(--purple)", icon: "fa-activity", prefix: "P" },
  Mood: { color: "var(--pink)", icon: "fa-heart", prefix: "M" },
  Vibe: { color: "var(--yellow)", icon: "fa-sparkles", prefix: "V" },
  Merkmal: { color: "var(--green)", icon: "fa-hash", prefix: "—" },
  Setlist: { color: "var(--text-muted)", icon: "fa-list-music", prefix: "—" },
};

const CATEGORY_OPTIONS = [
  { value: "", label: "All Categories" },
  ...Object.keys(CATEGORY_INFO).map((k) => ({ value: k, label: k })),
];

const TAGS_COLUMNS = [
  { id: "name", label: "Tag", sortable: true, sortKey: "name", defaultWidth: 350 },
  {
    id: "category",
    label: "Category",
    sortable: true,
    sortKey: "category",
    defaultWidth: 250,
  },
  {
    id: "files",
    label: "Files",
    sortable: true,
    sortKey: "file_count",
    defaultWidth: 150,
  },
  {
    id: "created",
    label: "Created",
    sortable: true,
    sortKey: "created_at",
    defaultWidth: 150,
  },
  { id: "backpack", label: "Backpack", sortable: false, defaultWidth: 60 },
  { id: "actions", label: "Actions", sortable: false, defaultWidth: 100 },
];

const TAGS_CELL_RENDERERS = {
  name: (t) => `<strong>${escapeHtml(t.name)}</strong>`,
  category: (t) => {
    const cat = CATEGORY_INFO[t.category] || CATEGORY_INFO.Setlist;
    return `<span style="color:${cat.color}"><i class="fas ${cat.icon} mr-1"></i>${escapeHtml(t.category)}</span>`;
  },
  files: (t) =>
    `<span class="font-mono" style="text-align:right">${(t.fileCount || 0).toLocaleString()}</span>`,
  created: (t) => {
    if (!t.createdAt) return '<span class="text-muted">—</span>';
    const d = new Date(t.createdAt * 1000);
    return `<span class="font-mono text-xs">${d.toLocaleDateString()}</span>`;
  },
  backpack: (t) => {
    const backpack = t.backpack ? true : false;
    const icon = backpack ? "fa-box" : "fa-box-open";
    const title = backpack
      ? "Backpack \u2014 files for this tag are kept offline"
      : "Not backpack \u2014 files may be pruned if backed up";
    return `<button class="btn btn-sm btn-icon backpack-toggle-btn"
      data-id="${t.id}" data-backpack="${backpack ? "1" : "0"}"
      title="${title}">
      <i class="fas ${icon}" style="${backpack ? "color:var(--primary)" : "color:var(--text-muted)"}"></i>
    </button>`;
  },
  actions: (t) => {
    const edit = `<button class="btn btn-sm btn-edit-tag" data-id="${t.id}" data-tag="${escapeHtml(t.name)}" title="Edit tag"><i class="fas fa-pencil-alt"></i></button>`;
    const del = `<button class="btn btn-sm btn-red btn-delete-tag" data-id="${t.id}" data-tag="${escapeHtml(t.name)}" title="Delete tag"><i class="fas fa-times"></i></button>`;
    return `${edit} ${del}`;
  },
};

const HASH_DEFAULTS = {
  sort: "",
  order: "asc",
  search: "",
  selectedCategories: "",
  page: 0,
};

const HASH_SCHEMA = {
  sort: { type: "string", default: "" },
  order: { type: "string", default: "asc" },
  search: { type: "string", default: "" },
  selectedCategories: { type: "string", default: "" },
  page: { type: "number", default: 0 },
};

/* ------------------------------------------------------------------ */
/*  Adapter                                                           */
/* ------------------------------------------------------------------ */

function adaptTag(t) {
  return {
    id: t.id,
    name: t.name,
    category: t.category,
    categoryIcon: t.categoryIcon,
    fileCount: t.fileCount || 0,
    createdAt: t.createdAt,
    backpack: t.backpack || false,
  };
}

/* ------------------------------------------------------------------ */
/*  Build API query params from state                                 */
/* ------------------------------------------------------------------ */

function buildParams(state) {
  const params = new URLSearchParams();
  params.set("limit", String(state.pageSize));
  params.set("offset", String(state.page * state.pageSize));
  if (state.sort) params.set("sort", state.sort);
  if (state.order) params.set("order", state.order);
  if (state.search) params.set("search", state.search);
  if (state.selectedCategories && state.selectedCategories.length > 0) {
    params.set("category", state.selectedCategories.join(","));
  }
  return params;
}

/* ------------------------------------------------------------------ */
/*  Render: stable toolbar (rendered once)                             */
/* ------------------------------------------------------------------ */

function renderToolbar(state) {
  const selected = state.selectedCategories || [];
  const categoryBtns = CATEGORY_OPTIONS.map(
    (cat) =>
      `<button class="filter-btn${selected.includes(cat.value) ? " active" : ""}" data-value="${escapeHtml(cat.value)}">${escapeHtml(cat.label)}</button>`,
  ).join("");

  return `<div class="filter-panel" id="tags-filter-panel">
    <div class="filter-panel-header">
      <span style="flex:1;font-weight:600"><i class="fas fa-tags"></i> Filters</span>
      <button class="filter-panel-toggle" id="tags-filter-toggle" title="Toggle filters">
        <i class="fas fa-chevron-up chevron"></i>
      </button>
    </div>
    <div class="filter-panel-body">
      <div class="filter-panel-scroll" style="display:grid;grid-template-columns:1fr 1fr;gap:var(--space-2) var(--space-4);">
        <div>
          <div class="filter-section-header" style="margin-top:0"><i class="fas fa-music"></i> Tag Info</div>
          <div class="filter-row">
            <span class="filter-row-label toggleable" data-filter="category">Category</span>
            <div class="filter-group" id="tags-category-filter" style="flex-wrap:wrap">
              ${categoryBtns}
            </div>
          </div>
        </div>
        <div>
          <div class="filter-section-header" style="margin-top:0"><i class="fas fa-tag"></i> Filter</div>
          <div class="filter-row" style="flex-wrap:wrap;gap:var(--space-2)">
            <div style="flex:1;min-width:180px">${renderSearchInput("tags", state.search)}</div>
            <button class="btn btn-primary" id="tags-new-btn" style="white-space:nowrap"><i class="fas fa-plus"></i> New Tag</button>
          </div>
        </div>
      </div>
    </div>
  </div>`;
}

/* ------------------------------------------------------------------ */
/*  Render: body (stats + table + pagination, re-rendered)            */
/* ------------------------------------------------------------------ */

function renderBody(data, state, totalCount) {
  const config = loadColumnConfig("tags", TAGS_COLUMNS);
  const visibleCount = config.filter((c) => c.visible).length;

  // Checkbox column
  const selectedSet = state.selectedTagIds || new Set();
  const allOnPageSelected = data.length > 0 && data.every((t) => selectedSet.has(t.id));
  const checkboxHeader =
    '<th class="col-checkbox"><input type="checkbox" class="tags-select-all" id="tags-select-all"' +
    (allOnPageSelected ? " checked" : "") +
    "></th>";

  const rows = data
    .map((t) => {
      const checked = selectedSet.has(t.id) ? " checked" : "";
      const cb =
        '<td class="col-checkbox"><input type="checkbox" class="tags-row-checkbox" data-tag-id="' +
        t.id +
        '"' +
        checked +
        "></td>";
      const cells = renderColumnCells(config, TAGS_COLUMNS, TAGS_CELL_RENDERERS, t);
      return `<tr>${cb}${cells}</tr>`;
    })
    .join("");

  const totalPages = Math.max(1, Math.ceil(totalCount / state.pageSize));
  const pageInfo = `Page ${state.page + 1} of ${totalPages}`;

  return `
    <div class="stats-row">
      <div class="stats-group">
        <button class="btn btn-sm btn-icon" id="tags-refresh" title="Refresh">
          <i class="fa-solid fa-rotate"></i>
        </button>
        <strong>${totalCount.toLocaleString()}</strong> tags
        <span style="margin:0 6px;color:var(--text-subtle);">\u00b7</span>
        <strong>${Object.keys(CATEGORY_INFO).length}</strong> categories
        ${renderPageSizeSelector(state.pageSize)}
        ${renderColumnConfigTrigger()}
        ${
          state.layoutMode
            ? '<button class="btn btn-sm btn-primary" id="tags-layout-btn" style="margin-left:8px"><i class="fas fa-check"></i> Done</button>'
            : '<button class="btn btn-sm" id="tags-layout-btn" style="margin-left:8px"><i class="fas fa-arrows-alt"></i> Modify Column Layout</button>'
        }
      </div>
    </div>
    <div class="table-wrap">
      <table class="data-table" id="tags-table">
        <thead>
          <tr>
            ${checkboxHeader}
            ${renderColumnHeaders(config, TAGS_COLUMNS, state, sortableTh)}
          </tr>
        </thead>
        <tbody>${rows || `<tr><td class="col-checkbox"></td><td colspan="${visibleCount}" class="text-center text-subtle" style="padding:32px">No tags match your filters</td></tr>`}</tbody>
      </table>
    </div>
    <div class="pagination" id="tags-pagination">
      <button class="pagination-btn" id="tags-prev"${state.page === 0 ? " disabled" : ""}>
        <i class="fas fa-chevron-left"></i>
      </button>
      <span class="pagination-info" id="tags-page-info">${pageInfo}</span>
      <button class="pagination-btn" id="tags-next"${state.page >= totalPages - 1 ? " disabled" : ""}>
        <i class="fas fa-chevron-right"></i>
      </button>
    </div>
  `;
}

/* ------------------------------------------------------------------ */
/*  Fetch + render loop                                                */
/* ------------------------------------------------------------------ */

async function fetchAndRender(container, signal, state) {
  const content = container.querySelector("#tags-content");
  if (!content) return;

  // Show spinner while loading
  content.innerHTML = renderLoading("Loading tags…");

  try {
    const [dataResp, countResp] = await Promise.all([
      fetchJSON(`/api/tags?${buildParams(state)}`, { signal }),
      fetchJSON(`/api/tags/count?${buildParams(state)}`, { signal }),
    ]);

    if (signal.aborted) return;

    const data = dataResp.data.map(adaptTag);
    const totalCount = countResp.data;

    content.innerHTML = renderBody(data, state, totalCount);

    // Wire events after content render
    wireContentEvents(container, signal, state);
    updateSelectionUI(container, state);
  } catch (err) {
    if (err.name === "AbortError") return;
    content.innerHTML = renderErrorBlock({
      title: "Failed to load tags",
      detail: err.message,
      retryFn: "window.location.hash='#tags'",
    });
  }
}

/* ------------------------------------------------------------------ */
/*  Event wiring (called after each renderBody)                        */
/* ------------------------------------------------------------------ */

function wireContentEvents(container, signal, state) {
  // ── Checkbox selection ──
  const selectAllCb = container.querySelector("#tags-select-all");
  if (selectAllCb) {
    selectAllCb.onclick = () => {
      const checked = selectAllCb.checked;
      const rowCbs = container.querySelectorAll(".tags-row-checkbox");
      rowCbs.forEach((cb) => {
        const tagId = parseInt(cb.dataset.tagId, 10);
        if (checked) state.selectedTagIds.add(tagId);
        else state.selectedTagIds.delete(tagId);
        cb.checked = checked;
      });
      updateSelectionUI(container, state);
    };
  }

  const rowCbs = container.querySelectorAll(".tags-row-checkbox");
  rowCbs.forEach((cb) => {
    cb.onclick = () => {
      const tagId = parseInt(cb.dataset.tagId, 10);
      if (cb.checked) state.selectedTagIds.add(tagId);
      else state.selectedTagIds.delete(tagId);
      const allCb = container.querySelector("#tags-select-all");
      if (allCb) {
        const allRowCbs = container.querySelectorAll(".tags-row-checkbox");
        allCb.checked =
          allRowCbs.length > 0 && Array.from(allRowCbs).every((rc) => rc.checked);
      }
      updateSelectionUI(container, state);
    };
  });

  const table = container.querySelector("#tags-table");
  if (table) {
    wireSortableHeaders(table, state, () => {
      updateHash("tags", state, HASH_DEFAULTS, HASH_SCHEMA);
      fetchAndRender(container, signal, state);
    });
  }

  wirePageSizeSelector(container, state, () => {
    updateHash("tags", state, HASH_DEFAULTS, HASH_SCHEMA);
    fetchAndRender(container, signal, state);
  });

  // Column config (resize, drag-reorder, config trigger)
  const colConfig = loadColumnConfig("tags", TAGS_COLUMNS);
  if (state.layoutMode) {
    wireColumnResize(container, "tags", TAGS_COLUMNS, colConfig);
    wireColumnDragReorder(container, "tags", TAGS_COLUMNS, colConfig, () => {
      fetchAndRender(container, signal, state);
    });
  }
  wireConfigTrigger(container, "tags", TAGS_COLUMNS, colConfig, () => {
    fetchAndRender(container, signal, state);
  });

  // Layout mode toggle
  const layoutBtn = container.querySelector("#tags-layout-btn");
  if (layoutBtn) {
    layoutBtn.addEventListener("click", () => {
      state.layoutMode = !state.layoutMode;
      document.body.classList.toggle("layout-mode", state.layoutMode);
      updateHash("tags", state, HASH_DEFAULTS, HASH_SCHEMA);
      fetchAndRender(container, signal, state);
    });
  }

  // Pagination prev/next
  const prevBtn = container.querySelector("#tags-prev");
  const nextBtn = container.querySelector("#tags-next");
  if (prevBtn) {
    prevBtn.addEventListener("click", () => {
      if (state.page > 0) {
        state.page--;
        updateHash("tags", state, HASH_DEFAULTS, HASH_SCHEMA);
        fetchAndRender(container, signal, state);
      }
    });
  }
  if (nextBtn) {
    nextBtn.addEventListener("click", () => {
      state.page++;
      updateHash("tags", state, HASH_DEFAULTS, HASH_SCHEMA);
      fetchAndRender(container, signal, state);
    });
  }
}

/* ------------------------------------------------------------------ */
/*  CRUD helpers                                                       */
/* ------------------------------------------------------------------ */

function showNewTagModal(categories, reloadFn) {
  const catOptions = categories
    .map((c) => `<option value="${c.id}">${escapeHtml(c.name)}</option>`)
    .join("");

  showModal({
    title: "New Tag",
    bodyHtml: `
      <div style="padding:var(--space-6);">
        <div class="form-group">
          <label>Tag Name</label>
          <input type="text" class="input-text w-full" id="new-tag-name" placeholder="Enter tag name">
        </div>
        <div class="form-group">
          <label>Category</label>
          <select class="input-text w-full" id="new-tag-category">
            ${catOptions}
          </select>
        </div>
      </div>
      <div class="modal-actions" style="padding:0 var(--space-6) var(--space-6)">
        <button class="btn" data-modal-action="close">Cancel</button>
        <button class="btn btn-primary" data-modal-action="save-tag">Create</button>
      </div>
    `,
    onAction: async (action, close) => {
      if (action !== "save-tag") return;
      const name = document.getElementById("new-tag-name")?.value.trim();
      const categoryId = parseInt(document.getElementById("new-tag-category")?.value, 10);
      if (!name) {
        showToast("Name is required", "error");
        return;
      }
      if (!categoryId) {
        showToast("Category is required", "error");
        return;
      }
      try {
        await fetchJSON("/api/tags", {
          method: "POST",
          body: JSON.stringify({ name, categoryId }),
        });
        showToast("Tag created", "success");
        close();
        reloadFn();
      } catch (err) {
        showToast(`Failed: ${err.message}`, "error");
      }
    },
  });
}

async function showEditTagModal(
  tagId,
  currentName,
  currentCategory,
  categories,
  reloadFn,
) {
  const isSetlist = currentCategory === "Setlist";

  // Fetch parent tags if this is a Setlist tag
  let parents = [];
  if (isSetlist) {
    try {
      const resp = await fetchJSON(`/api/tags/${tagId}/parents`);
      parents = resp.data || [];
    } catch (_) {
      // Ignore — parents section just won't show pre-loaded data
    }
  }

  const catOptions = categories
    .map(
      (c) =>
        `<option value="${c.id}"${c.name === currentCategory ? " selected" : ""}>${escapeHtml(c.name)}</option>`,
    )
    .join("");

  // Build parent tags section HTML (only for Setlist tags)
  let parentSectionHtml = "";
  if (isSetlist) {
    const chipHtml =
      parents.length > 0
        ? parents
            .map(
              (p) =>
                `<span class="tag-chip" data-parent-id="${p.id}" style="display:inline-flex;align-items:center;gap:4px;padding:2px 8px;background:var(--surface-2);border-radius:4px;font-size:0.85rem;margin:2px;">
                  <span class="category-badge" style="color:${(CATEGORY_INFO[p.category] || CATEGORY_INFO.Setlist).color};font-size:0.7rem;font-weight:700;">${p.category || "?"}</span>
                  ${escapeHtml(p.name)}
                  <button class="btn btn-sm btn-icon parent-remove-btn" data-parent-id="${p.id}" style="padding:0 4px;line-height:1;color:var(--text-subtle);" title="Remove parent"><i class="fas fa-times"></i></button>
                </span>`,
            )
            .join("")
        : `<span class="text-subtle" style="font-size:0.85rem;">No parent tags set. This long tag name will appear in comments as-is.</span>`;

    parentSectionHtml = `
      <div class="form-group" style="margin-top:var(--space-4);padding-top:var(--space-4);border-top:1px solid var(--border-color);">
        <label style="display:flex;align-items:center;gap:6px;">
          <i class="fas fa-sitemap"></i> Parent Tags
          <span class="text-subtle" style="font-weight:400;font-size:0.8rem;">(aliases used in comments)</span>
        </label>
        <div id="edit-tag-parents-chips" style="display:flex;flex-wrap:wrap;gap:2px;margin-bottom:8px;min-height:28px;">
          ${chipHtml}
        </div>
        <div style="display:flex;gap:6px;">
          <div class="typeahead-wrap" style="flex:1;position:relative;">
            <input type="text" class="input-text w-full" id="edit-tag-parent-search" placeholder="Search tags to add as parents…" autocomplete="off">
            <div id="edit-tag-parent-dropdown" class="typeahead-dropdown" style="display:none;position:absolute;top:100%;left:0;right:0;max-height:200px;overflow-y:auto;background:var(--bg);border:1px solid var(--border-color);border-radius:0 0 var(--radius) var(--radius);z-index:100;"></div>
          </div>
        </div>
        <div class="text-subtle" style="font-size:0.75rem;margin-top:4px;">
          Parent tags replace this tag in file comments. Each parent contributes its own category (P/M/V/E).
        </div>
      </div>`;
  }

  showModal({
    title: `Edit Tag`,
    width: isSetlist ? "560px" : "500px",
    bodyHtml: `
      <div style="padding:var(--space-6);">
        <div class="form-group">
          <label>Tag Name</label>
          <input type="text" class="input-text w-full" id="edit-tag-name" value="${escapeHtml(currentName)}" placeholder="Enter tag name">
        </div>
        <div class="form-group">
          <label>Category</label>
          <select class="input-text w-full" id="edit-tag-category">
            ${catOptions}
          </select>
        </div>
        ${parentSectionHtml}
      </div>
      <div class="modal-actions" style="padding:0 var(--space-6) var(--space-6)">
        <button class="btn" data-modal-action="close">Cancel</button>
        <button class="btn btn-primary" data-modal-action="save-tag">Save</button>
      </div>
    `,
    onAction: async (action, close) => {
      if (action !== "save-tag") return;
      const name = document.getElementById("edit-tag-name")?.value.trim();
      const categoryId = parseInt(
        document.getElementById("edit-tag-category")?.value,
        10,
      );
      if (!name) {
        showToast("Name is required", "error");
        return;
      }
      if (!categoryId) {
        showToast("Category is required", "error");
        return;
      }
      try {
        // Save tag metadata
        await fetchJSON(`/api/tags/${tagId}`, {
          method: "PUT",
          body: JSON.stringify({ name, categoryId }),
        });

        // Save parent tags (only if Setlist — collect from chips)
        if (isSetlist) {
          const chips = document.querySelectorAll(
            "#edit-tag-parents-chips .tag-chip[data-parent-id]",
          );
          const parentIds = Array.from(chips).map((c) =>
            parseInt(c.dataset.parentId, 10),
          );
          await fetchJSON(`/api/tags/${tagId}/parents`, {
            method: "PUT",
            body: JSON.stringify({ parentTagIds: parentIds }),
          });
        }

        showToast("Tag updated", "success");
        close();
        reloadFn();
      } catch (err) {
        showToast(`Failed: ${err.message}`, "error");
      }
    },
  });

  // Wire parent tag typeahead (only for Setlist tags)
  if (isSetlist) {
    wireParentTypeahead(tagId, parents);
  }
}

/** Wire the parent tag typeahead search + chip management */
function wireParentTypeahead(tagId, initialParents) {
  const searchInput = document.getElementById("edit-tag-parent-search");
  const dropdown = document.getElementById("edit-tag-parent-dropdown");
  const chipsContainer = document.getElementById("edit-tag-parents-chips");
  if (!searchInput || !dropdown || !chipsContainer) return;

  // Track parent IDs in a Set for quick lookup
  const parentIds = new Set(initialParents.map((p) => p.id));
  let debounceTimer = null;

  // Add a parent tag chip to the UI
  function addParentChip(tag) {
    if (parentIds.has(tag.id)) return;
    parentIds.add(tag.id);

    const catInfo = CATEGORY_INFO[tag.category] || CATEGORY_INFO.Setlist;
    const chip = document.createElement("span");
    chip.className = "tag-chip";
    chip.dataset.parentId = tag.id;
    chip.style.cssText =
      "display:inline-flex;align-items:center;gap:4px;padding:2px 8px;background:var(--surface-2);border-radius:4px;font-size:0.85rem;margin:2px;";
    chip.innerHTML = `
      <span class="category-badge" style="color:${catInfo.color};font-size:0.7rem;font-weight:700;">${tag.category || "?"}</span>
      ${escapeHtml(tag.name)}
      <button class="btn btn-sm btn-icon parent-remove-btn" data-parent-id="${tag.id}" style="padding:0 4px;line-height:1;color:var(--text-subtle);" title="Remove parent"><i class="fas fa-times"></i></button>
    `;

    // Wire remove button
    chip.querySelector(".parent-remove-btn").addEventListener("click", () => {
      parentIds.delete(tag.id);
      chip.remove();
      updatePlaceholder();
    });

    chipsContainer.appendChild(chip);
    updatePlaceholder();
  }

  // Show/hide the "no parents" placeholder
  function updatePlaceholder() {
    const existing = chipsContainer.querySelector(".text-subtle");
    if (parentIds.size === 0) {
      if (!existing) {
        const ph = document.createElement("span");
        ph.className = "text-subtle";
        ph.style.cssText = "font-size:0.85rem;";
        ph.textContent =
          "No parent tags set. This long tag name will appear in comments as-is.";
        chipsContainer.appendChild(ph);
      }
    } else {
      if (existing) existing.remove();
    }
  }

  // Wire remove on existing chips
  chipsContainer.querySelectorAll(".parent-remove-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      const pid = parseInt(btn.dataset.parentId, 10);
      parentIds.delete(pid);
      btn.closest(".tag-chip")?.remove();
      updatePlaceholder();
    });
  });

  // Typeahead search
  searchInput.addEventListener("input", () => {
    clearTimeout(debounceTimer);
    const q = searchInput.value.trim();
    if (q.length < 1) {
      dropdown.style.display = "none";
      return;
    }
    debounceTimer = setTimeout(async () => {
      try {
        const resp = await fetchJSON(
          `/api/tags?search=${encodeURIComponent(q)}&limit=10`,
        );
        const results = (resp.data || []).filter(
          (t) => t.id !== tagId && !parentIds.has(t.id),
        );
        if (results.length === 0) {
          dropdown.innerHTML =
            '<div style="padding:8px 12px;color:var(--text-subtle);font-size:0.85rem;">No matching tags</div>';
        } else {
          dropdown.innerHTML = results
            .map((t) => {
              const catInfo = CATEGORY_INFO[t.category] || CATEGORY_INFO.Setlist;
              return `<div class="typeahead-item" data-id="${t.id}" data-name="${escapeHtml(t.name)}" data-category="${escapeHtml(t.category || "")}" style="padding:6px 12px;cursor:pointer;display:flex;align-items:center;gap:6px;">
                <span class="category-badge" style="color:${catInfo.color};font-size:0.7rem;font-weight:700;">${t.category || "?"}</span>
                ${escapeHtml(t.name)}
              </div>`;
            })
            .join("");
        }
        dropdown.style.display = "block";
      } catch (_) {
        dropdown.style.display = "none";
      }
    }, 250);
  });

  // Click to select from dropdown
  dropdown.addEventListener("click", (e) => {
    const item = e.target.closest(".typeahead-item");
    if (!item) return;
    const id = parseInt(item.dataset.id, 10);
    const name = item.dataset.name;
    const category = item.dataset.category;
    addParentChip({ id, name, category });
    searchInput.value = "";
    dropdown.style.display = "none";
  });

  // Close dropdown on outside click
  document.addEventListener("click", (e) => {
    if (!searchInput.contains(e.target) && !dropdown.contains(e.target)) {
      dropdown.style.display = "none";
    }
  });

  // Keyboard: Enter to select first, Escape to close
  searchInput.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      dropdown.style.display = "none";
    } else if (e.key === "Enter") {
      const first = dropdown.querySelector(".typeahead-item");
      if (first) {
        const id = parseInt(first.dataset.id, 10);
        const name = first.dataset.name;
        const category = first.dataset.category;
        addParentChip({ id, name, category });
        searchInput.value = "";
        dropdown.style.display = "none";
      }
    }
  });
}

/* ------------------------------------------------------------------ */
/*  Selection + Bulk Actions                                           */
/* ------------------------------------------------------------------ */

function updateSelectionUI(container, state) {
  const count = state.selectedTagIds.size;
  updateSelectionCount(container, "tags", count);
  const btn = container.querySelector("#tags-actions-categorize");
  if (btn) {
    btn.disabled = count === 0;
  }
}

async function showCategorizeModal(container, state, signal) {
  // Fetch categories for the dropdown
  let categories = [];
  try {
    const resp = await fetchJSON("/api/tag-categories", { signal });
    categories = resp.data || [];
  } catch (_) {
    return;
  }

  const catOptions = categories
    .map((c) => `<option value="${c.id}">${escapeHtml(c.name)}</option>`)
    .join("");

  const tagCount = state.selectedTagIds.size;

  showModal({
    title: `Change Category (${tagCount} tag${tagCount !== 1 ? "s" : ""})`,
    bodyHtml: `
      <div style="padding:var(--space-6);">
        <p style="color:var(--text-muted);margin-bottom:var(--space-4);">
          Change the category for ${tagCount} selected tag${tagCount !== 1 ? "s" : ""}.
        </p>
        <div class="form-group">
          <label>New Category</label>
          <select class="input-text w-full" id="bulk-categorize-category">
            ${catOptions}
          </select>
        </div>
      </div>
      <div class="modal-actions" style="padding:0 var(--space-6) var(--space-6)">
        <button class="btn" data-modal-action="close">Cancel</button>
        <button class="btn btn-primary" data-modal-action="apply">Apply</button>
      </div>
    `,
    onAction: async (action, close) => {
      if (action !== "apply") return;
      const categoryId = parseInt(
        document.getElementById("bulk-categorize-category")?.value,
        10,
      );
      if (!categoryId) {
        showToast("Please select a category", "error");
        return;
      }
      const tagIds = Array.from(state.selectedTagIds);
      try {
        const resp = await fetchJSON("/api/tags/bulk-categorize", {
          method: "POST",
          body: JSON.stringify({ tagIds, categoryId }),
        });
        const updated = resp.data?.updated || 0;
        showToast(`${updated} tag${updated !== 1 ? "s" : ""} updated`, "success");
        state.selectedTagIds.clear();
        close();
        fetchAndRender(container, signal, state);
      } catch (err) {
        showToast(`Failed: ${err.message}`, "error");
      }
    },
  });
}

async function deleteTag(tagId, tagName, reloadFn) {
  if (!confirm(`Delete tag "${tagName}"? This cannot be undone.`)) return;

  try {
    await fetchJSON(`/api/tags/${tagId}`, { method: "DELETE" });
    showToast("Tag deleted", "success");
    reloadFn();
  } catch (err) {
    showToast(`Failed: ${err.message}`, "error");
  }
}

/* ------------------------------------------------------------------ */
/*  Page initialisation                                                */
/* ------------------------------------------------------------------ */

export async function init(container, signal, hashParams) {
  // Build state from hash params + global page size
  const parsed = parseHash(hashParams, HASH_SCHEMA);
  const hashCats = parsed.selectedCategories
    ? parsed.selectedCategories.split(",").filter(Boolean)
    : [];
  const state = {
    page: 0,
    pageSize: getPageSize(),
    search: "",
    sort: "",
    order: "asc",
    selectedCategories: hashCats,
    categoryEnabled: localStorage.getItem("filterRowState_tags_category") !== "false",
    ...parsed,
  };
  // Ensure selectedCategories is always an array
  if (!Array.isArray(state.selectedCategories)) {
    state.selectedCategories = [];
  }
  state.layoutMode = false;
  state.selectedTagIds = new Set();

  // Reset layout mode on page entry
  document.body.classList.remove("layout-mode");

  // Fetch categories once (they rarely change)
  let categories = [];
  try {
    const catsResp = await fetchJSON("/api/tag-categories", { signal });
    if (signal.aborted) return;
    categories = catsResp.data;
  } catch (err) {
    if (err.name === "AbortError") return;
  }

  // Render stable toolbar + actions panel + content wrapper (once)
  container.innerHTML = `
    <div style="display:flex;flex-direction:column;gap:var(--space-4);">
      <div style="display:flex;gap:var(--space-4);align-items:flex-start;">
        <div style="flex:4;min-width:0;">${renderToolbar(state)}</div>
        ${renderActionsPanel("tags", [
          {
            id: "categorize",
            label: "CHANGE CATEGORY",
            icon: "fas fa-tag",
            cls: "btn-primary",
            action: "categorize",
          },
        ])}
      </div>
      <div id="tags-content" style="min-height:200px;">${renderLoading("Loading tags…")}</div>
    </div>`;

  // Wire CHANGE CATEGORY button in actions panel
  const catBtn = container.querySelector("#tags-actions-categorize");
  if (catBtn) {
    catBtn.onclick = () => showCategorizeModal(container, state, signal);
    catBtn.disabled = true;
  }

  // Wire toolbar events (search + filter)
  const toolbar = container.querySelector("#tags-filter-panel");
  if (toolbar) {
    wireSearchFilter(toolbar, state, () => {
      updateHash("tags", state, HASH_DEFAULTS, HASH_SCHEMA);
      fetchAndRender(container, signal, state);
    });

    // ── Category multi-select toggle ──
    const catFilter = toolbar.querySelector("#tags-category-filter");

    function syncCategoryFilterUI() {
      if (!catFilter) return;
      const btns = catFilter.querySelectorAll(".filter-btn[data-value]");
      btns.forEach((btn) => {
        const val = btn.dataset.value;
        if (val === "") {
          // "All Categories" is active when nothing else is selected
          btn.classList.toggle("active", state.selectedCategories.length === 0);
        } else {
          btn.classList.toggle("active", state.selectedCategories.includes(val));
        }
      });
    }

    if (catFilter) {
      catFilter.addEventListener("click", (e) => {
        const btn = e.target.closest(".filter-btn[data-value]");
        if (!btn) return;
        const val = btn.dataset.value;
        const idx = state.selectedCategories.indexOf(val);
        if (idx >= 0) {
          state.selectedCategories.splice(idx, 1);
        } else {
          if (val === "") {
            // "All Categories" — clear selection
            state.selectedCategories = [];
          } else {
            // Remove "" if it's in the array (All Categories deselected)
            const allIdx = state.selectedCategories.indexOf("");
            if (allIdx >= 0) state.selectedCategories.splice(allIdx, 1);
            state.selectedCategories.push(val);
          }
        }
        state.page = 0;
        syncCategoryFilterUI();
        updateHash("tags", state, HASH_DEFAULTS, HASH_SCHEMA);
        fetchAndRender(container, signal, state);
      });
    }

    // ── Generic toggle for [data-filter] labels ──
    toolbar.querySelectorAll("[data-filter]").forEach((label) => {
      function updateFilterUI() {
        const key = label.dataset.filter + "Enabled";
        const isActive = state[key] !== false;
        label.classList.toggle("active", isActive);
        label.classList.toggle("off", !isActive);
        const row = label.closest(".filter-row");
        if (row) {
          const inputs = row.querySelectorAll(
            "select, input, button, .filter-group, .tag-chips, .dual-range-wrap, .key-grid-wrap, .typeahead-wrap",
          );
          inputs.forEach((el) => el.classList.toggle("filter-disabled", !isActive));
        }
      }
      label.addEventListener("click", () => {
        const key = label.dataset.filter + "Enabled";
        state[key] = state[key] === false ? true : false;
        localStorage.setItem("filterRowState_tags_" + label.dataset.filter, state[key]);
        state.page = 0;
        updateFilterUI();
        updateHash("tags", state, HASH_DEFAULTS, HASH_SCHEMA);
        fetchAndRender(container, signal, state);
      });
      updateFilterUI();
    });

    // ── Auto-enable disabled filter sections on click ──
    toolbar.addEventListener("click", (e) => {
      const row = e.target.closest(".filter-row");
      if (!row) return;
      const label = row.querySelector("[data-filter]");
      if (!label) return;
      const key = label.dataset.filter + "Enabled";
      if (state[key] !== false) return;
      if (e.target.closest("[data-filter]")) return;
      state[key] = true;
      localStorage.setItem("filterRowState_tags_" + label.dataset.filter, state[key]);
      state.page = 0;
      label.classList.add("active");
      label.classList.remove("off");
      row
        .querySelectorAll(
          "select, input, button, .filter-group, .tag-chips, .dual-range-wrap, .key-grid-wrap, .typeahead-wrap",
        )
        .forEach((el) => el.classList.remove("filter-disabled"));
      updateHash("tags", state, HASH_DEFAULTS, HASH_SCHEMA);
      fetchAndRender(container, signal, state);
    });
  }

  // Filter panel collapse toggle (persists to localStorage)
  const toggleBtn = container.querySelector("#tags-filter-toggle");
  const filterPanel = container.querySelector("#tags-filter-panel");
  if (toggleBtn && filterPanel) {
    const saved = localStorage.getItem("filterPanelCollapsed_tags");
    if (saved === "true") filterPanel.classList.add("collapsed");
    toggleBtn.addEventListener("click", () => {
      filterPanel.classList.toggle("collapsed");
      localStorage.setItem(
        "filterPanelCollapsed_tags",
        filterPanel.classList.contains("collapsed"),
      );
    });
  }

  // Wire actions panel refresh
  import("../shared/actions-panel.js").then(({ wireActionsRefresh }) => {
    wireActionsRefresh(container, "tags", () => {
      state.page = 0;
      return fetchAndRender(container, signal, state);
    });
  });

  // Wire global toolbar actions via event delegation
  container.addEventListener("click", (e) => {
    const refreshBtn = e.target.closest("#tags-refresh");
    if (refreshBtn) {
      fetchAndRender(container, signal, state);
      return;
    }

    const newBtn = e.target.closest("#tags-new-btn");
    if (newBtn) {
      showNewTagModal(categories, () => {
        fetchAndRender(container, signal, state);
      });
      return;
    }

    const editBtn = e.target.closest(".btn-edit-tag");
    if (editBtn) {
      const id = parseInt(editBtn.dataset.id, 10);
      const name = editBtn.dataset.tag;
      // We don't have the full tag object in the current page data,
      // but the edit modal just needs the category name.
      // Fetch the single tag to get current category.
      const doEdit = async () => {
        try {
          const resp = await fetchJSON(`/api/tags/${id}`, { signal });
          if (signal.aborted) return;
          const tag = resp.data;
          showEditTagModal(id, tag.name, tag.category || "Setlist", categories, () => {
            fetchAndRender(container, signal, state);
          });
        } catch (err) {
          if (err.name === "AbortError") return;
          showToast(`Failed to load tag: ${err.message}`, "error");
        }
      };
      doEdit();
      return;
    }

    const delBtn = e.target.closest(".btn-delete-tag");
    if (delBtn) {
      const id = parseInt(delBtn.dataset.id, 10);
      const name = delBtn.dataset.tag;
      deleteTag(id, name, () => {
        fetchAndRender(container, signal, state);
      });
      return;
    }

    const backpackBtn = e.target.closest(".backpack-toggle-btn");
    if (backpackBtn) {
      const tagId = parseInt(backpackBtn.dataset.id, 10);
      const currentBackpack = backpackBtn.dataset.backpack === "1";
      const newBackpack = !currentBackpack;

      backpackBtn.disabled = true;
      backpackBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i>';

      (async () => {
        try {
          await fetchJSON(`/api/tags/${tagId}/backpack`, {
            method: "PUT",
            body: JSON.stringify({ backpack: newBackpack }),
          });
          updateHash("tags", state, HASH_DEFAULTS, HASH_SCHEMA);
          fetchAndRender(container, signal, state);
        } catch (err) {
          showToast(`Backpack toggle failed: ${err.message}`, "error");
          backpackBtn.disabled = false;
          const icon = currentBackpack ? "fa-box" : "fa-box-open";
          backpackBtn.innerHTML = `<i class="fas ${icon}"></i>`;
          backpackBtn.dataset.backpack = currentBackpack ? "1" : "0";
        }
      })();
      return;
    }
  });

  // Initial data fetch
  updateHash("tags", state, HASH_DEFAULTS, HASH_SCHEMA);
  await fetchAndRender(container, signal, state);
}
