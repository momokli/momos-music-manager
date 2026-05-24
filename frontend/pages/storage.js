/**
 * storage.js — Storage management page.
 *
 * Views local vs backup storage status, triggers backups per folder,
 * previews and executes pruning of backed-up files.
 */

import { fetchJSON } from "../shared/api.js";
import {
  escapeHtml,
  renderLoading,
  renderErrorBlock,
  showToast,
  showConfirmModal,
} from "../shared/components.js";

/* ------------------------------------------------------------------ */
/*  State                                                              */
/* ------------------------------------------------------------------ */

let state = {
  status: null,
  settings: null,
  folders: [],
  pruneCandidates: [],
  loading: false,
  loadingPrune: false,
};

let _container = null;
let _signal = null;

/* ------------------------------------------------------------------ */
/*  Exports                                                            */
/* ------------------------------------------------------------------ */

export async function init(container, signal) {
  _container = container;
  _signal = signal;

  renderLayout(container);
  await loadStatus(container);
  await loadFolders(container);
  wireEvents(container);
}

/* ------------------------------------------------------------------ */
/*  Render                                                             */
/* ------------------------------------------------------------------ */

function renderLayout(container) {
  container.innerHTML = `
    <div class="page-header">
      <h1><i class="fas fa-hdd"></i> Storage</h1>
    </div>
    <div id="storage-status-cards"></div>
    <div id="storage-file-types"></div>
    <div id="storage-stem-preference"></div>
    <div id="storage-folders"></div>
    <div id="storage-prune-section">
      <h2 class="section-title"><i class="fas fa-trash-alt"></i> Prune Preview</h2>
      <div id="storage-prune-content"></div>
    </div>
  `;
}

function renderStatusCards(container, status) {
  const el = container.querySelector("#storage-status-cards");
  if (!el) return;

  const formatBytes = (bytes) => {
    if (bytes === 0) return "0 B";
    const k = 1024;
    const sizes = ["B", "KB", "MB", "GB", "TB"];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
  };

  el.innerHTML = `
    <div class="storage-section">
      <h2 class="storage-section-title">Summary</h2>
      <div class="storage-cards">
        <div class="storage-card" style="flex:1">
          <div class="storage-card-icon"><i class="fas fa-laptop"></i></div>
          <div class="storage-card-body">
            <div class="storage-card-value">${(status.localFileCount ?? 0).toLocaleString()}</div>
            <div class="storage-card-label">Local Files</div>
            <div class="storage-card-hint">${formatBytes(status.localSizeBytes ?? 0)} total</div>
          </div>
        </div>
        <div class="storage-card" style="flex:1">
          <div class="storage-card-icon"><i class="fas fa-cloud"></i></div>
          <div class="storage-card-body">
            <div class="storage-card-value">${(status.backupCount ?? 0).toLocaleString()}</div>
            <div class="storage-card-label">Backed Up</div>
          </div>
        </div>
        <div class="storage-card" style="flex:1;border-color: ${(status.pruneCandidateCount ?? 0) > 0 ? "var(--red)" : "var(--green)"}">
          <div class="storage-card-icon"><i class="fas fa-trash-alt"></i></div>
          <div class="storage-card-body">
            <div class="storage-card-value">${(status.pruneCandidateCount ?? 0).toLocaleString()}</div>
            <div class="storage-card-label">Prune Candidates</div>
            <div class="storage-card-hint">${formatBytes(status.pruneCandidateBytes ?? 0)} can be freed</div>
          </div>
        </div>
      </div>
    </div>
  `;
}

