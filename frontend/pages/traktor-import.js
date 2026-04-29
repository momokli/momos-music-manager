/**
 * traktor-import.js — Import play counts and last-played dates from Traktor's collection.nml.
 *
 * Sections:
 *   Top Bar — Mode toggle (Auto/Manual) + path input + continuous toggle + import button
 *   Status Bar — Detected path, last modified, live watch indicator
 *   Progress Panel — Status badge, progress bar, expandable logs
 *
 * Settings are persisted in localStorage.
 * Uses existing CSS design system classes (card, btn, input-text, status-badge, etc.)
 */

import { fetchJSON } from "../shared/api.js";
import { renderLoading } from "../shared/components.js";

/* ------------------------------------------------------------------ */
/*  Constants                                                          */
/* ------------------------------------------------------------------ */

const LS_PREFIX = "traktor_import_";
const LS_PATH_MODE = LS_PREFIX + "pathMode";
const LS_CUSTOM_PATH = LS_PREFIX + "customPath";
const LS_CONTINUOUS = LS_PREFIX + "continuous";
const LS_INTERVAL = LS_PREFIX + "interval";

const INTERVAL_OPTIONS = [
  { value: 5, label: "5 min" },
  { value: 15, label: "15 min" },
  { value: 30, label: "30 min" },
  { value: 60, label: "1 hour" },
];

/* ------------------------------------------------------------------ */
/*  State                                                              */
/* ------------------------------------------------------------------ */

let state = {
  pathMode: localStorage.getItem(LS_PATH_MODE) || "auto",
  customPath: localStorage.getItem(LS_CUSTOM_PATH) || "",
  continuous: localStorage.getItem(LS_CONTINUOUS) === "true",
  intervalMinutes: parseInt(localStorage.getItem(LS_INTERVAL), 10) || 15,

  detectedPath: null,
  detectedModifiedAt: null,
  lastKnownModifiedAt: null,

  taskId: null,
  taskStatus: "",
  taskMessage: "",
  taskLogs: [],
  taskPercent: null,
  pollHandle: null,

  contPollHandle: null,
  lastContCheckTime: null,
};

let containerEl = null;
let abortSignal = null;

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

