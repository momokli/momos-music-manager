/**
 * bpm-key-playlists.js — BPM // Key system playlists (Tools page).
 *
 * Previews one Spotify playlist per (BPM, key) combination found in the
 * library (e.g. `124bpm // 12m`), lets the user configure the naming /
 * visibility / schedule settings, and pushes the playlists to Spotify as a
 * background task. The task is polled through `GET /api/tasks/{id}` exactly
 * like `pages/tasks.js` does.
 *
 * Server responses are wrapped as `{ data: ... }` (see shared/api.js).
 *
 * Endpoints:
 *   GET  /api/bpm-key-playlists/preview
 *   POST /api/bpm-key-playlists/sync          { strict }
 *   GET  /api/bpm-key-playlists
 *   GET  /api/bpm-key-playlists/settings
 *   PUT  /api/bpm-key-playlists/settings
 *
 * Page module contract (see app.js): exports `init(container, signal, hashParams)`.
 */

import {
  escapeHtml,
  renderLoading,
  renderErrorBlock,
  renderEmpty,
  showToast,
} from "../shared/components.js";
import { fetchJSON } from "../shared/api.js";

/* ------------------------------------------------------------------ */
/*  Constants                                                          */
/* ------------------------------------------------------------------ */

const POLL_INTERVAL = 2000; // ms — task progress poll interval

/** Settings defaults — mirror the backend defaults of the frozen contract. */
const DEFAULT_SETTINGS = {
  enabled: false,
  nameTemplate: "{bpm}bpm // {key}",
  namePrefix: "",
  minTracks: 1,
  public: false,
  keyStyle: "md",
  strict: false,
  scheduleEnabled: false,
  scheduleIntervalSecs: 3600,
};

const KEY_STYLE_OPTIONS = [
  { value: "md", label: "Traktor (12m / 12d)" },
  { value: "camelot", label: "Camelot (12A / 12B)" },
];

const TASK_LABELS = {
  pending: "Queued",
  running: "Syncing",
  completed: "Done",
  failed: "Failed",
  cancelled: "Cancelled",
};

/* ------------------------------------------------------------------ */
/*  State (kept out of the global scope — module-local only)           */
/* ------------------------------------------------------------------ */

const state = {
  settings: { ...DEFAULT_SETTINGS },
  groups: [],
  total: 0,
  settingsError: null,
  previewError: null,
  taskId: null,
  pollTimer: null,
};

/* ------------------------------------------------------------------ */
/*  Page init                                                         */
/* ------------------------------------------------------------------ */

/**
 * Page init — called by the SPA router on #bpm-key-playlists.
 * @param {HTMLElement} container
 * @param {AbortSignal} signal
 * @param {Object} hashParams
 */
export async function init(container, signal, hashParams) {
  stopPolling();
  state.groups = [];
  state.total = 0;
  state.settings = { ...DEFAULT_SETTINGS };
  state.settingsError = null;
  state.previewError = null;
  state.taskId = null;

  signal.addEventListener("abort", () => stopPolling());

  container.innerHTML = renderLoading("Loading BPM // key playlists…");

  // Load settings + preview in parallel. Both are isolated so a single
  // failure (e.g. Spotify not configured yet) never blanks the whole page.
  const [settingsResult, previewResult] = await Promise.allSettled([
    fetchJSON("/api/bpm-key-playlists/settings", { signal }),
    fetchJSON("/api/bpm-key-playlists/preview", { signal }),
  ]);
  if (signal.aborted) return;

  if (settingsResult.status === "fulfilled") {
    state.settings = {
      ...DEFAULT_SETTINGS,
      ...(settingsResult.value.data?.settings || {}),
    };
  } else {
    state.settingsError =
      settingsResult.reason?.message || "Failed to load settings";
  }

  if (previewResult.status === "fulfilled") {
    state.groups = previewResult.value.data?.groups || [];
    state.total = previewResult.value.data?.total ?? state.groups.length;
  } else {
    state.previewError =
      previewResult.reason?.message || "Failed to load preview";
  }

  renderPage(container);
  wireEvents(container, signal);

  if (state.settingsError) {
    showToast(`Could not load settings: ${state.settingsError}`, "error");
  }
  if (state.previewError) {
    showToast(`Could not load preview: ${state.previewError}`, "error");
  }

  // Non-critical: the already-created system playlists.
  loadExisting(container, signal);
}

