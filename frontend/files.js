const API_BASE = "http://localhost:3000/api";
const FILES_PER_PAGE = 50;

let currentPage = 0;
let totalFiles = 0;
let currentSearch = "";
// Only local files (use fileType field from API)
let isLoading = false;

// DOM elements
const filesContent = document.getElementById("files-content");
const errorMessage = document.getElementById("error-message");
const totalFilesElement = document.getElementById("total-files");
const currentPageElement = document.getElementById("current-page");
const showingFilesElement = document.getElementById("showing-files");
const searchInput = document.getElementById("search-input");
const searchBtn = document.getElementById("search-btn");
const firstPageBtn = document.getElementById("first-page-btn");
const prevPageBtn = document.getElementById("prev-page-btn");
const nextPageBtn = document.getElementById("next-page-btn");
const lastPageBtn = document.getElementById("last-page-btn");
const pageInfo = document.getElementById("page-info");
const writeAllBtn = document.getElementById("write-all-btn");
const taskStatusDiv = document.getElementById("task-status");
const taskStatusText = document.getElementById("task-status-text");
const taskStatusProgress = document.getElementById("task-status-progress");

// Task polling state
let activeTaskId = null;
let activeTaskInterval = null;

// Format milliseconds to minutes:seconds
function formatDuration(ms) {
  if (!ms) return "";
  const totalSeconds = Math.floor(ms / 1000);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `${minutes}:${seconds.toString().padStart(2, "0")}`;
}

// Format BPM with one decimal place if needed
function formatBPM(bpm) {
  if (!bpm) return "";
  return bpm % 1 === 0 ? bpm.toString() : bpm.toFixed(1);
}

// Render comment status with diff display
function renderCommentStatus(file) {
  const current = file.comment || "";
  const target = file.commentTarget || "";
  const needsUpdate = file.commentNeedsUpdate || false;

  if (!current && !target) {
    return `<span style="color: #475569;">—</span>`;
  }

  if (!needsUpdate) {
    // Up to date - show current comment with checkmark
    return `
      <div style="display: flex; align-items: center; gap: 8px;">
        <span style="color: #10b981; font-size: 0.9rem;">✅</span>
        <span style="color: #cbd5e1; font-size: 0.85rem; word-break: break-word;">${current}</span>
      </div>
    `;
  }

  // Needs update - show diff
  return `
    <div style="font-size: 0.85rem;">
      <div style="color: #ef4444; text-decoration: line-through; margin-bottom: 4px; word-break: break-word;">
        ${current || "(empty)"}
      </div>
      <div style="color: #10b981; word-break: break-word;">
        → ${target}
      </div>
    </div>
  `;
}

// Show error message
function showError(message) {
  errorMessage.textContent = message;
  errorMessage.style.display = "block";
  setTimeout(() => {
    errorMessage.style.display = "none";
  }, 5000);
}

// Fetch JSON from API
async function fetchJSON(url, options = {}) {
  try {
    const response = await fetch(url, {
      headers: {
        "Content-Type": "application/json",
      },
      ...options,
    });

    if (!response.ok) {
      throw new Error(`HTTP ${response.status}: ${response.statusText}`);
    }

    return await response.json();
  } catch (error) {
    console.error("Fetch error:", error);
    throw error;
  }
}

