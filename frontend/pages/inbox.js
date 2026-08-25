/**
 * inbox.js — Tag Roundtrip Inbox page.
 *
 * Lists files whose stored comment does NOT match the generated target
 * comment (compared via roundtrip: parse → generate → compare). Formatting
 * differences (tag order, quoting, case) never produce false positives —
 * only real content changes are listed.
 *
 * FULL feature set (see plans/proposed/tag-roundtrip-inbox.md):
 *   - Similar-tag suggestions: every NEW tag (not yet canonical) in an item's
 *     diff is matched against the existing vocabulary (case-insensitive
 *     Levenshtein ≤ 2) and offered as click-to-merge chips.
 *   - Rename: fix the spelling of a new tag inline (typo fix).
 *   - Click-to-Merge: click a suggested existing tag → the typo tag is merged
 *     into it. ALL files carrying the typo are retagged on the next write.
 *   - Dismiss: acknowledge the tag without any mapping effect.
 *   - Staging: resolving only records the decision (GET /api/inbox/mappings).
 *     The canonical (mapped) tag is written on the NEXT comment write —
 *     nothing is auto-applied.
 *
 * API:
 *   GET  /api/inbox?limit=&offset= → { files: [{ fileId, filePath, title,
 *       artist, comment, targetComment, diff, newTags: [{ tag, added,
 *       suggestions: [{tag, distance, count}], mapping }] }], total }
 *   GET  /api/inbox/count     → { count }
 *   GET  /api/inbox/mappings  → { mappings: [{ rawTag, targetTag, action,
 *       status, fileCount }] }
 *   POST /api/inbox/resolve   → { mapping } — { tag, action, targetTag? }
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
  mappings: [],
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
    const [listResp, mappingsResp] = await Promise.all([
      fetchJSON(
        `/api/inbox?limit=${PAGE_SIZE}&offset=${state.page * PAGE_SIZE}`,
        { signal },
      ),
      fetchJSON("/api/inbox/mappings", { signal }).catch(() => ({ data: { mappings: [] } })),
    ]);
    if (signal.aborted) return;
    state.files = listResp.data.files || [];
    state.total = listResp.data.total || 0;
    state.mappings = mappingsResp.data.mappings || [];

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
  const openMappings = state.mappings.filter((m) => m.status === "open");

  container.innerHTML = `
    <div class="page-header-row">
      <h1><i class="fas fa-inbox"></i> Tag Inbox</h1>
      <div class="inbox-actions">
        <span class="badge ${state.total > 0 ? "badge-warning" : ""}">${state.total} file${state.total !== 1 ? "s" : ""} need comment update</span>
        <button class="btn btn-sm" id="inbox-refresh" title="Refresh"><i class="fas fa-sync"></i> Refresh</button>
      </div>
    </div>

    ${renderMappingsStrip(openMappings)}

    ${
      hasFiles
        ? renderTable(
            [
              { label: "File" },
              { label: "Stored Comment" },
              { label: "Target Comment" },
              { label: "Diff & New Tags" },
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

/**
 * Strip of the user's open staging decisions (rename/merge/dismiss), so a
 * resolved typo shows up exactly ONCE in the inbox instead of per file.
 */
function renderMappingsStrip(openMappings) {
  if (!openMappings.length) return "";
  const chips = openMappings
    .map(
      (m) => `
        <span class="tag-chip inbox-mapping" title="${escapeHtml(m.action)} — applies to ${m.fileCount ?? 0} file(s) on next write">
          ${escapeHtml(m.rawTag)} → ${escapeHtml(m.targetTag)}
          <em class="inbox-mapping-action">${escapeHtml(m.action)}</em>
        </span>`,
    )
    .join("");
  return `
    <div class="inbox-mappings-strip">
      <strong class="text-muted text-xs">Active mappings (staged — applied on next write):</strong>
      <div class="inbox-mappings-chips">${chips}</div>
    </div>
  `;
}

function renderDiffChips(diff) {
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
  return diffChips;
}

/**
 * The "new tags" panel for one inbox item: for every tag that is not yet
 * canonically established, offer rename / dismiss / click-to-merge into a
 * similar existing tag. Resolved tags show their mapping instead.
 */
function renderNewTags(f) {
  const newTags = f.newTags || [];
  if (!newTags.length) return "";

  const blocks = newTags.map(renderNewTagBlock).join("");
  return `
    <div class="inbox-newtags">
      <div class="text-muted text-xs" style="margin-top:0.5rem;margin-bottom:0.25rem;">New tags — rename or merge before writing:</div>
      ${blocks}
    </div>
  `;
}

