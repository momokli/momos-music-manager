/**
 * tasks.js — Task management page.
 *
 * CRUD page following the canonical pattern: stable toolbar + body with
 * sortable table headers, page-size selector, hash URL state, and 5s
 * auto-poll that only re-renders the body (not the toolbar).
 *
 * Actions: cancel (running/pending), retry (failed), view logs (completed/failed).
 */

import {
  escapeHtml,
  renderLoading,
  renderEmpty,
  renderErrorBlock,
  renderTable,
  td,
  showToast,
  showModal,
} from "../shared/components.js";
import { formatDateTime } from "../shared/format.js";
import { fetchJSON } from "../shared/api.js";
import {
  renderSearchInput,
  renderFilterGroup,
  wireSearchFilter,
} from "../shared/search-filter.js";
import {
  getPageSize,
  renderPageSizeSelector,
  wirePageSizeSelector,
  sortableTh,
  wireSortableHeaders,
  updateHash,
  parseHash,
} from "../shared/crud.js";
import {
  loadColumnConfig,
  renderColumnConfigTrigger,
  renderColumnHeaders,
  renderColumnCells,
  wireColumnResize,
  wireColumnDragReorder,
  wireConfigTrigger,
} from "../shared/column-config.js";

/* ------------------------------------------------------------------ */
/*  Constants                                                          */
/* ------------------------------------------------------------------ */

const POLL_INTERVAL = 5000; // ms

const STATUS_ICONS = {
  running: "fa-spinner fa-spin",
  completed: "fa-check",
  pending: "fa-clock",
  failed: "fa-xmark",
  cancelled: "fa-ban",
};

const TYPE_LABELS = {
  spotify_sync: { label: "Spotify Sync", svc: "spotify" },
  soundcloud_sync: { label: "SoundCloud Sync", svc: "soundcloud" },
  youtube_sync: { label: "YouTube Sync", svc: "youtube" },
  write_comment: { label: "Write Comment", svc: "files" },
  scan_folder: { label: "Scan Folder", svc: "folders" },
  recompute_embeddings: { label: "Recompute Embeddings", svc: "tags" },
  traktor_import: { label: "Traktor Import", svc: "files" },
  deemix_sync: { label: "Deemix Sync", svc: "deemix" },
};

const STATUS_MAP = {
  Pending: "pending",
  Running: "running",
  Completed: "completed",
  Failed: "failed",
  Cancelled: "cancelled",
};

const STATUS_OPTIONS = [
  { value: "all", label: "All" },
  { value: "running", label: "Running" },
  { value: "pending", label: "Pending" },
  { value: "completed", label: "Completed" },
  { value: "failed", label: "Failed" },
  { value: "cancelled", label: "Cancelled" },
];

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

function adaptTask(t) {
  let progress = 0;
  if (t.percent != null) {
    progress = Math.round(t.percent);
  } else if (t.progress) {
    const m = String(t.progress).match(/(\d+(?:\.\d+)?)/);
    if (m) progress = Math.round(parseFloat(m[1]));
  }

  const typeInfo = TYPE_LABELS[t.task_type] || {
    label: t.task_type || "Unknown",
    svc: "tasks",
  };

  const status = STATUS_MAP[t.status] || String(t.status || "pending").toLowerCase();
  let error = "";
  if (status === "failed") {
    if (t.progress && !t.progress.match(/^\d/)) {
      error = String(t.progress);
    } else if (t.logs && t.logs.length > 0) {
      error = t.logs[t.logs.length - 1];
    }
    if (!error && t.logs && t.logs.length > 0) {
      const last = t.logs[t.logs.length - 1];
      if (last.toLowerCase().includes("error") || last.toLowerCase().includes("fail")) {
        error = last;
      }
    }
  }

  return {
    id: t.id,
    type: typeInfo.label,
    service: typeInfo.svc,
    status,
    progress,
    created:
      t.created_at_secs != null
        ? new Date(t.created_at_secs * 1000).toISOString()
        : new Date().toISOString(),
    updated:
      t.updated_at_secs != null ? new Date(t.updated_at_secs * 1000).toISOString() : null,
    details: t.task_details ? JSON.stringify(t.task_details) : "",
    logs: t.logs || [],
    error,
  };
}

