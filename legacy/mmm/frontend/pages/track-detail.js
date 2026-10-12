/**
 * track-detail.js — Single track detail page.
 *
 * Shows all metadata for a service track: track info, Spotify audio features,
 * linked local files, tags, and playlists.
 *   #track-detail?id=<track_id>
 *
 * API: GET /api/tracks/{id}/detail
 */

import { fetchJSON } from "../shared/api.js";
import { showToast } from "../shared/components.js";

/* ------------------------------------------------------------------ */
/*  State                                                              */
/* ------------------------------------------------------------------ */

let abortController = null;
let currentTrackId = null;
let typeaheadTimer = null;

/* ------------------------------------------------------------------ */
/*  Initialization                                                     */
/* ------------------------------------------------------------------ */

export async function init(container, signal) {
  const id = getIdFromHash();
  if (!id) {
    container.innerHTML = renderError("No track ID specified. Use #track-detail?id=123");
    return;
  }

  abortController = new AbortController();
  const combinedSignal = signal || abortController.signal;

  container.innerHTML = renderLoading();

  currentTrackId = id;

  try {
    const resp = await fetchJSON(`/api/tracks/${id}/detail`, { signal: combinedSignal });
    const data = resp.data || resp;
    container.innerHTML = renderPage(data);
    wireCorrectionEvents(container, id);
  } catch (err) {
    if (combinedSignal.aborted) return;
    container.innerHTML = renderError(`Failed to load: ${err.message}`);
  }
}

/* ------------------------------------------------------------------ */
/*  Layout                                                             */
/* ------------------------------------------------------------------ */

function renderLoading() {
  return `<div class="detail-loading"><i class="fa-solid fa-spinner fa-spin"></i> Loading track details…</div>`;
}

function renderError(msg) {
  return `<div class="detail-error"><i class="fa-solid fa-triangle-exclamation"></i> ${escHtml(msg)}</div>`;
}

