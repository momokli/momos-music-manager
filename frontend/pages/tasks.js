/**
 * tasks.js — Task management page.
 *
 * Lists sync/scan tasks with status, progress, and actions:
 * cancel (running/pending), retry (failed), view logs (completed/failed).
 * Auto-polls while any task is running.
 */

import {
  renderLoading,
  renderEmpty,
  renderErrorBlock,
  Pagination,
  td,
} from "../shared/components.js";
import { formatDateTime } from "../shared/format.js";
import { fetchJSON } from "../shared/api.js";
import { renderFilterGroup, wireSearchFilter } from "../shared/search-filter.js";

/* ------------------------------------------------------------------ */
/*  Constants                                                          */
/* ------------------------------------------------------------------ */

const PAGE_SIZE = 15;
const POLL_INTERVAL = 2500; // ms

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
};

const STATUS_MAP = {
  Pending: "pending",
  Running: "running",
  Completed: "completed",
  Failed: "failed",
  Cancelled: "cancelled",
};

const STATUS_OPTIONS = [
  { value: "", label: "All" },
  { value: "running", label: "Running" },
  { value: "pending", label: "Pending" },
  { value: "completed", label: "Completed" },
  { value: "failed", label: "Failed" },
  { value: "cancelled", label: "Cancelled" },
];

/* ------------------------------------------------------------------ */
/*  State                                                              */
/* ------------------------------------------------------------------ */

let state = {
  tasks: [],
  pollTimer: null,
  status: "",
  page: 0,
};

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

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

function esc(str) {
  if (typeof str !== "string") return str;
  const d = document.createElement("div");
  d.textContent = str;
  return d.innerHTML;
}

/* ------------------------------------------------------------------ */
/*  Adapter                                                            */
/* ------------------------------------------------------------------ */

function adaptTask(t) {
  // Use percent field if available (0-100), fall back to parsing from progress text
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

  // Derive error from progress text or last log entry for failed tasks
  const status = STATUS_MAP[t.status] || String(t.status || "pending").toLowerCase();
  let error = "";
  if (status === "failed") {
    if (t.progress && !t.progress.match(/^\d/)) {
      error = String(t.progress);
    } else if (t.logs && t.logs.length > 0) {
      error = t.logs[t.logs.length - 1];
    }
    // Fallback: try last log even if progress starts with a digit
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
    created: t.created_at_secs
      ? new Date(t.created_at_secs * 1000).toISOString()
      : new Date().toISOString(),
    details: t.task_details ? JSON.stringify(t.task_details) : "",
    logs: t.logs || [],
    error,
  };
}

/* ------------------------------------------------------------------ */
/*  Action handlers                                                    */
/* ------------------------------------------------------------------ */

async function cancelTask(id) {
  if (!confirm("Cancel this task?")) return;

  try {
    await fetchJSON(`/api/tasks/${id}`, { method: "DELETE" });
    showSuccess("Task cancelled");
    await loadTasks();
  } catch (err) {
    showError(`Failed to cancel task: ${err.message}`);
  }
}

async function retryTask(task) {
  // We don't have a dedicated retry endpoint, so we re-trigger
  // based on the task type. For now, show guidance.
  showError(
    `Cannot retry automatically. ${task.type} tasks must be re-triggered from their source page.`,
  );
}