function statusBadge(task) {
  const icon = STATUS_ICONS[task.status] || "fa-circle";
  return `<span class="status-badge ${task.status}"><i class="fas ${icon}"></i> ${task.status.charAt(0).toUpperCase() + task.status.slice(1)}</span>`;
}

function progressCell(task) {
  const barColor =
    task.status === "completed"
      ? "var(--green)"
      : task.status === "failed"
        ? "var(--red)"
        : task.status === "cancelled"
          ? "var(--text-muted)"
          : "var(--accent)";

  return `<div class="flex items-center gap-2">
    <span class="font-mono text-sm" style="min-width:36px;text-align:right">${task.progress}%</span>
    <div class="progress-bar" style="width:80px;flex-shrink:0">
      <div class="progress-bar-fill" style="width:${task.progress}%;background:${barColor}"></div>
    </div>
  </div>`;
}

function actionButtons(task) {
  if (task.status === "running" || task.status === "pending") {
    return `<button class="btn btn-sm btn-red" data-action="cancel" data-id="${task.id}"><i class="fa-solid fa-stop"></i> Cancel</button>`;
  }
  if (task.status === "completed") {
    return `<button class="btn btn-sm" data-action="logs" data-id="${task.id}"><i class="fa-solid fa-file-lines"></i> Logs</button>`;
  }
  if (task.status === "failed") {
    return `<div class="flex gap-1">
      <button class="btn btn-sm btn-yellow" data-action="retry" data-id="${task.id}"><i class="fa-solid fa-redo"></i> Retry</button>
      <button class="btn btn-sm" data-action="logs" data-id="${task.id}"><i class="fa-solid fa-file-lines"></i> Logs</button>
    </div>`;
  }
  return `<span class="text-muted" style="font-size:0.85rem">—</span>`;
}

/* ------------------------------------------------------------------ */
/*  Render helpers                                                     */
/* ------------------------------------------------------------------ */

function renderTaskRow(t) {
  const svcIcon =
    t.service === "spotify"
      ? "fa-brands fa-spotify"
      : t.service === "soundcloud"
        ? "fa-brands fa-soundcloud"
        : t.service === "youtube"
          ? "fa-brands fa-youtube"
          : t.service === "deemix"
            ? "fa-solid fa-download"
            : t.service === "folders"
              ? "fa-regular fa-folder-open"
              : "fa-solid fa-tag";

  const detailsHtml = `<div class="flex items-center gap-2" style="margin-bottom:2px"><i class="${svcIcon}" style="color:var(--text-muted);width:16px"></i> <strong>${escapeHtml(t.type)}</strong></div>
    <div style="font-size:0.8rem;color:var(--text-muted)">#${t.id}${t.details ? " · " + escapeHtml(t.details.substring(0, 60)) : ""}</div>`;

  const createdHtml = t.created
    ? `<span style="color:var(--text-muted);font-size:0.85rem">${formatDateTime(t.created)}</span>`
    : `<span class="text-muted" style="font-size:0.85rem">—</span>`;

  const updatedHtml =
    t.updated && t.updated !== t.created
      ? `<span style="color:var(--text-muted);font-size:0.85rem">${formatDateTime(t.updated)}</span>`
      : `<span class="text-muted" style="font-size:0.85rem">—</span>`;

  return `<tr>
    ${td(statusBadge(t), { style: "width:8%" })}
    ${td(detailsHtml, { style: "width:30%" })}
    ${td(progressCell(t), { style: "width:20%" })}
    ${td(createdHtml, { style: "width:14%" })}
    ${td(updatedHtml, { style: "width:14%" })}
    ${td(actionButtons(t), { style: "width:14%" })}
  </tr>`;
}

/* ------------------------------------------------------------------ */
/*  Column model and cell renderers                                    */
/* ------------------------------------------------------------------ */

const TASKS_COLUMNS = [
  { id: "status", label: "Status", sortable: true, sortKey: "status", defaultWidth: 80 },
  { id: "details", label: "Details", sortable: false, defaultWidth: 300 },
  {
    id: "progress",
    label: "Progress",
    sortable: true,
    sortKey: "progress",
    defaultWidth: 200,
  },
  {
    id: "created",
    label: "Created",
    sortable: true,
    sortKey: "created_at",
    defaultWidth: 140,
  },
  {
    id: "updated",
    label: "Updated",
    sortable: true,
    sortKey: "updated_at",
    defaultWidth: 140,
  },
  { id: "actions", label: "Actions", sortable: false, defaultWidth: 140 },
];

