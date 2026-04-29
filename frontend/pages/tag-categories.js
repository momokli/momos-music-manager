/**
 * tag-categories.js — Tag Categories management page.
 *
 * Row-based layout with drag-to-reorder (visual drop indicator),
 * icon picker, and Phase energy columns with ▲/▼ controls.
 *
 * GET  /api/tag-categories     → { data: TagCategory[] }
 * POST /api/tag-categories     → { data: TagCategory }
 * PUT  /api/tag-categories/:id → { data: TagCategory }
 * DEL  /api/tag-categories/:id → { data: null }
 *
 * GET  /api/tag-energy-levels        → { data: TagWithEnergy[] }
 * PUT  /api/tag-energy-levels/:tagId → { data: "..." }
 */

import {
  renderLoading,
  renderEmpty,
  renderErrorBlock,
  showToast,
  showModal,
  escapeHtml,
} from "../shared/components.js";
import { fetchJSON } from "../shared/api.js";

/* ================================================================== */
/*  Constants                                                         */
/* ================================================================== */

const PHASE_CATEGORY_NAME = "Phase";
const ENERGY_LEVELS = [0, 1, 2, 3, 4, 5];

/** Curated Font Awesome icons relevant to music/DJ categories. */
const CURATED_ICONS = [
  "fa-solid fa-music",
  "fa-solid fa-headphones",
  "fa-solid fa-microphone",
  "fa-solid fa-radio",
  "fa-solid fa-compact-disc",
  "fa-solid fa-cassette",
  "fa-solid fa-record-vinyl",
  "fa-solid fa-drum",
  "fa-solid fa-guitar",
  "fa-solid fa-piano-keyboard",
  "fa-solid fa-waveform",
  "fa-solid fa-sliders",
  "fa-solid fa-heart",
  "fa-solid fa-star",
  "fa-solid fa-fire",
  "fa-solid fa-sun",
  "fa-solid fa-moon",
  "fa-solid fa-cloud",
  "fa-solid fa-bolt",
  "fa-solid fa-snowflake",
  "fa-solid fa-face-smile",
  "fa-solid fa-ghost",
  "fa-solid fa-feather",
  "fa-solid fa-droplet",
  "fa-solid fa-layer-group",
  "fa-solid fa-list",
  "fa-solid fa-table",
  "fa-solid fa-grid-2",
  "fa-solid fa-timeline",
  "fa-solid fa-signal",
  "fa-solid fa-chart-line",
  "fa-solid fa-sitemap",
  "fa-solid fa-diagram-project",
  "fa-solid fa-plus",
  "fa-solid fa-pencil",
  "fa-solid fa-trash-can",
  "fa-solid fa-gear",
  "fa-solid fa-flag",
  "fa-solid fa-bookmark",
  "fa-solid fa-filter",
  "fa-solid fa-magnifying-glass",
  "fa-solid fa-tag",
  "fa-solid fa-hashtag",
  "fa-solid fa-circle",
  "fa-solid fa-square",
  "fa-solid fa-gem",
  "fa-solid fa-crown",
  "fa-solid fa-key",
  "fa-solid fa-lock",
  "fa-solid fa-bell",
  "fa-solid fa-clock",
  "fa-solid fa-calendar",
  "fa-solid fa-folder",
  "fa-solid fa-note-sticky",
  "fa-solid fa-turntable",
  "fa-solid fa-volume-high",
  "fa-solid fa-wave-square",
  "fa-solid fa-bars-progress",
  "fa-solid fa-gauge-high",
  "fa-solid fa-bolt-lightning",
  "fa-solid fa-warehouse",
  "fa-solid fa-water",
  "fa-solid fa-wind",
  "fa-solid fa-smog",
  "fa-solid fa-mountain",
  "fa-solid fa-spa",
  "fa-solid fa-wand-magic-sparkles",
  "fa-solid fa-rainbow",
  "fa-solid fa-sunrise",
  "fa-solid fa-temperature-high",
  "fa-solid fa-tornado",
];

/** Colors for tag energy badges. */
const ENERGY_COLORS = [
  "var(--text-subtle)", // 0 – grey
  "var(--green)", // 1 – green
  "var(--accent)", // 2 – indigo
  "var(--yellow)", // 3 – amber
  "var(--orange)", // 4 – orange
  "var(--red)", // 5 – red
];

/* ================================================================== */
/*  State                                                              */
/* ================================================================== */

/** @type {Array<import("../shared/api.js").TagCategory>} */
let categories = [];

/** @type {Array<{tagId:number, tagName:string, energyLevel:number, sortOrder:number}>} */
let phaseTags = [];

/** @type {number|null} */
let dragSourceId = null;

/** @type {number} index (among non-dragging rows) where indicator is shown */
let dropTargetIndex = -1;

/** @type {HTMLDivElement|null} */
let dropIndicatorEl = null;

let loadAbortController = null;