/* ------------------------------------------------------------------ */
/*  Render                                                            */
/* ------------------------------------------------------------------ */

function renderPage(container) {
  container.innerHTML = `
    <div class="page-header-row">
      <h1><i class="fas fa-sliders"></i> BPM // Key Playlists</h1>
      <div style="display:flex;gap:var(--space-2);flex-wrap:wrap;justify-content:flex-end;">
        <button class="btn" id="bpmkey-refresh-preview">
          <i class="fas fa-rotate"></i> Refresh preview
        </button>
        <button class="btn btn-spotify" id="bpmkey-sync">
          <i class="fab fa-spotify"></i> Sync to Spotify
        </button>
      </div>
    </div>

    <p class="text-muted" style="margin:0 0 var(--space-3);">
      One private Spotify playlist per <strong>BPM</strong> × <strong>key</strong>
      combination in your library (e.g. <code>124bpm // 12m</code>). These system
      playlists are hidden from the Playlists page by default.
    </p>

    <div id="bpmkey-task-status" class="card" style="display:none;margin-bottom:var(--space-4);padding:var(--space-3);"></div>

    <div style="display:grid;grid-template-columns:minmax(0,1fr);gap:var(--space-4);">
      <div class="card" id="bpmkey-preview-card">
        <div class="card-header" style="display:flex;align-items:center;justify-content:space-between;gap:var(--space-2);flex-wrap:wrap;">
          <span><i class="fas fa-table-list"></i> Preview</span>
          <span class="text-muted" style="font-size:0.85rem;">
            <strong id="bpmkey-group-count">${state.total}</strong> groups ·
            <strong id="bpmkey-exists-count">${countExisting()}</strong> on Spotify
          </span>
        </div>
        <div class="card-body" id="bpmkey-content">${renderPreview()}</div>
      </div>

      <div class="card" id="bpmkey-settings-card">
        <div class="card-header"><i class="fas fa-gear"></i> Settings</div>
        <div class="card-body" id="bpmkey-settings-body">${renderSettingsForm()}</div>
      </div>

      <div class="card" id="bpmkey-existing-card" style="display:none;">
        <div class="card-header">
          <i class="fab fa-spotify"></i> On Spotify
          <span class="text-muted" style="font-size:0.85rem;" id="bpmkey-existing-count"></span>
        </div>
        <div class="card-body" id="bpmkey-existing"></div>
      </div>
    </div>
  `;
}

function countExisting() {
  return state.groups.filter((g) => g.exists).length;
}

function renderPreview() {
  if (state.previewError) {
    return renderErrorBlock({
      title: "Failed to load preview",
      detail: state.previewError,
      retryFn: "window.location.hash='#bpm-key-playlists'",
    });
  }

  if (!state.groups.length) {
    return renderEmpty({
      icon: "music",
      title: "No BPM/key combinations yet",
      message:
        "Scan a folder or import a Traktor collection to populate BPM and key metadata, then hit “Refresh preview”.",
    });
  }

  const rows = state.groups.map(renderGroupRow).join("");
  return `<div class="table-wrap"><table class="data-table" id="bpmkey-tbl">
    <thead><tr>
      <th>BPM</th>
      <th>Key</th>
      <th>Playlist name</th>
      <th>Files</th>
      <th>Tracks</th>
      <th>On Spotify</th>
    </tr></thead>
    <tbody>${rows}</tbody>
  </table></div>`;
}

function renderGroupRow(g) {
  const bpm = escapeHtml(String(g.bpm ?? ""));
  const key = escapeHtml(String(g.key ?? ""));
  const name = escapeHtml(String(g.name ?? ""));
  const systemKey = escapeHtml(String(g.systemKey ?? ""));
  const fileCount = escapeHtml(String(g.fileCount ?? 0));
  const trackCount = escapeHtml(String(g.trackCount ?? 0));

  let existsCell;
  if (g.exists && g.spotifyUrl) {
    existsCell = `<a class="btn btn-sm btn-spotify" href="${escapeHtml(
      String(g.spotifyUrl),
    )}" target="_blank" rel="noopener" title="Open in Spotify"><i class="fab fa-spotify"></i> Open</a>`;
  } else if (g.exists) {
    existsCell = `<span class="status-badge" style="background:rgba(34,197,94,0.12);color:var(--green)"><i class="fas fa-check"></i> Yes</span>`;
  } else {
    existsCell = `<span class="text-muted" title="Not created on Spotify yet">—</span>`;
  }

  return `<tr data-system-key="${systemKey}" data-bpm="${bpm}" data-key="${key}" data-exists="${g.exists ? "true" : "false"}">
    <td class="font-mono">${bpm}</td>
    <td class="font-mono">${key}</td>
    <td>${name}</td>
    <td class="font-mono">${fileCount}</td>
    <td class="font-mono">${trackCount}</td>
    <td>${existsCell}</td>
  </tr>`;
}

