import { fetchJSON } from "./shared/api.js";
import { useErrorBanner, renderLoading, renderEmpty } from "./shared/components.js";
import { formatNumber } from "./shared/format.js";
import { renderNav } from "./shared/nav.js";

renderNav("index");

let pollInterval = null;

const errorBanner = useErrorBanner(document.getElementById("error-message"));
const statsContainer = document.getElementById("stats-container");
const backendStatus = document.getElementById("backend-status");
const backendDot = document.getElementById("backend-dot");
const refreshBtn = document.getElementById("refresh-btn");
const debugToggleBtn = document.getElementById("debug-toggle-btn");
const syncStatusDiv = document.getElementById("sync-status");
const syncStatusText = document.getElementById("sync-status-text");
const syncStatusProgress = document.getElementById("sync-status-progress");

async function loadStats() {
  try {
    const health = await fetchJSON("/health");
    backendDot.style.background = "var(--green)";
    backendStatus.textContent = "Backend connected";
  } catch {
    backendDot.style.background = "var(--red)";
    backendStatus.textContent = "Backend not connected";
    statsContainer.innerHTML = renderEmpty({
      icon: "exclamation-triangle",
      title: "Backend Unavailable",
      message: "Make sure the server is running on port 3000.",
      actionHtml: "<button class="btn btn-primary" onclick="window.location.reload()"><i class="fas fa-redo"></i> Retry</button>"
    });
    return;
  }

  try {
    const data = await fetchJSON("/stats");
    const s = data.data || data;
    let html = "<div class="card"><div class="stats-row" style="margin-bottom:0;"><div class="stats-group">";
    if (s.totalFiles != null) html += "<span>Files: <strong>" + formatNumber(s.totalFiles) + "</strong></span>";
    if (s.totalTracks != null) html += "<span>Tracks: <strong>" + formatNumber(s.totalTracks) + "</strong></span>";
    if (s.totalPlaylists != null) html += "<span>Playlists: <strong>" + formatNumber(s.totalPlaylists) + "</strong></span>";
    if (s.totalFolders != null) html += "<span>Folders: <strong>" + formatNumber(s.totalFolders) + "</strong></span>";
    if (s.totalTags != null) html += "<span>Tags: <strong>" + formatNumber(s.totalTags) + "</strong></span>";
    html += "</div></div></div>";
    statsContainer.innerHTML = html;
  } catch (err) {
    statsContainer.innerHTML = "<div class="card"><p class="text-muted" style="text-align:center;">Could not load stats: " + err.message + "</p></div>";
  }
}

function startPolling() {
  if (pollInterval) clearInterval(pollInterval);
  pollInterval = setInterval(async () => {
    try {
      const data = await fetchJSON("/services");
      const services = data.data || data;
      const anySyncing = services.some(function(s) { return s.syncing; });
      if (anySyncing) {
        syncStatusDiv.style.display = "block";
        syncStatusText.textContent = "Sync in progress...";
        syncStatusProgress.textContent = services.filter(function(s) { return s.syncing; }).map(function(s) { return s.service || s.name; }).join(", ") + " syncing";
      } else {
        syncStatusDiv.style.display = "none";
        if (pollInterval) {
          clearInterval(pollInterval);
          pollInterval = null;
        }
      }
    } catch {}
  }, 3000);
}

// Debug panel
const debugPanel = document.getElementById("debug-panel");

function debugLog(msg) {
  var logDiv = document.getElementById("debug-log");
  if (!logDiv) return;
  var entry = document.createElement("div");
  entry.textContent = "[" + new Date().toLocaleTimeString() + "] " + msg;
  logDiv.appendChild(entry);
  logDiv.scrollTop = logDiv.scrollHeight;
}

function updateDebugStatus(id, text) {
  var el = document.getElementById(id);
  if (el) el.textContent = text;
}

function updateDebugResponse(data) {
  var el = document.getElementById("debug-response");
  if (el) el.textContent = JSON.stringify(data, null, 2);
}

async function testConnection(url, label) {
  var id = label === "localhost" ? "debug-localhost" : "debug-127";
  updateDebugStatus(id, label + ": Testing...");
  try {
    var res = await fetch(url);
    var data = await res.json();
    updateDebugStatus(id, label + ": ✅ Connected (" + res.status + ")");
    updateDebugResponse(data);
    updateDebugStatus("debug-active", "Active endpoint: " + url);
    debugLog(label + ": " + res.status + " OK");
  } catch (err) {
    updateDebugStatus(id, label + ": ❌ " + err.message);
    debugLog(label + ": " + err.message);
  }
}

// Init
document.addEventListener("DOMContentLoaded", function() {
  refreshBtn.addEventListener("click", loadStats);

  debugToggleBtn.addEventListener("click", function() {
    debugPanel.style.display = debugPanel.style.display === "none" ? "block" : "none";
  });

  document.getElementById("debug-close-btn").addEventListener("click", function() {
    debugPanel.style.display = "none";
  });

  document.getElementById("debug-test-localhost").addEventListener("click", function() {
    testConnection("http://localhost:3000/api/health", "localhost");
  });

  document.getElementById("debug-test-127").addEventListener("click", function() {
    testConnection("http://127.0.0.1:3000/api/health", "127");
  });

  document.getElementById("debug-clear").addEventListener("click", function() {
    var logDiv = document.getElementById("debug-log");
    if (logDiv) logDiv.innerHTML = "<div>Log cleared</div>";
  });

  loadStats();
  startPolling();

  setTimeout(function() {
    testConnection("http://localhost:3000/api/health", "localhost");
    testConnection("http://127.0.0.1:3000/api/health", "127");
  }, 1000);
});

setInterval(function() {
  loadStats();
}, 30000);
