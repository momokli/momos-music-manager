/**
 * file-detail.js — Single file detail page.
 *
 * Shows all metadata for a local file: file info, linked service tracks
 * with audio features, tags, and playlists.
 *   #file-detail?id=<file_id>
 *
 * API: GET /api/files/{id}/detail
 */

import { fetchJSON } from "../shared/api.js";

/* ------------------------------------------------------------------ */
/*  State                                                              */
/* ------------------------------------------------------------------ */

let abortController = null;

/* ------------------------------------------------------------------ */
/*  Initialization                                                     */
/* ------------------------------------------------------------------ */

export async function init(container, signal) {
  const id = getIdFromHash();
  if (!id) {
    container.innerHTML = renderError("No file ID specified. Use #file-detail?id=123");
    return;
  }

  abortController = new AbortController();
  const combinedSignal = signal || abortController.signal;

  container.innerHTML = renderLoading();

  let data;
  let variants;

  try {
    const resp = await fetchJSON(`/api/files/${id}/detail`, { signal: combinedSignal });
    data = resp.data || resp;
  } catch (err) {
    if (combinedSignal.aborted) return;
    container.innerHTML = renderError(`Failed to load: ${err.message}`);
    return;
  }

  try {
    if (combinedSignal.aborted) return;
    const vResp = await fetchJSON(`/api/files/${id}/variants`, {
      signal: combinedSignal,
    });
    variants = vResp.data || vResp;
  } catch (err) {
    if (combinedSignal.aborted) return;
    // Variants are optional — don't fail the page
    variants = null;
  }

  container.innerHTML = renderPage(data, variants);
}

/* ------------------------------------------------------------------ */
/*  Layout                                                             */
/* ------------------------------------------------------------------ */

function renderLoading() {
  return `<div class="detail-loading"><i class="fa-solid fa-spinner fa-spin"></i> Loading file details…</div>`;
}

function renderError(msg) {
  return `<div class="detail-error"><i class="fa-solid fa-triangle-exclamation"></i> ${escHtml(msg)}</div>`;
}

function renderPage(data, variants) {
  const f = data.file || data;
  const tracks = data.tracks || [];
  const tags = data.tags || [];
  const playlists = data.playlists || [];
  const filePath = f.filePath || "";
  const fileName = filePath.split("/").pop() || "(unknown)";

  return /* html */ `
    <div class="page-header">
      <h1><i class="fa-solid fa-file-audio"></i> ${escHtml(f.title || fileName)}</h1>
      <span class="page-subtitle">${escHtml(f.artist || "Unknown Artist")}</span>
      <div class="detail-subtitle-meta">
        <span class="text-muted text-sm">${escHtml(filePath)}</span>
        ${f.fileType ? `<span class="service-badge">${escHtml(f.fileType.toUpperCase())}</span>` : ""}
      </div>
    </div>

    <div class="detail-grid">
      ${renderSection("📄 File Info", renderFileInfo(f))}
      ${tracks.length > 0 ? renderSection("🔗 Linked Tracks", renderTrackCards(tracks, f)) : ""}
      ${tags.length > 0 ? renderSection("🏷 Tags", renderTags(tags)) : ""}
      ${playlists.length > 0 ? renderSection("📋 Playlists", renderPlaylists(playlists)) : ""}
      ${variants && variants.variants && variants.variants.length > 0 ? renderSection("🎛 Variants", renderVariants(variants.variants)) : ""}
    </div>
  `;
}

/* ------------------------------------------------------------------ */
/*  Sections                                                           */
/* ------------------------------------------------------------------ */

function renderSection(title, body) {
  return /* html */ `
    <div class="detail-section">
      <h2 class="detail-section-title">${title}</h2>
      ${body}
    </div>
  `;
}

