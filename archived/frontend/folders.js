const API_BASE = "http://localhost:3000/api";

// DOM elements
const foldersContent = document.getElementById("folders-content");
const errorMessage = document.getElementById("error-message");
const totalFoldersElement = document.getElementById("total-folders");
const currentPageElement = document.getElementById("current-page");
const showingFoldersElement = document.getElementById("showing-folders");
const searchInput = document.getElementById("search-input");
const searchBtn = document.getElementById("search-btn");
const folderModal = document.getElementById("folder-modal");
const modalTitle = document.getElementById("modal-title");
const folderPathInput = document.getElementById("folder-path-input");
const scanRecursiveCheckbox = document.getElementById("scan-recursive-checkbox");
const fixedExtensionsCheckbox = document.getElementById("fixed-extensions-checkbox");
const extensionsContainer = document.getElementById("extensions-container");
const fileExtensionsInput = document.getElementById("file-extensions-input");

let currentFolderId = null; // For editing
let isLoading = false;

// Format date from timestamp
function formatDate(timestamp) {
  if (!timestamp) return "Never";
  const date = new Date(timestamp * 1000);
  return (
    date.toLocaleDateString() +
    " " +
    date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
  );
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

// Load folders from API
async function loadFolders(search = "") {
  if (isLoading) return;

  isLoading = true;
  foldersContent.innerHTML = `
        <div class="loading">
            <div class="loading-spinner"></div>
            <p>Loading folders...</p>
        </div>
    `;

  // Update button states
  if (searchBtn) searchBtn.disabled = true;

  try {
    let url = `${API_BASE}/folders`;
    // Note: Search filtering not implemented in backend yet

    const data = await fetchJSON(url);

    if (data && data.data) {
      renderFolders(data.data);
      updateStats(data.data.length);
    } else {
      throw new Error("Invalid response format");
    }
  } catch (error) {
    console.error("Failed to load folders:", error);
    foldersContent.innerHTML = `
            <div style="text-align: center; padding: 40px;">
                <p><i class="fas fa-exclamation-triangle" style="font-size: 3rem; color: #dc2626; margin-bottom: 20px;"></i></p>
                <p style="font-size: 1.2rem; margin-bottom: 10px; color: #fecaca;">Failed to load folders</p>
                <p style="color: #94a3b8; margin-bottom: 20px;">${error.message}</p>
                <button onclick="loadFolders()"
                        style="background: #3b82f6; padding: 12px 24px; border-radius: 8px; color: white; border: none; cursor: pointer; font-weight: 600;">
                    <i class="fas fa-redo"></i> Try Again
                </button>
            </div>
        `;
    showError(`Failed to load folders: ${error.message}`);
  } finally {
    isLoading = false;
    if (searchBtn) searchBtn.disabled = false;
  }
}

// Render folders table
function renderFolders(folders) {
  if (!folders || folders.length === 0) {
    foldersContent.innerHTML = `
            <div style="text-align: center; padding: 40px;">
                <p><i class="fas fa-folder-open" style="font-size: 3rem; color: #475569; margin-bottom: 20px;"></i></p>
                <p style="font-size: 1.2rem; margin-bottom: 10px; color: #cbd5e1;">No folders found</p>
                <p style="color: #94a3b8; margin-bottom: 20px;">Add a folder to start monitoring music files</p>
                <button onclick="showAddFolderModal()"
                        style="background: #10b981; padding: 12px 24px; border-radius: 8px; color: white; border: none; cursor: pointer; font-weight: 600;">
                    <i class="fas fa-plus"></i> Add Your First Folder
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
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 30%;">Path</th>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 10%;">Status</th>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 10%;">Files</th>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 15%;">Config</th>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 15%;">Last Scanned</th>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 20%;">Actions</th>
                    </tr>
                </thead>
                <tbody>
    `;

  folders.forEach((folder) => {
    // Status badge
    const statusBadge = folder.watchEnabled
      ? `<span style="background: #10b981; color: white; padding: 4px 8px; border-radius: 4px; font-size: 0.8rem; font-weight: 600;">Active</span>`
      : `<span style="background: #6b7280; color: white; padding: 4px 8px; border-radius: 4px; font-size: 0.8rem; font-weight: 600;">Inactive</span>`;

    // Config summary
    let configSummary = [];
    if (folder.scanRecursive) configSummary.push("Recursive");
    if (folder.fixedExtensions && folder.fileExtensions) {
      configSummary.push(`${folder.fileExtensions.split(",").length} extensions`);
    } else if (!folder.fixedExtensions) {
      configSummary.push("All audio");
    }

    html += `
            <tr style="border-bottom: 1px solid #334155; transition: background 0.2s;">
                <td style="padding: 16px; color: #cbd5e1;">
                    <div style="font-weight: 500; color: white; font-family: monospace; font-size: 0.9rem;">${folder.path}</div>
                </td>
                <td style="padding: 16px; color: #cbd5e1; text-align: center;">
                    ${statusBadge}
                </td>
                <td style="padding: 16px; color: #cbd5e1; text-align: center; font-weight: 600; color: ${folder.fileCount > 0 ? "#3b82f6" : "#94a3b8"};">${folder.fileCount}</td>
                <td style="padding: 16px; color: #cbd5e1; font-size: 0.85rem; color: #94a3b8;">
                    ${configSummary.join(", ") || "Default"}
                </td>
                <td style="padding: 16px; color: #cbd5e1; font-size: 0.9rem; color: #94a3b8;">
                    ${formatDate(folder.lastScanned)}
                </td>
                <td style="padding: 16px; color: #cbd5e1;">
                    <div style="display: flex; gap: 8px; flex-wrap: wrap;">
                        <button onclick="scanFolder(${folder.id})" class="action-btn" style="background: #3b82f6;">
                            <i class="fas fa-sync-alt"></i> Scan
                        </button>
                        <button onclick="toggleWatch(${folder.id})" class="action-btn" style="background: ${folder.watchEnabled ? "#dc2626" : "#10b981"}">
                            <i class="fas ${folder.watchEnabled ? "fa-pause" : "fa-play"}"></i> ${folder.watchEnabled ? "Pause" : "Resume"}
                        </button>
                        <button onclick="editFolder(${folder.id}, '${folder.path}', ${folder.scanRecursive}, ${folder.fixedExtensions}, '${folder.fileExtensions}')" class="action-btn" style="background: #8b5cf6;">
                            <i class="fas fa-edit"></i> Edit
                        </button>
                        <button onclick="deleteFolder(${folder.id}, '${folder.path}')" class="action-btn" style="background: #dc2626;">
                            <i class="fas fa-trash"></i> Delete
                        </button>
                    </div>
                </td>
            </tr>
        `;
  });

  html += `
                </tbody>
            </table>
        </div>
    `;

  foldersContent.innerHTML = html;
}

// Update stats display
function updateStats(count) {
  if (totalFoldersElement) {
    totalFoldersElement.textContent = `Total: ${count}`;
  }
}

// Show add folder modal
function showAddFolderModal() {
  currentFolderId = null;
  modalTitle.textContent = "Add New Folder";
  folderPathInput.value = "";
  scanRecursiveCheckbox.checked = false;
  fixedExtensionsCheckbox.checked = false;
  fileExtensionsInput.value = "";
  extensionsContainer.style.display = "none";

  // Clear all extension checkboxes for new folder
  deselectAllExtensions();

  folderModal.style.display = "block";
}

// Close folder modal
function closeFolderModal() {
  folderModal.style.display = "none";
  currentFolderId = null;
}

// Edit folder - populate modal with existing data
function editFolder(id, path, scanRecursive, fixedExtensions, fileExtensions) {
  currentFolderId = id;
  modalTitle.textContent = "Edit Folder";
  folderPathInput.value = path;
  scanRecursiveCheckbox.checked = scanRecursive;
  fixedExtensionsCheckbox.checked = fixedExtensions;
  fileExtensionsInput.value = fileExtensions;
  extensionsContainer.style.display = fixedExtensions ? "block" : "none";

  // Set extension checkboxes based on saved extensions
  if (fixedExtensions && fileExtensions) {
    setSelectedExtensions(fileExtensions);
  } else {
    deselectAllExtensions();
  }

  folderModal.style.display = "block";
}

// Save folder (create or update)
async function saveFolder() {
  const path = folderPathInput.value.trim();
  if (!path) {
    showError("Please enter a folder path");
    return;
  }

  // Get selected extensions from checkboxes
  const selectedExtensions = getSelectedExtensions();

  const folderData = {
    path: path,
    watchEnabled: true,
    scanRecursive: scanRecursiveCheckbox.checked,
    fixedExtensions: fixedExtensionsCheckbox.checked,
    fileExtensions: fixedExtensionsCheckbox.checked ? selectedExtensions : "",
    maxDepth: scanRecursiveCheckbox.checked ? 10 : 1,
  };

  try {
    let url = `${API_BASE}/folders`;
    let method = "POST";

    if (currentFolderId) {
      url = `${API_BASE}/folders/${currentFolderId}`;
      method = "PUT";
    }

    const data = await fetchJSON(url, {
      method: method,
      body: JSON.stringify(folderData),
    });

    if (data && data.data) {
      closeFolderModal();
      loadFolders();
      showError(
        currentFolderId ? "Folder updated successfully" : "Folder added successfully",
      );
    } else {
      throw new Error("Invalid response format");
    }
  } catch (error) {
    console.error("Failed to save folder:", error);
    showError(`Failed to save folder: ${error.message}`);
  }
}

// Delete folder
async function deleteFolder(id, path) {
  if (
    !confirm(
      `Are you sure you want to delete folder:\n${path}\n\nThis will not delete the actual files on disk.`,
    )
  ) {
    return;
  }

  try {
    const data = await fetchJSON(`${API_BASE}/folders/${id}`, {
      method: "DELETE",
    });

    if (data && data.data) {
      loadFolders();
      showError("Folder deleted successfully");
    } else {
      throw new Error("Invalid response format");
    }
  } catch (error) {
    console.error("Failed to delete folder:", error);
    showError(`Failed to delete folder: ${error.message}`);
  }
}

// Scan folder
async function scanFolder(id) {
  try {
    const data = await fetchJSON(`${API_BASE}/folders/${id}/scan`, {
      method: "POST",
    });

    if (data && data.data) {
      showError("Folder scan started in background");
      // Refresh after a short delay to show updated file count
      setTimeout(() => loadFolders(), 2000);
    } else {
      throw new Error("Invalid response format");
    }
  } catch (error) {
    console.error("Failed to scan folder:", error);
    showError(`Failed to scan folder: ${error.message}`);
  }
}

// Toggle watch status
async function toggleWatch(id) {
  try {
    const data = await fetchJSON(`${API_BASE}/folders/${id}/watch`, {
      method: "POST",
    });

    if (data && data.data) {
      loadFolders();
      showError(`Folder ${enable ? "activated" : "deactivated"} successfully`);
    } else {
      throw new Error("Invalid response format");
    }
  } catch (error) {
    console.error("Failed to toggle watch status:", error);
    showError(`Failed to toggle watch status: ${error.message}`);
  }
}

// Search folders (client-side filtering for now)
function searchFolders() {
  const searchTerm = searchInput.value.trim().toLowerCase();
  // For now, just reload all folders
  // In the future, we could implement client-side filtering
  loadFolders(searchTerm);
}

// Initialize on page load
document.addEventListener("DOMContentLoaded", () => {
  // Check backend connection first
  fetchJSON(`${API_BASE}/health`)
    .then((data) => {
      console.log("Backend connected:", data);
      loadFolders();
    })
    .catch((error) => {
      console.error("Backend connection failed:", error);
      showError("Backend is not responding. Make sure the server is running.");
      foldersContent.innerHTML = `
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
        searchFolders();
      }
    });
  }

  // Set up checkbox event listeners for modal
  if (fixedExtensionsCheckbox) {
    fixedExtensionsCheckbox.addEventListener("change", function () {
      if (this.checked) {
        extensionsContainer.style.display = "block";
      } else {
        extensionsContainer.style.display = "none";
      }
    });
  }

  // Close modal when clicking outside
  window.addEventListener("click", function (event) {
    if (event.target === folderModal) {
      closeFolderModal();
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

    // Escape to close modal
    if (event.key === "Escape" && folderModal.style.display === "block") {
      closeFolderModal();
    }

    // Escape to clear search
    if (event.key === "Escape" && document.activeElement === searchInput && searchInput) {
      searchInput.value = "";
      loadFolders("");
    }
  });
});

// Helper functions for extension checkboxes
function getSelectedExtensions() {
  const checkboxes = document.querySelectorAll(".extension-checkbox:checked");
  const selectedValues = Array.from(checkboxes).map((cb) => cb.value);
  return selectedValues.join(",");
}

function setSelectedExtensions(extensionsString) {
  // Clear all checkboxes first
  deselectAllExtensions();

  if (!extensionsString || extensionsString.trim() === "") {
    return;
  }

  const extensions = extensionsString.split(",").map((ext) => ext.trim());
  extensions.forEach((ext) => {
    const checkbox = document.querySelector(`.extension-checkbox[value="${ext}"]`);
    if (checkbox) {
      checkbox.checked = true;
    }
  });
}

function selectAllExtensions() {
  const checkboxes = document.querySelectorAll(".extension-checkbox");
  checkboxes.forEach((cb) => (cb.checked = true));
}

function deselectAllExtensions() {
  const checkboxes = document.querySelectorAll(".extension-checkbox");
  checkboxes.forEach((cb) => (cb.checked = false));
}

// Dummy pagination functions to prevent JavaScript errors
// Note: Folders API doesn't currently support pagination
function previousPage() {
  console.log("Pagination not implemented for folders");
}

function nextPage() {
  console.log("Pagination not implemented for folders");
}

function goToPage(page) {
  console.log(`Pagination not implemented for folders, requested page: ${page}`);
}

function goToLastPage() {
  console.log("Pagination not implemented for folders");
}
