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
};

/* ------------------------------------------------------------------ */
/*  Column model                                                        */
/* ------------------------------------------------------------------ */

const FILES_COLUMNS = [
  { id: "title", label: "Title", sortable: true, sortKey: "title", defaultWidth: 18 },
  { id: "artist", label: "Artist", sortable: true, sortKey: "artist", defaultWidth: 6 },
  { id: "bpm", label: "BPM", sortable: true, sortKey: "bpm", defaultWidth: 8 },
  { id: "key", label: "Key", sortable: true, sortKey: "key", defaultWidth: 3 },
  { id: "linked", label: "Linked", sortable: false, defaultWidth: 2 },
  { id: "isrc", label: "ISRC", sortable: true, sortKey: "isrc", defaultWidth: 3 },
  { id: "plays", label: "Plays", sortable: true, sortKey: "play_count", defaultWidth: 3 },
  {
    id: "duration",
    label: "Duration",
    sortable: true,
    sortKey: "duration_ms",
    defaultWidth: 5,
  },
  { id: "album", label: "Album", sortable: false, defaultWidth: 5 },
  {
    id: "created",
    label: "Created",
    sortable: true,
    sortKey: "created_at",
    defaultWidth: 7,
  },
  {
    id: "lastPlayed",
    label: "Last Played",
    sortable: true,
    sortKey: "last_played",
    defaultWidth: 7,
  },
  { id: "comment", label: "Comment Diff", sortable: false, defaultWidth: 25 },
  { id: "actions", label: "Actions", sortable: false, defaultWidth: 12 },
];

/* ------------------------------------------------------------------ */
/*  Cell renderers (columnId → render function)                         */
/* ------------------------------------------------------------------ */

const FILES_CELL_RENDERERS = {
  title: (f) => escapeHtml(f.title),
  artist: (f) => escapeHtml(f.artist),
  bpm: (f) => `<span class="font-mono">${formatBPM(f.bpm)}</span>`,
  key: (f) => renderKeyBadge(f.key),
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
    needsUpdate: f.needsUpdate,
    comment: f.comment,
    commentTarget: f.commentTarget,
    playCount: f.playCount,
    lastPlayed: f.lastPlayed || null,
    matchedServices: f.matchedServices || [],
    album: f.album || null,
    duration: f.durationMs ? Math.round(f.durationMs / 1000) : 0,
    createdAt: f.createdAt || null,
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
  const diffClass = f.commentUnchanged ? "diff-line-unchanged" : "diff-line";
  if (f.commentUnchanged) {
    return `<div class="${diffClass}"><span class="diff-sign check">✓</span>${escapeHtml(f.comment)}</div>`;
  }
  return `<div class="${diffClass}">
    <div class="diff-line-old"><span class="diff-sign minus">−</span>${escapeHtml(f.diffOld)}</div>
    <div class="diff-line-new"><span class="diff-sign plus">+</span>${escapeHtml(f.diffNew)}</div>
  </div>`;
}

function renderFileActions(f) {
  return `
    <button class="btn btn-sm btn-icon" data-action="view" data-id="${f.id}" title="View details"><i class="fas fa-eye"></i></button>
    <button class="btn btn-sm btn-icon" data-action="similar" data-id="${f.id}" title="Similar tracks by tag"><i class="fas fa-project-diagram"></i></button>
    <button class="btn btn-sm btn-icon" data-action="write-comment" data-id="${f.id}" title="Write comment to file" ${f.commentTarget ? "" : "disabled"}><i class="fas fa-pen"></i></button>
  `;
}

