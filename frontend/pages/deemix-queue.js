/**
 * deemix-queue.js — Deemix download queue page.
 */

import { fetchJSON } from "../shared/api.js";
import {
  renderLoading,
  renderErrorBlock,
  showToast,
  escapeHtml,
} from "../shared/components.js";
import {
  loadColumnConfig,
  renderColumnConfigTrigger,
  renderColumnHeaders,
  renderColumnCells,
  wireColumnResize,
  wireColumnDragReorder,
  wireConfigTrigger,
  reorderTableColumns,
} from "../shared/column-config.js";
import { renderSearchInput, wireSearchFilter } from "../shared/search-filter.js";
import {
  getPageSize,
  renderPageSizeSelector,
  sortableTh,
  wireSortableHeaders,
  wirePageSizeSelector,
  updateHash,
} from "../shared/crud.js";

const STATUS_OPTIONS = [
  { value: "all", label: "All Statuses" },
  { value: "queued", label: "Queued" },
  { value: "downloading", label: "Downloading" },
  { value: "completed", label: "Completed" },
  { value: "failed", label: "Failed" },
];

const DEEMIX_COLUMNS = [
  { id: "status", label: "Status", sortable: true, sortKey: "status", defaultWidth: 8 },
  { id: "title", label: "Title", sortable: true, sortKey: "title", defaultWidth: 16 },
  { id: "artist", label: "Artist", sortable: true, sortKey: "artist", defaultWidth: 12 },
  {
    id: "playlistName",
    label: "Playlist Name",
    sortable: true,
    sortKey: "playlist_name",
    defaultWidth: 14,
  },
  { id: "url", label: "URL", sortable: false, defaultWidth: 6 },
  {
    id: "progress",
    label: "Progress",
    sortable: true,
    sortKey: "progress",
    defaultWidth: 8,
  },
  {
    id: "total",
    label: "Total",
    sortable: true,
    sortKey: "track_count_total",
    defaultWidth: 6,
  },
  {
    id: "downloaded",
    label: "Downloaded",
    sortable: true,
    sortKey: "track_count_downloaded",
    defaultWidth: 8,
  },
  { id: "detail", label: "Detail", sortable: false, defaultWidth: 10 },
  {
    id: "created",
    label: "Created",
    sortable: true,
    sortKey: "created_at",
    defaultWidth: 8,
  },
  {
    id: "updated",
    label: "Updated",
    sortable: true,
    sortKey: "updated_at",
    defaultWidth: 8,
  },
  { id: "actions", label: "Actions", sortable: false, defaultWidth: 10 },
];

const DEEMIX_CELL_RENDERERS = {
  status: (item) => statusBadge(item.status),
  title: (item) =>
    item.title ? escapeHtml(item.title) : '<span class="text-muted">—</span>',
  artist: (item) =>
    item.artist ? escapeHtml(item.artist) : '<span class="text-muted">—</span>',
  playlistName: (item) =>
    item.playlistName
      ? escapeHtml(item.playlistName)
      : '<span class="text-muted">—</span>',
  url: (item) =>
    item.spotifyPlaylistUrl
      ? `<a href="${escapeHtml(item.spotifyPlaylistUrl)}" target="_blank" rel="noopener" class="btn btn-sm btn-icon" title="${escapeHtml(item.spotifyPlaylistUrl)}"><i class="fa-solid fa-external-link-alt"></i></a>`
      : '<span class="text-muted">—</span>',
  progress: (item) => progressBar(item.progress),
  total: (item) =>
    item.trackCountTotal > 0
      ? String(item.trackCountTotal)
      : '<span class="text-muted">—</span>',
  downloaded: (item) => {
    if (item.trackCountTotal > 0) {
      return (
        escapeHtml(String(item.trackCountDownloaded)) +
        " / " +
        escapeHtml(String(item.trackCountTotal))
      );
    }
    return '<span class="text-muted">—</span>';
  },
  detail: (item) => {
    let html = item.uuid
      ? `<span class="font-mono text-sm" title="${escapeHtml(item.uuid)}">${escapeHtml(item.uuid.slice(0, 24))}…</span>`
      : '<span class="text-muted">—</span>';
    if (item.errorMessage) {
      html += `<div class="text-sm" style="color:var(--red);margin-top:2px" title="${escapeHtml(item.errorMessage)}">${escapeHtml(item.errorMessage.slice(0, 40))}${item.errorMessage.length > 40 ? "…" : ""}</div>`;
    }
    return html;
  },
  created: (item) => formatTimestamp(item.createdAt),
  updated: (item) => formatTimestamp(item.updatedAt),
  actions: (item) => {
    let html = "";
    if (item.status === "failed" && item.id)
      html += `<button class="btn btn-sm btn-icon" data-act="retry" data-id="${item.id}" title="Retry"><i class="fa-solid fa-rotate"></i></button>`;
    if (item.spotifyPlaylistUrl)
      html += `<button class="btn btn-sm btn-icon" data-act="restart" data-id="${item.id || ""}" data-url="${escapeHtml(item.spotifyPlaylistUrl)}" title="Re-download via deemix"><i class="fa-solid fa-arrows-rotate"></i></button>`;
    if (item.id)
      html += `<button class="btn btn-sm btn-icon" data-act="delete" data-id="${item.id}" title="Remove"><i class="fa-solid fa-trash"></i></button>`;
    return html || '<span class="text-muted">—</span>';
  },
};