const TASKS_CELL_RENDERERS = {
  status: (t) => statusBadge(t),
  details: (t) => {
    const svcIcon =
      t.service === "spotify"
        ? "fa-brands fa-spotify"
        : t.service === "soundcloud"
          ? "fa-brands fa-soundcloud"
          : t.service === "youtube"
            ? "fa-brands fa-youtube"
            : t.service === "deemix"
              ? "fa-solid fa-download"
              : t.service === "folders"
                ? "fa-regular fa-folder-open"
                : "fa-solid fa-tag";
    return `<div class="flex items-center gap-2" style="margin-bottom:2px"><i class="${svcIcon}" style="color:var(--text-muted);width:16px"></i> <strong>${escapeHtml(t.type)}</strong></div>
    <div style="font-size:0.8rem;color:var(--text-muted)">#${t.id}${t.details ? " · " + escapeHtml(t.details.substring(0, 60)) : ""}</div>`;
  },
  progress: (t) => progressCell(t),
  created: (t) =>
    t.created
      ? `<span style="color:var(--text-muted);font-size:0.85rem">${formatDateTime(t.created)}</span>`
      : `<span class="text-muted" style="font-size:0.85rem">—</span>`,
  updated: (t) =>
    t.updated && t.updated !== t.created
      ? `<span style="color:var(--text-muted);font-size:0.85rem">${formatDateTime(t.updated)}</span>`
      : `<span class="text-muted" style="font-size:0.85rem">—</span>`,
  actions: (t) => actionButtons(t),
};

/* ------------------------------------------------------------------ */
/*  Body render (re-rendered on every data change)                     */
/* ------------------------------------------------------------------ */

function renderBody(data, state) {
  const tasks = data.tasks || [];
  const total = data._total || tasks.length;
  const running = tasks.filter((t) => t.status === "running").length;
  const pending = tasks.filter((t) => t.status === "pending").length;
  const failed = tasks.filter((t) => t.status === "failed").length;
  const totalPages = Math.max(1, Math.ceil(total / state.pageSize));
  const currentPage = state.page + 1;
  const colConfig = state._colConfig || loadColumnConfig("tasks", TASKS_COLUMNS);

  const countHtml = [
    running
      ? `<span class="status-badge running"><i class="fa-solid fa-spinner fa-spin"></i> ${running} running</span>`
      : "",
    pending
      ? `<span class="status-badge pending"><i class="fa-regular fa-clock"></i> ${pending} pending</span>`
      : "",
    failed
      ? `<span class="status-badge failed"><i class="fa-solid fa-xmark"></i> ${failed} failed</span>`
      : "",
  ]
    .filter(Boolean)
    .join(" ");

  const headers = renderColumnHeaders(colConfig, TASKS_COLUMNS, state, sortableTh);
  const visibleCount = colConfig.filter((c) => c.visible).length;

  const rows =
    tasks.length > 0
      ? tasks
          .map(
            (t) =>
              `<tr>${renderColumnCells(colConfig, TASKS_COLUMNS, TASKS_CELL_RENDERERS, t)}</tr>`,
          )
          .join("")
      : `<tr><td colspan="${visibleCount}"><div class="text-center text-muted" style="padding:32px">No tasks found</div></td></tr>`;

  return `
    <div class="stats-row">
      <div class="stats-group">
        <button class="btn btn-sm btn-icon" id="tasks-refresh-btn" title="Refresh"><i class="fa-solid fa-redo"></i></button>
        <strong>${total}</strong> tasks
        ${countHtml ? " " + countHtml : ""}
      </div>
      <div class="stats-group">
        ${renderColumnConfigTrigger()}
        ${renderPageSizeSelector(state.pageSize)}
      </div>
    </div>

    <div class="table-wrap" style="overflow-x:auto">
      <table class="data-table" id="tasks-table">
        <thead><tr>${headers}</tr></thead>
        <tbody>${rows}</tbody>
      </table>
    </div>

    <div class="pagination">
      <button class="pagination-btn" id="tasks-prev" ${state.page === 0 ? "disabled" : ""}>
        <i class="fa-solid fa-chevron-left"></i>
      </button>
      <span class="pagination-info">Page ${currentPage} of ${totalPages}</span>
      <button class="pagination-btn" id="tasks-next" ${state.page >= totalPages - 1 ? "disabled" : ""}>
        <i class="fa-solid fa-chevron-right"></i>
      </button>
    </div>
  `;
}

