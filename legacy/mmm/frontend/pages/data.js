/**
 * data.js — Import/Export Database page.
 *
 * Layout:
 *   ┌── EXPORT ────────────────────────────────────────────┐
 *   │  Download a full database backup as JSON.             │
 *   │  [ Export Database ]  (spinner while loading)         │
 *   └───────────────────────────────────────────────────────┘
 *   ┌── IMPORT ────────────────────────────────────────────┐
 *   │  ⚠️ This will replace ALL existing data.             │
 *   │  [ Choose File ] (.json only)                        │
 *   │  ┌── PREVIEW ─────────────────────────────────┐     │
 *   │  │  File: backup.json (1.2 MB)                 │     │
 *   │  │  Dumped at: 2026-06-10 12:34:56             │     │
 *   │  │  tag_categories: 5 rows                      │     │
 *   │  │  tags: 142 rows                              │     │
 *   │  │  ...                                         │     │
 *   │  │  [ Restore from Backup ] (red button)        │     │
 *   │  └──────────────────────────────────────────────┘     │
 *   └───────────────────────────────────────────────────────┘
 */

import { showToast } from "../shared/components.js";
import { API_BASE } from "../shared/api.js";

/* ------------------------------------------------------------------ */
/*  State                                                              */
/* ------------------------------------------------------------------ */

let _container = null;
let _signal = null;

/* ------------------------------------------------------------------ */
/*  Initialization                                                     */
/* ------------------------------------------------------------------ */

export async function init(container, signal) {
  _container = container;
  _signal = signal;

  container.innerHTML = `
    <div class="page-header">
      <h1><i class="fa-solid fa-database"></i> Import / Export</h1>
    </div>
    <div class="data-page-content">
      ${renderExportSection()}
      ${renderImportSection()}
    </div>
  `;

  wireExportButton(container);
  wireImportFileInput(container);
}

/* ------------------------------------------------------------------ */
/*  Export Section                                                     */
/* ------------------------------------------------------------------ */

function renderExportSection() {
  return `
    <div class="card" id="export-card">
      <div class="card-header">
        <h2><i class="fa-solid fa-download"></i> Export Database</h2>
      </div>
      <div class="card-body">
        <p>Download a complete backup of your database as a JSON file.
        This includes all tags, tracks, playlists, files, and configuration.</p>
        <div class="export-actions">
          <button class="btn btn-primary" id="export-btn">
            <i class="fa-solid fa-download"></i> Export Database
          </button>
          <span id="export-spinner" class="spinner" style="display:none;"></span>
        </div>
      </div>
    </div>
  `;
}

function wireExportButton(container) {
  const btn = container.querySelector("#export-btn");
  const spinner = container.querySelector("#export-spinner");
  if (!btn) return;

  btn.addEventListener("click", async () => {
    btn.disabled = true;
    spinner.style.display = "inline-block";

    try {
      const url = `${API_BASE}/api/dump`;
      const res = await fetch(url);

      if (!res.ok) {
        let detail = res.statusText;
        try {
          const err = await res.json();
          detail = err.error || detail;
        } catch {}
        throw new Error(`HTTP ${res.status}: ${detail}`);
      }

      // Get filename from Content-Disposition header or generate one
      const disposition = res.headers.get("Content-Disposition") || "";
      const match = disposition.match(/filename="?([^"]+)"?/);
      const filename = match
        ? match[1]
        : `momos-dump-${new Date().toISOString().slice(0, 19).replace(/[:-]/g, "")}.json`;

      // Read blob and trigger download
      const blob = await res.blob();
      const blobUrl = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = blobUrl;
      a.download = filename;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(blobUrl);

      showToast("Database exported successfully", "success");
    } catch (err) {
      if (err.name === "AbortError") return;
      showToast(`Export failed: ${err.message}`, "error");
    } finally {
      btn.disabled = false;
      spinner.style.display = "none";
    }
  });
}

/* ------------------------------------------------------------------ */
/*  Import Section                                                     */
/* ------------------------------------------------------------------ */

function renderImportSection() {
  return `
    <div class="card" id="import-card">
      <div class="card-header">
        <h2><i class="fa-solid fa-upload"></i> Import Database</h2>
      </div>
      <div class="card-body">
        <div class="warning-banner" style="background:var(--red-light,#fef2f2);border:1px solid var(--red,#ef4444);border-radius:8px;padding:12px 16px;margin-bottom:16px;">
          <p style="color:var(--red,#ef4444);font-weight:600;margin:0;">
            <i class="fa-solid fa-triangle-exclamation"></i>
            ⚠️ This will replace ALL existing data. Make sure you have a backup.
          </p>
        </div>

        <div class="import-file-row" style="margin-bottom:16px;">
          <label for="import-file-input" class="btn btn-secondary" style="cursor:pointer;">
            <i class="fa-solid fa-folder-open"></i> Choose File
          </label>
          <input type="file" id="import-file-input" accept=".json" style="display:none;" />
          <span id="import-file-name" style="margin-left:12px;color:var(--text-muted,#888);">No file selected</span>
        </div>

        <div id="import-preview" style="display:none;"></div>

        <div id="import-error" class="error-block" style="display:none;"></div>
        <div id="import-spinner" class="loading" style="display:none;">
          <div class="spinner"></div>
          <p>Restoring database...</p>
        </div>
      </div>
    </div>
  `;
}

