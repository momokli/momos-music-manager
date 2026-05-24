/**
 * folder-detail.js — Single folder detail page.
 * Shows detailed stats and backup status for a folder.
 *   #folder-detail?id=<folder_id>
 *
 * API: GET /api/folders/{id}/stats
 */

import { fetchJSON } from "../shared/api.js";

/* ------------------------------------------------------------------ */
/*  State                                                              */
/* ------------------------------------------------------------------ */

let abortController = null;

/* ------------------------------------------------------------------ */
/*  Initialization                                                     */
/* ------------------------------------------------------------------ */

export async function init(container, signal) {
  const id = getIdFromHash();
  if (!id) {
    container.innerHTML = renderError("No folder ID specified. Use #folder-detail?id=123");
    return;
  }

  abortController = new AbortController();
  const combinedSignal = signal || abortController.signal;

  container.innerHTML = renderLoading();

  try {
    const resp = await fetchJSON(`/api/folders/${id}/stats`, { signal: combinedSignal });
    const data = resp.data || resp;
    container.innerHTML = renderPage(data);
  } catch (err) {
    if (combinedSignal.aborted) return;
    container.innerHTML = renderError(`Failed to load: ${err.message}`);
  }
}

/* ------------------------------------------------------------------ */
/*  Layout                                                             */
/* ------------------------------------------------------------------ */

function renderLoading() {
  return `<div class="detail-loading"><i class="fas fa-spinner fa-spin"></i> Loading folder details…</div>`;
}

function renderError(msg) {
  return `<div class="detail-error"><i class="fas fa-triangle-exclamation"></i> ${escHtml(msg)}</div>`;
}

function renderPage(data) {
  const s = data;
  const fp = s.folderPath ?? s.folder_path ?? "";
  const name = fp.split("/").pop() || fp;

  return /* html */ `
    <div class="page-header">
      <h1><i class="fas fa-folder-open"></i> ${escHtml(name)}</h1>
      <span class="page-subtitle">${escHtml(fp)}</span>
    </div>

    <div class="detail-grid">
      ${renderSection("Overview", renderOverviewCards(s))}
      ${renderSection("File Types", renderFileTypeBreakdown(s))}
      ${renderSection("Configuration", renderConfigTable(s))}
    </div>
  `;
}

/* ------------------------------------------------------------------ */
/*  Sections                                                           */
/* ------------------------------------------------------------------ */

function renderSection(title, body) {
  return /* html */ `
    <div class="detail-section">
      <h2 class="detail-section-title">${title}</h2>
      ${body}
    </div>
  `;
}

function renderOverviewCards(s) {
  return /* html */ `
    <div class="storage-cards" style="margin-bottom:0">
      <div class="storage-card">
        <div class="storage-card-icon"><i class="fas fa-file-audio"></i></div>
        <div class="storage-card-body">
          <div class="storage-card-value">${(s.totalFiles ?? s.total_files ?? 0).toLocaleString()}</div>
          <div class="storage-card-label">Total Files</div>
          <div class="storage-card-hint">${formatBytes(s.totalSizeBytes ?? s.total_size_bytes ?? 0)}</div>
        </div>
      </div>
      <div class="storage-card">
        <div class="storage-card-icon"><i class="fas fa-cloud-arrow-up"></i></div>
        <div class="storage-card-body">
          <div class="storage-card-value">${(s.backedUp ?? s.backed_up ?? 0).toLocaleString()}</div>
          <div class="storage-card-label">Backed Up</div>
          <div class="storage-card-hint">${formatBytes(s.backedUpSizeBytes ?? s.backed_up_size_bytes ?? 0)}</div>
        </div>
      </div>
      <div class="storage-card">
        <div class="storage-card-icon"><i class="fas fa-wave-square"></i></div>
        <div class="storage-card-body">
          <div class="storage-card-value">${(s.wavSourceDirs ?? s.wav_source_dirs ?? 0).toLocaleString()}</div>
          <div class="storage-card-label">WAV Source Dirs</div>
          <div class="storage-card-hint">${(s.wavSourceFiles ?? s.wav_source_files ?? 0).toLocaleString()} files, ${(s.wavBackedUp ?? s.wav_backed_up ?? 0).toLocaleString()} backed up</div>
        </div>
      </div>
    </div>
  `;
}

function renderFileTypeBreakdown(s) {
  return renderKvTable([
    ["Stems", (s.stems ?? 0).toLocaleString()],
    ["FLACs", (s.flacs ?? 0).toLocaleString()],
    ["WAVs", (s.wavs ?? 0).toLocaleString()],
    ["MP3s", (s.mp3s ?? 0).toLocaleString()],
    ["Other", (s.other ?? 0).toLocaleString()],
  ]);
}

function renderConfigTable(s) {
  const backupPath = s.backupPath ?? s.backup_path;
  return renderKvTable([
    [
      "Backup Path",
      backupPath ? `<code>${escHtml(backupPath)}</code>` : '<span class="text-muted">Not configured</span>',
    ],
    [
      "WAV Sources",
      (s.scanSources ?? s.scan_sources) ? "Enabled" : "Disabled",
    ],
    [
      "Watching",
      (s.watchEnabled ?? s.watch_enabled) ? "Active" : "Paused",
    ],
    [
      "Recursive",
      (s.scanRecursive ?? s.scan_recursive) ? "Yes" : "No",
    ],
    [
      "Max Depth",
      s.maxDepth ?? s.max_depth ?? 1,
    ],
    [
      "Last Scanned",
      (s.lastScanned ?? s.last_scanned)
        ? new Date((s.lastScanned ?? s.last_scanned) * 1000).toLocaleString()
        : "Never",
    ],
  ]);
}

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

function renderKvTable(rows) {
  return /* html */ `
    <table class="detail-kv">
      <tbody>
        ${rows
          .filter(([l]) => l != null)
          .map(
            ([label, value]) => `
          <tr>
            <th>${label}</th>
            <td>${value}</td>
          </tr>
        `,
          )
          .join("")}
      </tbody>
    </table>
  `;
}

function formatBytes(bytes) {
  if (!bytes || bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}

function escHtml(s) {
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function getIdFromHash() {
  const raw = window.location.hash.replace(/^#/, "");
  const [page, query] = raw.split("?");
  if (page !== "folder-detail" || !query) return null;
  const params = new URLSearchParams(query);
  return parseInt(params.get("id"), 10) || null;
}