function formatTimestamp(ts) {
  if (!ts) return '<span class="text-muted">—</span>';
  const d = new Date(ts * 1000);
  return `<span class="font-mono text-sm" title="${d.toISOString()}">${d.toLocaleDateString()} ${d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</span>`;
}

function statusBadge(status) {
  const styles = {
    queued: {
      bg: "rgba(245,158,11,0.1)",
      color: "var(--yellow)",
      icon: "fa-solid fa-clock",
    },
    downloading: {
      bg: "rgba(59,130,246,0.1)",
      color: "var(--blue, #3b82f6)",
      icon: "fa-solid fa-spinner fa-spin",
    },
    completed: {
      bg: "rgba(34,197,94,0.1)",
      color: "var(--green)",
      icon: "fa-solid fa-check",
    },
    failed: {
      bg: "rgba(239,68,68,0.1)",
      color: "var(--red)",
      icon: "fa-solid fa-exclamation-triangle",
    },
  };
  const s = styles[status] || styles.queued;
  return `<span class="status-badge" style="background:${s.bg};color:${s.color};white-space:nowrap"><i class="${s.icon}"></i> ${escapeHtml(status)}</span>`;
}

function progressBar(pct) {
  const clamped = Math.min(100, Math.max(0, pct));
  const color =
    clamped >= 100
      ? "var(--green, #22c55e)"
      : clamped > 0
        ? "var(--accent, #6366f1)"
        : "var(--text-muted)";
  return `<div style="display:flex;align-items:center;gap:6px">
    <div style="flex:1;height:6px;background:var(--border);border-radius:3px;overflow:hidden">
      <div style="width:${clamped}%;height:100%;background:${color};border-radius:3px;transition:width 0.3s"></div>
    </div>
    <span class="font-mono text-sm" style="color:var(--text-muted);min-width:32px;text-align:right">${clamped}%</span>
  </div>`;
}

function adaptItem(item) {
  return {
    id: item.id,
    uuid: item.uuid || null,
    spotifyPlaylistUrl: item.spotifyPlaylistUrl || null,
    playlistName: item.playlistName || null,
    status: item.status || "queued",
    trackCountTotal: item.trackCountTotal || 0,
    trackCountDownloaded: item.trackCountDownloaded || 0,
    errorMessage: item.errorMessage || null,
    createdAt: item.createdAt || null,
    updatedAt: item.updatedAt || null,
    title: item.title || null,
    artist: item.artist || null,
    progress: item.progress || 0,
  };
}

function renderToolbar(search, state) {
  const selStatus = state.statusFilter || "all";
  return `<div class="filter-panel" id="dq-filter-panel">
    <div class="filter-panel-header">
      ${renderSearchInput("deemix-queue", search)}
      <button class="filter-panel-toggle" id="dq-filter-toggle" title="Toggle filters">
        <i class="fas fa-chevron-up chevron"></i>
      </button>
    </div>
    <div class="filter-panel-body">
      <div class="filter-row">
        <span class="filter-row-label toggleable" data-filter="status">Status</span>
        <div class="filter-group" id="dq-status-btns" style="flex-wrap:wrap">
          ${STATUS_OPTIONS.map(
            (opt) =>
              `<button class="filter-btn${selStatus === opt.value ? " active" : ""}" data-value="${opt.value}">${opt.label}</button>`,
          ).join("")}
        </div>
      </div>
    </div>
  </div>`;
}

