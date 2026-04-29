/**
 * files.js — Browse and manage local music files with comment diff previews.
 *
 * Exports: init(container, signal)
 */

import { escapeHtml, renderLoading, renderErrorBlock, td } from "../shared/components.js";
import { formatBPM } from "../shared/format.js";
import { fetchJSON } from "../shared/api.js";
import { renderSearchInput, wireSearchFilter } from "../shared/search-filter.js";

/* ------------------------------------------------------------------ */
/*  Constants                                                          */
/* ------------------------------------------------------------------ */

const PAGE_SIZE = 50;
const BPM_MAX = 300;

/**
 * Musical keys in Camelot notation as stored in DB.
 * Minor: 1m–12m  |  Major: 1d–12d
 */
const MINOR_KEYS = [];
const MAJOR_KEYS = [];

for (let i = 1; i <= 12; i++) {
  MINOR_KEYS.push(`${i}m`);
  MAJOR_KEYS.push(`${i}d`);
}

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

/**
 * Compare two comment strings and return whether they differ.
 * Returns { diffOld, diffNew, unchanged } with the full plain text strings.
 */
function computeDiff(oldComment, targetComment) {
  const oldStr = oldComment || "";
  const targetStr = targetComment || "";

  if (oldStr === targetStr) {
    return { diffOld: oldStr, diffNew: targetStr, unchanged: true };
  }

  return { diffOld: oldStr, diffNew: targetStr, unchanged: false };
}

/* ------------------------------------------------------------------ */
/*  Toast helpers                                                      */
/* ------------------------------------------------------------------ */

function showToast(message, type) {
  const existing = document.querySelector(".toast-notification");
  if (existing) existing.remove();

  const bg =
    type === "error"
      ? "var(--red, #ef4444)"
      : type === "success"
        ? "var(--green, #22c55e)"
        : "var(--accent, #6366f1)";

  const toast = document.createElement("div");
  toast.className = "toast-notification";
  toast.textContent = message;
  Object.assign(toast.style, {
    position: "fixed",
    bottom: "24px",
    right: "24px",
    background: bg,
    color: "#fff",
    padding: "12px 20px",
    borderRadius: "8px",
    fontSize: "0.9rem",
    zIndex: "9999",
    boxShadow: "0 4px 20px rgba(0,0,0,0.3)",
    transition: "opacity 0.3s ease",
    cursor: "pointer",
  });
  toast.addEventListener("click", () => toast.remove());
  document.body.appendChild(toast);

  setTimeout(() => {
    toast.style.opacity = "0";
    setTimeout(() => toast.remove(), 300);
  }, 4000);
}

function showError(message) {
  showToast(message, "error");
}

function showSuccess(message) {
  showToast(message, "success");
}

function adaptFile(f) {
  const diff = computeDiff(f.comment, f.commentTarget);
  return {
    id: f.id,
    title: f.title,
    artist: f.artist,
    bpm: f.bpm,
    key: f.key,
    diffOld: diff.diffOld,
    diffNew: diff.diffNew,
    commentUnchanged: diff.unchanged,
    needsUpdate: f.needsUpdate,
    comment: f.comment,
    commentTarget: f.commentTarget,
    playCount: f.playCount,
    lastPlayed: f.lastPlayed,
  };
}

/* ------------------------------------------------------------------ */
/*  Render helpers                                                     */
/* ------------------------------------------------------------------ */

function renderKeyBadge(key) {
  if (!key) return "";
  return `<span class="badge badge-key">${escapeHtml(key)}</span>`;
}

function renderRows(files) {
  return files
    .map((f) => {
      const diffClass = f.commentUnchanged ? "diff-line-unchanged" : "diff-line";
      const diffRow = f.commentUnchanged
        ? `<span class="diff-sign check">✓</span>${escapeHtml(f.comment)}`
        : `<div class="diff-line-old"><span class="diff-sign minus">−</span>${escapeHtml(f.diffOld)}</div>
           <div class="diff-line-new"><span class="diff-sign plus">+</span>${escapeHtml(f.diffNew)}</div>`;
      return `<tr>
        <td>${escapeHtml(f.title)}</td>
        <td>${escapeHtml(f.artist)}</td>
        <td>${f.bpm ? formatBPM(f.bpm) : ""}</td>
        <td>${renderKeyBadge(f.key)}</td>
        <td>${f.playCount ?? 0}</td>
        <td><div class="${diffClass}">${diffRow}</div></td>
        <td>
          <button class="btn btn-sm btn-icon" data-action="view" data-id="${f.id}" title="View details"><i class="fas fa-eye"></i></button>
          <button class="btn btn-sm btn-icon" data-action="write-comment" data-id="${f.id}" title="Write comment to file" ${f.commentTarget ? "" : "disabled"}><i class="fas fa-pen"></i></button>
        </td>
      </tr>`;
    })
    .join("");
}

