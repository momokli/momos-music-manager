// Tasks page - Background task management
// API: GET /api/tasks, GET /api/tasks/{id}, DELETE /api/tasks/{id}

const API_BASE = "http://localhost:3000/api";
const PAGE_SIZE = 50;

let currentPage = 1;
let totalTasks = 0;
let pollInterval = null;

async function apiFetch(url, options = {}) {
  try {
    const res = await fetch(url, options);
    if (!res.ok) {
      const text = await res.text();
      throw new Error(`HTTP ${res.status}: ${text}`);
    }
    return await res.json();
  } catch (err) {
    console.error("API error:", err);
    showError(err.message);
    throw err;
  }
}

function showError(msg) {
  const el = document.getElementById("error-message");
  if (!el) return;
  el.textContent = msg;
  el.style.display = "block";
  setTimeout(() => {
    el.style.display = "none";
  }, 5000);
}

function formatTimeAgo(createdAtSecs) {
  if (!createdAtSecs) return "—";
  const now = Date.now() / 1000;
  const diff = now - createdAtSecs;
  if (diff < 60) return "just now";
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  return `${Math.floor(diff / 86400)}d ago`;
}

function formatTimestamp(createdAtSecs) {
  if (!createdAtSecs) return "—";
  const d = new Date(createdAtSecs * 1000);
  return d.toLocaleString();
}

function truncateId(id) {
  if (!id) return "—";
  return id.substring(0, 8) + "…";
}

function statusIcon(status) {
  switch (status) {
    case "Pending":
      return '<i class="fas fa-clock"></i>';
    case "Running":
      return '<i class="fas fa-spinner"></i>';
    case "Completed":
      return '<i class="fas fa-check-circle"></i>';
    case "Failed":
      return '<i class="fas fa-times-circle"></i>';
    case "Cancelled":
      return '<i class="fas fa-ban"></i>';
    default:
      return "";
  }
}

function renderTypeBadge(taskType) {
  const cls =
    taskType === "spotify_sync"
      ? "spotify_sync"
      : taskType === "recompute_embeddings"
        ? "recompute_embeddings"
        : "write_comment";
  const icon =
    taskType === "spotify_sync"
      ? '<i class="fab fa-spotify"></i>'
      : taskType === "recompute_embeddings"
        ? '<i class="fas fa-brain"></i>'
        : '<i class="fas fa-pen"></i>';
  const label =
    taskType === "spotify_sync"
      ? "Spotify Sync"
      : taskType === "recompute_embeddings"
        ? "Recompute Embeddings"
        : "Write Comment";
  return `<span class="task-type-badge ${cls}">${icon} ${label}</span>`;
}

function renderStatusBadge(status) {
  const cls = status.toLowerCase();
  return `<span class="status-badge ${cls}">${statusIcon(status)} ${status}</span>`;
}

function renderServiceBadge(service) {
  if (!service) return "—";
  const cls = service.toLowerCase();
  return `<span class="service-badge ${cls}">${service}</span>`;
}

function renderTaskRow(task) {
  const canCancel = task.status === "Pending" || task.status === "Running";
  const cancelBtn = canCancel
    ? `<button class="btn-cancel" onclick="cancelTask('${task.id}')"><i class="fas fa-stop"></i> Cancel</button>`
    : '<button class="btn-cancel" disabled>Cancel</button>';
  const showLogsBtn =
    task.logs && task.logs.length > 0
      ? `<button onclick="showLogs('${task.id}')" style="background:transparent;border:1px solid #475569;border-radius:4px;color:#94a3b8;cursor:pointer;padding:4px 8px;font-size:0.75rem;margin-left:4px;" title="View logs"><i class="fas fa-list"></i></button>`
      : "";

  return `<tr>
        <td><span class="task-id" title="${task.id}">${truncateId(task.id)}</span></td>
        <td>${renderTypeBadge(task.taskType || task.task_type)}</td>
        <td>${renderStatusBadge(task.status)}</td>
        <td>${renderServiceBadge(task.service)}</td>
        <td><span class="progress-text">${task.progress || "—"}</span> ${showLogsBtn}</td>
        <td><span class="created-time" title="${formatTimestamp(task.createdAtSecs || task.created_at_secs)}">${formatTimeAgo(task.createdAtSecs || task.created_at_secs)}</span></td>
        <td>${cancelBtn}</td>
    </tr>`;
}

