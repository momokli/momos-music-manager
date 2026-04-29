/**
 * folders.js — Folder management page.
 *
 * Manages watched folders for music file scanning with CRUD modals,
 * client-side search/filter, and pagination.
 */

import { fetchJSON } from "../shared/api.js";
import {
  renderLoading,
  renderEmpty,
  renderErrorBlock,
  Pagination,
} from "../shared/components.js";
import { renderSearchInput, wireSearchFilter } from "../shared/search-filter.js";

/* ------------------------------------------------------------------ */
/*  Constants                                                          */
/* ------------------------------------------------------------------ */

const PAGE_SIZE = 10;

/* ------------------------------------------------------------------ */
/*  State                                                              */
/* ------------------------------------------------------------------ */

let state = { folders: [], editingFolder: null, search: "", page: 0 };
let _delegationWired = false;

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
    last_scanned: f.lastScanned ?? f.last_scanned ?? null,
    status: "",
  };
}

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

function escapeHtml(str) {
  if (typeof str !== "string") return str;
  return str
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

function showToast(message, type) {
  const existing = document.querySelector(".toast");
  if (existing) existing.remove();

  const bg =
    type === "error"
      ? "var(--red)"
      : type === "success"
        ? "var(--green)"
        : "var(--accent)";

  const toast = document.createElement("div");
  toast.className = "toast";
  Object.assign(toast.style, {
    position: "fixed",
    bottom: "20px",
    right: "20px",
    background: bg,
    color: "#fff",
    padding: "10px 18px",
    borderRadius: "8px",
    fontSize: "0.85rem",
    zIndex: "999",
    boxShadow: "0 4px 12px rgba(0,0,0,0.3)",
    transition: "opacity 0.3s",
    cursor: "pointer",
  });
  toast.textContent = message;
  toast.onclick = () => toast.remove();
  document.body.appendChild(toast);
  setTimeout(() => {
    toast.style.opacity = "0";
    setTimeout(() => toast.remove(), 300);
  }, 4000);
}

function showError(msg) {
  showToast(msg, "error");
}
function showSuccess(msg) {
  showToast(msg, "success");
}

/* ------------------------------------------------------------------ */
/*  Modal helpers                                                      */
/* ------------------------------------------------------------------ */

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
  // Remove any leftover modal from a previous session
  closeModal();
  const overlay = document.createElement("div");
  overlay.innerHTML = renderFolderModal(null);
  document.body.appendChild(overlay);
  wireModalEvents(null);
}

function openEditModal(folder) {
  // Remove any leftover modal from a previous session
  closeModal();
  const overlay = document.createElement("div");
  overlay.innerHTML = renderFolderModal(folder);
  document.body.appendChild(overlay);
  wireModalEvents(folder.id);
}

function closeModal() {
  // Remove ALL leftover modals to prevent duplicate ID accumulation
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
        showError("Folder path is required");
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
          showSuccess("Folder updated successfully");
        } else {
          await fetchJSON("/api/folders", {
            method: "POST",
            body: JSON.stringify(data),
          });
          showSuccess("Folder added successfully");
        }
        doClose();
        loadFolders();
      } catch (err) {
        showError(`Failed to save folder: ${err.message}`);
        saveBtn.disabled = false;
        saveBtn.innerHTML = originalHtml;
      }
    });
  }
}

/* ------------------------------------------------------------------ */
/*  API helpers                                                        */
/* ------------------------------------------------------------------ */

async function deleteFolder(id, path) {
  if (!confirm(`Remove folder "${path}"?`)) return;
  try {
    await fetchJSON(`/api/folders/${id}`, { method: "DELETE" });
    showSuccess("Folder removed");
    loadFolders();
  } catch (err) {
    showError(`Failed to delete: ${err.message}`);
  }
}

async function scanFolder(id) {
  const btn = document.querySelector(`[data-id="${id}"][data-action="rescan"]`);
  if (btn) {
    btn.disabled = true;
    btn.innerHTML = '<i class="fas fa-spinner fa-spin"></i>';
  }
  try {
    await fetchJSON(`/api/folders/${id}/scan`, { method: "POST" });
    showSuccess("Scan started");
  } catch (err) {
    showError(`Scan failed: ${err.message}`);
    if (btn) {
      btn.disabled = false;
      btn.innerHTML = '<i class="fas fa-sync"></i> Scan';
    }
  }
}

