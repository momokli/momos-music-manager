/**
 * data.js — Import/Export page.
 *
 * Two sections:
 *   - Export: download the full DB as a JSON file
 *   - Import: upload a JSON dump file to restore the DB (destructive!)
 */

import { escapeHtml, showToast } from "../shared/components.js";

/* ------------------------------------------------------------------ */
/*  State                                                              */
/* ------------------------------------------------------------------ */

let state = {
  exportLoading: false,
  importFile: null,
  importPreview: null,
  importLoading: false,
};

let _container = null;
let _signal = null;

/* ------------------------------------------------------------------ */
/*  Constants                                                          */
/* ------------------------------------------------------------------ */

const TABLE_LABELS = {
  tagCategories: "Tag Categories",
  tags: "Tags",
  tagEmbeddings: "Tag Embeddings",
  tagEnergyLevels: "Tag Energy Levels",
  folders: "Folders",
  serviceConfig: "Service Config",
  serviceTracks: "Service Tracks",
  servicePlaylists: "Service Playlists",
  servicePlaylistTracks: "Service Playlist Tracks",
  files: "Files",
  playlistSubscriptions: "Playlist Subscriptions",
  deemixDownloads: "Deemix Downloads",
};

/* ------------------------------------------------------------------ */
/*  Render                                                             */
/* ------------------------------------------------------------------ */

function renderPage() {
  if (!_container) return;

  _container.innerHTML = `
    <div class="page-header">
      <h1><i class="fa-solid fa-database"></i> Import / Export</h1>
      <p>Backup your database or restore from a previous backup.</p>
    </div>

    <div class="data-grid">
      ${renderExportCard()}
      ${renderImportCard()}
    </div>
  `;

  wireEvents();
}

function renderExportCard() {
  return `
    <div class="card data-card">
      <div class="card-header">
        <h2><i class="fa-solid fa-download"></i> Export Database</h2>
      </div>
      <div class="card-body">
        <p>Download a complete backup of your music manager database as a JSON file.
        This includes all files, tracks, playlists, tags, folders, and service configurations.</p>
        <button id="export-btn" class="btn btn-primary" ${state.exportLoading ? "disabled" : ""}>
          ${state.exportLoading
            ? '<i class="fa-solid fa-spinner fa-spin"></i> Exporting...'
            : '<i class="fa-solid fa-download"></i> Export Database'}
        </button>
      </div>
    </div>
  `;
}

function renderImportCard() {
  const hasPreview = state.importPreview !== null;
  const hasFile = state.importFile !== null;

  return `
    <div class="card data-card danger-zone">
      <div class="card-header">
        <h2><i class="fa-solid fa-upload"></i> Import Database</h2>
      </div>
      <div class="card-body">
        <div class="warning-banner">
          <i class="fa-solid fa-triangle-exclamation"></i>
          <strong>Destructive operation:</strong> This will replace ALL existing data
          with the contents of the backup file. Make sure you have exported a backup first.
        </div>

        ${!hasFile ? `
          <div class="file-drop-zone" id="drop-zone">
            <i class="fa-solid fa-cloud-arrow-up fa-2x"></i>
            <p>Drag & drop a JSON dump file here, or click to browse</p>
            <input type="file" id="file-input" accept=".json" hidden />
            <button class="btn btn-outline" id="browse-btn">
              <i class="fa-solid fa-folder-open"></i> Browse Files
            </button>
          </div>
        ` : `
          <div class="import-preview">
            <div class="preview-header">
              <span><i class="fa-solid fa-file"></i> ${escapeHtml(state.importFile.name)}</span>
              <span class="file-size">${formatFileSize(state.importFile.size)}</span>
              <button class="btn btn-sm btn-outline" id="clear-file-btn">
                <i class="fa-solid fa-xmark"></i> Remove
              </button>
            </div>

            ${hasPreview ? `
              <div class="preview-details">
                ${state.importPreview.dumpedAt ? `
                  <div class="preview-meta">
                    <strong>Backup date:</strong>
                    <span>${new Date(state.importPreview.dumpedAt * 1000).toLocaleString()}</span>
                  </div>
                ` : ""}

                <h4>Table contents</h4>
                <table class="preview-table">
                  <thead>
                    <tr>
                      <th>Table</th>
                      <th class="num-col">Rows</th>
                    </tr>
                  </thead>
                  <tbody>
                    ${Object.entries(TABLE_LABELS)
                      .filter(([key]) => state.importPreview[key] !== undefined)
                      .map(([key, label]) => {
                        const count = state.importPreview[key]?.length ?? 0;
                        return `
                          <tr>
                            <td>${escapeHtml(label)}</td>
                            <td class="num-col">${count.toLocaleString()}</td>
                          </tr>
                        `;
                      })
                      .join("")}
                  </tbody>
                  <tfoot>
                    <tr>
                      <th>Total</th>
                      <th class="num-col">${countTotalRows(state.importPreview).toLocaleString()}</th>
                    </tr>
                  </tfoot>
                </table>
              </div>

              <button id="restore-btn" class="btn btn-danger"
                ${state.importLoading ? "disabled" : ""}>
                ${state.importLoading
                  ? '<i class="fa-solid fa-spinner fa-spin"></i> Restoring...'
                  : '<i class="fa-solid fa-triangle-exclamation"></i> Restore from Backup'}
              </button>
              <p class="restore-warning">This cannot be undone. All current data will be lost.</p>
            ` : `
              <p class="preview-loading"><i class="fa-solid fa-spinner fa-spin"></i> Parsing backup file...</p>
            `}
          </div>
        `}
      </div>
    </div>
  `;
}