// Load files from API
async function loadFiles(page = currentPage, search = currentSearch) {
  if (isLoading) return;

  isLoading = true;
  filesContent.innerHTML = `
        <div class="loading">
            <div class="loading-spinner"></div>
            <p>Loading files...</p>
        </div>
    `;

  // Update button states
  if (searchBtn) searchBtn.disabled = true;
  if (firstPageBtn) firstPageBtn.disabled = true;
  if (prevPageBtn) prevPageBtn.disabled = true;
  if (nextPageBtn) nextPageBtn.disabled = true;
  if (lastPageBtn) lastPageBtn.disabled = true;

  try {
    const offset = page * FILES_PER_PAGE;
    let url = `${API_BASE}/files?limit=${FILES_PER_PAGE}&offset=${offset}`;

    if (search && search.trim() !== "") {
      url += `&search=${encodeURIComponent(search.trim())}`;
    }

    // Also get total count
    let countUrl = `${API_BASE}/files/count`;
    if (search && search.trim() !== "") {
      countUrl += `&search=${encodeURIComponent(search.trim())}`;
    }

    const [filesData, countData] = await Promise.all([
      fetchJSON(url),
      fetchJSON(countUrl),
    ]);

    if (filesData && filesData.data && countData && countData.data !== undefined) {
      currentPage = page;
      currentSearch = search;
      totalFiles = countData.data;

      renderFiles(filesData.data);
      updatePagination();
      updateStats(filesData.data.length, offset);
    } else {
      throw new Error("Invalid response format");
    }
  } catch (error) {
    console.error("Failed to load files:", error);
    filesContent.innerHTML = `
            <div style="text-align: center; padding: 40px;">
                <p><i class="fas fa-exclamation-triangle" style="font-size: 3rem; color: #dc2626; margin-bottom: 20px;"></i></p>
                <p style="font-size: 1.2rem; margin-bottom: 10px; color: #fecaca;">Failed to load files</p>
                <p style="color: #94a3b8; margin-bottom: 20px;">${error.message}</p>
                <button onclick="loadFiles()"
                        style="background: #3b82f6; padding: 12px 24px; border-radius: 8px; color: white; border: none; cursor: pointer; font-weight: 600;">
                    <i class="fas fa-redo"></i> Try Again
                </button>
            </div>
        `;
    showError(`Failed to load files: ${error.message}`);
  } finally {
    isLoading = false;
    if (searchBtn) searchBtn.disabled = false;
    updatePagination();
  }
}

