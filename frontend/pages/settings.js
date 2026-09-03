/**
 * settings.js — Settings page.
 *
 * Update controls (Phase A+B of the update-settings feature + channel
 * select + Phase C auto-apply interval):
 * - version / channel / update status display (GET /api/update/status)
 * - "Check now" (POST /api/update/check)
 * - auto-update toggle with persistence (POST /api/update/settings)
 * - auto-apply interval select (POST /api/update/settings) — when
 *   auto-update is on, updates are installed automatically every interval
 *   and the server restarts itself (self-restart)
 * - update-channel dropdown (`release` | `rolling`) with confirm modal
 * - manual "Update now" (POST /api/update/apply)
 *
 * Toggle, channel dropdown and interval select are disabled when their
 * effective value is pinned by config.toml or an environment variable
 * (source "toml"/"env") — the precedence rule is Env > UI > TOML > default
 * (enabled: true; channel: embedded channel of the running build; interval:
 * 4 h). check/apply run against the selected channel; an explicit
 * cross-channel switch is confirmed via modal.
 */

import { fetchJSON } from "../shared/api.js";
import {
  escapeHtml,
  renderLoading,
  renderErrorBlock,
  showToast,
  showConfirmModal,
} from "../shared/components.js";

/* ------------------------------------------------------------------ */
/*  State                                                              */
/* ------------------------------------------------------------------ */

let state = {
  status: null,
  checking: false,
  applying: false,
  inlineHtml: null,
};

/** Auto-apply interval presets (seconds → label). 0 = periodic loop off. */
const INTERVAL_PRESETS = [
  { secs: 0, label: "Off (manual updates only)" },
  { secs: 3600, label: "Every hour" },
  { secs: 14400, label: "Every 4 hours" },
  { secs: 43200, label: "Every 12 hours" },
  { secs: 86400, label: "Every day" },
];

function intervalLabel(secs) {
  const preset = INTERVAL_PRESETS.find((p) => p.secs === secs);
  if (preset) return preset.label;
  if (secs === 0) return "Off (manual updates only)";
  return `Every ${secs} seconds`;
}

function intervalOptionsHtml(status) {
  const effective = Number.isFinite(status.autoApplyIntervalSecs)
    ? status.autoApplyIntervalSecs
    : 14400;
  const known = INTERVAL_PRESETS.some((p) => p.secs === effective);
  return [
    ...(known
      ? []
      : [`<option value="${effective}" selected>${escapeHtml(intervalLabel(effective))} (custom)</option>`]),
    ...INTERVAL_PRESETS.map(
      (p) =>
        `<option value="${p.secs}"${p.secs === effective ? " selected" : ""}>${p.label}${p.secs === 14400 ? " (default)" : ""}</option>`,
    ),
  ].join("");
}

let _container = null;
let _signal = null;

/* ------------------------------------------------------------------ */
/*  Exports                                                            */
/* ------------------------------------------------------------------ */

export async function init(container, signal) {
  _container = container;
  _signal = signal;

  container.innerHTML = `
    <div class="page-header">
      <h1><i class="fas fa-gear"></i> Settings</h1>
    </div>
    <div class="card" id="settings-updates-card">
      <h3><i class="fas fa-rotate"></i> Updates</h3>
      <div id="settings-updates-content">${renderLoading("Loading update status...")}</div>
    </div>
    <div class="card" id="settings-telemetry-card">
      <h3><i class="fas fa-tower-broadcast"></i> Telemetry</h3>
      <div id="settings-telemetry-content">${renderLoading("Loading telemetry settings...")}</div>
    </div>
    <div class="card" id="settings-cli-card">
      <h3><i class="fas fa-terminal"></i> CLI access</h3>
      <div id="settings-cli-content">${renderLoading("Loading CLI state...")}</div>
    </div>
  `;

  await Promise.all([loadStatus(container), loadTelemetryStatus(container)]);
  wireEvents(container);
  wireTelemetryEvents(container);
}

/* ------------------------------------------------------------------ */
/*  Rendering                                                          */
/* ------------------------------------------------------------------ */

function stateBadge(status) {
  const result = status.lastCheckResult;
  const st = result ? result.state : null;

  if (st === "updateAvailable") {
    return '<span class="badge" style="background:rgba(234,179,8,0.15);color:var(--yellow)">Update available</span>';
  }
  if (st === "upToDate") {
    return '<span class="badge" style="background:rgba(34,197,94,0.15);color:var(--green)">Up to date</span>';
  }
  if (st === "channelMismatch") {
    return '<span class="badge" style="background:rgba(249,115,22,0.15);color:#fb923c">Channel mismatch</span>';
  }
  if (st === "disabled") {
    return '<span class="badge" style="background:rgba(100,116,139,0.15);color:var(--text-muted)">Updates disabled</span>';
  }
  if (st === "unsupportedPlatform") {
    return '<span class="badge" style="background:rgba(239,68,68,0.15);color:var(--red)">Unsupported platform</span>';
  }
  if (st === "error") {
    return '<span class="badge" style="background:rgba(239,68,68,0.15);color:var(--red)">Check failed</span>';
  }
  return '<span class="badge" style="background:rgba(100,116,139,0.15);color:var(--text-muted)">Never checked</span>';
}