/* ================================================================== */
/*  Adapter                                                            */
/* ================================================================== */

function adaptCategory(c) {
  return {
    id: c.id,
    name: c.name,
    prefix: c.prefix || "",
    icon: c.icon,
    isDefault: !!c.isDefault,
    sortOrder: c.sortOrder ?? 0,
    tagCount: c.tagCount ?? 0,
  };
}

/* ================================================================== */
/*  Drop indicator helpers                                             */
/* ================================================================== */

function getDropIndicator() {
  if (!dropIndicatorEl) {
    dropIndicatorEl = document.createElement("div");
    dropIndicatorEl.className = "drop-indicator";
  }
  return dropIndicatorEl;
}

function positionDropIndicator(container, index) {
  const ind = getDropIndicator();
  if (ind.parentNode) ind.parentNode.removeChild(ind);
  const rows = container.querySelectorAll(".cat-row:not(.dragging)");
  if (index >= rows.length) {
    container.appendChild(ind);
  } else if (index <= 0) {
    container.insertBefore(ind, rows[0]);
  } else {
    container.insertBefore(ind, rows[index]);
  }
}

function hideDropIndicator() {
  if (dropIndicatorEl && dropIndicatorEl.parentNode) {
    dropIndicatorEl.parentNode.removeChild(dropIndicatorEl);
  }
  dropTargetIndex = -1;
}

/**
 * Find the closest gap (insertion index) for a Y coordinate.
 * Index refers to position among non-dragging rows.
 */
function findDropIndex(container, clientY) {
  const rows = Array.from(container.querySelectorAll(".cat-row:not(.dragging)"));
  const containerRect = container.getBoundingClientRect();
  const gaps = [];

  if (rows.length === 0) {
    return 0;
  }

  // Before first row
  const firstRect = rows[0].getBoundingClientRect();
  gaps.push({ index: 0, y: (containerRect.top + firstRect.top) / 2 });

  // Between rows
  for (let i = 0; i < rows.length - 1; i++) {
    const bot = rows[i].getBoundingClientRect().bottom;
    const top = rows[i + 1].getBoundingClientRect().top;
    gaps.push({ index: i + 1, y: (bot + top) / 2 });
  }

  // After last row
  const lastRect = rows[rows.length - 1].getBoundingClientRect();
  gaps.push({ index: rows.length, y: (lastRect.bottom + containerRect.bottom) / 2 });

  let minDist = Infinity;
  let closest = 0;
  for (const g of gaps) {
    const d = Math.abs(g.y - clientY);
    if (d < minDist) {
      minDist = d;
      closest = g.index;
    }
  }
  return closest;
}

/* ================================================================== */
/*  Render helpers                                                     */
/* ================================================================== */

/**
 * Render the Phase energy columns (0–5 left to right).
 */
function renderEnergyColumns() {
  // Group phaseTags by energy level
  const grouped = {};
  for (const lv of ENERGY_LEVELS) grouped[lv] = [];
  for (const pt of phaseTags) {
    const lv = Math.min(5, Math.max(0, pt.energyLevel ?? 0));
    grouped[lv].push(pt);
  }

  const columnsHtml = ENERGY_LEVELS.map((level) => {
    const tags = grouped[level] || [];
    const color = ENERGY_COLORS[level] || ENERGY_COLORS[0];
    const tagsHtml = tags
      .map(
        (pt) => `
      <div class="energy-tag" draggable="true" data-tag-id="${pt.tagId}" data-energy="${pt.energyLevel}" data-sort="${pt.sortOrder}">
        <button class="energy-btn up" ${pt.energyLevel >= 5 ? "disabled" : ""} title="Increase energy">▲</button>
        <span class="energy-tag-name energy-tag-handle">${escapeHtml(pt.tagName)}</span>
        <button class="energy-btn down" ${pt.energyLevel <= 0 ? "disabled" : ""} title="Decrease energy">▼</button>
      </div>`,
      )
      .join("");

    return `<div class="energy-column" data-level="${level}">
      <div class="energy-column-header" style="color:${color}">⚡${level}</div>
      <div class="energy-column-body">${tagsHtml || ""}</div>
    </div>`;
  }).join("");

  return `<div class="phase-energy-section">
    <div class="phase-energy-columns">${columnsHtml}</div>
  </div>`;
}

/**
 * Render a single category row.
 */
