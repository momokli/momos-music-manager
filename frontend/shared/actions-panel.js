/**
 * actions-panel.js — Shared actions panel for CRUD pages.
 *
 * Provides rendering and wiring for the right-side actions panel
 * that appears on CRUD pages with bulk selection support.
 *
 * Panel structure:
 *   Header: "Actions" + selection count badge
 *   Buttons: Refresh + page-specific action buttons
 *
 * Exports:
 *   renderActionsPanel(buttons)
 *   wireActionsRefresh(container, pageId, refreshFn)
 *   updateSelectionCount(container, count)
 */

import { escapeHtml } from "./components.js";

/**
 * Render the actions panel HTML.
 *
 * @param {Array<{id: string, label: string, icon: string, cls?: string, action: string}>} buttons
 * @returns {string} HTML
 */
export function renderActionsPanel(buttons = []) {
  const buttonsHtml = buttons
    .map(
      (b) =>
        `<button class="btn btn-sm ${b.cls || ""}" id="tracks-actions-${b.id}" data-action="${b.action}"><i class="${b.icon}"></i> ${escapeHtml(b.label)}</button>`,
    )
    .join("");

  return `
    <div class="filter-panel" style="flex:1;min-width:180px;max-width:240px;">
      <div class="filter-panel-header">
        <span style="font-weight:600;font-size:0.75rem;color:var(--text-muted);text-transform:uppercase;letter-spacing:0.04em;"><i class="fas fa-bolt"></i> Actions</span>
        <span class="actions-sel-count" id="tracks-sel-count" style="display:none">0</span>
      </div>
      <div class="filter-panel-body" style="padding:var(--space-3) var(--space-4);display:flex;flex-direction:column;gap:var(--space-2);">
        <button class="btn btn-sm" id="tracks-actions-refresh"><i class="fas fa-rotate"></i> Refresh</button>
        ${buttonsHtml}
      </div>
    </div>`;
}

/**
 * Wire the refresh button in the actions panel.
 *
 * @param {HTMLElement} container - The page container
 * @param {string} pageId - Page identifier
 * @param {() => Promise<void>} refreshFn - Function to call on refresh
 */
export function wireActionsRefresh(container, pageId, refreshFn) {
  const btn = container.querySelector(`#${pageId}-actions-refresh`);
  if (btn) {
    btn.onclick = () => refreshFn();
  }
}

/**
 * Update the selection count badge in the actions panel.
 *
 * @param {HTMLElement} container - The page container
 * @param {number} count - Number of selected items
 */
export function updateSelectionCount(container, count) {
  const badge = container.querySelector("#tracks-sel-count");
  if (badge) {
    if (count > 0) {
      badge.textContent = String(count);
      badge.style.display = "";
    } else {
      badge.style.display = "none";
    }
  }
}