async function viewLogs(task) {
  try {
    // Fetch full task details
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

    const logsHtml = `
      <div class="modal open" id="task-logs-modal">
        <div class="modal-content" style="max-width:600px">
          <div class="modal-header">
            <h3><i class="fa-solid fa-file-lines" style="margin-right:8px;color:var(--accent)"></i> Task #${t.id} — ${esc(t.type)}</h3>
            <button class="close-btn" id="logs-modal-close">&times;</button>
          </div>
          <div style="padding:var(--space-6);">
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
                <pre style="background:var(--surface);padding:var(--space-3);border-radius:var(--radius-md);font-size:0.85rem;overflow-x:auto;max-height:200px;overflow-y:auto;white-space:pre-wrap;word-break:break-word;">${esc(t.details)}</pre>
              </div>`
                : ""
            }
            ${
              t.error
                ? `<div class="form-group">
                <label style="color:var(--red)">Error</label>
                <pre style="background:rgba(239,68,68,0.1);padding:var(--space-3);border-radius:var(--radius-md);font-size:0.85rem;overflow-x:auto;border:1px solid rgba(239,68,68,0.2);white-space:pre-wrap;word-break:break-word;">${esc(t.error)}</pre>
              </div>`
                : ""
            }
            ${
              t.logs && t.logs.length > 0
                ? `<div class="form-group">
                <label>Logs <span style="font-weight:400;color:var(--text-muted);font-size:0.8rem">(${t.logs.length} entries)</span></label>
                <pre style="background:var(--surface);padding:var(--space-3);border-radius:var(--radius-md);font-size:0.82rem;overflow-x:auto;max-height:300px;overflow-y:auto;white-space:pre-wrap;word-break:break-word;font-family:var(--font-mono, monospace);line-height:1.5">${t.logs.map((l) => esc(l)).join("\n")}</pre>
              </div>`
                : ""
            }
            <div class="form-group">
              <label>Created</label>
              <div style="color:var(--text-muted);font-size:0.9rem;">${formatDateTime(t.created)}</div>
            </div>
          </div>
          <div class="modal-actions">
            <button class="btn" id="logs-modal-close-btn">Close</button>
          </div>
        </div>
      </div>
    `;

    const overlay = document.createElement("div");
    overlay.innerHTML = logsHtml;
    document.body.appendChild(overlay.firstElementChild);

    const modal = document.getElementById("task-logs-modal");
    const doClose = () => {
      modal?.classList.remove("open");
      modal?.remove();
    };

    document.getElementById("logs-modal-close")?.addEventListener("click", doClose);
    document.getElementById("logs-modal-close-btn")?.addEventListener("click", doClose);
    modal?.addEventListener("click", (e) => {
      if (e.target === modal) doClose();
    });
    document.addEventListener("keydown", function escHandler(e) {
      if (e.key === "Escape") {
        doClose();
        document.removeEventListener("keydown", escHandler);
      }
    });
  } catch (err) {
    showError(`Failed to load task details: ${err.message}`);
  }
}

/* ------------------------------------------------------------------ */
/*  Polling                                                            */
/* ------------------------------------------------------------------ */

function startPolling() {
  if (state.pollTimer) return;
  state.pollTimer = setInterval(async () => {
    try {
      const resp = await fetchJSON(
        `/api/tasks?limit=${PAGE_SIZE}&offset=0&status=${state.status}`,
      );
      const tasks = (resp.data.tasks || resp.data).map(adaptTask);
      const anyRunning = tasks.some(
        (t) => t.status === "running" || t.status === "pending",
      );

      if (anyRunning) {
        // Refresh the task list in-place
        state.tasks = tasks;
        // Re-render just the table body
        updateTaskTable(tasks);
      } else {
        // All done — reload fully
        stopPolling();
        await loadTasks();
      }
    } catch {
      stopPolling();
    }
  }, POLL_INTERVAL);
}

function stopPolling() {
  if (state.pollTimer) {
    clearInterval(state.pollTimer);
    state.pollTimer = null;
  }
}

function updateTaskTable(tasks) {
  const tbody = document.querySelector("#tasks-tbody");
  if (!tbody) return;

  const rows = tasks.map(renderTaskRow).join("");
  const prevHtml = tbody.innerHTML;
  tbody.innerHTML = rows;

  // Only rewire events if content actually changed
  if (tbody.innerHTML !== prevHtml) {
    // Events are handled via delegation on container, so no need to rewire
  }
}

/* ------------------------------------------------------------------ */
/*  Render helpers                                                     */
/* ------------------------------------------------------------------ */

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
  // cancelled
  return `<span class="text-muted" style="font-size:0.85rem">—</span>`;
}

function renderTaskRow(t) {
  const svcIcon =
    t.service === "spotify"
      ? "fa-brands fa-spotify"
      : t.service === "soundcloud"
        ? "fa-brands fa-soundcloud"
        : t.service === "youtube"
          ? "fa-brands fa-youtube"
          : t.service === "folders"
            ? "fa-regular fa-folder-open"
            : "fa-solid fa-tag";

  return `<tr>
    ${td(`<span class="font-mono text-sm" style="color:var(--text-muted)">#${t.id}</span>`)}
    ${td(`<div class="flex items-center gap-2"><i class="${svcIcon}" style="color:var(--text-muted);width:16px"></i> ${esc(t.type)}</div>`)}
    ${td(statusBadge(t))}
    ${td(progressCell(t))}
    ${td(`<span style="color:var(--text-muted);font-size:0.85rem">${formatDateTime(t.created)}</span>`)}
    ${td(actionButtons(t))}
  </tr>`;
}

/* ------------------------------------------------------------------ */
/*  Render                                                             */
/* ------------------------------------------------------------------ */

function render(container, data) {
  const tasks = data.tasks;
  const running = tasks.filter((t) => t.status === "running").length;
  const pending = tasks.filter((t) => t.status === "pending").length;
  const failed = tasks.filter((t) => t.status === "failed").length;

  const totalPages = Math.max(1, Math.ceil(data._total / PAGE_SIZE));
  const currentPage = data._page + 1;

  container.innerHTML = `
    <div class="toolbar">
      ${renderFilterGroup("status", STATUS_OPTIONS, state.status)}
      <div class="flex items-center gap-3">
        ${
          running > 0
            ? `<span class="status-badge running"><i class="fa-solid fa-spinner fa-spin"></i> ${running} running</span>`
            : ""
        }
        ${pending > 0 ? `<span class="status-badge pending"><i class="fa-regular fa-clock"></i> ${pending} pending</span>` : ""}
        ${failed > 0 ? `<span class="status-badge failed"><i class="fa-solid fa-xmark"></i> ${failed} failed</span>` : ""}
      </div>
    </div>

    <div class="stats-row">
      <div class="stats-group">
        <button class="btn btn-sm btn-icon" id="tasks-refresh-btn" title="Refresh"><i class="fa-solid fa-redo"></i></button>
        <strong>${data._total}</strong> tasks
      </div>
    </div>

    <div class="table-wrap">
      <table class="data-table" id="tasks-table">
        <thead>
          <tr>
            <th style="width:240px">ID</th>
            <th style="width:160px">Type</th>
            <th style="width:125px">Status</th>
            <th style="width:180px">Progress</th>
            <th style="width:140px">Created</th>
            <th style="width:140px">Actions</th>
          </tr>
        </thead>
        <tbody id="tasks-tbody">
          ${tasks.length ? tasks.map(renderTaskRow).join("") : `<tr><td colspan="6"><div class="text-center text-muted" style="padding:32px">No tasks found</div></td></tr>`}
        </tbody>
      </table>
    </div>

    <div class="pagination">
      <button class="pagination-btn" id="tasks-bottom-prev" ${data._page === 0 ? "disabled" : ""}>
        <i class="fa-solid fa-chevron-left"></i>
      </button>
      <span class="pagination-info">Page ${currentPage} of ${totalPages}</span>
      <button class="pagination-btn" id="tasks-bottom-next" ${data._page >= totalPages - 1 ? "disabled" : ""}>
        <i class="fa-solid fa-chevron-right"></i>
      </button>
    </div>
  `;

  // Wire events
  wireEvents(container, data);
}

/* ------------------------------------------------------------------ */
/*  Event wiring                                                       */
/* ------------------------------------------------------------------ */

function wireEvents(container, data) {
  // Refresh button
  const refreshBtn = container.querySelector("#tasks-refresh-btn");
  if (refreshBtn) {
    refreshBtn.addEventListener("click", () => loadTasks());
  }

  // Status filter — wire the button group using shared search-filter
  const toolbar = container.querySelector(".toolbar");
  if (toolbar) {
    wireSearchFilter(toolbar, state, () => {
      data._page = 0;
      loadTasks();
    });
  }

  // Pagination: prev
  const doPrev = () => {
    if (data._page > 0) {
      data._page--;
      loadTasks();
    }
  };
  const prevBtns = ["tasks-bottom-prev"];
  for (const id of prevBtns) {
    const btn = container.querySelector(`#${id}`);
    if (btn) btn.addEventListener("click", doPrev);
  }

  // Pagination: next
  const totalPages = Math.max(1, Math.ceil(data._total / PAGE_SIZE));
  const doNext = () => {
    if (data._page < totalPages - 1) {
      data._page++;
      loadTasks();
    }
  };
  const nextBtns = ["tasks-bottom-next"];
  for (const id of nextBtns) {
    const btn = container.querySelector(`#${id}`);
    if (btn) btn.addEventListener("click", doNext);
  }

  // Action buttons (event delegation on table)
  const table = container.querySelector("#tasks-table");
  if (table) {
    table.addEventListener("click", (e) => {
      const btn = e.target.closest("[data-action]");
      if (!btn) return;
      e.preventDefault();

      const action = btn.dataset.action;
      const id = btn.dataset.id;

      const task = state.tasks.find((t) => t.id === id);
      if (!task) return;

      switch (action) {
        case "cancel":
          cancelTask(id);
          break;
        case "retry":
          retryTask(task);
          break;
        case "logs":
          viewLogs(task);
          break;
      }
    });
  }
}