function formatLastCheck(status) {
  if (!status.lastCheckAt) return "never";
  const d = new Date(status.lastCheckAt * 1000);
  return d.toLocaleString();
}

function channelBadge(status) {
  const isRolling = status.channel === "rolling";
  const color = isRolling ? "var(--yellow)" : "var(--green)";
  return `<span class="badge" style="background:rgba(100,116,139,0.15);color:${color}">${escapeHtml(status.channel)}</span>`;
}

function channelOptionLabel(channel) {
  return channel === "rolling"
    ? "Rolling (dev builds of main)"
    : "Release (stable)";
}

function channelOptionsHtml(status) {
  const list =
    Array.isArray(status.availableChannels) && status.availableChannels.length
      ? status.availableChannels
      : ["release", "rolling"];
  return list
    .map(
      (c) =>
        `<option value="${c}"${c === status.channel ? " selected" : ""}>${channelOptionLabel(c)}</option>`,
    )
    .join("");
}

function renderStatus(container) {
  const el = container.querySelector("#settings-updates-content");
  if (!el) return;
  const s = state.status;
  const result = s.lastCheckResult;
  const st = result ? result.state : null;
  const channelMismatch = st === "channelMismatch";
  const updateAvailable = !!s.updateAvailable || st === "updateAvailable";
  const canApply = updateAvailable && !channelMismatch && s.enabled && !state.applying;

  // Toggle state
  const toggleDisabled = s.enabledSource === "env" || s.enabledSource === "toml";
  const sourceHint = toggleDisabled
    ? s.enabledSource === "env"
      ? 'Pinned by the environment variable <code>MOMOS_AUTOUPDATE_ENABLED</code> — change it there to edit this toggle.'
      : 'Set in <code>config.toml</code> (<code>[autoupdate] enabled</code>) — edit the file to change this toggle.'
    : "";

  // Channel state (same precedence rule as the toggle)
  const channelPinned = s.channelSource === "env" || s.channelSource === "toml";
  const channelSourceHint = channelPinned
    ? s.channelSource === "env"
      ? 'Pinned by the environment variable <code>MOMOS_AUTOUPDATE_CHANNEL</code> — change it there to edit the channel.'
      : 'Set in <code>config.toml</code> (<code>[autoupdate] channel</code>) — edit the file to change the channel.'
    : "";

  // Auto-apply interval state (same precedence rule as the toggle)
  const intervalPinned = s.autoApplyIntervalSource === "env" || s.autoApplyIntervalSource === "toml";
  const intervalSourceHint = intervalPinned
    ? s.autoApplyIntervalSource === "env"
      ? 'Pinned by the environment variable <code>MOMOS_AUTOUPDATE_INTERVAL_SECS</code> — change it there to edit the interval.'
      : 'Set in <code>config.toml</code> (<code>[autoupdate] interval_secs</code>) — edit the file to change the interval.'
    : "";

  // Channel mismatch explanation — the update source serves the *other*
  // channel than selected (inconsistent base URL / feed).
  let mismatchHtml = "";
  if (channelMismatch && result) {
    const cur = result.currentVersion || s.currentVersion;
    const avail = result.availableVersion || "?";
    const published = String(avail).includes("-dev+") ? "a dev build" : "a stable release";
    const tracks =
      s.channel === "rolling"
        ? "rolling dev builds of main"
        : "stable semver releases";
    mismatchHtml = `
      <div class="help-text" style="margin-top:0.5rem;color:#fb923c">
        <i class="fas fa-arrow-right-arrow-left"></i>
        Channel mismatch: update channel is <strong>${escapeHtml(s.channel)}</strong> (tracks ${tracks}), but the update source serves ${published} <strong>${escapeHtml(avail)}</strong> (current build: <code>${escapeHtml(cur)}</code>).
        Pick the matching channel in the dropdown above or fix the update source (<code>base_url</code>).
      </div>`;
  }

  // Pending update note
  let pendingHtml = "";
  if (s.pendingUpdate) {
    const p = s.pendingUpdate;
    pendingHtml = `
      <div class="help-text" style="margin-top:0.5rem;color:var(--yellow)">
        <i class="fas fa-hourglass-half"></i>
        Pending update: v${escapeHtml(p.oldVersion)} → v${escapeHtml(p.newVersion)}
        (${p.committed ? "committed — restart to activate" : "waiting for health check"}).
      </div>`;
  }

  // Last check error detail
  let lastCheckDetail = "";
  if (st === "error" && s.lastCheckError) {
    lastCheckDetail = `<div class="help-text" style="color:var(--red)">${escapeHtml(s.lastCheckError)}</div>`;
  }

  el.innerHTML = `
    <div class="settings-update-row">
      <div class="settings-update-label">Version</div>
      <div class="settings-update-value">
        <code>v${escapeHtml(s.currentVersion)}</code>
        ${channelBadge(s)}
        <span class="text-muted" style="font-size:0.8rem">${escapeHtml(s.baseUrl)}</span>
      </div>
    </div>
    <div class="settings-update-row">
      <div class="settings-update-label">Status</div>
      <div class="settings-update-value">${stateBadge(s)}</div>
    </div>
    <div class="settings-update-row">
      <div class="settings-update-label">Last check</div>
      <div class="settings-update-value">
        ${formatLastCheck(s)}
        ${s.lastCheckStatus === "error" ? ' <span class="text-muted">(failed)</span>' : ""}
        ${lastCheckDetail}
      </div>
    </div>
    <div class="settings-update-row">
      <div class="settings-update-label">Auto-update</div>
      <div class="settings-update-value">
        <div style="display:flex;gap:1rem;align-items:center;flex-wrap:wrap">
          <span style="display:flex;gap:0.5rem;align-items:center">
            <label class="switch">
              <input type="checkbox" id="autoupdate-toggle" ${s.enabled ? "checked" : ""} ${toggleDisabled ? "disabled" : ""}>
              <span class="slider"></span>
            </label>
            <span class="text-muted">
              ${s.enabled ? "Enabled" : "Disabled"}
              ${s.enabledSource === "ui" ? "(set in UI)" : ""}
            </span>
          </span>
          <span style="display:flex;gap:0.4rem;align-items:center">
            <label for="update-channel-select" class="text-muted" style="font-size:0.8rem">Channel</label>
            <select id="update-channel-select" ${channelPinned ? "disabled" : ""} title="Which builds Check now / Update now track">
              ${channelOptionsHtml(s)}
            </select>
          </span>
          <span style="display:flex;gap:0.4rem;align-items:center">
            <label for="autoupdate-interval-select" class="text-muted" style="font-size:0.8rem">Auto-apply every</label>
            <select id="autoupdate-interval-select" ${intervalPinned ? "disabled" : ""} title="How often updates are checked and applied automatically (0 = off — updates are only checked at startup and applied manually)">
              ${intervalOptionsHtml(s)}
            </select>
          </span>
        </div>
        ${s.enabled ? `<div class="help-text" style="margin-top:0.25rem"><i class="fas fa-rotate"></i> ${intervalLabel(s.autoApplyIntervalSecs ?? 14400).startsWith("Off")
          ? 'Updates are checked at startup only and applied manually ("Update now") — automatic applying is off.'
          : `When enabled, available updates are installed automatically ${intervalLabel(s.autoApplyIntervalSecs ?? 14400).toLowerCase()} and the server restarts itself afterwards. Manual "Update now" stays available anytime.`}</div>` : ""}
        ${toggleDisabled ? `<div class="help-text" style="margin-top:0.25rem">${sourceHint}</div>` : ""}
        ${channelPinned ? `<div class="help-text" style="margin-top:0.25rem">${channelSourceHint}</div>` : ""}
        ${intervalPinned ? `<div class="help-text" style="margin-top:0.25rem">${intervalSourceHint}</div>` : ""}
      </div>
    </div>
    <div class="settings-update-actions" style="margin-top:1rem;display:flex;gap:0.75rem;align-items:center">
      <button class="btn btn-primary" id="update-check-now-btn" ${state.checking ? "disabled" : ""}>
        <i class="fas ${state.checking ? "fa-spinner fa-spin" : "fa-magnifying-glass"}"></i>
        ${state.checking ? "Checking..." : "Check now"}
      </button>
      <button class="btn" id="update-apply-now-btn" ${canApply ? "" : "disabled"} title="${canApply ? "" : "Only available when an update is published on this channel"}" style="${updateAvailable && !channelMismatch && s.enabled ? "" : "display:none"}">
        <i class="fas ${state.applying ? "fa-spinner fa-spin" : "fa-download"}"></i>
        ${state.applying ? "Updating..." : "Update now"}
      </button>
      <span id="settings-update-inline" class="text-muted"></span>
    </div>
    ${mismatchHtml}
    ${pendingHtml}
  `;

  // Re-apply any transient inline message (check/apply feedback) — the
  // innerHTML above replaces the span, so restore it here.
  if (state.inlineHtml) {
    const inline = el.querySelector("#settings-update-inline");
    if (inline) inline.innerHTML = state.inlineHtml;
  }
}

