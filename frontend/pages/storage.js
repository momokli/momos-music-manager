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
  await loadFormatPriority(container);
  await loadPrunePreview(container);
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
    <div id="storage-format-priority"></div>
    <div id="storage-folders"></div>
    <div id="storage-prune-section">
      <h2 class="section-title"><i class="fas fa-trash-alt"></i> Prune Preview</h2>
      <div id="storage-prune-filters"></div>
      <div id="storage-prune-content"></div>
    </div>
  `;
}

function renderStatusCards(container, status) {
  const el = container.querySelector("#storage-status-cards");
  if (!el) return;

  const formatBytes = (bytes) => {
    if (!bytes || bytes === 0) return "0 B";
    const k = 1024;
    const sizes = ["B", "KB", "MB", "GB", "TB"];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
  };

  const lfc = status.localFileCount ?? 0;
  const tfc = status.trackedFileCount ?? 0;
  const tsize = status.trackedSizeBytes ?? 0;
  const bc = status.backupCount ?? 0;
  const nb = tfc - bc; // not backed up
  const pcc = status.pruneCandidateCount ?? 0;
  const pcb = status.pruneCandidateBytes ?? 0;

  // Warning banner: local tracking missing but files exist in DB
  let warningHtml = "";
  if (lfc === 0 && tfc > 0) {
    warningHtml = `
      <div class="storage-warning">
        <i class="fas fa-exclamation-triangle"></i>
        No local disk presence data. Run a full scan to detect which files are currently on disk.
        <button class="btn btn-sm" id="full-scan-btn">Run Full Scan</button>
      </div>
    `;
  }

  el.innerHTML = `
    ${warningHtml}
    <div class="storage-section">
      <h2 class="storage-section-title">Summary</h2>
      <div class="storage-cards">
        <div class="storage-card" style="flex:1">
          <div class="storage-card-icon"><i class="fas fa-laptop"></i></div>
          <div class="storage-card-body">
            <div class="storage-card-value">${lfc.toLocaleString()}</div>
            <div class="storage-card-label">On Disk</div>
            <div class="storage-card-hint">${formatBytes(status.localSizeBytes ?? 0)} total</div>
          </div>
        </div>
        <div class="storage-card" style="flex:1">
          <div class="storage-card-icon"><i class="fas fa-cloud"></i></div>
          <div class="storage-card-body">
            <div class="storage-card-value">${bc.toLocaleString()}</div>
            <div class="storage-card-label">On Backup</div>
          </div>
        </div>
        <div class="storage-card" style="flex:1">
          <div class="storage-card-icon"><i class="fas fa-database"></i></div>
          <div class="storage-card-body">
            <div class="storage-card-value">${tfc.toLocaleString()}</div>
            <div class="storage-card-label">Tracked</div>
            <div class="storage-card-hint">${formatBytes(tsize)} archive</div>
          </div>
        </div>
        <div class="storage-card" style="flex:1;border-color: ${nb > 0 ? "var(--yellow)" : "var(--green)"}">
          <div class="storage-card-icon"><i class="fas fa-clock"></i></div>
          <div class="storage-card-body">
            <div class="storage-card-value">${nb.toLocaleString()}</div>
            <div class="storage-card-label">Not Backed Up</div>
            <div class="storage-card-hint">files need backup</div>
          </div>
        </div>
        <div class="storage-card" style="flex:1;border-color: ${pcc > 0 ? "var(--red)" : "var(--green)"}">
          <div class="storage-card-icon"><i class="fas fa-trash-alt"></i></div>
          <div class="storage-card-body">
            <div class="storage-card-value">${pcc.toLocaleString()}</div>
            <div class="storage-card-label">Prune Candidates</div>
            <div class="storage-card-hint">${formatBytes(pcb)} can be freed</div>
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