/* ------------------------------------------------------------------ */
/*  Data loading                                                       */
/* ------------------------------------------------------------------ */

async function loadTasks() {
  const container = document.getElementById("main-content");
  if (!container) return;

  // If we have a signal from init, check it; otherwise use fresh fetch
  const hasActiveSignal = signal && !signal.aborted;

  container.innerHTML = renderLoading("Loading tasks...");

  try {
    const page = typeof data?._page === "number" ? data._page : 0;
    let url = `/api/tasks?limit=${PAGE_SIZE}&offset=${page * PAGE_SIZE}`;
    if (state.status) {
      url += `&status=${state.status}`;
    }

    const opts = hasActiveSignal ? { signal } : {};
    const resp = await fetchJSON(url, opts);
    if (signal?.aborted) return;

    const rawTasks = resp.data.tasks || resp.data || [];
    const tasks = rawTasks.map(adaptTask);

    state.tasks = tasks;

    const total = resp.data.total ?? tasks.length;
    const runningCount = tasks.filter((t) => t.status === "running").length;
    const pendingCount = tasks.filter((t) => t.status === "pending").length;

    render(container, {
      tasks,
      _total: total,
      _page: data?._page || 0,
    });

    // Auto-poll if any tasks are running or pending
    if (runningCount > 0 || pendingCount > 0) {
      startPolling();
    } else {
      stopPolling();
    }
  } catch (err) {
    if (err.name === "AbortError") return;
    container.innerHTML = renderErrorBlock({
      title: "Failed to load tasks",
      detail: err.message,
      retryFn: "window.location.hash='#tasks'",
    });
  }
}