function renderInline(html) {
  state.inlineHtml = html;
  const el = _container?.querySelector("#settings-update-inline");
  if (el) el.innerHTML = html;
}

/* ------------------------------------------------------------------ */
/*  Data Loading                                                       */
/* ------------------------------------------------------------------ */

async function loadStatus(container) {
  try {
    const resp = await fetchJSON("/api/update/status", { signal: _signal });
    state.status = resp.data;
    renderStatus(container);
  } catch (err) {
    if (err.name === "AbortError") return;
    const el = container.querySelector("#settings-updates-content");
    if (el) {
      el.innerHTML = renderErrorBlock({
        title: "Failed to load update status",
        detail: err.message,
      });
    }
  }
}

/* ------------------------------------------------------------------ */
/*  Events                                                             */
/* ------------------------------------------------------------------ */

function wireEvents(container) {
  container.addEventListener("click", async (e) => {
    const checkBtn = e.target.closest("#update-check-now-btn");
    if (checkBtn) {
      state.checking = true;
      renderStatus(container);
      renderInline('<i class="fas fa-spinner fa-spin"></i> Checking for updates...');
      try {
        const resp = await fetchJSON("/api/update/check", {
          method: "POST",
          signal: _signal,
        });
        state.status = resp.data;
        const st = resp.data.lastCheckResult?.state;
        let msg;
        if (st === "updateAvailable") {
          msg = `<span style="color:var(--yellow)">Update available: v${escapeHtml(resp.data.lastCheckResult.availableVersion)}</span>`;
        } else if (st === "error") {
          msg = `<span style="color:var(--red)">Check failed: ${escapeHtml(resp.data.lastCheckError || "unknown error")}</span>`;
        } else if (st === "channelMismatch") {
          msg = '<span style="color:#fb923c">Checked — channel mismatch (see above)</span>';
        } else if (st === "disabled") {
          msg = '<span class="text-muted">Updates are disabled</span>';
        } else {
          msg = '<span style="color:var(--green)">Up to date</span>';
        }
        renderStatus(container);
        renderInline(msg);
      } catch (err) {
        if (err.name === "AbortError") return;
        renderStatus(container);
        renderInline(`<span style="color:var(--red)">${escapeHtml(err.message)}</span>`);
      } finally {
        state.checking = false;
        renderStatus(container);
      }
      return;
    }

    const applyBtn = e.target.closest("#update-apply-now-btn");
    if (applyBtn) {
      const confirmed = await showConfirmModal(
        "Update now",
        `Install <strong>v${escapeHtml(state.status.lastCheckResult?.availableVersion || "?")}</strong> now? The current version is replaced (Linux/Windows: atomic binary swap; macOS: the app bundle is replaced in the install directory) and the server must be restarted afterwards.`,
        "Update now",
        "primary",
      );
      if (!confirmed) return;

      state.applying = true;
      renderStatus(container);
      renderInline('<i class="fas fa-spinner fa-spin"></i> Downloading and verifying...');
      try {
        const resp = await fetchJSON("/api/update/apply", {
          method: "POST",
          signal: _signal,
        });
        const outcome = resp.data.outcome;
        let msg;
        if (outcome === "installed") {
          msg = `
            <span style="color:var(--green)">
              Installed v${escapeHtml(resp.data.oldVersion)} → v${escapeHtml(resp.data.newVersion)}.
            </span>
            <strong>Restart the server to activate the update.</strong>
          `;
          showToast("Update installed — restart the server to activate", "success");
        } else if (outcome === "downloaded") {
          msg = `
            <span style="color:var(--green)">Verified download saved:</span>
            <code>${escapeHtml(resp.data.path)}</code>
            <span class="help-text">Open the DMG and drag the app to Applications, then restart the server.</span>
          `;
          showToast("Update downloaded — install from Downloads", "success");
        } else {
          msg = `<span class="text-muted">${escapeHtml(outcome)}</span>`;
        }
        // Keep the full status shape; loadStatus() below refreshes it — the
        // apply response only carries the outcome fields.
        renderStatus(container);
        renderInline(msg);
        await loadStatus(container);
      } catch (err) {
        if (err.name === "AbortError") return;
        renderStatus(container);
        renderInline(`<span style="color:var(--red)">${escapeHtml(err.message)}</span>`);
      } finally {
        state.applying = false;
        renderStatus(container);
      }
      return;
    }
  });

  container.addEventListener("change", async (e) => {
    const toggle = e.target.closest("#autoupdate-toggle");
    if (!toggle) return;

    const wanted = toggle.checked;
    try {
      const resp = await fetchJSON("/api/update/settings", {
        method: "POST",
        body: JSON.stringify({ autoUpdateEnabled: wanted }),
        signal: _signal,
      });
      state.status.enabled = resp.data.enabled;
      state.status.enabledSource = resp.data.enabledSource;
      renderStatus(container);
      showToast(`Auto-update ${resp.data.enabled ? "enabled" : "disabled"}`, "success");
    } catch (err) {
      if (err.name === "AbortError") return;
      toggle.checked = !wanted; // revert
      showToast(`Failed to update setting: ${err.message}`, "error");
      await loadStatus(container);
    }
  });

  container.addEventListener("change", async (e) => {
    const intervalSelect = e.target.closest("#autoupdate-interval-select");
    if (!intervalSelect) return;

    const wanted = Number(intervalSelect.value);
    if (!Number.isInteger(wanted) || wanted < 0) return;
    const previous = state.status?.autoApplyIntervalSecs ?? 14400;
    if (wanted === previous) return;

    try {
      const resp = await fetchJSON("/api/update/settings", {
        method: "POST",
        body: JSON.stringify({ autoApplyIntervalSecs: wanted }),
        signal: _signal,
      });
      state.status.autoApplyIntervalSecs =
        resp.data.autoApplyIntervalSecs ?? wanted;
      state.status.autoApplyIntervalSource =
        resp.data.autoApplyIntervalSource || "ui";
      renderStatus(container);
      const msg =
        wanted === 0
          ? "Automatic updates turned off — updates are checked at startup only"
          : `Updates will be applied automatically ${intervalLabel(wanted).toLowerCase()}`;
      showToast(msg, "success");
    } catch (err) {
      if (err.name === "AbortError") return;
      intervalSelect.value = String(previous); // revert
      renderStatus(container);
      showToast(`Failed to change auto-apply interval: ${err.message}`, "error");
    }
  });

  container.addEventListener("change", async (e) => {
    const channelSelect = e.target.closest("#update-channel-select");
    if (!channelSelect) return;

    const previous = state.status?.channel;
    const wanted = channelSelect.value;
    if (!wanted || !previous || wanted === previous) return;

    // Cross-channel switch: the next check/apply runs against the other
    // channel and Update now may install a binary of the other build type.
    const tracks =
      wanted === "rolling"
        ? "<strong>rolling</strong> tracks dev builds of <code>main</code> (published on latest-main)"
        : "<strong>release</strong> tracks stable semver releases (releases/latest)";
    const confirmed = await showConfirmModal(
      "Switch update channel",
      `Change the update channel from <strong>${escapeHtml(previous)}</strong> to <strong>${escapeHtml(wanted)}</strong>?<br><br>` +
        `This decides which builds <strong>Check now</strong> and <strong>Update now</strong> use — ${tracks}. ` +
        `The next <strong>Update now</strong> may therefore install a binary of the other channel type (e.g. a release build on a dev install); after the restart the app runs on that channel's builds.`,
      "Switch channel",
    );
    if (!confirmed) {
      channelSelect.value = previous; // revert
      return;
    }

    try {
      const resp = await fetchJSON("/api/update/settings", {
        method: "POST",
        body: JSON.stringify({ channel: wanted }),
        signal: _signal,
      });
      const newChannel = resp.data.channel || wanted;
      state.status.channel = newChannel;
      state.status.channelSource = resp.data.channelSource || "ui";
      // The server cleared the stale last-check cache of the old channel —
      // reload the status so the card shows the honest "never checked" state.
      await loadStatus(container);
      renderStatus(container);
      showToast(`Update channel set to ${newChannel}`, "success");
      renderInline(
        `<span style="color:var(--green)">Channel switched to <strong>${escapeHtml(newChannel)}</strong> — run Check now to see updates on this channel.</span>`,
      );
    } catch (err) {
      if (err.name === "AbortError") return;
      channelSelect.value = previous; // revert
      renderStatus(container);
      showToast(`Failed to switch channel: ${err.message}`, "error");
    }
  });
}