async function writeComment(id) {
  try {
    const resp = await fetchJSON(`/api/files/${id}/write-comment`, {
      method: "POST",
    });
    const taskId = resp.data?.taskId || resp.data;
    showSuccess(`Comment write queued (task #${taskId})`);
  } catch (err) {
    showError(`Failed to queue comment write: ${err.message}`);
  }
}

async function viewFile(id) {
  try {
    const resp = await fetchJSON(`/api/files/${id}`);
    const f = adaptFile(resp.data);
    const detailsHtml = `
      <div style="display:grid;grid-template-columns:auto 1fr;gap:8px 16px;font-size:0.9rem;">
        <strong>ID:</strong><span>${f.id}</span>
        <strong>Title:</strong><span>${escapeHtml(f.title)}</span>
        <strong>Artist:</strong><span>${escapeHtml(f.artist)}</span>
        <strong>BPM:</strong><span>${f.bpm ? formatBPM(f.bpm) : "—"}</span>
        <strong>Key:</strong><span>${renderKeyBadge(f.key)}</span>
        <strong>Plays:</strong><span>${f.playCount ?? 0}</span>
        <strong>Last played:</strong><span>${f.lastPlayed || "—"}</span>
        ${f.diffOld ? `<strong>Comment (current):</strong><span class="diff-line-old">${escapeHtml(f.diffOld)}</span>` : ""}
        ${f.diffNew ? `<strong>Comment (target):</strong><span class="diff-line-new">${escapeHtml(f.diffNew)}</span>` : ""}
        ${f.commentUnchanged ? `<strong>Comment:</strong><span>${escapeHtml(f.comment)}</span>` : ""}
      </div>`;

    const overlay = document.createElement("div");
    overlay.className = "modal open";
    overlay.style.cssText =
      "position:fixed;inset:0;background:rgba(0,0,0,0.6);z-index:999;display:flex;align-items:center;justify-content:center;";

    const modal = document.createElement("div");
    modal.className = "modal-content";
    modal.style.maxWidth = "600px";
    modal.innerHTML = `
      <div class="modal-header">
        <h3>${escapeHtml(f.title)}</h3>
        <button class="close-btn" id="modal-close">&times;</button>
      </div>
      <div style="padding:16px">${detailsHtml}</div>`;

    const doClose = () => overlay.remove();
    modal.querySelector("#modal-close").onclick = doClose;
    overlay.onclick = (e) => {
      if (e.target === overlay) doClose();
    };
    document.addEventListener(
      "keydown",
      (e) => {
        if (e.key === "Escape") doClose();
      },
      { once: true },
    );

    overlay.appendChild(modal);
    document.body.appendChild(overlay);
  } catch (err) {
    showError(`Failed to load file details: ${err.message}`);
  }
}

/* ------------------------------------------------------------------ */
/*  Render                                                             */
/* ------------------------------------------------------------------ */