function renderPruneFilters(container) {
  const el = container.querySelector("#storage-prune-filters");
  if (!el) return;

  const stemOnly = container.querySelector(".prune-filter-stem-only")?.checked ?? false;
  const typeFilter = container.querySelector(".prune-filter-type")?.value ?? "all";

  let html = `
    <div class="prune-filter-bar">
      <label class="prune-filter-checkbox">
        <input type="checkbox" class="prune-filter-stem-only" ${stemOnly ? "checked" : ""}>
        Show only files with stem variant
      </label>
      <select class="prune-filter-type">
        <option value="all" ${typeFilter === "all" ? "selected" : ""}>All types</option>
        <option value="flac" ${typeFilter === "flac" ? "selected" : ""}>FLACs only</option>
        <option value="wav" ${typeFilter === "wav" ? "selected" : ""}>WAVs only</option>
        <option value="stem.m4a" ${typeFilter === "stem.m4a" ? "selected" : ""}>Stems only</option>
      </select>
      <button class="btn btn-sm" id="prune-select-stem-variants">Select all with stem variant</button>
      <button class="btn btn-sm" id="prune-deselect-all">Deselect all</button>
    </div>
  `;
  el.innerHTML = html;
}

function renderPrunePreview(container, candidates) {
  const el = container.querySelector("#storage-prune-content");
  if (!el) return;

  if (!candidates || candidates.length === 0) {
    el.innerHTML =
      '<div class="text-muted">No files eligible for pruning. All local files are either followed/backpack-tagged, or not yet backed up.</div>';
    // Clear filter bar since there's nothing to filter
    const fb = container.querySelector("#storage-prune-filters");
    if (fb) fb.innerHTML = "";
    return;
  }

  // Apply active filters
  let filtered = candidates;
  const stemOnly = container.querySelector(".prune-filter-stem-only")?.checked ?? false;
  const typeVal = container.querySelector(".prune-filter-type")?.value ?? "all";
  if (stemOnly) {
    filtered = filtered.filter((c) => c.hasStemVariant);
  }
  if (typeVal !== "all") {
    filtered = filtered.filter((c) => c.fileType === typeVal);
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

  const totalBytes = filtered.reduce((sum, c) => sum + c.fileSize, 0);

  let html = `<div style="margin-bottom:1rem;display:flex;gap:1rem;align-items:center">
    <span><strong>${filtered.length.toLocaleString()}</strong> files (of ${candidates.length.toLocaleString()} total), <strong>${formatBytes(totalBytes)}</strong> can be freed</span>
    <button class="btn btn-red" id="prune-execute-btn">
      <i class="fas fa-trash-alt"></i> Delete All Selected
    </button>
  </div>`;

  html +=
    '<div class="table-wrap" style="max-height:400px;overflow-y:auto"><table class="table"><thead><tr>';
  html +=
    '<th style="width:32px"></th><th>Title</th><th>Artist</th><th>Type</th><th>Stem Variant</th><th>Size</th><th>Reason</th><th>Backup</th>';
  html += "</tr></thead><tbody>";

  for (const c of filtered) {
    html += "<tr>";
    html += `<td><input type="checkbox" class="prune-checkbox" data-file-id="${c.fileId}" checked></td>`;
    html += `<td>${escapeHtml(c.title || "—")}</td>`;
    html += `<td>${escapeHtml(c.artist || "—")}</td>`;
    html += `<td>${c.fileType}</td>`;
    html += `<td>${
      c.hasStemVariant
        ? '<span class="variant-badge variant-badge-stem" style="font-size:0.65rem;padding:0.1rem 0.4rem">stem ✓</span>'
        : '<span class="text-muted" style="font-size:0.75rem">—</span>'
    }</td>`;
    html += `<td>${formatBytes(c.fileSize)}</td>`;
    html += `<td>${reasonLabels[c.reason] || c.reason}</td>`;
    html += `<td>${c.backupPath ? escapeHtml(c.backupPath) : '<span class="text-muted">—</span>'}</td>`;
    html += "</tr>";
  }

  html += "</tbody></table></div>";
  el.innerHTML = html;

  // Render filters (filter bar lives outside el, never destroyed by innerHTML)
  renderPruneFilters(container);
  updatePruneSelectedCount(container);
}

function updatePruneSelectedCount(container) {
  const checked = container.querySelectorAll(".prune-checkbox:checked").length;
  const total = container.querySelectorAll(".prune-checkbox").length;
  const btn = container.querySelector("#prune-execute-btn");
  if (btn) {
    btn.innerHTML = `<i class="fas fa-trash-alt"></i> Delete Selected (${checked} of ${total})`;
  }
}

/* ------------------------------------------------------------------ */
/*  Data Loading                                                       */
/* ------------------------------------------------------------------ */

async function loadStatus(container) {
  try {
    state.loading = true;
    const resp = await fetchJSON("/api/storage/status");
    state.status = resp.data;
    renderStatusCards(container, state.status);
    renderFileTypeBreakdown(container, state.status);
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

/* ------------------------------------------------------------------ */
/*  Format Priority                                                     */
/* ------------------------------------------------------------------ */

async function loadFormatPriority(container) {
  const el = container.querySelector("#storage-format-priority");
  if (!el) return;
  try {
    const resp = await fetchJSON("/api/storage/settings/format-priority");
    const priorities = resp.data?.priorities ?? ["stem.m4a", "flac", "mp3", "wav"];
    renderFormatPriority(el, priorities);
  } catch (err) {
    el.innerHTML = renderErrorBlock({
      title: "Failed to load format priority settings",
      detail: err.message,
    });
  }
}

function renderFormatPriority(el, priorities) {
  let html = `<div class="card" id="format-priority-card">
    <h3><i class="fas fa-sort-amount-down"></i> Format Priority</h3>
    <p class="help-text">When pulling from backup, higher formats are preferred.</p>
    <ul class="format-priority-list" id="format-priority-list">
      ${priorities
        .map(
          (fmt, i) => `
        <li class="format-priority-item" data-format="${escapeHtml(fmt)}">
          <span class="format-priority-drag"><i class="fas fa-grip-lines"></i></span>
          <span class="format-priority-name">${escapeHtml(fmt)}</span>
          <span class="format-priority-buttons">
            <button class="btn btn-sm btn-icon format-priority-up" ${i === 0 ? "disabled" : ""} title="Move up"><i class="fas fa-chevron-up"></i></button>
            <button class="btn btn-sm btn-icon format-priority-down" ${i === priorities.length - 1 ? "disabled" : ""} title="Move down"><i class="fas fa-chevron-down"></i></button>
          </span>
        </li>`,
        )
        .join("")}
    </ul>
    <div class="format-priority-actions">
      <input type="text" id="format-priority-add" placeholder="flac" class="input-text" style="width: 120px; margin-right: 0.5rem;" />
      <button id="format-priority-add-btn" class="btn btn-sm">Add</button>
      <button id="format-priority-reset" class="btn btn-sm btn-outline">Reset</button>
      <button id="format-priority-save" class="btn btn-sm btn-primary">Save</button>
    </div>
  </div>`;
  el.innerHTML = html;
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
    // Prune filter toggles
    const stemOnlyToggle = e.target.closest(".prune-filter-stem-only");
    if (stemOnlyToggle) {
      renderPrunePreview(container, state.pruneCandidates);
    }

    const typeFilter = e.target.closest(".prune-filter-type");
    if (typeFilter) {
      renderPrunePreview(container, state.pruneCandidates);
    }

    // Prune checkbox change
    const pruneCheckbox = e.target.closest(".prune-checkbox");
    if (pruneCheckbox) {
      updatePruneSelectedCount(container);
    }
  });

  // Delegate clicks
  container.addEventListener("click", async (e) => {
    // Full scan button (in warning banner)
    const scanBtn = e.target.closest("#full-scan-btn");
    if (scanBtn) {
      scanBtn.disabled = true;
      scanBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Scanning...';
      try {
        let scanned = 0;
        for (const folder of state.folders) {
          try {
            await fetchJSON(`/api/folders/${folder.id}/scan?mode=full`, {
              method: "POST",
            });
            scanned++;
          } catch (scanErr) {
            console.warn(`Scan failed for folder ${folder.id}:`, scanErr);
          }
        }
        showToast(`Full scan triggered for ${scanned} folder(s)`, "success");
        // Reload status after scan completes (give it a moment)
        setTimeout(async () => {
          await loadStatus(container);
        }, 3000);
      } catch (err) {
        showToast(`Failed to trigger scan: ${err.message}`, "error");
        scanBtn.disabled = false;
        scanBtn.innerHTML = "Run Full Scan";
      }
      return;
    }
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

    // ── Format Priority ──────────────────────────────────────────────

    // Move up ▲
    const upBtn = e.target.closest(".format-priority-up");
    if (upBtn) {
      const li = upBtn.closest(".format-priority-item");
      const prev = li?.previousElementSibling;
      if (li && prev) {
        li.parentNode.insertBefore(li, prev);
        // Re-enable/disable buttons after move
        const list = li.parentNode;
        Array.from(list.children).forEach((item, i) => {
          const up = item.querySelector(".format-priority-up");
          const down = item.querySelector(".format-priority-down");
          if (up) up.disabled = i === 0;
          if (down) down.disabled = i === list.children.length - 1;
        });
      }
      return;
    }

    // Move down ▼
    const downBtn = e.target.closest(".format-priority-down");
    if (downBtn) {
      const li = downBtn.closest(".format-priority-item");
      const next = li?.nextElementSibling;
      if (li && next) {
        li.parentNode.insertBefore(next, li);
        // Re-enable/disable buttons after move
        const list = li.parentNode;
        Array.from(list.children).forEach((item, i) => {
          const up = item.querySelector(".format-priority-up");
          const down = item.querySelector(".format-priority-down");
          if (up) up.disabled = i === 0;
          if (down) down.disabled = i === list.children.length - 1;
        });
      }
      return;
    }

    // Save
    const saveBtn = e.target.closest("#format-priority-save");
    if (saveBtn) {
      const items = container.querySelectorAll(".format-priority-item");
      const priorities = Array.from(items).map((li) => li.dataset.format);
      try {
        await fetchJSON("/api/storage/settings/format-priority", {
          method: "PUT",
          body: JSON.stringify({ priorities }),
        });
        showToast("Format priority saved", "success");
      } catch (err) {
        showToast("Failed to save format priority: " + err.message, "error");
      }
      return;
    }

    // Reset
    const resetBtn = e.target.closest("#format-priority-reset");
    if (resetBtn) {
      await loadFormatPriority(container);
      showToast("Reset to defaults", "success");
      return;
    }

    // Add format
    const addBtn = e.target.closest("#format-priority-add-btn");
    if (addBtn) {
      const input = container.querySelector("#format-priority-add");
      const fmt = input?.value?.trim().toLowerCase();
      const list = container.querySelector("#format-priority-list");
      if (!fmt || !list) return;
      if (!/^[a-z0-9.]+$/.test(fmt)) {
        showToast("Invalid format: " + fmt, "error");
        return;
      }
      // Check for duplicate
      const existing = list.querySelector(`.format-priority-item[data-format="${fmt}"]`);
      if (existing) {
        showToast("Format already in list", "error");
        return;
      }
      const li = document.createElement("li");
      li.className = "format-priority-item";
      li.dataset.format = fmt;
      li.innerHTML = `<span class="format-priority-drag"><i class="fas fa-grip-lines"></i></span>
        <span class="format-priority-name">${escapeHtml(fmt)}</span>
        <span class="format-priority-buttons">
          <button class="btn btn-sm btn-icon format-priority-up" title="Move up"><i class="fas fa-chevron-up"></i></button>
          <button class="btn btn-sm btn-icon format-priority-down" title="Move down"><i class="fas fa-chevron-down"></i></button>
        </span>`;
      list.appendChild(li);
      // Update button states
      Array.from(list.children).forEach((item, i) => {
        const up = item.querySelector(".format-priority-up");
        const down = item.querySelector(".format-priority-down");
        if (up) up.disabled = i === 0;
        if (down) down.disabled = i === list.children.length - 1;
      });
      input.value = "";
      return;
    }

    // ── Prune filters / actions ─────────────────────────────────────

    // Select all with stem variant (only affects stem-variant items, leaves others untouched)
    const selectStem = e.target.closest("#prune-select-stem-variants");
    if (selectStem) {
      container.querySelectorAll(".prune-checkbox").forEach((cb) => {
        const fileId = parseInt(cb.dataset.fileId, 10);
        const candidate = state.pruneCandidates.find((c) => c.fileId === fileId);
        if (candidate && candidate.hasStemVariant) {
          cb.checked = true;
        }
      });
      updatePruneSelectedCount(container);
    }

    // Deselect all
    const deselectAll = e.target.closest("#prune-deselect-all");
    if (deselectAll) {
      container.querySelectorAll(".prune-checkbox").forEach((cb) => {
        cb.checked = false;
      });
      updatePruneSelectedCount(container);
    }

    // Prune execute button
    const pruneBtn = e.target.closest("#prune-execute-btn");
    if (pruneBtn) {
      e.preventDefault();
      // Collect only checked file IDs
      const selectedFileIds = [];
      container.querySelectorAll(".prune-checkbox:checked").forEach((cb) => {
        selectedFileIds.push(parseInt(cb.dataset.fileId, 10));
      });

      if (selectedFileIds.length === 0) {
        showToast("No files selected for pruning", "error");
        return;
      }

      const confirmed = await showConfirmModal(
        "Delete Prune Candidates",
        `Are you sure you want to delete <strong>${selectedFileIds.length}</strong> local files? They are backed up and will be removed from local storage. This cannot be undone.`,
        "Delete Files",
        "red",
      );
      if (!confirmed) return;

      pruneBtn.disabled = true;
      pruneBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Starting prune...';
      try {
        const resp = await fetchJSON("/api/storage/prune", {
          method: "POST",
          body: JSON.stringify({ fileIds: selectedFileIds }),
        });
        const taskId = resp.data?.taskId || resp.taskId;

        pruneBtn.innerHTML =
          '<i class="fas fa-spinner fa-spin"></i> Pruning (Task running)...';

        // Poll task status every 2 seconds
        const pollInterval = setInterval(async () => {
          try {
            const taskResp = await fetchJSON(`/api/tasks/${taskId}`);
            const task = taskResp.data;
            if (
              task.status === "Completed" ||
              task.status === "Failed" ||
              task.status === "Cancelled"
            ) {
              clearInterval(pollInterval);
              pruneBtn.disabled = false;

              if (task.status === "Completed") {
                showToast(task.progress || "Prune completed", "success");
              } else if (task.status === "Failed") {
                showToast(`Prune failed: ${task.progress || "Unknown error"}`, "error");
              } else {
                showToast("Prune cancelled", "info");
              }

              // Refresh the page state
              await loadStatus(container);
              await loadPrunePreview(container);
            }
          } catch (err) {
            clearInterval(pollInterval);
            showToast(`Failed to check task status: ${err.message}`, "error");
            pruneBtn.disabled = false;
          }
        }, 2000);
      } catch (err) {
        showToast(`Prune failed: ${err.message}`, "error");
        pruneBtn.disabled = false;
      }
    }
  });
}