/* ------------------------------------------------------------------ */
/*  Telemetry settings (client push)                                   */
/* ------------------------------------------------------------------ */

/**
 * Telemetry card — Settings-page surface of the telemetry client:
 * - effective values + sources (Env > TOML > Defaults) for enabled /
 *   base_url / token / instance / full_db_interval_secs
 * - save persists into `[telemetry]` of config.toml (fields pinned by an
 *   env var are disabled; config.toml values are editable)
 * - "Push now" runs the same one-shot push as the CLI (`telemetry push`)
 *   and shows success/error + the timestamp of the last successful push
 * - CLI-access card: state of the `momos-music-manager` symlink
 *
 * Background loops (periodic push, event pipeline) pick changes up after
 * the next restart; the status + Push now always use the current file state.
 */

let telemetryState = {
  status: null,
  saving: false,
  pushing: false,
  inlineHtml: null,
  dirty: {},
};

const TELEMETRY_FIELDS = [
  { key: "enabled", envVar: "MOMOS_TELEMETRY_ENABLED", tomlKey: "[telemetry] enabled", control: "toggle" },
  { key: "baseUrl", envVar: "MOMOS_TELEMETRY_BASE_URL", tomlKey: "[telemetry] base_url", control: "field" },
  { key: "token", envVar: "MOMOS_TELEMETRY_TOKEN", tomlKey: "[telemetry] token", control: "field" },
  { key: "instance", envVar: "MOMOS_TELEMETRY_INSTANCE", tomlKey: "[telemetry] instance", control: "field" },
  { key: "fullDbIntervalSecs", envVar: "MOMOS_TELEMETRY_FULL_DB_INTERVAL_SECS", tomlKey: "[telemetry] full_db_interval_secs", control: "field" },
];

