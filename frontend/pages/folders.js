/**
 * folders.js — Folder management page.
 *
 * Stable toolbar + server-side pagination/sort + modal-based CRUD.
 * Follows the canonical CRUD blueprint pattern.
 */

import { fetchJSON } from "../shared/api.js";
import {
  escapeHtml,
  renderLoading,
  renderErrorBlock,
  showToast,
} from "../shared/components.js";
import { renderSearchInput, wireSearchFilter } from "../shared/search-filter.js";
import {
  getPageSize,
  renderPageSizeSelector,
  sortableTh,
  wireSortableHeaders,
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
/*  State                                                              */
/* ------------------------------------------------------------------ */

/** @type {{ page: number, pageSize: number, search: string, sort: string, order: string }} */
let state = {
  page: 0,
  pageSize: 25,
  search: "",
  sort: "",
  order: "asc",
};

let _delegationWired = false;
let _container = null;
let _signal = null;

/* ------------------------------------------------------------------ */
/*  Constants                                                          */
/* ------------------------------------------------------------------ */

const HASH_DEFAULTS = { sort: "", order: "asc", search: "", page: 0 };

const HASH_SCHEMA = {
  sort: { type: "string", default: "" },
  order: { type: "string", default: "asc" },
  search: { type: "string", default: "" },
  page: { type: "number", default: 0 },
};

const ALL_EXTENSIONS = [
  "mp3",
  "m4a",
  "aac",
  "wav",
  "flac",
  "ogg",
  "wma",
  "aiff",
  "alac",
  "dsf",
  "dff",
  "stem.m4a",
  "stem.mp3",
];

/* ------------------------------------------------------------------ */
/*  Column model                                                       */
/* ------------------------------------------------------------------ */

const FOLDERS_COLUMNS = [
  { id: "path", label: "Path", sortable: true, sortKey: "path", defaultWidth: 25 },
  { id: "files", label: "Files", sortable: true, sortKey: "file_count", defaultWidth: 8 },
  {
    id: "watch",
    label: "Watch",
    sortable: true,
    sortKey: "watch_enabled",
    defaultWidth: 6,
  },
  {
    id: "recursive",
    label: "Recursive",
    sortable: true,
    sortKey: "scan_recursive",
    defaultWidth: 6,
  },
  { id: "extensions", label: "Extensions", sortable: false, defaultWidth: 12 },
  {
    id: "maxDepth",
    label: "Max Depth",
    sortable: true,
    sortKey: "max_depth",
    defaultWidth: 6,
  },
  {
    id: "scanned",
    label: "Scanned",
    sortable: true,
    sortKey: "last_scanned",
    defaultWidth: 12,
  },
  { id: "actions", label: "Actions", sortable: false, defaultWidth: 25 },
];

/* ------------------------------------------------------------------ */
/*  Cell renderers                                                     */
/* ------------------------------------------------------------------ */

const FOLDERS_CELL_RENDERERS = {
  path: (f) =>
    `<code class="font-mono" style="font-size:0.8rem">${escapeHtml(f.path)}</code>`,
  files: (f) => `<strong style="font-size:0.9rem">${f.files}</strong>`,
  watch: (f) =>
    f.watch
      ? '<span class="status-badge connected"><i class="fas fa-eye"></i></span>'
      : '<span class="status-badge disconnected"><i class="fas fa-eye-slash"></i></span>',
  recursive: (f) =>
    f.recursive
      ? '<span class="status-badge connected"><i class="fas fa-folder-tree"></i></span>'
      : '<span class="status-badge disconnected"><i class="fas fa-folder"></i></span>',
  extensions: (f) => {
    if (f.fixedExtensions && f.fileExtensions) {
      const exts = f.fileExtensions
        .split(",")
        .filter(Boolean)
        .map((ext) => {
          const extClean = ext.trim();
          const badgeClass = extClean.includes("stem") ? "badge-stem" : "badge-standard";
          return `<span class="badge ${badgeClass}" style="font-size:0.7rem;padding:1px 5px;margin:1px">${escapeHtml(extClean)}</span>`;
        });
      return exts.length
        ? exts.join(" ")
        : '<span class="text-muted" style="font-size:0.75rem">All</span>';
    }
    return '<span class="text-muted" style="font-size:0.75rem">All</span>';
  },
  maxDepth: (f) =>
    f.maxDepth === 0
      ? '<span class="text-muted" style="font-size:0.75rem">No limit</span>'
      : `<span style="font-size:0.8rem">${f.maxDepth}</span>`,
  scanned: (f) =>
    f.lastScanned
      ? `<span style="color:var(--text-muted);font-size:0.75rem">${new Date(f.lastScanned * 1000).toLocaleString()}</span>`
      : '<span class="status-badge pending">Never</span>',
  actions: (f) => `
    <div style="display:flex;gap:4px;flex-wrap:nowrap">
      <button class="btn btn-sm btn-primary" data-folder-action="edit" data-id="${f.id}" title="Edit folder"><i class="fas fa-pen"></i></button>
      <button class="btn btn-sm" data-folder-action="rescan" data-id="${f.id}" title="Rescan folder"><i class="fas fa-sync"></i></button>
      <button class="btn btn-sm ${f.watch ? "btn-yellow" : ""}" data-folder-action="toggle-watch" data-id="${f.id}" title="${f.watch ? "Pause watching" : "Start watching"}">
        <i class="fas ${f.watch ? "fa-pause" : "fa-play"}"></i>
      </button>
      <button class="btn btn-sm btn-red" data-folder-action="remove" data-id="${f.id}" title="Remove folder">
        <i class="fas fa-trash"></i>
      </button>
    </div>
  `,
};

/* ------------------------------------------------------------------ */
/*  Adapter                                                            */
/* ------------------------------------------------------------------ */

function adaptFolder(f) {
  return {
    id: f.id,
    path: f.path,
    files: f.fileCount ?? f.file_count ?? 0,
    watch: f.watchEnabled ?? f.watch_enabled ?? false,
    recursive: f.scanRecursive ?? f.scan_recursive ?? false,
    fixedExtensions: f.fixedExtensions ?? f.fixed_extensions ?? false,
    fileExtensions: f.fileExtensions ?? f.file_extensions ?? "",
    maxDepth: f.maxDepth ?? f.max_depth ?? 1,
    lastScanned: f.lastScanned ?? f.last_scanned ?? null,
  };
}

/* ------------------------------------------------------------------ */
/*  Modal helpers                                                      */
/* ------------------------------------------------------------------ */

function renderExtensionCheckboxes(selected) {
  const sel = new Set(
    (selected || "")
      .split(",")
      .map((s) => s.trim().toLowerCase())
      .filter(Boolean),
  );
  return ALL_EXTENSIONS.map(
    (ext) =>
      `<label style="display:inline-flex;align-items:center;gap:4px;font-size:0.85rem;cursor:pointer;margin:3px">
        <input type="checkbox" class="ext-check" value="${ext}"${sel.has(ext) ? " checked" : ""}>
        ${ext}
      </label>`,
  ).join(" ");
}

function renderFolderModal(folder) {
  const isEdit = !!folder;
  const title = isEdit ? "Edit Folder" : "Add Folder";
  const f = isEdit
    ? folder
    : {
        path: "",
        watch: false,
        recursive: true,
        fixedExtensions: true,
        fileExtensions: "mp3,m4a,flac,wav,aiff",
        maxDepth: 1,
      };

  return `
    <div class="modal open" id="folder-modal">
      <div class="modal-content">
        <div class="modal-header">
          <h3>${title}</h3>
          <button class="close-btn close-modal-btn">&times;</button>
        </div>
        <form id="folder-form">
          <div class="form-group">
            <label>Folder Path</label>
            <input type="text" class="input-text w-full" id="folder-path"
                   value="${escapeHtml(f.path)}" placeholder="/path/to/music" required>
          </div>
          <div class="form-group">
            <label class="checkbox-label">
              <input type="checkbox" id="folder-watch" ${f.watch ? "checked" : ""}>
              Enable folder watching
            </label>
          </div>
          <div class="form-group">
            <label class="checkbox-label">
              <input type="checkbox" id="folder-recursive" ${f.recursive ? "checked" : ""}>
              Scan subdirectories recursively
            </label>
          </div>
          <div class="form-group">
            <label>Max Depth (0 = no limit)</label>
            <input type="number" class="input-text w-full" id="folder-depth"
                   value="${f.maxDepth}" min="0" max="10">
          </div>
          <div class="form-group">
            <label class="checkbox-label">
              <input type="checkbox" id="folder-fixed-ext" ${f.fixedExtensions ? "checked" : ""}>
              Only scan specific file types
            </label>
          </div>
          <div class="form-group" id="ext-group">
            <label>
              File Extensions
              <button type="button" class="btn btn-xs" id="ext-select-none" style="margin-left:8px;font-size:0.75rem">Select None</button>
            </label>
            <div style="display:flex;flex-wrap:wrap;gap:6px;margin-top:6px">
              ${renderExtensionCheckboxes(f.fileExtensions)}
            </div>
          </div>
          <div class="modal-actions">
            <button type="button" class="btn" id="folder-cancel">Cancel</button>
            <button type="submit" class="btn btn-primary" id="folder-save">
              ${isEdit ? "Save Changes" : "Add Folder"}
            </button>
          </div>
        </form>
      </div>
    </div>
  `;
}

function openAddModal() {
  closeModal();
  const overlay = document.createElement("div");
  overlay.innerHTML = renderFolderModal(null);
  document.body.appendChild(overlay);
  wireModalEvents(null);
}

function openEditModal(folder) {
  closeModal();
  const overlay = document.createElement("div");
  overlay.innerHTML = renderFolderModal(folder);
  document.body.appendChild(overlay);
  wireModalEvents(folder.id);
}

function closeModal() {
  document.querySelectorAll("#folder-modal").forEach((el) => el.remove());
}

function collectFormData() {
  const path = document.getElementById("folder-path")?.value.trim();
  const watchEnabled = document.getElementById("folder-watch")?.checked ?? false;
  const scanRecursive = document.getElementById("folder-recursive")?.checked ?? false;
  const fixedExtensions = document.getElementById("folder-fixed-ext")?.checked ?? false;
  let fileExtensions = "";

  if (fixedExtensions) {
    const checked = document.querySelectorAll(".ext-check:checked");
    fileExtensions = Array.from(checked)
      .map((cb) => cb.value)
      .join(",");
  }

  const maxDepth = parseInt(document.getElementById("folder-depth")?.value, 10) || 1;

  return { path, watchEnabled, scanRecursive, fixedExtensions, fileExtensions, maxDepth };
}

function wireModalEvents(folderId) {
  const modal = document.querySelector("#folder-modal");
  if (!modal) return;

  const closeBtn = modal.querySelector(".close-modal-btn");
  const cancelBtn = modal.querySelector("#folder-cancel");

  const doClose = () => closeModal();

  if (closeBtn) closeBtn.addEventListener("click", doClose);
  if (cancelBtn) cancelBtn.addEventListener("click", doClose);

  // Close on Escape
  const escHandler = (e) => {
    if (e.key === "Escape") {
      doClose();
      document.removeEventListener("keydown", escHandler);
    }
  };
  document.addEventListener("keydown", escHandler);

  // Fixed extensions toggle
  const fixedExtCheck = modal.querySelector("#folder-fixed-ext");
  const extContainer = modal.querySelector("#ext-group");
  if (fixedExtCheck && extContainer) {
    fixedExtCheck.addEventListener("change", () => {
      extContainer.style.display = fixedExtCheck.checked ? "block" : "none";
    });
    extContainer.style.display = fixedExtCheck.checked ? "block" : "none";
  }

  // Select None button for file extensions
  const selectNoneBtn = modal.querySelector("#ext-select-none");
  if (selectNoneBtn) {
    selectNoneBtn.addEventListener("click", () => {
      modal.querySelectorAll(".ext-check").forEach((cb) => (cb.checked = false));
    });
  }

  // Form submit
  const form = modal.querySelector("#folder-form");
  if (form) {
    form.addEventListener("submit", async (e) => {
      e.preventDefault();
      const data = collectFormData();

      if (!data.path) {
        showToast("Folder path is required", "error");
        return;
      }

      const saveBtn = modal.querySelector("#folder-save");
      const originalHtml = saveBtn.innerHTML;
      saveBtn.disabled = true;
      saveBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Saving...';

      try {
        if (folderId) {
          await fetchJSON(`/api/folders/${folderId}`, {
            method: "PUT",
            body: JSON.stringify(data),
          });
          showToast("Folder updated successfully", "success");
        } else {
          await fetchJSON("/api/folders", {
            method: "POST",
            body: JSON.stringify(data),
          });
          showToast("Folder added successfully", "success");
        }
        doClose();
        fetchAndRender();
      } catch (err) {
        showToast(`Failed to save folder: ${err.message}`, "error");
        saveBtn.disabled = false;
        saveBtn.innerHTML = originalHtml;
      }
    });
  }
}

/* ------------------------------------------------------------------ */
/*  Action helpers                                                     */
/* ------------------------------------------------------------------ */

async function deleteFolder(id, path) {
  if (!confirm(`Remove folder "${path}"?`)) return;
  try {
    await fetchJSON(`/api/folders/${id}`, { method: "DELETE" });
    showToast("Folder removed", "success");
    fetchAndRender();
  } catch (err) {
    showToast(`Failed to delete: ${err.message}`, "error");
  }
}

async function scanFolder(id, btnEl) {
  if (btnEl) {
    btnEl.disabled = true;
    btnEl.innerHTML = '<i class="fas fa-spinner fa-spin"></i>';
  }
  try {
    await fetchJSON(`/api/folders/${id}/scan`, { method: "POST" });
    showToast("Scan started", "success");
  } catch (err) {
    showToast(`Scan failed: ${err.message}`, "error");
    if (btnEl) {
      btnEl.disabled = false;
      btnEl.innerHTML = '<i class="fas fa-sync"></i>';
    }
  }
}

async function toggleWatch(id) {
  try {
    await fetchJSON(`/api/folders/${id}/watch`, { method: "POST" });
    showToast("Watch toggled", "success");
    fetchAndRender();
  } catch (err) {
    showToast(`Failed to toggle watch: ${err.message}`, "error");
  }
}

/* ------------------------------------------------------------------ */
/*  Build params                                                       */
/* ------------------------------------------------------------------ */

function buildParams(s) {
  const params = new URLSearchParams();
  params.set("limit", String(s.pageSize));
  params.set("offset", String(s.page * s.pageSize));
  if (s.sort) params.set("sort", s.sort);
  if (s.order) params.set("order", s.order);
  if (s.search) params.set("search", s.search);
  return params;
}

/* ------------------------------------------------------------------ */
/*  Render                                                             */
/* ------------------------------------------------------------------ */

function renderFolderRow(f) {
  // Watch status
  const watchHtml = f.watch
    ? '<span class="status-badge connected"><i class="fas fa-eye"></i></span>'
    : '<span class="status-badge disconnected"><i class="fas fa-eye-slash"></i></span>';

  // Recursive indicator
  const recursiveHtml = f.recursive
    ? '<span class="status-badge connected"><i class="fas fa-folder-tree"></i></span>'
    : '<span class="status-badge disconnected"><i class="fas fa-folder"></i></span>';

  // Extensions display
  let extHtml = "";
  if (f.fixedExtensions && f.fileExtensions) {
    const exts = f.fileExtensions
      .split(",")
      .filter(Boolean)
      .map((ext) => {
        const extClean = ext.trim();
        const badgeClass = extClean.includes("stem") ? "badge-stem" : "badge-standard";
        return `<span class="badge ${badgeClass}" style="font-size:0.7rem;padding:1px 5px;margin:1px">${escapeHtml(extClean)}</span>`;
      });
    extHtml = exts.length
      ? exts.join(" ")
      : `<span class="text-muted" style="font-size:0.75rem">All</span>`;
  } else {
    extHtml = `<span class="text-muted" style="font-size:0.75rem">All</span>`;
  }

  // Max depth
  const depthHtml =
    f.maxDepth === 0
      ? `<span class="text-muted" style="font-size:0.75rem">No limit</span>`
      : `<span style="font-size:0.8rem">${f.maxDepth}</span>`;

  // Last scanned
  const scannedHtml = f.lastScanned
    ? `<span style="color:var(--text-muted);font-size:0.75rem">${new Date(f.lastScanned * 1000).toLocaleString()}</span>`
    : '<span class="status-badge pending">Never</span>';

  // Actions
  const actionsHtml = `
    <div style="display:flex;gap:4px;flex-wrap:nowrap">
      <button class="btn btn-sm btn-primary" data-folder-action="edit" data-id="${f.id}" title="Edit folder"><i class="fas fa-pen"></i></button>
      <button class="btn btn-sm" data-folder-action="rescan" data-id="${f.id}" title="Rescan folder"><i class="fas fa-sync"></i></button>
      <button class="btn btn-sm ${f.watch ? "btn-yellow" : ""}" data-folder-action="toggle-watch" data-id="${f.id}" title="${f.watch ? "Pause watching" : "Start watching"}">
        <i class="fas ${f.watch ? "fa-pause" : "fa-play"}"></i>
      </button>
      <button class="btn btn-sm btn-red" data-folder-action="remove" data-id="${f.id}" title="Remove folder">
        <i class="fas fa-trash"></i>
      </button>
    </div>
  `;

  return `<tr>
    ${td(`<code class="font-mono" style="font-size:0.8rem">${escapeHtml(f.path)}</code>`, { style: "width:25%" })}
    ${td(`<strong>${f.files}</strong>`, { style: "width:8%;text-align:center" })}
    ${td(watchHtml, { style: "width:6%;text-align:center" })}
    ${td(recursiveHtml, { style: "width:6%;text-align:center" })}
    ${td(extHtml, { style: "width:12%" })}
    ${td(depthHtml, { style: "width:6%;text-align:center" })}
    ${td(scannedHtml, { style: "width:12%" })}
    ${td(actionsHtml, { style: "width:25%" })}
  </tr>`;
}

function renderBody(data, totalCount, colConfig) {
  const { page, pageSize, sort, order, search } = state;
  const totalPages = Math.ceil(totalCount / pageSize) || 1;

  const visibleCount = colConfig.filter((c) => c.visible).length;

  const rowsHtml = data.length
    ? data
        .map(
          (f) =>
            `<tr>${renderColumnCells(colConfig, FOLDERS_COLUMNS, FOLDERS_CELL_RENDERERS, f)}</tr>`,
        )
        .join("")
    : `<tr><td colspan="${visibleCount}"><div class="text-center text-muted" style="padding:24px">${
        search
          ? "No folders match your filters"
          : "No folders configured yet. Click <strong>Add Folder</strong> to get started."
      }</div></td></tr>`;

  const prevDisabled = page <= 0;
  const nextDisabled = page >= totalPages - 1;

  return `
    <div class="stats-row">
      <div class="stats-group">
        <button class="btn btn-sm btn-icon" id="folders-refresh" title="Refresh"><i class="fa-solid fa-rotate"></i></button>
        <strong>${totalCount}</strong> folders
        ${renderColumnConfigTrigger()}
      </div>
      <div class="stats-group">
        ${renderPageSizeSelector(pageSize)}
      </div>
    </div>

    <div class="table-wrap">
      <table class="data-table sortable-table" id="folders-table">
        <thead><tr>${renderColumnHeaders(colConfig, FOLDERS_COLUMNS, { sort, order }, sortableTh)}</tr></thead>
        <tbody>${rowsHtml}</tbody>
      </table>
    </div>

    <div class="pagination">
      <button class="pagination-btn" data-pagination="prev" data-total-count="${totalCount}"${prevDisabled ? " disabled" : ""}>
        <i class="fa-solid fa-chevron-left"></i> Prev
      </button>
      <span class="pagination-info">Page ${page + 1} of ${totalPages}</span>
      <button class="pagination-btn" data-pagination="next" data-total-count="${totalCount}"${nextDisabled ? " disabled" : ""}>
        Next <i class="fa-solid fa-chevron-right"></i>
      </button>
    </div>
  `;
}

/* ------------------------------------------------------------------ */
/*  Fetch + Render loop                                                */
/* ------------------------------------------------------------------ */

async function fetchAndRender() {
  if (!_container) return;

  // Show loading state in content area (toolbar stays stable)
  const contentEl = _container.querySelector("#folders-content");
  if (contentEl) {
    contentEl.innerHTML = renderLoading("Loading folders…");
  }

  try {
    const colConfig = loadColumnConfig("folders", FOLDERS_COLUMNS);
    const params = buildParams(state);
    const [foldersResp, countResp] = await Promise.all([
      fetchJSON(`/api/folders?${params.toString()}`, { signal: _signal }),
      fetchJSON(`/api/folders/count?search=${encodeURIComponent(state.search)}`, {
        signal: _signal,
      }),
    ]);

    if (_signal?.aborted) return;

    const folders = (foldersResp.data || []).map(adaptFolder);
    const totalCount = countResp.data ?? 0;

    // Update content
    if (contentEl) {
      contentEl.innerHTML = renderBody(folders, totalCount, colConfig);

      // Wire sortable headers (inside the re-rendered content)
      const tableEl = contentEl.querySelector("#folders-table");
      if (tableEl) {
        wireSortableHeaders(tableEl, state, () => {
          updateHash("folders", state, HASH_DEFAULTS);
          fetchAndRender();
        });
      }

      // Wire column resize/drag/config
      wireColumnResize(contentEl, "folders", FOLDERS_COLUMNS, colConfig);
      wireColumnDragReorder(contentEl, "folders", FOLDERS_COLUMNS, colConfig, () => {
        fetchAndRender();
      });
      wireConfigTrigger(contentEl, "folders", FOLDERS_COLUMNS, colConfig, () => {
        fetchAndRender();
      });

      // Stash folder data on action buttons for delegation
      stashFolderData(contentEl, folders);

      // Wire page size selector
      wirePageSizeSelectorLocal(contentEl);
    }
  } catch (err) {
    if (err.name === "AbortError") return;
    if (contentEl) {
      contentEl.innerHTML = renderErrorBlock({
        title: "Failed to load folders",
        detail: err.message,
        retryFn: "void(0)",
      });
    }
    showToast(`Failed to load folders: ${err.message}`, "error");
  }
}

function wirePageSizeSelectorLocal(contentEl) {
  const sel = contentEl.querySelector("[data-page-size]");
  if (!sel) return;
  sel.addEventListener("change", () => {
    const val = parseInt(sel.value, 10);
    localStorage.setItem("crudPageSize", String(val));
    state.pageSize = val;
    state.page = 0;
    updateHash("folders", state, HASH_DEFAULTS);
    fetchAndRender();
  });
}

/* ------------------------------------------------------------------ */
/*  Toolbar rendering (once)                                           */
/* ------------------------------------------------------------------ */

function renderToolbar() {
  return `
    <div class="filter-panel" id="folders-filter-panel">
      <div class="filter-panel-header">
        ${renderSearchInput("folders", state.search)}
        <button class="btn btn-primary" id="folders-add-btn"><i class="fa-solid fa-plus"></i> Add Folder</button>
        <button class="filter-panel-toggle" id="folders-filter-toggle" title="Toggle filters">
          <i class="fa-solid fa-chevron-up chevron"></i>
        </button>
      </div>
    </div>
  `;
}

function wireToolbar() {
  const toolbar = document.getElementById("folders-toolbar");
  if (!toolbar) return;

  // Wire search filter
  wireSearchFilter(toolbar, state, () => {
    state.page = 0;
    updateHash("folders", state, HASH_DEFAULTS);
    fetchAndRender();
  });
}

/* ------------------------------------------------------------------ */
/*  Event delegation (persistent)                                      */
/* ------------------------------------------------------------------ */

function wireDelegation() {
  if (_delegationWired || !_container) return;
  _delegationWired = true;

  _container.addEventListener("click", (e) => {
    const btn = e.target.closest("[data-folder-action]");
    if (btn) {
      const action = btn.dataset.folderAction;
      const id = parseInt(btn.dataset.id, 10);

      switch (action) {
        case "add-folder":
        case "add":
          // handled by wireToolbar for #folders-add-btn, but catch delegation too
          openAddModal();
          break;
        case "edit":
          openEditModal({
            id,
            path: btn.dataset.path || "",
            watch: btn.dataset.watch === "true",
            recursive: btn.dataset.recursive === "true",
            fixedExtensions: btn.dataset.fixedExt === "true",
            fileExtensions: btn.dataset.fileExt || "",
            maxDepth: parseInt(btn.dataset.maxDepth, 10) || 1,
            lastScanned: parseInt(btn.dataset.lastScanned, 10) || null,
            files: parseInt(btn.dataset.files, 10) || 0,
          });
          break;
        case "remove": {
          const path = btn.dataset.path || "this folder";
          deleteFolder(id, path);
          break;
        }
        case "rescan":
          scanFolder(id, btn);
          break;
        case "toggle-watch":
          toggleWatch(id);
          break;
      }
      return;
    }

    // Refresh button
    if (e.target.closest("#folders-refresh")) {
      e.preventDefault();
      fetchAndRender();
      return;
    }

    // Add folder button
    if (e.target.closest("#folders-add-btn")) {
      e.preventDefault();
      openAddModal();
      return;
    }

    // Pagination
    const pagBtn = e.target.closest("[data-pagination]");
    if (pagBtn && !pagBtn.disabled) {
      e.preventDefault();
      const dir = pagBtn.dataset.pagination;
      const totalItems = pagBtn.dataset.totalCount
        ? parseInt(pagBtn.dataset.totalCount, 10)
        : 0;
      const totalPages = Math.ceil(totalItems / state.pageSize) || 1;

      if (dir === "prev" && state.page > 0) {
        state.page--;
      } else if (dir === "next" && state.page < totalPages - 1) {
        state.page++;
      } else {
        return;
      }
      updateHash("folders", state, HASH_DEFAULTS);
      fetchAndRender();
    }
  });
}

/* ------------------------------------------------------------------ */
/*  Stash folder data on action buttons for delegation                  */
/* ------------------------------------------------------------------ */

function stashFolderData(contentEl, folders) {
  // Attach folder metadata to action buttons as data attributes
  // so the delegation handler can access it without a global ref
  for (const f of folders) {
    contentEl
      .querySelectorAll(`[data-folder-action][data-id="${f.id}"]`)
      .forEach((btn) => {
        btn.dataset.path = f.path;
        btn.dataset.watch = String(f.watch);
        btn.dataset.recursive = String(f.recursive);
        btn.dataset.fixedExt = String(f.fixedExtensions);
        btn.dataset.fileExt = f.fileExtensions;
        btn.dataset.maxDepth = String(f.maxDepth);
        btn.dataset.lastScanned = String(f.lastScanned ?? "");
        btn.dataset.files = String(f.files);
      });
  }
}

/* ------------------------------------------------------------------ */
/*  Init                                                               */
/* ------------------------------------------------------------------ */

export async function init(container, signal, hashParams) {
  _container = container;
  _signal = signal;
  _delegationWired = false;

  // Clean up any leftover modal overlay from a previous page visit
  closeModal();

  // Parse hash params into state
  const parsed = parseHash(hashParams, HASH_SCHEMA);
  state = {
    page: parsed.page,
    pageSize: getPageSize(25),
    search: parsed.search,
    sort: parsed.sort,
    order: parsed.order,
  };

  // Render stable toolbar + content wrapper
  container.innerHTML = `
    ${renderToolbar()}
    <div id="folders-content">${renderLoading("Loading folders…")}</div>
  `;

  // Wire toolbar (search input — stable, wired once)
  wireToolbar();

  // Wire filter panel collapse toggle
  const toggleBtn = container.querySelector("#folders-filter-toggle");
  const filterPanel = container.querySelector("#folders-filter-panel");
  if (toggleBtn && filterPanel) {
    const saved = localStorage.getItem("filterPanelCollapsed_folders");
    if (saved === "true") filterPanel.classList.add("collapsed");
    toggleBtn.addEventListener("click", () => {
      filterPanel.classList.toggle("collapsed");
      localStorage.setItem(
        "filterPanelCollapsed_folders",
        filterPanel.classList.contains("collapsed"),
      );
    });
  }

  // Wire persistent event delegation (covers all row actions + pagination + refresh)
  wireDelegation();

  // Fetch initial data
  await fetchAndRender();
}