// Render files table
function renderFiles(files) {
  // DEBUG: Check first file's musicalKey
  if (files && files.length > 0) {
    const firstFile = files[0];
    console.log(
      "DEBUG: First file musicalKey:",
      firstFile.musicalKey,
      "Type:",
      typeof firstFile.musicalKey,
    );
    console.log("DEBUG: First file object keys:", Object.keys(firstFile));
  }

  if (!files || files.length === 0) {
    filesContent.innerHTML = `
            <div style="text-align: center; padding: 40px;">
                <p><i class="fas fa-music" style="font-size: 3rem; color: #475569; margin-bottom: 20px;"></i></p>
                <p style="font-size: 1.2rem; margin-bottom: 10px; color: #cbd5e1;">No files found</p>
                <p style="color: #94a3b8; margin-bottom: 20px;">${currentSearch ? "Try adjusting your search" : "Try scanning folders first"}</p>
                <button onclick="window.location.href='folders.html'"
                        style="background: #8b5cf6; padding: 12px 24px; border-radius: 8px; color: white; border: none; cursor: pointer; font-weight: 600;">
                    <i class="fas fa-folder"></i> Manage Folders
                </button>
            </div>
        `;
    return;
  }

  let html = `
        <div style="background: #1a1a2e; border-radius: 12px; overflow: hidden; border: 1px solid #334155;">
            <table style="width: 100%; border-collapse: collapse;">
                <thead style="background: #0f172a;">
                    <tr>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 18%;">Title</th>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 15%;">Artist</th>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 6%;">BPM</th>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 6%;">Key</th>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 6%;">Type</th>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 12%;">Services</th>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 15%;">Comment</th>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 8%;">Duration</th>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 10%;">Rating</th>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 8%;">Actions</th>
                    </tr>
                </thead>
                <tbody>
    `;

  files.forEach((file) => {
    // Extract file type from fileType or filePath
    let fileType = file.fileType || "file";
    if (!fileType || fileType === "file") {
      // Fallback to extracting from filePath
      if (file.filePath && file.filePath.includes(".")) {
        const ext = file.filePath.split(".").pop().toLowerCase();
        if (["mp3", "flac", "m4a", "wav", "opus", "stem.m4a"].includes(ext)) {
          fileType = ext.toUpperCase();
        }
      }
    }

    // Format rating as stars
    let ratingHtml = "";
    if (file.rating !== undefined && file.rating !== null) {
      const rating = parseInt(file.rating);
      ratingHtml = `<div style="color: #fbbf24;">${"★".repeat(rating)}${"☆".repeat(5 - rating)}</div>`;
    }

    // Determine row color based on rating
    const rowColor =
      file.rating >= 4
        ? "rgba(251, 191, 36, 0.05)"
        : file.rating >= 2
          ? "rgba(59, 130, 246, 0.05)"
          : "";

    // Render matched services badges
    let servicesHtml = "";
    if (file.matchedServices && file.matchedServices.length > 0) {
      const serviceColors = {
        spotify: "#1db954",
        soundcloud: "#ff7700",
        youtube: "#ff0000",
      };
      const serviceIcons = {
        spotify: '<i class="fab fa-spotify"></i>',
        soundcloud: '<i class="fab fa-soundcloud"></i>',
        youtube: '<i class="fab fa-youtube"></i>',
      };
      servicesHtml = file.matchedServices
        .map(
          (s) =>
            `<span style="display: inline-block; background: ${serviceColors[s] || "#334155"}; padding: 4px 8px; border-radius: 4px; font-size: 0.8rem; font-weight: 600; color: white; margin: 2px;">${serviceIcons[s] || ""} ${s.charAt(0).toUpperCase() + s.slice(1)}</span>`,
        )
        .join(" ");
    } else {
      servicesHtml = `<span style="color: #475569;">—</span>`;
    }

    html += `
            <tr style="border-bottom: 1px solid #334155; transition: background 0.2s; background: ${rowColor};">
                <td style="padding: 16px; color: #cbd5e1;">
                    <div style="font-weight: 500; color: white;">${file.title || "Untitled"}</div>
                    ${file.isrc ? `<div style="font-size: 0.75rem; color: #94a3b8; margin-top: 2px;">ISRC: ${file.isrc}</div>` : ""}
                </td>
                <td style="padding: 16px; color: #cbd5e1;">${file.artist || "Unknown"}</td>
                <td style="padding: 16px; color: #cbd5e1; text-align: center; font-weight: 600; color: ${file.bpm ? "#3b82f6" : "#94a3b8"};">${formatBPM(file.bpm) || "-"}</td>
                <td style="padding: 16px; color: #cbd5e1; text-align: center; font-weight: 600;">${file.musicalKey || "-"}</td>
                <td style="padding: 16px; color: #cbd5e1; text-align: center;">
                    <span style="background: #334155; padding: 4px 8px; border-radius: 4px; font-size: 0.8rem; font-weight: 600;">${fileType}</span>
                </td>
                <td style="padding: 16px; color: #cbd5e1; text-align: center;">
                    <div style="display: flex; gap: 4px; justify-content: center; flex-wrap: wrap;">${servicesHtml}</div>
                </td>
                <td style="padding: 16px; color: #cbd5e1;">
                    ${renderCommentStatus(file)}
                </td>
                <td style="padding: 16px; color: #cbd5e1; text-align: center;">${formatDuration(file.durationMs) || "-"}</td>
                <td style="padding: 16px; color: #cbd5e1; text-align: center;">${ratingHtml || "-"}</td>
                <td style="padding: 16px; text-align: center;">
                    ${renderActions(file)}
                </td>
            </tr>
        `;
  });

  html += `
                </tbody>
            </table>
        </div>
    `;

  filesContent.innerHTML = html;

  // Show/hide Write All button based on whether any file needs update
  const hasNeedsUpdate = files.some((f) => f.commentNeedsUpdate);
  if (writeAllBtn) {
    writeAllBtn.style.display = hasNeedsUpdate ? "inline-flex" : "none";
  }
}

// Render Actions column content
function renderActions(file) {
  if (activeTaskId) {
    // A task is running – show spinner for all files
    return `<div class="loading-spinner" style="width: 16px; height: 16px; border-width: 2px; margin: 0 auto;"></div>`;
  }

  if (!file.commentNeedsUpdate) {
    return `<span style="color: #475569;">—</span>`;
  }

  return `
    <button onclick="writeSingleComment(${file.id})"
            style="background: #10b981; padding: 6px 12px; border-radius: 6px; color: white; border: none; cursor: pointer; font-size: 0.8rem; font-weight: 600; white-space: nowrap;">
      <i class="fas fa-pen"></i> Write
    </button>
  `;
}

// ── Write comment actions ──────────────────────────────────────────────────

