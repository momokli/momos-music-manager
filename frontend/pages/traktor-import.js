/**
 * traktor-import.js — Import metadata from Traktor's collection.nml.
 *
 * Sections:
 *   Header — Detected path, last modified, mode toggle, import button
 *   Progress Panel — Import stats, auto-import info
 *
 * Settings are persisted in localStorage.
 * Auto-import is handled server-side by the Maintainer.
 */

import { fetchJSON } from "../shared/api.js";
import { renderLoading } from "../shared/components.js";

/* ------------------------------------------------------------------ */
/*  Constants                                                          */
/* ------------------------------------------------------------------ */

const LS_PREFIX = "traktor_import_";
const LS_PATH_MODE = LS_PREFIX + "pathMode";
const LS_CUSTOM_PATH = LS_PREFIX + "customPath";

/* ------------------------------------------------------------------ */
/*  State                                                              */
/* ------------------------------------------------------------------ */

let state = {
  pathMode: localStorage.getItem(LS_PATH_MODE) || "auto",
  customPath: localStorage.getItem(LS_CUSTOM_PATH) || "",

  detectedPath: null,
  detectedModifiedAt: null,

  taskId: null,
  taskStatus: "",
  taskMessage: "",
  taskLogs: [],
  taskPercent: null,
  pollHandle: null,
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
    const rawStatus = data.status || "";
    const map = {
      pending: "running",
      running: "running",
      completed: "completed",
      failed: "failed",
      cancelled: "cancelled",
    };
    const status = map[rawStatus] || rawStatus;
    state.taskStatus = status;
    state.taskMessage = data.message || data.progress?.message || "";
    state.taskLogs = data.logs || [];
    state.taskPercent = data.progress?.percent ?? null;

    const btn = containerEl?.querySelector("#traktor-import-btn");
    const st = state.taskStatus;
    if (st === "completed" || st === "failed" || st === "cancelled") {
      stopTaskPolling();
      if (btn) btn.disabled = false;
    }
    updateUI();
  } catch (_) {
    /* silently retry */
  }
}

function startTaskPolling() {
  stopTaskPolling();
  state.pollHandle = setInterval(pollTask, 1000);
}

function stopTaskPolling() {
  if (state.pollHandle) {
    clearInterval(state.pollHandle);
    state.pollHandle = null;
  }
}

/* ------------------------------------------------------------------ */
/*  Settings actions                                                   */
/* ------------------------------------------------------------------ */

function setPathMode(mode) {
  if (state.pathMode === mode) return;
  state.pathMode = mode;
  saveSetting(LS_PATH_MODE, mode);
  fetchStatus()
    .then(() => updateUI())
    .catch(() => {});
}

function saveCustomPath() {
  const input = containerEl?.querySelector("#traktor-path-input");
  if (!input) return;
  state.customPath = input.value.trim();
  saveSetting(LS_CUSTOM_PATH, state.customPath);
  fetchStatus()
    .then(() => updateUI())
    .catch(() => {});
}

function refreshDetectedPath() {
  fetchStatus()
    .then(() => updateUI())
    .catch(() => {});
}

/* ------------------------------------------------------------------ */
/*  Rendering helpers                                                  */
/* ------------------------------------------------------------------ */

function modeChip(mode, label) {
  const active = state.pathMode === mode;
  return `<button class="btn btn-sm ${active ? "btn-primary" : "btn-ghost"}"
    data-action="set-path-mode" data-mode="${mode}">${label}</button>`;
}

function pathSection() {
  const isManual = state.pathMode === "manual";
  const isRunning = state.taskStatus === "running";

  let html = '<div class="form-row">';
  html += `<span class="form-label">Mode</span>`;
  html += `<div class="filter-group">${modeChip("auto", "Auto-detect")}${modeChip("manual", "Manual path")}</div>`;
  html += "</div>";

  if (isManual) {
    html += '<div class="form-row" style="margin-top: 0.5rem;">';
    html += `<input type="text" class="input-text" id="traktor-path-input"
      value="${escapeHtml(state.customPath)}"
      placeholder="/path/to/collection.nml"
      data-action="save-custom-path" />`;
    html += "</div>";
  }

  html += '<div class="form-row" style="margin-top: 0.75rem;">';
  html += `<button class="btn btn-primary" id="traktor-import-btn"
    ${isRunning ? "disabled" : ""} data-action="start-import">
    <i class="fas fa-download"></i> Import Now
  </button>`;
  html += "</div>";

  return html;
}

