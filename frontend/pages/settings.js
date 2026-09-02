/**
 * settings.js — Settings page.
 *
 * Update controls (Phase A+B of the update-settings feature + channel
 * select):
 * - version / channel / update status display (GET /api/update/status)
 * - "Check now" (POST /api/update/check)
 * - auto-update toggle with persistence (POST /api/update/settings)
 * - update-channel dropdown (`release` | `rolling`) with confirm modal
 * - manual "Update now" (POST /api/update/apply)
 *
 * Toggle and channel dropdown are disabled when their effective value is
 * pinned by config.toml or an environment variable (source "toml"/"env") —
 * the precedence rule is Env > UI > TOML > default (enabled: true; channel:
 * embedded channel of the running build). check/apply run against the
 * selected channel; an explicit cross-channel switch is confirmed via modal.
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
  `;

  await loadStatus(container);
  wireEvents(container);
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
        </div>
        ${toggleDisabled ? `<div class="help-text" style="margin-top:0.25rem">${sourceHint}</div>` : ""}
        ${channelPinned ? `<div class="help-text" style="margin-top:0.25rem">${channelSourceHint}</div>` : ""}
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
        `Install <strong>v${escapeHtml(state.status.lastCheckResult?.availableVersion || "?")}</strong> now? The server will be replaced (Linux/Windows) or the verified download will be saved to Downloads (macOS).`,
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