function escapeHtml(str) {
  if (typeof str !== "string") return str;
  return str
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

/* ------------------------------------------------------------------ */
/*  Persistence helpers                                                */
/* ------------------------------------------------------------------ */

function saveSetting(key, value) {
  localStorage.setItem(key, String(value));
}

/* ------------------------------------------------------------------ */
/*  API calls                                                          */
/* ------------------------------------------------------------------ */

async function fetchStatus() {
  const params = {};
  if (state.pathMode === "manual" && state.customPath.trim()) {
    params.customPath = state.customPath.trim();
  }
  const qs = Object.keys(params).length
    ? "?" + new URLSearchParams(params).toString()
    : "";
  const resp = await fetchJSON(`/api/traktor/status${qs}`, { signal: abortSignal });
  const data = resp.data || resp;
  state.detectedPath = data.path || null;
  state.detectedModifiedAt = data.modifiedAt != null ? data.modifiedAt : null;
  return data;
}

async function startImport() {
  const btn = containerEl?.querySelector("#traktor-import-btn");
  if (btn) btn.disabled = true;

  try {
    const body = {};
    if (state.pathMode === "manual" && state.customPath.trim()) {
      body.customPath = state.customPath.trim();
    }

    const resp = await fetchJSON("/api/traktor/import", {
      method: "POST",
      body: JSON.stringify(body),
      signal: abortSignal,
    });

    state.taskId = resp.data?.taskId;
    if (!state.taskId) throw new Error("No task ID returned from server");

    state.taskStatus = "running";
    state.taskMessage = "Import started\u2026";
    state.taskLogs = [];
    state.taskPercent = 0;
    updateUI();
    startTaskPolling();
  } catch (err) {
    if (err.name === "AbortError") return;
    state.taskId = null;
    state.taskStatus = "failed";
    state.taskMessage = err.message;
    updateUI();
    if (btn) btn.disabled = false;
  }
}

async function pollTask() {
  if (!state.taskId) return;
  try {
    const resp = await fetchJSON(`/api/tasks/${state.taskId}`, { signal: abortSignal });
    const data = resp.data || resp;
    const rawStatus = (data.status || "").toLowerCase();
    const map = {
      pending: "pending",
      running: "running",
      completed: "completed",
      failed: "failed",
      cancelled: "cancelled",
    };
    state.taskStatus = map[rawStatus] || rawStatus;
    state.taskMessage = data.message || "";
    state.taskPercent = data.percent != null ? Math.round(data.percent) : null;
    if (Array.isArray(data.logs)) state.taskLogs = data.logs;
    updateUI();

    if (["completed", "failed", "cancelled"].includes(state.taskStatus)) {
      stopTaskPolling();
      const btn = containerEl?.querySelector("#traktor-import-btn");
      if (btn) btn.disabled = false;
      if (state.taskStatus === "completed") {
        try {
          const st = await fetchStatus();
          state.lastKnownModifiedAt = st.modifiedAt;
        } catch (_) {
          /* ignore */
        }
        updateUI();
      }
    }
  } catch (err) {
    if (err.name === "AbortError") return;
    state.taskMessage = `Poll error: ${err.message}`;
    updateUI();
  }
}

function startTaskPolling() {
  stopTaskPolling();
  state.pollHandle = setInterval(pollTask, 1500);
}

function stopTaskPolling() {
  if (state.pollHandle) {
    clearInterval(state.pollHandle);
    state.pollHandle = null;
  }
}

/* ------------------------------------------------------------------ */
/*  Continuous polling                                                 */
/* ------------------------------------------------------------------ */

async function checkForChanges() {
  if (state.taskStatus === "running") return;

  try {
    const data = await fetchStatus();
    const currentMtime = data.modifiedAt;
    state.lastContCheckTime = Date.now();

    if (
      currentMtime != null &&
      state.lastKnownModifiedAt != null &&
      currentMtime > state.lastKnownModifiedAt
    ) {
      state.lastKnownModifiedAt = currentMtime;
      state.taskMessage = "Collection changed \u2014 auto-importing\u2026";
      updateUI();
      await startImport();
    } else if (currentMtime != null && state.lastKnownModifiedAt == null) {
      state.lastKnownModifiedAt = currentMtime;
    }
    updateUI();
  } catch (_) {
    /* silent */
  }
}

function startContinuousPolling() {
  stopContinuousPolling();
  if (!state.continuous) return;
  const ms = state.intervalMinutes * 60 * 1000;
  state.contPollHandle = setInterval(checkForChanges, ms);
  setTimeout(checkForChanges, 2000);
}

function stopContinuousPolling() {
  if (state.contPollHandle) {
    clearInterval(state.contPollHandle);
    state.contPollHandle = null;
  }
}

/* ------------------------------------------------------------------ */
/*  Settings actions                                                   */
/* ------------------------------------------------------------------ */

function setPathMode(mode) {
  if (state.pathMode === mode) return;
  state.pathMode = mode;
  saveSetting(LS_PATH_MODE, mode);
  updateUI();
  refreshDetectedPath();
}

function saveCustomPath() {
  const input = containerEl?.querySelector("#traktor-custom-path-input");
  if (input) {
    state.customPath = input.value.trim();
    saveSetting(LS_CUSTOM_PATH, state.customPath);
  }
  refreshDetectedPath();
}

function toggleContinuous() {
  state.continuous = !state.continuous;
  saveSetting(LS_CONTINUOUS, state.continuous);
  if (state.continuous) {
    startContinuousPolling();
  } else {
    stopContinuousPolling();
  }
  updateUI();
}

function setIntervalMinutes(val) {
  state.intervalMinutes = parseInt(val, 10);
  saveSetting(LS_INTERVAL, state.intervalMinutes);
  if (state.continuous) {
    startContinuousPolling();
  }
  updateUI();
}

async function refreshDetectedPath() {
  try {
    await fetchStatus();
  } catch (_) {
    /* ignore */
  }
  if (state.detectedPath) {
    state.lastKnownModifiedAt = state.detectedModifiedAt;
  }
  updateUI();
}

/* ------------------------------------------------------------------ */
/*  UI Rendering                                                       */
/* ------------------------------------------------------------------ */

/**
 * Render a mode chip — shows currently selected mode with a clear visual.
 * Clicking it switches to that mode.
 */
function modeChip(mode, label, icon) {
  const active = state.pathMode === mode;
  return `<button class="btn ${active ? "btn-primary" : ""}" data-mode="${mode}"
    style="flex:1;justify-content:center;font-size:0.8rem;${
      active ? "box-shadow:0 0 0 1px var(--accent);" : "opacity:0.7;"
    }"
    onclick="window.traktorSetPathMode('${mode}')">
    <i class="fa-solid ${icon}"></i> ${label}
  </button>`;
}

/**
 * Render the path editing area.
 */
function pathSection() {
  const isManual = state.pathMode === "manual";
  const isRunning = state.taskStatus === "running";

  if (isManual) {
    return `
      <div class="flex items-center gap-2" style="flex-wrap:wrap;">
        <input
          type="text"
          id="traktor-custom-path-input"
          class="input-text"
          placeholder="/absolute/path/to/collection.nml"
          value="${escapeHtml(state.customPath)}"
          style="flex:1;min-width:200px;font-family:var(--font-mono);font-size:0.8rem;"
          ${isRunning ? "disabled" : ""}
        />
        <button class="btn btn-primary btn-sm" onclick="window.traktorSavePath()" ${isRunning ? "disabled" : ""}>
          <i class="fa-solid fa-check"></i> Save
        </button>
      </div>
      <div style="margin-top:6px;font-size:0.75rem;color:var(--text-subtle);">
        <i class="fa-solid fa-circle-info"></i> Full path to <code style="font-size:0.75rem;">collection.nml</code>
      </div>`;
  }

  // Auto mode — show detected path
  return `
    <div class="flex items-center gap-2" style="flex-wrap:wrap;">
      <code style="flex:1;background:var(--bg);padding:6px 10px;border-radius:var(--radius-md);font-size:0.8rem;color:var(--text-secondary);min-width:200px;border:1px solid var(--border);">
        ${state.detectedPath ? escapeHtml(state.detectedPath) : '<span style="color:var(--text-subtle);">Not detected</span>'}
      </code>
      <button class="btn btn-sm" onclick="window.traktorRefreshStatus()" title="Refresh">
        <i class="fa-solid fa-rotate"></i>
      </button>
    </div>
    `;
}

/**
 * Render the full page.
 */
function renderPage() {
  const isRunning = state.taskStatus === "running";
  const hasTask = !!state.taskId;
  const taskStatus = state.taskStatus;

  const isWatching = state.continuous && state.detectedPath && taskStatus !== "running";

  /* ──── Top bar: mode chips + continuous toggle + import button ──── */
  const topBarHtml = `
    <div style="display:flex;align-items:stretch;gap:var(--space-3);flex-wrap:wrap;">

      <!-- Mode chips (left group) -->
      <div style="display:flex;gap:2px;background:var(--bg);padding:2px;border-radius:var(--radius-md);border:1px solid var(--border);">
        ${modeChip("auto", "Auto", "fa-magnifying-glass")}
        ${modeChip("manual", "Manual", "fa-pen")}
      </div>

      <!-- Continuous toggle (fixed width, no jump) -->
      <button class="btn" id="traktor-continuous-btn" ${isRunning ? "disabled" : ""}
        style="min-width:120px;justify-content:center;gap:6px;${
          state.continuous
            ? "background:rgba(16,185,129,0.1);border-color:var(--green);color:var(--green);"
            : "opacity:0.7;"
        }">
        <span style="width:8px;height:8px;border-radius:50%;display:inline-block;
          ${state.continuous ? "background:var(--green);box-shadow:0 0 6px var(--green);" : "background:var(--text-subtle);"}">
        </span>
        <span style="min-width:54px;display:inline-block;text-align:left;">Watching</span>
        <span style="font-size:0.7rem;opacity:0.7;min-width:28px;display:inline-block;text-align:right;">${state.intervalMinutes}min</span>
      </button>

      <!-- Interval buttons (always visible, greyed when not watching) -->
      <div style="display:flex;gap:2px;background:var(--bg);padding:2px;border-radius:var(--radius-md);border:1px solid var(--border);opacity:${state.continuous ? "1" : "0.4"};">
        ${INTERVAL_OPTIONS.map(
          (o) =>
            `<button class="btn btn-sm interval-btn" data-interval="${o.value}"
              style="${state.intervalMinutes === o.value && state.continuous ? "background:var(--accent);border-color:var(--accent);color:#fff;" : ""}"
              ${isRunning || !state.continuous ? "disabled" : ""}>${o.label}</button>`,
        ).join("")}
      </div>

      <!-- Import button (right, prominent) -->
      <button
        id="traktor-import-btn"
        class="btn" ${isRunning ? "disabled" : ""}
        style="flex:1;justify-content:center;background:var(--accent);border-color:var(--accent);color:#fff;font-weight:600;gap:var(--space-2);min-width:160px;
          ${isRunning ? "opacity:0.6;" : ""}
          ${isRunning ? "" : "box-shadow:0 0 20px rgba(99,102,241,0.15);"}">
        <i class="fa-solid ${isRunning ? "fa-spinner fa-spin" : "fa-upload"}"></i>
        <span>${isRunning ? "Importing\u2026" : "Import from Traktor"}</span>
      </button>
    </div>`;

  /* ──── Path / status row ──── */
  const statusRowHtml = `
    <div class="card" style="padding:var(--space-4);">
      <div style="display:flex;align-items:flex-start;gap:var(--space-3);flex-wrap:wrap;">

        <!-- Left: path info -->
        <div style="flex:1;min-width:200px;">
          <div style="font-size:0.7rem;font-weight:600;text-transform:uppercase;letter-spacing:0.05em;color:var(--text-subtle);margin-bottom:6px;">
            <i class="fa-solid fa-folder-tree" style="margin-right:4px;"></i>
            ${state.pathMode === "manual" ? "Manual Path" : "Detected Path"}
          </div>
          ${pathSection()}
        </div>

        <!-- Right: status info -->
        <div style="display:flex;flex-direction:column;gap:6px;align-items:flex-end;flex-shrink:0;">
          ${
            state.detectedModifiedAt
              ? `<span class="status-badge" style="font-size:0.7rem;">
                 <i class="fa-regular fa-calendar"></i>
                 Modified ${new Date(state.detectedModifiedAt * 1000).toLocaleString()}
               </span>`
              : ""
          }

          ${
            isWatching
              ? `<span class="status-badge running" style="font-size:0.7rem;animation:pulse-dot 2s ease-in-out infinite;">
                 <span style="width:6px;height:6px;border-radius:50%;background:var(--accent);display:inline-block;"></span>
                 Watching ${state.lastContCheckTime ? "\u2014 last check " + new Date(state.lastContCheckTime).toLocaleTimeString() : ""}
               </span>`
              : ""
          }
        </div>
      </div>
    </div>`;

  /* ──── Progress panel ──── */
  const progressHtml = hasTask
    ? `<div class="card" id="traktor-progress-panel" style="padding:var(--space-4);border-left:3px solid ${
        taskStatus === "running"
          ? "var(--accent)"
          : taskStatus === "completed"
            ? "var(--green)"
            : taskStatus === "failed"
              ? "var(--red)"
              : "var(--text-muted)"
      };">

        <!-- Progress header -->
        <div class="flex items-center justify-between" style="margin-bottom:var(--space-3);">
          <div class="flex items-center gap-2">
            <h3 style="font-size:0.85rem;font-weight:600;color:var(--text-secondary);margin:0;">
              <i class="fa-solid fa-list-check" style="margin-right:6px;color:var(--accent);"></i>
              Import Task
            </h3>
            <span class="status-badge ${taskStatus}" style="font-size:0.7rem;">
              <i class="fa-solid ${taskStatus === "running" ? "fa-spinner fa-spin" : taskStatus === "completed" ? "fa-check-circle" : taskStatus === "failed" ? "fa-times-circle" : taskStatus === "cancelled" ? "fa-ban" : "fa-clock"}"></i>
              ${taskStatus.charAt(0).toUpperCase() + taskStatus.slice(1)}
            </span>
            ${state.taskPercent != null ? `<span style="font-size:0.75rem;color:var(--text-muted);font-weight:500;">${state.taskPercent}%</span>` : ""}
          </div>

          ${
            taskStatus === "failed" ||
            taskStatus === "completed" ||
            taskStatus === "cancelled"
              ? `<button class="btn btn-sm" onclick="window.traktorResetTask()">
                 <i class="fa-solid fa-xmark"></i> Dismiss
               </button>`
              : ""
          }
        </div>

        <!-- Progress bar -->
        ${
          state.taskPercent != null
            ? `<div style="margin-bottom:var(--space-3);">
               <div style="width:100%;height:6px;background:var(--border);border-radius:999px;overflow:hidden;">
                 <div style="width:${state.taskPercent}%;height:100%;background:${taskStatus === "failed" ? "var(--red)" : taskStatus === "completed" ? "var(--green)" : "var(--accent)"};border-radius:999px;transition:width 0.3s ease;"></div>
               </div>
             </div>`
            : ""
        }

        <!-- Message -->
        ${
          state.taskMessage
            ? `<p style="color:var(--text);font-size:0.85rem;margin-bottom:var(--space-3);">${escapeHtml(state.taskMessage)}</p>`
            : ""
        }

        <!-- Expandable logs -->
        <details ${state.taskLogs.length > 0 ? "open" : ""}>
          <summary style="cursor:pointer;color:var(--text-subtle);font-size:0.8rem;user-select:none;padding:2px 0;">
            <i class="fa-solid fa-terminal" style="margin-right:6px;"></i>
            Logs (${state.taskLogs.length})
          </summary>
          <div style="margin-top:var(--space-2);max-height:240px;overflow-y:auto;background:var(--bg);border:1px solid var(--border);border-radius:var(--radius-md);padding:var(--space-3);font-family:var(--font-mono);font-size:0.78rem;line-height:1.6;">
            ${
              state.taskLogs.length > 0
                ? state.taskLogs
                    .map(
                      (l) =>
                        `<div style="color:var(--text-secondary);white-space:pre-wrap;">${escapeHtml(l)}</div>`,
                    )
                    .join("")
                : '<div style="color:var(--text-muted);font-style:italic;">No logs yet\u2026</div>'
            }
          </div>
        </details>

        <!-- Success actions -->
        ${
          taskStatus === "completed"
            ? `<div style="margin-top:var(--space-3);padding-top:var(--space-3);border-top:1px solid var(--border);display:flex;gap:var(--space-2);">
               <a href="#tracks" class="btn btn-sm btn-green"><i class="fa-solid fa-music"></i> View Tracks</a>
               <a href="#dashboard" class="btn btn-sm"><i class="fa-solid fa-gauge-high"></i> Dashboard</a>
             </div>`
            : ""
        }
      </div>`
    : "";

  /* ──── Assemble everything ──── */
  return `


    ${topBarHtml}
    <div style="margin-top:var(--space-3);">${statusRowHtml}</div>
    ${progressHtml ? `<div style="margin-top:var(--space-3);">${progressHtml}</div>` : ""}
  `;
}

function updateUI() {
  if (!containerEl) return;
  containerEl.innerHTML = renderPage();
  wireEvents();
}

/* ------------------------------------------------------------------ */
/*  Event wiring                                                       */
/* ------------------------------------------------------------------ */

function wireEvents() {
  // Continuous toggle button
  const contBtn = document.getElementById("traktor-continuous-btn");
  if (contBtn) {
    contBtn.onclick = () => toggleContinuous();
  }

  // Interval buttons
  document.querySelectorAll(".interval-btn").forEach((btn) => {
    btn.onclick = () => setIntervalMinutes(btn.dataset.interval);
  });

  // Import button
  const importBtn = document.getElementById("traktor-import-btn");
  if (importBtn) {
    importBtn.onclick = () => startImport();
  }

  // Global handlers
  window.traktorSetPathMode = (mode) => setPathMode(mode);
  window.traktorSavePath = () => saveCustomPath();
  window.traktorRefreshStatus = () => refreshDetectedPath();
  window.traktorResetTask = () => {
    state.taskId = null;
    state.taskStatus = "";
    state.taskMessage = "";
    state.taskLogs = [];
    state.taskPercent = null;
    updateUI();
  };
}

/* ------------------------------------------------------------------ */
/*  Cleanup                                                            */
/* ------------------------------------------------------------------ */

function cleanup() {
  stopTaskPolling();
  stopContinuousPolling();
  delete window.traktorSetPathMode;
  delete window.traktorSavePath;
  delete window.traktorRefreshStatus;
  delete window.traktorResetTask;
}

/* ------------------------------------------------------------------ */
/*  Init                                                               */
/* ------------------------------------------------------------------ */

export function init(container, signal) {
  containerEl = container;
  abortSignal = signal;

  // Reset task-only fields (keep settings)
  state.taskId = null;
  state.taskStatus = "";
  state.taskMessage = "";
  state.taskLogs = [];
  state.taskPercent = null;
  stopTaskPolling();

  container.innerHTML = renderLoading("Loading Traktor Import\u2026");

  requestAnimationFrame(async () => {
    if (signal.aborted) return;

    try {
      await fetchStatus();
      if (state.detectedPath) {
        state.lastKnownModifiedAt = state.detectedModifiedAt;
      }
    } catch (_) {
      /* ok */
    }

    if (signal.aborted) return;

    container.innerHTML = renderPage();
    wireEvents();

    if (state.continuous) {
      startContinuousPolling();
    }

    signal.addEventListener("abort", cleanup);
  });
}
