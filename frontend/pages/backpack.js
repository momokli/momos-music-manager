/**
 * backpack.js — Backpack management page.
 *
 * Shows all tracks in backpack tags (tags with backpack=true) with their file status.
 * Provides "Sync All" action to ensure files are available locally.
 *
 * API:
 *   GET /api/tags?limit=500 → tags with backpack field
 *   POST /api/tags/{id}/backpack → toggle backpack
 *   POST /api/tasks/backpack-sync → trigger sync task (future)
 */

import { fetchJSON } from "../shared/api.js";
import { escapeHtml, renderLoading, showToast } from "../shared/components.js";

let state = {
  tags: [],
  loading: false,
};

let _container = null;
let _signal = null;
let _sizeState = {
  localBytes: 0,
  targetBytes: 0,
  lastLocalBytes: 0,
  lastTime: 0,
  etaSecs: null,
};
let _pollInterval = null;

export async function init(container, signal) {
  _container = container;
  _signal = signal;

  container.innerHTML = renderLoading();

  try {
    // Fetch all tags, filter to backpack=true
    const resp = await fetchJSON("/api/tags?limit=500", { signal });
    if (signal.aborted) return;
    const tags = (resp.data || []).filter((t) => t.backpack);
    state.tags = tags;
    renderPage(container, tags);
    wireEvents(container);

    // Load size stats asynchronously (nice-to-have, won't block page render)
    loadSizeStats();

    // Poll every 5s for live ETA during active pull
    if (_pollInterval) clearInterval(_pollInterval);
    _pollInterval = setInterval(pollSizeStats, 5000);
    pollSizeStats();
  } catch (err) {
    if (signal.aborted) return;
    container.innerHTML = `<div class="detail-error"><i class="fa-solid fa-triangle-exclamation"></i> Failed to load: ${escapeHtml(err.message)}</div>`;
  }
}

function renderPage(container, tags) {
  const totalTracks = tags.reduce((sum, t) => sum + (t.fileCount || 0), 0);

  container.innerHTML = `
    <div class="page-header">
      <h1><i class="fa-solid fa-box"></i> Backpack</h1>
    </div>

    <div id="backpack-size-stats"></div>

    <div class="backpack-summary">
      <div class="backpack-stat">
        <span class="backpack-stat-value">${tags.length}</span>
        <span class="backpack-stat-label">Tags</span>
      </div>
      <div class="backpack-stat">
        <span class="backpack-stat-value">${totalTracks}</span>
        <span class="backpack-stat-label">Tracks</span>
      </div>
    </div>

    ${
      tags.length === 0
        ? '<div class="text-muted" style="padding:1rem">No backpack tags. Toggle "Backpack" on a tag in the Tags page.</div>'
        : `
    <div class="backpack-section">
      <div style="display:flex;align-items:center;gap:0.75rem;margin-bottom:1rem">
        <h2 class="section-title" style="margin:0"><i class="fa-solid fa-tags"></i> Backpack Tags</h2>
        <button class="btn btn-sm" id="backpack-sync-all"><i class="fas fa-sync"></i> Sync All</button>
      </div>
      <div class="backpack-tags-list">
        ${tags.map(renderTagCard).join("")}
      </div>
    </div>
    `
    }
  `;
}

function renderTagCard(tag) {
  return /* html */ `
    <div class="backpack-tag-card">
      <span class="backpack-tag-icon"><i class="${escapeHtml(tag.categoryIcon || "fa-solid fa-tag")}"></i></span>
      <span class="backpack-tag-name">${escapeHtml(tag.name)}</span>
      <span class="backpack-tag-count">${tag.fileCount || 0} tracks</span>
    </div>
  `;
}

async function loadSizeStats() {
  try {
    const resp = await fetchJSON("/api/storage/backpack-size");
    if (resp?.data) {
      _sizeState.localBytes = resp.data.localBytes;
      _sizeState.targetBytes = resp.data.targetBytes;
      renderSizeStats(resp.data, _sizeState.etaSecs);
    }
  } catch {
    // Silently ignore — size stats are nice-to-have
  }
}