function renderFilterPanel(state) {
  const chipsHtml = (state.selectedTags || [])
    .map(
      (t) =>
        `<span class="tag-chip" data-tag="${t}">${t} <i class="fas fa-times tag-chip-x"></i></span>`,
    )
    .join("");

  // Key buttons: two rows – minor (1m-12m) and major (1d-12d)
  const selectedKeys = new Set(state.keys || []);
  const keyBtn = (key, cls) =>
    `<button class="key-btn ${cls}${selectedKeys.has(key) ? " active" : ""}" data-key="${key}">${key}</button>`;

  const minorRow = MINOR_KEYS.map((k) => keyBtn(k, "minor")).join("");
  const majorRow = MAJOR_KEYS.map((k) => keyBtn(k, "major")).join("");

  // BPM slider ranges
  const bpmMin = parseFloat(state.bpmMin) || 0;
  const bpmMax = parseFloat(state.bpmMax) || BPM_MAX;
  const pctMin = (bpmMin / BPM_MAX) * 100;
  const pctMax = (bpmMax / BPM_MAX) * 100;

  const actionBtn = (label, action, cls = "") =>
    `<button class="key-btn action ${cls}" data-key-action="${action}">${label}</button>`;

  return `
    <div class="filter-panel" id="files-filter-panel">
      <div class="filter-panel-header">
        ${renderSearchInput("files", state.search)}
        <button class="filter-panel-toggle" id="files-filter-toggle" title="Toggle filter panel">
          <i class="fas fa-chevron-up chevron"></i>
        </button>
      </div>
      <div class="filter-panel-body">
        <div class="filter-panel-scroll">
          <div class="filter-row">
            <span class="filter-row-label">BPM</span>
            <div class="dual-range-wrap">
              <div class="dual-range">
                <div class="dual-range-track">
                  <div class="dual-range-fill" style="left:${pctMin}%;width:${pctMax - pctMin}%"></div>
                </div>
                <input type="range" class="dual-range-input" data-sf-filter="bpmMin"
                       min="0" max="${BPM_MAX}" step="1" value="${bpmMin}">
                <input type="range" class="dual-range-input" data-sf-filter="bpmMax"
                       min="0" max="${BPM_MAX}" step="1" value="${bpmMax}">
              </div>
              <div class="dual-range-values">
                <span class="dual-range-min-val">${bpmMin}</span>
                <span class="sep">──</span>
                <span class="dual-range-max-val">${bpmMax}</span>
              </div>
            </div>
          </div>
          <div class="filter-row">
            <span class="filter-row-label">Key</span>
            <div class="key-grid-wrap">
              <div class="key-grid" data-key-row="minor">${minorRow}
                ${actionBtn("ALL m", "minor-all")}
                ${actionBtn("NONE m", "minor-none")}
              </div>
              <div class="key-grid" data-key-row="major">${majorRow}
                ${actionBtn("ALL d", "major-all")}
                ${actionBtn("NONE d", "major-none")}
              </div>
            </div>
          </div>
        </div>
        <div class="filter-tag-area">
          <div class="tag-search-wrap">
            <i class="fas fa-tag"></i>
            <input type="text" class="input-text input-search" id="files-tag-search"
                   placeholder="filter by TAG" autocomplete="off">
            <div class="tag-dropdown" id="files-tag-dropdown"></div>
          </div>
          <div class="tag-chips" id="files-tag-chips">${chipsHtml}</div>
        </div>
      </div>
    </div>`;
}

function render(container, data, state) {
  const totalPages = Math.ceil(data._total / PAGE_SIZE) || 1;
  const currentPage = state.page + 1;

  container.innerHTML = `
    ${renderFilterPanel(state)}
    <div class="stats-row">
      <div class="stats-group">
        <button class="btn btn-sm btn-icon" id="files-refresh" title="Refresh"><i class="fa-solid fa-rotate"></i></button>
        <strong>${data._total}</strong> files
      </div>
      <div class="stats-actions">
        <button class="btn btn-yellow btn-sm" id="files-write-comments" title="Write all comment diffs to file metadata">
          <i class="fas fa-pen"></i> Write Comment Diffs
        </button>
      </div>
    </div>
    <div class="table-wrap"><table class="data-table">
      <thead><tr><th style="width:22%">Title</th><th style="width:7%">Artist</th><th style="width:18%">BPM</th><th style="width:3%">Key</th><th style="width:3%">Plays</th><th style="width:35%">Comment Diff</th><th style="width:12%">Actions</th></tr></thead>
      <tbody>${renderRows(data.files)}</tbody>
    </table></div>
    <div class="pagination">
      <button class="pagination-btn" id="files-page-prev" ${state.page === 0 ? "disabled" : ""}><i class="fas fa-chevron-left"></i></button>
      <span class="pagination-info">Page ${currentPage} of ${totalPages}</span>
      <button class="pagination-btn" id="files-page-next" ${state.page >= totalPages - 1 ? "disabled" : ""}><i class="fas fa-chevron-right"></i></button>
    </div>`;
}

/* ------------------------------------------------------------------ */
/*  Fetch + Render cycle                                               */
/* ------------------------------------------------------------------ */

function buildParams(state) {
  const params = new URLSearchParams();
  params.set("limit", String(PAGE_SIZE));
  params.set("offset", String(state.page * PAGE_SIZE));
  if (state.search) params.set("search", state.search);
  if (state.bpmMin > 0) params.set("bpmMin", state.bpmMin);
  if (state.bpmMax < BPM_MAX) params.set("bpmMax", state.bpmMax);
  if (state.keys && state.keys.length > 0) {
    params.set("key", state.keys.join(","));
  }
  if (state.selectedTags && state.selectedTags.length > 0) {
    params.set("tags", state.selectedTags.join(","));
  }
  return params;
}

