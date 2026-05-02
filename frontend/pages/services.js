/**
 * services.js — Service connections page.
 *
 * Full interactive: OAuth auth, config modal, sync, reset, fetch-counts,
 * and sync status polling.
 */

import { fetchJSON } from "../shared/api.js";
import {
  renderLoading,
  renderErrorBlock,
  renderTable,
  td,
} from "../shared/components.js";
import { formatNumber } from "../shared/format.js";

/* ------------------------------------------------------------------ */
/*  Service Metadata                                                   */
/* ------------------------------------------------------------------ */

const SERVICE_META = {
  spotify: { name: "Spotify", icon: "fa-brands fa-spotify" },
  soundcloud: { name: "SoundCloud", icon: "fa-brands fa-soundcloud" },
  youtube: { name: "YouTube", icon: "fa-brands fa-youtube" },
  deemix: { name: "Deemix", icon: "fa-solid fa-download" },
};

const SERVICE_COLORS = {
  spotify: "#1db954",
  soundcloud: "#ff7700",
  youtube: "#ff0000",
  deemix: "#8b5cf6",
};

/* ------------------------------------------------------------------ */
/*  State                                                              */
/* ------------------------------------------------------------------ */

let state = {
  services: [],
  pollTimer: null,
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
/*  Adapter                                                            */
/* ------------------------------------------------------------------ */

function adaptService(s) {
  const meta = SERVICE_META[s.service] || { name: s.service, icon: "fa-solid fa-cloud" };
  let status, statusLabel;
  if (!s.configured) {
    status = "unconfigured";
    statusLabel = "Not Configured";
  } else if (!s.connected) {
    status = "disconnected";
    statusLabel = "Auth Needed";
  } else {
    status = "connected";
    statusLabel = "Connected";
  }

  return {
    id: s.service,
    name: meta.name,
    icon: meta.icon,
    configured: s.configured,
    connected: s.connected,
    status,
    statusLabel,
    clientId: "—",
    playlists: s.playlistsLocal ?? 0,
    tracks: s.tracksLocal ?? 0,
    playlistsRemote: s.playlistsRemote ?? 0,
    tracksRemote: s.tracksRemote ?? 0,
    syncing: s.isSyncing || false,
    lastSync: s.lastSync || null,
    syncCurrentPlaylist: s.syncCurrentPlaylist,
    syncTotalPlaylists: s.syncTotalPlaylists,
    syncCurrentTrack: s.syncCurrentTrack,
    syncTotalTracks: s.syncTotalTracks,
    syncLog: s.syncLog,
  };
}

/* ------------------------------------------------------------------ */
/*  Action handlers                                                    */
/* ------------------------------------------------------------------ */

async function authorizeService(service) {
  const btn = document.querySelector(`[data-action="authorize"][data-id="${service}"]`);
  if (!btn) return;
  const originalHtml = btn.innerHTML;
  btn.disabled = true;
  btn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i> Connecting...';

  try {
    const resp = await fetchJSON(`/api/services/${service}/auth`, { method: "POST" });
    // resp.data contains the redirect URL
    window.location.href = resp.data;
  } catch (err) {
    showError(`OAuth failed: ${err.message}`);
    btn.disabled = false;
    btn.innerHTML = originalHtml;
  }
}

async function resyncService(service) {
  const btn = document.querySelector(`[data-action="resync"][data-id="${service}"]`);
  if (!btn) return;
  const originalHtml = btn.innerHTML;
  btn.disabled = true;
  btn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i> Syncing...';

  try {
    const resp = await fetchJSON(`/api/services/${service}/sync`, { method: "POST" });
    showSuccess(resp.data || "Sync started");
    // Start polling for sync progress
    startPolling();
    // Reload after a moment
    setTimeout(() => loadServices(), 1500);
  } catch (err) {
    showError(`Sync failed: ${err.message}`);
    btn.disabled = false;
    btn.innerHTML = originalHtml;
  }
}

async function resetService(service) {
  if (
    !confirm(
      `Reset ${SERVICE_META[service]?.name || service} connection? This will clear the access token.`,
    )
  ) {
    return;
  }

  const btn = document.querySelector(`[data-action="reset"][data-id="${service}"]`);
  if (!btn) return;
  const originalHtml = btn.innerHTML;
  btn.disabled = true;
  btn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i> Resetting...';

  try {
    const resp = await fetchJSON(`/api/services/${service}/reset`, { method: "POST" });
    showSuccess(resp.data || "Connection reset");
    setTimeout(() => loadServices(), 1000);
  } catch (err) {
    showError(`Reset failed: ${err.message}`);
    btn.disabled = false;
    btn.innerHTML = originalHtml;
  }
}

async function fetchCounts(service) {
  const btn = document.querySelector(
    `[data-action="fetch-counts"][data-id="${service}"]`,
  );
  if (!btn) return;
  const originalHtml = btn.innerHTML;
  btn.disabled = true;
  btn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i> Fetching...';

  try {
    await fetchJSON(`/api/services/${service}/fetch-counts`);
    showSuccess("Counts updated");
    setTimeout(() => loadServices(), 1000);
  } catch (err) {
    showError(`Fetch counts failed: ${err.message}`);
    btn.disabled = false;
    btn.innerHTML = originalHtml;
  }
}

/* ------------------------------------------------------------------ */
/*  Config Modal                                                       */
/* ------------------------------------------------------------------ */

async function openConfigModal(service) {
  const meta = SERVICE_META[service] || { name: service };

  let envStatus = "";
  let configFields = "";

  try {
    const svc = state.services.find((s) => s.id === service);
    envStatus = svc?.configured
      ? `<div style="padding:8px 12px;background:rgba(34,197,94,0.1);border:1px solid rgba(34,197,94,0.2);border-radius:6px;margin-bottom:16px;color:var(--green);font-size:0.85rem;">
          <i class="fa-solid fa-check-circle"></i> Configured via .env file
         </div>`
      : `<div style="padding:8px 12px;background:rgba(245,158,11,0.1);border:1px solid rgba(245,158,11,0.2);border-radius:6px;margin-bottom:16px;color:var(--yellow);font-size:0.85rem;">
          <i class="fa-solid fa-exclamation-triangle"></i> Not configured — add credentials to .env file
         </div>`;

    // Fetch current config
    let currentConfig = {};
    try {
      const configResp = await fetchJSON(`/api/services/${service}/config`);
      currentConfig = configResp.data || {};
    } catch {
      // Config endpoint may fail if not configured, that's ok
    }

    if (service === "spotify") {
      configFields = `
        <div class="form-group">
          <label>Redirect URI (for Spotify Developer Dashboard)</label>
          <input type="text" class="input-text w-full" value="http://localhost:3000/callback" readonly
                 style="background:var(--surface);color:var(--text-muted);cursor:default;">
          <small style="color:var(--text-muted);display:block;margin-top:4px;font-size:0.8rem;">
            Add this exact URI to your Spotify app settings
          </small>
        </div>`;
    } else if (service === "soundcloud") {
      configFields = `
        <div class="form-group">
          <label>SoundCloud User ID (for personal playlists)</label>
          <input type="text" class="input-text w-full" id="config-user-id"
                 value="${currentConfig.userId || ""}"
                 placeholder="Enter your SoundCloud user ID">
          <small style="color:var(--text-muted);display:block;margin-top:4px;font-size:0.8rem;">
            Required to sync your personal playlists
          </small>
        </div>`;
    } else if (service === "youtube") {
      configFields = `
        <div class="form-group">
          <label>YouTube Playlist ID (optional)</label>
          <input type="text" class="input-text w-full" id="config-playlist-id"
                 value="${currentConfig.playlistId || ""}"
                 placeholder="Enter a YouTube playlist ID">
          <small style="color:var(--text-muted);display:block;margin-top:4px;font-size:0.8rem;">
            Sync a specific playlist
          </small>
        </div>`;
    } else if (service === "deemix") {
      // No env status for deemix — it's configured entirely via the web UI
      envStatus = "";
      // Parse stored metadata JSON (API returns snake_case + metadata_json is a JSON string)
      let currentHost = "http://localhost:6596";
      const storedArl = currentConfig.access_token || currentConfig.accessToken || "";
      try {
        const meta = JSON.parse(currentConfig.metadata_json || "{}");
        if (meta.host) currentHost = meta.host;
      } catch {
        /* use default */
      }
      configFields = `
        <div class="form-group">
          <label>ARL (Account Registration Link)</label>
          <input type="password" class="input-text w-full" id="deemix-arl"
                 value="${storedArl}"
                 placeholder="Enter your deemix ARL...">
          <small style="color:var(--text-muted);display:block;margin-top:4px;font-size:0.8rem;">
            Your deemix ARL cookie value — stored securely in the database
          </small>
        </div>
        <div class="form-group">
          <label>Deemix Web API Host</label>
          <input type="text" class="input-text w-full" id="deemix-host"
                 value="${currentHost}"
                 placeholder="http://localhost:6596">
          <small style="color:var(--text-muted);display:block;margin-top:4px;font-size:0.8rem;">
            Host where deemix-pyweb is running (default port 6596)
          </small>
        </div>`;
    } else {
      configFields = `<p style="color:var(--text-muted);">No additional configuration available for ${meta.name}.</p>`;
    }
  } catch (err) {
    showError(`Failed to load config: ${err.message}`);
    return;
  }

  const modalHtml = `
    <div class="modal open" id="service-config-modal">
      <div class="modal-content" style="max-width:520px">
        <div class="modal-header">
          <h3><i class="${meta.icon}" style="margin-right:8px;color:${SERVICE_COLORS[service] || "var(--accent)"}"></i> Configure ${meta.name}</h3>
          <button class="close-btn" id="config-modal-close">&times;</button>
        </div>
        <form id="config-form" onsubmit="return false;">
          ${envStatus}
          ${configFields}
          <div class="modal-actions">
            <button type="button" class="btn" id="config-modal-cancel">Cancel</button>
            <button type="submit" class="btn btn-primary" id="config-modal-save">
              <i class="fa-solid fa-floppy-disk"></i> Save
            </button>
          </div>
        </form>
      </div>
    </div>
  `;

  const overlay = document.createElement("div");
  overlay.innerHTML = modalHtml;
  document.body.appendChild(overlay.firstElementChild);

  // Wire modal events
  const modal = document.getElementById("service-config-modal");
  const doClose = () => {
    modal?.classList.remove("open");
    modal?.remove();
  };

  document.getElementById("config-modal-close")?.addEventListener("click", doClose);
  document.getElementById("config-modal-cancel")?.addEventListener("click", doClose);
  modal?.addEventListener("click", (e) => {
    if (e.target === modal) doClose();
  });

  document.addEventListener("keydown", function escHandler(e) {
    if (e.key === "Escape") {
      doClose();
      document.removeEventListener("keydown", escHandler);
    }
  });

  // Save handler
  document.getElementById("config-form")?.addEventListener("submit", async (e) => {
    e.preventDefault();

    const saveBtn = document.getElementById("config-modal-save");
    const originalHtml = saveBtn.innerHTML;
    saveBtn.disabled = true;
    saveBtn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i> Saving...';

    try {
      if (service === "deemix") {
        const arlInput = document.getElementById("deemix-arl");
        const hostInput = document.getElementById("deemix-host");
        await fetchJSON("/api/services/deemix/auth", {
          method: "POST",
          body: JSON.stringify({
            arl: arlInput?.value || "",
            host: hostInput?.value || "http://localhost:6596",
          }),
        });
        showSuccess("Deemix configured and connected!");
      } else {
        const userIdInput = document.getElementById("config-user-id");
        const playlistIdInput = document.getElementById("config-playlist-id");

        const configData = {};
        if (userIdInput) configData.userId = userIdInput.value || null;
        if (playlistIdInput) configData.playlistId = playlistIdInput.value || null;

        await fetchJSON(`/api/services/${service}/config`, {
          method: "PUT",
          body: JSON.stringify(configData),
        });
        showSuccess("Configuration saved");
      }
      doClose();
      setTimeout(() => loadServices(), 500);
    } catch (err) {
      showError(`Failed to save config: ${err.message}`);
      saveBtn.disabled = false;
      saveBtn.innerHTML = originalHtml;
    }
  });
}

/* ------------------------------------------------------------------ */
/*  Sync polling                                                       */
/* ------------------------------------------------------------------ */

function startPolling() {
  if (state.pollTimer) return;

  state.pollTimer = setInterval(async () => {
    try {
      const resp = await fetchJSON("/api/services");
      const services = resp.data.map(adaptService);

      // Check if any service is still syncing
      const anySyncing = services.some((s) => s.syncing);

      if (anySyncing) {
        // Update services in-place without full re-render
        state.services = services;
        updateSyncStatus(services);
      } else {
        // All done — stop polling and reload
        stopPolling();
        await loadServices();
      }
    } catch {
      stopPolling();
    }
  }, 2000);
}

function stopPolling() {
  if (state.pollTimer) {
    clearInterval(state.pollTimer);
    state.pollTimer = null;
  }
}

function updateSyncStatus(services) {
  // Update the sync status badges without re-rendering the whole page
  for (const svc of services) {
    const row = document.querySelector(`tr[data-service-id="${svc.id}"]`);
    if (!row) continue;

    const actionsCell = row.querySelector("td:last-child");
    if (actionsCell) {
      if (svc.syncing) {
        actionsCell.innerHTML = `
          <div class="flex items-center gap-2">
            <span class="status-badge running"><i class="fa-solid fa-spinner fa-spin"></i> Syncing</span>
          </div>`;
      } else {
        const color = SERVICE_COLORS[svc.id] || "var(--text-muted)";
        actionsCell.innerHTML = `
          <div class="flex items-center gap-2">
            <button class="btn btn-sm btn-green" data-action="resync" data-id="${svc.id}">
              <i class="fa-solid fa-rotate"></i> Re-sync
            </button>
            <button class="btn btn-sm" data-action="fetch-counts" data-id="${svc.id}">
              <i class="fa-solid fa-chart-bar"></i>
            </button>
            <button class="btn btn-sm btn-red" data-action="reset" data-id="${svc.id}">
              <i class="fa-solid fa-unlink"></i>
            </button>
          </div>`;
      }
    }
  }
}

/* ------------------------------------------------------------------ */
/*  Render                                                             */
/* ------------------------------------------------------------------ */

function renderServiceRow(s) {
  const color = SERVICE_COLORS[s.id] || "var(--text-muted)";

  let actionHtml;
  if (s.id === "deemix") {
    if (s.status === "unconfigured") {
      actionHtml = `<button class="btn btn-sm btn-green" data-action="configure" data-id="${s.id}" title="Configure Deemix"><i class="fa-solid fa-download"></i> Configure</button>`;
    } else {
      actionHtml = `
      <div class="flex items-center gap-2" style="flex-wrap:nowrap">
        <button class="btn btn-sm" data-action="configure" data-id="${s.id}" title="Reconfigure"><i class="fa-solid fa-gear"></i> Reconfigure</button>
        <button class="btn btn-sm btn-green" data-action="test-deemix" data-id="${s.id}" title="Test connection"><i class="fa-solid fa-flask"></i> Test</button>
        <button class="btn btn-sm btn-red" data-action="reset" data-id="${s.id}" title="Disconnect"><i class="fa-solid fa-unlink"></i> Disconnect</button>
      </div>`;
    }
  } else if (s.status === "unconfigured") {
    actionHtml = `<button class="btn btn-sm btn-primary" data-action="configure" data-id="${s.id}" title="Configure service"><i class="fa-solid fa-plus"></i></button>`;
  } else if (s.status === "disconnected") {
    actionHtml = `
      <div class="flex items-center gap-2" style="flex-wrap:nowrap">
        <button class="btn btn-sm btn-yellow" data-action="authorize" data-id="${s.id}" title="Authorize service"><i class="fa-solid fa-plug"></i></button>
        <button class="btn btn-sm" data-action="configure" data-id="${s.id}" title="Configure"><i class="fa-solid fa-gear"></i></button>
      </div>`;
  } else if (s.syncing) {
    actionHtml = `<span class="status-badge running"><i class="fa-solid fa-spinner fa-spin"></i> Syncing</span>`;
  } else {
    actionHtml = `
      <div class="flex items-center gap-2" style="flex-wrap:nowrap">
        <button class="btn btn-sm btn-green" data-action="resync" data-id="${s.id}" title="Re-sync now"><i class="fa-solid fa-rotate"></i></button>
        <button class="btn btn-sm" data-action="fetch-counts" data-id="${s.id}" title="Fetch remote counts"><i class="fa-solid fa-chart-bar"></i></button>
        <button class="btn btn-sm" data-action="configure" data-id="${s.id}" title="Configure"><i class="fa-solid fa-gear"></i></button>
        <button class="btn btn-sm btn-red" data-action="reset" data-id="${s.id}" title="Reset connection"><i class="fa-solid fa-unlink"></i></button>
      </div>`;
  }

  const syncInfoHtml = s.lastSync
    ? `<span style="color:var(--text-muted);font-size:0.85rem;">${new Date(s.lastSync * 1000).toLocaleString()}</span>`
    : '<span class="status-badge pending">Never</span>';

  return `<tr data-service-id="${s.id}">
    ${td(
      `<span class="service-badge ${s.id}" style="background:${color}22;color:${color};">
        <i class="${s.icon}"></i> ${s.name}
      </span>`,
    )}
    ${td(`<span class="status-badge ${s.status}">${s.statusLabel}</span>`)}

    ${td(`<div style="text-align:center">${formatNumber(s.playlists)} <small style="color:var(--text-muted)">/ ${formatNumber(s.playlistsRemote)}</small></div>`)}
    ${td(`<div style="text-align:center">${formatNumber(s.tracks)} <small style="color:var(--text-muted)">/ ${formatNumber(s.tracksRemote)}</small></div>`)}
    ${td(syncInfoHtml)}
    ${td(`<div class="flex items-center">${actionHtml}</div>`)}
  </tr>`;
}

function render(container, data) {
  const headers = [
    { label: "Service", style: "width:15%" },
    { label: "Status", style: "width:20%" },
    { label: "Playlists", style: "width:12%;text-align:center" },
    { label: "Tracks", style: "width:12%;text-align:center" },
    { label: "Last Full Sync", style: "width:18%" },
    { label: "Actions", style: "width:23%" },
  ];
  const rows = data.services.map(renderServiceRow).join("");
  const unconfigured = data.services.filter((s) => s.status === "unconfigured").length;

  container.innerHTML = `
    <div class="toolbar">
      <div class="flex items-center gap-2">
${
  data.services.some((s) => s.syncing)
    ? `<span class="status-badge running"><i class="fa-solid fa-spinner fa-spin"></i> Sync in progress...</span>`
    : ""
}
      </div>
    </div>

    <div class="stats-row">
      <div class="stats-group"><strong>${data.total}</strong> total services</div>
      <div class="stats-group"><strong>${data.connected}</strong> connected</div>
      <div class="stats-group"><strong>${unconfigured}</strong> unconfigured</div>
      <div class="stats-group"><strong>${data.services.reduce((s, svc) => s + svc.tracks, 0)}</strong> local tracks</div>
    </div>

    ${renderTable(headers, rows)}

    <div class="card" style="margin-top:var(--space-6);padding:var(--space-6);">
      <h3 style="margin-bottom:var(--space-3);font-size:1rem;">
        <i class="fa-solid fa-gear" style="margin-right:var(--space-2);color:var(--accent);"></i>Configuration
      </h3>
      <p style="color:var(--text-muted);margin-bottom:var(--space-3);">
        Service credentials are stored in your <code style="color:var(--accent);">.env</code> file
        at the project root. Restart the backend after making changes.
      </p>
      <pre style="background:var(--surface);padding:var(--space-4);border-radius:var(--radius-md);border:1px solid var(--border);font-size:0.85rem;overflow-x:auto;"><code># Spotify
SPOTIFY_CLIENT_ID=your_client_id
SPOTIFY_CLIENT_SECRET=your_client_secret

# SoundCloud
SOUNDCLOUD_CLIENT_ID=your_client_id
SOUNDCLOUD_CLIENT_SECRET=your_client_secret

# YouTube
YOUTUBE_API_KEY=your_api_key</code></pre>
      <p style="color:var(--text-muted);margin-top:var(--space-3);font-size:0.85rem;">
        <i class="fa-solid fa-download" style="margin-right:var(--space-1);color:var(--accent);"></i>
        <strong>Deemix</strong> is configured entirely via the web UI — click Configure on the Deemix row above.
      </p>
    </div>
  `;

  // Wire events
  wireEvents(container);
}

/* ------------------------------------------------------------------ */
/*  Event wiring                                                       */
/* ------------------------------------------------------------------ */

function wireEvents(container) {
  // Event delegation for all action buttons
  container.addEventListener("click", (e) => {
    const btn = e.target.closest("[data-action]");
    if (!btn) return;
    e.preventDefault();

    const action = btn.dataset.action;
    const id = btn.dataset.id;

    switch (action) {
      case "authorize":
        authorizeService(id);
        break;
      case "resync":
        resyncService(id);
        break;
      case "reset":
        resetService(id);
        break;
      case "configure":
        openConfigModal(id);
        break;
      case "fetch-counts":
        fetchCounts(id);
        break;
      case "test-deemix":
        testDeemixConnection(btn, id);
        break;
    }
  });
}

async function testDeemixConnection(btn, id) {
  const originalHtml = btn.innerHTML;
  btn.disabled = true;
  btn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i> Testing…';
  try {
    const resp = await fetchJSON("/api/services/deemix/queue");
    const items = resp.data || [];
    showSuccess(
      `Deemix connected — ${items.length} item${items.length !== 1 ? "s" : ""} in queue`,
    );
  } catch (err) {
    showError(`Deemix test failed: ${err.message}`);
  } finally {
    btn.disabled = false;
    btn.innerHTML = originalHtml;
  }
}

/* ------------------------------------------------------------------ */
/*  Data loading                                                       */
/* ------------------------------------------------------------------ */

async function loadServices() {
  const container = document.getElementById("main-content");
  if (!container) return;

  try {
    const resp = await fetchJSON("/api/services");
    const services = resp.data.map(adaptService);
    state.services = services;

    const data = {
      total: services.length,
      connected: services.filter((s) => s.connected).length,
      services,
    };

    render(container, data);

    // Start polling if any service is syncing
    if (services.some((s) => s.syncing)) {
      startPolling();
    } else {
      stopPolling();
    }
  } catch (err) {
    container.innerHTML = renderErrorBlock({
      title: "Failed to load services",
      detail: err.message,
      retryFn: "window.location.hash='#services'",
    });
  }
}

/* ------------------------------------------------------------------ */
/*  Initialisation                                                     */
/* ------------------------------------------------------------------ */

export async function init(container, signal) {
  container.innerHTML = renderLoading("Loading services…");
  stopPolling();

  try {
    const resp = await fetchJSON("/api/services", { signal });
    if (signal.aborted) return;

    const services = resp.data.map(adaptService);
    state.services = services;

    const data = {
      total: services.length,
      connected: services.filter((s) => s.connected).length,
      services,
    };

    render(container, data);

    // Start polling if any service is syncing
    if (services.some((s) => s.syncing)) {
      startPolling();
    }

    // Visibility change — reload when user comes back
    document.addEventListener(
      "visibilitychange",
      () => {
        if (!document.hidden) {
          loadServices();
        }
      },
      { signal },
    );
  } catch (err) {
    if (err.name === "AbortError") return;
    container.innerHTML = renderErrorBlock({
      title: "Failed to load services",
      detail: err.message,
      retryFn: "window.location.hash='#services'",
    });
  }
}
