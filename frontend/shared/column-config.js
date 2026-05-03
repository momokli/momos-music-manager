/**
 * column-config.js — Traktor-like column customization system.
 *
 * Provides per-page column visibility, reorder, and resize management
 * with localStorage persistence.
 *
 * Usage in a page module:
 *
 *   const COLUMNS = [
 *     { id: "title", label: "Title", sortable: true, sortKey: "title", defaultWidth: 22 },
 *     { id: "artist", label: "Artist", sortable: true, sortKey: "artist", defaultWidth: 16 },
 *   ];
 *
 *   const config = loadColumnConfig("files", COLUMNS);
 *
 *   // In renderBody:
 *   const headerHtml = renderColumnHeaders(config, COLUMNS, state, sortableTh);
 *   const cellsHtml = renderColumnCells(config, COLUMNS, cellRenderers, row);
 *
 *   // After renderBody:
 *   wireColumnResize(container, "files", COLUMNS, config);
 *   wireColumnDragReorder(container, "files", COLUMNS, config, () => fetchAndRender(...));
 *   wireConfigTrigger(container, "files", COLUMNS, config, () => fetchAndRender(...));
 */

import { escapeHtml, showModal } from "./components.js";

/* ------------------------------------------------------------------ */
/*  Config CRUD                                                        */
/* ------------------------------------------------------------------ */

/**
 * Load column config for a page from localStorage.
 * Falls back to defaults from the column model if nothing stored.
 *
 * @param {string} pageId — localStorage key suffix (e.g. "files", "tracks")
 * @param {Array<object>} columns — column model array (from page module)
 * @returns {Array<{id:string, visible:boolean, width:number}>}
 */
export function loadColumnConfig(pageId, columns) {
  const key = `columnConfig_${pageId}`;
  const saved = localStorage.getItem(key);
  if (saved) {
    try {
      const parsed = JSON.parse(saved);
      // Validate: ensure all model columns exist in config, add missing ones
      const existingIds = new Set(parsed.map((c) => c.id));
      const merged = columns.map((col) => {
        const existing = parsed.find((p) => p.id === col.id);
        return existing
          ? { id: col.id, visible: existing.visible, width: existing.width }
          : { id: col.id, visible: true, width: col.defaultWidth };
      });
      return merged;
    } catch {
      // corrupted data, fall back to defaults
    }
  }
  return columns.map((col) => ({
    id: col.id,
    visible: true,
    width: col.defaultWidth,
  }));
}

/**
 * Save column config to localStorage.
 * @param {string} pageId
 * @param {Array} config
 */
export function saveColumnConfig(pageId, config) {
  localStorage.setItem(`columnConfig_${pageId}`, JSON.stringify(config));
}

/**
 * Reset config to defaults.
 * @param {string} pageId
 * @param {Array<object>} columns — column model
 * @returns {Array} fresh default config
 */
export function resetColumnConfig(pageId, columns) {
  const defaults = columns.map((col) => ({
    id: col.id,
    visible: true,
    width: col.defaultWidth,
  }));
  saveColumnConfig(pageId, defaults);
  return defaults;
}

/* ------------------------------------------------------------------ */
/*  Render helpers                                                      */
/* ------------------------------------------------------------------ */

/**
 * Render a "Columns" trigger button for the stats row.
 * @returns {string} HTML
 */
export function renderColumnConfigTrigger() {
  return `<button class="col-config-trigger" data-col-config="true" title="Configure columns">
    <i class="fas fa-table-cells"></i> Columns
  </button>`;
}

/**
 * Render column header HTML from config + column model.
 * Only visible columns are rendered, in config order.
 *
 * @param {Array} config — current column config
 * @param {Array<object>} columns — column model (from page module)
 * @param {object} state — CRUD state { sort, order }
 * @param {Function} sortableTh — sortableTh function from crud.js
 * @returns {string} concatenated `<th>` HTML
 */
export function renderColumnHeaders(config, columns, state, sortableTh) {
  return config
    .filter((c) => c.visible)
    .map((c) => {
      const model = columns.find((m) => m.id === c.id);
      if (!model) return "";
      if (model.sortable) {
        const th = sortableTh(model.label, model.sortKey, state, {
          style: `width:${c.width}%`,
        });
        // Inject resize handle before closing </th>
        return th.replace("</th>", `${addResizeHandle()}</th>`);
      }
      return `<th style="width:${c.width}%">${escapeHtml(model.label)}${addResizeHandle()}</th>`;
    })
    .join("");
}