function renderBody(data, state) {
  const items = data.items || [];
  const totalCount = data._total ?? items.length;
  const totalPages = Math.ceil(totalCount / state.pageSize) || 1;
  const pageId = "dq";
  const colConfig = loadColumnConfig("deemix-queue", DEEMIX_COLUMNS);

  const rowsHtml = items
    .map((item) => {
      return `<tr>
      ${renderColumnCells(colConfig, DEEMIX_COLUMNS, DEEMIX_CELL_RENDERERS, item)}
    </tr>`;
    })
    .join("");

  const thHtml = renderColumnHeaders(colConfig, DEEMIX_COLUMNS, state, sortableTh);
  const stats = `<div class="stats-row"><div class="stats-group">
    <button class="btn btn-sm btn-icon" id="dq-refresh" title="Refresh"><i class="fa-solid fa-rotate"></i></button>
    <strong>${totalCount.toLocaleString()}</strong> queue item${totalCount !== 1 ? "s" : ""}
    ${renderPageSizeSelector(state.pageSize)}
    ${renderColumnConfigTrigger()}
    ${
      state.layoutMode
        ? '<button class="btn btn-sm btn-primary" id="dq-layout-btn" style="margin-left:8px"><i class="fas fa-check"></i> Done</button>'
        : '<button class="btn btn-sm" id="dq-layout-btn" style="margin-left:8px"><i class="fas fa-arrows-alt"></i> Modify Column Layout</button>'
    }
  </div></div>`;

  const pagination = `<div class="pagination" id="${pageId}-pagination">
    <button class="pagination-btn" id="${pageId}-prev" disabled><i class="fa-solid fa-chevron-left"></i></button>
    <span class="pagination-info" id="${pageId}-info">Page ${state.page + 1} of ${totalPages}</span>
    <button class="pagination-btn" id="${pageId}-next" ${totalPages <= 1 ? "disabled" : ""}><i class="fa-solid fa-chevron-right"></i></button>
  </div>`;

  return `${stats}\n<div class="table-wrap"><table class="data-table"><thead><tr>${thHtml}</tr></thead><tbody>${rowsHtml}</tbody></table></div>\n${pagination}`;
}

function renderEmptyBody(state) {
  const colConfig = loadColumnConfig("deemix-queue", DEEMIX_COLUMNS);
  const thHtml = renderColumnHeaders(colConfig, DEEMIX_COLUMNS, state, sortableTh);
  const visibleCount = colConfig.filter((c) => c.visible).length;
  return `<div class="stats-row"><div class="stats-group">
    <button class="btn btn-sm btn-icon" id="dq-refresh" title="Refresh"><i class="fa-solid fa-rotate"></i></button>
    <strong>0</strong> queue items
    ${renderPageSizeSelector(state.pageSize)}
    ${renderColumnConfigTrigger()}
  </div></div>
  <div class="table-wrap"><table class="data-table"><thead><tr>${thHtml}</tr></thead>
    <tbody><tr><td colspan="${visibleCount}" class="text-center text-muted" style="padding:32px">No queue items found. Add a playlist URL to get started.</td></tr></tbody>
  </table></div>`;
}

function buildParams(state) {
  const params = new URLSearchParams();
  params.set("limit", String(state.pageSize));
  params.set("offset", String(state.page * state.pageSize));
  if (state.sort) params.set("sort", state.sort);
  if (state.order) params.set("order", state.order);
  if (state.search) params.set("search", state.search);
  if (state.statusFilter !== "all") params.set("status", state.statusFilter);
  return params;
}

function setContent(html) {
  const el = document.getElementById("dq-content");
  if (el) el.innerHTML = html;
}

async function fetchAndRender(container, signal, state) {
  updateHash("deemix-queue", state, {
    sort: "",
    order: "asc",
    search: "",
    status: "all",
    page: 0,
  });
  setContent(renderLoading("Loading queue…"));
  try {
    const resp = await fetchJSON(`/api/services/deemix/queue?${buildParams(state)}`, {
      signal,
    });
    if (signal.aborted) return;
    const data = resp.data || {};
    const items = (data.items || []).map(adaptItem);
    const totalCount = data.total ?? items.length;
    if (items.length === 0 && totalCount === 0) {
      setContent(renderEmptyBody(state));
      wireContentEvents(container, signal, state);
      return;
    }
    setContent(renderBody({ _total: totalCount, items: items }, state));
    wireContentEvents(container, signal, state);
  } catch (err) {
    if (err.name === "AbortError") return;
    try {
      if (signal.aborted) return;
    } catch {
      return;
    }
    setContent(
      renderErrorBlock({
        title: "Failed to load deemix queue",
        detail: err.message,
        retryFn: "window.location.hash='#deemix-queue'",
      }),
    );
  }
}