/* ------------------------------------------------------------------ */
/*  Helper functions                                                   */
/* ------------------------------------------------------------------ */

function formatFileSize(bytes) {
  if (bytes < 1024) return bytes + " B";
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + " KB";
  return (bytes / (1024 * 1024)).toFixed(1) + " MB";
}

function countTotalRows(preview) {
  return Object.entries(TABLE_LABELS).reduce((sum, [key]) => {
    return sum + (preview[key]?.length ?? 0);
  }, 0);
}

function parseImportPreview(text) {
  try {
    const data = JSON.parse(text);
    // Validate it's at least somewhat a DataDump
    if (!data || typeof data !== "object") return null;
    return data;
  } catch {
    return null;
  }
}

/* ------------------------------------------------------------------ */
/*  Actions                                                            */
/* ------------------------------------------------------------------ */

async function handleExport() {
  if (state.exportLoading) return;
  state.exportLoading = true;
  renderPage();

  try {
    const res = await fetch("/api/dump", { signal: _signal });
    if (!res.ok) {
      const err = await res.json().catch(() => ({ error: res.statusText }));
      throw new Error(err.error || "Export failed");
    }

    // Determine filename from Content-Disposition or use a default
    const disposition = res.headers.get("Content-Disposition") || "";
    const match = disposition.match(/filename="?(.+?)"?$/);
    const filename = match ? match[1] : `momos-dump-${Date.now()}.json`;

    const blob = await res.blob();
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);

    showToast("Database exported successfully", "success");
  } catch (err) {
    if (err.name === "AbortError") return;
    showToast(`Export failed: ${err.message}`, "error");
  } finally {
    state.exportLoading = false;
    renderPage();
  }
}

async function handleFileSelected(file) {
  state.importFile = file;
  state.importPreview = null;
  renderPage();

  // Parse the file client-side for preview
  try {
    const text = await file.text();
    const preview = parseImportPreview(text);
    if (preview) {
      state.importPreview = preview;
    } else {
      showToast("Invalid backup file: could not parse JSON", "error");
      state.importFile = null;
    }
  } catch (err) {
    showToast(`Failed to read file: ${err.message}`, "error");
    state.importFile = null;
  }
  renderPage();
}

async function handleRestore() {
  if (state.importLoading || !state.importFile || !state.importPreview) return;

  // Extra confirmation
  if (!confirm("Are you absolutely sure? This will DELETE all existing data and replace it with the backup. This cannot be undone.")) {
    return;
  }

  state.importLoading = true;
  renderPage();

  try {
    const formData = new FormData();
    formData.append("file", state.importFile);

    const res = await fetch("/api/restore?confirm=true", {
      method: "POST",
      body: formData,
      signal: _signal,
    });

    const data = await res.json();
    if (!res.ok) {
      throw new Error(data.error || "Restore failed");
    }

    showToast("Database restored successfully! Redirecting...", "success");

    // Reset state and redirect to dashboard after delay
    state.importFile = null;
    state.importPreview = null;

    setTimeout(() => {
      window.location.hash = "#dashboard";
    }, 2000);
  } catch (err) {
    if (err.name === "AbortError") return;
    showToast(`Restore failed: ${err.message}`, "error");
  } finally {
    state.importLoading = false;
    renderPage();
  }
}

function handleClearFile() {
  state.importFile = null;
  state.importPreview = null;
  renderPage();
}

/* ------------------------------------------------------------------ */
/*  Events                                                             */
/* ------------------------------------------------------------------ */

function wireEvents() {
  if (!_container) return;

  // Export
  const exportBtn = _container.querySelector("#export-btn");
  if (exportBtn) {
    exportBtn.addEventListener("click", handleExport);
  }

  // File input / drop zone
  const fileInput = _container.querySelector("#file-input");
  const browseBtn = _container.querySelector("#browse-btn");
  const dropZone = _container.querySelector("#drop-zone");

  if (fileInput) {
    fileInput.addEventListener("change", (e) => {
      const file = e.target.files?.[0];
      if (file) handleFileSelected(file);
    });
  }

  if (browseBtn && fileInput) {
    browseBtn.addEventListener("click", () => fileInput.click());
  }

  if (dropZone) {
    dropZone.addEventListener("dragover", (e) => {
      e.preventDefault();
      dropZone.classList.add("drag-over");
    });
    dropZone.addEventListener("dragleave", () => {
      dropZone.classList.remove("drag-over");
    });
    dropZone.addEventListener("drop", (e) => {
      e.preventDefault();
      dropZone.classList.remove("drag-over");
      const file = e.dataTransfer?.files?.[0];
      if (file) handleFileSelected(file);
    });
    dropZone.addEventListener("click", () => {
      if (fileInput) fileInput.click();
    });
  }

  // Restore
  const restoreBtn = _container.querySelector("#restore-btn");
  if (restoreBtn) {
    restoreBtn.addEventListener("click", handleRestore);
  }

  // Clear file
  const clearBtn = _container.querySelector("#clear-file-btn");
  if (clearBtn) {
    clearBtn.addEventListener("click", handleClearFile);
  }
}

/* ------------------------------------------------------------------ */
/*  Entry point                                                        */
/* ------------------------------------------------------------------ */

export async function init(container, signal) {
  _container = container;
  _signal = signal;

  signal.addEventListener("abort", () => {
    _container = null;
    _signal = null;
  });

  renderPage();
}