/* ------------------------------------------------------------------ */
/*  URL params builder                                                 */
/* ------------------------------------------------------------------ */

function buildParams(state) {
  const params = new URLSearchParams();
  params.set("limit", String(state.pageSize));
  params.set("offset", String(state.page * state.pageSize));
  if (state.sort) params.set("sort", state.sort);
  if (state.order) params.set("order", state.order);
  if (state.search) params.set("search", state.search);
  if (state.status && state.status !== "all") params.set("status", state.status);
  return params;
}

/* ------------------------------------------------------------------ */
/*  Action handlers                                                    */
/* ------------------------------------------------------------------ */

async function cancelTask(id, onRefresh) {
  if (!confirm("Cancel this task?")) return;
  try {
    await fetchJSON(`/api/tasks/${id}`, { method: "DELETE" });
    showToast("Task cancelled", "success");
    if (onRefresh) onRefresh();
  } catch (err) {
    showToast(`Failed to cancel task: ${err.message}`, "error");
  }
}

async function retryTask(task) {
  showToast(
    `Cannot retry automatically. ${task.type} tasks must be re-triggered from their source page.`,
    "error",
  );
}

async function viewLogs(task) {
  try {
    let taskDetails = task;
    try {
      const resp = await fetchJSON(`/api/tasks/${task.id}`);
      if (resp && resp.data) {
        taskDetails = { ...task, ...adaptTask(resp.data) };
      }
    } catch {
      // Fall back to what we have
    }

    const t = taskDetails;

    const logsBody = `
      <div class="modal-body" style="padding:var(--space-6);">
        <div class="form-group">
          <label>Status</label>
          <div><span class="status-badge ${t.status}"><i class="fas ${STATUS_ICONS[t.status] || "fa-circle"}"></i> ${t.status.charAt(0).toUpperCase() + t.status.slice(1)}</span></div>
        </div>
        <div class="form-group">
          <label>Progress</label>
          <div class="progress-bar" style="width:100%">
            <div class="progress-bar-fill" style="width:${t.progress}%;background:${t.status === "completed" ? "var(--green)" : t.status === "failed" ? "var(--red)" : "var(--accent)"}"></div>
          </div>
          <div style="text-align:right;font-size:0.85rem;color:var(--text-muted)">${t.progress}%</div>
        </div>
        ${
          t.details
            ? `<div class="form-group">
          <label>Details</label>
          <pre style="background:var(--surface);padding:var(--space-3);border-radius:var(--radius-md);font-size:0.85rem;overflow-x:auto;max-height:200px;overflow-y:auto;white-space:pre-wrap;word-break:break-word;">${escapeHtml(t.details)}</pre>
        </div>`
            : ""
        }
        ${
          t.error
            ? `<div class="form-group">
          <label style="color:var(--red)">Error</label>
          <pre style="background:rgba(239,68,68,0.1);padding:var(--space-3);border-radius:var(--radius-md);font-size:0.85rem;overflow-x:auto;border:1px solid rgba(239,68,68,0.2);white-space:pre-wrap;word-break:break-word;">${escapeHtml(t.error)}</pre>
        </div>`
            : ""
        }
        ${
          t.logs && t.logs.length > 0
            ? `<div class="form-group">
          <label>Logs <span style="font-weight:400;color:var(--text-muted);font-size:0.8rem">(${t.logs.length} entries)</span></label>
          <pre style="background:var(--surface);padding:var(--space-3);border-radius:var(--radius-md);font-size:0.82rem;overflow-x:auto;max-height:300px;overflow-y:auto;white-space:pre-wrap;word-break:break-word;font-family:var(--font-mono, monospace);line-height:1.5">${t.logs.map((l) => escapeHtml(l)).join("\n")}</pre>
        </div>`
            : ""
        }
        <div class="form-group">
          <label>Created</label>
          <div style="color:var(--text-muted);font-size:0.9rem;">${formatDateTime(t.created)}</div>
        </div>
      </div>
      <div class="modal-actions">
        <button class="btn" data-modal-action="close">Close</button>
      </div>
    `;

    showModal({
      title: `<i class="fa-solid fa-file-lines" style="margin-right:8px;color:var(--accent)"></i> Task #${t.id} — ${escapeHtml(t.type)}`,
      bodyHtml: logsBody,
      width: "600px",
    });
  } catch (err) {
    showToast(`Failed to load task details: ${err.message}`, "error");
  }
}

