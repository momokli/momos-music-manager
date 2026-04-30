/**
 * dashboard.js — Revamped dashboard overview page.
 *
 * Layout:
 *   Row 1 (4-col): TAGS | PLAYLISTS | TRACKS | FILES in library  (service % breakdown)
 *   Row 2 (4-col): TRAKTOR COLLECTION | SPOTIFY | SOUNDCLOUD | YOUTUBE  (status)
 *   Row 3 (2-col): MANAGED FOLDERS | SUBSCRIBED PLAYLISTS
 *   Row 4 (3-col): TAGS FROM PLAYLISTS | AUTO-CATEGORIZE | COMMENT DIFFS
 */

import {
  renderLoading,
  renderEmpty,
  renderErrorBlock,
  showToast,
} from "../shared/components.js";
import { formatNumber, formatDateTime } from "../shared/format.js";
import { fetchJSON } from "../shared/api.js";
import { renderCommentWriter, wireCommentWriter } from "../shared/comment-writer.js";

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

/** Relative time string from a Unix-seconds timestamp (or ms). */
function timeAgo(ts) {
  if (!ts) return "never";
  const now = Date.now();
  const diff = now - (ts < 1e12 ? ts * 1000 : ts); // handle both unix-s and ms
  if (diff < 0) return "just now";
  const mins = Math.floor(diff / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d ago`;
  return formatDateTime(new Date(ts < 1e12 ? ts * 1000 : ts));
}

function unixTsToMs(unix) {
  if (!unix) return null;
  return unix * 1000;
}

function pct(part, total) {
  if (!total) return "0%";
  return Math.round((part / total) * 100) + "%";
}

/* ── Service icon / colour helpers ─────────────────────────── */
const SERVICE_STYLE = {
  spotify: { icon: "fa-brands fa-spotify", color: "#1DB954" },
  soundcloud: { icon: "fa-brands fa-soundcloud", color: "#FF7700" },
  youtube: { icon: "fa-brands fa-youtube", color: "#FF0000" },
};

function serviceIcon(service) {
  return SERVICE_STYLE[service]?.icon ?? "fa-solid fa-circle";
}

function serviceColor(service) {
  return SERVICE_STYLE[service]?.color ?? "#888";
}

/* ================================================================== */
/*  Row 1: Service-breakdown cards (TAGS / PLAYLISTS / TRACKS / FILES) */
/* ================================================================== */

/**
 * Render a single "dash-card" showing a breakdown per service as bars.
 */
function renderBreakdownCard(title, icon, total, breakdown, extraFooter = "") {
  // breakdown: [{ service, count }]
  const body = breakdown
    .map((b) => {
      const p = pct(b.count, total);
      const color = serviceColor(b.service);
      return `<div class="dash-service-row">
        <span class="dash-service-icon" style="color:${color}"><i class="${serviceIcon(b.service)}"></i></span>
        <span class="pct-label" style="color:${color}">${p}</span>
        <div class="bar-track">
          <div class="bar-fill" style="width:${p};background:${color}"></div>
        </div>
      </div>`;
    })
    .join("");

  return `<div class="dash-card fade-in">
    <div class="dash-card-header">
      <span><i class="${icon}" style="margin-right:4px;"></i> ${title}</span>
      <span>${formatNumber(total)}</span>
    </div>
    <div class="dash-card-body">${body}</div>
    ${extraFooter ? `<div class="dash-card-footer">${extraFooter}</div>` : ""}
  </div>`;
}

/**
 * Build "TAGS" card using tags/service-coverage data.
 */
function renderTagsCard(tagSvcCoverage) {
  const total = tagSvcCoverage?.total ?? 0;
  const breakdown = [
    { service: "spotify", count: tagSvcCoverage?.spotify ?? 0 },
    { service: "soundcloud", count: tagSvcCoverage?.soundcloud ?? 0 },
    { service: "youtube", count: tagSvcCoverage?.youtube ?? 0 },
  ];
  return renderBreakdownCard("Tags", "fa-solid fa-tags", total, breakdown);
}

/**
 * Build "PLAYLISTS" card using service connections data.
 */
function renderPlaylistsCard(svcConns) {
  const total = (svcConns || []).reduce((a, s) => a + s.playlistsLocal, 0);
  const breakdown = (svcConns || []).map((s) => ({
    service: s.service,
    count: s.playlistsLocal,
  }));
  return renderBreakdownCard("Playlists", "fa-solid fa-list", total, breakdown);
}

/**
 * Build "TRACKS" card using service connections data.
 */
function renderTracksCard(svcConns) {
  const total = (svcConns || []).reduce((a, s) => a + s.tracksLocal, 0);
  const breakdown = (svcConns || []).map((s) => ({
    service: s.service,
    count: s.tracksLocal,
  }));
  return renderBreakdownCard("Tracks", "fa-solid fa-music", total, breakdown);
}

/**
 * Build "FILES in library" card using service-links + needs-update count.
 */
function renderFilesCard(filesCount, svcLinks, unlinked, needsUpdateCount) {
  const total = filesCount;
  const links = svcLinks || { spotify: 0, soundcloud: 0, youtube: 0 };
  const breakdown = [
    { service: "spotify", count: links.spotify ?? 0 },
    { service: "soundcloud", count: links.soundcloud ?? 0 },
    { service: "youtube", count: links.youtube ?? 0 },
  ];

  let footer = "";
  if (unlinked > 0) {
    footer += `<a href="#files?unlinked=true" style="color:var(--text-muted);text-decoration:underline;cursor:pointer;">${formatNumber(unlinked)} unlinked</a>`;
  }
  if (needsUpdateCount > 0) {
    if (footer) footer += " · ";
    footer += `<span style="color:var(--accent);">${formatNumber(needsUpdateCount)} with comment update</span>`;
  }

  return renderBreakdownCard(
    "Files in library",
    "fa-solid fa-file-audio",
    total,
    breakdown,
    footer,
  );
}

/* ================================================================== */
/*  Row 2: Service status cards (TRAKTOR / SPOTIFY / SOUNDCLOUD / YT) */
/* ================================================================== */

/**
 * Render a status card for a service with two key-value rows.
 */
function renderStatusCard(title, icon, statusRows, actionBtn = "") {
  const rowsHtml = statusRows
    .flatMap((r) => [
      `<span class="label">${r.label}</span>`,
      `<span class="value">${r.value}</span>`,
    ])
    .join("");

  return `<div class="dash-card fade-in">
    <div class="dash-card-header">
      <span><i class="${icon}" style="margin-right:4px;color:var(--accent);"></i> ${title}</span>
      ${actionBtn || ""}
    </div>
    <div class="dash-card-body">
      <div class="dash-status-grid">${rowsHtml}</div>
    </div>
  </div>`;
}

function connectionDot(connected, configured) {
  if (!configured) return `<span class="status-dot unconfigured"></span> unconfigured`;
  return connected
    ? `<span class="status-dot connected"></span> connected`
    : `<span class="status-dot disconnected"></span> disconnected`;
}

function renderTraktorCard(traktorData) {
  const hasCollection = traktorData && traktorData.path;
  const modified = hasCollection ? timeAgo(unixTsToMs(traktorData.modifiedAt)) : "—";
  const shortPath = hasCollection
    ? "…/" + traktorData.path.split("/").slice(-3).join("/")
    : "not detected";

  const rows = [
    { label: "Last modified", value: modified },
    { label: "Collection", value: `<span class="font-mono text-xs">${shortPath}</span>` },
  ];

  const btn = `<button class="btn btn-xs btn-primary" data-action="traktor-import">
    <i class="fa-solid fa-upload"></i> Import
  </button>`;

  return renderStatusCard("Traktor Collection", "fa-solid fa-compact-disc", rows, btn);
}

function renderServiceStatusCard(conn) {
  const {
    service,
    connected,
    configured,
    last_sync,
    playlistsLocal,
    tracksLocal,
    playlistsRemote,
    tracksRemote,
  } = conn;
  const icon = serviceIcon(service);
  const title = service.charAt(0).toUpperCase() + service.slice(1);
  const connectedLabel = connectionDot(connected, configured);
  const lastSync = last_sync ? timeAgo(unixTsToMs(last_sync)) : "never";

  const rows = [
    { label: "Status", value: connectedLabel },
    { label: "Last full sync", value: lastSync },
    {
      label: "Tracks",
      value: `${formatNumber(tracksLocal)} <span class="text-xs text-muted">/ ${formatNumber(tracksRemote)}</span>`,
    },
    {
      label: "Playlists",
      value: `${formatNumber(playlistsLocal)} <span class="text-xs text-muted">/ ${formatNumber(playlistsRemote)}</span>`,
    },
  ];

  const btn = `<button class="btn btn-xs btn-primary" data-action="service-sync" data-service="${service}">
    <i class="fa-solid fa-cloud-arrow-up"></i> Sync
  </button>`;

  return renderStatusCard(title, icon, rows, btn);
}

/* ================================================================== */
/*  Row 3: Managed Folders + Subscribed Playlists                       */
/* ================================================================== */

/**
 * Full-width card with title/icon. (Wraps existing card pattern)
 */
function renderCard(title, icon, bodyHtml, footerHtml = "") {
  return `<div class="card fade-in" style="margin-top:0;">
    <h3 class="card-title"><i class="${icon}" style="margin-right:var(--space-2);color:var(--accent);"></i> ${title}</h3>
    ${bodyHtml}
    ${footerHtml ? `<div class="text-xs text-muted" style="margin-top:var(--space-2);">${footerHtml}</div>` : ""}
  </div>`;
}

function renderFoldersCard(folders) {
  if (!folders || folders.length === 0) {
    return renderCard(
      "Managed Folders",
      "fa-solid fa-folder-tree",
      `<div class="empty-state" style="padding:var(--space-4) 0;"><p>No folders configured. <a href="#folders">Add a folder →</a></p></div>`,
    );
  }

  const rows = folders
    .map((f) => {
      const lastScan = f.lastScanned ? timeAgo(unixTsToMs(f.lastScanned)) : "never";
      const fc = f.fileCount ?? "?";
      return `<div class="flex items-center justify-between" style="padding:var(--space-2) 0;border-bottom:1px solid var(--border);">
        <div style="flex:1;min-width:0;">
          <div class="font-mono text-sm" style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${f.path}</div>
          <div class="text-xs text-muted">${fc} files · last scan: <strong>${lastScan}</strong></div>
        </div>
        <button class="btn btn-xs btn-primary" data-action="folder-scan" data-id="${f.id}" style="white-space:nowrap;margin-left:var(--space-2);">
          <i class="fa-solid fa-rotate"></i> Scan
        </button>
      </div>`;
    })
    .join("");

  return renderCard(
    "Managed Folders",
    "fa-solid fa-folder-tree",
    `<div style="margin-top:var(--space-1);">${rows}</div>`,
    `<a href="#folders">Manage folders →</a>`,
  );
}

function renderSubscribedPlaylistsCard(subscriptions) {
  if (!subscriptions || subscriptions.length === 0) {
    return renderCard(
      "Subscribed Playlists",
      "fa-solid fa-list-check",
      `<div class="empty-state" style="padding:var(--space-4) 0;"><p>No subscriptions. <a href="#playlists">Go to Playlists →</a></p></div>`,
    );
  }

  const rows = subscriptions
    .map((sub) => {
      const lastSync = sub.lastPolledAt ? timeAgo(unixTsToMs(sub.lastPolledAt)) : "never";
      const svcIcon = serviceIcon(sub.service);
      const svcColor = serviceColor(sub.service);
      return `<div class="flex items-center justify-between" style="padding:var(--space-2) 0;border-bottom:1px solid var(--border);">
        <div style="flex:1;min-width:0;">
          <div class="text-sm" style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">
            <i class="${svcIcon}" style="color:${svcColor};width:14px;margin-right:4px;font-size:0.7rem;"></i>
            ${sub.playlistName}
          </div>
          <div class="text-xs text-muted">${sub.trackCount} tracks · last synced: <strong>${lastSync}</strong></div>
        </div>
        ${
          sub.service === "spotify"
            ? `<button class="btn btn-xs btn-green" data-action="playlist-sync" data-id="${sub.id}" data-playlist-id="${sub.playlistId}" data-service="${sub.service}" style="white-space:nowrap;margin-left:var(--space-2);">
          <i class="fa-solid fa-cloud-arrow-up"></i> Sync
        </button>`
            : ""
        }
      </div>`;
    })
    .join("");

  return renderCard(
    "Subscribed Playlists",
    "fa-solid fa-list-check",
    `<div style="margin-top:var(--space-1);">${rows}</div>`,
    `<a href="#playlists">Go to Playlists →</a>`,
  );
}

/* ================================================================== */
/*  Row 4: TAGS FROM PLAYLISTS · AUTO-CATEGORIZE · COMMENT DIFFS      */
/* ================================================================== */

function renderTagsFromPlaylistsCard(untaggedCount) {
  return `<div class="dash-card fade-in">
    <div class="dash-card-header">
      <span><i class="fa-solid fa-hashtag" style="margin-right:4px;color:var(--accent);"></i> Tags from Playlists</span>
    </div>
    <div class="dash-card-body" style="display:flex;flex-direction:column;gap:var(--space-3);">
      <div class="flex items-center justify-between">
        <span class="text-muted text-sm">Playlists without a tag:</span>
        <strong>${formatNumber(untaggedCount)}</strong>
      </div>
      <button class="btn btn-sm btn-primary" data-action="create-tags-from-playlists" style="width:100%;">
        <i class="fa-solid fa-tag"></i> Create Tags from Playlists
      </button>
      <a href="#playlists?untagged=1" class="btn btn-sm btn-yellow" style="width:100%;text-align:center;">
        <i class="fa-solid fa-eye"></i> View Untagged
      </a>
    </div>
  </div>`;
}

function renderAutoCategorizeCard(unreviewedCount) {
  return `<div class="dash-card fade-in">
    <div class="dash-card-header">
      <span><i class="fa-solid fa-brain" style="margin-right:4px;color:var(--accent);"></i> Auto-Categorize</span>
    </div>
    <div class="dash-card-body" style="display:flex;flex-direction:column;gap:var(--space-3);">
      <div class="flex items-center justify-between">
        <span class="text-muted text-sm">Tags pending review:</span>
        <strong>${formatNumber(unreviewedCount)}</strong>
      </div>
      <a href="#auto-categorize" class="btn btn-sm btn-purple" style="width:100%;text-align:center;">
        <i class="fa-solid fa-arrow-right"></i> Go to Auto-Categorize
      </a>
    </div>
  </div>`;
}

function renderCommentDiffsCard(needsUpdateCount) {
  return `<div class="dash-card fade-in">
    <div class="dash-card-header">
      <span><i class="fa-solid fa-pen-to-square" style="margin-right:4px;color:var(--accent);"></i> Comment Diffs</span>
    </div>
    <div class="dash-card-body" style="display:flex;flex-direction:column;gap:var(--space-3);">
      <div class="flex items-center justify-between">
        <span class="text-muted text-sm">Files needing update:</span>
        <strong>${formatNumber(needsUpdateCount)}</strong>
      </div>
      ${renderCommentWriter({ linkedOnly: true, tagNames: [], nonDefaultOnly: true })}
    </div>
  </div>`;
}

/* ================================================================== */
/*  Event wiring                                                        */
/* ================================================================== */

function wireEvents(container, signal) {
  // --- Traktor Import ---
  container.addEventListener("click", async (e) => {
    if (signal.aborted) return;
    const btn = e.target.closest("[data-action='traktor-import']");
    if (!btn) return;

    btn.disabled = true;
    btn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i> Importing…';
    try {
      const resp = await fetchJSON("/api/traktor/import", { method: "POST" });
      showToast(`Traktor import started (task: ${resp.data.taskId})`, "success");
      setTimeout(() => reinit(container, signal), 2000);
    } catch (err) {
      showToast(`Traktor import failed: ${err.message}`, "error");
      btn.disabled = false;
      btn.innerHTML = '<i class="fa-solid fa-upload"></i> Import';
    }
  });

  // --- Service Sync ---
  container.addEventListener("click", async (e) => {
    if (signal.aborted) return;
    const btn = e.target.closest("[data-action='service-sync']");
    if (!btn) return;

    const service = btn.dataset.service;
    btn.disabled = true;
    btn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i>';
    try {
      await fetchJSON(`/api/services/${service}/sync`, { method: "POST" });
      showToast(`${service} sync started`, "success");
      setTimeout(() => reinit(container, signal), 2000);
    } catch (err) {
      showToast(`${service} sync failed: ${err.message}`, "error");
      btn.disabled = false;
      btn.innerHTML = '<i class="fa-solid fa-cloud-arrow-up"></i> Sync';
    }
  });

  // --- Folder Scan ---
  container.addEventListener("click", async (e) => {
    if (signal.aborted) return;
    const btn = e.target.closest("[data-action='folder-scan']");
    if (!btn) return;

    btn.disabled = true;
    btn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i>';
    try {
      await fetchJSON(`/api/folders/${btn.dataset.id}/scan`, { method: "POST" });
      showToast("Folder scan started", "success");
      setTimeout(() => reinit(container, signal), 2000);
    } catch (err) {
      showToast(`Scan failed: ${err.message}`, "error");
      btn.disabled = false;
      btn.innerHTML = '<i class="fa-solid fa-rotate"></i> Scan';
    }
  });

  // --- Playlist Sync (any service) ---
  container.addEventListener("click", async (e) => {
    if (signal.aborted) return;
    const btn = e.target.closest("[data-action='playlist-sync']");
    if (!btn) return;

    btn.disabled = true;
    btn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i>';
    try {
      await fetchJSON(
        `/api/services/${btn.dataset.service}/sync/playlists/${btn.dataset.playlistId}/tracks`,
        { method: "POST" },
      );
      showToast("Playlist sync started", "success");
      setTimeout(() => reinit(container, signal), 2000);
    } catch (err) {
      showToast(`Sync failed: ${err.message}`, "error");
      btn.disabled = false;
      btn.innerHTML = '<i class="fa-solid fa-cloud-arrow-up"></i> Sync';
    }
  });

  // --- Create Tags from Playlists ---
  container.addEventListener("click", async (e) => {
    if (signal.aborted) return;
    const btn = e.target.closest("[data-action='create-tags-from-playlists']");
    if (!btn) return;

    btn.disabled = true;
    btn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i> Creating…';
    try {
      const resp = await fetchJSON("/api/tags/create-from-playlists", { method: "POST" });
      showToast(resp.data?.message || "Tags created", "success");
      setTimeout(() => reinit(container, signal), 2000);
    } catch (err) {
      showToast(`Failed: ${err.message}`, "error");
      btn.disabled = false;
      btn.innerHTML = '<i class="fa-solid fa-tag"></i> Create Tags from Playlists';
    }
  });

  // --- Write All Comments (via shared comment-writer) ---
  wireCommentWriter(container, signal, async (linkedOnly, tagNames, nonDefaultOnly) => {
    const execBtn = container.querySelector("#cw-execute");
    if (!execBtn) return;
    execBtn.disabled = true;
    const originalHtml = execBtn.innerHTML;
    execBtn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i> Writing…';
    try {
      const body = {
        linkedOnly,
        tags: tagNames.length > 0 ? tagNames : undefined,
        nonDefaultOnly,
      };
      await fetchJSON("/api/files/write-comments", {
        method: "POST",
        body: JSON.stringify(body),
      });
      showToast("Comments written successfully", "success");
      setTimeout(() => reinit(container, signal), 2000);
    } catch (err) {
      showToast(`Write failed: ${err.message}`, "error");
      execBtn.disabled = false;
      execBtn.innerHTML = originalHtml;
    }
  });
}

/* ================================================================== */
/*  Re-init wrapper                                                     */
/* ================================================================== */

async function reinit(container, signal) {
  await init(container, signal);
}

/* ================================================================== */
/*  Initialisation                                                      */
/* ================================================================== */

export async function init(container, signal) {
  container.innerHTML = renderLoading("Loading dashboard…");

  try {
    const [
      filesResp,
      servicesResp,
      svcLinksResp,
      foldersResp,
      traktorResp,
      unreviewedResp,
      untaggedResp,
      needsUpdateResp,
      subscriptionsResp,
      tagCoverageResp,
    ] = await Promise.all([
      fetchJSON("/api/files/count", { signal }),
      fetchJSON("/api/services", { signal }),
      fetchJSON("/api/files/service-links", { signal }),
      fetchJSON("/api/folders", { signal }),
      fetchJSON("/api/traktor/status", { signal }),
      fetchJSON("/api/tags/unreviewed", { signal }),
      fetchJSON("/api/tags/from-playlists", { signal }),
      fetchJSON("/api/files/needs-update-count", { signal }),
      fetchJSON("/api/playlists/subscriptions", { signal }),
      fetchJSON("/api/tags/service-coverage", { signal }),
    ]);

    if (signal.aborted) return;

    // --- Extract data ---
    const filesCount = filesResp.data ?? 0;
    const svcConns = servicesResp.data ?? [];
    const svcLinks = svcLinksResp.data;
    const unlinked = svcLinks?.unlinked ?? 0;
    const folders = foldersResp.data ?? [];
    const traktorData = traktorResp.data;
    const unreviewedCount = unreviewedResp.data?.total_unreviewed ?? 0;
    const untaggedCount = untaggedResp.data?.count ?? 0;
    const needsUpdateCount = needsUpdateResp.data ?? 0;
    const allSubs = subscriptionsResp.data?.subscriptions ?? [];
    const tagCoverage = tagCoverageResp.data;

    // Build HTML

    // Row 1 – Service breakdown (4 columns)
    const row1Html = `<div class="dashboard-four-col">
      ${renderTagsCard(tagCoverage)}
      ${renderPlaylistsCard(svcConns)}
      ${renderTracksCard(svcConns)}
      ${renderFilesCard(filesCount, svcLinks, unlinked, needsUpdateCount)}
    </div>`;

    // Row 2 – Service status (4 columns)
    const spotifyConn = svcConns.find((s) => s.service === "spotify");
    const soundcloudConn = svcConns.find((s) => s.service === "soundcloud");
    const youtubeConn = svcConns.find((s) => s.service === "youtube");

    const row2Html = `<div class="dashboard-four-col">
      ${renderTraktorCard(traktorData)}
      ${spotifyConn ? renderServiceStatusCard(spotifyConn) : ""}
      ${soundcloudConn ? renderServiceStatusCard(soundcloudConn) : ""}
      ${youtubeConn ? renderServiceStatusCard(youtubeConn) : ""}
    </div>`;

    // Row 3 – Folders + Subscribed Playlists (2 columns)
    const row3Html = `<div class="dashboard-two-col" style="margin-bottom:var(--space-6);">
      ${renderFoldersCard(folders)}
      ${renderSubscribedPlaylistsCard(allSubs)}
    </div>`;

    // Row 4 – Action cards (3 columns)
    const row4Html = `<div class="dashboard-three-col">
      ${renderTagsFromPlaylistsCard(untaggedCount)}
      ${renderAutoCategorizeCard(unreviewedCount)}
      ${renderCommentDiffsCard(needsUpdateCount)}
    </div>`;

    container.innerHTML = `
      ${row1Html}
      ${row2Html}
      ${row3Html}
      ${row4Html}
    `;

    wireEvents(container, signal);
  } catch (err) {
    if (err.name === "AbortError") return;
    container.innerHTML = renderErrorBlock({
      title: "Failed to load dashboard",
      detail: err.message,
      retryFn: "window.location.hash='#dashboard'",
    });
  }
}