function renderRows(files) {
  return files
    .map((f) => {
      const diffClass = f.commentUnchanged ? "diff-line-unchanged" : "diff-line";
      const diffRow = f.commentUnchanged
        ? `<span class="diff-sign check">✓</span>${escapeHtml(f.comment)}`
        : `<div class="diff-line-old"><span class="diff-sign minus">−</span>${escapeHtml(f.diffOld)}</div>
           <div class="diff-line-new"><span class="diff-sign plus">+</span>${escapeHtml(f.diffNew)}</div>`;
      return `<tr>
        <td>${escapeHtml(f.title)}</td>
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
  try {
    const resp = await fetchJSON(`/api/files/${id}`);
    const f = adaptFile(resp.data);
    const detailsHtml = `
      <div style="display:grid;grid-template-columns:auto 1fr;gap:8px 16px;font-size:0.9rem;">
        <strong>ID:</strong><span>${f.id}</span>
        <strong>Title:</strong><span>${escapeHtml(f.title)}</span>
        <strong>Artist:</strong><span>${escapeHtml(f.artist)}</span>
        <strong>BPM:</strong><span>${f.bpm ? formatBPM(f.bpm) : "—"}</span>
        <strong>Key:</strong><span>${renderKeyBadge(f.key)}</span>
        <strong>Linked:</strong><span>${renderLinkBadge(f.matchedServices)}</span>
        <strong>Plays:</strong><span>${f.playCount ?? 0}</span>
        <strong>Last played:</strong><span>${f.lastPlayed || "—"}</span>
        ${f.diffOld ? `<strong>Comment (current):</strong><span class="diff-line-old">${escapeHtml(f.diffOld)}</span>` : ""}
        ${f.diffNew ? `<strong>Comment (target):</strong><span class="diff-line-new">${escapeHtml(f.diffNew)}</span>` : ""}
        ${f.commentUnchanged ? `<strong>Comment:</strong><span>${escapeHtml(f.comment)}</span>` : ""}
      </div>`;

    showModal({
      title: escapeHtml(f.title),
      bodyHtml: `<div style="padding:16px">${detailsHtml}</div>`,
      width: "600px",
    });
  } catch (err) {
    showToast(`Failed to load file details: ${err.message}`, "error");
  }
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
        <div class="filter-panel-scroll">
          <div class="filter-row">
            <span class="filter-row-label">BPM</span>
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
                <span class="sep">──</span>
                <span class="dual-range-max-val">${bpmMax}</span>
              </div>
            </div>
          </div>
          <div class="filter-row">
            <span class="filter-row-label">Key</span>
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
        </div>
        <div class="filter-tag-area">
          <div class="tag-search-wrap">
            <i class="fas fa-tag"></i>
            <input type="text" class="input-text input-search" id="files-tag-search"
                   placeholder="filter by TAG" autocomplete="off">
            <div class="tag-dropdown" id="files-tag-dropdown"></div>
          </div>
          <div class="tag-chips" id="files-tag-chips">${chipsHtml}</div>
        </div>
        <div class="filter-row">
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

  return `
    <div style="display:flex;gap:var(--space-4);align-items:flex-start;">
      <div style="flex:2;min-width:0;">${toolbarHtml}</div>
      ${commentWriterHtml}
    </div>`;
}

/* ------------------------------------------------------------------ */
/*  Render: Body (re-rendered on each fetch)                           */
/* ------------------------------------------------------------------ */

function renderEmptyBody() {
  const config = loadColumnConfig("files", FILES_COLUMNS);
  const headers = renderColumnHeaders(
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
      <thead><tr>${headers}</tr></thead>
      <tbody><tr><td colspan="${visibleCount}"><div class="text-center text-muted" style="padding:32px;text-align:center;">No files found. Scan a folder to get started.</div></td></tr></tbody>
    </table></div>`;
}

function renderBody(data, state) {
  const totalPages = Math.ceil(data._total / state.pageSize) || 1;
  const currentPage = state.page + 1;
  const config = loadColumnConfig("files", FILES_COLUMNS);
  const headers = renderColumnHeaders(config, FILES_COLUMNS, state, sortableTh);

  const rowsHtml = data.files
    .map(
      (f) =>
        `<tr>${renderColumnCells(config, FILES_COLUMNS, FILES_CELL_RENDERERS, f)}</tr>`,
    )
    .join("");

  return `
    <div class="stats-row">
      <div class="stats-group">
        <button class="btn btn-sm btn-icon" id="files-refresh" title="Refresh"><i class="fa-solid fa-rotate"></i></button>
        <strong>${data._total}</strong> files
        ${renderPageSizeSelector(state.pageSize)}
        ${renderColumnConfigTrigger()}
      </div>
    </div>
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
  if (state.sort) params.set("sort", state.sort);
  if (state.order === "desc") params.set("order", "desc");
  return params;
}

/* ------------------------------------------------------------------ */
/*  Fetch + Render cycle                                               */
/* ------------------------------------------------------------------ */

async function fetchAndRender(signal, state) {
  const contentEl = document.getElementById("files-content");
  if (!contentEl) return;
  contentEl.innerHTML = renderLoading("Loading files…");

  try {
    const params = buildParams(state);
    const countParams = new URLSearchParams(params);
    countParams.delete("limit");
    countParams.delete("offset");

    const [filesResp, countResp] = await Promise.all([
      fetchJSON(`/api/files?${params}`, { signal }),
      fetchJSON(`/api/files/count?${countParams}`, { signal }),
    ]);
    if (signal.aborted) return;

    const data = {
      _total: countResp.data,
      files: filesResp.data.map(adaptFile),
    };

    if (data.files.length === 0 && data._total === 0) {
      contentEl.innerHTML = renderEmptyBody();
    } else {
      contentEl.innerHTML = renderBody(data, state);
    }

    wireContentEvents(signal, state);
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
      updateHash("files", state, HASH_DEFAULTS);
      fetchAndRender(signal, state);
    });
  }

  // ── Filter panel toggle (collapsible) ──
  const panelToggle = container.querySelector("#files-filter-toggle");
  const panel = container.querySelector("#files-filter-panel");
  if (panelToggle && panel) {
    panelToggle.addEventListener(
      "click",
      () => {
        panel.classList.toggle("collapsed");
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
          updateHash("files", state, HASH_DEFAULTS);
          fetchAndRender(signal, state);
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
        state.page = 0;
        updateHash("files", state, HASH_DEFAULTS);
        fetchAndRender(signal, state);
      },
      { signal },
    );
  }

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
      selectedIndex = -1;
      renderTagChips();
      updateHash("files", state, HASH_DEFAULTS);
      fetchAndRender(signal, state);
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
          return;
        }
        timer = setTimeout(async () => {
          try {
            const resp = await fetchJSON(`/api/tags?search=${encodeURIComponent(q)}`);
            const tags = resp.data || [];
            if (tags.length === 0) {
              tagDropdown.innerHTML = `<div class="tag-dropdown-empty">No tags found</div>`;
              selectedIndex = -1;
            } else {
              tagDropdown.innerHTML = tags
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
            tagDropdown.classList.add("open");
          } catch {
            // ignore errors during search
          }
        }, 150);
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
        selectedIndex = -1;
        renderTagChips();
        updateHash("files", state, HASH_DEFAULTS);
        fetchAndRender(signal, state);
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
          selectedIndex = -1;
        }
      },
      { signal },
    );
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
        updateHash("files", state, HASH_DEFAULTS);
        fetchAndRender(signal, state);
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
        state.page = 0;
        updateHash("files", state, HASH_DEFAULTS);
        fetchAndRender(signal, state);
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
        state.page = 0;
        updateHash("files", state, HASH_DEFAULTS);
        fetchAndRender(signal, state);
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
        state.page = 0;
        updateHash("files", state, HASH_DEFAULTS);
        fetchAndRender(signal, state);
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
}

/* ------------------------------------------------------------------ */
/*  Content event wiring (called after each body render)               */
/* ------------------------------------------------------------------ */

function wireContentEvents(signal, state) {
  const contentEl = document.getElementById("files-content");
  if (!contentEl) return;

  // ── Refresh button ──
  const refreshBtn = contentEl.querySelector("#files-refresh");
  if (refreshBtn) {
    refreshBtn.addEventListener(
      "click",
      () => {
        updateHash("files", state, HASH_DEFAULTS);
        fetchAndRender(signal, state);
      },
      { signal },
    );
  }

  // ── Sortable headers ──
  const tableEl = contentEl.querySelector(".data-table");
  if (tableEl) {
    wireSortableHeaders(tableEl, state, () => {
      updateHash("files", state, HASH_DEFAULTS);
      fetchAndRender(signal, state);
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
        updateHash("files", state, HASH_DEFAULTS);
        fetchAndRender(signal, state);
      },
      { signal },
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
          updateHash("files", state, HASH_DEFAULTS);
          fetchAndRender(signal, state);
        }
      },
      { signal },
    );
  }

  const nextBtn = contentEl.querySelector("#files-page-next");
  if (nextBtn) {
    nextBtn.addEventListener(
      "click",
      () => {
        state.page++;
        updateHash("files", state, HASH_DEFAULTS);
        fetchAndRender(signal, state);
      },
      { signal },
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
    { signal },
  );

  // ── Column resize, reorder, config modal ──
  const colConfig = loadColumnConfig("files", FILES_COLUMNS);
  wireColumnResize(contentEl, "files", FILES_COLUMNS, colConfig);
  wireColumnDragReorder(contentEl, "files", FILES_COLUMNS, colConfig, () => {
    fetchAndRender(signal, state);
  });
  wireConfigTrigger(contentEl, "files", FILES_COLUMNS, colConfig, () => {
    fetchAndRender(signal, state);
  });
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
  };

  // Render toolbar ONCE (stable, preserves focus)
  container.innerHTML = `
    <div id="files-toolbar">${renderToolbar(state)}</div>
    <div id="files-content"></div>`;

  // Wire toolbar events (runs once)
  wireToolbarEvents(container, signal, state);

  // Initial fetch + render
  await fetchAndRender(signal, state);
}
