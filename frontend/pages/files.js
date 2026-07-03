/**
 * files.js — Browse and manage local music files with comment diff previews.
 *
 * Canonical CRUD blueprint page. Uses stable toolbar pattern:
 *   TOOLBAR (rendered once) — filter panel + comment writer sidebar
 *   CONTENT (re-rendered)   — stats row + sortable table + pagination
 *
 * Exports: init(container, signal, hashParams)
 */

import {
  escapeHtml,
  renderLoading,
  renderErrorBlock,
  showToast,
  showModal,
} from "../shared/components.js";
import { renderActionsPanel, updateSelectionCount } from "../shared/actions-panel.js";
import { formatBPM, formatDuration } from "../shared/format.js";
import { fetchJSON } from "../shared/api.js";
import { renderSearchInput, wireSearchFilter } from "../shared/search-filter.js";
import { renderCommentWriter, wireCommentWriter } from "../shared/comment-writer.js";
import {
  getPageSize,
  renderPageSizeSelector,
  sortableTh,
  wireSortableHeaders,
  updateHash,
  parseHash,
} from "../shared/crud.js";
import {
  loadColumnConfig,
  saveColumnConfig,
  renderColumnConfigTrigger,
  renderColumnHeaders,
  renderColumnCells,
  wireColumnResize,
  wireColumnDragReorder,
  wireConfigTrigger,
} from "../shared/column-config.js";

/* ------------------------------------------------------------------ */
/*  Constants                                                          */
/* ------------------------------------------------------------------ */

const BPM_MAX = 300;

/**
 * Musical keys in Camelot notation as stored in DB.
 * Minor: 1m–12m  |  Major: 1d–12d
 */
const MINOR_KEYS = [];
const MAJOR_KEYS = [];

for (let i = 1; i <= 12; i++) {
  MINOR_KEYS.push(`${i}m`);
  MAJOR_KEYS.push(`${i}d`);
}

/* ------------------------------------------------------------------ */
/*  Hash schema for URL state                                          */
/* ------------------------------------------------------------------ */

const HASH_SCHEMA = {
  page: { type: "number", default: 0 },
  search: { type: "string", default: "" },
  sort: { type: "string", default: "" },
  order: { type: "string", default: "asc" },
  bpmMin: { type: "number", default: 0 },
  bpmMax: { type: "number", default: BPM_MAX },
  keys: { type: "array", default: [] },
  selectedTags: { type: "array", default: [] },
  linkedOnly: { type: "boolean", default: false },
  unlinked: { type: "boolean", default: false },
  nonDefaultOnly: { type: "boolean", default: false },
  selectedServices: { type: "array", default: [] },
  pmvCategories: { type: "array", default: [] },
  pmvAggregate: { type: "string", default: "" },
  commentStatuses: { type: "array", default: [] },
  fileTypes: { type: "array", default: [] },
  backedUp: { type: "boolean", default: null },
  safeToDelete: { type: "boolean", default: null },
  isLocal: { type: "boolean", default: null },
  ratingMin: { type: "number", default: 0 },
  playCountMin: { type: "number", default: 0 },
};

/**
 * Default state values used to skip in URL hash.
 */
const HASH_DEFAULTS = {
  sort: "",
  order: "asc",
  search: "",
  bpmMin: 0,
  bpmMax: BPM_MAX,
  keys: [],
  selectedTags: [],
  linkedOnly: false,
  unlinked: false,
  nonDefaultOnly: false,
  selectedServices: [],
  pmvCategories: [],
  pmvAggregate: "",
  commentStatuses: [],
  fileTypes: [],
  backedUp: null,
  safeToDelete: null,
  isLocal: null,
  ratingMin: 0,
  playCountMin: 0,
};

/* ------------------------------------------------------------------ */
/*  Column model                                                        */
/* ------------------------------------------------------------------ */

const FILES_COLUMNS = [
  { id: "title", label: "Title", sortable: true, sortKey: "title", defaultWidth: 180 },
  { id: "artist", label: "Artist", sortable: true, sortKey: "artist", defaultWidth: 80 },
  { id: "bpm", label: "BPM", sortable: true, sortKey: "bpm", defaultWidth: 80 },
  { id: "key", label: "Key", sortable: true, sortKey: "musical_key", defaultWidth: 50 },
  {
    id: "format",
    label: "Format",
    sortable: true,
    sortKey: "file_type",
    defaultWidth: 60,
  },
  { id: "linked", label: "Linked", sortable: false, defaultWidth: 50 },
  { id: "isrc", label: "ISRC", sortable: true, sortKey: "isrc", defaultWidth: 50 },
  { id: "rating", label: "★", sortable: true, sortKey: "rating", defaultWidth: 70 },
  {
    id: "plays",
    label: "Plays",
    sortable: true,
    sortKey: "play_count",
    defaultWidth: 50,
  },
  {
    id: "duration",
    label: "Duration",
    sortable: true,
    sortKey: "duration_ms",
    defaultWidth: 60,
  },
  { id: "album", label: "Album", sortable: false, defaultWidth: 60 },
  {
    id: "created",
    label: "Created",
    sortable: true,
    sortKey: "created_at",
    defaultWidth: 80,
  },
  {
    id: "lastPlayed",
    label: "Last Played",
    sortable: true,
    sortKey: "last_played",
    defaultWidth: 80,
  },
  { id: "backedUp", label: "Backup", sortable: false, defaultWidth: 70 },
  { id: "isLocal", label: "Local", sortable: false, defaultWidth: 60 },
  { id: "safeToDelete", label: "Prune", sortable: false, defaultWidth: 70 },
  { id: "comment", label: "Comment Diff", sortable: false, defaultWidth: 250 },
  { id: "actions", label: "Actions", sortable: false, defaultWidth: 120 },
];

/* ------------------------------------------------------------------ */
/*  Cell renderers (columnId → render function)                         */
/* ------------------------------------------------------------------ */

const FILES_CELL_RENDERERS = {
  title: (f) => escapeHtml(f.title),
  artist: (f) => escapeHtml(f.artist),
  bpm: (f) => `<span class="font-mono">${formatBPM(f.bpm)}</span>`,
  key: (f) => renderKeyBadge(f.key),
  rating: (f) =>
    f.rating != null && f.rating > 0
      ? `<span class="rating-stars">${"★".repeat(Math.min(f.rating, 5))}${"☆".repeat(Math.max(5 - f.rating, 0))}</span>`
      : '<span class="text-muted">—</span>',
  format: (f) =>
    f.fileType
      ? `<span class="badge badge-format">${escapeHtml(f.fileType.toUpperCase())}</span>`
      : '<span class="text-muted">—</span>',
  linked: (f) => renderLinkBadge(f.matchedServices),
  isrc: (f) =>
    f.isrc ? `<code>${escapeHtml(f.isrc)}</code>` : '<span class="text-muted">—</span>',
  plays: (f) => `<span class="font-mono text-sm">${escapeHtml(f.playCount || 0)}</span>`,
  duration: (f) =>
    f.duration > 0
      ? `<span class="font-mono text-sm">${formatDuration(f.duration)}</span>`
      : '<span class="text-muted">—</span>',
  album: (f) => (f.album ? escapeHtml(f.album) : '<span class="text-muted">—</span>'),
  created: (f) =>
    f.createdAt ? formatTimestamp(f.createdAt) : '<span class="text-muted">—</span>',
  lastPlayed: (f) =>
    f.lastPlayed ? formatTimestamp(f.lastPlayed) : '<span class="text-muted">—</span>',
  backedUp: (r) => {
    return r.backedUp
      ? '<span class="status-badge connected" title="Backed up"><i class="fas fa-cloud"></i></span>'
      : '<span class="status-badge disconnected" title="Not backed up"><i class="fas fa-cloud"></i></span>';
  },
  isLocal: (r) => {
    if (r.isLocal)
      return '<span class="status-badge connected" title="On disk"><i class="fas fa-hdd"></i></span>';
    return '<span class="status-badge disconnected" title="Backup only"><i class="fas fa-cloud"></i></span>';
  },
  safeToDelete: (r) => {
    return r.safeToDelete
      ? '<span class="status-badge" style="color:var(--red);background:rgba(239,68,68,0.1)" title="Backed up, stem exists — safe to delete locally"><i class="fas fa-trash-alt"></i></span>'
      : '<span class="text-muted" title="Keep locally">—</span>';
  },
  comment: (f) => renderCommentDiff(f),
  actions: (f) => renderFileActions(f),
};

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */

/**
 * Compare two comment strings and return whether they differ.
 * Returns { diffOld, diffNew, unchanged } with the full plain text strings.
 */
function formatTimestamp(ts) {
  if (!ts) return '<span class="text-muted">—</span>';
  const d = new Date(ts * 1000);
  return `<span class="font-mono text-xs" title="${d.toISOString()}">${d.toLocaleDateString()} ${d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</span>`;
}

function computeDiff(oldComment, targetComment) {
  const oldStr = oldComment || "";
  const targetStr = targetComment || "";

  if (oldStr === targetStr) {
    return { diffOld: oldStr, diffNew: targetStr, unchanged: true };
  }

  return { diffOld: oldStr, diffNew: targetStr, unchanged: false };
}