function renderCategoryRow(c) {
  const isPhase = c.name === PHASE_CATEGORY_NAME;
  const phaseClass = isPhase ? " phase-category" : "";
  const hashColor = stringToHashColor(c.name);
  const deleteDisabled = c.isDefault ? "disabled" : "";

  const defaultStar = `<span class="cat-default-star" data-id="${c.id}" title="${c.isDefault ? "Default category" : "Set as default"}">
    <i class="${c.isDefault ? "fa-solid fa-star" : "fa-regular fa-star"}" style="color:${c.isDefault ? "var(--yellow)" : "var(--text-subtle)"};font-size:0.8rem;"></i>
  </span>`;

  const prefixHtml = `<span class="cat-info-wrap">
    <span class="prefix-badge${c.prefix ? "" : " prefix-empty"}">${c.prefix ? escapeHtml(c.prefix) : "&mdash;"}</span>
    <button class="cat-edit-btn" data-edit="prefix" data-id="${c.id}" title="${c.prefix ? "Edit prefix" : "Add prefix"}"><i class="fa-solid fa-pen"></i></button>
    <input type="text" class="cat-inline-input" data-field="prefix" value="${escapeAttr(c.prefix)}" maxlength="1" hidden>
  </span>`;

  const deleteTitle = "Delete category";

  const mainHtml = `
    <div class="cat-row-main">
      <div class="cat-row-drag-handle" title="Drag to reorder">
        <i class="fa-solid fa-grip-vertical"></i>
      </div>
      <span class="cat-row-icon" data-id="${c.id}" data-icon="${escapeAttr(c.icon)}" style="--cat-color:${hashColor}" title="Click to change icon">
        <i class="${c.icon}"></i>
      </span>
      ${prefixHtml}
      <span class="cat-info-wrap">
        <span class="cat-row-name">${escapeHtml(c.name)}</span>
        <button class="cat-edit-btn" data-edit="name" data-id="${c.id}" title="Edit name"><i class="fa-solid fa-pen"></i></button>
        <input type="text" class="cat-inline-input" data-field="name" value="${escapeAttr(c.name)}" hidden>
      </span>
      <span class="cat-row-meta-group">
        ${defaultStar}
        <span class="cat-row-meta">
          <span><strong>${c.tagCount}</strong> tag${c.tagCount !== 1 ? "s" : ""}</span>
        </span>
      </span>
      <div class="cat-row-actions">
        <button class="btn btn-sm btn-red" data-action="delete" data-id="${c.id}" ${deleteDisabled} title="${deleteTitle}">
          <i class="fa-solid fa-trash-can"></i>
        </button>
      </div>
    </div>
  `;

  const energyHtml = isPhase ? renderEnergyColumns() : "";

  return `<div class="cat-row${phaseClass}" draggable="true" data-id="${c.id}" data-sort="${c.sortOrder}">
    ${mainHtml}
    ${energyHtml}
  </div>`;
}

/**
 * Simple hash of a string to a CSS hue value for icon background.
 */
function stringToHashColor(str) {
  let hash = 0;
  for (let i = 0; i < str.length; i++) {
    hash = (hash * 31 + str.charCodeAt(i)) & 0xffff;
  }
  const hue = hash % 360;
  return `hsl(${hue}, 60%, 50%)`;
}

/* ================================================================== */
/*  Main render                                                        */
/* ================================================================== */

function render(container) {
  const totalTags = categories.reduce((s, c) => s + c.tagCount, 0);
  const phaseEnergySummary = phaseTags.length
    ? `<span class="text-muted" style="margin:0 6px">·</span><span><strong>${phaseTags.length}</strong> phase tags with energy</span>`
    : "";

  const rowsHtml = categories.map(renderCategoryRow).join("");

  const newFormHtml = `
    <div class="cat-row cat-row-new" id="new-cat-form">
      <div class="cat-row-main">
        <span class="cat-row-icon new-icon-trigger" id="new-cat-icon" data-icon="fa-solid fa-tag" title="Click to choose icon">
          <i class="fa-solid fa-tag"></i>
        </span>
        <input type="text" class="input-text new-name-input" id="new-cat-name" placeholder="Category name">
        <input type="text" class="input-text new-prefix-input" id="new-cat-prefix" placeholder="Prefix" maxlength="1">
        <button class="btn btn-sm btn-primary" id="new-cat-add">Add</button>
      </div>
    </div>
  `;

  container.innerHTML = `
    <div class="toolbar" style="justify-content:space-between">
      <div style="font-size:0.85rem;color:var(--text-muted)">
        <strong style="color:var(--text-primary)">${categories.length}</strong> categories
        <span class="text-muted" style="margin:0 6px">·</span>
        <strong style="color:var(--text-primary)">${totalTags}</strong> total tags
        ${phaseEnergySummary}
      </div>
    </div>

    ${
      categories.length === 0
        ? renderEmpty({
            icon: "tags",
            title: "No categories yet",
            message: "Create your first category to get started.",
          })
        : `<div class="cat-rows">${rowsHtml}</div>`
    }

    ${newFormHtml}
  `;

  wireDragAndDrop(container);
  wireEnergyButtons(container);
  wirePhaseTagDrag(container);
}

/* ================================================================== */
/*  Drag-and-Drop Reorder                                              */
/* ================================================================== */

let _dragFromHandle = false;