async function loadTasks() {
  const filterEl = document.getElementById("status-filter");
  const statusFilter = filterEl ? filterEl.value : "";

  const offset = (currentPage - 1) * PAGE_SIZE;
  let url = `${API_BASE}/tasks?limit=${PAGE_SIZE}&offset=${offset}`;
  if (statusFilter) {
    url += `&status=${statusFilter}`;
  }

  const loadingEl = document.getElementById("loading-state");
  const contentEl = document.getElementById("content-area");
  const emptyEl = document.getElementById("empty-state");

  loadingEl.style.display = "";
  contentEl.classList.add("hidden");
  emptyEl.classList.add("hidden");

  try {
    const response = await apiFetch(url);
    const data = response.data || response;

    const tasks = data.tasks || [];
    totalTasks = data.total || tasks.length;

    loadingEl.style.display = "none";

    const tbody = document.getElementById("tasks-tbody");
    const taskCount = document.getElementById("task-count");

    tbody.innerHTML = "";
    taskCount.textContent = `${totalTasks} task(s)`;

    if (tasks.length === 0) {
      contentEl.classList.add("hidden");
      emptyEl.classList.remove("hidden");
    } else {
      contentEl.classList.remove("hidden");
      emptyEl.classList.add("hidden");
      tasks.forEach((task) => {
        tbody.innerHTML += renderTaskRow(task);
      });
    }

    renderPagination();
    updatePolling(tasks);
  } catch (err) {
    loadingEl.style.display = "none";
    contentEl.classList.add("hidden");
    emptyEl.classList.remove("hidden");
    document.querySelector("#empty-state p").textContent = "Failed to load tasks.";
  }
}

function renderPagination() {
  const paginator = document.getElementById("paginator");
  const totalPages = Math.max(1, Math.ceil(totalTasks / PAGE_SIZE));

  paginator.innerHTML = `
        <button id="prev-page" ${currentPage <= 1 ? "disabled" : ""} onclick="goToPage(${currentPage - 1})">
            <i class="fas fa-chevron-left"></i> Previous
        </button>
        <span>Page ${currentPage} of ${totalPages} (${totalTasks} tasks)</span>
        <button id="next-page" ${currentPage >= totalPages ? "disabled" : ""} onclick="goToPage(${currentPage + 1})">
            Next <i class="fas fa-chevron-right"></i>
        </button>
    `;
}

function goToPage(page) {
  currentPage = page;
  loadTasks();
}

async function cancelTask(taskId) {
  if (!confirm("Cancel this task?")) return;

  try {
    const response = await apiFetch(`${API_BASE}/tasks/${taskId}`, {
      method: "DELETE",
    });
    loadTasks();
  } catch (err) {
    showError("Failed to cancel task: " + err.message);
  }
}

function updatePolling(tasks) {
  const hasActive = tasks.some((t) => t.status === "Running" || t.status === "Pending");

  if (hasActive && !pollInterval) {
    pollInterval = setInterval(() => {
      loadTasks();
    }, 2000);
  } else if (!hasActive && pollInterval) {
    clearInterval(pollInterval);
    pollInterval = null;
  }
}

function showLogs(taskId) {
  // Fetch task detail to get logs
  apiFetch(`${API_BASE}/tasks/${taskId}`)
    .then((response) => {
      const task = response.data || response;
      const logs = task.logs || [];
      const content = document.getElementById("logs-content");
      if (logs.length === 0) {
        content.innerHTML = '<div style="color:#64748b;">No logs available.</div>';
      } else {
        content.innerHTML = logs.map((log) => `<div>${escapeHtml(log)}</div>`).join("");
      }
      document.getElementById("logs-popup").classList.add("open");
      document.getElementById("overlay-backdrop").classList.add("open");
    })
    .catch((err) => {
      showError("Failed to fetch task logs: " + err.message);
    });
}

function closeLogs() {
  document.getElementById("logs-popup").classList.remove("open");
  document.getElementById("overlay-backdrop").classList.remove("open");
}

function escapeHtml(text) {
  const div = document.createElement("div");
  div.textContent = text;
  return div.innerHTML;
}

// Initialize on page load
document.addEventListener("DOMContentLoaded", () => {
  loadTasks();
  // Clean up polling on page unload
  window.addEventListener("beforeunload", () => {
    if (pollInterval) {
      clearInterval(pollInterval);
    }
  });
});