function adaptFile(f) {
  const diff = computeDiff(f.comment, f.commentTarget);
  return {
    id: f.id,
    title: f.title,
    artist: f.artist,
    bpm: f.bpm,
    key: f.musicalKey,
    isrc: f.isrc,
    diffOld: diff.diffOld,
    diffNew: diff.diffNew,
    commentUnchanged: diff.unchanged,
    needsUpdate: f.commentNeedsUpdate,
    comment: f.comment,
    commentTarget: f.commentTarget,
    rating: f.rating,
    playCount: f.playCount,
    lastPlayed: f.lastPlayed || null,
    matchedServices: f.matchedServices || [],
    album: f.album || null,
    duration: f.durationMs ? Math.round(f.durationMs / 1000) : 0,
    createdAt: f.createdAt || null,
    fileType: f.fileType || null,
    backedUp: !!f.backedUp,
    isLocal: !!f.isLocal,
    hasStem: !!f.hasStem,
    safeToDelete: !!f.safeToDelete,
  };
}

function renderKeyBadge(key) {
  if (!key) return "";
  return `<span class="badge badge-key">${escapeHtml(key)}</span>`;
}

function renderLinkBadge(services) {
  if (!services || services.length === 0) {
    return '<span style="color:var(--text-muted);">—</span>';
  }
  const icons = {
    spotify: '<i class="fab fa-spotify"></i>',
    soundcloud: '<i class="fab fa-soundcloud"></i>',
    youtube: '<i class="fab fa-youtube"></i>',
  };
  return services
    .map((s) => {
      const icon = icons[s] || s;
      return `<span class="service-badge ${escapeHtml(s)}" title="${escapeHtml(s)}">${icon}</span>`;
    })
    .join(" ");
}

function renderCommentDiff(f) {
  // Backup-only files can't have comments written
  if (!f.isLocal) {
    return '<span class="text-muted"><i class="fas fa-cloud"></i> Backup only</span>';
  }
  if (f.needsUpdate) {
    return `<div class="diff-line">
      <div class="diff-line-old"><span class="diff-sign minus">−</span>${escapeHtml(f.diffOld || "(empty)")}</div>
      <div class="diff-line-new"><span class="diff-sign plus">+</span>${escapeHtml(f.diffNew)}</div>
    </div>`;
  }
  return `<div class="diff-line-unchanged"><span class="diff-sign check">✓</span>${f.comment ? escapeHtml(f.comment) : '<span class="text-muted">(empty)</span>'}</div>`;
}

function renderFileActions(f) {
  return `
    <button class="btn btn-sm btn-icon" data-action="view" data-id="${f.id}" title="View details"><i class="fas fa-eye"></i></button>
    <button class="btn btn-sm btn-icon" data-action="similar" data-id="${f.id}" title="Similar tracks by tag"><i class="fas fa-project-diagram"></i></button>
    <button class="btn btn-sm btn-icon" data-action="write-comment" data-id="${f.id}" title="Write comment to file" ${f.commentTarget ? "" : "disabled"}><i class="fas fa-pen"></i></button>
  `;
}

/**
 * Render table rows with comment diff
 */
function renderRows(files) {
  return files
    .map((f) => {
      const diffRow = f.needsUpdate
        ? `<div class="diff-line">
             <div class="diff-line-old"><span class="diff-sign minus">−</span>${escapeHtml(f.diffOld || "(empty)")}</div>
             <div class="diff-line-new"><span class="diff-sign plus">+</span>${escapeHtml(f.diffNew)}</div>
           </div>`
        : `<div class="diff-line-unchanged"><span class="diff-sign check">✓</span>${f.comment ? escapeHtml(f.comment) : '<span class="text-muted">(empty)</span>'}</div>`;
      return `<tr>
        <td><a href="#file-detail?id=${f.id}" class="track-title-link">${escapeHtml(f.title)}</a></td>
        <td>${escapeHtml(f.artist)}</td>
        <td>${f.bpm ? formatBPM(f.bpm) : ""}</td>
        <td>${renderKeyBadge(f.key)}</td>
        <td>${renderLinkBadge(f.matchedServices)}</td>
        <td>${f.isrc ? escapeHtml(f.isrc) : ""}</td>
        <td>${f.playCount ?? 0}</td>
        <td>${f.duration ? `<span class="font-mono text-sm">${escapeHtml(f.duration)}</span>` : '<span class="text-muted">—</span>'}</td>
        <td>${f.album ? escapeHtml(f.album) : '<span class="text-muted">—</span>'}</td>
        <td>${f.createdAt ? formatTimestamp(f.createdAt) : '<span class="text-muted">—</span>'}</td>
        <td>${f.lastPlayed ? formatTimestamp(f.lastPlayed) : '<span class="text-muted">—</span>'}</td>
        <td><div class="${diffClass}">${diffRow}</div></td>
        <td>
          <button class="btn btn-sm btn-icon" data-action="view" data-id="${f.id}" title="View details"><i class="fas fa-eye"></i></button>
          <button class="btn btn-sm btn-icon" data-action="similar" data-id="${f.id}" title="Similar tracks by tag"><i class="fas fa-project-diagram"></i></button>
          <button class="btn btn-sm btn-icon" data-action="write-comment" data-id="${f.id}" title="Write comment to file" ${f.commentTarget ? "" : "disabled"}><i class="fas fa-pen"></i></button>
        </td>
      </tr>`;
    })
    .join("");
}

/* ------------------------------------------------------------------ */
/*  Action handlers                                                    */
/* ------------------------------------------------------------------ */

async function writeComment(id) {
  try {
    const resp = await fetchJSON(`/api/files/${id}/write-comment`, {
      method: "POST",
    });
    const taskId = resp.data?.taskId || resp.data;
    showToast(`Comment write queued (task #${taskId})`, "success");
  } catch (err) {
    showToast(`Failed to queue comment write: ${err.message}`, "error");
  }
}

async function viewFile(id) {
  window.location.hash = `#file-detail?id=${id}`;
}

async function showSimilarTracks(id) {
  try {
    const resp = await fetchJSON(`/api/files/${id}/similar-tracks?limit=15`);
    const results = resp.data || [];

    let bodyHtml;
    if (results.length === 0) {
      bodyHtml = `<div style="padding:32px;text-align:center;color:var(--text-muted);">
        <i class="fas fa-project-diagram" style="font-size:2rem;margin-bottom:12px;"></i>
        <p>No similar tracks found.</p>
        <p style="font-size:0.85rem;">Ensure tag embeddings and similarities have been computed.</p>
      </div>`;
    } else {
      const rows = results
        .map((r) => {
          const [fid, title, artist, bpm, key, score, matchedTagsJson] = r;
          let matchedTags;
          try {
            matchedTags = JSON.parse(matchedTagsJson);
          } catch {
            matchedTags = [];
          }
          const pct = Math.round(score * 100);
          const pctClass = pct >= 60 ? "green" : pct >= 40 ? "yellow" : "text-muted";
          const tagsHtml = matchedTags
            .map(
              ([seedTag, matchTag, sim]) =>
                `<span class="badge" style="background:var(--surface-hover);border:1px solid var(--border);font-size:0.7rem;padding:1px 6px;border-radius:4px;white-space:nowrap;" title="${escapeHtml(seedTag)} → ${escapeHtml(matchTag)} (${(sim * 100).toFixed(0)}%)">
            ${escapeHtml(matchTag)}
          </span>`,
            )
            .join(" ");
          return `<tr>
          <td style="padding:8px 12px;border-bottom:1px solid var(--border);">
            <div style="font-weight:500;">${escapeHtml(title)}</div>
            <div style="font-size:0.8rem;color:var(--text-muted);">${artist ? escapeHtml(artist) : "—"}</div>
          </td>
          <td style="padding:8px 12px;border-bottom:1px solid var(--border);font-family:var(--font-mono);">${bpm ? formatBPM(bpm) : "—"}</td>
          <td style="padding:8px 12px;border-bottom:1px solid var(--border);">${key ? renderKeyBadge(key) : "—"}</td>
          <td style="padding:8px 12px;border-bottom:1px solid var(--border);">
            <div style="display:flex;gap:2px;flex-wrap:wrap;">${tagsHtml}</div>
          </td>
          <td style="padding:8px 12px;border-bottom:1px solid var(--border);font-family:var(--font-mono);font-weight:600;color:var(--${pctClass});">${pct}%</td>
        </tr>`;
        })
        .join("");
      bodyHtml = `<table style="width:100%;border-collapse:collapse;">
        <thead><tr style="font-size:0.75rem;color:var(--text-muted);text-transform:uppercase;letter-spacing:0.04em;">
          <th style="padding:8px 12px;text-align:left;border-bottom:1px solid var(--border-strong);">Track</th>
          <th style="padding:8px 12px;text-align:left;border-bottom:1px solid var(--border-strong);">BPM</th>
          <th style="padding:8px 12px;text-align:left;border-bottom:1px solid var(--border-strong);">Key</th>
          <th style="padding:8px 12px;text-align:left;border-bottom:1px solid var(--border-strong);">Similar Tags</th>
          <th style="padding:8px 12px;text-align:right;border-bottom:1px solid var(--border-strong);">Match</th>
        </tr></thead>
        <tbody>${rows}</tbody>
      </table>`;
    }

    showModal({
      title: '<i class="fas fa-project-diagram"></i> Similar Tracks by Tag',
      bodyHtml: `<div style="padding:16px;overflow-y:auto;">${bodyHtml}</div>`,
      width: "900px",
    });
  } catch (err) {
    showToast(`Failed to load similar tracks: ${err.message}`, "error");
  }
}

/* ------------------------------------------------------------------ */
/*  Render: Toolbar (stable, rendered once)                            */
/* ------------------------------------------------------------------ */