function renderSettingsForm() {
  const s = state.settings;
  const errBanner = state.settingsError
    ? `<div class="help-text" style="color:var(--red);margin-bottom:var(--space-3)">${escapeHtml(
        state.settingsError,
      )}</div>`
    : "";

  const keyStyleOpts = KEY_STYLE_OPTIONS.map(
    (o) =>
      `<option value="${escapeHtml(o.value)}"${o.value === s.keyStyle ? " selected" : ""}>${escapeHtml(
        o.label,
      )}</option>`,
  ).join("");

  return `
    ${errBanner}
    <div class="form-group">
      <label class="checkbox-label">
        <input type="checkbox" id="bpmkey-setting-enabled" ${s.enabled ? "checked" : ""}>
        Enable BPM // key playlists
      </label>
      <span class="help-text">When enabled, a sync is enqueued automatically after folder scans and Traktor imports.</span>
    </div>

    <div class="form-group">
      <label for="bpmkey-setting-name-template">Name template</label>
      <input type="text" class="input-text w-full" id="bpmkey-setting-name-template"
             value="${escapeHtml(String(s.nameTemplate ?? ""))}" placeholder="{bpm}bpm // {key}">
      <span class="help-text">Placeholders: <code>{bpm}</code>, <code>{key}</code>.</span>
    </div>

    <div class="form-group">
      <label for="bpmkey-setting-name-prefix">Name prefix</label>
      <input type="text" class="input-text w-full" id="bpmkey-setting-name-prefix"
             value="${escapeHtml(String(s.namePrefix ?? ""))}" placeholder="(optional)">
    </div>

    <div class="form-group">
      <label for="bpmkey-setting-min-tracks">Minimum tracks per playlist</label>
      <input type="number" class="input-text w-full" id="bpmkey-setting-min-tracks"
             min="1" value="${escapeHtml(String(s.minTracks ?? 1))}">
    </div>

    <div class="form-group">
      <label for="bpmkey-setting-key-style">Key style</label>
      <select class="input-text w-full" id="bpmkey-setting-key-style">${keyStyleOpts}</select>
    </div>

    <div class="form-group">
      <label class="checkbox-label">
        <input type="checkbox" id="bpmkey-setting-public" ${s.public ? "checked" : ""}>
        Public playlists
      </label>
      <span class="help-text">Off = private (default).</span>
    </div>

    <div class="form-group">
      <label class="checkbox-label">
        <input type="checkbox" id="bpmkey-setting-strict" ${s.strict ? "checked" : ""}>
        Strict mode
      </label>
      <span class="help-text">Removes playlists whose combination vanished from the library.</span>
    </div>

    <div class="form-group">
      <label class="checkbox-label">
        <input type="checkbox" id="bpmkey-setting-schedule-enabled" ${s.scheduleEnabled ? "checked" : ""}>
        Scheduled sync
      </label>
    </div>

    <div class="form-group">
      <label for="bpmkey-setting-schedule-interval">Schedule interval (seconds)</label>
      <input type="number" class="input-text w-full" id="bpmkey-setting-schedule-interval"
             min="60" step="60" value="${escapeHtml(String(s.scheduleIntervalSecs ?? 3600))}">
    </div>

    <div style="display:flex;align-items:center;gap:var(--space-2);margin-top:var(--space-3);">
      <button class="btn btn-primary" id="bpmkey-save-settings">
        <i class="fas fa-floppy-disk"></i> Save settings
      </button>
      <span class="text-muted" id="bpmkey-save-status"></span>
    </div>
  `;
}