/* ------------------------------------------------------------------ */
/*  Event wiring                                                       */
/* ------------------------------------------------------------------ */

function wireContentEvents(container, signal, state, onChange) {
  // Refresh button
  const refreshBtn = container.querySelector("#tasks-refresh-btn");
  if (refreshBtn) {
    refreshBtn.addEventListener("click", onChange, { signal });
  }

  // Sortable headers
  const table = container.querySelector("#tasks-table");
  if (table) {
    wireSortableHeaders(table, state, onChange);
  }

  // Page size selector
  wirePageSizeSelector(container, state, onChange);

  // Pagination
  const prevBtn = container.querySelector("#tasks-prev");
  const nextBtn = container.querySelector("#tasks-next");
  if (prevBtn) {
    prevBtn.addEventListener(
      "click",
      () => {
        if (state.page > 0) {
          state.page--;
          onChange();
        }
      },
      { signal },
    );
  }
  if (nextBtn) {
    nextBtn.addEventListener(
      "click",
      () => {
        const totalPages = Math.max(1, Math.ceil((state._total || 0) / state.pageSize));
        if (state.page < totalPages - 1) {
          state.page++;
          onChange();
        }
      },
      { signal },
    );
  }

  // Action buttons (event delegation on table)
  if (table) {
    table.addEventListener(
      "click",
      (e) => {
        const btn = e.target.closest("[data-action]");
        if (!btn) return;
        e.preventDefault();

        const action = btn.dataset.action;
        const id = btn.dataset.id;
        const task = state.tasks.find((t) => String(t.id) === String(id));
        if (!task) return;

        switch (action) {
          case "cancel":
            cancelTask(id, onChange);
            break;
          case "retry":
            retryTask(task);
            break;
          case "logs":
            viewLogs(task);
            break;
        }
      },
      { signal },
    );
  }

  // Column config
  const colConfig = state._colConfig || loadColumnConfig("tasks", TASKS_COLUMNS);
  state._colConfig = colConfig;
  wireColumnResize(container, "tasks", TASKS_COLUMNS, colConfig);
  wireColumnDragReorder(container, "tasks", TASKS_COLUMNS, colConfig, () => {
    fetchAndRender(container, signal, state);
  });
  wireConfigTrigger(container, "tasks", TASKS_COLUMNS, colConfig, () => {
    fetchAndRender(container, signal, state);
  });
}

/* ------------------------------------------------------------------ */
/*  Polling                                                            */
/* ------------------------------------------------------------------ */

function startPolling(container, signal, state) {
  if (state.pollTimer) return;

  state.pollTimer = setInterval(async () => {
    if (signal.aborted) {
      stopPolling(state);
      return;
    }
    try {
      const resp = await fetchJSON(`/api/tasks?${buildParams(state)}`, { signal });
      if (signal.aborted) return;

      const rawTasks = resp.data?.tasks ?? resp.data ?? [];
      const tasks = rawTasks.map(adaptTask);
      state.tasks = tasks;
      state._total = resp.data?.total ?? tasks.length;

      const contentEl = document.getElementById("tasks-content");
      if (contentEl) {
        contentEl.innerHTML = renderBody({ tasks, _total: state._total }, state);
        wireContentEvents(container, signal, state, () => {
          stopPolling(state);
          fetchAndRender(container, signal, state);
        });
      }
    } catch (err) {
      if (err.name === "AbortError") return;
      // Silently ignore polling errors — next tick may work
    }
  }, POLL_INTERVAL);
}

function stopPolling(state) {
  if (state.pollTimer) {
    clearInterval(state.pollTimer);
    state.pollTimer = null;
  }
}

function hasActiveTasks(tasks) {
  return tasks.some((t) => t.status === "running" || t.status === "pending");
}

/* ------------------------------------------------------------------ */
/*  Data fetching                                                      */
/* ------------------------------------------------------------------ */

