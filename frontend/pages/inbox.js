/**
 * inbox.js — Tag Roundtrip Inbox page.
 *
 * Lists files whose stored comment does NOT match the generated target
 * comment (compared via roundtrip: parse → generate → compare). Formatting
 * differences (tag order, quoting, case) never produce false positives —
 * only real content changes are listed.
 *
 * API:
 *   GET /api/inbox?limit=&offset= → { files: [{ fileId, filePath, title,
 *       artist, comment, targetComment, diff }], total }
 *   GET /api/inbox/count → { count }
 *
 * Each row shows the stored comment, the target comment, and a structured
 * diff (tags added/removed, PMV changes, source IDs). A "Write" button
 * queues the existing comment-write task for that file.
 */

import { fetchJSON } from "../shared/api.js";
import {
  escapeHtml,
  renderLoading,
  renderErrorBlock,
  renderEmpty,
  renderTable,
  td,
  showToast,
} from "../shared/components.js";

const PAGE_SIZE = 50;

let state = {
  files: [],
  total: 0,
  page: 0,
  loading: false,
};

let _container = null;
let _signal = null;

/* ------------------------------------------------------------------ */
/*  Page Init                                                         */
/* ------------------------------------------------------------------ */

/**
 * Page init — called by the SPA router on #inbox.
 * @param {HTMLElement} container
 * @param {AbortSignal} signal
 */
export async function init(container, signal, hashParams) {
  _container = container;
  _signal = signal;
  state.page = parseInt(hashParams.page || "0", 10) || 0;

  container.innerHTML = renderLoading("Loading inbox…");
  if (signal.aborted) return;

  try {
    const resp = await fetchJSON(
      `/api/inbox?limit=${PAGE_SIZE}&offset=${state.page * PAGE_SIZE}`,
      { signal },
    );
    if (signal.aborted) return;
    state.files = resp.data.files || [];
    state.total = resp.data.total || 0;

    renderPage(container);
    wireEvents(container, signal);
  } catch (err) {
    if (err.name === "AbortError") return;
    container.innerHTML = renderErrorBlock({
      title: "Failed to load inbox",
      detail: err.message || "Unknown error",
      retryFn: "window.location.hash='#inbox'",
    });
  }
}

/* ------------------------------------------------------------------ */
/*  Render                                                            */
/* ------------------------------------------------------------------ */

function renderPage(container) {
  const hasFiles = state.files.length > 0;
  const totalPages = Math.max(1, Math.ceil(state.total / PAGE_SIZE));

  container.innerHTML = `
    <div class="page-header-row">
      <h1><i class="fas fa-inbox"></i> Tag Inbox</h1>
      <div class="inbox-actions">
        <span class="badge ${state.total > 0 ? "badge-warning" : ""}">${state.total} file${state.total !== 1 ? "s" : ""} need comment update</span>
        <button class="btn btn-sm" id="inbox-refresh" title="Refresh"><i class="fas fa-sync"></i> Refresh</button>
      </div>
    </div>

    ${
      hasFiles
        ? renderTable(
            [
              { label: "File" },
              { label: "Stored Comment" },
              { label: "Target Comment" },
              { label: "Diff" },
              { label: "", style: "width:80px" },
            ],
            state.files.map(renderRow).join(""),
          )
        : renderEmpty({
            icon: "inbox",
            title: "Inbox is empty",
            message:
              "Every file's stored comment already matches its generated target comment. 🎉",
          })
    }

    ${renderPagination(totalPages)}
  `;
}