function renderFileTypeBreakdown(container, status) {
  const el = container.querySelector("#storage-file-types");
  if (!el) return;

  const formatBytes = (bytes) => {
    if (bytes === 0) return "0 B";
    const k = 1024;
    const sizes = ["B", "KB", "MB", "GB", "TB"];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
  };

  const types = [
    {
      name: "Stems",
      count: status.localStems ?? 0,
      size: status.localStemsSize ?? status.local_stems_size ?? 0,
    },
    {
      name: "FLACs",
      count: status.localFlacs ?? status.local_flacs ?? 0,
      size: status.localFlacsSize ?? status.local_flacs_size ?? 0,
    },
    {
      name: "WAVs",
      count: status.localWavs ?? status.local_wavs ?? 0,
      size: status.localWavsSize ?? status.local_wavs_size ?? 0,
    },
    {
      name: "MP3s",
      count: status.localMp3s ?? status.local_mp3s ?? 0,
      size: status.localMp3sSize ?? status.local_mp3s_size ?? 0,
    },
  ].filter((t) => t.count > 0);

  const totalSize = types.reduce((a, t) => a + t.size, 0);

  el.innerHTML = `<div class="storage-section">
    <h2 class="storage-section-title">File Types</h2>
    <div class="file-type-breakdown">
      ${types
        .map((t) => {
          const pct = totalSize > 0 ? (t.size / totalSize) * 100 : 0;
          return `<div class="file-type-row">
          <span class="file-type-name">${t.name}</span>
          <div class="file-type-bar-wrap">
            <div class="file-type-bar" style="width:${pct}%"></div>
          </div>
          <span class="file-type-count">${t.count.toLocaleString()}</span>
          <span class="file-type-size">${formatBytes(t.size)}</span>
        </div>`;
        })
        .join("")}
    </div>
  </div>`;
}

function renderStemPreference(container, stemPreferred) {
  const el = container.querySelector("#storage-stem-preference");
  if (!el) return;
  el.innerHTML = `
    <div class="storage-section">
      <h2 class="storage-section-title">Format Preferences</h2>
      <div class="storage-preference-card">
        <label class="preference-toggle">
          <input type="checkbox" id="stem-pref-toggle" ${stemPreferred ? "checked" : ""}>
          <span class="toggle-slider"></span>
          <span class="toggle-label">Prefer stem files</span>
        </label>
        <p class="preference-hint">
          When enabled: if a track has a stem.m4a version, other formats (FLAC, MP3, WAV)
          with the same ISRC become eligible for pruning if backed up.
          Currently <strong>${(status.localFlacs ?? status.local_flacs ?? 0).toLocaleString()} FLACs</strong> would become prune candidates.
        </p>
      </div>
    </div>
  `;
}

function renderFolders(container, folders) {
  const el = container.querySelector("#storage-folders");
  if (!el) return;

  let html = '<div class="folders-section"><h2 class="section-title">Folders</h2>';

  if (folders.length === 0) {
    html += '<div class="text-muted" style="padding:1rem">No folders configured</div>';
  } else {
    html += '<div class="folder-cards">';
    for (const f of folders) {
      const folderPath = escapeHtml(f.path ?? f.folderPath ?? f.folder_path ?? "?");
      const fileCount = f.fileCount ?? f.file_count ?? "—";
      const bp = f.backupPath ?? f.backup_path ?? "";
      const hasBackup = !!bp;
      const scanSrc = f.scanSources ?? f.scan_sources ?? false;

      html += `<div class="folder-card${hasBackup ? " has-backup" : ""}">
        <div class="folder-card-header">
          <code class="folder-path">${folderPath}</code>
          <span class="folder-file-count">${fileCount} files</span>
        </div>
        <div class="folder-card-body">
          <div class="folder-info-row">
            <span class="folder-info-label">Backup</span>
            <span class="folder-info-value">${hasBackup ? escapeHtml(bp) : '<span class="text-muted">Not configured</span>'}</span>
          </div>
          <div class="folder-info-row">
            <span class="folder-info-label">WAV Sources</span>
            <span class="folder-info-value">${scanSrc ? '<span style="color:var(--green)">Enabled</span>' : '<span class="text-muted">Disabled</span>'}</span>
          </div>
          <div class="folder-info-row">
            <span class="folder-info-label">Auto Backup</span>
            <span class="folder-info-value">${(f.autoBackup ?? f.auto_backup ?? true) ? '<span style="color:var(--green)">Enabled</span>' : '<span class="text-muted">Disabled</span>'}</span>
          </div>
        </div>
        <div class="folder-card-actions">
          <button class="btn btn-sm" data-act="backup-folder" data-id="${f.id}" ${hasBackup ? "" : "disabled title='Set backup path first'"}>
            <i class="fas fa-cloud-upload-alt"></i> Backup
          </button>
          ${
            scanSrc
              ? `<button class="btn btn-sm" data-act="scan-wavs" data-id="${f.id}">
            <i class="fas fa-wave-square"></i> Scan WAVs
          </button>`
              : ""
          }
        </div>
      </div>`;
    }
    html += "</div>";
  }

  html += "</div>";
  el.innerHTML = html;
}