function renderFileInfo(f) {
  return renderKvTable([
    [
      "Path",
      f.isLocal
        ? escHtml(f.filePath || "—")
        : '<span style="opacity:0.5;text-decoration:line-through">' +
          escHtml(f.filePath || "—") +
          '</span><div style="color:var(--muted);font-size:0.8rem;margin-top:2px">↳ On backup: ' +
          escHtml(f.backupPath || "—") +
          "</div>",
    ],
    [
      "On Disk",
      f.isLocal
        ? '<span style="color:var(--green)">✓ Yes</span>'
        : '<span style="color:var(--red)">✗ No — backup only</span>',
    ],
    [
      "Backed Up",
      f.backedUp
        ? '<span style="color:var(--green)">✓ Yes' +
          (f.backupPath ? " — " + escHtml(f.backupPath) : "") +
          "</span>"
        : '<span style="color:var(--text-muted)">— No</span>',
    ],
    ["Type", f.fileType ? escHtml(f.fileType.toUpperCase()) : "—"],
    ["Size", f.fileSize != null ? formatBytes(f.fileSize) : "—"],
    ["ISRC", escHtml(f.isrc || "—")],
    ["Album", escHtml(f.album || "—")],
    ["Genre", escHtml(f.genre || "—")],
    ["Year", f.year != null ? String(f.year) : "—"],
    ["Duration", f.durationMs != null ? formatDuration(f.durationMs) : "—"],
    ["Bitrate", f.bitrate != null ? `${f.bitrate} kbps` : "—"],
    [
      "Sample Rate",
      f.sampleRate != null ? `${(f.sampleRate / 1000).toFixed(1)} kHz` : "—",
    ],
    [
      "Channels",
      f.channels != null ? (f.channels === 2 ? "Stereo" : String(f.channels)) : "—",
    ],
    ["BPM", f.bpm != null ? f.bpm.toFixed(1) : "—"],
    ["Key (Camelot)", escHtml(f.musicalKey || "—")],
    ["Comment", f.comment ? `<code>${escHtml(f.comment)}</code>` : "—"],
    ["Rating", f.rating != null ? starRating(f.rating) : "—"],
    ["Play Count", f.playCount != null ? String(f.playCount) : "—"],
    ["Last Played", f.lastPlayed != null ? formatDate(f.lastPlayed) : "—"],
  ]);
}

function renderTrackCards(tracks, file) {
  return /* html */ `
    <div class="detail-track-list">
      ${tracks.map((t) => renderTrackCard(t, file)).join("")}
    </div>
  `;
}

function renderTrackCard(t, file) {
  const hasSpotifyFeatures = t.spotifyTempo != null || t.spotifyDanceability != null;
  const serviceIcon = getServiceIcon(t.service);
  const serviceBadgeClass =
    t.service === "spotify"
      ? "spotify"
      : t.service === "soundcloud"
        ? "soundcloud"
        : t.service === "youtube"
          ? "youtube"
          : "";

  // BPM comparison row
  let bpmRow = "";
  if (file.bpm != null && t.spotifyTempo != null) {
    const match = Math.abs(file.bpm - t.spotifyTempo) <= 1;
    bpmRow = /* html */ `<tr>
      <th>Tempo</th>
      <td>${file.bpm.toFixed(1)} vs ${t.spotifyTempo.toFixed(1)} <span class="detail-${match ? "match" : "mismatch"}">${match ? "✓" : "✗"}</span></td>
    </tr>`;
  }

  // Key comparison row
  let keyRow = "";
  if (file.musicalKey != null && t.spotifyKeyCamelot != null) {
    const match = file.musicalKey === t.spotifyKeyCamelot;
    keyRow = /* html */ `<tr>
      <th>Key</th>
      <td>${escHtml(file.musicalKey)} vs ${escHtml(t.spotifyKeyCamelot)} <span class="detail-${match ? "match" : "mismatch"}">${match ? "✓" : "✗"}</span></td>
    </tr>`;
  }

  return /* html */ `
    <div class="detail-track-card">
      <div class="detail-track-card-header">
        <span class="detail-sr-only">${escHtml(t.service)}</span>
        <span class="service-badge ${serviceBadgeClass}">
          <i class="${serviceIcon}"></i>
        </span>
        <span class="detail-track-title">${escHtml(t.title || "—")}</span>
        <span class="detail-track-artist">${escHtml(t.artist || "")}</span>
        <span class="detail-track-pop">${t.popularity != null ? `${t.popularity}/100` : ""}</span>
      </div>
      <table class="detail-kv">
        <tbody>
          <tr><th>Service ID</th><td><code>${escHtml(t.serviceId || "—")}</code></td></tr>
          <tr><th>Album</th><td>${escHtml(t.album || "—")}</td></tr>
          <tr><th>ISRC</th><td>${escHtml(t.isrc || "—")}</td></tr>
          <tr><th>Duration</th><td>${t.durationMs != null ? formatDuration(t.durationMs) : "—"}</td></tr>
          ${bpmRow}
          ${keyRow}
          ${
            hasSpotifyFeatures
              ? `
          <tr><th>Tempo (Spotify)</th><td>${t.spotifyTempo != null ? t.spotifyTempo.toFixed(2) : "—"}</td></tr>
          <tr><th>Key (Camelot)</th><td>${escHtml(t.spotifyKeyCamelot || "—")}</td></tr>
          <tr><th>Danceability</th><td>${t.spotifyDanceability != null ? t.spotifyDanceability.toFixed(3) : "—"}</td></tr>
          <tr><th>Energy</th><td>${t.spotifyEnergy != null ? t.spotifyEnergy.toFixed(3) : "—"}</td></tr>
          <tr><th>Valence</th><td>${t.spotifyValence != null ? t.spotifyValence.toFixed(3) : "—"}</td></tr>
          <tr><th>Acousticness</th><td>${t.spotifyAcousticness != null ? t.spotifyAcousticness.toFixed(3) : "—"}</td></tr>
          <tr><th>Instrumentalness</th><td>${t.spotifyInstrumentalness != null ? t.spotifyInstrumentalness.toFixed(3) : "—"}</td></tr>
          <tr><th>Liveness</th><td>${t.spotifyLiveness != null ? t.spotifyLiveness.toFixed(3) : "—"}</td></tr>
          <tr><th>Speechiness</th><td>${t.spotifySpeechiness != null ? t.spotifySpeechiness.toFixed(3) : "—"}</td></tr>
          <tr><th>Loudness</th><td>${t.spotifyLoudness != null ? `${t.spotifyLoudness.toFixed(1)} dB` : "—"}</td></tr>
          <tr><th>Time Signature</th><td>${t.spotifyTimeSignature != null ? `${t.spotifyTimeSignature}/4` : "—"}</td></tr>
          `
              : ""
          }
        </tbody>
      </table>
    </div>
  `;
}