function renderPage(d) {
  const allFiles = d.files || [];
  const tags = d.tags || [];
  const playlists = d.playlists || [];
  const hasAudioFeatures = d.spotifyTempo != null || d.spotifyDanceability != null;

  // Separate primary files (stems, FLACs) from WAV source files (linked via source_of)
  const primaryFiles = allFiles.filter((f) => !f.stemType);
  const wavFiles = allFiles.filter((f) => f.stemType);
  const serviceBadgeClass =
    d.service === "spotify"
      ? "spotify"
      : d.service === "soundcloud"
        ? "soundcloud"
        : d.service === "youtube"
          ? "youtube"
          : "";
  const serviceIcon = getServiceIcon(d.service);

  return /* html */ `
    <div class="page-header">
      <h1><i class="fa-solid fa-stream"></i> ${escHtml(d.title || "(untitled)")}</h1>
      <span class="page-subtitle">${escHtml(d.artist || "Unknown Artist")}</span>
      <div class="detail-subtitle-meta">
        <span class="service-badge ${serviceBadgeClass}">
          <i class="${serviceIcon}"></i>
          ${escHtml(d.service || "")}
        </span>
      </div>
    </div>

    <div class="detail-grid">
      ${renderSection("🎵 Track Info", renderTrackInfo(d))}
      ${hasAudioFeatures ? renderSection("📊 Audio Features", renderAudioFeatures(d)) : ""}
      ${primaryFiles.length > 0 ? renderSection("💾 Linked Files", renderFileCards(primaryFiles, d)) : ""}
      ${tags.length > 0 ? renderSection("🏷 Tags", renderTags(tags)) : ""}
      ${playlists.length > 0 ? renderSection("📋 Playlists", renderPlaylists(playlists)) : ""}
      ${wavFiles.length > 0 ? renderSection("🎛 WAV Sources", renderWavSources(wavFiles)) : ""}
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

function renderTrackInfo(d) {
  return renderKvTable([
    ["Service", escHtml(d.service || "—")],
    ["Service ID", d.serviceId ? `<code>${escHtml(d.serviceId)}</code>` : "—"],
    ["ISRC", escHtml(d.isrc || "—")],
    ["Album", escHtml(d.album || "—")],
    ["Duration", d.durationMs != null ? formatDuration(d.durationMs) : "—"],
    ["Popularity", d.popularity != null ? `${d.popularity}/100` : "—"],
  ]);
}

function renderAudioFeatures(d) {
  return renderKvTable([
    ["Tempo", d.spotifyTempo != null ? d.spotifyTempo.toFixed(2) : "—"],
    ["Key (Camelot)", escHtml(d.spotifyKeyCamelot || "—")],
    ["Raw Key", d.spotifyKeyRaw != null ? String(d.spotifyKeyRaw) : "—"],
    [
      "Mode",
      d.spotifyMode != null
        ? d.spotifyMode === 0
          ? "Minor"
          : d.spotifyMode === 1
            ? "Major"
            : "—"
        : "—",
    ],
    [
      "Danceability",
      d.spotifyDanceability != null ? d.spotifyDanceability.toFixed(3) : "—",
    ],
    ["Energy", d.spotifyEnergy != null ? d.spotifyEnergy.toFixed(3) : "—"],
    ["Valence", d.spotifyValence != null ? d.spotifyValence.toFixed(3) : "—"],
    [
      "Acousticness",
      d.spotifyAcousticness != null ? d.spotifyAcousticness.toFixed(3) : "—",
    ],
    [
      "Instrumentalness",
      d.spotifyInstrumentalness != null ? d.spotifyInstrumentalness.toFixed(3) : "—",
    ],
    ["Liveness", d.spotifyLiveness != null ? d.spotifyLiveness.toFixed(3) : "—"],
    ["Speechiness", d.spotifySpeechiness != null ? d.spotifySpeechiness.toFixed(3) : "—"],
    ["Loudness", d.spotifyLoudness != null ? `${d.spotifyLoudness.toFixed(1)} dB` : "—"],
    [
      "Time Signature",
      d.spotifyTimeSignature != null ? `${d.spotifyTimeSignature}/4` : "—",
    ],
  ]);
}

function renderFileCards(files, track) {
  return /* html */ `
    <div class="detail-file-list">
      ${files.map((f) => renderFileCard(f, track)).join("")}
    </div>
    <div class="link-file-typeahead">
      <input
        type="text"
        class="input-text input-search"
        id="link-file-search"
        placeholder="Link a file… (search by name)"
        autocomplete="off"
      />
      <div class="tag-dropdown" id="link-file-dropdown"></div>
    </div>
  `;
}

function renderFileCard(f, track) {
  // BPM comparison row
  let bpmRow = "";
  if (f.bpm != null && track.spotifyTempo != null) {
    const match = Math.abs(f.bpm - track.spotifyTempo) <= 1;
    bpmRow = /* html */ `<tr>
      <th>Tempo</th>
      <td>${f.bpm.toFixed(1)} vs ${track.spotifyTempo.toFixed(1)} <span class="detail-${match ? "match" : "mismatch"}">${match ? "✓" : "✗"}</span></td>
    </tr>`;
  }

  // Key comparison row
  let keyRow = "";
  if (f.musicalKey != null && track.spotifyKeyCamelot != null) {
    const match = f.musicalKey === track.spotifyKeyCamelot;
    keyRow = /* html */ `<tr>
      <th>Key</th>
      <td>${escHtml(f.musicalKey)} vs ${escHtml(track.spotifyKeyCamelot)} <span class="detail-${match ? "match" : "mismatch"}">${match ? "✓" : "✗"}</span></td>
    </tr>`;
  }

  return /* html */ `
    <div class="detail-file-card">
      <div class="detail-file-card-header">
        <span class="service-badge">${escHtml(f.fileType ? f.fileType.toUpperCase() : "FILE")}</span>
        <span class="detail-file-title">${escHtml(f.title || f.filePath?.split("/").pop() || "—")}</span>
        <button class="disconnect-btn" data-file-id="${f.id}" title="Disconnect this file from track">
          <i class="fa-solid fa-xmark"></i>
        </button>
      </div>
      <table class="detail-kv">
        <tbody>
          <tr><th>On Disk</th><td>${f.isLocal ? '<span style="color:var(--green)">✓ Yes</span>' : '<span style="color:var(--red)">✗ No — backup only</span>'}</td></tr>
          <tr><th>Backed Up</th><td>${f.backedUp ? '<span style="color:var(--green)">✓ Yes' + (f.backupPath ? " — " + escHtml(f.backupPath) : "") + "</span>" : '<span style="color:var(--text-muted)">— No</span>'}</td></tr>
          <tr><th>File Path</th><td>${
            f.isLocal
              ? `<code>${escHtml(f.filePath || "—")}</code>`
              : `<code style="opacity:0.5;text-decoration:line-through">${escHtml(f.filePath || "—")}</code>
               <div style="color:var(--muted);font-size:0.8rem;margin-top:2px">↳ On backup: ${escHtml(f.backupPath || "—")}</div>`
          }</td></tr>
          <tr><th>ISRC</th><td>${escHtml(f.isrc || "—")}</td></tr>
          <tr><th>Album</th><td>${escHtml(f.album || "—")}</td></tr>
          <tr><th>BPM</th><td>${f.bpm != null ? f.bpm.toFixed(1) : "—"}</td></tr>
          <tr><th>Key (Camelot)</th><td>${escHtml(f.musicalKey || "—")}</td></tr>
          <tr><th>Duration</th><td>${f.durationMs != null ? formatDuration(f.durationMs) : "—"}</td></tr>
          <tr><th>Bitrate</th><td>${f.bitrate != null ? `${f.bitrate} kbps` : "—"}</td></tr>
          <tr><th>Sample Rate</th><td>${f.sampleRate != null ? `${(f.sampleRate / 1000).toFixed(1)} kHz` : "—"}</td></tr>
          <tr><th>Channels</th><td>${f.channels != null ? (f.channels === 2 ? "Stereo" : String(f.channels)) : "—"}</td></tr>
          ${bpmRow}
          ${keyRow}
          <tr><th>Comment</th><td>${f.comment ? `<code>${escHtml(f.comment)}</code>` : "—"}</td></tr>
          <tr><th>Rating</th><td>${f.rating != null ? starRating(f.rating) : "—"}</td></tr>
          <tr><th>Play Count</th><td>${f.playCount != null ? String(f.playCount) : "—"}</td></tr>
          <tr><th>Last Played</th><td>${f.lastPlayed != null ? formatDate(f.lastPlayed) : "—"}</td></tr>
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

/* ------------------------------------------------------------------ */
/*  WAV Source Variants                                                */
/* ------------------------------------------------------------------ */

function renderWavSources(wavFiles) {
  return /* html */ `
    <div class="variants-list">
      ${wavFiles.map((w) => renderWavCard(w)).join("")}
    </div>
  `;
}

function renderWavCard(w) {
  const fileName = (w.filePath || "").split("/").pop() || "";
  const stemLabel = w.stemType
    ? w.stemType.charAt(0).toUpperCase() + w.stemType.slice(1)
    : "";
  const localIcon = w.isLocal
    ? '<span class="variant-local" title="On disk">💻</span>'
    : '<span class="variant-backup-only" title="Backup only">💾</span>';
  const backupIcon = w.backedUp
    ? '<span class="variant-backed-up" title="Backed up">&#10003;</span>'
    : '<span class="variant-not-backed-up" title="Not backed up">&#10007;</span>';

  return /* html */ `
    <div class="variant-card">
      <span class="variant-badge variant-badge-wav">WAV</span>
      <span class="variant-stem-type">${escHtml(stemLabel)}</span>
      <span class="variant-filename" title="${escHtml(w.filePath)}">${escHtml(fileName)}</span>
      <span class="variant-size">${formatBytes(w.fileSize)}</span>
      ${localIcon}
      ${backupIcon}
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
/*  Correction Events                                                  */
/* ------------------------------------------------------------------ */

function wireCorrectionEvents(container, trackId) {
  // Disconnect button clicks
  container.querySelectorAll(".disconnect-btn").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const fileId = parseInt(btn.dataset.fileId, 10);
      if (!fileId) return;
      await disconnectFile(fileId, trackId, btn);
    });
  });

  // Typeahead input
  const searchInput = container.querySelector("#link-file-search");
  const dropdown = container.querySelector("#link-file-dropdown");
  if (!searchInput || !dropdown) return;

  searchInput.addEventListener("input", () => {
    clearTimeout(typeaheadTimer);
    const q = searchInput.value.trim();
    if (q.length < 2) {
      dropdown.classList.remove("open");
      dropdown.innerHTML = "";
      return;
    }
    typeaheadTimer = setTimeout(
      () => searchFilesForLinking(q, dropdown, trackId, searchInput),
      250,
    );
  });

  // Keyboard navigation
  searchInput.addEventListener("keydown", (e) => {
    const items = dropdown.querySelectorAll(".tag-dropdown-item");
    const active = dropdown.querySelector(".tag-dropdown-item.active");
    let idx = Array.from(items).indexOf(active);

    if (e.key === "ArrowDown") {
      e.preventDefault();
      idx = Math.min(idx + 1, items.length - 1);
      items.forEach((it) => it.classList.remove("active"));
      if (items[idx]) items[idx].classList.add("active");
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      idx = Math.max(idx - 1, 0);
      items.forEach((it) => it.classList.remove("active"));
      if (items[idx]) items[idx].classList.add("active");
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (active) active.click();
    } else if (e.key === "Escape") {
      dropdown.classList.remove("open");
      dropdown.innerHTML = "";
      searchInput.blur();
    }
  });

  // Click outside closes dropdown
  document.addEventListener("click", (e) => {
    if (!searchInput.contains(e.target) && !dropdown.contains(e.target)) {
      dropdown.classList.remove("open");
      dropdown.innerHTML = "";
    }
  });
}

async function disconnectFile(fileId, trackId, btn) {
  const origHtml = btn.innerHTML;
  btn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i>';
  btn.disabled = true;
  try {
    await fetchJSON(`/api/files/${fileId}/track-corrections`, {
      method: "PUT",
      body: JSON.stringify({
        corrections: [{ trackId, linkType: "exclude" }],
      }),
    });
    showToast("File disconnected from track", "success");
    // Re-fetch the page
    await refreshDetail();
  } catch (err) {
    showToast(`Failed: ${err.message}`, "error");
    btn.innerHTML = origHtml;
    btn.disabled = false;
  }
}

async function searchFilesForLinking(q, dropdown, trackId, searchInput) {
  try {
    const resp = await fetchJSON(
      `/api/files?search=${encodeURIComponent(q)}&isLocal=true&limit=10`,
    );
    const files = resp.data || [];
    if (files.length === 0) {
      dropdown.innerHTML = '<div class="tag-dropdown-empty">No files found</div>';
      dropdown.classList.add("open");
      return;
    }
    dropdown.innerHTML = files
      .map(
        (f) =>
          `<div class="tag-dropdown-item" data-file-id="${f.id}">${escHtml(f.artist || "")} — ${escHtml(f.title || f.filePath?.split("/").pop() || "")}</div>`,
      )
      .join("");
    dropdown.classList.add("open");

    // Wire clicks on dropdown items
    dropdown.querySelectorAll(".tag-dropdown-item").forEach((item) => {
      item.addEventListener("click", async () => {
        const fileId = parseInt(item.dataset.fileId, 10);
        if (!fileId) return;
        await linkFile(fileId, trackId, searchInput, dropdown);
      });
    });
  } catch (err) {
    dropdown.innerHTML = `<div class="tag-dropdown-empty">Error: ${escHtml(err.message)}</div>`;
    dropdown.classList.add("open");
  }
}

async function linkFile(fileId, trackId, searchInput, dropdown) {
  dropdown.classList.remove("open");
  dropdown.innerHTML = "";
  searchInput.value = "";
  searchInput.placeholder = "Linking…";
  try {
    await fetchJSON(`/api/files/${fileId}/track-corrections`, {
      method: "PUT",
      body: JSON.stringify({
        corrections: [{ trackId, linkType: "include" }],
      }),
    });
    showToast("File linked to track", "success");
    await refreshDetail();
  } catch (err) {
    showToast(`Failed: ${err.message}`, "error");
    searchInput.placeholder = "Link a file… (search by name)";
  }
}

async function refreshDetail() {
  if (!currentTrackId) return;
  const container = document.getElementById("main-content");
  if (!container) return;
  try {
    const resp = await fetchJSON(`/api/tracks/${currentTrackId}/detail`);
    const data = resp.data || resp;
    container.innerHTML = renderPage(data);
    wireCorrectionEvents(container, currentTrackId);
  } catch (err) {
    showToast(`Failed to refresh: ${err.message}`, "error");
  }
}

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

function formatBytes(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

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
  if (page !== "track-detail" || !query) return null;
  const params = new URLSearchParams(query);
  return parseInt(params.get("id"), 10) || null;
}