function wireDragAndDrop(container) {
  const rowsContainer = container.querySelector(".cat-rows");
  if (!rowsContainer) return;

  const rows = rowsContainer.querySelectorAll(".cat-row[draggable]");

  rows.forEach((row) => {
    // Only allow drag from the handle
    row.addEventListener("mousedown", (e) => {
      _dragFromHandle = !!e.target.closest(".cat-row-drag-handle");
    });

    row.addEventListener("dragstart", onDragStart);
    row.addEventListener("dragend", onDragEnd);
    row.addEventListener("dragover", onDragOver);
    row.addEventListener("dragenter", onDragEnter);
    row.addEventListener("dragleave", onDragLeave);
    row.addEventListener("drop", onDrop);
  });
}

function onDragStart(e) {
  // Don't interfere with phase tag tile dragging
  if (e.target.closest(".energy-tag")) return;

  if (!_dragFromHandle) {
    e.preventDefault();
    return;
  }
  const row = e.target.closest(".cat-row");
  if (!row) return;
  dragSourceId = parseInt(row.dataset.id, 10);
  row.classList.add("dragging");
  e.dataTransfer.effectAllowed = "move";
  e.dataTransfer.setData("text/plain", String(dragSourceId));
}

function onDragEnd(e) {
  const row = e.target.closest(".cat-row");
  if (row) row.classList.remove("dragging");
  hideDropIndicator();
  dragSourceId = null;
}

function onDragOver(e) {
  // Don't interfere with phase tag tile dragging
  if (e.target.closest(".energy-tag")) return;

  e.preventDefault();
  e.dataTransfer.dropEffect = "move";

  const container = e.currentTarget.closest(".cat-rows");
  if (!container) return;

  const idx = findDropIndex(container, e.clientY);
  dropTargetIndex = idx;
  positionDropIndicator(container, idx);
}

function onDragEnter(e) {
  // Don't interfere with phase tag tile dragging
  if (e.target.closest(".energy-tag")) return;
  e.preventDefault();
}

function onDragLeave(e) {
  // Don't interfere with phase tag tile dragging
  if (e.target.closest(".energy-tag")) return;

  // Only hide if actually leaving the container
  const container = e.currentTarget.closest(".cat-rows");
  if (!container) return;
  const related = e.relatedTarget;
  if (!related || !container.contains(related)) {
    hideDropIndicator();
  }
}

async function onDrop(e) {
  e.preventDefault();

  const container = e.currentTarget.closest(".cat-rows");
  if (!container) return;

  // Don't interfere with phase tag tile dragging
  if (e.target.closest(".energy-tag")) {
    hideDropIndicator();
    return;
  }

  if (dropTargetIndex < 0 || dragSourceId === null) {
    hideDropIndicator();
    return;
  }

  const rows = Array.from(container.querySelectorAll(".cat-row:not(.dragging)"));
  const draggedRow = container.querySelector(".cat-row.dragging");
  if (!draggedRow) {
    hideDropIndicator();
    return;
  }

  // Move the dragged row to the target position
  if (dropTargetIndex >= rows.length) {
    container.appendChild(draggedRow);
  } else if (dropTargetIndex <= 0) {
    container.insertBefore(draggedRow, rows[0]);
  } else {
    container.insertBefore(draggedRow, rows[dropTargetIndex]);
  }

  hideDropIndicator();
  draggedRow.classList.remove("dragging");

  // Re-wire events on the container (rows may have changed)
  wireDragAndDrop(container.closest("#main-content") || document);

  await persistSortOrder(container);
  dragSourceId = null;
}

/**
 * Read the current DOM order, assign sequential sort_order,
 * and batch-update via API.
 */
async function persistSortOrder(container) {
  const rows = container.querySelectorAll(".cat-row");
  const updates = Array.from(rows).map((row, index) => ({
    id: parseInt(row.dataset.id, 10),
    sortOrder: index,
  }));

  for (const u of updates) {
    const cat = categories.find((c) => c.id === u.id);
    if (cat) cat.sortOrder = u.sortOrder;
  }

  let failed = false;
  for (const u of updates) {
    try {
      await fetchJSON(`/api/tag-categories/${u.id}`, {
        method: "PUT",
        body: JSON.stringify({ sortOrder: u.sortOrder }),
      });
    } catch (err) {
      console.error(`Failed to update sort order for category ${u.id}:`, err);
      failed = true;
    }
  }

  if (failed) {
    showToast("Some sort order updates failed. Refreshing…", "error");
    loadCategories();
  } else {
    showToast("Categories reordered", "success");
  }
}

/* ================================================================== */
/*  Phase Energy Buttons                                              */
/* ================================================================== */