// Write comment for a single file
async function writeSingleComment(fileId) {
  try {
    const result = await fetchJSON(`${API_BASE}/files/${fileId}/write-comment`, {
      method: "POST",
    });

    if (result && result.data && result.data.taskId) {
      startTaskPolling(result.data.taskId);
    } else {
      showError("Failed to start write task: no task ID returned");
    }
  } catch (error) {
    showError(`Failed to start write: ${error.message}`);
  }
}

// Write comments for all files that need update
async function writeAllComments() {
  if (!writeAllBtn) return;

  writeAllBtn.disabled = true;
  writeAllBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Starting...';

  try {
    const result = await fetchJSON(`${API_BASE}/files/write-comments`, {
      method: "POST",
    });

    if (result && result.data && result.data.taskId) {
      startTaskPolling(result.data.taskId);
    } else if (result && result.data && result.data.message) {
      // All up to date
      showError(result.data.message);
      writeAllBtn.style.display = "none";
    } else {
      showError("Failed to start batch write: no task ID returned");
    }
  } catch (error) {
    showError(`Failed to start batch write: ${error.message}`);
  } finally {
    writeAllBtn.disabled = false;
    writeAllBtn.innerHTML = '<i class="fas fa-pen"></i> Write All Comments';
  }
}

// ── Task polling ────────────────────────────────────────────────────────────

// Start polling for task status
function startTaskPolling(taskId) {
  activeTaskId = taskId;

  // Show task status indicator
  if (taskStatusDiv) taskStatusDiv.style.display = "block";
  if (taskStatusText) taskStatusText.textContent = "Writing comments...";
  if (taskStatusProgress) taskStatusProgress.textContent = "Starting...";

  // Disable Write All button
  if (writeAllBtn) {
    writeAllBtn.disabled = true;
    writeAllBtn.style.display = "none";
  }

  // Poll immediately, then every 2 seconds
  checkTaskStatus();
  if (activeTaskInterval) clearInterval(activeTaskInterval);
  activeTaskInterval = setInterval(checkTaskStatus, 2000);
}

// Check the current task status
async function checkTaskStatus() {
  if (!activeTaskId) return;

  try {
    const result = await fetchJSON(`${API_BASE}/tasks/${activeTaskId}`);
    if (!result || !result.data) {
      return;
    }

    const task = result.data;

    // Update progress display
    if (taskStatusProgress) {
      taskStatusProgress.textContent = task.progress || "";
    }

    // Logs show latest message
    if (task.logs && task.logs.length > 0 && taskStatusText) {
      const latestLog = task.logs[task.logs.length - 1];
      taskStatusText.textContent = latestLog;
    }

    // Check if task completed
    if (
      task.status === "Completed" ||
      task.status === "Failed" ||
      task.status === "Cancelled"
    ) {
      stopTaskPolling(task.status);
    }
  } catch (error) {
    console.error("Task status check failed:", error);
  }
}

// Stop polling and refresh
function stopTaskPolling(status) {
  if (activeTaskInterval) {
    clearInterval(activeTaskInterval);
    activeTaskInterval = null;
  }

  const taskId = activeTaskId;
  activeTaskId = null;

  // Hide task status indicator after a short delay
  if (taskStatusDiv) {
    // Update with final status
    if (taskStatusText) {
      const icon = status === "Completed" ? "✅" : status === "Failed" ? "❌" : "🚫";
      taskStatusText.textContent = `${icon} Task ${status}`;
    }
    setTimeout(() => {
      if (taskStatusDiv) taskStatusDiv.style.display = "none";
    }, 3000);
  }

  // Re-enable Write All button
  if (writeAllBtn) {
    writeAllBtn.disabled = false;
  }

  // Refresh the files list to update comment statuses
  loadFiles();
}

