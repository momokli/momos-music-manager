const API_BASE = "http://localhost:3000/api";
const PLAYLISTS_PER_PAGE = 50;

let currentPage = 0;
let totalPlaylists = 0;
let currentSearch = "";
let currentService = "all"; // Default to showing all services
let isLoading = false;

// DOM elements
const playlistsContent = document.getElementById("playlists-content");
const errorMessage = document.getElementById("error-message");
const totalPlaylistsElement = document.getElementById("total-playlists");
const currentPageElement = document.getElementById("current-page");
const showingPlaylistsElement = document.getElementById("showing-playlists");
const searchInput = document.getElementById("search-input");
const searchBtn = document.getElementById("search-btn");
const firstPageBtn = document.getElementById("first-page-btn");
const prevPageBtn = document.getElementById("prev-page-btn");
const nextPageBtn = document.getElementById("next-page-btn");
const lastPageBtn = document.getElementById("last-page-btn");
const pageInfo = document.getElementById("page-info");

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

// Load playlists from API
async function loadPlaylists(page = currentPage, search = currentSearch) {
  if (isLoading) return;

  isLoading = true;
  playlistsContent.innerHTML = `
        <div class="loading">
            <div class="loading-spinner"></div>
            <p>Loading playlists...</p>
        </div>
    `;

  // Update button states
  if (searchBtn) searchBtn.disabled = true;
  if (firstPageBtn) firstPageBtn.disabled = true;
  if (prevPageBtn) prevPageBtn.disabled = true;
  if (nextPageBtn) nextPageBtn.disabled = true;
  if (lastPageBtn) lastPageBtn.disabled = true;

  try {
    const offset = page * PLAYLISTS_PER_PAGE;
    let url = `${API_BASE}/playlists?limit=${PLAYLISTS_PER_PAGE}&offset=${offset}`;

    // Only add service filter if not "all" (default shows all services)
    if (currentService && currentService !== "all") {
      url += `&service=${currentService}`;
    }

    if (search && search.trim() !== "") {
      url += `&search=${encodeURIComponent(search.trim())}`;
    }

    const data = await fetchJSON(url);

    if (data && data.data) {
      currentPage = page;
      currentSearch = search;
      totalPlaylists = data.data.total;

      renderPlaylists(data.data.playlists);
      updatePagination();
      updateStats(data.data.playlists.length, offset);
    } else {
      throw new Error("Invalid response format");
    }
  } catch (error) {
    console.error("Failed to load playlists:", error);
    playlistsContent.innerHTML = `
            <div style="text-align: center; padding: 40px;">
                <p><i class="fas fa-exclamation-triangle" style="font-size: 3rem; color: #dc2626; margin-bottom: 20px;"></i></p>
                <p style="font-size: 1.2rem; margin-bottom: 10px; color: #fecaca;">Failed to load playlists</p>
                <p style="color: #94a3b8; margin-bottom: 20px;">${error.message}</p>
                <button onclick="loadPlaylists()"
                        style="background: #3b82f6; padding: 12px 24px; border-radius: 8px; color: white; border: none; cursor: pointer; font-weight: 600;">
                    <i class="fas fa-redo"></i> Try Again
                </button>
            </div>
        `;
    showError(`Failed to load playlists: ${error.message}`);
  } finally {
    isLoading = false;
    if (searchBtn) searchBtn.disabled = false;
    updatePagination();
  }
}

