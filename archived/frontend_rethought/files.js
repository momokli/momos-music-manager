import { API_BASE, fetchJSON } from "./shared/api.js";
import {
  useErrorBanner,
  renderLoading,
  renderEmpty,
  renderErrorBlock,
  renderTable,
  td,
  Pagination,
  initSearchBar,
} from "./shared/components.js";
import { formatDuration, formatBPM } from "./shared/format.js";
import { renderNav } from "./shared/nav.js";

renderNav("files");

const ITEMS_PER_PAGE = 50;

let currentSearch = "";
let currentPage = 0;
let isLoading = false;
let loadToken = 0;

// Task polling state
let activeTaskId = null;
let activeTaskInterval = null;

// DOM refs
const filesContent = document.getElementById("files-content");
const errorMessage = document.getElementById("error-message");
const searchInput = document.getElementById("search-input");
const searchBtn = document.getElementById("search-btn");
const refreshBtn = document.getElementById("refresh-btn");
const writeAllBtn = document.getElementById("write-all-btn");
const taskStatusDiv = document.getElementById("task-status");
const taskStatusText = document.getElementById("task-status-text");
const taskStatusProgress = document.getElementById("task-status-progress");
const checkTaskBtn = document.getElementById("check-task-btn");

const errorBanner = useErrorBanner(errorMessage);

// Search bar (shared component handles ENTER, button click, Cmd+F, Escape)
const searchBar = initSearchBar({
  onSearch: (term) => {
    currentSearch = term;
    loadFiles(0, term);
  },
});

// Pagination
const pagination = new Pagination({
  itemsPerPage: ITEMS_PER_PAGE,
  onPageChange: (page) => loadFiles(page, currentSearch),
  showFirstLast: true,
  bindings: {
    prev: "prev-page-btn",
    next: "next-page-btn",
    first: "first-page-btn",
    last: "last-page-btn",
    info: "page-info",
    total: "total-files",
    showing: "showing-files",
  },
});

// Render helpers