function renderPage() {
  const isRunning = state.taskStatus === "running";
  const taskStatus = state.taskStatus;
  const hasTask = taskStatus === "completed" || taskStatus === "failed";

  const detectedPath = state.detectedPath || "\u2014";
  const modifiedAt = state.detectedModifiedAt
    ? new Date(state.detectedModifiedAt * 1000).toLocaleString()
    : "\u2014";

  const topBarHtml = `
    <div class="card" style="margin-bottom: 1rem;">
      <div class="card-header">
        <h2><i class="fas fa-file-import"></i> Traktor Import</h2>
      </div>
      <div class="card-body">
        <div class="form-row">
          <span class="form-label">Collection</span>
          <span class="text-mono" style="font-size: 0.8rem;">${escapeHtml(detectedPath)}</span>
        </div>
        <div class="form-row">
          <span class="form-label">Last modified</span>
          <span>${modifiedAt}</span>
        </div>
        ${pathSection()}
      </div>
    </div>`;

  const progressHtml = hasTask
    ? `
    <div class="card">
      <div class="card-header">
        <h3>Last Import</h3>
      </div>
      <div class="card-body">
        <div class="form-row">
          <span class="status-badge status-${taskStatus}">${taskStatus === "completed" ? "\u2713 Completed" : "\u2717 Failed"}</span>
        </div>
        ${state.taskMessage ? `<p style="margin-top: 0.5rem;">${escapeHtml(state.taskMessage)}</p>` : ""}
        ${state.taskLogs.length ? `<pre class="task-logs" style="margin-top: 0.5rem; font-size: 0.75rem; max-height: 200px; overflow-y: auto;">${state.taskLogs.map(escapeHtml).join("\n")}</pre>` : ""}
      </div>
    </div>`
    : "";

  const autoImportInfo = `
    <div class="card" style="margin-top: 1rem; opacity: 0.7;">
      <div class="card-body">
        <p><i class="fas fa-info-circle"></i> <strong>Auto-import is handled by the server.</strong></p>
        <p style="font-size: 0.8rem;">The maintainer checks <code>collection.nml</code> periodically and imports BPM, musical key, rating, and play stats whenever it changes. No browser tab needed.</p>
      </div>
    </div>`;

  return topBarHtml + progressHtml + autoImportInfo;
}

/* ------------------------------------------------------------------ */
/*  Update UI                                                          */
/* ------------------------------------------------------------------ */

function updateUI() {
  if (!containerEl) return;
  containerEl.innerHTML = renderPage();
  wireEvents();
}

/* ------------------------------------------------------------------ */
/*  Event wiring                                                       */
/* ------------------------------------------------------------------ */

function wireEvents() {
  if (!containerEl) return;

  // Import button
  const importBtn = containerEl.querySelector("#traktor-import-btn");
  if (importBtn) {
    importBtn.addEventListener("click", () => startImport());
  }

  // Path mode toggle
  containerEl.querySelectorAll('[data-action="set-path-mode"]').forEach((btn) => {
    btn.addEventListener("click", () => setPathMode(btn.dataset.mode));
  });

  // Custom path input (Enter to save)
  const pathInput = containerEl.querySelector("#traktor-path-input");
  if (pathInput) {
    pathInput.addEventListener("keydown", (e) => {
      if (e.key === "Enter") saveCustomPath();
    });
  }
}

/* ------------------------------------------------------------------ */
/*  Cleanup                                                            */
/* ------------------------------------------------------------------ */

function cleanup() {
  stopTaskPolling();
  containerEl = null;
  abortSignal = null;
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
    } catch (_) {
      /* ok */
    }

    if (signal.aborted) return;

    container.innerHTML = renderPage();
    wireEvents();

    signal.addEventListener("abort", cleanup);
  });
}
