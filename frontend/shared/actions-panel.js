/**
 * actions-panel.js — Shared actions panel for CRUD pages.
 *
 * Renders a small action card (refresh + selection count) in the right column
 * of the filter area (4/5 + 1/5 grid). Each page uses this in init().
 *
 * Usage:
 *   import { renderActionsPanel } from "../shared/actions-panel.js";
 *   const actionsHtml = renderActionsPanel("files");
 *   // In toolbar container:
 *   container.innerHTML = `
 *     <div style="display:grid;grid-template-columns:4fr 1fr;gap:var(--space-4);">
 *       <div>${toolbarHtml}</div>
 *       ${actionsHtml}
 *     </div>
 *     <div id="files-content">...</div>
 *   `;
 */

/**
 * Render the actions panel HTML.
 * @param {string} pageId — used to create unique DOM IDs
 * @param {object} [opts]
 * @param {boolean} [opts.hideRefresh] — hide the refresh button
 * @returns {string} HTML
 */
export function renderActionsPanel(pageId, opts = {}) {
  return `
    <div class="actions-panel">
      <div class="actions-panel-header">
        <span><i class="fas fa-bolt"></i> Actions</span>
        <span class="actions-sel-count" id="${pageId}-sel-count">0</span>
      </div>
      ${opts.hideRefresh ? "" : `<button class="btn btn-sm" id="${pageId}-actions-refresh"><i class="fas fa-rotate"></i> Refresh</button>`}
    </div>`;
}

/**
 * Wire the actions panel refresh button.
 * @param {Element} container
 * @param {string} pageId
 * @param {Function} onRefresh — async callback, return a promise
 */
export function wireActionsRefresh(container, pageId, onRefresh) {
  const btn = container.querySelector(`#${pageId}-actions-refresh`);
  if (!btn) return;
  btn.addEventListener("click", () => {
    btn.disabled = true;
    btn.innerHTML = '<i class="fas fa-spinner fa-spin"></i>';
    Promise.resolve(onRefresh()).finally(() => {
      btn.disabled = false;
      btn.innerHTML = '<i class="fas fa-rotate"></i> Refresh';
    });
  });
}

/**
 * Update the selection count in the actions panel.
 * @param {Element} container
 * @param {string} pageId
 * @param {number} count
 */
export function updateActionsSelCount(container, pageId, count) {
  const el = container.querySelector(`#${pageId}-sel-count`);
  if (el) el.textContent = count > 0 ? String(count) : "0";
}
