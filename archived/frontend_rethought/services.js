import { fetchJSON } from "./shared/api.js";
import {
  useErrorBanner,
  renderLoading,
  renderEmpty,
  renderErrorBlock,
  renderTable,
  td,
} from "./shared/components.js";
import { renderNav } from "./shared/nav.js";

renderNav("services");

let servicesData = [];
let pollInterval = null;

const servicesContent = document.getElementById("services-content");
const errorBanner = useErrorBanner(document.getElementById("error-message"));
const configModal = document.getElementById("config-modal");
const modalBody = document.getElementById("modal-body");
const modalTitle = document.getElementById("modal-title");

function escapeHtml(str) {
  if (typeof str !== "string") return str;
  return str
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

// ── Data loading ─────────────────────────────

async function loadServices() {
  servicesContent.innerHTML = renderLoading("Loading services...");

  try {
    const data = await fetchJSON("/services");
    servicesData = data.data || data;
    renderServices(servicesData);
  } catch (err) {
    servicesContent.innerHTML = renderErrorBlock({
      title: "Failed to load services",
      detail: err.message,
      retryFn: "loadServices()",
    });
  }
}

// ── Rendering ─────────────────────────────────

function renderServices(services) {
  if (!services || services.length === 0) {
    servicesContent.innerHTML = renderEmpty({
      icon: "cloud",
      title: "No services found",
      message: "No music services are configured yet.",
      actionHtml:
        '<p class="text-muted" style="margin-top: var(--space-2);">Add credentials via the .env file or use the Configure button below.</p>',
    });
    return;
  }

  const headers = ["Service", "Status", "Client ID", "Playlists", "Tracks", "Actions"];

  const rowsHtml = services
    .map((svc) => {
      const name = svc.service || svc.name || "unknown";
      const configured = !!(
        svc.configured ||
        svc.config?.client_id ||
        svc.config?.api_key
      );
      const connected = !!svc.connected;
      const syncing = !!svc.syncing;

      // Icon
      const iconMap = {
        spotify: ["fa-spotify", "fab", "#1db954"],
        soundcloud: ["fa-soundcloud", "fab", "#ff5500"],
        youtube: ["fa-youtube", "fab", "#ff0000"],
      };
      const [iconName, iconPrefix, iconColor] = iconMap[name] || [
        "fa-cloud",
        "fas",
        "#64748b",
      ];

      const serviceHtml = `<div style="display:flex;align-items:center;gap:var(--space-2);">
        <i class="${iconPrefix} ${iconName}" style="color:${iconColor};font-size:1.1rem;width:20px;text-align:center;"></i>
        <span style="font-weight:600;">${escapeHtml(
          name.charAt(0).toUpperCase() + name.slice(1),
        )}</span>
      </div>`;

      // Status badges
      const configuredBadge = configured
        ? `<span class="badge" style="background:rgba(16,185,129,0.15);color:#6ee7b7;border:1px solid rgba(16,185,129,0.3);">Configured</span>`
        : `<span class="badge" style="background:rgba(100,116,139,0.15);color:#94a3b8;border:1px solid rgba(100,116,139,0.3);">Not Configured</span>`;

      const connectedBadge = connected
        ? `<span class="badge" style="background:rgba(16,185,129,0.15);color:#6ee7b7;border:1px solid rgba(16,185,129,0.3);">Connected</span>`
        : `<span class="badge" style="background:rgba(239,68,68,0.1);color:#fca5a5;border:1px solid rgba(239,68,68,0.3);">Disconnected</span>`;

      const syncingBadge = syncing
        ? `<span class="badge" style="background:rgba(99,102,241,0.15);color:#a5b4fc;border:1px solid rgba(99,102,241,0.3);"><i class="fas fa-spinner fa-spin"></i> Syncing</span>`
        : "";

      // Client ID
      const clientId =
        svc.env_client_id || svc.config?.client_id || svc.config?.api_key || "\u2014";

      // Counts
      const playlistCount = svc.playlistCount ?? svc.playlists ?? "\u2014";
      const trackCount = svc.trackCount ?? svc.tracks ?? "\u2014";

      // Actions
      let actionsHtml = `<button class="btn btn-sm" onclick="window.openConfigModal('${name}')"><i class="fas fa-cog"></i> Configure</button>`;

      if (configured) {
        actionsHtml += `
          <button class="btn btn-primary btn-sm" onclick="window.startAuth('${name}')"><i class="fas fa-plug"></i> Connect</button>
          <button class="btn btn-sm btn-red" onclick="window.resetConnection('${name}')"><i class="fas fa-undo"></i> Reset</button>`;
      }

      if (configured && connected) {
        actionsHtml += `
          <button class="btn btn-green btn-sm" onclick="window.syncNow('${name}')"><i class="fas fa-sync-alt"></i> Sync</button>`;
      }

      return `<tr>
        ${td(serviceHtml)}
        ${td(`${configuredBadge} ${connectedBadge} ${syncingBadge}`)}
        ${td(escapeHtml(clientId))}
        ${td(`<strong>${playlistCount}</strong>`, { style: "text-align:center;" })}
        ${td(`<strong>${trackCount}</strong>`, { style: "text-align:center;" })}
        ${td(actionsHtml)}
      </tr>`;
    })
    .join("");

  servicesContent.innerHTML = renderTable(headers, rowsHtml);
}

// ── Config Modal ──────────────────────────────

const FIELD_CONFIGS = {
  spotify: ["client_id", "client_secret", "redirect_uri"],
  soundcloud: ["client_id", "client_secret"],
  youtube: ["api_key"],
};

window.openConfigModal = async function (name) {
  const service = servicesData.find((s) => (s.service || s.name) === name);
  const config = service?.config || {};
  const hasEnvClientId = service?.env_client_id;

  const envStatus = hasEnvClientId
    ? '<span class="badge" style="background:rgba(16,185,129,0.15);color:#6ee7b7;border:1px solid rgba(16,185,129,0.3);">.env client_id found</span>'
    : '<span class="badge" style="background:rgba(239,68,68,0.1);color:#fca5a5;border:1px solid rgba(239,68,68,0.3);">No .env client_id</span>';

  const fieldsList = FIELD_CONFIGS[name] || ["client_id", "client_secret"];

  let fieldsHtml = "";
  for (const field of fieldsList) {
    const label = field.replace(/_/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
    fieldsHtml += `<div class="form-group">
      <label>${label}</label>
      <input type="text" id="config-${field}" class="input-text" value="${
        config[field] || ""
      }" placeholder="${field}" />
    </div>`;
  }

  modalTitle.textContent = `Configure ${name.charAt(0).toUpperCase() + name.slice(1)}`;
  modalBody.innerHTML = `
    <div style="margin-bottom: var(--space-4); font-size: 0.85rem; color: var(--text-muted);">
      ${envStatus}
    </div>
    ${fieldsHtml}
    <div class="modal-actions">
      <button id="save-config-btn" class="btn btn-primary" style="flex:1;">
        <i class="fas fa-save"></i> Save
      </button>
      <button id="cancel-config-btn" class="btn" style="flex:1;">Cancel</button>
    </div>
  `;
  configModal.classList.add("open");

  document.getElementById("save-config-btn").addEventListener("click", async () => {
    const configData = {};
    for (const field of fieldsList) {
      configData[field] = document.getElementById(`config-${field}`).value;
    }
    try {
      await fetchJSON(`/services/${name}/config`, {
        method: "PUT",
        body: JSON.stringify(configData),
      });
      configModal.classList.remove("open");
      await loadServices();
    } catch (err) {
      errorBanner.showError(err.message);
    }
  });

  document.getElementById("cancel-config-btn").addEventListener("click", () => {
    configModal.classList.remove("open");
  });
};

// ── Action Functions ──────────────────────────

window.startAuth = async function (name) {
  try {
    const data = await fetchJSON(`/auth/${name}/url`, { method: "POST" });
    const url = data.auth_url || data.url;
    if (url) window.open(url, "_blank");
  } catch (err) {
    errorBanner.showError(err.message);
  }
};

window.resetConnection = async function (name) {
  try {
    await fetchJSON(`/auth/${name}/reset`, { method: "POST" });
    await loadServices();
  } catch (err) {
    errorBanner.showError(err.message);
  }
};

window.syncNow = async function (name) {
  try {
    await fetchJSON(`/sync/${name}`, { method: "POST" });
    errorBanner.showError("Sync started");
    startPolling();
  } catch (err) {
    errorBanner.showError(err.message);
  }
};

// ── Polling ───────────────────────────────────

function startPolling() {
  if (pollInterval) clearInterval(pollInterval);
  pollInterval = setInterval(async () => {
    try {
      const data = await fetchJSON("/services");
      servicesData = data.data || data;
      const anySyncing = servicesData.some((s) => s.syncing);
      renderServices(servicesData);
      if (!anySyncing && pollInterval) {
        clearInterval(pollInterval);
        pollInterval = null;
      }
    } catch {
      // ignore polling errors
    }
  }, 3000);
}

// ── Init ──────────────────────────────────────

document.addEventListener("DOMContentLoaded", () => {
  document.getElementById("close-modal-btn")?.addEventListener("click", () => {
    configModal.classList.remove("open");
  });

  configModal?.addEventListener("click", (e) => {
    if (e.target === configModal) configModal.classList.remove("open");
  });

  loadServices();

  // Auto-refresh every 30s
  setInterval(() => {
    if (!configModal.classList.contains("open")) {
      loadServices();
    }
  }, 30000);
});