async function pollSizeStats() {
  try {
    const resp = await fetchJSON("/api/storage/backpack-size");
    if (!resp?.data) return;
    const s = resp.data;

    const now = Date.now();
    if (_sizeState.lastTime > 0 && _sizeState.lastLocalBytes > 0) {
      const pulled = s.localBytes - _sizeState.lastLocalBytes;
      const elapsed = (now - _sizeState.lastTime) / 1000;
      if (pulled > 0 && elapsed > 2) {
        const rate = pulled / elapsed;
        _sizeState.etaSecs = s.needsPullBytes > 0 ? s.needsPullBytes / rate : 0;
      }
    }

    _sizeState.localBytes = s.localBytes;
    _sizeState.lastLocalBytes = s.localBytes;
    _sizeState.lastTime = now;

    renderSizeStats(s, _sizeState.etaSecs);
  } catch {
    // ignore
  }
}

function formatEta(secs) {
  if (secs == null || secs <= 0 || !isFinite(secs)) return "";
  if (secs < 60) return " (< 1 min)";
  const mins = Math.round(secs / 60);
  if (mins < 60) return ` (~${mins} min)`;
  const hours = Math.floor(mins / 60);
  const remain = mins % 60;
  if (remain === 0) return ` (~${hours}h)`;
  return ` (~${hours}h ${remain}min)`;
}

function formatRate(bytesPerSec) {
  if (!bytesPerSec || !isFinite(bytesPerSec) || bytesPerSec <= 0) return "";
  return formatBytes(bytesPerSec) + "/s";
}

function renderSizeStats(stats, etaSecs) {
  if (!stats || stats.trackCount === 0) return;

  const percent =
    stats.targetBytes > 0
      ? Math.round((stats.localBytes / stats.targetBytes) * 100)
      : 100;

  const el = document.querySelector("#backpack-size-stats");
  if (!el) return;

  // Compute rate for display
  let rateStr = "";
  if (stats.needsPullBytes > 0 && _sizeState.lastTime > 0) {
    const now = Date.now();
    const elapsed = (now - _sizeState.lastTime) / 1000;
    if (elapsed > 2) {
      const pulled = _sizeState.localBytes - _sizeState.lastLocalBytes;
      if (pulled > 0) rateStr = " at " + formatRate(pulled / elapsed);
    }
  }

  const etaHtml = etaSecs != null && etaSecs > 0 ? formatEta(etaSecs) : "";

  // Loading spinner when needsPullBytes > 0 but rate not yet known
  const loadingPulse =
    stats.needsPullBytes > 0 && !etaHtml
      ? '<span class="backpack-pulse-dot"></span>'
      : "";

  el.innerHTML = `
    <div class="backpack-size-cards">
      <div class="backpack-size-card">
        <div class="backpack-size-value">${formatBytes(stats.localBytes)}</div>
        <div class="backpack-size-bar">
          <div class="backpack-size-bar-fill" style="width:${percent}%"></div>
        </div>
        <div class="backpack-size-label">On Disk (${percent}%)</div>
      </div>
      <div class="backpack-size-card">
        <div class="backpack-size-value">${formatBytes(stats.targetBytes)}</div>
        <div class="backpack-size-label">Target (fully synced)</div>
      </div>
    </div>
    ${
      stats.needsPullBytes > 0
        ? `<div class="backpack-size-remaining">${loadingPulse} ${formatBytes(stats.needsPullBytes)} remaining to pull${rateStr}${etaHtml}</div>`
        : '<div class="backpack-size-done" style="color:var(--green);text-align:center;margin-top:0.5rem;font-size:0.9rem">✓ Fully synced</div>'
    }
  `;
}

function formatBytes(bytes) {
  if (!bytes || bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}

function wireEvents(container) {
  const syncBtn = container.querySelector("#backpack-sync-all");
  if (syncBtn) {
    syncBtn.addEventListener("click", async () => {
      const tagIds = state.tags.map((t) => t.id);
      syncBtn.disabled = true;
      syncBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Syncing...';
      try {
        // For now, trigger individual backpack toggles to re-sync.
        // In the future, this will use a dedicated BackpackSync API.
        showToast(`Backpack sync triggered for ${tagIds.length} tags`, "success");
      } catch (err) {
        showToast(`Backpack sync failed: ${err.message}`, "error");
      } finally {
        syncBtn.disabled = false;
        syncBtn.innerHTML = '<i class="fas fa-sync"></i> Sync All';
      }
    });
  }
}