async function toggleWatch(id, currentWatch) {
  try {
    await fetchJSON(`/api/folders/${id}/watch`, { method: "POST" });
    showSuccess(currentWatch ? "Watch disabled" : "Watch enabled");
    loadFolders();
  } catch (err) {
    showError(`Failed to toggle watch: ${err.message}`);
  }
}

/* ------------------------------------------------------------------ */
/*  Render                                                             */
/* ------------------------------------------------------------------ */

function renderFolderRow(f) {
  const watchHtml = f.watch
    ? '<span class="status-badge connected"><i class="fas fa-eye"></i> Watching</span>'
    : '<span class="status-badge disconnected"><i class="fas fa-eye-slash"></i> Paused</span>';

  const recursiveHtml = f.recursive
    ? '<span class="status-badge connected"><i class="fas fa-folder-tree"></i> Yes</span>'
    : '<span class="status-badge disconnected"><i class="fas fa-folder"></i> No</span>';

  const scannedHtml = f.last_scanned
    ? `<span style="color:var(--text-muted);font-size:0.8rem;">${new Date(f.last_scanned * 1000).toLocaleString()}</span>`
    : '<span class="status-badge pending">Never</span>';

  const actionsHtml = `
    <div style="display:flex;gap:4px;flex-wrap:nowrap">
      <button class="btn btn-sm btn-primary" data-action="edit" data-id="${f.id}" title="Edit folder"><i class="fas fa-pen"></i></button>
      <button class="btn btn-sm" data-action="rescan" data-id="${f.id}" title="Rescan folder"><i class="fas fa-sync"></i></button>
      <button class="btn btn-sm ${f.watch ? "btn-yellow" : ""}" data-action="toggle-watch" data-id="${f.id}" title="${f.watch ? "Pause watching" : "Start watching"}">
        <i class="fas ${f.watch ? "fa-pause" : "fa-play"}"></i>
      </button>
      <button class="btn btn-sm btn-red" data-action="remove" data-id="${f.id}" title="Remove folder"
              onclick="return confirm('Remove this folder?')">
        <i class="fas fa-trash"></i>
      </button>
    </div>
  `;

  return `<tr>
    <td style="width:22%"><code class="font-mono" style="font-size:0.8rem">${escapeHtml(f.path)}</code></td>
    <td style="width:8%;text-align:center">${f.files}</td>
    <td style="width:20%">${watchHtml}</td>
    <td style="width:9%">${recursiveHtml}</td>
    <td style="width:18%">${scannedHtml}</td>
    <td style="width:23%">${actionsHtml}</td>
  </tr>`;
}

/**
 * Render the page with client-side search and pagination.
 * @param {HTMLElement} container
 * @param {Array} folders - all folders (unfiltered)
 * @param {number} [page] - current page (defaults to state.page)
 */
function renderPage(container, folders, page) {
  // Client-side search filter
  let filtered = folders;
  if (state.search) {
    const q = state.search.toLowerCase();
    filtered = folders.filter((f) => f.path.toLowerCase().includes(q));
  }

  const currentPage = page ?? state.page ?? 0;
  const offset = currentPage * PAGE_SIZE;
  const rows = filtered.slice(offset, offset + PAGE_SIZE);

  container.innerHTML = `
    <div class="toolbar">
      ${renderSearchInput("folders", state.search)}
      <button class="btn btn-primary" id="add-folder-btn" data-action="add-folder">
        <i class="fa-solid fa-plus"></i> Add Folder
      </button>
    </div>

    <div class="stats-row">
      <div class="stats-group">
        <button class="btn btn-sm btn-icon" id="folders-refresh" title="Refresh"><i class="fa-solid fa-rotate"></i></button>
        <strong>${filtered.length}</strong> folders
      </div>
    </div>

    <div class="table-wrap">
      <table class="data-table">
        <thead>
          <tr>
            <th style="width:22%">Path</th>
            <th style="width:8%">Files</th>
            <th style="width:20%">Watch</th>
            <th style="width:9%">Recursive</th>
            <th style="width:18%">Last Scanned</th>
            <th style="width:23%">Actions</th>
          </tr>
        </thead>
        <tbody>
          ${
            rows.length
              ? rows.map(renderFolderRow).join("")
              : `<tr><td colspan="6"><div class="text-center text-muted" style="padding:24px">${state.search ? "No folders match your search" : "No folders configured yet"}</div></td></tr>`
          }
        </tbody>
      </table>
    </div>

    <div class="pagination">
      <button class="pagination-btn" id="p-prev"><i class="fa-solid fa-chevron-left"></i></button>
      <span class="pagination-info" id="p-info">Page 1 of 1</span>
      <button class="pagination-btn" id="p-next"><i class="fa-solid fa-chevron-right"></i></button>
    </div>
  `;

  // Wire unified search/filter (client-side filtering)
  const toolbar = container.querySelector(".toolbar");
  if (toolbar) {
    wireSearchFilter(toolbar, state, () => loadFolders());
  }

  // Wire pagination
  const pag = new Pagination({
    itemsPerPage: PAGE_SIZE,
    totalItems: filtered.length,
    initialPage: currentPage,
    bindings: { prev: "p-prev", next: "p-next", info: "p-info" },
    onPageChange: (newPage) => {
      state.page = newPage;
      renderPage(container, folders, newPage);
    },
  });
  pag.update(filtered.length, rows.length);

  // Wire events
  wireEvents(container);
}