function renderPrunePreview(container, candidates) {
  const el = container.querySelector("#storage-prune-content");
  if (!el) return;

  if (!candidates || candidates.length === 0) {
    el.innerHTML =
      '<div class="text-muted">No files eligible for pruning. All local files are either followed/backpack-tagged, or not yet backed up.</div>';
    return;
  }

  const formatBytes = (bytes) => {
    if (bytes === 0) return "0 B";
    const k = 1024;
    const sizes = ["B", "KB", "MB", "GB", "TB"];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
  };

  const reasonLabels = {
    not_followed:
      '<span class="badge" style="background:rgba(100,116,139,0.1);color:var(--text-muted)">Not followed</span>',
  };

  const totalBytes = candidates.reduce((sum, c) => sum + c.fileSize, 0);

  let html = `<div style="margin-bottom:1rem;display:flex;gap:1rem;align-items:center">
    <span><strong>${candidates.length.toLocaleString()}</strong> files, <strong>${formatBytes(totalBytes)}</strong> can be freed</span>
    <button class="btn btn-red" id="prune-execute-btn">
      <i class="fas fa-trash-alt"></i> Delete All Prune Candidates
    </button>
  </div>`;

  html +=
    '<div class="table-wrap" style="max-height:400px;overflow-y:auto"><table class="table"><thead><tr>';
  html +=
    "<th>Title</th><th>Artist</th><th>Type</th><th>Size</th><th>Reason</th><th>Backup</th>";
  html += "</tr></thead><tbody>";

  for (const c of candidates) {
    html += "<tr>";
    html += `<td>${escapeHtml(c.title || "—")}</td>`;
    html += `<td>${escapeHtml(c.artist || "—")}</td>`;
    html += `<td>${c.fileType}</td>`;
    html += `<td>${formatBytes(c.fileSize)}</td>`;
    html += `<td>${reasonLabels[c.reason] || c.reason}</td>`;
    html += `<td>${c.backupPath ? escapeHtml(c.backupPath) : '<span class="text-muted">—</span>'}</td>`;
    html += "</tr>";
  }

  html += "</tbody></table></div>";
  el.innerHTML = html;
}

/* ------------------------------------------------------------------ */
/*  Data Loading                                                       */
/* ------------------------------------------------------------------ */

async function loadStatus(container) {
  try {
    state.loading = true;
    const [statusResp, settingsResp] = await Promise.all([
      fetchJSON("/api/storage/status"),
      fetchJSON("/api/storage/settings"),
    ]);
    state.status = statusResp.data;
    state.settings = settingsResp.data || settingsResp;
    renderStatusCards(container, state.status);
    renderFileTypeBreakdown(container, state.status);
    renderStemPreference(container, state.settings.stemPreferred);
  } catch (err) {
    showToast(`Failed to load storage status: ${err.message}`, "error");
  } finally {
    state.loading = false;
  }
}

async function loadFolders(container) {
  try {
    const resp = await fetchJSON("/api/folders?page_size=100");
    state.folders = resp.data || [];
    renderFolders(container, state.folders);
  } catch (err) {
    showToast(`Failed to load folders: ${err.message}`, "error");
  }
}

