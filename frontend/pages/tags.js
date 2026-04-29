/**
 * tags.js — Tags page module.
 * Lists all tags grouped by category with filtering, pagination, and full CRUD.
 */

import {
  renderLoading,
  renderEmpty,
  renderErrorBlock,
  Pagination,
  showToast,
  showModal,
  escapeHtml,
} from "../shared/components.js";
import { fetchJSON } from "../shared/api.js";
import {
  renderSearchInput,
  renderFilterGroup,
  wireSearchFilter,
} from "../shared/search-filter.js";

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

const ITEMS_PER_PAGE = 10;

function adaptTag(t) {
  const cat = t.category || "Setlist";
  return {
    id: t.id,
    name: t.name,
    category: cat,
    files: 0, // Not available from API
  };
}

/**
 * Apply client-side filters and re-render.
 */
function applyFilters(container, allTags, state) {
  const q = (state.search || "").toLowerCase().trim();
  const cat = state.category || "";

  let filtered = allTags;
  if (cat) {
    filtered = filtered.filter((t) => t.category === cat);
  }
  if (q) {
    filtered = filtered.filter(
      (t) => t.name.toLowerCase().includes(q) || t.category.toLowerCase().includes(q),
    );
  }

  renderPage(container, filtered, 0, allTags.length, state, allTags);
}

function renderPage(container, pageItems, page, totalCount, state, allTags) {
  const start = page * ITEMS_PER_PAGE;
  const items = pageItems.slice(start, start + ITEMS_PER_PAGE);
  const totalPages = Math.ceil(pageItems.length / ITEMS_PER_PAGE) || 1;

  const rows = items
    .map((t) => {
      const cat = CATEGORY_INFO[t.category];
      const catBadge = `<span style="color:${cat.color}"><i class="fas ${cat.icon} mr-1"></i>${escapeHtml(t.category)}</span>`;
      const editBtn = `<button class="btn btn-sm btn-edit-tag" data-id="${t.id}" data-tag="${escapeHtml(t.name)}" title="Edit tag"><i class="fas fa-pencil-alt"></i></button>`;
      const delBtn = `<button class="btn btn-sm btn-red btn-delete-tag" data-id="${t.id}" data-tag="${escapeHtml(t.name)}" title="Delete tag"><i class="fas fa-times"></i></button>`;
      return `<tr><td><strong>${escapeHtml(t.name)}</strong></td><td>${catBadge}</td><td class="text-right">${t.files}</td><td style="white-space:nowrap">${editBtn} ${delBtn}</td></tr>`;
    })
    .join("");

  const paginationId = "tags-pagination";
  const prevId = "tags-prev";
  const nextId = "tags-next";
  const infoId = "tags-info";

  container.innerHTML = `
    <div class="toolbar">
      ${renderSearchInput("tags", state.search)}
      ${renderFilterGroup("category", CATEGORY_OPTIONS, state.category || "")}
      <button class="btn btn-primary" id="tags-new-btn"><i class="fas fa-plus"></i> New Tag</button>
    </div>
    <div class="stats-row">
      <div class="stats-group">
        <button class="btn btn-sm btn-icon" id="tags-refresh" title="Refresh"><i class="fa-solid fa-rotate"></i></button>
        <strong>${totalCount.toLocaleString()}</strong> tags
        <span style="margin:0 6px;color:var(--text-subtle);">·</span>
        <strong>${Object.keys(CATEGORY_INFO).length}</strong> categories
      </div>
    </div>
    <div class="table-wrap">
      <table class="data-table">
        <thead><tr><th style="width:35%">Tag</th><th style="width:30%">Category</th><th style="width:15%;text-align:right">Files</th><th style="width:20%">Actions</th></tr></thead>
        <tbody>${rows}</tbody>
      </table>
    </div>
    <div class="pagination" id="${paginationId}">
      <button class="pagination-btn" id="${prevId}" disabled><i class="fas fa-chevron-left"></i></button>
      <span class="pagination-info" id="${infoId}">Page ${page + 1} of ${totalPages}</span>
      <button class="pagination-btn" id="${nextId}" disabled><i class="fas fa-chevron-right"></i></button>
    </div>
  `;

  // Wire unified search/filter (re-filters client-side)
  const toolbar = container.querySelector(".toolbar");
  if (toolbar) {
    wireSearchFilter(toolbar, state, () => {
      applyFilters(container, allTags, state);
    });
  }

  // Wire pagination
  const pag = new Pagination({
    itemsPerPage: ITEMS_PER_PAGE,
    initialPage: page,
    bindings: { prev: prevId, next: nextId, info: infoId },
    onPageChange: (newPage) => {
      renderPage(container, pageItems, newPage, totalCount, state, allTags);
    },
  });
  pag.update(pageItems.length, items.length);
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

// Module-level mutable state so event listener is added only once
let _tagsContainer = null;
let _tagsData = { tags: [], categories: [] };
let _tagsState = { search: "", category: "" };
let _tagsReloading = false;

async function tagsReload() {
  if (_tagsReloading) return;
  _tagsReloading = true;
  try {
    const [tagsResp, catsResp] = await Promise.all([
      fetchJSON("/api/tags"),
      fetchJSON("/api/tag-categories"),
    ]);
    _tagsData.tags = tagsResp.data.map(adaptTag);
    _tagsData.categories = catsResp.data;
    applyFilters(_tagsContainer, _tagsData.tags, _tagsState);
  } catch (err) {
    showToast(`Failed to reload: ${err.message}`, "error");
  } finally {
    _tagsReloading = false;
  }
}

function initPage(container, tags, categories) {
  _tagsContainer = container;
  _tagsData.tags = tags;
  _tagsData.categories = categories;

  // Reset filter state only on first init (not on reload)
  if (!container._tagsListenerAttached) {
    _tagsState = { search: "", category: "" };
  }

  // Initial render
  applyFilters(container, tags, _tagsState);

  // Add event listener only once
  if (container._tagsListenerAttached) return;
  container._tagsListenerAttached = true;

  container.addEventListener("click", (e) => {
    const refreshBtn = e.target.closest("#tags-refresh");
    if (refreshBtn) {
      tagsReload();
      return;
    }

    const newBtn = e.target.closest("#tags-new-btn");
    if (newBtn) {
      showNewTagModal(_tagsData.categories, tagsReload);
      return;
    }

    const editBtn = e.target.closest(".btn-edit-tag");
    if (editBtn) {
      const id = parseInt(editBtn.dataset.id, 10);
      const name = editBtn.dataset.tag;
      const tag = _tagsData.tags.find((t) => t.id === id);
      const category = tag ? tag.category : "Setlist";
      showEditTagModal(id, name, category, _tagsData.categories, tagsReload);
      return;
    }

    const delBtn = e.target.closest(".btn-delete-tag");
    if (delBtn) {
      const id = parseInt(delBtn.dataset.id, 10);
      const name = delBtn.dataset.tag;
      deleteTag(id, name, tagsReload);
      return;
    }
  });
}

export async function init(container, signal) {
  container.innerHTML = renderLoading("Loading tags…");

  try {
    const [tagsResp, catsResp] = await Promise.all([
      fetchJSON("/api/tags", { signal }),
      fetchJSON("/api/tag-categories", { signal }),
    ]);
    if (signal.aborted) return;

    const tags = tagsResp.data.map(adaptTag);
    const categories = catsResp.data;
    initPage(container, tags, categories);
  } catch (err) {
    if (err.name === "AbortError") return;
    container.innerHTML = renderErrorBlock({
      title: "Failed to load tags",
      detail: err.message,
      retryFn: "window.location.hash='#tags'",
    });
  }
}