function wireEnergyButtons(container) {
  container.addEventListener("click", async (e) => {
    const btn = e.target.closest(".energy-btn");
    if (!btn) return;

    const tagDiv = btn.closest(".energy-tag");
    if (!tagDiv) return;

    const tagId = parseInt(tagDiv.dataset.tagId, 10);
    const currentEnergy = parseInt(tagDiv.dataset.energy, 10);
    const isUp = btn.classList.contains("up");
    const newEnergy = isUp
      ? Math.min(5, currentEnergy + 1)
      : Math.max(0, currentEnergy - 1);

    if (newEnergy === currentEnergy) return;

    try {
      await fetchJSON(`/api/tag-energy-levels/${tagId}`, {
        method: "PUT",
        body: JSON.stringify({ energyLevel: newEnergy }),
      });
      showToast(`Energy level changed to ⚡${newEnergy}`, "success");
      loadCategories();
    } catch (err) {
      showToast(`Failed to update energy: ${err.message}`, "error");
    }
  });
}

/* ================================================================== */
/*  Phase Tag Drag-and-Drop                                            */
/* ================================================================== */

let _phaseTagDragSrc = null;

function wirePhaseTagDrag(container) {
  const cols = container.querySelectorAll(".energy-column-body");
  const tags = container.querySelectorAll(".energy-tag");

  tags.forEach((el) => {
    el.addEventListener("dragstart", onPhaseTagDragStart);
    el.addEventListener("dragend", onPhaseTagDragEnd);
  });

  cols.forEach((col) => {
    col.addEventListener("dragover", onPhaseTagDragOver);
    col.addEventListener("dragleave", onPhaseTagDragLeave);
    col.addEventListener("drop", onPhaseTagDrop);
  });
}

function onPhaseTagDragStart(e) {
  const tag = e.target.closest(".energy-tag");
  if (!tag) return;
  _phaseTagDragSrc = tag;
  tag.classList.add("dragging");
  e.dataTransfer.effectAllowed = "move";
  e.dataTransfer.setData("text/plain", tag.dataset.tagId);
}

function onPhaseTagDragEnd(e) {
  const tag = e.target.closest(".energy-tag");
  if (tag) tag.classList.remove("dragging");
  // Remove all drop highlights
  document.querySelectorAll(".energy-column-body.drag-over").forEach((el) => {
    el.classList.remove("drag-over");
  });
  _phaseTagDragSrc = null;
}

function onPhaseTagDragOver(e) {
  e.preventDefault();
  e.dataTransfer.dropEffect = "move";
  const col = e.currentTarget.closest(".energy-column-body");
  if (col) col.classList.add("drag-over");
}

function onPhaseTagDragLeave(e) {
  const col = e.currentTarget.closest(".energy-column-body");
  if (!col) return;
  const related = e.relatedTarget;
  if (!related || !col.contains(related)) {
    col.classList.remove("drag-over");
  }
}

async function onPhaseTagDrop(e) {
  e.preventDefault();
  const targetCol = e.currentTarget.closest(".energy-column-body");
  if (!targetCol || !_phaseTagDragSrc) return;

  targetCol.classList.remove("drag-over");

  const tagId = parseInt(_phaseTagDragSrc.dataset.tagId, 10);
  const targetLevel = parseInt(targetCol.closest(".energy-column").dataset.level, 10);

  // Remove from old position
  const srcTag = _phaseTagDragSrc;
  srcTag.remove();

  // Gather existing tags in target column to find position
  const existingTags = Array.from(targetCol.querySelectorAll(".energy-tag"));
  const insertBefore = findPhaseDropIndex(targetCol, e.clientY);

  if (insertBefore < 0 || insertBefore >= existingTags.length) {
    targetCol.appendChild(srcTag);
  } else {
    targetCol.insertBefore(srcTag, existingTags[insertBefore]);
  }

  // Update data attributes
  srcTag.dataset.energy = targetLevel;
  srcTag.querySelector(".energy-btn.up").disabled = targetLevel >= 5;
  srcTag.querySelector(".energy-btn.down").disabled = targetLevel <= 0;

  _phaseTagDragSrc = null;

  // Persist
  await persistPhaseTagOrder();
}

/**
 * Find insertion index within a column body based on mouse Y.
 */
function findPhaseDropIndex(columnBody, clientY) {
  const tags = columnBody.querySelectorAll(".energy-tag");
  if (!tags.length) return 0;
  const bodyRect = columnBody.getBoundingClientRect();
  const midY = bodyRect.top + bodyRect.height / 2;
  if (clientY < midY) return 0;
  // Find closest gap
  let closestIdx = tags.length;
  let minDist = Infinity;
  tags.forEach((tag, i) => {
    const rect = tag.getBoundingClientRect();
    const gapY = rect.top + rect.height / 2;
    const d = Math.abs(clientY - gapY);
    if (d < minDist) {
      minDist = d;
      closestIdx = clientY < gapY ? i : i + 1;
    }
  });
  return closestIdx;
}

/**
 * Read the current DOM order of all energy tags and batch-persist via API.
 */