// Module-level data and signal reference for loadTasks
let data = { _page: 0 };
let signal = null;

/* ------------------------------------------------------------------ */
/*  Initialisation                                                     */
/* ------------------------------------------------------------------ */

export async function init(container, sig) {
  signal = sig;
  data = { _page: 0 };
  state.tasks = [];
  state.page = 0;
  stopPolling();

  container.innerHTML = renderLoading("Loading tasks...");

  try {
    let url = `/api/tasks?limit=${PAGE_SIZE}&offset=0`;
    if (state.status) {
      url += `&status=${state.status}`;
    }

    const resp = await fetchJSON(url, { signal });
    if (signal.aborted) return;

    const rawTasks = resp.data.tasks || resp.data || [];
    const tasks = rawTasks.map(adaptTask);

    state.tasks = tasks;

    const total = resp.data.total ?? tasks.length;

    render(container, {
      tasks,
      _total: total,
      _page: 0,
    });

    const runningCount = tasks.filter((t) => t.status === "running").length;
    if (runningCount > 0) {
      startPolling();
    }

    // Visibility change — reload when user comes back
    document.addEventListener(
      "visibilitychange",
      () => {
        if (!document.hidden) {
          loadTasks();
        }
      },
      { signal },
    );
  } catch (err) {
    if (err.name === "AbortError") return;
    container.innerHTML = renderErrorBlock({
      title: "Failed to load tasks",
      detail: err.message,
      retryFn: "window.location.hash='#tasks'",
    });
  }
}