function renderToolbar(state) {
  const chipsHtml = (state.selectedTags || [])
    .map(
      (t) =>
        `<span class="tag-chip" data-tag="${t}">${escapeHtml(t)} <i class="fas fa-times tag-chip-x"></i></span>`,
    )
    .join("");

  const selectedKeys = new Set(state.keys || []);
  const keyBtn = (key, cls) =>
    `<button class="key-btn ${cls}${selectedKeys.has(key) ? " active" : ""}" data-key="${key}">${key}</button>`;

  const minorRow = MINOR_KEYS.map((k) => keyBtn(k, "minor")).join("");
  const majorRow = MAJOR_KEYS.map((k) => keyBtn(k, "major")).join("");

  const bpmMin = parseFloat(state.bpmMin) || 0;
  const bpmMax = parseFloat(state.bpmMax) || BPM_MAX;
  const pctMin = (bpmMin / BPM_MAX) * 100;
  const pctMax = (bpmMax / BPM_MAX) * 100;

  const actionBtn = (label, action, cls = "") =>
    `<button class="key-btn action ${cls}" data-key-action="${action}">${label}</button>`;

  const toolbarHtml = `
    <div class="filter-panel" id="files-filter-panel">
      <div class="filter-panel-header">
        ${renderSearchInput("files", state.search)}
        <button class="filter-panel-toggle" id="files-filter-toggle" title="Toggle filter panel">
          <i class="fas fa-chevron-up chevron"></i>
        </button>
      </div>
      <div class="filter-panel-body">
        <div class="filter-panel-scroll" style="display:grid;grid-template-columns:1fr 1fr;gap:var(--space-2) var(--space-4);">
          <div>
            <div class="filter-section-header" style="margin-top:0"><i class="fas fa-music"></i> File Info</div>
            <div class="filter-row">
              <span class="filter-row-label toggleable" data-filter="bpm">BPM</span>
              <div class="dual-range-wrap">
                <div class="dual-range">
                  <div class="dual-range-track">
                    <div class="dual-range-fill" style="left:${pctMin}%;width:${pctMax - pctMin}%"></div>
                  </div>
                  <input type="range" class="dual-range-input" data-sf-filter="bpmMin"
                         min="0" max="${BPM_MAX}" step="1" value="${bpmMin}">
                  <input type="range" class="dual-range-input" data-sf-filter="bpmMax"
                         min="0" max="${BPM_MAX}" step="1" value="${bpmMax}">
                </div>
                <div class="dual-range-values">
                  <span class="dual-range-min-val">${bpmMin}</span>
                  <span class="sep">——</span>
                  <span class="dual-range-max-val">${bpmMax}</span>
                </div>
              </div>
            </div>
            <div class="filter-row">
              <span class="filter-row-label toggleable" data-filter="key">Key</span>
              <div class="key-grid-wrap">
                <div class="key-grid" data-key-row="minor">${minorRow}
                  ${actionBtn("ALL m", "minor-all")}
                  ${actionBtn("NONE m", "minor-none")}
                </div>
                <div class="key-grid" data-key-row="major">${majorRow}
                  ${actionBtn("ALL d", "major-all")}
                  ${actionBtn("NONE d", "major-none")}
                </div>
              </div>
            </div>
            <div class="filter-row">
              <span class="filter-row-label toggleable" data-filter="rating">Rating</span>
              <input type="number" class="input-text" data-filter-input="ratingMin" min="0" max="5" placeholder="Min ★" style="width:80px">
            </div>
            <div class="filter-row">
              <span class="filter-row-label toggleable" data-filter="plays">Plays</span>
              <input type="number" class="input-text" data-filter-input="playCountMin" min="0" placeholder="Min plays" style="width:80px">
            </div>
            <div class="filter-row">
              <span class="filter-row-label toggleable" data-filter="tag">Tags</span>
              <div class="typeahead-wrap" style="flex:1">
                <div class="tag-search-wrap">
                  <i class="fas fa-tag"></i>
                  <input type="text" class="input-text input-search" id="files-tag-search"
                         placeholder="filter by TAG" autocomplete="off">
                  <div class="tag-dropdown" id="files-tag-dropdown"></div>
                </div>
              </div>
              <div class="tag-chips" id="files-tag-chips">${chipsHtml}</div>
            </div>
          </div>
          <div>
            <div class="filter-section-header" style="margin-top:0"><i class="fas fa-tag"></i> Classification</div>
            <div class="filter-row">
              <span class="filter-row-label toggleable" data-filter="service">Service</span>
              <div class="filter-group service-filter-group">
                <button class="filter-btn${(state.selectedServices || []).includes("spotify") ? " active" : ""}" data-value="spotify" title="Spotify"><i class="fab fa-spotify"></i></button>
                <button class="filter-btn${(state.selectedServices || []).includes("soundcloud") ? " active" : ""}" data-value="soundcloud" title="SoundCloud"><i class="fab fa-soundcloud"></i></button>
                <button class="filter-btn${(state.selectedServices || []).includes("youtube") ? " active" : ""}" data-value="youtube" title="YouTube"><i class="fab fa-youtube"></i></button>
              </div>
            </div>
            <div class="filter-row">
              <span class="filter-row-label toggleable" data-filter="pmv">PMV</span>
              <div class="filter-group" id="pmv-cat-btns" style="flex-wrap:wrap">
                <button class="filter-btn${(state.pmvCategories || []).includes("p") ? " active" : ""}" data-value="p" title="Has Phase tags">P</button>
                <button class="filter-btn${(state.pmvCategories || []).includes("m") ? " active" : ""}" data-value="m" title="Has Mood tags">M</button>
                <button class="filter-btn${(state.pmvCategories || []).includes("v") ? " active" : ""}" data-value="v" title="Has Vibe tags">V</button>
              </div>
              <span class="pmv-sep">|</span>
              <div class="filter-group" id="pmv-agg-btns" style="flex-wrap:wrap">
                <button class="filter-btn${state.pmvAggregate === "full" ? " active" : ""}" data-value="full" title="Has all three categories">Full</button>
                <button class="filter-btn${state.pmvAggregate === "partial" ? " active" : ""}" data-value="partial" title="Has at least one category">Partial</button>
                <button class="filter-btn${state.pmvAggregate === "none" ? " active" : ""}" data-value="none" title="Has no PMV categories">None</button>
              </div>
            </div>
            <div class="filter-row">
              <span class="filter-row-label toggleable" data-filter="comment">Comment</span>
              <div class="filter-group" id="files-comment-btns">
                <button class="filter-btn${(state.commentStatuses || []).includes("needs_update") ? " active" : ""}" data-value="needs_update">Needs Update</button>
                <button class="filter-btn${(state.commentStatuses || []).includes("uptodate") ? " active" : ""}" data-value="uptodate">Up to Date</button>
              </div>
            </div>
            <div class="filter-row">
              <span class="filter-row-label toggleable" data-filter="fileType">Type</span>
              <div class="filter-group" id="files-filetype-filter">
                <button class="filter-btn${(state.fileTypes || []).includes("mp3") ? " active" : ""}" data-value="mp3">MP3</button>
                <button class="filter-btn${(state.fileTypes || []).includes("flac") ? " active" : ""}" data-value="flac">FLAC</button>
                <button class="filter-btn${(state.fileTypes || []).includes("stem.m4a") ? " active" : ""}" data-value="stem.m4a">Stem</button>
                <button class="filter-btn${(state.fileTypes || []).includes("wav") ? " active" : ""}" data-value="wav">WAV</button>
              </div>
            </div>
            <div class="filter-row" data-filter="backup">
              <span class="filter-row-label toggleable" data-filter="backup">Backup</span>
              <div class="filter-group">
                <button class="filter-btn${!state.backedUp ? " active" : ""}" data-backup-filter="all">All</button>
                <button class="filter-btn${state.backedUp === true ? " active" : ""}" data-backup-filter="yes"><i class="fas fa-cloud"></i> Yes</button>
                <button class="filter-btn${state.backedUp === false ? " active" : ""}" data-backup-filter="no"><i class="fas fa-cloud"></i> No</button>
              </div>
            </div>
            <div class="filter-row" data-filter="local">
              <span class="filter-row-label toggleable" data-filter="local">On Disk</span>
              <div class="filter-group">
                <button class="filter-btn${!state.isLocal ? " active" : ""}" data-local-filter="all">All</button>
                <button class="filter-btn${state.isLocal === true ? " active" : ""}" data-local-filter="yes"><i class="fas fa-hdd"></i> Yes</button>
                <button class="filter-btn${state.isLocal === false ? " active" : ""}" data-local-filter="no"><i class="fas fa-cloud"></i> No</button>
              </div>
            </div>
            <div class="filter-row" data-filter="safe">
              <span class="filter-row-label toggleable" data-filter="safe">Safe to Delete</span>
              <div class="filter-group">
                <button class="filter-btn${!state.safeToDelete ? " active" : ""}" data-safe-filter="all">All</button>
                <button class="filter-btn${state.safeToDelete === true ? " active" : ""}" data-safe-filter="yes"><i class="fas fa-trash-alt"></i> Yes</button>
              </div>
            </div>
          </div>
        </div>
        <div class="filter-row" style="margin-top:var(--space-2)">
          <div class="filter-link-status toggle-group">
            <button class="toggle-btn ${state.linkedOnly ? "active" : ""}" id="files-filter-linked" data-link-filter="linked">
              <i class="fas fa-link"></i> Linked
            </button>
            <button class="toggle-btn ${state.unlinked ? "active" : ""}" id="files-filter-unlinked" data-link-filter="unlinked">
              <i class="fas fa-unlink"></i> Unlinked
            </button>
          </div>
        </div>
        <div class="filter-row">
          <div class="filter-link-status">
            <button class="filter-action-btn ${state.nonDefaultOnly ? "active" : ""}" id="files-filter-non-default" data-link-filter="nonDefaultOnly">
              <i class="fas fa-tag"></i> ignore files with only default tags
            </button>
          </div>
        </div>
    </div>
</div>`;

  const commentWriterHtml = `
    <div class="filter-panel" style="flex:1;min-width:260px;max-width:320px;">
      <div class="filter-panel-header">
        <span style="font-weight:600;font-size:0.75rem;color:var(--text-muted);text-transform:uppercase;letter-spacing:0.04em;">Write Comments</span>
      </div>
      <div class="filter-panel-body" style="padding:var(--space-3) var(--space-4);">
        ${renderCommentWriter({
          linkedOnly: state.linkedOnly || false,
          tagNames: state.selectedTags || [],
          nonDefaultOnly: state.nonDefaultOnly || false,
        })}
      </div>
    </div>`;

  return toolbarHtml;
}