async function persistPhaseTagOrder() {
  const allCols = document.querySelectorAll(".energy-column");
  const updates = [];

  for (const col of allCols) {
    const level = parseInt(col.dataset.level, 10);
    const tags = col.querySelectorAll(".energy-tag");
    tags.forEach((tag, idx) => {
      const tid = parseInt(tag.dataset.tagId, 10);
      updates.push({ tagId: tid, energyLevel: level, sortOrder: idx });
      // Update local data
      const pt = phaseTags.find((p) => p.tagId === tid);
      if (pt) {
        pt.energyLevel = level;
        pt.sortOrder = idx;
      }
    });
  }

  try {
    await fetchJSON("/api/tag-energy-levels/batch", {
      method: "PUT",
      body: JSON.stringify({ tags: updates }),
    });
    showToast("Phase tags reordered", "success");
  } catch (err) {
    showToast(`Failed to save order: ${err.message}`, "error");
    // Reload to get back to a known state
    loadCategories();
  }
}

/* ================================================================== */
/*  Inline Editing (Icon, Name, Prefix)                                */
/* ================================================================== */

/**
 * Make a reusable icon-picker popover.
 * @param {HTMLElement} anchor - element to position popover under
 * @param {string} currentIcon - currently selected icon class
 * @param {(icon: string) => void} onSelect - called when user picks an icon
 */
function showIconPopover(anchor, currentIcon, onSelect) {
  // Remove any existing popover on this anchor
  const old = anchor.querySelector(".icon-picker-popover");
  if (old) {
    old.remove();
    return;
  }

  // Close others
  document.querySelectorAll(".icon-picker-popover").forEach((el) => el.remove());

  const popover = document.createElement("div");
  popover.className = "icon-picker-popover";
  popover.classList.add("open");
  popover.style.cssText =
    "position:absolute;top:100%;left:0;z-index:300;min-width:320px;margin-top:4px;right:auto;";

  const searchInput = document.createElement("input");
  searchInput.type = "text";
  searchInput.className = "input-text icon-picker-search";
  searchInput.placeholder = "Search icons…";

  const grid = document.createElement("div");
  grid.className = "icon-picker-grid";

  let filteredIcons = CURATED_ICONS;
  function renderGrid() {
    grid.innerHTML = filteredIcons
      .map(
        (ic) =>
          `<button type="button" class="icon-picker-btn${ic === currentIcon ? " selected" : ""}" data-icon="${ic}"><i class="${ic}"></i></button>`,
      )
      .join("");
  }
  renderGrid();

  popover.appendChild(searchInput);
  popover.appendChild(grid);

  anchor.style.position = "relative";
  anchor.appendChild(popover);
  searchInput.focus();
  searchInput.select();

  searchInput.addEventListener("input", () => {
    const q = searchInput.value.toLowerCase().trim();
    filteredIcons = q
      ? CURATED_ICONS.filter((ic) => ic.toLowerCase().includes(q))
      : CURATED_ICONS;
    renderGrid();
  });

  grid.addEventListener("click", (ev) => {
    const btn = ev.target.closest(".icon-picker-btn");
    if (!btn) return;
    onSelect(btn.dataset.icon);
    popover.remove();
  });

  // Close on click outside
  const closeHandler = (ev2) => {
    if (!anchor.contains(ev2.target)) {
      popover.remove();
      document.removeEventListener("click", closeHandler);
    }
  };
  setTimeout(() => document.addEventListener("click", closeHandler), 0);
}

/**
 * Wire up the inline icon picker on category row icons.
 */
function wireInlineIconPicker(container) {
  container.querySelectorAll(".cat-row-icon:not(.new-icon-trigger)").forEach((el) => {
    // Skip the new-category form icon (no data-id)
    if (!el.dataset.id) return;
    el.addEventListener("click", (e) => {
      if (e.target.closest(".icon-picker-popover")) return;
      const catId = parseInt(el.dataset.id, 10);
      const currentIcon = el.dataset.icon;
      showIconPopover(el, currentIcon, async (newIcon) => {
        try {
          await fetchJSON(`/api/tag-categories/${catId}`, {
            method: "PUT",
            body: JSON.stringify({ icon: newIcon }),
          });
          showToast("Icon updated", "success");
          loadCategories();
        } catch (err) {
          showToast(`Failed to update icon: ${err.message}`, "error");
        }
      });
    });
  });
}

/**
 * Wire up the icon picker for the new-category inline form.
 */
function wireNewFormIconPicker() {
  const iconEl = document.getElementById("new-cat-icon");
  if (!iconEl) return;
  iconEl.addEventListener("click", (e) => {
    if (e.target.closest(".icon-picker-popover")) return;
    showIconPopover(iconEl, iconEl.dataset.icon, (newIcon) => {
      iconEl.dataset.icon = newIcon;
      iconEl.innerHTML = `<i class="${newIcon}"></i>`;
    });
  });
}

/**
 * Reset the inline new-category form to default state.
 */