/**
 * Render table cells for a single row based on config + column model.
 *
 * @param {Array} config — current column config
 * @param {Array<object>} columns — column model
 * @param {object} cellRenderers — { columnId: (rowData) => HTML string }
 * @param {object} row — the row data object
 * @returns {string} concatenated `<td>` HTML
 */
export function renderColumnCells(config, columns, cellRenderers, row) {
  return config
    .filter((c) => c.visible)
    .map((c) => {
      const model = columns.find((m) => m.id === c.id);
      if (!model) return "";
      const renderer = cellRenderers[c.id];
      if (!renderer) return "<td></td>";
      const content = renderer(row);
      return `<td style="width:${c.width}%">${content}</td>`;
    })
    .join("");
}

function addResizeHandle() {
  return `<div class="col-resize-handle"></div>`;
}

/* ------------------------------------------------------------------ */
/*  Resize handles                                                      */
/* ------------------------------------------------------------------ */

/**
 * Wire column resize handles on `<th>` elements.
 * Drag the handle on the right edge of each `<th>` to change width.
 *
 * @param {HTMLElement} container — element containing the table
 * @param {string} pageId — for localStorage key
 * @param {Array<object>} columns — column model
 * @param {Array} config — mutable config array (mutated + saved)
 */
export function wireColumnResize(container, pageId, columns, config) {
  const handles = container.querySelectorAll(".col-resize-handle");
  let activeHandle = null;
  let startX = 0;
  let startWidth = 0;
  let thEl = null;

  handles.forEach((handle) => {
    handle.addEventListener("mousedown", (e) => {
      e.preventDefault();
      activeHandle = handle;
      thEl = handle.parentElement;
      startX = e.clientX;
      startWidth = thEl.offsetWidth;
      handle.classList.add("resizing");
      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";
    });
  });

  document.addEventListener("mousemove", (e) => {
    if (!activeHandle || !thEl) return;
    const diff = e.clientX - startX;
    let newWidth = startWidth + diff;
    newWidth = Math.max(30, Math.min(500, newWidth));

    // Convert to percentage of parent table
    const table = thEl.closest(".data-table");
    if (!table) return;
    const tableWidth = table.offsetWidth;
    if (tableWidth === 0) return;
    const pct = Math.round((newWidth / tableWidth) * 100);

    // Find the config entry and update
    const colId = findColumnIdFromTh(thEl, columns);
    if (colId) {
      const entry = config.find((c) => c.id === colId);
      if (entry) {
        entry.width = Math.max(1, Math.min(100, pct));
        // Update style directly for smooth resize
        thEl.style.width = `${pct}%`;
        // Also update all cells in this column
        const colIndex = Array.from(thEl.parentElement.children).indexOf(thEl);
        table.querySelectorAll("tbody tr").forEach((tr) => {
          const cell = tr.children[colIndex];
          if (cell) cell.style.width = `${pct}%`;
        });
      }
    }
  });

  document.addEventListener("mouseup", () => {
    if (activeHandle) {
      activeHandle.classList.remove("resizing");
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      saveColumnConfig(pageId, config);
      activeHandle = null;
      thEl = null;
    }
  });
}

/**
 * Given a `<th>` element, find the column ID from the column model.
 */
function findColumnIdFromTh(thEl, columns) {
  // Try data-sort attribute (sortable columns)
  const sortKey = thEl.dataset.sort;
  if (sortKey) {
    const model = columns.find((m) => m.sortKey === sortKey);
    if (model) return model.id;
  }
  // Fall back to matching by visible text content
  const text = thEl.textContent.replace("▲", "").replace("▼", "").trim();
  const model = columns.find((m) => m.label.toLowerCase() === text.toLowerCase());
  if (model) return model.id;
  return null;
}

/* ------------------------------------------------------------------ */
/*  Drag-to-reorder (header drag)                                       */
/* ------------------------------------------------------------------ */

/**
 * Wire drag-to-reorder on table headers.
 * Users can drag a `<th>` to a new position to reorder columns.
 *
 * @param {HTMLElement} container
 * @param {string} pageId
 * @param {Array<object>} columns — column model
 * @param {Array} config — mutable config array
 * @param {Function} onSave — called after reorder + save
 */