function renderExisting(playlists) {
  const rows = playlists
    .map((p) => {
      const url =
        p.spotifyUrl ||
        (p.spotifyPlaylistId
          ? `https://open.spotify.com/playlist/${p.spotifyPlaylistId}`
          : "");
      const open = url
        ? `<a class="btn btn-sm btn-spotify" href="${escapeHtml(
            String(url),
          )}" target="_blank" rel="noopener" title="Open in Spotify"><i class="fab fa-spotify"></i></a>`
        : `<span class="text-muted">—</span>`;
      return `<tr data-system-key="${escapeHtml(String(p.systemKey ?? ""))}">
        <td>${escapeHtml(String(p.name ?? ""))}</td>
        <td class="font-mono">${escapeHtml(String(p.trackCount ?? 0))}</td>
        <td>${open}</td>
      </tr>`;
    })
    .join("");

  return `<div class="table-wrap"><table class="data-table" id="bpmkey-existing-tbl">
    <thead><tr><th>Name</th><th>Tracks</th><th>Open</th></tr></thead>
    <tbody>${rows}</tbody>
  </table></div>`;
}

/* ------------------------------------------------------------------ */
/*  Event wiring                                                      */
/* ------------------------------------------------------------------ */

function wireEvents(container, signal) {
  wireHeaderActions(container, signal);
  wireSaveButton(container, signal);
}

function wireHeaderActions(container, signal) {
  const refreshBtn = container.querySelector("#bpmkey-refresh-preview");
  if (refreshBtn) {
    refreshBtn.addEventListener(
      "click",
      () => refreshPreview(container, signal),
      { signal },
    );
  }

  const syncBtn = container.querySelector("#bpmkey-sync");
  if (syncBtn) {
    syncBtn.addEventListener("click", () => startSync(container, signal), {
      signal,
    });
  }
}

function wireSaveButton(container, signal) {
  const saveBtn = container.querySelector("#bpmkey-save-settings");
  if (saveBtn) {
    saveBtn.addEventListener("click", () => saveSettings(container, signal), {
      signal,
    });
  }
}

/* ------------------------------------------------------------------ */
/*  Preview                                                           */
/* ------------------------------------------------------------------ */

async function refreshPreview(container, signal) {
  const btn = container.querySelector("#bpmkey-refresh-preview");
  const original = btn ? btn.innerHTML : "";
  if (btn) {
    btn.disabled = true;
    btn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Refreshing…';
  }

  try {
    const resp = await fetchJSON("/api/bpm-key-playlists/preview", { signal });
    if (signal.aborted) return;
    state.groups = resp.data?.groups || [];
    state.total = resp.data?.total ?? state.groups.length;
    state.previewError = null;
    updatePreview(container);
  } catch (err) {
    if (err.name === "AbortError") return;
    state.groups = [];
    state.total = 0;
    state.previewError = err.message || "Failed to load preview";
    updatePreview(container);
    showToast(`Could not refresh preview: ${state.previewError}`, "error");
  } finally {
    if (btn) {
      btn.disabled = false;
      btn.innerHTML = original;
    }
  }
}

function updatePreview(container) {
  const contentEl = container.querySelector("#bpmkey-content");
  if (contentEl) contentEl.innerHTML = renderPreview();

  const groupEl = container.querySelector("#bpmkey-group-count");
  if (groupEl) groupEl.textContent = String(state.total);

  const existsEl = container.querySelector("#bpmkey-exists-count");
  if (existsEl) existsEl.textContent = String(countExisting());
}

/* ------------------------------------------------------------------ */
/*  Settings                                                          */
/* ------------------------------------------------------------------ */

function toPositiveInt(value, fallback) {
  const n = parseInt(value, 10);
  return Number.isFinite(n) && n > 0 ? n : fallback;
}