function wireToolbarEvents(container, signal, state) {
  // ── Status filter buttons (single-select) ──
  const statusGroup = container.querySelector("#dq-status-btns");
  if (statusGroup) {
    statusGroup.addEventListener(
      "click",
      (e) => {
        const btn = e.target.closest(".filter-btn");
        if (!btn) return;
        const val = btn.dataset.value;
        state.statusFilter = state.statusFilter === val ? "all" : val;
        state.page = 0;
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  }

  // ── Generic toggle for data-filter labels ──
  const filterPanel = container.querySelector("#dq-filter-panel");
  filterPanel?.querySelectorAll("[data-filter]").forEach((label) => {
    function updateFilterUI() {
      const key = label.dataset.filter + "Enabled";
      const isActive = state[key] !== false;
      label.classList.toggle("active", isActive);
      label.classList.toggle("off", !isActive);
      const row = label.closest(".filter-row");
      if (row) {
        const inputs = row.querySelectorAll("select, input, button, .filter-group");
        inputs.forEach((el) => el.classList.toggle("filter-disabled", !isActive));
      }
    }
    label.addEventListener("click", () => {
      const key = label.dataset.filter + "Enabled";
      state[key] = state[key] === false ? true : false;
      state.page = 0;
      updateFilterUI();
      updateHash("deemix-queue", state, {
        sort: "",
        order: "asc",
        search: "",
        status: "all",
        page: 0,
      });
      fetchAndRender(container, signal, state);
    });
    updateFilterUI();
  });

  // ── Auto-enable disabled filter sections on click ──
  filterPanel?.addEventListener("click", (e) => {
    const row = e.target.closest(".filter-row");
    if (!row) return;
    const label = row.querySelector("[data-filter]");
    if (!label) return;
    const key = label.dataset.filter + "Enabled";
    if (state[key] !== false) return;
    if (e.target.closest("[data-filter]")) return;
    state[key] = true;
    state.page = 0;
    label.classList.add("active");
    label.classList.remove("off");
    const inputs = row.querySelectorAll("select, input, button, .filter-group");
    inputs.forEach((el) => el.classList.remove("filter-disabled"));
    fetchAndRender(container, signal, state);
  });
}

function wireContentEvents(container, signal, state) {
  const refreshBtn = container.querySelector("#dq-refresh");
  if (refreshBtn) refreshBtn.onclick = () => fetchAndRender(container, signal, state);
  const prevBtn = container.querySelector("#dq-prev");
  if (prevBtn) {
    prevBtn.disabled = state.page === 0;
    prevBtn.onclick = () => {
      if (state.page > 0) {
        state.page--;
        fetchAndRender(container, signal, state);
      }
    };
  }
  const nextBtn = container.querySelector("#dq-next");
  if (nextBtn) {
    nextBtn.onclick = () => {
      state.page++;
      fetchAndRender(container, signal, state);
    };
  }
  const tableEl = container.querySelector(".data-table");
  if (tableEl) {
    wireSortableHeaders(tableEl, state, () => {
      updateHash("deemix-queue", state, {
        sort: "",
        order: "asc",
        search: "",
        status: "all",
        page: 0,
      });
      fetchAndRender(container, signal, state);
    });
  }
  wirePageSizeSelector(container, state, () => {
    updateHash("deemix-queue", state, {
      sort: "",
      order: "asc",
      search: "",
      status: "all",
      page: 0,
    });
    fetchAndRender(container, signal, state);
  });
  const table = container.querySelector(".data-table");
  if (table) {
    table.addEventListener(
      "click",
      async (e) => {
        const btn = e.target.closest("[data-act]");
        if (!btn) return;
        const act = btn.dataset.act;
        const id = parseInt(btn.dataset.id, 10);
        if (act === "retry") {
          btn.disabled = true;
          btn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i>';
          try {
            await fetchJSON(`/api/services/deemix/queue/${id}/retry`, { method: "POST" });
            await fetchAndRender(container, signal, state);
          } catch (err) {
            showToast(`Retry failed: ${err.message}`, "error");
            btn.disabled = false;
            btn.innerHTML = '<i class="fa-solid fa-rotate"></i>';
          }
        } else if (act === "restart") {
          const localId = parseInt(btn.dataset.id, 10);
          const url = btn.dataset.url;
          if (!url) return;
          btn.disabled = true;
          btn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i>';
          try {
            if (localId) {
              await fetchJSON(`/api/services/deemix/queue/${localId}/retry`, {
                method: "POST",
              });
            } else {
              await fetchJSON("/api/services/deemix/queue", {
                method: "POST",
                body: JSON.stringify({ url }),
              });
            }
            showToast("Re-download triggered", "success");
            await fetchAndRender(container, signal, state);
          } catch (err) {
            showToast(`Re-download failed: ${err.message}`, "error");
            btn.disabled = false;
            btn.innerHTML = '<i class="fa-solid fa-arrows-rotate"></i>';
          }
        } else if (act === "delete") {
          if (!confirm("Remove this item?")) return;
          btn.disabled = true;
          btn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i>';
          try {
            await fetchJSON(`/api/services/deemix/queue/${id}`, { method: "DELETE" });
            await fetchAndRender(container, signal, state);
          } catch (err) {
            showToast(`Delete failed: ${err.message}`, "error");
            btn.disabled = false;
            btn.innerHTML = '<i class="fa-solid fa-trash"></i>';
          }
        }
      },
      { signal },
    );
  }

  // Column customization wiring
  const colConfig = loadColumnConfig("deemix-queue", DEEMIX_COLUMNS);
  if (state.layoutMode) {
    wireColumnResize(container, "deemix-queue", DEEMIX_COLUMNS, colConfig);
    wireColumnDragReorder(container, "deemix-queue", DEEMIX_COLUMNS, colConfig, () => {
      reorderTableColumns(container, colConfig);
    });
  }
  wireConfigTrigger(container, "deemix-queue", DEEMIX_COLUMNS, colConfig, () => {
    fetchAndRender(container, signal, state);
  });

  // Layout mode toggle
  const layoutBtn = container.querySelector("#dq-layout-btn");
  if (layoutBtn) {
    layoutBtn.onclick = () => {
      state.layoutMode = !state.layoutMode;
      document.body.classList.toggle("layout-mode", state.layoutMode);
      fetchAndRender(container, signal, state);
    };
  }
}

export async function init(container, signal, hashParams) {
  const state = {
    page: parseInt(hashParams?.page) || 0,
    pageSize: getPageSize(),
    search: hashParams?.search || "",
    sort: hashParams?.sort || "",
    order: hashParams?.order || "asc",
    statusFilter: hashParams?.status || "all",
    statusEnabled: true,
    layoutMode: false,
  };

  // Reset layout mode on page entry
  document.body.classList.remove("layout-mode");

  // Render stable toolbar + actions panel + content wrapper ONCE
  container.innerHTML = `
    <div style="display:flex;flex-direction:column;gap:var(--space-4);">
      <div style="display:flex;gap:var(--space-4);align-items:flex-start;">
        <div style="flex:4;min-width:0;">${renderToolbar(state.search, state)}</div>
        <div class="actions-panel" style="flex:1;min-width:180px;max-width:220px;">
          <div class="actions-panel-header">
            <span><i class="fas fa-bolt"></i> Actions</span>
            <span class="actions-sel-count" id="dq-sel-count">0</span>
          </div>
          <button class="btn btn-sm" id="dq-actions-refresh"><i class="fas fa-rotate"></i> Refresh</button>
        </div>
      </div>
      <div id="dq-content" style="min-height:200px;">${renderLoading("Loading queue…")}</div>
    </div>`;

  // Wire filter panel toggle
  const toggleBtn = container.querySelector("#dq-filter-toggle");
  const filterPanel = container.querySelector("#dq-filter-panel");
  if (toggleBtn && filterPanel) {
    const saved = localStorage.getItem("filterPanelCollapsed_deemix-queue");
    if (saved === "true") filterPanel.classList.add("collapsed");
    toggleBtn.addEventListener("click", () => {
      filterPanel.classList.toggle("collapsed");
      localStorage.setItem(
        "filterPanelCollapsed_deemix-queue",
        filterPanel.classList.contains("collapsed"),
      );
    });
  }

  // Wire search + filter once (toolbar is stable)
  const toolbar = container.querySelector(".filter-panel");
  if (toolbar)
    wireSearchFilter(toolbar, state, () => fetchAndRender(container, signal, state));

  // Wire toolbar filter events (status buttons)
  wireToolbarEvents(container, signal, state);

  // Wire actions panel refresh
  import("../shared/actions-panel.js").then(({ wireActionsRefresh }) => {
    wireActionsRefresh(container, "dq", () => {
      state.page = 0;
      return fetchAndRender(container, signal, state);
    });
  });

  // Fetch initial data
  await fetchAndRender(container, signal, state);
}