function telemetrySourceOf(status, key) {
  const map = {
    enabled: "enabledSource",
    baseUrl: "baseUrlSource",
    token: "tokenSource",
    instance: "instanceSource",
    fullDbIntervalSecs: "fullDbIntervalSource",
  };
  return status[map[key]] || "default";
}

function telemetryPinHint(key) {
  const f = TELEMETRY_FIELDS.find((x) => x.key === key);
  return `Pinned by the environment variable <code>${f.envVar}</code> — change it there to edit this ${f.control}.`;
}

function telemetryTomlHint(key) {
  const f = TELEMETRY_FIELDS.find((x) => x.key === key);
  return `Set in <code>config.toml</code> (<code>${f.tomlKey}</code>) — editable here; the save writes it back.`;
}

function telemetryBadge(status) {
  if (status.enabled) {
    return '<span class="badge" style="background:rgba(34,197,94,0.15);color:var(--green)">Enabled</span>';
  }
  return '<span class="badge" style="background:rgba(100,116,139,0.15);color:var(--text-muted)">Disabled (default)</span>';
}

function telemetryPeriodicText(status) {
  if (!status.enabled) {
    return "Periodic full-DB push and event collection are off — the push button below still works after enabling.";
  }
  if (status.periodicPushActive) {
    return `Full-DB snapshot is pushed automatically every ${status.fullDbIntervalSecs} s (event batches are sent continuously). Changes apply after the next restart.`;
  }
  return "Enabled — but no periodic full-DB push interval set (0). Events are collected; use \"Push now\" for a snapshot push.";
}