function readSettingsForm(container) {
  const q = (sel) => container.querySelector(sel);
  return {
    enabled: !!q("#bpmkey-setting-enabled")?.checked,
    nameTemplate: q("#bpmkey-setting-name-template")?.value ?? "",
    namePrefix: q("#bpmkey-setting-name-prefix")?.value ?? "",
    minTracks: toPositiveInt(
      q("#bpmkey-setting-min-tracks")?.value,
      DEFAULT_SETTINGS.minTracks,
    ),
    public: !!q("#bpmkey-setting-public")?.checked,
    keyStyle: q("#bpmkey-setting-key-style")?.value || "md",
    strict: !!q("#bpmkey-setting-strict")?.checked,
    scheduleEnabled: !!q("#bpmkey-setting-schedule-enabled")?.checked,
    scheduleIntervalSecs: toPositiveInt(
      q("#bpmkey-setting-schedule-interval")?.value,
      DEFAULT_SETTINGS.scheduleIntervalSecs,
    ),
  };
}

async function saveSettings(container, signal) {
  const btn = container.querySelector("#bpmkey-save-settings");
  const statusEl = container.querySelector("#bpmkey-save-status");
  const body = readSettingsForm(container);

  if (btn) {
    btn.disabled = true;
    btn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Saving…';
  }
  if (statusEl) statusEl.textContent = "";

  try {
    const resp = await fetchJSON("/api/bpm-key-playlists/settings", {
      method: "PUT",
      body: JSON.stringify(body),
      signal,
    });
    if (signal.aborted) return;

    state.settings = {
      ...DEFAULT_SETTINGS,
      ...(resp.data?.settings || body),
    };
    state.settingsError = null;

    // Re-render the form from the canonical values and re-wire its button.
    const bodyEl = container.querySelector("#bpmkey-settings-body");
    if (bodyEl) {
      bodyEl.innerHTML = renderSettingsForm();
      wireSaveButton(container, signal);
    }

    showToast("Settings saved", "success");
    // Names/templates may have changed — refresh the preview.
    refreshPreview(container, signal);
  } catch (err) {
    if (err.name === "AbortError") return;
    if (statusEl) statusEl.textContent = "Save failed";
    showToast(`Failed to save settings: ${err.message}`, "error");
  } finally {
    const b = container.querySelector("#bpmkey-save-settings");
    if (b) {
      b.disabled = false;
      b.innerHTML = '<i class="fas fa-floppy-disk"></i> Save settings';
    }
  }
}

/* ------------------------------------------------------------------ */
/*  Sync + task polling                                               */
/* ------------------------------------------------------------------ */

async function startSync(container, signal) {
  const btn = container.querySelector("#bpmkey-sync");
  if (btn) {
    btn.disabled = true;
    btn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Syncing…';
  }

  try {
    const resp = await fetchJSON("/api/bpm-key-playlists/sync", {
      method: "POST",
      body: JSON.stringify({ strict: !!state.settings.strict }),
      signal,
    });
    if (signal.aborted) return;

    const taskId = resp.data?.taskId;
    const groupCount = resp.data?.groupCount ?? 0;

    if (groupCount === 0) {
      showToast("Nothing to sync — no BPM/key groups found", "info");
    } else {
      showToast(
        `Sync started for ${groupCount} playlist${groupCount === 1 ? "" : "s"}`,
        "info",
      );
    }

    if (taskId) {
      beginTaskPolling(container, signal, taskId);
    } else {
      // No task id (e.g. feature disabled) — just refresh the preview.
      setTimeout(() => {
        if (!signal.aborted) refreshPreview(container, signal);
      }, 1500);
    }
  } catch (err) {
    if (err.name === "AbortError") return;
    // Covers "Spotify not configured" and any other sync failure.
    showToast(`Sync failed: ${err.message}`, "error");
  } finally {
    if (btn) {
      btn.disabled = false;
      btn.innerHTML = '<i class="fab fa-spotify"></i> Sync to Spotify';
    }
  }
}

function beginTaskPolling(container, signal, taskId) {
  state.taskId = taskId;
  stopPolling();
  showTaskStatus(container, { status: "Pending", percent: 0, text: "Queued…" });

  const tick = async () => {
    if (signal.aborted) {
      stopPolling();
      return;
    }
    try {
      const resp = await fetchJSON(`/api/tasks/${taskId}`, { signal });
      if (signal.aborted) return;

      const t = resp.data || {};
      const raw = String(t.status || "Pending");
      const st = raw.toLowerCase();
      const percent = parsePercent(t);

      showTaskStatus(container, {
        status: raw,
        percent,
        text: t.progress || "",
        error: t.error_message || "",
      });

      if (st === "completed") {
        stopPolling();
        showToast("BPM // key playlist sync complete", "success");
        await refreshPreview(container, signal);
        loadExisting(container, signal);
        setTimeout(() => {
          if (!signal.aborted) hideTaskStatus(container);
        }, 4000);
      } else if (st === "failed" || st === "cancelled") {
        stopPolling();
        showToast(
          `Sync ${st}: ${t.error_message || t.progress || "unknown error"}`,
          "error",
        );
      }
    } catch (err) {
      if (err.name === "AbortError") return;
      // Transient poll failure — the next tick may succeed.
    }
  };

  tick();
  state.pollTimer = setInterval(tick, POLL_INTERVAL);
}