function resetNewCategoryForm() {
  document.getElementById("new-cat-name").value = "";
  document.getElementById("new-cat-prefix").value = "";
  const iconEl = document.getElementById("new-cat-icon");
  if (iconEl) {
    iconEl.dataset.icon = "fa-solid fa-tag";
    iconEl.innerHTML = '<i class="fa-solid fa-tag"></i>';
  }
}

/**
 * Wire the inline new-category form.
 */
function wireNewCategoryForm(container) {
  document.getElementById("new-cat-add")?.addEventListener("click", async () => {
    const name = document.getElementById("new-cat-name")?.value.trim();
    if (!name) {
      showToast("Name is required", "error");
      return;
    }
    const prefix = document.getElementById("new-cat-prefix")?.value.trim() || "";
    const icon =
      document.getElementById("new-cat-icon")?.dataset.icon || "fa-solid fa-tag";
    try {
      await fetchJSON("/api/tag-categories", {
        method: "POST",
        body: JSON.stringify({ name, prefix, icon, sortOrder: categories.length }),
      });
      showToast(`Category "${name}" created`, "success");
      resetNewCategoryForm();
      loadCategories();
    } catch (err) {
      showToast(`Failed to create category: ${err.message}`, "error");
    }
  });
}

/**
 * Wire default-star toggles.
 */
function wireDefaultToggle(container) {
  container.addEventListener("click", async (e) => {
    const star = e.target.closest(".cat-default-star");
    if (!star) return;
    const catId = parseInt(star.dataset.id, 10);
    const cat = categories.find((c) => c.id === catId);
    if (!cat) return;
    const newDefault = !cat.isDefault;
    try {
      // If setting a new default, unset the old one first
      if (newDefault) {
        const oldDefault = categories.find((c) => c.isDefault && c.id !== catId);
        if (oldDefault) {
          await fetchJSON(`/api/tag-categories/${oldDefault.id}`, {
            method: "PUT",
            body: JSON.stringify({ isDefault: false }),
          });
        }
      }
      await fetchJSON(`/api/tag-categories/${catId}`, {
        method: "PUT",
        body: JSON.stringify({ isDefault: newDefault }),
      });
      showToast(
        newDefault ? "Default category changed" : "Default category removed",
        "success",
      );
      loadCategories();
    } catch (err) {
      showToast(`Failed to update default: ${err.message}`, "error");
    }
  });
}

/**
 * Wire up inline name / prefix editing.
 */
function wireInlineEdits(container) {
  // Pencil click → show input
  container.addEventListener("click", (e) => {
    const btn = e.target.closest(".cat-edit-btn");
    if (!btn) return;
    e.stopPropagation();

    const wrap = btn.closest(".cat-info-wrap");
    if (!wrap) return;

    const textSpan = wrap.querySelector("span.cat-row-name, span.prefix-badge");
    const input = wrap.querySelector(".cat-inline-input");
    if (!textSpan || !input) return;

    textSpan.hidden = true;
    btn.hidden = true;
    input.hidden = false;
    input.focus();
    input.select();
  });

  // Commit on Enter, cancel on Escape
  container.addEventListener("keydown", async (e) => {
    const input = e.target.closest(".cat-inline-input");
    if (!input || input.hidden) return;

    if (e.key === "Enter") {
      e.preventDefault();
      await commitInlineEdit(input);
    } else if (e.key === "Escape") {
      e.preventDefault();
      cancelInlineEdit(input);
    }
  });

  // Commit on blur
  container.addEventListener(
    "blur",
    async (e) => {
      const input = e.target.closest(".cat-inline-input");
      if (!input || input.hidden) return;
      await commitInlineEdit(input);
    },
    true,
  );
}

/**
 * Save the inline-edited value via API.
 */
async function commitInlineEdit(input) {
  const wrap = input.closest(".cat-info-wrap");
  if (!wrap) return;

  const row = input.closest(".cat-row");
  if (!row) return;
  const catId = parseInt(row.dataset.id, 10);

  const textSpan = wrap.querySelector("span.cat-row-name, span.prefix-badge");
  const btn = wrap.querySelector(".cat-edit-btn");
  const field = input.dataset.field; // "name" or "prefix"
  const value = input.value.trim();
  const oldValue = textSpan.textContent.trim();

  if (!value || value === oldValue) {
    cancelInlineEdit(input);
    return;
  }

  const body = {};
  body[field] = value;

  try {
    await fetchJSON(`/api/tag-categories/${catId}`, {
      method: "PUT",
      body: JSON.stringify(body),
    });
    showToast(`${field === "name" ? "Name" : "Prefix"} updated`, "success");
    loadCategories();
  } catch (err) {
    showToast(`Failed to update ${field}: ${err.message}`, "error");
    cancelInlineEdit(input);
  }
}

/**
 * Cancel inline edit and restore text display.
 */
