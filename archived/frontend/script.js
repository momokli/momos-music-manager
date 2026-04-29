// Music Manager POC - Simple Frontend JavaScript
// Backend API: /api/ (proxied through frontend server)

// Configuration
const API_BASE = "http://localhost:3000/api";
const API_BASE_FALLBACK = "http://127.0.0.1:3000/api";
let currentService = null;
let servicesData = [];
let pollInterval = null;

// DOM elements
const servicesContainer = document.getElementById("services-container");
const errorMessage = document.getElementById("error-message");
const configModal = document.getElementById("config-modal");
const modalBody = document.getElementById("modal-body");
const modalTitle = document.getElementById("modal-title");
const backendStatus = document.getElementById("backend-status");
const backendStatusDot = document.getElementById("backend-status-dot");
const apiUrlElement = document.getElementById("api-url");

// Utility functions
function showError(message) {
  errorMessage.textContent = message;
  errorMessage.style.display = "block";
  setTimeout(() => {
    errorMessage.style.display = "none";
  }, 5000);
}

function formatDate(timestamp) {
  if (!timestamp) return "Never";
  return new Date(timestamp).toLocaleString();
}

// API calls
async function fetchJSON(url, options = {}) {
  try {
    console.log(`Fetching: ${url}`);
    // Only add Content-Type for requests with body (POST, PUT)
    const headers = {};
    if (options.method && ["POST", "PUT"].includes(options.method.toUpperCase())) {
      headers["Content-Type"] = "application/json";
    }
    const response = await fetch(url, {
      headers,
      ...options,
    });
    if (!response.ok) {
      throw new Error(`HTTP ${response.status}: ${response.statusText}`);
    }
    const data = await response.json();
    console.log(`Fetch success: ${url}`);
    // Handle both wrapped ({data: ...}) and unwrapped responses
    return data.data !== undefined ? data.data : data;
  } catch (error) {
    console.error(`Fetch error for ${url}:`, error);
    throw error;
  }
}

async function checkBackend() {
  let health = null;

  // Try primary URL first
  try {
    console.log(`Trying backend at: ${API_BASE}/health`);
    health = await fetchJSON(`${API_BASE}/health`);
    console.log(`Primary backend responded:`, health);
  } catch (primaryError) {
    console.log(`Primary backend failed: ${primaryError.message}`);

    // Try fallback URL
    try {
      console.log(`Trying fallback backend at: ${API_BASE_FALLBACK}/health`);
      health = await fetchJSON(`${API_BASE_FALLBACK}/health`);
      console.log(`Fallback backend responded:`, health);

      // Update global API_BASE to fallback since it worked
      window.API_BASE_USED = API_BASE_FALLBACK;
    } catch (fallbackError) {
      console.log(`Fallback backend also failed: ${fallbackError.message}`);
      backendStatus.textContent =
        "Backend: Not responding (tried localhost and 127.0.0.1)";
      backendStatusDot.className = "status-dot red";
      return false;
    }
  }

  backendStatus.textContent = `Backend: ${health.status} (DB: ${health.database})`;
  backendStatusDot.className = "status-dot green";
  return true;
}

async function loadServices() {
  try {
    servicesContainer.innerHTML = `
            <div class="loading">
                <div class="loading-spinner"></div>
                <p>Loading services...</p>
            </div>
        `;

    const isBackendOk = await checkBackend();
    if (!isBackendOk) {
      throw new Error(
        `Backend not available at localhost:3000 or 127.0.0.1:3000.\n\nMake sure:\n1. Backend is running: cargo run -- serve --host 127.0.0.1 --port 3000\n2. Check if backend binds to 127.0.0.1 instead of localhost`,
      );
    }

    // Use whichever API base worked
    const activeApiBase = window.API_BASE_USED || API_BASE;
    console.log(`Using API base: ${activeApiBase}`);
    servicesData = await fetchJSON(`${activeApiBase}/services`);
    renderServices();

    // Start polling for sync status if any service is syncing
    startPolling(activeApiBase);
  } catch (error) {
    showError(`Failed to load services: ${error.message}`);
    servicesContainer.innerHTML = `
            <div style="text-align: center; padding: 40px; color: #ef4444;">
                <i class="fas fa-exclamation-triangle" style="font-size: 3rem; margin-bottom: 20px;"></i>
                <h3>Failed to connect to backend</h3>
                <p>${error.message}</p>
                <button class="refresh-btn" onclick="loadServices()" style="margin-top: 20px;">
                    <i class="fas fa-sync-alt"></i> Retry
                </button>
            </div>
        `;
  }
}