function stopPolling() {
  if (state.pollTimer) {
    clearInterval(state.pollTimer);
    state.pollTimer = null;
  }
}

function parsePercent(task) {
  if (task.percent != null) {
    return Math.max(0, Math.min(100, Math.round(task.percent)));
  }
  const text = String(task.progress || "");
  const pct = text.match(/(\d+(?:\.\d+)?)\s*%/);
  if (pct) {
    return Math.max(0, Math.min(100, Math.round(parseFloat(pct[1]))));
  }
  const frac = text.match(/(\d+)\s*\/\s*(\d+)/);
  if (frac) {
    const done = parseInt(frac[1], 10);
    const total = parseInt(frac[2], 10);
    if (total > 0) {
      return Math.max(0, Math.min(100, Math.round((done / total) * 100)));
    }
  }
  return null;
}

function taskColor(st) {
  if (st === "completed") return "var(--green)";
  if (st === "failed") return "var(--red)";
  if (st === "cancelled") return "var(--text-muted)";
  return "var(--accent)";
}

function showTaskStatus(container, { status, percent, text, error }) {
  const el = container.querySelector("#bpmkey-task-status");
  if (!el) return;

  const st = String(status || "pending").toLowerCase();
  const pct = percent == null ? 0 : percent;
  const color = taskColor(st);
  const icon =
    st === "completed"
      ? "fa-check"
      : st === "failed"
        ? "fa-xmark"
        : st === "cancelled"
          ? "fa-ban"
          : "fa-spinner fa-spin";

  el.style.display = "block";
  el.innerHTML = `
    <div style="display:flex;align-items:center;justify-content:space-between;gap:var(--space-3);flex-wrap:wrap;">
      <span style="display:flex;align-items:center;gap:var(--space-2);min-width:0;">
        <i class="fas ${icon}" style="color:${color}"></i>
        <strong id="bpmkey-task-status-label">${escapeHtml(TASK_LABELS[st] || st)}</strong>
        <span class="text-muted" id="bpmkey-task-text" style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${escapeHtml(text || "")}</span>
      </span>
      <span class="font-mono" id="bpmkey-task-percent">${pct}%</span>
    </div>
    <div class="progress-bar" style="width:100%;margin-top:var(--space-2);">
      <div class="progress-bar-fill" id="bpmkey-task-progress" style="width:${pct}%;background:${color};"></div>
    </div>
    ${
      error
        ? `<div class="help-text" style="color:var(--red);margin-top:var(--space-2);">${escapeHtml(error)}</div>`
        : ""
    }
  `;
}

function hideTaskStatus(container) {
  const el = container.querySelector("#bpmkey-task-status");
  if (el) el.style.display = "none";
}

/* ------------------------------------------------------------------ */
/*  Existing system playlists (non-critical)                          */
/* ------------------------------------------------------------------ */

async function loadExisting(container, signal) {
  const card = container.querySelector("#bpmkey-existing-card");
  const body = container.querySelector("#bpmkey-existing");
  const countEl = container.querySelector("#bpmkey-existing-count");
  if (!card || !body) return;

  try {
    const resp = await fetchJSON("/api/bpm-key-playlists", { signal });
    if (signal.aborted) return;

    const playlists = Array.isArray(resp.data?.playlists)
      ? resp.data.playlists
      : [];
    if (playlists.length === 0) {
      card.style.display = "none";
      return;
    }

    body.innerHTML = renderExisting(playlists);
    if (countEl) countEl.textContent = `(${playlists.length})`;
    card.style.display = "";
  } catch (err) {
    if (err.name === "AbortError") return;
    // Non-critical — the preview already carries the important information.
  }
}