function renderRow(f) {
  const diff = f.diff || {};
  const diffChips = [];

  for (const tag of diff.tagsAdded || []) {
    diffChips.push(`<span class="tag-chip inbox-add">+ ${escapeHtml(tag)}</span>`);
  }
  for (const tag of diff.tagsRemoved || []) {
    diffChips.push(`<span class="tag-chip inbox-remove">− ${escapeHtml(tag)}</span>`);
  }
  if (diff.phaseChanged || diff.moodChanged || diff.vibeChanged) {
    diffChips.push(`<span class="tag-chip inbox-pmv">PMV</span>`);
  }
  for (const id of diff.sourceIdsAdded || []) {
    diffChips.push(`<span class="tag-chip inbox-add">+ ${escapeHtml(id)}</span>`);
  }
  for (const id of diff.sourceIdsRemoved || []) {
    diffChips.push(`<span class="tag-chip inbox-remove">− ${escapeHtml(id)}</span>`);
  }
  if (diff.rawCommentChanged) {
    diffChips.push(`<span class="tag-chip inbox-pmv">raw comment</span>`);
  }

  const title = f.title || f.filePath.split("/").pop() || `#${f.fileId}`;
  const subtitle = f.artist ? `${f.artist}` : f.filePath;

  return `
    <tr data-file-id="${f.fileId}">
      ${td(`
        <a href="#file-detail?id=${f.fileId}" class="track-title-link">${escapeHtml(title)}</a>
        <div class="text-muted text-xs">${escapeHtml(subtitle)}</div>
      `)}
      ${td(`<code class="inbox-comment">${escapeHtml(f.comment ?? "(empty)")}</code>`)}
      ${td(`<code class="inbox-comment inbox-target">${escapeHtml(f.targetComment)}</code>`)}
      ${td(diffChips.length > 0 ? `<div class="inbox-diff-chips">${diffChips.join("")}</div>` : '<span class="text-muted">—</span>')}
      ${td(`
        <button class="btn btn-sm btn-primary" data-action="write" data-id="${f.fileId}" title="Write target comment to file">
          <i class="fas fa-pen"></i> Write
        </button>
      `)}
    </tr>
  `;
}

function renderPagination(totalPages) {
  if (state.total <= PAGE_SIZE) return "";
  const prevDisabled = state.page <= 0 ? "disabled" : "";
  const nextDisabled = state.page >= totalPages - 1 ? "disabled" : "";
  return `
    <div class="pagination" style="display:flex;align-items:center;gap:0.75rem;justify-content:center;margin-top:1rem;">
      <button class="btn btn-sm" id="inbox-prev" ${prevDisabled}><i class="fas fa-chevron-left"></i> Prev</button>
      <span class="text-muted">Page ${state.page + 1} / ${totalPages}</span>
      <button class="btn btn-sm" id="inbox-next" ${nextDisabled}>Next <i class="fas fa-chevron-right"></i></button>
    </div>
  `;
}

/* ------------------------------------------------------------------ */
/*  Events                                                            */
/* ------------------------------------------------------------------ */

function wireEvents(container, signal) {
  container.addEventListener("click", async (e) => {
    const writeBtn = e.target.closest('[data-action="write"]');
    const refreshBtn = e.target.closest("#inbox-refresh");
    const prevBtn = e.target.closest("#inbox-prev");
    const nextBtn = e.target.closest("#inbox-next");

    if (refreshBtn) {
      window.location.hash = "#inbox";
      return;
    }

    if (prevBtn && state.page > 0) {
      window.location.hash = `#inbox?page=${state.page - 1}`;
      return;
    }

    if (nextBtn && state.files.length === PAGE_SIZE) {
      window.location.hash = `#inbox?page=${state.page + 1}`;
      return;
    }

    if (writeBtn && !state.loading) {
      const fileId = writeBtn.dataset.id;
      state.loading = true;
      writeBtn.disabled = true;
      writeBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i>';
      try {
        await fetchJSON(`/api/files/${fileId}/write-comment`, { method: "POST" }, signal);
        showToast("Comment write queued — check the Tasks page.", "success");
        // Refresh after a short delay so the task manager has registered it.
        setTimeout(() => {
          window.location.hash = `#inbox?page=${state.page}`;
        }, 800);
      } catch (err) {
        showToast(`Failed to queue comment write: ${err.message}`, "error");
        writeBtn.disabled = false;
        writeBtn.innerHTML = '<i class="fas fa-pen"></i> Write';
      } finally {
        state.loading = false;
      }
    }
  });
}