/* ------------------------------------------------------------------ */
/*  Render: Body (re-rendered on each fetch)                           */
/* ------------------------------------------------------------------ */

function renderEmptyBody() {
  const config = loadColumnConfig("files", FILES_COLUMNS);
  const checkboxHeader = '<th class="col-checkbox"></th>';
  const dataHeaders = renderColumnHeaders(
    config,
    FILES_COLUMNS,
    { sort: "", order: "" },
    sortableTh,
  );
  const visibleCount = config.filter((c) => c.visible).length;
  return `
    <div class="stats-row">
      <div class="stats-group">
        <button class="btn btn-sm btn-icon" id="files-refresh" title="Refresh"><i class="fa-solid fa-rotate"></i></button>
        <strong>0</strong> files
        ${renderPageSizeSelector(getPageSize())}
        ${renderColumnConfigTrigger()}
      </div>
    </div>
    <div class="table-wrap"><table class="data-table">
      <thead><tr>${checkboxHeader}${dataHeaders}</tr></thead>
      <tbody><tr><td colspan="${visibleCount + 1}"><div class="text-center text-muted" style="padding:32px;text-align:center;">No files found. Scan a folder to get started.</div></td></tr></tbody>
    </table></div>`;
}

function renderBody(data, state) {
  const totalPages = Math.ceil(data._total / state.pageSize) || 1;
  const currentPage = state.page + 1;
  const config = loadColumnConfig("files", FILES_COLUMNS);
  const dataHeaders = renderColumnHeaders(config, FILES_COLUMNS, state, sortableTh);

  // Checkbox column (select-all header, per-row checkboxes)
  const selectedSet = state.selectedFileIds || new Set();
  const inAllMode = state.selectAllMode;
  const allOnPageSelected =
    inAllMode ||
    (data.files.length > 0 && data.files.every((f) => selectedSet.has(f.id)));
  const checkboxChecked = inAllMode || allOnPageSelected;
  const checkboxHeader =
    '<th class="col-checkbox"><input type="checkbox" class="files-select-all" id="files-select-all"' +
    (checkboxChecked ? " checked" : "") +
    "></th>";
  const headers = checkboxHeader + dataHeaders;

  const rowsHtml = data.files
    .map((f) => {
      const checked = inAllMode || selectedSet.has(f.id);
      const cb =
        '<td class="col-checkbox"><input type="checkbox" class="files-row-checkbox" data-file-id="' +
        f.id +
        '"' +
        (checked ? " checked" : "") +
        "></td>";
      return `<tr>${cb}${renderColumnCells(config, FILES_COLUMNS, FILES_CELL_RENDERERS, f)}</tr>`;
    })
    .join("");

  return `
    <div class="stats-row">
      <div class="stats-group">
        <button class="btn btn-sm btn-icon" id="files-refresh" title="Refresh"><i class="fa-solid fa-rotate"></i></button>
        <strong>${data._total}</strong> files
        ${renderPageSizeSelector(state.pageSize)}
        ${renderColumnConfigTrigger()}
        ${
          state.layoutMode
            ? '<button class="btn btn-sm btn-primary" id="files-layout-btn" style="margin-left:8px"><i class="fas fa-check"></i> Done</button>'
            : '<button class="btn btn-sm" id="files-layout-btn" style="margin-left:8px"><i class="fas fa-arrows-alt"></i> Modify Column Layout</button>'
        }
      </div>
    </div>
    <div id="files-select-all-banner" class="select-all-banner" style="display:none"></div>
    <div class="table-wrap"><table class="data-table">
      <thead><tr>${headers}</tr></thead>
      <tbody>${rowsHtml}</tbody>
    </table></div>
    <div class="pagination">
      <button class="pagination-btn" id="files-page-prev" ${state.page === 0 ? "disabled" : ""}><i class="fas fa-chevron-left"></i></button>
      <span class="pagination-info">Page ${currentPage} of ${totalPages}</span>
      <button class="pagination-btn" id="files-page-next" ${state.page >= totalPages - 1 ? "disabled" : ""}><i class="fas fa-chevron-right"></i></button>
    </div>`;
}

/* ------------------------------------------------------------------ */
/*  Build API params                                                   */
/* ------------------------------------------------------------------ */

function buildParams(state) {
  const params = new URLSearchParams();
  params.set("limit", String(state.pageSize));
  params.set("offset", String(state.page * state.pageSize));
  if (state.search) params.set("search", state.search);
  if (state.bpmMin > 0) params.set("bpmMin", state.bpmMin);
  if (state.bpmMax < BPM_MAX) params.set("bpmMax", state.bpmMax);
  if (state.keys && state.keys.length > 0) {
    params.set("key", state.keys.join(","));
  }
  if (state.selectedTags && state.selectedTags.length > 0) {
    params.set("tags", state.selectedTags.join(","));
  }
  if (state.linkedOnly) params.set("linkedOnly", "true");
  if (state.unlinked) params.set("unlinked", "true");
  if (state.nonDefaultOnly) params.set("nonDefaultOnly", "true");
  // Server-side filters
  if (state.selectedServices && state.selectedServices.length > 0) {
    params.set("selectedServices", state.selectedServices.join(","));
  }
  if (state.ratingMin > 0) params.set("ratingMin", state.ratingMin);
  if (state.playCountMin > 0) params.set("playCountMin", state.playCountMin);
  if (state.pmvCategories && state.pmvCategories.length > 0) {
    params.set("pmvCategories", state.pmvCategories.join(","));
  }
  if (state.pmvAggregate) {
    params.set("pmvAggregate", state.pmvAggregate);
  }
  if (state.fileTypes && state.fileTypes.length > 0) {
    params.set("fileTypes", state.fileTypes.join(","));
  }
  if (state.commentStatuses && state.commentStatuses.length > 0) {
    params.set("commentStatuses", state.commentStatuses.join(","));
  }
  if (state.backedUp !== null) params.set("backedUp", String(state.backedUp));
  if (state.safeToDelete !== null) params.set("safeToDelete", String(state.safeToDelete));
  if (state.isLocal !== null) params.set("isLocal", String(state.isLocal));
  if (state.sort) params.set("sort", state.sort);
  if (state.order === "desc") params.set("order", "desc");
  return params;
}

/**
 * Build a plain filter-params object (no pagination/sort) for the
 * "select all" endpoints that accept JSON filter bodies.
 */
function buildFilterParams(state) {
  const f = {};
  if (state.search) f.search = state.search;
  if (state.bpmMin > 0) f.bpmMin = state.bpmMin;
  if (state.bpmMax < BPM_MAX) f.bpmMax = state.bpmMax;
  if (state.keys && state.keys.length > 0) f.key = state.keys.join(",");
  if (state.selectedTags && state.selectedTags.length > 0)
    f.tags = state.selectedTags.join(",");
  if (state.linkedOnly) f.linkedOnly = true;
  if (state.unlinked) f.unlinked = true;
  if (state.nonDefaultOnly) f.nonDefaultOnly = true;
  if (state.selectedServices && state.selectedServices.length > 0)
    f.selectedServices = state.selectedServices.join(",");
  if (state.pmvCategories && state.pmvCategories.length > 0)
    f.pmvCategories = state.pmvCategories.join(",");
  if (state.pmvAggregate) f.pmvAggregate = state.pmvAggregate;
  if (state.fileTypes && state.fileTypes.length > 0)
    f.fileTypes = state.fileTypes.join(",");
  if (state.commentStatuses && state.commentStatuses.length > 0)
    f.commentStatuses = state.commentStatuses.join(",");
  if (state.ratingMin > 0) f.ratingMin = state.ratingMin;
  if (state.playCountMin > 0) f.playCountMin = state.playCountMin;
  if (state.backedUp !== null) f.backedUp = state.backedUp;
  if (state.safeToDelete !== null) f.safeToDelete = state.safeToDelete;
  if (state.isLocal !== null) f.isLocal = state.isLocal;
  return f;
}

/* ------------------------------------------------------------------ */
/*  Fetch + Render cycle                                               */
/* ------------------------------------------------------------------ */