function escapeHtml(str) {
  if (typeof str !== "string") return str;
  return str
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

function renderCommentStatus(file) {
  const current = file.comment || "";
  const target = file.commentTarget || "";
  const needsUpdate = file.commentNeedsUpdate || false;

  if (!current && !target) {
    return `<span style="color: var(--text-subtle);">—</span>`;
  }

  if (!needsUpdate) {
    return `
      <div style="display: flex; align-items: center; gap: 8px;">
        <span style="color: var(--green); font-size: 0.9rem;">✅</span>
        <span style="color: var(--text-secondary); font-size: 0.85rem; word-break: break-word;">${escapeHtml(current)}</span>
      </div>
    `;
  }

  return `
    <div style="font-size: 0.85rem;">
      <div style="color: var(--red); text-decoration: line-through; margin-bottom: 4px; word-break: break-word;">
        ${escapeHtml(current || "(empty)")}
      </div>
      <div style="color: var(--green); word-break: break-word;">
        → ${escapeHtml(target)}
      </div>
    </div>
  `;
}

function renderStars(rating) {
  if (rating == null) return "";
  const full = Math.min(5, Math.max(0, Math.round(rating)));
  return `<span style="color: var(--yellow);">${String.fromCodePoint(9733).repeat(full)}${String.fromCodePoint(9734).repeat(5 - full)}</span>`;
}

function getFileType(file) {
  let ft = file.fileType || "FILE";
  if ((!ft || ft === "file") && file.filePath) {
    const parts = file.filePath.split(".");
    const ext = parts.pop().toLowerCase();
    if (["mp3", "flac", "m4a", "wav", "opus", "stem.m4a"].includes(ext)) {
      ft = ext.toUpperCase();
    }
  }
  return ft;
}

function renderServiceBadges(file) {
  const services = file.matchedServices || [];
  if (!services.length) {
    return `<span style="color: var(--text-subtle);">—</span>`;
  }
  const iconMap = {
    spotify: '<i class="fab fa-spotify"></i>',
    soundcloud: '<i class="fab fa-soundcloud"></i>',
    youtube: '<i class="fab fa-youtube"></i>',
  };
  return services
    .map((s) => {
      const cls = s.toLowerCase();
      return `<span class="service-badge ${cls}">${iconMap[s] || ""} ${s.charAt(0).toUpperCase() + s.slice(1)}</span>`;
    })
    .join(" ");
}

function renderActions(file) {
  if (activeTaskId) {
    return `<div class="spinner" style="width: 16px; height: 16px; border-width: 2px; margin: 0 auto;"></div>`;
  }
  if (!file.commentNeedsUpdate) {
    return `<span style="color: var(--text-subtle);">—</span>`;
  }
  return `<button class="btn btn-sm btn-green write-single-btn" data-file-id="${file.id}"><i class="fas fa-pen"></i> Write</button>`;
}

// Load & render

async function loadFiles(page, search) {
  if (page === undefined) page = currentPage;
  if (search === undefined) search = currentSearch;
  const token = ++loadToken;
  isLoading = true;
  pagination.setLoading(true);
  filesContent.innerHTML = renderLoading("Loading files...");

  try {
    const offset = page * ITEMS_PER_PAGE;
    let url = `/files?limit=${ITEMS_PER_PAGE}&offset=${offset}`;
    let countUrl = `/files/count`;

    if (search && search.trim() !== "") {
      const encoded = encodeURIComponent(search.trim());
      url += `&search=${encoded}`;
      countUrl += `?search=${encoded}`;
    }

    const [filesData, countData] = await Promise.all([
      fetchJSON(url),
      fetchJSON(countUrl),
    ]);

    const files = filesData?.data || [];
    const total = countData?.data ?? 0;

    if (token !== loadToken) return;

    currentPage = page;
    currentSearch = search;

    renderFiles(files);
    pagination.update(total, files.length);
  } catch (err) {
    console.error("Failed to load files:", err);
    filesContent.innerHTML = renderErrorBlock({
      title: "Failed to load files",
      detail: err.message,
      retryFn: "window.location.reload()",
    });
    errorBanner.showError(`Failed to load files: ${err.message}`);
  } finally {
    isLoading = false;
    pagination.setLoading(false);
  }
}

function renderFiles(files) {
  if (!files || files.length === 0) {
    filesContent.innerHTML = renderEmpty({
      icon: "music",
      title: "No files found",
      message: currentSearch
        ? "Try adjusting your search terms"
        : "Try scanning folders first by adding music directories",
      actionHtml:
        '<a href="folders.html" class="btn btn-purple"><i class="fas fa-folder"></i> Manage Folders</a>',
    });
    if (writeAllBtn) writeAllBtn.style.display = "none";
    return;
  }

  const headers = [
    "Title",
    "Artist",
    "BPM",
    "Key",
    "Type",
    "Services",
    "Comment",
    "Duration",
    "Rating",
    "Actions",
  ];

  const rowsHtml = files
    .map((file) => {
      const fileType = getFileType(file);
      const bpmStr = file.bpm ? formatBPM(file.bpm) : "—";
      const keyStr = file.musicalKey || file.key || "—";
      const durationStr = file.durationMs ? formatDuration(file.durationMs / 1000) : "—";
      const ratingHtml = renderStars(file.rating);
      const servicesHtml = renderServiceBadges(file);
      const commentHtml = renderCommentStatus(file);
      const actionsHtml = renderActions(file);

      let rowStyle = "border-bottom: 1px solid var(--border);";
      if (file.rating >= 4) {
        rowStyle += " background: rgba(251, 191, 36, 0.05);";
      } else if (file.rating >= 2) {
        rowStyle += " background: rgba(99, 102, 241, 0.05);";
      }

      const titleHtml = `
      <div style="font-weight: 500; color: var(--text);">${escapeHtml(file.title || "Untitled")}</div>
      ${file.isrc ? `<div style="font-size: 0.75rem; color: var(--text-muted); margin-top: 2px;">ISRC: ${escapeHtml(file.isrc)}</div>` : ""}
    `;

      const bpmCell = file.bpm
        ? `<span style="color: var(--accent); font-weight: 600;">${bpmStr}</span>`
        : `<span style="color: var(--text-subtle);">—</span>`;

      const typeHtml = `<span class="badge" style="background: var(--border); color: var(--text-secondary);">${escapeHtml(fileType)}</span>`;

      return `<tr style="${rowStyle}">
      ${td(titleHtml)}
      ${td(escapeHtml(file.artist || "Unknown"))}
      ${td(bpmCell, { style: "text-align: center;" })}
      ${td(`<span style="font-weight: 600;">${keyStr}</span>`, { style: "text-align: center;" })}
      ${td(typeHtml, { style: "text-align: center;" })}
      ${td(servicesHtml, { style: "text-align: center;" })}
      ${td(commentHtml)}
      ${td(durationStr, { style: "text-align: center;" })}
      ${td(ratingHtml || `<span style="color: var(--text-subtle);">—</span>`, { style: "text-align: center;" })}
      ${td(actionsHtml, { style: "text-align: center;" })}
    </tr>`;
    })
    .join("");

  filesContent.innerHTML = renderTable(headers, rowsHtml);

  const hasNeedsUpdate = files.some((f) => f.commentNeedsUpdate);
  if (writeAllBtn) {
    writeAllBtn.style.display = hasNeedsUpdate ? "inline-flex" : "none";
  }
}

// Comment writing
async function writeSingleComment(fileId) {
  try {
    const result = await fetchJSON(`/files/${fileId}/write-comment`, { method: "POST" });
    if (result?.data?.taskId) {
      startTaskPolling(result.data.taskId);
    } else {
      errorBanner.showError("Failed to start write task: no task ID returned");
    }
  } catch (err) {
    errorBanner.showError(`Failed to start write: ${err.message}`);
  }
}

async function writeAllComments() {
  if (!writeAllBtn) return;
  writeAllBtn.disabled = true;
  writeAllBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Starting...';

  try {
    const result = await fetchJSON("/files/write-comments", { method: "POST" });
    if (result?.data?.taskId) {
      startTaskPolling(result.data.taskId);
    } else if (result?.data?.message) {
      errorBanner.showError(result.data.message);
      writeAllBtn.style.display = "none";
    } else {
      errorBanner.showError("Failed to start batch write: no task ID returned");
    }
  } catch (err) {
    errorBanner.showError(`Failed to start batch write: ${err.message}`);
  } finally {
    writeAllBtn.disabled = false;
    writeAllBtn.innerHTML = '<i class="fas fa-pen"></i> Write All Comments';
  }
}

// Task polling
function startTaskPolling(taskId) {
  activeTaskId = taskId;
  if (taskStatusDiv) taskStatusDiv.classList.add("visible");
  if (taskStatusText) taskStatusText.textContent = "Writing comments...";
  if (taskStatusProgress) taskStatusProgress.textContent = "Starting...";
  if (writeAllBtn) {
    writeAllBtn.disabled = true;
    writeAllBtn.style.display = "none";
  }
  checkTaskStatus();
  if (activeTaskInterval) clearInterval(activeTaskInterval);
  activeTaskInterval = setInterval(checkTaskStatus, 2000);
}

async function checkTaskStatus() {
  if (!activeTaskId) return;
  try {
    const result = await fetchJSON(`/tasks/${activeTaskId}`);
    if (!result?.data) return;
    const task = result.data;
    if (taskStatusProgress) {
      taskStatusProgress.textContent = task.progress || "";
    }
    if (task.logs?.length > 0 && taskStatusText) {
      taskStatusText.textContent = task.logs[task.logs.length - 1];
    }
    if (
      task.status === "Completed" ||
      task.status === "Failed" ||
      task.status === "Cancelled"
    ) {
      stopTaskPolling(task.status);
    }
  } catch (err) {
    console.error("Task status check failed:", err);
  }
}

function stopTaskPolling(status) {
  if (activeTaskInterval) {
    clearInterval(activeTaskInterval);
    activeTaskInterval = null;
  }
  activeTaskId = null;
  if (taskStatusDiv) {
    var icon;
    if (status === "Completed") icon = "\u2705";
    else if (status === "Failed") icon = "\u274c";
    else icon = "\u26a0\ufe0f";
    if (taskStatusText) taskStatusText.textContent = icon + " Task " + status;
    setTimeout(function () {
      if (taskStatusDiv) taskStatusDiv.classList.remove("visible");
    }, 3000);
  }
  if (writeAllBtn) writeAllBtn.disabled = false;
  loadFiles();
}

// Event delegation for write buttons
document.getElementById("files-content").addEventListener("click", function (e) {
  var btn = e.target.closest(".write-single-btn");
  if (btn) {
    var fileId = parseInt(btn.getAttribute("data-file-id"), 10);
    if (fileId) writeSingleComment(fileId);
  }
});

// Keyboard shortcuts (page-specific: pagination arrows)
document.addEventListener("keydown", function (event) {
  if (event.key === "ArrowLeft" && !isLoading) {
    var btn = document.getElementById("prev-page-btn");
    if (btn && !btn.disabled) btn.click();
  }
  if (event.key === "ArrowRight" && !isLoading) {
    var btn = document.getElementById("next-page-btn");
    if (btn && !btn.disabled) btn.click();
  }
});

// Init
document.addEventListener("DOMContentLoaded", async function () {
  // Wire up toolbar
  if (refreshBtn)
    refreshBtn.addEventListener("click", function () {
      loadFiles(currentPage, currentSearch);
    });
  if (writeAllBtn) writeAllBtn.addEventListener("click", writeAllComments);
  if (checkTaskBtn) checkTaskBtn.addEventListener("click", checkTaskStatus);

  // Health check
  try {
    var health = await fetchJSON("/health");
    console.log("Backend connected:", health);
  } catch (err) {
    console.error("Backend not responding:", err);
    errorBanner.showError("Backend is not responding. Make sure the server is running.");
    filesContent.innerHTML = renderEmpty({
      icon: "exclamation-triangle",
      title: "Backend Unavailable",
      message: "Could not connect to the backend server. Please ensure it is running.",
      actionHtml:
        '<button class="btn btn-primary" onclick="window.location.reload()"><i class="fas fa-redo"></i> Retry</button>',
    });
    return;
  }

  // Load initial data
  loadFiles(0, "");
});