function formatLastPush(status) {
  if (!status.lastPushAt) return "never";
  const d = new Date(status.lastPushAt * 1000);
  return d.toLocaleString();
}

function renderTelemetryInline(html) {
  telemetryState.inlineHtml = html;
  const el = _container?.querySelector("#settings-telemetry-inline");
  if (el) el.innerHTML = html;
}

function renderTelemetry(container) {
  const el = container.querySelector("#settings-telemetry-content");
  if (!el) return;
  const s = telemetryState.status;
  if (!s) return;

  const pinned = (key) => telemetrySourceOf(s, key) === "env";
  const sourceText = (key) => {
    const src = telemetrySourceOf(s, key);
    if (src === "env") return telemetryPinHint(key);
    if (src === "toml") return telemetryTomlHint(key);
    return "Built-in default";
  };
  const editable = (key) => !pinned(key);

  const valueAttr = (v) => escapeHtml(v ?? "");
  const hintFor = (key) =>
    `<div class="help-text" style="margin-top:0.25rem">${sourceText(key)}</div>`;

  el.innerHTML = `
    <div class="settings-update-row">
      <div class="settings-update-label">Status</div>
      <div class="settings-update-value">
        ${telemetryBadge(s)}
        <span class="text-muted" style="font-size:0.8rem">
          version ${escapeHtml(s.currentVersion)} · event endpoint: <code>${escapeHtml(s.eventsEndpoint || "not configured")}</code>
        </span>
        <div class="help-text" style="margin-top:0.25rem">${telemetryPeriodicText(s)}</div>
      </div>
    </div>
    <div class="settings-update-row">
      <div class="settings-update-label">Telemetry</div>
      <div class="settings-update-value">
        <div style="display:flex;gap:1rem;align-items:center;flex-wrap:wrap">
          <span style="display:flex;gap:0.5rem;align-items:center">
            <label class="switch">
              <input type="checkbox" id="telemetry-enabled-toggle" ${s.enabled ? "checked" : ""} ${editable("enabled") ? "" : "disabled"}>
              <span class="slider"></span>
            </label>
            <span class="text-muted">${s.enabled ? "Enabled" : "Disabled"}${s.enabledSource === "env" ? " (env)" : ""}</span>
          </span>
        </div>
        ${hintFor("enabled")}
      </div>
    </div>
    <div class="settings-update-row">
      <div class="settings-update-label">Collector base URL</div>
      <div class="settings-update-value">
        <input type="text" id="telemetry-base-url" class="input" style="width:min(420px,100%)" placeholder="https://collector.example" value="${valueAttr(s.baseUrl)}" ${editable("baseUrl") ? "" : "disabled"}>
        ${hintFor("baseUrl")}
      </div>
    </div>
    <div class="settings-update-row">
      <div class="settings-update-label">Token</div>
      <div class="settings-update-value">
        <input type="password" id="telemetry-token" class="input" style="width:min(420px,100%)" placeholder="Bearer token (leave empty to clear)" value="${valueAttr(s.token)}" ${editable("token") ? "" : "disabled"} autocomplete="off">
        ${hintFor("token")}
      </div>
    </div>
    <div class="settings-update-row">
      <div class="settings-update-label">Instance name</div>
      <div class="settings-update-value">
        <input type="text" id="telemetry-instance" class="input" style="width:min(420px,100%)" placeholder="macbook" value="${valueAttr(s.instance)}" ${editable("instance") ? "" : "disabled"}>
        ${hintFor("instance")}
      </div>
    </div>
    <div class="settings-update-row">
      <div class="settings-update-label">Full-DB push interval</div>
      <div class="settings-update-value">
        <div style="display:flex;gap:0.5rem;align-items:center;flex-wrap:wrap">
          <input type="number" id="telemetry-interval" class="input" min="0" step="60" style="width:9rem" value="${valueAttr(s.fullDbIntervalSecs)}" ${editable("fullDbIntervalSecs") ? "" : "disabled"}>
          <span class="text-muted" style="font-size:0.8rem">seconds — 0 = off (periodic)</span>
        </div>
        ${hintFor("fullDbIntervalSecs")}
      </div>
    </div>
    <div class="settings-update-row">
      <div class="settings-update-label">Last push</div>
      <div class="settings-update-value">
        ${formatLastPush(s)}
        ${s.lastPushStatus === "error" ? ' <span class="text-muted">(failed)</span>' : ""}
        ${s.lastPushStatus === "ok" ? ' <span style="color:var(--green)">✓</span>' : ""}
        ${s.lastPushError ? `<div class="help-text" style="color:var(--red)">${escapeHtml(s.lastPushError)}</div>` : ""}
      </div>
    </div>
    <div class="settings-update-actions" style="margin-top:1rem;display:flex;gap:0.75rem;align-items:center">
      <button class="btn btn-primary" id="telemetry-save-btn" disabled>
        <i class="fas ${telemetryState.saving ? "fa-spinner fa-spin" : "fa-floppy-disk"}"></i>
        ${telemetryState.saving ? "Saving..." : "Save settings"}
      </button>
      <button class="btn" id="telemetry-push-btn" ${s.enabled && !telemetryState.pushing ? "" : "disabled"} title="${s.enabled ? "" : "Enable telemetry first"}">
        <i class="fas ${telemetryState.pushing ? "fa-spinner fa-spin" : "fa-upload"}"></i>
        ${telemetryState.pushing ? "Pushing..." : "Push now"}
      </button>
      <span id="settings-telemetry-inline" class="text-muted"></span>
    </div>
  `;

  if (telemetryState.inlineHtml) {
    const inline = el.querySelector("#settings-telemetry-inline");
    if (inline) inline.innerHTML = telemetryState.inlineHtml;
  }
  updateTelemetrySaveButton();
}