async function loadPrunePreview(container) {
  try {
    state.loadingPrune = true;
    const el = container.querySelector("#storage-prune-content");
    if (el) el.innerHTML = renderLoading();

    const resp = await fetchJSON("/api/storage/prune-preview", {
      method: "POST",
    });
    state.pruneCandidates = resp.data || [];
    renderPrunePreview(container, state.pruneCandidates);
  } catch (err) {
    showToast(`Failed to load prune preview: ${err.message}`, "error");
  } finally {
    state.loadingPrune = false;
  }
}

/* ------------------------------------------------------------------ */
/*  Events                                                             */
/* ------------------------------------------------------------------ */

function wireEvents(container) {
  // Delegate change events (handles re-rendered elements via bubbling)
  container.addEventListener("change", async (e) => {
    const stemToggle = e.target.closest("#stem-pref-toggle");
    if (stemToggle) {
      await fetchJSON("/api/storage/settings", {
        method: "PUT",
        body: JSON.stringify({ stemPreferred: stemToggle.checked }),
      });
      showToast("Stem preference updated", "success");
      await loadStatus(container);
      await loadPrunePreview(container);
    }
  });

  // Delegate clicks
  container.addEventListener("click", async (e) => {
    const actBtn = e.target.closest("[data-act]");
    if (actBtn) {
      const act = actBtn.dataset.act;
      const id = parseInt(actBtn.dataset.id, 10);

      if (act === "backup-folder") {
        actBtn.disabled = true;
        actBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i>';
        try {
          const resp = await fetchJSON(`/api/storage/backup/${id}`, {
            method: "POST",
          });
          const result = resp.data;
          showToast(
            `Backup complete: ${result.copied} copied, ${result.verified} verified, ${result.errors} errors`,
            "success",
          );
          await loadStatus(container);
          await loadFolders(container);
          await loadPrunePreview(container);
        } catch (err) {
          showToast(`Backup failed: ${err.message}`, "error");
          actBtn.disabled = false;
          actBtn.innerHTML = '<i class="fas fa-cloud-upload-alt"></i> Backup';
        }
      }

      if (act === "scan-wavs") {
        actBtn.disabled = true;
        actBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i>';
        try {
          const resp = await fetchJSON(`/api/folders/${id}/scan-sources`, {
            method: "POST",
          });
          const result = resp.data;
          showToast(
            `WAV scan: ${result.wavIndexed} WAVs indexed, ${result.linkedToStems} linked to stems`,
            "success",
          );
          await loadStatus(container);
        } catch (err) {
          showToast(`WAV scan failed: ${err.message}`, "error");
          actBtn.disabled = false;
          actBtn.innerHTML = '<i class="fas fa-wave-square"></i> Scan WAVs';
        }
      }
    }

    // Prune execute button
    const pruneBtn = e.target.closest("#prune-execute-btn");
    if (pruneBtn) {
      e.preventDefault();
      const confirmed = await showConfirmModal(
        "Delete Prune Candidates",
        `Are you sure you want to delete <strong>${state.pruneCandidates.length}</strong> local files? They are backed up and will be removed from local storage. This cannot be undone.`,
        "Delete Files",
        "red",
      );
      if (!confirmed) return;

      pruneBtn.disabled = true;
      pruneBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Deleting...';
      try {
        const resp = await fetchJSON("/api/storage/prune", {
          method: "POST",
        });
        const result = resp.data;
        showToast(
          `Pruned ${result.deleted} files (${formatBytesLocal(result.freedBytes)})`,
          "success",
        );
        await loadStatus(container);
        await loadPrunePreview(container);
      } catch (err) {
        showToast(`Prune failed: ${err.message}`, "error");
        pruneBtn.disabled = false;
        pruneBtn.innerHTML =
          '<i class="fas fa-trash-alt"></i> Delete All Prune Candidates';
      }
    }
  });
}

function formatBytesLocal(bytes) {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}