export function wireColumnDragReorder(container, pageId, columns, config, onSave) {
  const table = container.querySelector(".data-table");
  if (!table) return;

  const thead = table.querySelector("thead");
  if (!thead) return;

  let dragTh = null;
  let dragConfigIndex = -1;

  thead.addEventListener("dragstart", (e) => {
    const th = e.target.closest("th.sortable, th:not(.sortable)");
    if (!th || e.target.closest(".col-resize-handle")) {
      e.preventDefault();
      return;
    }
    dragTh = th;
    dragConfigIndex = findConfigIndexFromTh(th, config, columns);
    th.classList.add("dragging");
    e.dataTransfer.effectAllowed = "move";
    // Need to set data for drag to work
    e.dataTransfer.setData("text/plain", "col");
  });

  thead.addEventListener("dragend", () => {
    if (dragTh) dragTh.classList.remove("dragging");
    thead.querySelectorAll("th").forEach((th) => th.classList.remove("drop-target"));
    dragTh = null;
    dragConfigIndex = -1;
  });

  thead.addEventListener("dragover", (e) => {
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
    const th = e.target.closest("th");
    if (!th || th === dragTh) return;
    // Position indicator
    const rect = th.getBoundingClientRect();
    const midX = rect.left + rect.width / 2;
    const isBefore = e.clientX < midX;
    th.classList.add("drop-target");
    // Use a pseudo-element border approach or just visual feedback
  });

  thead.addEventListener("dragleave", (e) => {
    const th = e.target.closest("th");
    if (th) th.classList.remove("drop-target");
  });

  thead.addEventListener("drop", (e) => {
    e.preventDefault();
    const targetTh = e.target.closest("th");
    if (!targetTh || dragConfigIndex === -1) return;

    const targetIndex = findConfigIndexFromTh(targetTh, config, columns);
    if (targetIndex === -1 || targetIndex === dragConfigIndex) return;

    // Move the config entry
    const [moved] = config.splice(dragConfigIndex, 1);
    config.splice(targetIndex, 0, moved);

    saveColumnConfig(pageId, config);
    thead.querySelectorAll("th").forEach((th) => th.classList.remove("drop-target"));

    if (onSave) onSave();
  });
}

function findConfigIndexFromTh(thEl, config, columns) {
  const sortKey = thEl.dataset.sort;
  if (sortKey) {
    const model = columns.find((m) => m.sortKey === sortKey);
    if (model) return config.findIndex((c) => c.id === model.id);
  }
  const text = thEl.textContent.replace("▲", "").replace("▼", "").trim();
  const model = columns.find((m) => m.label.toLowerCase() === text.toLowerCase());
  if (model) return config.findIndex((c) => c.id === model.id);
  return -1;
}

/* ------------------------------------------------------------------ */
/*  Config modal                                                        */
/* ------------------------------------------------------------------ */

/**
 * Wire the "Columns" config trigger button.
 * Opens a modal showing all columns with visibility toggles,
 * drag-to-reorder, and width inputs.
 *
 * @param {HTMLElement} container
 * @param {string} pageId
 * @param {Array<object>} columns — column model
 * @param {Array} config — mutable config array
 * @param {Function} onSave — called after changes are applied
 */
export function wireConfigTrigger(container, pageId, columns, config, onSave) {
  const trigger = container.querySelector("[data-col-config]");
  if (!trigger) return;

  trigger.addEventListener("click", () => {
    openConfigModal(pageId, columns, config, onSave);
  });
}

/**
 * Open the column configuration modal.
 */