// Polling for sync status
function startPolling(apiBase) {
  // Clear existing interval if any
  if (pollInterval) {
    clearInterval(pollInterval);
    pollInterval = null;
  }

  // Check if any service is currently syncing
  const anySyncing = servicesData && servicesData.some((service) => service.isSyncing);

  if (anySyncing) {
    // Start polling every 2 seconds when syncing
    pollInterval = setInterval(() => {
      checkForSync(apiBase);
    }, 2000);

    console.log(`Started polling for sync status every 2 seconds`);
  }
}

async function checkForSync(apiBase) {
  try {
    const newServicesData = await fetchJSON(`${apiBase}/services`);

    // Update servicesData and re-render if sync status changed
    let needsUpdate = false;
    if (servicesData && newServicesData) {
      // Check if any service's isSyncing status changed
      for (let i = 0; i < Math.min(servicesData.length, newServicesData.length); i++) {
        if (servicesData[i].isSyncing !== newServicesData[i].isSyncing) {
          needsUpdate = true;
          break;
        }
      }

      // Also check if remote counts changed (to show progress)
      for (let i = 0; i < Math.min(servicesData.length, newServicesData.length); i++) {
        if (
          servicesData[i].playlistsRemote !== newServicesData[i].playlistsRemote ||
          servicesData[i].tracksRemote !== newServicesData[i].tracksRemote
        ) {
          needsUpdate = true;
          break;
        }
      }

      // Check if sync progress fields changed
      for (let i = 0; i < Math.min(servicesData.length, newServicesData.length); i++) {
        if (
          servicesData[i].syncCurrentPlaylist !==
            newServicesData[i].syncCurrentPlaylist ||
          servicesData[i].syncTotalPlaylists !== newServicesData[i].syncTotalPlaylists ||
          servicesData[i].syncCurrentTrack !== newServicesData[i].syncCurrentTrack ||
          servicesData[i].syncTotalTracks !== newServicesData[i].syncTotalTracks ||
          servicesData[i].syncLog !== newServicesData[i].syncLog
        ) {
          needsUpdate = true;
          break;
        }
      }
    }

    // Update data
    servicesData = newServicesData;

    // Re-render if needed
    if (needsUpdate) {
      renderServices();

      // Check if we should continue polling
      const anySyncing =
        servicesData && servicesData.some((service) => service.isSyncing);
      if (!anySyncing) {
        console.log("All syncs completed, stopping polling");
        if (pollInterval) {
          clearInterval(pollInterval);
          pollInterval = null;
        }
      }
    }
  } catch (error) {
    console.error("Failed to check sync status:", error);
    // On error, stop polling to avoid spam
    if (pollInterval) {
      clearInterval(pollInterval);
      pollInterval = null;
    }
  }
}