function wireImportFileInput(container) {
  const fileInput = container.querySelector("#import-file-input");
  const fileNameEl = container.querySelector("#import-file-name");
  if (!fileInput) return;

  fileInput.addEventListener("change", () => {
    const file = fileInput.files?.[0];
    if (!file) {
      fileNameEl.textContent = "No file selected";
      container.querySelector("#import-preview").style.display = "none";
      return;
    }

    fileNameEl.textContent = file.name;

    // Read the file to show preview
    const reader = new FileReader();
    reader.onload = (e) => {
      try {
        const data = JSON.parse(e.target.result);
        renderPreview(container, file, data);
      } catch (err) {
        showPreviewError(
          container,
          "Invalid JSON file. Please select a valid database dump.",
        );
      }
    };
    reader.onerror = () => {
      showPreviewError(container, "Failed to read the selected file.");
    };
    reader.readAsText(file);
  });
}

function showPreviewError(container, message) {
  const previewEl = container.querySelector("#import-preview");
  previewEl.style.display = "block";
  previewEl.innerHTML = `
    <div class="error-block" style="margin-top:0;">
      <p>${message}</p>
    </div>
  `;
}

function renderPreview(container, file, data) {
  const previewEl = container.querySelector("#import-preview");

  // Validate this looks like a dump
  if (!data.dumped_at && !data.files) {
    previewEl.style.display = "block";
    previewEl.innerHTML = `
      <div class="error-block" style="margin-top:0;">
        <p>This doesn't look like a valid Momo's Music Manager database dump. The required fields are missing.</p>
      </div>
    `;
    return;
  }

  // Format dumped_at timestamp
  const dumpedAt = data.dumped_at
    ? new Date(data.dumped_at * 1000).toLocaleString()
    : "Unknown";

  // Count rows per table (skip empty ones)
  const tables = [
    { key: "tag_categories", label: "Tag Categories" },
    { key: "tags", label: "Tags" },
    { key: "tag_embeddings", label: "Tag Embeddings" },
    { key: "tag_energy_levels", label: "Tag Energy Levels" },
    { key: "folders", label: "Folders" },
    { key: "service_config", label: "Service Config" },
    { key: "service_tracks", label: "Service Tracks" },
    { key: "service_playlists", label: "Service Playlists" },
    { key: "service_playlist_tracks", label: "Service Playlist Tracks" },
    { key: "files", label: "Files" },
    { key: "playlist_subscriptions", label: "Playlist Subscriptions" },
    { key: "deemix_downloads", label: "Deemix Downloads" },
  ];

  const rowsHtml = tables
    .map((t) => {
      const count = Array.isArray(data[t.key]) ? data[t.key].length : 0;
      if (count === 0) return "";
      return `<tr><td>${t.label}</td><td style="text-align:right">${count.toLocaleString()} rows</td></tr>`;
    })
    .filter(Boolean)
    .join("");

  const totalRows = tables.reduce((sum, t) => {
    return sum + (Array.isArray(data[t.key]) ? data[t.key].length : 0);
  }, 0);

  const fileSize = (file.size / 1024 / 1024).toFixed(1);

  previewEl.style.display = "block";
  previewEl.innerHTML = `
    <div class="card" style="margin-top:0;border:1px solid var(--border,#333);">
      <div class="card-body">
        <div style="margin-bottom:12px;">
          <strong>File:</strong> ${escapeHtml(file.name)} (${fileSize} MB)
        </div>
        <div style="margin-bottom:12px;">
          <strong>Dumped at:</strong> ${dumpedAt}
        </div>
        <table class="data-table" style="margin-bottom:16px;">
          <thead>
            <tr>
              <th>Table</th>
              <th style="text-align:right">Rows</th>
            </tr>
          </thead>
          <tbody>
            ${rowsHtml}
          </tbody>
          <tfoot>
            <tr style="font-weight:600;">
              <td>Total</td>
              <td style="text-align:right">${totalRows.toLocaleString()} rows</td>
            </tr>
          </tfoot>
        </table>

        <button class="btn btn-danger" id="restore-btn">
          <i class="fa-solid fa-triangle-exclamation"></i> Restore from Backup
        </button>
      </div>
    </div>
  `;

  wireRestoreButton(container, file);
}

function wireRestoreButton(container, file) {
  const restoreBtn = container.querySelector("#restore-btn");
  if (!restoreBtn) return;

  const spinner = container.querySelector("#import-spinner");
  const errorEl = container.querySelector("#import-error");

  restoreBtn.addEventListener("click", async () => {
    // Double confirmation
    if (
      !confirm(
        "Are you sure you want to restore this backup? This will REPLACE ALL existing data and cannot be undone.",
      )
    ) {
      return;
    }

    restoreBtn.disabled = true;
    spinner.style.display = "flex";
    errorEl.style.display = "none";

    try {
      const formData = new FormData();
      formData.append("file", file);

      const url = `${API_BASE}/api/restore?confirm=true`;
      const res = await fetch(url, {
        method: "POST",
        body: formData,
      });

      if (!res.ok) {
        let detail = res.statusText;
        try {
          const err = await res.json();
          detail = err.error || err.message || detail;
        } catch {}
        throw new Error(`HTTP ${res.status}: ${detail}`);
      }

      const result = await res.json();
      showToast(result.message || "Database restored successfully!", "success");

      // Redirect to dashboard after 2 seconds
      setTimeout(() => {
        window.location.hash = "#dashboard";
      }, 2000);
    } catch (err) {
      if (err.name === "AbortError") return;
      errorEl.style.display = "block";
      errorEl.innerHTML = `<div class="error-icon"><i class="fas fa-exclamation-triangle"></i></div><h3>Restore failed</h3><p>${err.message}</p>`;
    } finally {
      restoreBtn.disabled = false;
      spinner.style.display = "none";
    }
  });
}

/* ------------------------------------------------------------------ */
/*  Escape HTML (local helper to avoid full import)                    */
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