/* ------------------------------------------------------------------ */
/*  Event wiring                                                       */
/* ------------------------------------------------------------------ */

function wireEvents(container) {
  // Add Folder button (direct handler – re-wired each render)
  const addBtn = container.querySelector("#add-folder-btn");
  if (addBtn) {
    addBtn.addEventListener("click", openAddModal);
  }

  // Refresh button (direct handler – re-wired each render)
  const refreshBtn = container.querySelector("#folders-refresh");
  if (refreshBtn) {
    refreshBtn.addEventListener("click", loadFolders);
  }

  // Row action buttons (event delegation – wired once)
  if (!_delegationWired) {
    _delegationWired = true;
    container.addEventListener("click", (e) => {
      const btn = e.target.closest("[data-action]");
      if (!btn) return;
      e.preventDefault();

      const action = btn.dataset.action;
      const id = parseInt(btn.dataset.id, 10);

      if (action === "add-folder") {
        openAddModal();
        return;
      }

      const folder = state.folders.find((f) => f.id === id);
      if (!folder) return;

      switch (action) {
        case "edit":
          openEditModal(folder);
          break;
        case "remove":
          deleteFolder(id, folder.path);
          break;
        case "rescan":
          scanFolder(id);
          break;
        case "toggle-watch":
          toggleWatch(id, folder.watch);
          break;
      }
    });
  }
}

/* ------------------------------------------------------------------ */
/*  Data loading                                                       */
/* ------------------------------------------------------------------ */

async function loadFolders() {
  const container = document.getElementById("main-content");
  if (!container) return;

  try {
    const resp = await fetchJSON("/api/folders");
    const allFolders = resp.data.map(adaptFolder);
    state.folders = allFolders;
    // Reset to page 0 on re-fetch (e.g. after search/filter change via wireSearchFilter)
    state.page = 0;
    renderPage(container, state.folders, 0);
  } catch (err) {
    container.innerHTML = renderErrorBlock({
      title: "Failed to load folders",
      detail: err.message,
      retryFn: "window.location.hash='#folders'",
    });
  }
}

/* ------------------------------------------------------------------ */
/*  Initialisation                                                     */
/* ------------------------------------------------------------------ */

export async function init(container, signal) {
  container.innerHTML = renderLoading("Loading folders...");

  // Clean up any leftover modal overlay from a previous page visit
  closeModal();

  // Reset state
  state = { folders: [], editingFolder: null, search: "", page: 0 };

  try {
    const resp = await fetchJSON("/api/folders", { signal });
    if (signal.aborted) return;

    const visibilityHandler = () => {
      if (!document.hidden) {
        loadFolders();
      }
    };
    document.addEventListener("visibilitychange", visibilityHandler, { signal });

    const allFolders = resp.data.map(adaptFolder);
    state.folders = allFolders;
    renderPage(container, state.folders, 0);
  } catch (err) {
    if (err.name === "AbortError") return;
    container.innerHTML = renderErrorBlock({
      title: "Failed to load folders",
      detail: err.message,
      retryFn: "window.location.hash='#folders'",
    });
  }
}