function renderServices() {
  if (!servicesData || servicesData.length === 0) {
    servicesContainer.innerHTML = `
            <div style="text-align: center; padding: 40px;">
                <p>No services found</p>
            </div>
        `;
    return;
  }

  let html = '<div class="services-grid">';

  servicesData.forEach((service) => {
    const iconClass = `${service.service}-icon`;
    const iconSymbol =
      service.service === "spotify"
        ? '<i class="fab fa-spotify"></i>'
        : service.service === "soundcloud"
          ? '<i class="fab fa-soundcloud"></i>'
          : '<i class="fab fa-youtube"></i>';

    const configuredBadge = service.configured
      ? '<span class="badge configured">Configured via .env</span>'
      : '<span class="badge not-configured">Not configured</span>';

    const connectedBadge = service.connected
      ? '<span class="badge connected">Connected</span>'
      : '<span class="badge disconnected">Disconnected</span>';

    const syncingBadge = service.isSyncing
      ? '<span class="badge syncing"><i class="fas fa-sync-alt fa-spin"></i> Syncing...</span>'
      : "";

    const lastSyncHtml = service.lastSync
      ? `<div class="last-sync">Last synced: ${formatDate(service.lastSync)}</div>`
      : "";

    const countsHtml = `
    <div class="service-counts">
        <div class="count-row">
            <span class="count-item"><i class="fas fa-list"></i> ${service.playlistsRemote || 0} on service</span>
            <span class="count-item"><i class="fas fa-download"></i> ${service.playlistsLocal || 0} locally</span>
        </div>
        <div class="count-row">
            <span class="count-item"><i class="fas fa-music"></i> ${service.tracksRemote || 0} on service</span>
            <span class="count-item"><i class="fas fa-download"></i> ${service.tracksLocal || 0} locally</span>
        </div>
    </div>`;

    // Sync progress visualization
    let progressHtml = "";
    if (service.isSyncing) {
      const currentPlaylist = service.syncCurrentPlaylist || 0;
      const totalPlaylists = service.syncTotalPlaylists || 0;
      const currentTrack = service.syncCurrentTrack || 0;
      const totalTracks = service.syncTotalTracks || 0;

      // Calculate progress percentage
      let progressPercent = 0;
      if (totalPlaylists > 0) {
        progressPercent = Math.min(100, (currentPlaylist / totalPlaylists) * 100);
      }

      // Check if we're in indeterminate state (no totals yet)
      const isIndeterminate = totalPlaylists === 0 || totalPlaylists === null;

      progressHtml = `
            <div class="sync-progress-container" style="margin: 15px 0; padding: 10px; background: #0f172a; border-radius: 8px;">
                <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 8px;">
                    <span style="font-size: 0.85rem; color: #94a3b8;">Sync Progress</span>
                    <span style="font-size: 0.85rem; color: #3b82f6;">
                        ${isIndeterminate ? "Initializing..." : `${currentPlaylist}/${totalPlaylists} playlists`}
                    </span>
                </div>
                <div style="width: 100%; height: 8px; background: #334155; border-radius: 4px; overflow: hidden;">
                    ${
                      isIndeterminate
                        ? `<div style="height: 100%; width: 100%; background: linear-gradient(90deg, transparent 0%, rgba(59, 130, 246, 0.8) 25%, rgb(59, 130, 246) 50%, rgba(59, 130, 246, 0.8) 75%, transparent 100%); background-size: 200% 100%; animation: indeterminate-progress 1.5s ease-in-out infinite;"></div>`
                        : `<div style="height: 100%; width: ${progressPercent}%; background: #3b82f6; border-radius: 4px; transition: width 0.3s ease;"></div>`
                    }
                </div>
                <div style="display: flex; justify-content: space-between; align-items: center; margin-top: 8px;">
                    <span style="font-size: 0.75rem; color: #94a3b8;">Progress:</span>
                    <span style="font-size: 0.75rem; color: #3b82f6;">
                        ${isIndeterminate ? "Initializing..." : `${progressPercent.toFixed(1)}%`}
                    </span>
                </div>
                ${
                  currentTrack > 0 || totalTracks > 0
                    ? `
                    <div style="display: flex; justify-content: space-between; align-items: center; margin-top: 4px;">
                        <span style="font-size: 0.75rem; color: #94a3b8;">Tracks:</span>
                        <span style="font-size: 0.75rem; color: #8b5cf6;">
                            ${currentTrack.toLocaleString()}/${totalTracks.toLocaleString()}
                        </span>
                    </div>
                `
                    : ""
                }
        `;

      // Add sync log if available
      if (service.syncLog) {
        try {
          const logEntries = JSON.parse(service.syncLog);
          const recentEntries = logEntries.slice(-5); // Show last 5 entries

          progressHtml += `
                    <div style="margin-top: 12px;">
                        <div style="font-size: 0.75rem; color: #94a3b8; margin-bottom: 4px;">Recent Activity:</div>
                        <div style="max-height: 80px; overflow-y: auto; background: #0f172a; border-radius: 4px; padding: 6px; font-size: 0.7rem;">
                `;

          if (recentEntries.length === 0) {
            progressHtml += `<div style="color: #64748b; font-style: italic;">No recent activity</div>`;
          } else {
            recentEntries.forEach((entry, index) => {
              // Parse format: "[timestamp] message"
              const match = entry.match(/^\[(\d+)\] (.+)$/);
              let displayText = entry;
              if (match) {
                const timestamp = parseInt(match[1], 10) * 1000;
                const message = match[2];
                const timeStr = new Date(timestamp).toLocaleTimeString([], {
                  hour: "2-digit",
                  minute: "2-digit",
                });
                displayText = `${timeStr}: ${message}`;
              }
              progressHtml += `<div style="color: #cbd5e1; padding: 2px 0; border-bottom: 1px solid #334155; ${index === recentEntries.length - 1 ? "border-bottom: none;" : ""}">${displayText}</div>`;
            });
          }

          progressHtml += `
                        </div>
                    </div>
                `;
        } catch (e) {
          console.error("Failed to parse sync log:", e);
        }
      }

      progressHtml += `</div>`;
    }

    html += `
            <div class="service-card" data-service="${service.service}">
                <div class="service-header">
                    <div class="service-icon ${iconClass}">
                        ${iconSymbol}
                    </div>
                    <div class="service-title">
                        <h3>${service.service.charAt(0).toUpperCase() + service.service.slice(1)}</h3>
                    </div>
                </div>
                <div class="service-status">
                    ${configuredBadge}
                    ${connectedBadge}
                    ${syncingBadge}
                </div>
                ${countsHtml}
                ${progressHtml}
                <div class="service-actions">
                    <div class="button-row">
                        <button class="btn btn-action ${service.connected ? "btn-disabled" : "btn-connect"}"
                                onclick="${service.connected ? "" : `startAuth('${service.service}', this)`}"
                                ${!service.configured || service.isSyncing ? "disabled" : ""}>
                            <i class="fas fa-plug"></i> Connect
                        </button>
                        <button class="btn btn-action ${service.connected ? "btn-reset" : "btn-disabled"}"
                                onclick="${service.connected ? `resetConnection('${service.service}', this)` : ""}"
                                ${!service.configured || service.isSyncing ? "disabled" : ""}>
                            <i class="fas fa-unlink"></i> Reset
                        </button>
                    </div>
                    <div class="button-row">
                        <button class="btn btn-action btn-sync-now"
                                onclick="syncNow('${service.service}', this)"
                                ${!service.configured || !service.connected || service.isSyncing ? "disabled" : ""}>
                            <i class="fas fa-sync-alt"></i> Sync Now
                        </button>
                    </div>
                    <div class="button-row">
                        <button class="btn btn-action btn-fetch-counts"
                                onclick="fetchCounts('${service.service}', this)"
                                ${!service.configured || !service.connected || service.isSyncing ? "disabled" : ""}>
                            <i class="fas fa-chart-bar"></i> Fetch Counts
                        </button>
                    </div>
                    <div class="button-row">
                        <button class="btn btn-action btn-show-playlists"
                                onclick="showPlaylists('${service.service}', this)"
                                ${service.service !== "spotify" || !service.configured || !service.connected || service.isSyncing ? "disabled" : ""}>
                            <i class="fas fa-list"></i> Show Playlists
                        </button>
                    </div>
                    <div class="button-row">
                        <button class="btn btn-action btn-configure" onclick="openConfigModal('${service.service}')">
                            <i class="fas fa-cog"></i> Configure
                        </button>
                    </div>
                </div>
                ${lastSyncHtml}
            </div>
        `;
  });

  html += "</div>";
  servicesContainer.innerHTML = html;
}