async function fetchAndRender(container, signal, state) {
  container.innerHTML = renderLoading("Loading files…");

  try {
    const params = buildParams(state);
    const countParams = new URLSearchParams(params);
    // Remove pagination params for count request — limit/offset shouldn't affect count
    countParams.delete("limit");
    countParams.delete("offset");

    const [filesResp, countResp] = await Promise.all([
      fetchJSON(`/api/files?${params}`, { signal }),
      fetchJSON(`/api/files/count?${countParams}`, { signal }),
    ]);
    if (signal.aborted) return;

    const data = {
      _total: countResp.data,
      files: filesResp.data.map(adaptFile),
    };

    if (data.files.length === 0 && data._total === 0) {
      container.innerHTML = `
        ${renderFilterPanel(state)}
        <div class="stats-row">
          <div class="stats-group">
            <button class="btn btn-sm btn-icon" id="files-refresh" title="Refresh"><i class="fa-solid fa-rotate"></i></button>
            <strong>0</strong> files
          </div>
          <div class="stats-actions">
            <button class="btn btn-yellow btn-sm" id="files-write-comments" title="Write all comment diffs to file metadata">
              <i class="fas fa-pen"></i> Write Comment Diffs
            </button>
          </div>
        </div>
        <div class="table-wrap"><table class="data-table">
          <thead><tr><th style="width:22%">Title</th><th style="width:7%">Artist</th><th style="width:18%">BPM</th><th style="width:3%">Key</th><th style="width:3%">Plays</th><th style="width:35%">Comment Diff</th><th style="width:12%">Actions</th></tr></thead>
          <tbody><tr><td colspan="7"><div class="empty-state" style="border:none;padding:32px"><div class="empty-icon"><i class="fas fa-music"></i></div><h3>No files found</h3><p>Scan a folder to start building your music library.</p></div></td></tr></tbody>
        </table></div>`;
      wireEvents(container, signal, state);
      return;
    }

    render(container, data, state);

    // Wire up events after render
    wireEvents(container, signal, state);
  } catch (err) {
    if (err.name === "AbortError") return;
    container.innerHTML = renderErrorBlock({
      title: "Failed to load files",
      detail: err.message,
      retryFn: "window.location.hash='#files'",
    });
  }
}

/* ------------------------------------------------------------------ */
/*  Event wiring                                                       */
/* ------------------------------------------------------------------ */