function renderTags(tags) {
  return /* html */ `
    <div class="detail-tags">
      ${tags
        .map(
          (t) => `
        <span class="tag-chip" title="${escHtml(t.categoryName)}">
          ${escHtml(t.prefix)}: ${escHtml(t.name)}
        </span>
      `,
        )
        .join("")}
    </div>
  `;
}

function renderPlaylists(playlists) {
  return /* html */ `
    <div class="detail-playlists">
      ${playlists
        .map(
          (p) => `
        <a href="#playlists" class="detail-playlist-link" title="${escHtml(p.service)}">
          <i class="${getServiceIcon(p.service)}"></i>
          ${escHtml(p.name)}
        </a>
      `,
        )
        .join("")}
    </div>
  `;
}

/* ------------------------------------------------------------------ */
/*  Variants                                                            */
/* ------------------------------------------------------------------ */

function renderVariants(variants) {
  return /* html */ `
    <div class="variants-list">
      ${variants.map((v) => renderVariantCard(v)).join("")}
    </div>
  `;
}

function renderVariantCard(v) {
  const typeLabel = v.fileType.toUpperCase();
  const typeBadgeClass =
    v.fileType === "stem.m4a"
      ? "variant-badge-stem"
      : v.fileType === "flac"
        ? "variant-badge-flac"
        : v.fileType === "wav"
          ? "variant-badge-wav"
          : "variant-badge-other";

  const stemTypeHtml = v.stemType
    ? `<span class="variant-stem-type">${escHtml(v.stemType)}</span>`
    : "";

  const backupIcon = v.backedUp
    ? '<span class="variant-backed-up" title="Backed up">✓</span>'
    : '<span class="variant-not-backed-up" title="Not backed up">✗</span>';

  const localIcon = v.isLocal
    ? '<span class="variant-local" title="On disk">💻</span>'
    : '<span class="variant-backup-only" title="Backup only">💾</span>';

  const fileName = (v.filePath || "").split("/").pop() || "";

  return /* html */ `
    <div class="variant-card" data-file-id="${v.id}">
      <span class="variant-badge ${typeBadgeClass}">${escHtml(typeLabel)}</span>
      ${stemTypeHtml}
      <span class="variant-filename" title="${escHtml(v.filePath)}">${escHtml(fileName)}</span>
      <span class="variant-size">${formatBytes(v.fileSize)}</span>
      ${localIcon}
      ${backupIcon}
    </div>
  `;
}

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

function renderKvTable(rows) {
  return /* html */ `
    <table class="detail-kv">
      <tbody>
        ${rows
          .filter(([l]) => l)
          .map(
            ([label, value]) => `
          <tr>
            <th>${label}</th>
            <td>${value}</td>
          </tr>
        `,
          )
          .join("")}
      </tbody>
    </table>
  `;
}

function getServiceIcon(service) {
  switch ((service || "").toLowerCase()) {
    case "spotify":
      return "fa-brands fa-spotify";
    case "soundcloud":
      return "fa-brands fa-soundcloud";
    case "youtube":
      return "fa-brands fa-youtube";
    case "local":
      return "fa-solid fa-hard-drive";
    default:
      return "fa-solid fa-music";
  }
}

function formatBytes(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function formatDuration(ms) {
  const s = Math.floor(ms / 1000);
  const m = Math.floor(s / 60);
  const sec = s % 60;
  return `${m}:${String(sec).padStart(2, "0")}`;
}

function formatDate(ts) {
  return new Date(ts * 1000).toLocaleDateString();
}

function starRating(rating) {
  const clamped = Math.max(0, Math.min(5, Math.round(rating)));
  return "★".repeat(clamped) + "☆".repeat(5 - clamped);
}

function escHtml(s) {
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function getIdFromHash() {
  const raw = window.location.hash.replace(/^#/, "");
  const [page, query] = raw.split("?");
  if (page !== "file-detail" || !query) return null;
  const params = new URLSearchParams(query);
  return parseInt(params.get("id"), 10) || null;
}