// Update pagination controls
function updatePagination() {
  const totalPages = Math.ceil(totalFiles / FILES_PER_PAGE);

  // Update page info
  if (pageInfo) {
    pageInfo.textContent = `Page ${currentPage + 1} of ${totalPages || 1}`;
  }

  // Update button states
  if (firstPageBtn) {
    firstPageBtn.disabled = currentPage === 0 || totalFiles === 0 || isLoading;
    firstPageBtn.style.opacity = firstPageBtn.disabled ? "0.5" : "1";
    firstPageBtn.style.cursor = firstPageBtn.disabled ? "not-allowed" : "pointer";
  }

  if (prevPageBtn) {
    prevPageBtn.disabled = currentPage === 0 || totalFiles === 0 || isLoading;
    prevPageBtn.style.opacity = prevPageBtn.disabled ? "0.5" : "1";
    prevPageBtn.style.cursor = prevPageBtn.disabled ? "not-allowed" : "pointer";
  }

  if (nextPageBtn) {
    nextPageBtn.disabled = currentPage >= totalPages - 1 || totalFiles === 0 || isLoading;
    nextPageBtn.style.opacity = nextPageBtn.disabled ? "0.5" : "1";
    nextPageBtn.style.cursor = nextPageBtn.disabled ? "not-allowed" : "pointer";
  }

  if (lastPageBtn) {
    lastPageBtn.disabled = currentPage >= totalPages - 1 || totalFiles === 0 || isLoading;
    lastPageBtn.style.opacity = lastPageBtn.disabled ? "0.5" : "1";
    lastPageBtn.style.cursor = lastPageBtn.disabled ? "not-allowed" : "pointer";
  }
}

// Update stats display
function updateStats(showingCount, offset) {
  if (totalFilesElement) {
    totalFilesElement.textContent = `Total: ${totalFiles.toLocaleString()}`;
  }

  if (currentPageElement) {
    currentPageElement.textContent = `Page: ${currentPage + 1}`;
  }

  if (showingFilesElement) {
    const start = totalFiles > 0 ? offset + 1 : 0;
    const end = Math.min(offset + showingCount, totalFiles);
    showingFilesElement.textContent = `Showing: ${start}-${end}`;
  }
}

// Search files
function searchFiles() {
  const searchTerm = searchInput.value.trim();
  loadFiles(0, searchTerm);
}

// Pagination functions
function previousPage() {
  if (currentPage > 0 && !isLoading) {
    loadFiles(currentPage - 1, currentSearch);
  }
}

function nextPage() {
  const totalPages = Math.ceil(totalFiles / FILES_PER_PAGE);
  if (currentPage < totalPages - 1 && !isLoading) {
    loadFiles(currentPage + 1, currentSearch);
  }
}

function goToPage(page) {
  if (!isLoading) {
    loadFiles(page, currentSearch);
  }
}

function goToLastPage() {
  const totalPages = Math.ceil(totalFiles / FILES_PER_PAGE);
  if (totalPages > 0 && !isLoading) {
    loadFiles(totalPages - 1, currentSearch);
  }
}

// Initialize on page load
document.addEventListener("DOMContentLoaded", () => {
  // Check backend connection first
  fetchJSON(`${API_BASE}/health`)
    .then((data) => {
      console.log("Backend connected:", data);
      loadFiles();
    })
    .catch((error) => {
      console.error("Backend connection failed:", error);
      showError("Backend is not responding. Make sure the server is running.");
      filesContent.innerHTML = `
                <div style="text-align: center; padding: 40px;">
                    <p><i class="fas fa-exclamation-triangle" style="font-size: 3rem; color: #dc2626; margin-bottom: 20px;"></i></p>
                    <p style="font-size: 1.2rem; margin-bottom: 10px; color: #fecaca;">Backend not responding</p>
                    <p style="color: #94a3b8; margin-bottom: 20px;">Make sure the server is running.</p>
                    <button onclick="location.reload()"
                            style="background: #3b82f6; padding: 12px 24px; border-radius: 8px; color: white; border: none; cursor: pointer; font-weight: 600;">
                        <i class="fas fa-redo"></i> Retry Connection
                    </button>
                </div>
            `;
    });

  // Set up search input
  if (searchInput) {
    searchInput.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        searchFiles();
      }
    });
  }
});

// Keyboard shortcuts
document.addEventListener("keydown", (event) => {
  // Ctrl+F or Cmd+F to focus search
  if ((event.ctrlKey || event.metaKey) && event.key === "f") {
    event.preventDefault();
    if (searchInput) {
      searchInput.focus();
    }
  }

  // Escape to clear search
  if (event.key === "Escape" && document.activeElement === searchInput && searchInput) {
    searchInput.value = "";
    loadFiles(0, "");
  }

  // Arrow keys for pagination
  if (event.key === "ArrowLeft" && !isLoading) {
    previousPage();
  }
  if (event.key === "ArrowRight" && !isLoading) {
    nextPage();
  }
});