function cancelInlineEdit(input) {
  const wrap = input.closest(".cat-info-wrap");
  if (!wrap) return;

  const textSpan = wrap.querySelector("span.cat-row-name, span.prefix-badge");
  const btn = wrap.querySelector(".cat-edit-btn");

  input.hidden = true;
  if (textSpan) textSpan.hidden = false;
  if (btn) btn.hidden = false;
}

/* ================================================================== */
/*  Icon Picker (modal version)                                        */
/* ================================================================== */

function escapeAttr(s) {
  if (typeof s !== "string") return "";
  return s
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

function deleteCategory(catId, catName) {
  const cleanup = showModal({
    title: "Delete Category",
    width: "480px",
    bodyHtml: `
      <div style="padding:0;">
        <p style="color:var(--text-muted);margin-bottom:var(--space-4);">
          Are you sure you want to delete the category <strong>"${escapeAttr(catName)}"</strong>?
        </p>
        <p style="font-size:0.85rem;color:var(--red);display:flex;align-items:center;gap:var(--space-1);">
          <i class="fa-solid fa-exclamation-triangle"></i>
          Tags in this category will lose their category assignment.
        </p>
      </div>
      <div class="modal-actions">
        <button id="cat-delete-cancel" class="btn">Cancel</button>
        <button id="cat-delete-confirm" class="btn btn-red">Delete Category</button>
      </div>
    `,
  });

  const modal = document.getElementById("shared-modal");
  modal?.addEventListener("click", async (e) => {
    if (e.target.id === "cat-delete-cancel") {
      cleanup();
    } else if (e.target.id === "cat-delete-confirm") {
      try {
        await fetchJSON(`/api/tag-categories/${catId}`, { method: "DELETE" });
        showToast(`Category "${catName}" deleted`, "success");
        cleanup();
        loadCategories();
      } catch (err) {
        showToast(`Failed to delete category: ${err.message}`, "error");
      }
    }
  });
}

/* ================================================================== */
/*  Data Loading                                                       */
/* ================================================================== */

async function loadCategories() {
  if (loadAbortController) loadAbortController.abort();
  loadAbortController = new AbortController();
  const signal = loadAbortController.signal;

  const container = document.getElementById("main-content");
  if (!container) return;

  container.innerHTML = renderLoading("Loading tag categories…");

  try {
    const [catResp, energyResp] = await Promise.all([
      fetchJSON("/api/tag-categories", { signal }),
      fetchJSON("/api/tag-energy-levels", { signal }),
    ]);
    if (signal.aborted) return;

    categories = catResp.data.map(adaptCategory);

    phaseTags = [];
    if (energyResp.data && Array.isArray(energyResp.data)) {
      for (const e of energyResp.data) {
        phaseTags.push({
          tagId: e.tagId,
          tagName: e.tagName,
          energyLevel: e.energyLevel ?? 0,
          sortOrder: e.sortOrder ?? 0,
        });
      }
    }

    render(container);
    wireEvents(container);
  } catch (err) {
    if (err.name === "AbortError") return;
    container.innerHTML = renderErrorBlock({
      title: "Failed to load tag categories",
      detail: err.message,
      retryFn: "window.location.hash='#tag-categories'",
    });
  }
}

/* ================================================================== */
/*  Event Wiring                                                       */
/* ================================================================== */

function wireEvents(container) {
  // Delete button – delegated
  container.addEventListener("click", (e) => {
    const btn = e.target.closest("[data-action='delete']");
    if (!btn) return;
    e.preventDefault();
    const id = Number(btn.dataset.id);
    const cat = categories.find((c) => c.id === id);
    deleteCategory(id, cat ? cat.name : "");
  });

  // Inline editing: icon picker, name, prefix
  wireInlineIconPicker(container);
  wireNewFormIconPicker();
  wireInlineEdits(container);
  wireNewCategoryForm(container);
  wireDefaultToggle(container);
}

/* ================================================================== */
/*  Initialisation                                                     */
/* ================================================================== */

export async function init(container, signal) {
  container.innerHTML = renderLoading("Loading tag categories…");

  try {
    const [catResp, energyResp] = await Promise.all([
      fetchJSON("/api/tag-categories", { signal }),
      fetchJSON("/api/tag-energy-levels", { signal }),
    ]);
    if (signal.aborted) return;

    categories = catResp.data.map(adaptCategory);

    phaseTags = [];
    if (energyResp.data && Array.isArray(energyResp.data)) {
      for (const e of energyResp.data) {
        phaseTags.push({
          tagId: e.tagId,
          tagName: e.tagName,
          energyLevel: e.energyLevel ?? 0,
          sortOrder: e.sortOrder ?? 0,
        });
      }
    }

    render(container);
    wireEvents(container);
  } catch (err) {
    if (err.name === "AbortError") return;
    container.innerHTML = renderErrorBlock({
      title: "Failed to load tag categories",
      detail: err.message,
      retryFn: "window.location.hash='#tag-categories'",
    });
  }
}
