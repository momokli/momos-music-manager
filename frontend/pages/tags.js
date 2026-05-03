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
import {
  renderSearchInput,
  renderFilterGroup,
  wireSearchFilter,
} from "../shared/search-filter.js";
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
  { id: "name", label: "Tag", sortable: true, sortKey: "name", defaultWidth: 35 },
  {
    id: "category",
    label: "Category",
    sortable: true,
    sortKey: "category",
    defaultWidth: 25,
  },
  {
    id: "files",
    label: "Files",
    sortable: true,
    sortKey: "file_count",
    defaultWidth: 15,
  },
  {
    id: "created",
    label: "Created",
    sortable: true,
    sortKey: "created_at",
    defaultWidth: 15,
  },
  { id: "actions", label: "Actions", sortable: false, defaultWidth: 10 },
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
    if (!t.createdAt) return '<span class="text-muted">\u2014</span>';
    const d = new Date(t.createdAt * 1000);
    return `<span class="font-mono text-xs">${d.toLocaleDateString()}</span>`;
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
  category: "",
  page: 0,
};

const HASH_SCHEMA = {
  sort: { type: "string", default: "" },
  order: { type: "string", default: "asc" },
  search: { type: "string", default: "" },
  category: { type: "string", default: "" },
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
  if (state.category) params.set("category", state.category);
  return params;
}

/* ------------------------------------------------------------------ */
/*  Render: stable toolbar (rendered once)                             */
/* ------------------------------------------------------------------ */

function renderToolbar(state) {
  return `<div class="filter-panel" id="tags-filter-panel">
    <div class="filter-panel-header">
      ${renderSearchInput("tags", state.search)}
      ${renderFilterGroup("category", CATEGORY_OPTIONS, state.category)}
      <button class="btn btn-primary" id="tags-new-btn"><i class="fas fa-plus"></i> New Tag</button>
      <button class="filter-panel-toggle" id="tags-filter-toggle" title="Toggle filters">
        <i class="fas fa-chevron-up chevron"></i>
      </button>
    </div>
  </div>`;
}

/* ------------------------------------------------------------------ */
/*  Render: body (stats + table + pagination, re-rendered)            */
/* ------------------------------------------------------------------ */

function renderBody(data, state, totalCount) {
  const config = loadColumnConfig("tags", TAGS_COLUMNS);
  const visibleCount = config.filter((c) => c.visible).length;

  const rows = data
    .map((t) => {
      const cells = renderColumnCells(config, TAGS_COLUMNS, TAGS_CELL_RENDERERS, t);
      return `<tr>${cells}</tr>`;
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
      </div>
    </div>
    <div class="table-wrap">
      <table class="data-table" id="tags-table">
        <thead>
          <tr>
            ${renderColumnHeaders(config, TAGS_COLUMNS, state, sortableTh)}
          </tr>
        </thead>
        <tbody>${rows || `<tr><td colspan="${visibleCount}" class="text-center text-subtle" style="padding:32px">No tags match your filters</td></tr>`}</tbody>
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
  const table = container.querySelector("#tags-table");
  if (table) {
    wireSortableHeaders(table, state, () => {
      updateHash("tags", state, HASH_DEFAULTS);
      fetchAndRender(container, signal, state);
    });
  }

  wirePageSizeSelector(container, state, () => {
    updateHash("tags", state, HASH_DEFAULTS);
    fetchAndRender(container, signal, state);
  });

  // Column config (resize, drag-reorder, config trigger)
  const colConfig = loadColumnConfig("tags", TAGS_COLUMNS);
  wireColumnResize(container, "tags", TAGS_COLUMNS, colConfig);
  wireColumnDragReorder(container, "tags", TAGS_COLUMNS, colConfig, () => {
    fetchAndRender(container, signal, state);
  });
  wireConfigTrigger(container, "tags", TAGS_COLUMNS, colConfig, () => {
    fetchAndRender(container, signal, state);
  });

  // Pagination prev/next
  const prevBtn = container.querySelector("#tags-prev");
  const nextBtn = container.querySelector("#tags-next");
  if (prevBtn) {
    prevBtn.addEventListener("click", () => {
      if (state.page > 0) {
        state.page--;
        updateHash("tags", state, HASH_DEFAULTS);
        fetchAndRender(container, signal, state);
      }
    });
  }
  if (nextBtn) {
    nextBtn.addEventListener("click", () => {
      state.page++;
      updateHash("tags", state, HASH_DEFAULTS);
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

function showEditTagModal(tagId, currentName, currentCategory, categories, reloadFn) {
  const catOptions = categories
    .map(
      (c) =>
        `<option value="${c.id}"${c.name === currentCategory ? " selected" : ""}>${escapeHtml(c.name)}</option>`,
    )
    .join("");

  showModal({
    title: `Edit Tag`,
    width: "500px",
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
        await fetchJSON(`/api/tags/${tagId}`, {
          method: "PUT",
          body: JSON.stringify({ name, categoryId }),
        });
        showToast("Tag updated", "success");
        close();
        reloadFn();
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
  const state = {
    page: 0,
    pageSize: getPageSize(),
    search: "",
    sort: "",
    order: "asc",
    category: "",
    ...parseHash(hashParams, HASH_SCHEMA),
  };

  // Fetch categories once (they rarely change)
  let categories = [];
  try {
    const catsResp = await fetchJSON("/api/tag-categories", { signal });
    if (signal.aborted) return;
    categories = catsResp.data;
  } catch (err) {
    if (err.name === "AbortError") return;
  }

  // Render stable toolbar + content wrapper (once)
  container.innerHTML = `
    ${renderToolbar(state)}
    <div id="tags-content">${renderLoading("Loading tags…")}</div>
  `;

  // Wire toolbar events (search + category filter)
  const toolbar = container.querySelector("#tags-filter-panel");
  if (toolbar) {
    wireSearchFilter(toolbar, state, () => {
      updateHash("tags", state, HASH_DEFAULTS);
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
  });

  // Initial data fetch
  updateHash("tags", state, HASH_DEFAULTS);
  await fetchAndRender(container, signal, state);
}