async function startAuth(service, button) {
  try {
    // Store original button state and disable during OAuth process
    let originalButtonHTML = null;
    if (button) {
      originalButtonHTML = button.innerHTML;
      button.disabled = true;
      button.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Connecting...';
    }

    const result = await fetch(
      `${window.API_BASE_USED || API_BASE}/services/${service}/auth`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
      },
    );

    if (!result.ok) {
      const errorData = await result.json();
      throw new Error(errorData.data || `HTTP ${result.status}`);
    }

    const data = await result.json();
    // Redirect to OAuth URL
    window.location.href = data.data;
  } catch (error) {
    showError(`Failed to start OAuth for ${service}: ${error.message}`);
    // Restore button state on error
    if (button && originalButtonHTML) {
      button.innerHTML = originalButtonHTML;
      button.disabled = false;
    }
  }
}

async function resetConnection(service, button) {
  try {
    if (button) {
      const originalHTML = button.innerHTML;
      button.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Resetting...';
      button.disabled = true;
    }

    const result = await fetch(
      `${window.API_BASE_USED || API_BASE}/services/${service}/reset`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
      },
    );

    if (!result.ok) {
      const errorData = await result.json();
      throw new Error(errorData.data || `HTTP ${result.status}`);
    }

    const data = await result.json();
    showError(`✅ ${data.data}`);

    // Reload services to update status
    setTimeout(() => loadServices(), 1000);
  } catch (error) {
    showError(`Failed to reset ${service}: ${error.message}`);
  }
}