function wireEvents(container, signal, state) {
  // Unified search + filter wiring (debounced) — search input + dual range slider
  const filterPanel = container.querySelector(".filter-panel");
  if (filterPanel) {
    wireSearchFilter(filterPanel, state, () => fetchAndRender(container, signal, state));
  }

  // Refresh button
  const refreshBtn = container.querySelector("#files-refresh");
  if (refreshBtn) {
    refreshBtn.onclick = () => fetchAndRender(container, signal, state);
  }

  // ── Filter panel toggle (collapsible) ──
  const panelToggle = container.querySelector("#files-filter-toggle");
  const panel = container.querySelector("#files-filter-panel");
  if (panelToggle && panel) {
    panelToggle.addEventListener("click", () => {
      panel.classList.toggle("collapsed");
      const icon = panelToggle.querySelector(".chevron");
      if (icon) {
        icon.classList.toggle("fa-chevron-up");
        icon.classList.toggle("fa-chevron-down");
      }
    });
  }

  // ── Dual range slider visual updates (fill bar + value labels) ──
  const dualRange = container.querySelector(".dual-range");
  if (dualRange) {
    const minInput = dualRange.querySelector('[data-sf-filter="bpmMin"]');
    const maxInput = dualRange.querySelector('[data-sf-filter="bpmMax"]');
    const fill = dualRange.querySelector(".dual-range-fill");
    const minVal = container.querySelector(".dual-range-min-val");
    const maxVal = container.querySelector(".dual-range-max-val");

    function updateDualRange() {
      let min = parseFloat(minInput.value) || 0;
      let max = parseFloat(maxInput.value) || BPM_MAX;
      // Swap if handles cross so min <= max
      if (min > max) {
        [min, max] = [max, min];
        minInput.value = min;
        maxInput.value = max;
      }
      const pctMin = (min / BPM_MAX) * 100;
      const pctMax = (max / BPM_MAX) * 100;
      if (fill) {
        fill.style.left = `${pctMin}%`;
        fill.style.width = `${pctMax - pctMin}%`;
      }
      if (minVal) minVal.textContent = min;
      if (maxVal) maxVal.textContent = max;
    }

    minInput.addEventListener("input", updateDualRange);
    maxInput.addEventListener("input", updateDualRange);
  }

  // ── Key buttons (toggle multiple) + ALL/NONE actions ──
  function setKeys(newKeys) {
    state.keys = newKeys;
    state.page = 0;
    fetchAndRender(container, signal, state);
  }

  const keyGridWrap = container.querySelector(".key-grid-wrap");
  if (keyGridWrap) {
    keyGridWrap.addEventListener("click", (e) => {
      const btn = e.target.closest(".key-btn");
      if (!btn) return;

      // Check if it's an action button (ALL/NONE)
      const action = btn.dataset.keyAction;
      if (action) {
        switch (action) {
          case "minor-all":
            setKeys([...state.keys.filter((k) => !k.endsWith("m")), ...MINOR_KEYS]);
            break;
          case "minor-none":
            setKeys(state.keys.filter((k) => !k.endsWith("m")));
            break;
          case "major-all":
            setKeys([...state.keys.filter((k) => !k.endsWith("d")), ...MAJOR_KEYS]);
            break;
          case "major-none":
            setKeys(state.keys.filter((k) => !k.endsWith("d")));
            break;
        }
        return;
      }

      // Regular key toggle
      const dbVal = btn.dataset.key;
      if (!dbVal) return;
      const idx = state.keys.indexOf(dbVal);
      if (idx >= 0) {
        state.keys.splice(idx, 1);
      } else {
        state.keys.push(dbVal);
      }
      state.page = 0;
      fetchAndRender(container, signal, state);
    });
  }

  // ── Tag search input with keyboard navigation ──
  const tagSearch = container.querySelector("#files-tag-search");
  const tagDropdown = container.querySelector("#files-tag-dropdown");
  if (tagSearch && tagDropdown) {
    let timer;
    let selectedIndex = -1;

    // Helper to update which item is highlighted
    function updateSelection() {
      const items = tagDropdown.querySelectorAll(".tag-dropdown-item");
      items.forEach((item, i) => {
        item.classList.toggle("selected", i === selectedIndex);
      });
      const selected = items[selectedIndex];
      if (selected) {
        selected.scrollIntoView({ block: "nearest" });
      }
    }

    // Helper to add the selected tag as a filter chip
    function addSelectedTag() {
      const items = tagDropdown.querySelectorAll(".tag-dropdown-item");
      const selected = items[selectedIndex];
      if (!selected) return;
      const tag = selected.dataset.tag;
      if (!tag) return;
      if (!state.selectedTags.includes(tag)) {
        state.selectedTags.push(tag);
        state.page = 0;
      }
      tagSearch.value = "";
      tagDropdown.classList.remove("open");
      tagDropdown.innerHTML = "";
      selectedIndex = -1;
      fetchAndRender(container, signal, state);
    }

    tagSearch.addEventListener("input", () => {
      clearTimeout(timer);
      selectedIndex = -1;
      const q = tagSearch.value.trim();
      if (!q) {
        tagDropdown.classList.remove("open");
        tagDropdown.innerHTML = "";
        return;
      }
      timer = setTimeout(async () => {
        try {
          const resp = await fetchJSON(`/api/tags?search=${encodeURIComponent(q)}`);
          const tags = resp.data || [];
          if (tags.length === 0) {
            tagDropdown.innerHTML = `<div class="tag-dropdown-empty">No tags found</div>`;
            selectedIndex = -1;
          } else {
            tagDropdown.innerHTML = tags
              .map(
                (t, i) =>
                  `<div class="tag-dropdown-item${i === 0 ? " selected" : ""}" data-tag="${t.name}">
                    <span class="tag-dropdown-name">${t.name}</span>
                    ${t.category ? `<span class="tag-dropdown-cat">${t.category}</span>` : ""}
                  </div>`,
              )
              .join("");
            selectedIndex = 0; // First item auto-selected
          }
          tagDropdown.classList.add("open");
        } catch {
          // ignore errors during search
        }
      }, 150);
    });

    // Click on dropdown item → add tag chip
    tagDropdown.addEventListener("click", (e) => {
      const item = e.target.closest(".tag-dropdown-item");
      if (!item) return;
      const tag = item.dataset.tag;
      if (!tag) return;
      if (!state.selectedTags.includes(tag)) {
        state.selectedTags.push(tag);
        state.page = 0;
      }
      tagSearch.value = "";
      tagDropdown.classList.remove("open");
      tagDropdown.innerHTML = "";
      selectedIndex = -1;
      fetchAndRender(container, signal, state);
    });

    // Keyboard navigation
    tagSearch.addEventListener("keydown", (e) => {
      if (!tagDropdown.classList.contains("open")) return;

      const items = tagDropdown.querySelectorAll(".tag-dropdown-item");

      switch (e.key) {
        case "ArrowDown":
          e.preventDefault();
          if (items.length === 0) return;
          selectedIndex = Math.min(selectedIndex + 1, items.length - 1);
          updateSelection();
          break;

        case "ArrowUp":
          e.preventDefault();
          if (items.length === 0) return;
          selectedIndex = Math.max(selectedIndex - 1, 0);
          updateSelection();
          break;

        case "Enter":
          e.preventDefault();
          addSelectedTag();
          break;

        case "Escape":
          tagDropdown.classList.remove("open");
          tagDropdown.innerHTML = "";
          selectedIndex = -1;
          tagSearch.blur();
          break;
      }
    });
  }

  // ── Tag chip removal (delegated) ──
  const chipsContainer = container.querySelector("#files-tag-chips");
  if (chipsContainer) {
    chipsContainer.addEventListener("click", (e) => {
      const x = e.target.closest(".tag-chip-x");
      if (!x) return;
      const chip = x.closest(".tag-chip");
      if (!chip) return;
      const tag = chip.dataset.tag;
      state.selectedTags = state.selectedTags.filter((t) => t !== tag);
      state.page = 0;
      fetchAndRender(container, signal, state);
    });
  }

  // ── Close tag dropdown on outside click ──
  document.addEventListener(
    "click",
    (e) => {
      const wrap = container.querySelector(".tag-search-wrap");
      if (!wrap || wrap.contains(e.target)) return;
      if (tagDropdown) {
        tagDropdown.classList.remove("open");
        tagDropdown.innerHTML = "";
        selectedIndex = -1;
      }
    },
    { signal },
  );

  // Write Comment Diffs button — queues a task to write all pending comment diffs
  const writeCommentsBtn = container.querySelector("#files-write-comments");
  if (writeCommentsBtn) {
    writeCommentsBtn.onclick = async () => {
      writeCommentsBtn.disabled = true;
      const originalHtml = writeCommentsBtn.innerHTML;
      writeCommentsBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Queuing...';
      try {
        const resp = await fetchJSON("/api/files/bulk-sync", {
          method: "POST",
          body: JSON.stringify({}),
        });
        const taskId = resp.data?.taskId || resp.data;
        if (taskId) {
          showSuccess(
            `Comment write task #${taskId} started. Check Tasks page for progress.`,
          );
        } else {
          showSuccess("All comments are up to date — nothing to write.");
        }
        writeCommentsBtn.disabled = false;
        writeCommentsBtn.innerHTML = originalHtml;
      } catch (err) {
        showError(`Failed to queue comment writes: ${err.message}`);
        writeCommentsBtn.disabled = false;
        writeCommentsBtn.innerHTML = originalHtml;
      }
    };
  }

  // Pagination: wire up both top and bottom prev/next sets
  const prevBtn = container.querySelector("#files-page-prev");
  if (prevBtn) {
    prevBtn.onclick = () => {
      if (state.page > 0) {
        state.page--;
        fetchAndRender(container, signal, state);
      }
    };
  }

  const nextBtn = container.querySelector("#files-page-next");
  if (nextBtn) {
    nextBtn.onclick = () => {
      state.page++;
      fetchAndRender(container, signal, state);
    };
  }

  // Action buttons via event delegation
  container.addEventListener(
    "click",
    (e) => {
      const btn = e.target.closest("button[data-action]");
      if (!btn) return;
      const action = btn.dataset.action;
      const id = parseInt(btn.dataset.id, 10);
      if (action === "write-comment") {
        writeComment(id);
      } else if (action === "view") {
        viewFile(id);
      }
    },
    { signal },
  );
}

/* ------------------------------------------------------------------ */
/*  Initialisation                                                     */
/* ------------------------------------------------------------------ */

export async function init(container, signal) {
  // State for pagination and filters — mutable, lives across renders
  const state = {
    page: 0,
    search: "",
    bpmMin: 0,
    bpmMax: BPM_MAX,
    keys: [],
    selectedTags: [],
  };

  await fetchAndRender(container, signal, state);
}