function renderNewTagBlock(nt) {
  const sign = nt.added ? "+" : "−";
  const chipClass = nt.added ? "inbox-add" : "inbox-remove";

  if (nt.mapping) {
    // Already resolved — show the staged decision (typo appears once).
    return `
      <div class="inbox-newtag inbox-newtag-resolved" data-tag="${escapeHtml(nt.tag)}">
        <span class="tag-chip ${chipClass}">${sign} ${escapeHtml(nt.tag)}</span>
        <span class="inbox-mapping-arrow">→</span>
        <span class="tag-chip inbox-mapping-target">${escapeHtml(nt.mapping.targetTag)}</span>
        <em class="inbox-mapping-action">${escapeHtml(nt.mapping.action)}</em>
        <span class="text-muted text-xs">staged · applied on next write</span>
      </div>
    `;
  }

  const suggestions = (nt.suggestions || [])
    .map(
      (s) => `
        <button type="button" class="tag-chip inbox-suggestion"
                data-role="nt-merge" data-tag="${escapeHtml(nt.tag)}" data-target="${escapeHtml(s.tag)}"
                title="Merge '${escapeHtml(nt.tag)}' into existing tag '${escapeHtml(s.tag)}' (distance ${s.distance}, ${s.count} file(s))">
          ${escapeHtml(s.tag)} <em>(${s.distance})</em>
        </button>`,
    )
    .join("");

  return `
    <div class="inbox-newtag" data-tag="${escapeHtml(nt.tag)}">
      <div class="inbox-newtag-row">
        <span class="tag-chip ${chipClass}">${sign} ${escapeHtml(nt.tag)}</span>
        <input type="text" class="input-text inbox-rename-input" data-role="nt-rename-input"
               placeholder="rename…" value="" autocomplete="off">
        <button type="button" class="btn btn-sm" data-role="nt-rename" data-tag="${escapeHtml(nt.tag)}">
          <i class="fas fa-pen"></i> Rename
        </button>
        <button type="button" class="btn btn-sm" data-role="nt-dismiss" data-tag="${escapeHtml(nt.tag)}"
                title="Acknowledge this tag without any mapping effect">
          <i class="fas fa-eye-slash"></i> Dismiss
        </button>
      </div>
      ${
        suggestions.length
          ? `<div class="inbox-suggestions"><span class="text-muted text-xs">merge into:</span> ${suggestions}</div>`
          : `<div class="text-muted text-xs">No similar existing tags found.</div>`
      }
    </div>
  `;
}

function renderRow(f) {
  const diff = f.diff || {};
  const diffChips = renderDiffChips(diff);

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
      ${td(`
        ${diffChips.length > 0 ? `<div class="inbox-diff-chips">${diffChips.join("")}</div>` : '<span class="text-muted">—</span>'}
        ${renderNewTags(f)}
      `)}
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
/*  Resolve actions (rename / merge / dismiss)                        */
/* ------------------------------------------------------------------ */

async function resolveTag(tag, action, targetTag) {
  const body = { tag, action };
  if (targetTag != null) body.targetTag = targetTag;
  const resp = await fetchJSON("/api/inbox/resolve", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  }, _signal);
  return resp.data;
}

async function handleResolve(e, role, tag, targetTag) {
  if (state.loading) return;
  const el = e.target.closest(`[data-role="${role}"]`);
  if (!el) return;

  let effectiveTarget = targetTag;
  if (role === "nt-rename") {
    const block = el.closest(".inbox-newtag");
    const input = block && block.querySelector('[data-role="nt-rename-input"]');
    effectiveTarget = (input && input.value.trim()) || "";
    if (!effectiveTarget) {
      showToast("Enter a new spelling first.", "error");
      return;
    }
  }

  state.loading = true;
  el.disabled = true;
  try {
    const mapping = await resolveTag(tag, role === "nt-merge" ? "merge" : role === "nt-rename" ? "rename" : "dismiss", effectiveTarget);
    const actionLabel = mapping.action;
    showToast(
      `Staged: ${mapping.rawTag} → ${mapping.targetTag} (${actionLabel}). Applied on the next write.`,
      "success",
    );
    // Re-fetch so the staged target + mapping strip update.
    window.location.hash = `#inbox?page=${state.page}`;
  } catch (err) {
    showToast(`Failed to resolve '${tag}': ${err.message}`, "error");
    el.disabled = false;
  } finally {
    state.loading = false;
  }
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
    const mergeBtn = e.target.closest('[data-role="nt-merge"]');
    const renameBtn = e.target.closest('[data-role="nt-rename"]');
    const dismissBtn = e.target.closest('[data-role="nt-dismiss"]');

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

    if (mergeBtn) {
      await handleResolve(e, "nt-merge", mergeBtn.dataset.tag, mergeBtn.dataset.target);
      return;
    }
    if (renameBtn) {
      await handleResolve(e, "nt-rename", renameBtn.dataset.tag);
      return;
    }
    if (dismissBtn) {
      await handleResolve(e, "nt-dismiss", dismissBtn.dataset.tag);
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

  // Enter in a rename input triggers the rename.
  container.addEventListener("keydown", (e) => {
    if (e.key !== "Enter") return;
    const input = e.target.closest('[data-role="nt-rename-input"]');
    if (!input) return;
    const block = input.closest(".inbox-newtag");
    const tag = block && block.dataset.tag;
    if (!tag) return;
    e.preventDefault();
    handleResolve(e, "nt-rename", tag);
  });
}