async function syncNow(service, button) {
  try {
    if (button) {
      const originalHTML = button.innerHTML;
      button.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Syncing...';
      button.disabled = true;
    }

    const result = await fetch(
      `${window.API_BASE_USED || API_BASE}/services/${service}/sync`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
      },
    );

    if (!result.ok) {
      const errorData = await result.json();
      throw new Error(errorData.data || `HTTP ${result.status}`);
    }

    const data = await result.json();
    showError(`✅ ${data.data}`);

    // Reload services to update status
    setTimeout(() => loadServices(), 1000);
  } catch (error) {
    showError(`Failed to sync ${service}: ${error.message}`);
  }
}

async function fetchCounts(service, button) {
  try {
    if (button) {
      const originalHTML = button.innerHTML;
      button.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Fetching...';
      button.disabled = true;
    }

    const result = await fetch(
      `${window.API_BASE_USED || API_BASE}/services/${service}/fetch-counts`,
      {
        method: "GET",
        headers: { "Content-Type": "application/json" },
      },
    );

    if (!result.ok) {
      const errorData = await result.json();
      throw new Error(errorData.data || `HTTP ${result.status}`);
    }

    const data = await result.json();
    showError(`✅ ${data.data.message || data.data}`);

    // Reload services to update counts
    setTimeout(() => loadServices(), 1000);
  } catch (error) {
    showError(`Failed to fetch counts for ${service}: ${error.message}`);
  } finally {
    if (button) {
      // Restore button after a short delay
      setTimeout(() => {
        button.innerHTML = '<i class="fas fa-chart-bar"></i> Fetch Counts';
        button.disabled = false;
      }, 1000);
    }
  }
}

async function openConfigModal(service) {
  currentService = service;
  modalTitle.textContent = `Configure ${service.charAt(0).toUpperCase() + service.slice(1)}`;

  try {
    const config = await fetchJSON(
      `${window.API_BASE_USED || API_BASE}/services/${service}/config`,
    );
    const serviceData = servicesData.find((s) => s.service === service);

    let envStatus = "";
    if (serviceData) {
      envStatus = serviceData.configured
        ? `<div class="env-status">
                    <i class="fas fa-check-circle"></i> Service is configured via .env file
                </div>`
        : `<div class="env-status warning">
                    <i class="fas fa-exclamation-triangle"></i> Service not configured - add credentials to .env file
                </div>`;
    }

    let fields = "";
    if (service === "spotify") {
      fields = `
                ${envStatus}
                <div class="form-group">
                    <label>Redirect URI (for Spotify Developer Dashboard)</label>
                    <input type="text" value="http://localhost:3000/callback" readonly>
                    <small style="color: #94a3b8; display: block; margin-top: 5px;">
                        Add this exact URI to your Spotify app settings
                    </small>
                </div>
            `;
    } else if (service === "soundcloud") {
      fields = `
                ${envStatus}
                <div class="form-group">
                    <label>User ID (optional)</label>
                    <input type="text" id="user-id" value="${config.userId || ""}"
                           placeholder="Enter SoundCloud User ID for personal playlists">
                </div>
            `;
    } else if (service === "youtube") {
      fields = `
                ${envStatus}
                <div class="form-group">
                    <label>Playlist ID (optional)</label>
                    <input type="text" id="playlist-id" value="${config.playlistId || ""}"
                           placeholder="Enter YouTube Playlist ID">
                </div>
            `;
    }

    modalBody.innerHTML = `
            ${fields}
            <div class="modal-actions">
                <button class="btn btn-cancel" onclick="closeModal()">Cancel</button>
                <button class="btn btn-save" onclick="saveConfig()">Save</button>
            </div>
        `;

    configModal.style.display = "flex";
  } catch (error) {
    showError(`Failed to load config for ${service}: ${error.message}`);
  }
}