// Render playlists table
function renderPlaylists(playlists) {
  if (!playlists || playlists.length === 0) {
    playlistsContent.innerHTML = `
            <div style="text-align: center; padding: 40px;">
                <p><i class="fas fa-music" style="font-size: 3rem; color: #475569; margin-bottom: 20px;"></i></p>
                <p style="font-size: 1.2rem; margin-bottom: 10px; color: #cbd5e1;">No playlists found</p>
                <p style="color: #94a3b8; margin-bottom: 20px;">${currentSearch ? "Try adjusting your search" : "Try syncing playlists first"}</p>
                <button onclick="window.open('/api/services/spotify/sync/playlists', '_blank')"
                        style="background: #1db954; padding: 12px 24px; border-radius: 8px; color: white; border: none; cursor: pointer; font-weight: 600;">
                    <i class="fas fa-sync-alt"></i> Sync Playlists Now
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
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 35%;">Playlist Name</th>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 15%;">Service</th>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 25%;">Service ID</th>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 10%;">Tracks</th>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 15%;">Last Updated</th>
                    </tr>
                </thead>
                <tbody>
    `;

  playlists.forEach((playlist) => {
    let descriptionHtml = "";
    if (playlist.description) {
      descriptionHtml = `<div style="font-size: 0.85rem; color: #94a3b8; margin-top: 4px;">${playlist.description}</div>`;
    }

    // Determine service color
    let serviceColor = "#94a3b8";
    let serviceText = playlist.service || "unknown";
    if (playlist.service === "spotify") {
      serviceColor = "#1db954";
    } else if (playlist.service === "soundcloud") {
      serviceColor = "#ff7700";
    } else if (playlist.service === "youtube") {
      serviceColor = "#ff0000";
    }

    html += `
          <tr style="border-bottom: 1px solid #334155; transition: background 0.2s;">
              <td style="padding: 16px; color: #cbd5e1;">
                  <div style="font-weight: 500; color: white;">${playlist.name || "Unnamed Playlist"}</div>
                  ${descriptionHtml}
              </td>
              <td style="padding: 16px; color: #cbd5e1; text-align: center;">
                  <span style="background: ${serviceColor}; padding: 4px 8px; border-radius: 4px; font-size: 0.8rem; font-weight: 600; color: white;">${serviceText.toUpperCase()}</span>
              </td>
              <td style="padding: 16px; color: #cbd5e1;">
                  <div style="font-family: monospace; font-size: 0.85rem; color: #94a3b8;">${playlist.playlistId}</div>
              </td>
              <td style="padding: 16px; text-align: center; font-weight: 600; color: #1db954;">${playlist.trackCount}</td>
              <td style="padding: 16px; color: #cbd5e1;">
                  <div style="font-size: 0.9rem; color: #94a3b8;">${formatDate(playlist.updatedAt)}</div>
              </td>
          </tr>
      `;
  });

  html += `
                </tbody>
            </table>
        </div>
    `;

  playlistsContent.innerHTML = html;
}

// Update pagination controls
function updatePagination() {
  const totalPages = Math.ceil(totalPlaylists / PLAYLISTS_PER_PAGE);

  // Update page info
  if (pageInfo) {
    pageInfo.textContent = `Page ${currentPage + 1} of ${totalPages || 1}`;
  }

  // Update button states
  if (firstPageBtn) {
    firstPageBtn.disabled = currentPage === 0 || totalPlaylists === 0 || isLoading;
    firstPageBtn.style.opacity = firstPageBtn.disabled ? "0.5" : "1";
    firstPageBtn.style.cursor = firstPageBtn.disabled ? "not-allowed" : "pointer";
  }

  if (prevPageBtn) {
    prevPageBtn.disabled = currentPage === 0 || totalPlaylists === 0 || isLoading;
    prevPageBtn.style.opacity = prevPageBtn.disabled ? "0.5" : "1";
    prevPageBtn.style.cursor = prevPageBtn.disabled ? "not-allowed" : "pointer";
  }

  if (nextPageBtn) {
    nextPageBtn.disabled =
      currentPage >= totalPages - 1 || totalPlaylists === 0 || isLoading;
    nextPageBtn.style.opacity = nextPageBtn.disabled ? "0.5" : "1";
    nextPageBtn.style.cursor = nextPageBtn.disabled ? "not-allowed" : "pointer";
  }

  if (lastPageBtn) {
    lastPageBtn.disabled =
      currentPage >= totalPages - 1 || totalPlaylists === 0 || isLoading;
    lastPageBtn.style.opacity = lastPageBtn.disabled ? "0.5" : "1";
    lastPageBtn.style.cursor = lastPageBtn.disabled ? "not-allowed" : "pointer";
  }
}

// Update stats display
function updateStats(showingCount, offset) {
  if (totalPlaylistsElement) {
    totalPlaylistsElement.textContent = `Total: ${totalPlaylists.toLocaleString()}`;
  }

  if (currentPageElement) {
    currentPageElement.textContent = `Page: ${currentPage + 1}`;
  }

  if (showingPlaylistsElement) {
    const start = totalPlaylists > 0 ? offset + 1 : 0;
    const end = Math.min(offset + showingCount, totalPlaylists);
    showingPlaylistsElement.textContent = `Showing: ${start}-${end}`;
  }
}

// Search playlists
function searchPlaylists() {
  const searchTerm = searchInput.value.trim();
  loadPlaylists(0, searchTerm);
}

// Pagination functions
function previousPage() {
  if (currentPage > 0 && !isLoading) {
    loadPlaylists(currentPage - 1, currentSearch);
  }
}

function nextPage() {
  const totalPages = Math.ceil(totalPlaylists / PLAYLISTS_PER_PAGE);
  if (currentPage < totalPages - 1 && !isLoading) {
    loadPlaylists(currentPage + 1, currentSearch);
  }
}

function goToPage(page) {
  if (!isLoading) {
    loadPlaylists(page, currentSearch);
  }
}

function goToLastPage() {
  const totalPages = Math.ceil(totalPlaylists / PLAYLISTS_PER_PAGE);
  if (totalPages > 0 && !isLoading) {
    loadPlaylists(totalPages - 1, currentSearch);
  }
}

// Initialize on page load
document.addEventListener("DOMContentLoaded", () => {
  // Check backend connection first
  fetchJSON(`${API_BASE}/health`)
    .then((data) => {
      console.log("Backend connected:", data);
      loadPlaylists();
    })
    .catch((error) => {
      console.error("Backend connection failed:", error);
      showError("Backend is not responding. Make sure the server is running.");
      playlistsContent.innerHTML = `
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
        searchPlaylists();
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
    loadPlaylists(0, "");
  }

  // Arrow keys for pagination
  if (event.key === "ArrowLeft" && !isLoading) {
    previousPage();
  }
  if (event.key === "ArrowRight" && !isLoading) {
    nextPage();
  }
});