async function fetchAndRender(container, signal, state) {
  const contentEl = container.querySelector("#files-content");
  if (!contentEl) return;
  contentEl.innerHTML = renderLoading("Loading files…");

  try {
    const params = buildParams(state);

    const [filesResp, countResp] = await Promise.all([
      fetchJSON(`/api/files?${params}`, { signal }),
      fetchJSON(`/api/files/count?${params}`, { signal }),
    ]);
    if (signal && signal.aborted) return;

    const files = (filesResp.data || []).map(adaptFile);
    const total = countResp.data;
    state._lastTotal = total;

    // Keep selectAllTotal in sync with the actual total when in "all" mode
    if (state.selectAllMode) {
      state.selectAllTotal = total;
    }

    if (files.length === 0 && total === 0) {
      contentEl.innerHTML = renderEmptyBody();
    } else {
      contentEl.innerHTML = renderBody({ _total: total, files }, state);
    }

    wireContentEvents(container, signal, state);
    updateSelectionUI(container, state);
    injectSelectAllBanner(container, state);
  } catch (err) {
    if (err.name === "AbortError") return;
    contentEl.innerHTML = renderErrorBlock({
      title: "Failed to load files",
      detail: err.message,
      retryFn: "window.location.hash='#files'",
    });
  }
}

/* ------------------------------------------------------------------ */
/*  Toolbar event wiring (called once on init)                         */
/* ------------------------------------------------------------------ */

