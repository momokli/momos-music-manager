const API_BASE = "http://localhost:3000/api";
const TRACKS_PER_PAGE = 50;

let currentPage = 0;
let totalTracks = 0;
let currentSearch = "";
let currentService = "all"; // "spotify", "soundcloud", "youtube", "all"
let isLoading = false;

// DOM elements
const tracksContent = document.getElementById("tracks-content");
const errorMessage = document.getElementById("error-message");
const totalTracksElement = document.getElementById("total-tracks");
const currentPageElement = document.getElementById("current-page");
const showingTracksElement = document.getElementById("showing-tracks");
const searchInput = document.getElementById("search-input");
const searchBtn = document.getElementById("search-btn");
const firstPageBtn = document.getElementById("first-page-btn");
const prevPageBtn = document.getElementById("prev-page-btn");
const nextPageBtn = document.getElementById("next-page-btn");
const lastPageBtn = document.getElementById("last-page-btn");
const pageInfo = document.getElementById("page-info");

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

// Load tracks from API
async function loadTracks(
  page = currentPage,
  search = currentSearch,
  service = currentService,
) {
  if (isLoading) return;

  isLoading = true;
  tracksContent.innerHTML = `
        <div class="loading">
            <div class="loading-spinner"></div>
            <p>Loading tracks...</p>
        </div>
    `;

  // Update button states
  if (searchBtn) searchBtn.disabled = true;
  if (firstPageBtn) firstPageBtn.disabled = true;
  if (prevPageBtn) prevPageBtn.disabled = true;
  if (nextPageBtn) nextPageBtn.disabled = true;
  if (lastPageBtn) lastPageBtn.disabled = true;

  try {
    const offset = page * TRACKS_PER_PAGE;
    let url = `${API_BASE}/tracks?limit=${TRACKS_PER_PAGE}&offset=${offset}`;

    if (service !== "all") {
      url += `&service=${service}`;
    }

    if (search && search.trim() !== "") {
      url += `&search=${encodeURIComponent(search.trim())}`;
    }

    // Also get total count
    let countUrl = `${API_BASE}/tracks/count`;
    if (service !== "all") {
      countUrl += `&service=${service}`;
    }
    if (search && search.trim() !== "") {
      countUrl += `&search=${encodeURIComponent(search.trim())}`;
    }

    const [tracksData, countData] = await Promise.all([
      fetchJSON(url),
      fetchJSON(countUrl),
    ]);

    if (tracksData && tracksData.data && countData && countData.data !== undefined) {
      currentPage = page;
      currentSearch = search;
      currentService = service;
      totalTracks = countData.data;

      renderTracks(tracksData.data);
      updatePagination();
      updateStats(tracksData.data.length, offset);
    } else {
      throw new Error("Invalid response format");
    }
  } catch (error) {
    console.error("Failed to load tracks:", error);
    tracksContent.innerHTML = `
            <div style="text-align: center; padding: 40px;">
                <p><i class="fas fa-exclamation-triangle" style="font-size: 3rem; color: #dc2626; margin-bottom: 20px;"></i></p>
                <p style="font-size: 1.2rem; margin-bottom: 10px; color: #fecaca;">Failed to load tracks</p>
                <p style="color: #94a3b8; margin-bottom: 20px;">${error.message}</p>
                <button onclick="loadTracks()"
                        style="background: #3b82f6; padding: 12px 24px; border-radius: 8px; color: white; border: none; cursor: pointer; font-weight: 600;">
                    <i class="fas fa-redo"></i> Try Again
                </button>
            </div>
        `;
    showError(`Failed to load tracks: ${error.message}`);
  } finally {
    isLoading = false;
    if (searchBtn) searchBtn.disabled = false;
    updatePagination();
  }
}

