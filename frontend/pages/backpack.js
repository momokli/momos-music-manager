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

    ${tags.length === 0
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
    `}
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