async function fetchAndRender(container, signal, state) {
  try {
    const resp = await fetchJSON(`/api/tasks?${buildParams(state)}`, { signal });
    if (signal.aborted) return;

    const rawTasks = resp.data?.tasks ?? resp.data ?? [];
    const tasks = rawTasks.map(adaptTask);
    const total = resp.data?.total ?? tasks.length;

    state.tasks = tasks;
    state._total = total;

    const contentEl = document.getElementById("tasks-content");
    if (contentEl) {
      contentEl.innerHTML = renderBody({ tasks, _total: total }, state);
      wireContentEvents(container, signal, state, () => {
        stopPolling(state);
        fetchAndRender(container, signal, state);
      });
    }

    // Manage polling
    if (hasActiveTasks(tasks)) {
      startPolling(container, signal, state);
    } else {
      stopPolling(state);
    }
  } catch (err) {
    if (err.name === "AbortError") return;
    const contentEl = document.getElementById("tasks-content");
    if (contentEl) {
      contentEl.innerHTML = renderErrorBlock({
        title: "Failed to load tasks",
        detail: err.message,
        retryFn: "window.location.hash='#tasks'",
      });
    }
  }
}

/* ------------------------------------------------------------------ */
/*  Toolbar render (stable, rendered once)                             */
/* ------------------------------------------------------------------ */

function renderToolbar(state) {
  return `<div class="filter-panel" id="tasks-filter-panel">
    <div class="filter-panel-header">
      ${renderSearchInput("tasks", state.search)}
      ${renderFilterGroup("status", STATUS_OPTIONS, state.status)}
      <button class="filter-panel-toggle" id="tasks-filter-toggle" title="Toggle filters">
        <i class="fas fa-chevron-up chevron"></i>
      </button>
    </div>
  </div>`;
}

/* ------------------------------------------------------------------ */
/*  Initialisation                                                     */
/* ------------------------------------------------------------------ */

export async function init(container, signal, hashParams) {
  // Parse hash params into state
  const parsed = parseHash(hashParams, {
    search: { type: "string", default: "" },
    status: { type: "string", default: "all" },
    sort: { type: "string", default: "" },
    order: { type: "string", default: "asc" },
    page: { type: "number", default: 0 },
  });

  const state = {
    page: parsed.page,
    pageSize: getPageSize(25),
    search: parsed.search,
    sort: parsed.sort,
    order: parsed.order,
    status: parsed.status,
    tasks: [],
    _total: 0,
    pollTimer: null,
  };

  stopPolling(state);

  // Render stable toolbar once
  container.innerHTML = `
    <div id="tasks-toolbar" class="toolbar">
      ${renderToolbar(state)}
    </div>
    <div id="tasks-content">${renderLoading("Loading tasks…")}</div>
  `;

  // Wire toolbar events
  const toolbarEl = container.querySelector("#tasks-toolbar");
  if (toolbarEl) {
    wireSearchFilter(toolbarEl, state, () => {
      stopPolling(state);
      updateHash("tasks", state, {
        sort: "",
        order: "asc",
        search: "",
        status: "all",
        page: 0,
      });
      fetchAndRender(container, signal, state);
    });
  }

  // Filter panel toggle
  const toggleBtn = container.querySelector("#tasks-filter-toggle");
  const filterPanel = container.querySelector("#tasks-filter-panel");
  if (toggleBtn && filterPanel) {
    const saved = localStorage.getItem("filterPanelCollapsed_tasks");
    if (saved === "true") filterPanel.classList.add("collapsed");
    toggleBtn.addEventListener("click", () => {
      filterPanel.classList.toggle("collapsed");
      localStorage.setItem(
        "filterPanelCollapsed_tasks",
        filterPanel.classList.contains("collapsed"),
      );
    });
  }

  // Initial data fetch
  await fetchAndRender(container, signal, state);

  // Sync hash with initial state
  updateHash("tasks", state, {
    sort: "",
    order: "asc",
    search: "",
    status: "all",
    page: 0,
  });

  // Visibility change — reload when user comes back
  document.addEventListener(
    "visibilitychange",
    () => {
      if (!document.hidden && !signal.aborted) {
        stopPolling(state);
        fetchAndRender(container, signal, state);
      }
    },
    { signal },
  );
}