async function saveConfig() {
  if (!currentService) return;

  const userIdInput = document.getElementById("user-id");
  const playlistIdInput = document.getElementById("playlist-id");

  const config = {};
  if (userIdInput) config.user_id = userIdInput.value || null;
  if (playlistIdInput) config.playlist_id = playlistIdInput.value || null;

  try {
    await fetchJSON(
      `${window.API_BASE_USED || API_BASE}/services/${currentService}/config`,
      {
        method: "PUT",
        body: JSON.stringify(config),
      },
    );

    closeModal();
    loadServices(); // Refresh services list
  } catch (error) {
    showError(`Failed to save config: ${error.message}`);
  }
}

function closeModal() {
  configModal.style.display = "none";
  currentService = null;
}

// Close modal on outside click
configModal.addEventListener("click", (e) => {
  if (e.target === configModal) {
    closeModal();
  }
});

// Debug functions
function debugLog(message) {
  const logDiv = document.getElementById("debug-log");
  if (logDiv) {
    const entry = document.createElement("div");
    entry.textContent = `[${new Date().toLocaleTimeString()}] ${message}`;
    logDiv.prepend(entry);
    // Keep only last 10 entries
    while (logDiv.children.length > 10) {
      logDiv.removeChild(logDiv.lastChild);
    }
  }
  console.log(`[Debug] ${message}`);
}

function updateDebugStatus(endpoint, status, color = "#94a3b8") {
  const elem = document.getElementById(`debug-${endpoint}`);
  if (elem) {
    elem.textContent = `${endpoint}: ${status}`;
    elem.style.color = color;
  }
}

function updateDebugResponse(data) {
  const elem = document.getElementById("debug-response");
  if (elem) {
    elem.textContent = JSON.stringify(data, null, 2);
  }
}

async function testConnection(url, endpointName) {
  debugLog(`Testing ${url}`);
  updateDebugStatus(endpointName, "Testing...", "#fbbf24");
  try {
    const response = await fetch(url);
    if (response.ok) {
      const data = await response.json();
      updateDebugStatus(endpointName, `OK: ${data.status}`, "#10b981");
      updateDebugResponse(data);
      debugLog(`${url}: SUCCESS`);
      return true;
    } else {
      updateDebugStatus(endpointName, `HTTP ${response.status}`, "#ef4444");
      debugLog(`${url}: HTTP ${response.status}`);
      return false;
    }
  } catch (error) {
    updateDebugStatus(endpointName, `Error: ${error.message}`, "#ef4444");
    debugLog(`${url}: ${error.message}`);
    return false;
  }
}

// Initialize debug panel
function initDebugPanel() {
  const debugPanel = document.getElementById("debug-panel");
  const testLocalhostBtn = document.getElementById("debug-test-localhost");
  const test127Btn = document.getElementById("debug-test-127");
  const clearBtn = document.getElementById("debug-clear");

  // Test buttons
  testLocalhostBtn?.addEventListener("click", () => {
    testConnection("http://localhost:3000/api/health", "localhost");
  });

  test127Btn?.addEventListener("click", () => {
    testConnection("http://127.0.0.1:3000/api/health", "127");
  });

  clearBtn?.addEventListener("click", () => {
    const logDiv = document.getElementById("debug-log");
    if (logDiv) logDiv.innerHTML = "<div>Log cleared</div>";
  });

  // Show debug panel by default
  debugPanel.style.display = "none";

  // Run initial tests
  setTimeout(() => {
    testConnection("http://localhost:3000/api/health", "localhost");
    testConnection("http://127.0.0.1:3000/api/health", "127");
  }, 1000);
}
// Initialize
document.addEventListener("DOMContentLoaded", () => {
  initDebugPanel();
  apiUrlElement.textContent = "http://localhost:3000/api";
  loadServices();
});

// Auto-refresh every 30 seconds
setInterval(() => {
  if (!configModal.style.display || configModal.style.display === "none") {
    loadServices();
  }

  // Navigate to playlists page for a service
  async function showPlaylists(service, button) {
    const originalHTML = button.innerHTML;
    button.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Loading...';
    button.disabled = true;

    try {
      // For now, just navigate to playlists.html (Spotify only)
      if (service === "spotify") {
        window.location.href = "playlists.html";
      } else {
        alert("Playlist view is only available for Spotify at the moment");
      }
    } catch (error) {
      console.error("Failed to show playlists:", error);
      alert("Failed to load playlists: " + error.message);
    } finally {
      button.innerHTML = originalHTML;
      button.disabled = false;
    }
  }
}, 30000);