function renderCliCard(container) {
  const el = container.querySelector("#settings-cli-content");
  if (!el) return;
  const s = telemetryState.status;
  if (!s) return;
  const cli = s.cli;

  if (!cli.supported) {
    el.innerHTML = `<span class="text-muted">CLI symlinks are not supported on this platform (Windows).</span>`;
    return;
  }
  if (cli.linkPath) {
    const needsPathExport = cli.linkPath.includes("/.local/bin/");
    el.innerHTML = `
      <div class="settings-update-row">
        <div class="settings-update-label">Command</div>
        <div class="settings-update-value"><code>momos-music-manager --version</code> · <code>momos-music-manager telemetry push</code> · <code>momos-music-manager update check</code></div>
      </div>
      <div class="settings-update-row">
        <div class="settings-update-label">Symlink</div>
        <div class="settings-update-value">
          <code>${escapeHtml(cli.linkPath)}</code>
          <span class="text-muted" style="font-size:0.8rem">→ <code>${escapeHtml(cli.targetPath)}</code></span>
        </div>
      </div>
      <div class="help-text" style="margin-top:0.5rem">
        <i class="fas fa-circle-info"></i>
        The app keeps this symlink up to date (first launch + after every self-update).
        ${needsPathExport ? `
        <code>~/.local/bin</code> is not on the default macOS PATH — add it to <code>~/.zprofile</code>:
        <code style="display:inline-block;margin-top:0.25rem">export PATH="$HOME/.local/bin:$PATH"</code>` : ""}
      </div>
    `;
  } else if (cli.reason) {
    el.innerHTML = `<span style="color:var(--yellow)"><i class="fas fa-triangle-exclamation"></i> CLI not available: ${escapeHtml(cli.reason)}</span>`;
  } else {
    el.innerHTML = `<span class="text-muted"><i class="fas fa-circle-info"></i> Running the bare binary (dev build / Linux) — the CLI is the binary itself, no symlink needed.</span>`;
  }
}

function updateTelemetrySaveButton() {
  const btn = _container?.querySelector("#telemetry-save-btn");
  if (!btn) return;
  const hasDirty = Object.values(telemetryState.dirty).some(Boolean);
  btn.disabled = !hasDirty || telemetryState.saving;
}

function markDirty(key, dirty) {
  telemetryState.dirty[key] = dirty;
  updateTelemetrySaveButton();
}

async function loadTelemetryStatus(container) {
  try {
    const resp = await fetchJSON("/api/telemetry-settings/status", { signal: _signal });
    telemetryState.status = resp.data;
    telemetryState.dirty = {};
    telemetryState.inlineHtml = null;
    renderTelemetry(container);
    renderCliCard(container);
  } catch (err) {
    if (err.name === "AbortError") return;
    const el = container.querySelector("#settings-telemetry-content");
    if (el) {
      el.innerHTML = renderErrorBlock({ title: "Failed to load telemetry settings", detail: err.message });
    }
    const cli = container.querySelector("#settings-cli-content");
    if (cli) cli.innerHTML = "";
  }
}