// Render local file type badges
function renderLocalFiles(localFiles) {
  if (!localFiles || localFiles.length === 0) {
    return `<span style="color: #475569;">—</span>`;
  }

  const fileTypeColors = {
    flac: "#9b59b6",
    mp3: "#3498db",
    "stem.m4a": "#e67e22",
    m4a: "#1abc9c",
    wav: "#e74c3c",
    opus: "#2ecc71",
  };

  const fileTypeIcons = {
    flac: "🎵",
    mp3: "🎵",
    "stem.m4a": "🎛️",
    m4a: "🎵",
    wav: "🎵",
    opus: "🎵",
  };

  return localFiles
    .map(
      (ft) =>
        `<span style="display: inline-block; background: ${fileTypeColors[ft] || "#334155"}; padding: 4px 8px; border-radius: 4px; font-size: 0.8rem; font-weight: 600; color: white; margin: 2px;">${fileTypeIcons[ft] || ""} ${ft}</span>`,
    )
    .join(" ");
}

// Render tracks table
function renderTracks(tracks) {
  if (!tracks || tracks.length === 0) {
    tracksContent.innerHTML = `
            <div style="text-align: center; padding: 40px;">
                <p><i class="fas fa-stream" style="font-size: 3rem; color: #475569; margin-bottom: 20px;"></i></p>
                <p style="font-size: 1.2rem; margin-bottom: 10px; color: #cbd5e1;">No tracks found</p>
                <p style="color: #94a3b8; margin-bottom: 20px;">${currentSearch ? "Try adjusting your search" : "Try syncing from services first"}</p>
                <button onclick="window.location.href='playlists.html'"
                        style="background: #1db954; padding: 12px 24px; border-radius: 8px; color: white; border: none; cursor: pointer; font-weight: 600;">
                    <i class="fas fa-music"></i> View Playlists
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
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 22%;">Title</th>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 18%;">Artist</th>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 12%;">Service</th>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 8%;">Album</th>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 12%;">Local Files</th>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 12%;">Duration</th>
                        <th style="padding: 16px; text-align: left; color: #94a3b8; font-weight: 600; border-bottom: 1px solid #334155; width: 16%;">ISRC</th>
                    </tr>
                </thead>
                <tbody>
    `;

  tracks.forEach((track) => {
    // Determine service color
    let serviceColor = "#94a3b8";
    let serviceText = track.service || "unknown";
    if (track.service === "spotify") {
      serviceColor = "#1db954";
    } else if (track.service === "soundcloud") {
      serviceColor = "#ff7700";
    } else if (track.service === "youtube") {
      serviceColor = "#ff0000";
    }

    html += `
            <tr style="border-bottom: 1px solid #334155; transition: background 0.2s;">
                <td style="padding: 16px; color: #cbd5e1;">
                    <div style="font-weight: 500; color: white;">${track.title || "Untitled"}</div>
                </td>
                <td style="padding: 16px; color: #cbd5e1;">${track.artist || "Unknown"}</td>
                <td style="padding: 16px; color: #cbd5e1; text-align: center;">
                    <span style="background: ${serviceColor}; padding: 4px 8px; border-radius: 4px; font-size: 0.8rem; font-weight: 600; color: white;">${serviceText.toUpperCase()}</span>
                </td>
                <td style="padding: 16px; color: #cbd5e1;">${track.album || "-"}</td>
                <td style="padding: 16px; color: #cbd5e1; text-align: center;">
                    ${renderLocalFiles(track.localFiles)}
                </td>
                <td style="padding: 16px; color: #cbd5e1; text-align: center;">${formatDuration(track.durationMs) || "-"}</td>
                <td style="padding: 16px; color: #cbd5e1; font-size: 0.85rem;">${track.isrc || "-"}</td>
            </tr>
        `;
  });

  html += `
                </tbody>
            </table>
        </div>
    `;

  tracksContent.innerHTML = html;
}

// Update pagination controls
function updatePagination() {
  const totalPages = Math.ceil(totalTracks / TRACKS_PER_PAGE);

  // Update page info
  if (pageInfo) {
    pageInfo.textContent = `Page ${currentPage + 1} of ${totalPages || 1}`;
  }

  // Update button states
  if (firstPageBtn) {
    firstPageBtn.disabled = currentPage === 0 || totalTracks === 0 || isLoading;
    firstPageBtn.style.opacity = firstPageBtn.disabled ? "0.5" : "1";
    firstPageBtn.style.cursor = firstPageBtn.disabled ? "not-allowed" : "pointer";
  }

  if (prevPageBtn) {
    prevPageBtn.disabled = currentPage === 0 || totalTracks === 0 || isLoading;
    prevPageBtn.style.opacity = prevPageBtn.disabled ? "0.5" : "1";
    prevPageBtn.style.cursor = prevPageBtn.disabled ? "not-allowed" : "pointer";
  }

  if (nextPageBtn) {
    nextPageBtn.disabled =
      currentPage >= totalPages - 1 || totalTracks === 0 || isLoading;
    nextPageBtn.style.opacity = nextPageBtn.disabled ? "0.5" : "1";
    nextPageBtn.style.cursor = nextPageBtn.disabled ? "not-allowed" : "pointer";
  }

  if (lastPageBtn) {
    lastPageBtn.disabled =
      currentPage >= totalPages - 1 || totalTracks === 0 || isLoading;
    lastPageBtn.style.opacity = lastPageBtn.disabled ? "0.5" : "1";
    lastPageBtn.style.cursor = lastPageBtn.disabled ? "not-allowed" : "pointer";
  }
}

// Update stats display
function updateStats(showingCount, offset) {
  if (totalTracksElement) {
    totalTracksElement.textContent = `Total: ${totalTracks.toLocaleString()}`;
  }

  if (currentPageElement) {
    currentPageElement.textContent = `Page: ${currentPage + 1}`;
  }

  if (showingTracksElement) {
    const start = totalTracks > 0 ? offset + 1 : 0;
    const end = Math.min(offset + showingCount, totalTracks);
    showingTracksElement.textContent = `Showing: ${start}-${end}`;
  }
}

// Search tracks
function searchTracks() {
  const searchTerm = searchInput.value.trim();
  loadTracks(0, searchTerm, currentService);
}

// Filter by service
function filterByService() {
  const serviceFilter = document.getElementById("service-filter");
  if (serviceFilter) {
    currentService = serviceFilter.value;
    loadTracks(0, currentSearch, currentService);
  }
}

// Pagination functions
function previousPage() {
  if (currentPage > 0 && !isLoading) {
    loadTracks(currentPage - 1, currentSearch, currentService);
  }
}

function nextPage() {
  const totalPages = Math.ceil(totalTracks / TRACKS_PER_PAGE);
  if (currentPage < totalPages - 1 && !isLoading) {
    loadTracks(currentPage + 1, currentSearch, currentService);
  }
}

function goToPage(page) {
  if (!isLoading) {
    loadTracks(page, currentSearch, currentService);
  }
}

function goToLastPage() {
  const totalPages = Math.ceil(totalTracks / TRACKS_PER_PAGE);
  if (totalPages > 0 && !isLoading) {
    loadTracks(totalPages - 1, currentSearch, currentService);
  }
}

// Initialize on page load
document.addEventListener("DOMContentLoaded", () => {
  // Check backend connection first
  fetchJSON(`${API_BASE}/health`)
    .then((data) => {
      console.log("Backend connected:", data);
      loadTracks();
    })
    .catch((error) => {
      console.error("Backend connection failed:", error);
      showError("Backend is not responding. Make sure the server is running.");
      tracksContent.innerHTML = `
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
        searchTracks();
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
    loadTracks(0, "", currentService);
  }

  // Arrow keys for pagination
  if (event.key === "ArrowLeft" && !isLoading) {
    previousPage();
  }
  if (event.key === "ArrowRight" && !isLoading) {
    nextPage();
  }
});