function wireToolbarEvents(container, signal, state) {
  const filterPanel = container.querySelector("#files-filter-panel");

  // ── Unified search + filter wiring (debounced) ──
  if (filterPanel) {
    wireSearchFilter(filterPanel, state, () => {
      updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
      fetchAndRender(container, signal, state);
    });
  }

  // ── Filter panel toggle (collapsible) ──
  const panelToggle = container.querySelector("#files-filter-toggle");
  const panel = container.querySelector("#files-filter-panel");
  if (panelToggle && panel) {
    // Restore saved collapse state
    if (localStorage.getItem("filterPanelCollapsed_files") === "true") {
      panel.classList.add("collapsed");
    }
    panelToggle.addEventListener(
      "click",
      () => {
        panel.classList.toggle("collapsed");
        localStorage.setItem(
          "filterPanelCollapsed_files",
          panel.classList.contains("collapsed"),
        );
        const icon = panelToggle.querySelector(".chevron");
        if (icon) {
          icon.classList.toggle("fa-chevron-up");
          icon.classList.toggle("fa-chevron-down");
        }
      },
      { signal },
    );
  }

  // ── Dual range slider visual updates (fill bar + value labels) ──
  const dualRange = container.querySelector(".dual-range");
  if (dualRange) {
    const minInput = dualRange.querySelector('[data-sf-filter="bpmMin"]');
    const maxInput = dualRange.querySelector('[data-sf-filter="bpmMax"]');
    const fill = dualRange.querySelector(".dual-range-fill");
    const minVal = container.querySelector(".dual-range-min-val");
    const maxVal = container.querySelector(".dual-range-max-val");

    function updateDualRange() {
      let min = parseFloat(minInput.value) || 0;
      let max = parseFloat(maxInput.value) || BPM_MAX;
      if (min > max) {
        [min, max] = [max, min];
        minInput.value = min;
        maxInput.value = max;
      }
      const pctMin = (min / BPM_MAX) * 100;
      const pctMax = (max / BPM_MAX) * 100;
      if (fill) {
        fill.style.left = `${pctMin}%`;
        fill.style.width = `${pctMax - pctMin}%`;
      }
      if (minVal) minVal.textContent = min;
      if (maxVal) maxVal.textContent = max;
    }

    minInput.addEventListener("input", updateDualRange);
    maxInput.addEventListener("input", updateDualRange);
  }

  // ── Key buttons (toggle multiple) + ALL/NONE actions ──
  const keyGridWrap = container.querySelector(".key-grid-wrap");
  if (keyGridWrap) {
    keyGridWrap.addEventListener(
      "click",
      (e) => {
        const btn = e.target.closest(".key-btn");
        if (!btn) return;

        const action = btn.dataset.keyAction;
        if (action) {
          switch (action) {
            case "minor-all":
              state.keys = [...state.keys.filter((k) => !k.endsWith("m")), ...MINOR_KEYS];
              break;
            case "minor-none":
              state.keys = state.keys.filter((k) => !k.endsWith("m"));
              break;
            case "major-all":
              state.keys = [...state.keys.filter((k) => !k.endsWith("d")), ...MAJOR_KEYS];
              break;
            case "major-none":
              state.keys = state.keys.filter((k) => !k.endsWith("d"));
              break;
          }
          state.page = 0;
          updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
          fetchAndRender(container, signal, state);
          // Re-sync all 24 key button active states
          container.querySelectorAll(".key-btn[data-key]").forEach((kb) => {
            kb.classList.toggle("active", state.keys.includes(kb.dataset.key));
          });
          return;
        }

        // Regular key toggle
        const dbVal = btn.dataset.key;
        if (!dbVal) return;
        const idx = state.keys.indexOf(dbVal);
        if (idx >= 0) {
          state.keys.splice(idx, 1);
        } else {
          state.keys.push(dbVal);
        }
        btn.classList.toggle("active");
        state.page = 0;
        updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  }

  // ── Filter input fields (rating, plays, etc.) ──
  filterPanel?.querySelectorAll("[data-filter-input]").forEach((input) => {
    input.addEventListener(
      "input",
      () => {
        const key = input.dataset.filterInput;
        const val = input.value.trim();
        if (key === "ratingMin" || key === "playCountMin") {
          state[key] = val ? parseInt(val, 10) : 0;
        } else {
          state[key] = val;
        }
        state.page = 0;
        updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  });

  // ── Tag search input with keyboard navigation ──
  const tagSearch = container.querySelector("#files-tag-search");
  const tagDropdown = container.querySelector("#files-tag-dropdown");
  if (tagSearch && tagDropdown) {
    let timer;
    let selectedIndex = -1;

    function updateSelection() {
      const items = tagDropdown.querySelectorAll(".tag-dropdown-item");
      items.forEach((item, i) => {
        item.classList.toggle("selected", i === selectedIndex);
      });
      const selected = items[selectedIndex];
      if (selected) {
        selected.scrollIntoView({ block: "nearest" });
      }
    }

    function addSelectedTag() {
      const items = tagDropdown.querySelectorAll(".tag-dropdown-item");
      const selected = items[selectedIndex];
      if (!selected) return;
      const tag = selected.dataset.tag;
      if (!tag) return;
      if (!state.selectedTags.includes(tag)) {
        state.selectedTags.push(tag);
        state.page = 0;
      }
      tagSearch.value = "";
      tagDropdown.classList.remove("open");
      tagDropdown.innerHTML = "";
      tagDropdown.style.position = "";
      tagDropdown.style.top = "";
      tagDropdown.style.left = "";
      tagDropdown.style.width = "";
      tagDropdown.style.zIndex = "";
      selectedIndex = -1;
      renderTagChips();
      updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
      fetchAndRender(container, signal, state);
    }

    tagSearch.addEventListener(
      "input",
      () => {
        clearTimeout(timer);
        selectedIndex = -1;
        const q = tagSearch.value.trim();
        if (!q) {
          tagDropdown.classList.remove("open");
          tagDropdown.innerHTML = "";
          tagDropdown.style.position = "";
          tagDropdown.style.top = "";
          tagDropdown.style.left = "";
          tagDropdown.style.width = "";
          tagDropdown.style.zIndex = "";
          return;
        }
        timer = setTimeout(() => {
          const qLower = q.toLowerCase();
          const filtered = (state.allTags || [])
            .filter((t) => t.name.toLowerCase().includes(qLower))
            .slice(0, 50);
          if (filtered.length === 0) {
            tagDropdown.innerHTML = `<div class="tag-dropdown-empty">No tags found</div>`;
            selectedIndex = -1;
          } else {
            tagDropdown.innerHTML = filtered
              .map(
                (t, i) =>
                  `<div class="tag-dropdown-item${i === 0 ? " selected" : ""}" data-tag="${t.name}">
                    <span class="tag-dropdown-name">${t.name}</span>
                    ${t.category ? `<span class="tag-dropdown-cat">${t.category}</span>` : ""}
                  </div>`,
              )
              .join("");
            selectedIndex = 0;
          }
          // Position dropdown relative to viewport to escape overflow clipping
          const rect = tagSearch.getBoundingClientRect();
          tagDropdown.style.position = "fixed";
          tagDropdown.style.top = rect.bottom + 2 + "px";
          tagDropdown.style.left = rect.left + "px";
          tagDropdown.style.width = rect.width + "px";
          tagDropdown.style.zIndex = "200";
          tagDropdown.classList.add("open");
        }, 50);
      },
      { signal },
    );

    tagDropdown.addEventListener(
      "click",
      (e) => {
        const item = e.target.closest(".tag-dropdown-item");
        if (!item) return;
        const tag = item.dataset.tag;
        if (!tag) return;
        if (!state.selectedTags.includes(tag)) {
          state.selectedTags.push(tag);
          state.page = 0;
        }
        tagSearch.value = "";
        tagDropdown.classList.remove("open");
        tagDropdown.innerHTML = "";
        tagDropdown.style.position = "";
        tagDropdown.style.top = "";
        tagDropdown.style.left = "";
        tagDropdown.style.width = "";
        tagDropdown.style.zIndex = "";
        selectedIndex = -1;
        renderTagChips();
        updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
        fetchAndRender(container, signal, state);
      },
      { signal },
    );

    tagSearch.addEventListener(
      "keydown",
      (e) => {
        if (!tagDropdown.classList.contains("open")) return;
        const items = tagDropdown.querySelectorAll(".tag-dropdown-item");
        switch (e.key) {
          case "ArrowDown":
            e.preventDefault();
            if (items.length === 0) return;
            selectedIndex = Math.min(selectedIndex + 1, items.length - 1);
            updateSelection();
            break;
          case "ArrowUp":
            e.preventDefault();
            if (items.length === 0) return;
            selectedIndex = Math.max(selectedIndex - 1, 0);
            updateSelection();
            break;
          case "Enter":
            e.preventDefault();
            addSelectedTag();
            break;
          case "Escape":
            tagDropdown.classList.remove("open");
            tagDropdown.innerHTML = "";
            tagDropdown.style.position = "";
            tagDropdown.style.top = "";
            tagDropdown.style.left = "";
            tagDropdown.style.width = "";
            tagDropdown.style.zIndex = "";
            selectedIndex = -1;
            tagSearch.blur();
            break;
        }
      },
      { signal },
    );

    // Close dropdown on outside click
    document.addEventListener(
      "click",
      (e) => {
        const wrap = container.querySelector(".tag-search-wrap");
        if (!wrap || wrap.contains(e.target)) return;
        if (tagDropdown) {
          tagDropdown.classList.remove("open");
          tagDropdown.innerHTML = "";
          tagDropdown.style.position = "";
          tagDropdown.style.top = "";
          tagDropdown.style.left = "";
          tagDropdown.style.width = "";
          tagDropdown.style.zIndex = "";
          selectedIndex = -1;
        }
      },
      { signal },
    );

    // Reposition dropdown on scroll/resize while open
    function repositionDropdown() {
      if (!tagDropdown.classList.contains("open")) return;
      const rect = tagSearch.getBoundingClientRect();
      tagDropdown.style.top = rect.bottom + 2 + "px";
      tagDropdown.style.left = rect.left + "px";
      tagDropdown.style.width = rect.width + "px";
    }
    window.addEventListener("scroll", repositionDropdown, { signal, passive: true });
    window.addEventListener("resize", repositionDropdown, { signal, passive: true });
  }

  // ── Tag chip rendering helper ──
  function renderTagChips() {
    const chipsContainer = container.querySelector("#files-tag-chips");
    if (!chipsContainer) return;
    chipsContainer.innerHTML = state.selectedTags
      .map(
        (t) =>
          `<span class="tag-chip" data-tag="${t}">${escapeHtml(t)} <i class="fas fa-times tag-chip-x"></i></span>`,
      )
      .join("");
  }

  // ── Tag chip removal (delegated) ──
  const chipsContainer = container.querySelector("#files-tag-chips");
  if (chipsContainer) {
    chipsContainer.addEventListener(
      "click",
      (e) => {
        const x = e.target.closest(".tag-chip-x");
        if (!x) return;
        const chip = x.closest(".tag-chip");
        if (!chip) return;
        const tag = chip.dataset.tag;
        state.selectedTags = state.selectedTags.filter((t) => t !== tag);
        state.page = 0;
        updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  }

  // ── Linked/Unlinked toggle (mutually exclusive) ──
  const linkedBtn = container.querySelector("#files-filter-linked");
  const unlinkedBtn = container.querySelector("#files-filter-unlinked");
  if (linkedBtn) {
    linkedBtn.addEventListener(
      "click",
      () => {
        if (state.linkedOnly) {
          state.linkedOnly = false;
        } else {
          state.linkedOnly = true;
          state.unlinked = false;
        }
        linkedBtn.classList.toggle("active", state.linkedOnly);
        if (unlinkedBtn) unlinkedBtn.classList.toggle("active", state.unlinked);
        state.page = 0;
        updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  }
  if (unlinkedBtn) {
    unlinkedBtn.addEventListener(
      "click",
      () => {
        if (state.unlinked) {
          state.unlinked = false;
        } else {
          state.unlinked = true;
          state.linkedOnly = false;
        }
        unlinkedBtn.classList.toggle("active", state.unlinked);
        if (linkedBtn) linkedBtn.classList.toggle("active", state.linkedOnly);
        state.page = 0;
        updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  }

  // ── Non-default tags filter toggle ──
  const nonDefaultBtn = container.querySelector("#files-filter-non-default");
  if (nonDefaultBtn) {
    nonDefaultBtn.addEventListener(
      "click",
      () => {
        state.nonDefaultOnly = !state.nonDefaultOnly;
        nonDefaultBtn.classList.toggle("active", state.nonDefaultOnly);
        state.page = 0;
        updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  }

  // ── Comment writer panel (shared component) ──
  wireCommentWriter(container, signal, async (linkedOnly, tagNames, nonDefaultOnly) => {
    const execBtn = container.querySelector("#cw-execute");
    if (!execBtn) return;
    execBtn.disabled = true;
    const originalHtml = execBtn.innerHTML;
    execBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Queuing...';
    try {
      const body = {
        linkedOnly,
        tags: tagNames.length > 0 ? tagNames : undefined,
        nonDefaultOnly,
      };
      const resp = await fetchJSON("/api/files/bulk-sync", {
        method: "POST",
        body: JSON.stringify(body),
      });
      const taskId = resp.data?.taskId || resp.data;
      if (taskId) {
        showToast(
          `Comment write task #${taskId} started. Check Tasks page for progress.`,
          "success",
        );
      } else {
        showToast("All comments are up to date — nothing to write.", "info");
      }
      execBtn.disabled = false;
      execBtn.innerHTML = originalHtml;
    } catch (err) {
      showToast(`Failed to queue comment writes: ${err.message}`, "error");
      execBtn.disabled = false;
      execBtn.innerHTML = originalHtml;
    }
  });

  // ── Multi-select service filter ──
  const serviceGroup = container.querySelector(".service-filter-group");
  if (serviceGroup) {
    serviceGroup.addEventListener(
      "click",
      (e) => {
        const btn = e.target.closest(".filter-btn");
        if (!btn) return;
        const value = btn.dataset.value;
        const idx = state.selectedServices.indexOf(value);
        if (idx >= 0) {
          state.selectedServices.splice(idx, 1);
        } else {
          state.selectedServices.push(value);
        }
        btn.classList.toggle("active");
        state.page = 0;
        updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  }

  // ── PMV category buttons (multi-select: P, M, V) ──
  const pmvCatBtns = container.querySelector("#pmv-cat-btns");
  if (pmvCatBtns) {
    pmvCatBtns.addEventListener(
      "click",
      (e) => {
        const btn = e.target.closest(".filter-btn");
        if (!btn) return;
        const val = btn.dataset.value;
        const idx = state.pmvCategories.indexOf(val);
        if (idx >= 0) {
          state.pmvCategories.splice(idx, 1);
        } else {
          // Clear aggregate group when picking categories
          state.pmvAggregate = "";
          container
            .querySelectorAll("#pmv-agg-btns .filter-btn")
            .forEach((b) => b.classList.remove("active"));
          state.pmvCategories.push(val);
        }
        btn.classList.toggle("active");
        state.page = 0;
        updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  }

  // ── PMV aggregate buttons (single-select: Full, Partial, None) ──
  const pmvAggBtns = container.querySelector("#pmv-agg-btns");
  if (pmvAggBtns) {
    pmvAggBtns.addEventListener(
      "click",
      (e) => {
        const btn = e.target.closest(".filter-btn");
        if (!btn) return;
        const val = btn.dataset.value;
        if (state.pmvAggregate === val) {
          state.pmvAggregate = "";
        } else {
          // Clear category group when picking aggregate
          state.pmvCategories = [];
          container
            .querySelectorAll("#pmv-cat-btns .filter-btn")
            .forEach((b) => b.classList.remove("active"));
          state.pmvAggregate = val;
        }
        btn.classList.toggle("active");
        state.page = 0;
        updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  }

  // ── Multi-select comment status filter ──
  const commentBtns = container.querySelector("#files-comment-btns");
  if (commentBtns) {
    commentBtns.addEventListener(
      "click",
      (e) => {
        const btn = e.target.closest(".filter-btn");
        if (!btn) return;
        const value = btn.dataset.value;
        const idx = state.commentStatuses.indexOf(value);
        if (idx >= 0) {
          state.commentStatuses.splice(idx, 1);
        } else {
          state.commentStatuses.push(value);
        }
        btn.classList.toggle("active");
        state.page = 0;
        updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  }

  // ── Multi-select file type filter ──
  const filetypeBtns = container.querySelector("#files-filetype-filter");
  if (filetypeBtns) {
    filetypeBtns.addEventListener(
      "click",
      (e) => {
        const btn = e.target.closest(".filter-btn");
        if (!btn) return;
        const value = btn.dataset.value;
        const idx = state.fileTypes.indexOf(value);
        if (idx >= 0) {
          state.fileTypes.splice(idx, 1);
        } else {
          state.fileTypes.push(value);
        }
        btn.classList.toggle("active");
        state.page = 0;
        updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  }

  // ── Backup status filter ──
  filterPanel?.querySelectorAll("[data-backup-filter]").forEach((btn) => {
    btn.addEventListener(
      "click",
      () => {
        const val = btn.dataset.backupFilter;
        state.backedUp = val === "all" ? null : val === "yes";
        state.page = 0;
        filterPanel
          .querySelectorAll("[data-backup-filter]")
          .forEach((b) => b.classList.remove("active"));
        btn.classList.add("active");
        updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  });

  // ── On Disk filter ──
  filterPanel?.querySelectorAll("[data-local-filter]").forEach((btn) => {
    btn.addEventListener(
      "click",
      () => {
        const val = btn.dataset.localFilter;
        state.isLocal = val === "all" ? null : val === "yes";
        state.page = 0;
        filterPanel
          .querySelectorAll("[data-local-filter]")
          .forEach((b) => b.classList.remove("active"));
        btn.classList.add("active");
        updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  });

  // ── Safe to delete filter ──
  filterPanel?.querySelectorAll("[data-safe-filter]").forEach((btn) => {
    btn.addEventListener(
      "click",
      () => {
        const val = btn.dataset.safeFilter;
        state.safeToDelete = val === "all" ? null : val === "yes";
        state.page = 0;
        filterPanel
          .querySelectorAll("[data-safe-filter]")
          .forEach((b) => b.classList.remove("active"));
        btn.classList.add("active");
        updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  });

  // ── Generic toggle for data-filter labels ──
  filterPanel?.querySelectorAll("[data-filter]").forEach((label) => {
    function updateFilterUI() {
      const key = label.dataset.filter + "Enabled";
      const isActive = state[key] !== false;
      label.classList.toggle("active", isActive);
      label.classList.toggle("off", !isActive);
      const row = label.closest(".filter-row");
      if (row) {
        const inputs = row.querySelectorAll(
          "select, input, button, .filter-group, .tag-chips, .dual-range-wrap, .key-grid-wrap, .typeahead-wrap",
        );
        inputs.forEach((el) => el.classList.toggle("filter-disabled", !isActive));
      }
    }
    label.addEventListener("click", () => {
      const key = label.dataset.filter + "Enabled";
      state[key] = state[key] === false ? true : false;
      state.page = 0;
      updateFilterUI();
      updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
      fetchAndRender(container, signal, state);
    });
    updateFilterUI();
  });

  // ── Auto-enable disabled filter sections on click ──
  filterPanel?.addEventListener("click", (e) => {
    const row = e.target.closest(".filter-row");
    if (!row) return;
    const label = row.querySelector("[data-filter]");
    if (!label) return;
    const key = label.dataset.filter + "Enabled";
    if (state[key] !== false) return;
    if (e.target.closest("[data-filter]")) return;
    state[key] = true;
    state.page = 0;
    label.classList.add("active");
    label.classList.remove("off");
    row
      .querySelectorAll(
        "select, input, button, .filter-group, .tag-chips, .dual-range-wrap, .key-grid-wrap, .typeahead-wrap",
      )
      .forEach((el) => el.classList.remove("filter-disabled"));
    updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
    fetchAndRender(container, signal, state);
  });
}

/* ------------------------------------------------------------------ */
/*  Content event wiring (called after each body render)               */
/* ------------------------------------------------------------------ */

function wireContentEvents(container, signal, state) {
  const contentEl = container.querySelector("#files-content");
  if (!contentEl) return;

  // Avoid { signal: null } — Safari throws TypeError on addEventListener
  const listenerOpts = signal != null ? { signal } : undefined;

  // ── Refresh button ──
  const refreshBtn = contentEl.querySelector("#files-refresh");
  if (refreshBtn) {
    refreshBtn.addEventListener(
      "click",
      () => {
        updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
        fetchAndRender(container, signal, state);
      },
      listenerOpts,
    );
  }

  // ── Sortable headers ──
  const tableEl = contentEl.querySelector(".data-table");
  if (tableEl) {
    wireSortableHeaders(tableEl, state, () => {
      updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
      fetchAndRender(container, signal, state);
    });
  }

  // ── Page size selector ──
  const pageSizeSel = contentEl.querySelector("[data-page-size]");
  if (pageSizeSel) {
    pageSizeSel.addEventListener(
      "change",
      () => {
        const val = parseInt(pageSizeSel.value, 10);
        localStorage.setItem("crudPageSize", String(val));
        state.pageSize = val;
        state.page = 0;
        updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
        fetchAndRender(container, signal, state);
      },
      listenerOpts,
    );
  }

  // ── Pagination ──
  const prevBtn = contentEl.querySelector("#files-page-prev");
  if (prevBtn) {
    prevBtn.addEventListener(
      "click",
      () => {
        if (state.page > 0) {
          state.page--;
          updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
          fetchAndRender(container, signal, state);
        }
      },
      listenerOpts,
    );
  }

  const nextBtn = contentEl.querySelector("#files-page-next");
  if (nextBtn) {
    nextBtn.addEventListener(
      "click",
      () => {
        state.page++;
        updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
        fetchAndRender(container, signal, state);
      },
      listenerOpts,
    );
  }

  // ── Action buttons (via delegation) ──
  contentEl.addEventListener(
    "click",
    (e) => {
      const btn = e.target.closest("button[data-action]");
      if (!btn) return;
      const action = btn.dataset.action;
      const id = parseInt(btn.dataset.id, 10);
      if (action === "write-comment") {
        writeComment(id);
      } else if (action === "view") {
        viewFile(id);
      } else if (action === "similar") {
        showSimilarTracks(id);
      }
    },
    listenerOpts,
  );

  // ── Column resize, reorder, config modal ──
  const colConfig = loadColumnConfig("files", FILES_COLUMNS);
  if (state.layoutMode) {
    wireColumnResize(contentEl, "files", FILES_COLUMNS, colConfig);
    wireColumnDragReorder(contentEl, "files", FILES_COLUMNS, colConfig, () => {
      fetchAndRender(container, signal, state);
    });
  }
  wireConfigTrigger(contentEl, "files", FILES_COLUMNS, colConfig, () => {
    fetchAndRender(container, signal, state);
  });

  // ── Layout mode toggle ──
  const layoutBtn = contentEl.querySelector("#files-layout-btn");
  if (layoutBtn) {
    layoutBtn.addEventListener(
      "click",
      () => {
        state.layoutMode = !state.layoutMode;
        document.body.classList.toggle("layout-mode", state.layoutMode);
        updateHash("files", state, HASH_DEFAULTS, HASH_SCHEMA);
        fetchAndRender(container, signal, state);
      },
      listenerOpts,
    );
  }

  // ── Checkbox selection ──
  // Select-all checkbox
  const selectAllCb = container.querySelector("#files-select-all");
  if (selectAllCb) {
    selectAllCb.onclick = () => {
      // In select-all mode, clicking the header checkbox exits all-mode and deselects everything
      if (state.selectAllMode) {
        state.selectAllMode = false;
        state.selectAllTotal = 0;
        state.selectedFileIds.clear();
        selectAllCb.checked = false;
        const rowCbs = container.querySelectorAll(".files-row-checkbox");
        rowCbs.forEach((cb) => {
          cb.checked = false;
        });
        updateSelectionUI(container, state);
        injectSelectAllBanner(container, state);
        return;
      }

      const checked = selectAllCb.checked;
      const rowCbs = container.querySelectorAll(".files-row-checkbox");
      rowCbs.forEach((cb) => {
        const fileId = parseInt(cb.dataset.fileId, 10);
        if (checked) state.selectedFileIds.add(fileId);
        else state.selectedFileIds.delete(fileId);
        cb.checked = checked;
      });
      updateSelectionUI(container, state);
      injectSelectAllBanner(container, state);
    };
  }

  // Individual row checkboxes
  const rowCbs = container.querySelectorAll(".files-row-checkbox");
  rowCbs.forEach((cb) => {
    cb.onclick = () => {
      // In select-all mode, clicking a row checkbox exits all-mode
      // and selects only the clicked file
      if (state.selectAllMode) {
        state.selectAllMode = false;
        state.selectAllTotal = 0;
        state.selectedFileIds.clear();
      }

      const fileId = parseInt(cb.dataset.fileId, 10);
      if (cb.checked) state.selectedFileIds.add(fileId);
      else state.selectedFileIds.delete(fileId);
      const allCb = container.querySelector("#files-select-all");
      if (allCb) {
        const allRowCbs = container.querySelectorAll(".files-row-checkbox");
        allCb.checked =
          allRowCbs.length > 0 && Array.from(allRowCbs).every((rc) => rc.checked);
      }
      updateSelectionUI(container, state);
      injectSelectAllBanner(container, state);
    };
  });
}

/* ------------------------------------------------------------------ */
/*  Selection + Bulk Actions                                           */
/* ------------------------------------------------------------------ */

/**
 * Inject or remove the "Select all N" banner into the DOM without re-rendering.
 * Reads total count from state._lastTotal and file count from DOM.
 */
function injectSelectAllBanner(container, state) {
  const banner = container.querySelector("#files-select-all-banner");
  if (!banner) return;

  const rowCbs = container.querySelectorAll(".files-row-checkbox");
  const pageCount = rowCbs.length;
  const total = state._lastTotal || 0;
  const pageSize = state.pageSize || 50;

  if (state.selectAllMode && state.selectAllTotal > 0) {
    banner.innerHTML = `<span>All <strong>${state.selectAllTotal}</strong> files are selected.</span>
      <button class="btn btn-sm" id="files-clear-selection">Clear selection</button>`;
    banner.style.display = "";
    // Wire the clear button
    const clearBtn = banner.querySelector("#files-clear-selection");
    if (clearBtn) {
      clearBtn.onclick = () => {
        state.selectAllMode = false;
        state.selectAllTotal = 0;
        state.selectedFileIds.clear();
        state.needsCommentCount = 0;
        updateSelectionUI(container, state);
        injectSelectAllBanner(container, state);
        const selectAllCb = container.querySelector("#files-select-all");
        if (selectAllCb) selectAllCb.checked = false;
        rowCbs.forEach((cb) => {
          cb.checked = false;
        });
      };
    }
  } else if (
    !state.selectAllMode &&
    pageCount > 0 &&
    total > pageSize &&
    Array.from(rowCbs).every((cb) => cb.checked)
  ) {
    banner.innerHTML = `<span>All <strong>${pageCount}</strong> files on this page are selected.</span>
      <button class="btn btn-sm btn-primary" id="files-select-all-pages">Select all <strong>${total}</strong> files matching current filters</button>`;
    banner.style.display = "";
    // Wire the select-all-pages button
    const allBtn = banner.querySelector("#files-select-all-pages");
    if (allBtn) {
      allBtn.onclick = () => {
        state.selectAllMode = true;
        state.selectAllTotal = total;
        updateSelectionUI(container, state);
        injectSelectAllBanner(container, state);
        rowCbs.forEach((cb) => {
          cb.checked = true;
        });
        const selectAllCb = container.querySelector("#files-select-all");
        if (selectAllCb) selectAllCb.checked = true;
      };
    }
  } else {
    banner.style.display = "none";
    banner.innerHTML = "";
  }
}

function updateSelectionUI(container, state) {
  const count = state.selectAllMode ? state.selectAllTotal : state.selectedFileIds.size;
  updateSelectionCount(container, "files", count);
  computeNeedsCount(container, state);
}

async function computeNeedsCount(container, state) {
  const btn = container.querySelector("#files-actions-write-comments");
  if (!btn) return;

  const hasSelection = state.selectAllMode || state.selectedFileIds.size > 0;
  if (!hasSelection) {
    btn.innerHTML = '<i class="fas fa-pen"></i> WRITE COMMENTS';
    state.needsCommentCount = 0;
    return;
  }

  btn.innerHTML = '<i class="fas fa-pen"></i> WRITE COMMENTS (...)';
  btn.disabled = true;

  try {
    let resp;
    if (state.selectAllMode) {
      // Use the same GET params as the page for guaranteed filter parity.
      // The count endpoint already handles isLocal, commentStatuses, etc.
      const params = buildParams(state);
      params.delete("limit");
      params.delete("offset");
      params.delete("sort");
      params.delete("order");
      resp = await fetchJSON(`/api/files/count?${params}`);
    } else {
      const selectedIds = Array.from(state.selectedFileIds);
      resp = await fetchJSON("/api/files/needs-comment-count", {
        method: "POST",
        body: JSON.stringify({ fileIds: selectedIds }),
      });
    }
    // count endpoint returns a plain number; needs-comment-count returns {filesNeedingUpdate}
    state.needsCommentCount =
      typeof resp.data === "number" ? resp.data : resp.data.filesNeedingUpdate || 0;
    btn.innerHTML = `<i class="fas fa-pen"></i> WRITE COMMENTS (${state.needsCommentCount})`;
  } catch (err) {
    console.warn("Failed to compute needs-comment count:", err);
    btn.innerHTML = '<i class="fas fa-pen"></i> WRITE COMMENTS';
  } finally {
    btn.disabled = !(state.selectAllMode || state.selectedFileIds.size > 0);
  }
}

async function writeCommentsForSelected(container, state) {
  const hasSelection = state.selectAllMode || state.selectedFileIds.size > 0;
  if (!hasSelection) {
    showToast("No files selected.", "warning");
    return;
  }

  const btn = container.querySelector("#files-actions-write-comments");
  if (btn) {
    btn.disabled = true;
    btn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Writing...';
  }

  try {
    let resp;
    if (state.selectAllMode) {
      const filterParams = buildFilterParams(state);
      resp = await fetchJSON("/api/files/write-comments-all", {
        method: "POST",
        body: JSON.stringify(filterParams),
      });
    } else {
      const selectedIds = Array.from(state.selectedFileIds);
      resp = await fetchJSON("/api/files/write-comments-by-ids", {
        method: "POST",
        body: JSON.stringify({ fileIds: selectedIds }),
      });
    }
    const data = resp.data;
    if (data.fileCount > 0) {
      showToast(
        `Comment write queued (task #${data.taskId}, ${data.fileCount} file(s))`,
        "success",
      );
    } else {
      showToast("All comments are up to date", "info");
    }
    state.selectedFileIds.clear();
    state.selectAllMode = false;
    state.selectAllTotal = 0;
    state.needsCommentCount = 0;
    updateSelectionUI(container, state);
    fetchAndRender(container, null, state);
  } catch (err) {
    showToast(`Failed to queue comment write: ${err.message}`, "error");
  } finally {
    if (btn) {
      btn.disabled = false;
      btn.innerHTML = '<i class="fas fa-pen"></i> WRITE COMMENTS';
    }
  }
}

/* ------------------------------------------------------------------ */
/*  Initialisation                                                     */
/* ------------------------------------------------------------------ */

/**
 * Entry point called by app.js.
 * @param {HTMLElement} container - The page container element
 * @param {AbortSignal} signal - Abort signal for cleanup
 * @param {object} hashParams - Parsed hash params from app.js getHashParams()
 */
export async function init(container, signal, hashParams) {
  // Parse hash params into state
  const parsed = parseHash(hashParams, HASH_SCHEMA);

  const state = {
    page: parsed.page,
    pageSize: getPageSize(),
    search: parsed.search,
    sort: parsed.sort,
    order: parsed.order,
    bpmMin: parsed.bpmMin,
    bpmMax: parsed.bpmMax,
    keys: parsed.keys,
    selectedTags: parsed.selectedTags,
    linkedOnly: parsed.linkedOnly,
    unlinked: parsed.unlinked,
    nonDefaultOnly: parsed.nonDefaultOnly,
    selectedServices: parsed.selectedServices,
    pmvCategories: parsed.pmvCategories,
    pmvAggregate: parsed.pmvAggregate,
    commentStatuses: parsed.commentStatuses,
    fileTypes: parsed.fileTypes,
    backedUp: parsed.backedUp,
    isLocal: parsed.isLocal,
    safeToDelete: parsed.safeToDelete,
    // Filter section enable/disable flags
    bpmEnabled: true,
    keyEnabled: true,
    ratingEnabled: true,
    playsEnabled: true,
    tagEnabled: true,
    serviceEnabled: true,
    pmvEnabled: true,
    commentEnabled: true,
    fileTypeEnabled: true,
    backupEnabled: true,
    safeEnabled: true,
    localEnabled: true,
    layoutMode: false,
    selectedFileIds: new Set(),
    selectAllMode: false,
    selectAllTotal: 0,
    _lastTotal: 0,
    needsCommentCount: 0,
    allTags: [], // pre-fetched at page load for typeahead
  };

  // Pre-fetch all tags once for client-side typeahead filtering
  fetchJSON("/api/tags?page_size=10000")
    .then((resp) => {
      state.allTags = resp.data || [];
    })
    .catch(() => {
      state.allTags = [];
    });

  // Reset layout mode on page entry
  document.body.classList.remove("layout-mode");

  // Build actions panel
  const actionsHtml = renderActionsPanel("files", [
    {
      id: "write-comments",
      label: "WRITE COMMENTS",
      icon: "fas fa-pen",
      cls: "btn-primary",
      action: "write-comments",
    },
    {
      id: "stage-conversion",
      label: "Stage for Conversion",
      icon: "fas fa-flask",
      cls: "btn-accent",
      action: "stage-conversion",
    },
  ]);

  // Render toolbar ONCE (stable, preserves focus)
  container.innerHTML = `
    <div style="display:flex;flex-direction:column;gap:var(--space-4);">
      <div style="display:flex;gap:var(--space-4);align-items:flex-start;">
        <div style="flex:4">${renderToolbar(state)}</div>
        ${actionsHtml}
      </div>
      <div id="files-content"></div>
    </div>`;

  // Wire toolbar events (runs once)
  wireToolbarEvents(container, signal, state);

  // Wire actions panel
  import("../shared/actions-panel.js").then(({ wireActionsRefresh }) => {
    wireActionsRefresh(container, "files", () => {
      state.page = 0;
      return fetchAndRender(container, signal, state);
    });
  });

  // Wire WRITE COMMENTS button in actions panel
  const writeBtn = container.querySelector("#files-actions-write-comments");
  if (writeBtn) {
    writeBtn.onclick = () => writeCommentsForSelected(container, state);
  }

  // Wire Stage for Conversion button in actions panel
  const stageBtn = container.querySelector("#files-actions-stage-conversion");
  if (stageBtn) {
    stageBtn.onclick = async () => {
      const filter = buildFilterParams(state);
      const originalHtml = stageBtn.innerHTML;
      stageBtn.disabled = true;
      stageBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Staging…';
      try {
        const resp = await fetchJSON("/api/files/stage-for-conversion", {
          method: "POST",
          body: JSON.stringify(filter),
        });
        const data = resp.data;
        showToast(
          `${data.staged} file(s) staged in ${data.directory}`,
          data.staged > 0 ? "success" : "info",
        );
      } catch (err) {
        showToast(`Staging failed: ${err.message}`, "error");
      } finally {
        stageBtn.disabled = false;
        stageBtn.innerHTML = originalHtml;
      }
    };
  }

  // Initial fetch + render
  await fetchAndRender(container, signal, state);
}