async function saveTelemetrySettings(container) {
  const s = telemetryState.status;
  if (!s) return;
  const pinned = (key) => telemetrySourceOf(s, key) === "env";
  const body = {};

  if (telemetryState.dirty.enabled && !pinned("enabled")) {
    body.enabled = document.querySelector("#telemetry-enabled-toggle")?.checked ?? s.enabled;
  }
  const readText = (id) => {
    const el = document.querySelector(id);
    return el ? el.value : null;
  };
  if (telemetryState.dirty.baseUrl && !pinned("baseUrl")) {
    body.baseUrl = readText("#telemetry-base-url") ?? s.baseUrl ?? "";
  }
  if (telemetryState.dirty.token && !pinned("token")) {
    body.token = readText("#telemetry-token") ?? s.token ?? "";
  }
  if (telemetryState.dirty.instance && !pinned("instance")) {
    body.instance = readText("#telemetry-instance") ?? s.instance ?? "";
  }
  if (telemetryState.dirty.fullDbIntervalSecs && !pinned("fullDbIntervalSecs")) {
    const raw = readText("#telemetry-interval");
    const secs = raw === null || raw === "" ? null : Number(raw);
    if (secs !== null && Number.isInteger(secs) && secs >= 0) {
      body.fullDbIntervalSecs = secs;
    } else {
      showToast("Full-DB interval must be a whole number ≥ 0", "error");
      return;
    }
  }
  if (Object.keys(body).length === 0) return;

  telemetryState.saving = true;
  updateTelemetrySaveButton();
  renderTelemetryInline('<i class="fas fa-spinner fa-spin"></i> Saving to config.toml...');
  try {
    await fetchJSON("/api/telemetry-settings/settings", {
      method: "POST",
      body: JSON.stringify(body),
      signal: _signal,
    });
    await loadTelemetryStatus(container);
    showToast("Telemetry settings saved to config.toml", "success");
  } catch (err) {
    if (err.name === "AbortError") return;
    renderTelemetryInline(`<span style="color:var(--red)">${escapeHtml(err.message)}</span>`);
    showToast(`Failed to save telemetry settings: ${err.message}`, "error");
  } finally {
    telemetryState.saving = false;
    renderTelemetry(container);
    updateTelemetrySaveButton();
  }
}

async function pushTelemetryNow(container) {
  telemetryState.pushing = true;
  renderTelemetry(container);
  renderTelemetryInline('<i class="fas fa-spinner fa-spin"></i> Pushing snapshot + metadata...');
  try {
    const resp = await fetchJSON("/api/telemetry-settings/push", {
      method: "POST",
      signal: _signal,
    });
    await loadTelemetryStatus(container); // refreshes last-push state
    if (resp.data.ok) {
      renderTelemetryInline('<span style="color:var(--green)">Push succeeded ✓</span>');
    } else {
      renderTelemetryInline(`<span style="color:var(--red)">Push failed: ${escapeHtml(resp.data.message)}</span>`);
    }
  } catch (err) {
    if (err.name === "AbortError") return;
    renderTelemetry(container);
    renderTelemetryInline(`<span style="color:var(--red)">${escapeHtml(err.message)}</span>`);
  } finally {
    telemetryState.pushing = false;
    renderTelemetry(container);
  }
}

/* ------------------------------------------------------------------ */
/*  Telemetry events                                                   */
/* ------------------------------------------------------------------ */

function wireTelemetryEvents(container) {
  // Dirty tracking on the editable controls (Save only sends changed
  // fields; pinned controls are disabled and never reach this code).
  const onValueChange = (key, current) => {
    const s = telemetryState.status;
    if (!s) return;
    const raw = current();
    const base =
      key === "fullDbIntervalSecs"
        ? String(s.fullDbIntervalSecs ?? 0)
        : key === "enabled"
          ? s.enabled
          : s[key] ?? "";
    telemetryState.dirty[key] = raw !== base;
    updateTelemetrySaveButton();
  };

  container.addEventListener("change", (e) => {
    const toggle = e.target.closest("#telemetry-enabled-toggle");
    if (toggle) {
      onValueChange("enabled", () => toggle.checked);
      return;
    }
    const baseUrl = e.target.closest("#telemetry-base-url");
    if (baseUrl) {
      onValueChange("baseUrl", () => baseUrl.value);
      return;
    }
    const token = e.target.closest("#telemetry-token");
    if (token) {
      onValueChange("token", () => token.value);
      return;
    }
    const instance = e.target.closest("#telemetry-instance");
    if (instance) {
      onValueChange("instance", () => instance.value);
      return;
    }
    const interval = e.target.closest("#telemetry-interval");
    if (interval) {
      onValueChange("fullDbIntervalSecs", () => interval.value);
      return;
    }
  });
  // Text inputs: track typing immediately (change only fires on blur).
  container.addEventListener("input", (e) => {
    for (const [id, key] of [
      ["#telemetry-base-url", "baseUrl"],
      ["#telemetry-token", "token"],
      ["#telemetry-instance", "instance"],
      ["#telemetry-interval", "fullDbIntervalSecs"],
    ]) {
      const field = e.target.closest(id);
      if (field) {
        onValueChange(key, () => field.value);
        return;
      }
    }
  });

  container.addEventListener("click", async (e) => {
    const saveBtn = e.target.closest("#telemetry-save-btn");
    if (saveBtn) {
      await saveTelemetrySettings(container);
      return;
    }
    const pushBtn = e.target.closest("#telemetry-push-btn");
    if (pushBtn) {
      await pushTelemetryNow(container);
      return;
    }
  });
}