function openConfigModal(pageId, columns, config, onSave) {
  // Work on a copy so changes can be cancelled by closing
  const workingCopy = config.map((c) => ({ ...c }));

  const itemsHtml = workingCopy
    .map((c, i) => {
      const model = columns.find((m) => m.id === c.id);
      const label = model ? model.label : c.id;
      return `<div class="col-config-item" data-index="${i}" draggable="true">
          <span class="col-config-drag-handle"><i class="fas fa-grip-lines"></i></span>
          <input type="checkbox" class="col-config-checkbox" data-cid="${c.id}" ${c.visible ? "checked" : ""}>
          <span class="col-config-label">${escapeHtml(label)}</span>
          <input type="number" class="col-config-width" data-cid="${c.id}" value="${c.width}" min="1" max="100">
        </div>`;
    })
    .join("");

  const bodyHtml = `
    <div style="padding:var(--space-4);max-height:400px;overflow-y:auto">
      ${itemsHtml}
    </div>
    <div class="modal-actions" style="padding:0 var(--space-4) var(--space-4);display:flex;gap:8px;flex-wrap:wrap">
      <button class="btn btn-sm col-config-reset" data-modal-action="reset">Reset to Defaults</button>
      <button class="btn btn-sm" data-modal-action="select-all">Select All</button>
      <button class="btn btn-sm" data-modal-action="deselect-all">Deselect All</button>
      <button class="btn btn-primary btn-sm" data-modal-action="apply" style="margin-left:auto">Apply</button>
    </div>`;

  showModal({
    title: `<i class="fas fa-table-cells"></i> Configure Columns — ${pageId}`,
    width: "500px",
    bodyHtml,
    onAction: (action, close) => {
      if (action === "close") {
        close();
        return;
      }

      // Read current state from DOM
      const modalContainer = document.getElementById("shared-modal");
      if (!modalContainer) return;

      if (action === "reset") {
        const defaults = resetColumnConfig(pageId, columns);
        workingCopy.length = 0;
        workingCopy.push(...defaults.map((d) => ({ ...d })));
        close();
        if (onSave) onSave();
        return;
      }

      if (action === "select-all" || action === "deselect-all") {
        const checked = action === "select-all";
        modalContainer.querySelectorAll(".col-config-checkbox").forEach((cb) => {
          cb.checked = checked;
        });
        return;
      }

      if (action === "apply") {
        // Read visibility and widths from DOM
        modalContainer.querySelectorAll(".col-config-checkbox").forEach((cb) => {
          const entry = workingCopy.find((c) => c.id === cb.dataset.cid);
          if (entry) entry.visible = cb.checked;
        });
        modalContainer.querySelectorAll(".col-config-width").forEach((input) => {
          const entry = workingCopy.find((c) => c.id === input.dataset.cid);
          if (entry)
            entry.width = Math.max(
              1,
              Math.min(100, parseInt(input.value, 10) || entry.width),
            );
        });

        // Read order from DOM (items may have been reordered)
        const items = modalContainer.querySelectorAll(".col-config-item");
        const reordered = [];
        items.forEach((item) => {
          const idx = parseInt(item.dataset.index, 10);
          reordered.push(workingCopy[idx]);
        });

        // Update original config array
        config.length = 0;
        config.push(...reordered);
        saveColumnConfig(pageId, config);
        close();
        if (onSave) onSave();
      }
    },
  });

  // Wire drag-to-reorder within the modal
  wireModalDragReorder();
}

/**
 * Wire HTML5 drag-and-drop within the config modal.
 */
function wireModalDragReorder() {
  const modal = document.getElementById("shared-modal");
  if (!modal) return;

  const items = modal.querySelectorAll(".col-config-item");
  let dragItem = null;

  items.forEach((item) => {
    item.addEventListener("dragstart", (e) => {
      dragItem = item;
      item.classList.add("dragging");
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", item.dataset.index);
    });

    item.addEventListener("dragend", () => {
      item.classList.remove("dragging");
      dragItem = null;
    });

    item.addEventListener("dragover", (e) => {
      e.preventDefault();
      e.dataTransfer.dropEffect = "move";
    });

    item.addEventListener("drop", (e) => {
      e.preventDefault();
      if (!dragItem || dragItem === item) return;
      const parent = item.parentElement;
      const itemsArr = Array.from(parent.querySelectorAll(".col-config-item"));
      const fromIdx = itemsArr.indexOf(dragItem);
      const toIdx = itemsArr.indexOf(item);
      if (fromIdx === -1 || toIdx === -1) return;

      // Reorder in the DOM
      if (fromIdx < toIdx) {
        parent.insertBefore(dragItem, item.nextSibling);
      } else {
        parent.insertBefore(dragItem, item);
      }

      // Update data-index attributes
      parent.querySelectorAll(".col-config-item").forEach((el, i) => {
        el.dataset.index = i;
      });
    });
  });
}
