/**
 * deemix-queue.js — Deemix download queue page.
 */

import { fetchJSON } from "../shared/api.js";
import {
  renderLoading,
  renderErrorBlock,
  renderTable,
  td,
} from "../shared/components.js";
import {
  renderSearchInput,
  renderFilterGroup,
  wireSearchFilter,
} from "../shared/search-filter.js";

const PAGE_SIZE = 15;

const STATUS_OPTIONS = [
  { value: "all", label: "All Statuses" },
  { value: "queued", label: "Queued" },
  { value: "downloading", label: "Downloading" },
  { value: "completed", label: "Completed" },
  { value: "failed", label: "Failed" },
];

const TABLE_HEADERS = [
  { label: "Status", style: "width:8%" },
  { label: "Title", style: "width:18%" },
  { label: "Artist", style: "width:14%" },
  { label: "Playlist Name", style: "width:16%" },
  { label: "Progress", style: "width:8%" },
  { label: "Downloaded", style: "width:10%" },
  { label: "Detail", style: "width:10%" },
  { label: "Created", style: "width:8%" },
  { label: "Updated", style: "width:8%" },
  { label: "Actions", style: "width:10%" },
];

function escapeHtml(str) {
  if (typeof str !== "string") return str;
  return str
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

function formatTimestamp(ts) {
  if (!ts) return '<span class="text-muted">—</span>';
  const d = new Date(ts * 1000);
  return `<span class="font-mono text-sm" title="${d.toISOString()}">${d.toLocaleDateString()} ${d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</span>`;
}

function statusBadge(status) {
  const styles = {
    queued: {
      bg: "rgba(245,158,11,0.1)",
      color: "var(--yellow)",
      icon: "fa-solid fa-clock",
    },
    downloading: {
      bg: "rgba(59,130,246,0.1)",
      color: "var(--blue, #3b82f6)",
      icon: "fa-solid fa-spinner fa-spin",
    },
    completed: {
      bg: "rgba(34,197,94,0.1)",
      color: "var(--green)",
      icon: "fa-solid fa-check",
    },
    failed: {
      bg: "rgba(239,68,68,0.1)",
      color: "var(--red)",
      icon: "fa-solid fa-exclamation-triangle",
    },
  };
  const s = styles[status] || styles.queued;
  return `<span class="status-badge" style="background:${s.bg};color:${s.color};white-space:nowrap"><i class="${s.icon}"></i> ${escapeHtml(status)}</span>`;
}

function progressBar(pct) {
  const clamped = Math.min(100, Math.max(0, pct));
  const color =
    clamped >= 100
      ? "var(--green, #22c55e)"
      : clamped > 0
        ? "var(--accent, #6366f1)"
        : "var(--text-muted)";
  return `<div style="display:flex;align-items:center;gap:6px">
    <div style="flex:1;height:6px;background:var(--border);border-radius:3px;overflow:hidden">
      <div style="width:${clamped}%;height:100%;background:${color};border-radius:3px;transition:width 0.3s"></div>
    </div>
    <span class="font-mono text-sm" style="color:var(--text-muted);min-width:32px;text-align:right">${clamped}%</span>
  </div>`;
}

function adaptItem(item) {
  return {
    id: item.id,
    uuid: item.uuid || null,
    spotifyPlaylistUrl: item.spotifyPlaylistUrl || null,
    playlistName: item.playlistName || null,
    status: item.status || "queued",
    trackCountTotal: item.trackCountTotal || 0,
    trackCountDownloaded: item.trackCountDownloaded || 0,
    errorMessage: item.errorMessage || null,
    createdAt: item.createdAt || null,
    updatedAt: item.updatedAt || null,
    title: item.title || null,
    artist: item.artist || null,
    progress: item.progress || 0,
  };
}

function renderToolbar(search, state) {
  return `<div class="toolbar">
    ${renderSearchInput("deemix-queue", search)}
    ${renderFilterGroup("status", STATUS_OPTIONS, state.statusFilter)}
  </div>`;
}

function renderBody(data, state) {
  const items = data.items || [];
  const totalCount = data._total ?? items.length;
  const totalPages = Math.ceil(totalCount / PAGE_SIZE) || 1;
  const pageId = "dq";

  const rowsHtml = items
    .map((item) => {
      const dlInfo =
        item.trackCountTotal > 0
          ? `${item.trackCountDownloaded} / ${item.trackCountTotal}`
          : '<span class="text-muted">—</span>';
      let detailHtml = item.uuid
        ? `<span class="font-mono text-sm" title="${escapeHtml(item.uuid)}">${escapeHtml(item.uuid.slice(0, 24))}…</span>`
        : '<span class="text-muted">—</span>';
      if (item.errorMessage)
        detailHtml += `<div class="text-sm" style="color:var(--red);margin-top:2px" title="${escapeHtml(item.errorMessage)}">${escapeHtml(item.errorMessage.slice(0, 40))}${item.errorMessage.length > 40 ? "…" : ""}</div>`;
      let actionsHtml = "";
      if (item.status === "failed" && item.id)
        actionsHtml = `<button class="btn btn-sm btn-icon" data-act="retry" data-id="${item.id}" title="Retry"><i class="fa-solid fa-rotate"></i></button>`;
      if (item.spotifyPlaylistUrl)
        actionsHtml += `<button class="btn btn-sm btn-icon" data-act="restart" data-id="${item.id || ""}" data-url="${escapeHtml(item.spotifyPlaylistUrl)}" title="Re-download via deemix"><i class="fa-solid fa-arrows-rotate"></i></button>`;
      if (item.id)
        actionsHtml += `<button class="btn btn-sm btn-icon" data-act="delete" data-id="${item.id}" title="Remove"><i class="fa-solid fa-trash"></i></button>`;
      if (!actionsHtml) actionsHtml = '<span class="text-muted">—</span>';
      return `<tr>
      ${td(statusBadge(item.status), { style: "width:8%" })}
      ${td(item.title ? escapeHtml(item.title) : '<span class="text-muted">—</span>', { style: "width:18%" })}
      ${td(item.artist ? escapeHtml(item.artist) : '<span class="text-muted">—</span>', { style: "width:14%" })}
      ${td(item.playlistName ? escapeHtml(item.playlistName) : '<span class="text-muted">—</span>', { style: "width:16%" })}
      ${td(progressBar(item.progress), { style: "width:8%" })}
      ${td(dlInfo, { style: "width:10%;text-align:center;font-family:var(--font-mono);font-size:0.85rem" })}
      ${td(detailHtml, { style: "width:10%" })}
      ${td(formatTimestamp(item.createdAt), { style: "width:8%" })}
      ${td(formatTimestamp(item.updatedAt), { style: "width:8%" })}
      ${td(`<div class="flex items-center gap-1">${actionsHtml}</div>`, { style: "width:10%" })}
    </tr>`;
    })
    .join("");

  const stats = `<div class="stats-row"><div class="stats-group">
    <button class="btn btn-sm btn-icon" id="dq-refresh" title="Refresh"><i class="fa-solid fa-rotate"></i></button>
    <strong>${totalCount.toLocaleString()}</strong> queue item${totalCount !== 1 ? "s" : ""}
  </div></div>`;

  const pagination = `<div class="pagination" id="${pageId}-pagination">
    <button class="pagination-btn" id="${pageId}-prev" disabled><i class="fa-solid fa-chevron-left"></i></button>
    <span class="pagination-info" id="${pageId}-info">Page ${state.page + 1} of ${totalPages}</span>
    <button class="pagination-btn" id="${pageId}-next" ${totalPages <= 1 ? "disabled" : ""}><i class="fa-solid fa-chevron-right"></i></button>
  </div>`;

  return `${stats}\n${renderTable(TABLE_HEADERS, rowsHtml)}\n${pagination}`;
}

function renderEmptyBody() {
  const theadHtml = TABLE_HEADERS.map(
    (h) => `<th${h.style ? ` style="${h.style}"` : ""}>${escapeHtml(h.label)}</th>`,
  ).join("");
  return `<div class="stats-row"><div class="stats-group">
    <button class="btn btn-sm btn-icon" id="dq-refresh" title="Refresh"><i class="fa-solid fa-rotate"></i></button>
    <strong>0</strong> queue items
  </div></div>
  <div class="table-wrap"><table class="data-table"><thead><tr>${theadHtml}</tr></thead>
    <tbody><tr><td colspan="10"><div class="text-center text-muted" style="padding:32px">No queue items found.</div></td></tr></tbody>
  </table></div>`;
}

function buildParams(state) {
  const params = new URLSearchParams();
  if (state.search) params.set("search", state.search);
  if (state.statusFilter !== "all") params.set("status", state.statusFilter);
  return params;
}

function setContent(html) {
  const el = document.getElementById("dq-content");
  if (el) el.innerHTML = html;
}

async function fetchAndRender(container, signal, state) {
  setContent(renderLoading("Loading queue…"));
  try {
    const resp = await fetchJSON(`/api/services/deemix/queue?${buildParams(state)}`, {
      signal,
    });
    if (signal.aborted) return;
    let items = (resp.data || []).map(adaptItem);
    if (state.search) {
      const q = state.search.toLowerCase();
      items = items.filter(
        (i) =>
          (i.title && i.title.toLowerCase().includes(q)) ||
          (i.artist && i.artist.toLowerCase().includes(q)) ||
          (i.playlistName && i.playlistName.toLowerCase().includes(q)) ||
          (i.spotifyPlaylistUrl && i.spotifyPlaylistUrl.toLowerCase().includes(q)),
      );
    }
    if (state.statusFilter && state.statusFilter !== "all")
      items = items.filter((i) => i.status === state.statusFilter);
    const totalCount = items.length;
    const start = state.page * PAGE_SIZE;
    const pagedItems = items.slice(start, start + PAGE_SIZE);
    if (pagedItems.length === 0 && totalCount === 0) {
      setContent(renderEmptyBody());
      wireContentEvents(container, signal, state);
      return;
    }
    setContent(renderBody({ _total: totalCount, items: pagedItems }, state));
    wireContentEvents(container, signal, state);
  } catch (err) {
    if (err.name === "AbortError") return;
    try {
      if (signal.aborted) return;
    } catch {
      return;
    }
    setContent(
      renderErrorBlock({
        title: "Failed to load deemix queue",
        detail: err.message,
        retryFn: "window.location.hash='#deemix-queue'",
      }),
    );
  }
}

function wireContentEvents(container, signal, state) {
  const refreshBtn = container.querySelector("#dq-refresh");
  if (refreshBtn) refreshBtn.onclick = () => fetchAndRender(container, signal, state);
  const prevBtn = container.querySelector("#dq-prev");
  if (prevBtn) {
    prevBtn.disabled = state.page === 0;
    prevBtn.onclick = () => {
      if (state.page > 0) {
        state.page--;
        fetchAndRender(container, signal, state);
      }
    };
  }
  const nextBtn = container.querySelector("#dq-next");
  if (nextBtn) {
    nextBtn.onclick = () => {
      state.page++;
      fetchAndRender(container, signal, state);
    };
  }
  const table = container.querySelector(".data-table");
  if (table) {
    table.addEventListener(
      "click",
      async (e) => {
        const btn = e.target.closest("[data-act]");
        if (!btn) return;
        const act = btn.dataset.act;
        const id = parseInt(btn.dataset.id, 10);
        if (act === "retry") {
          btn.disabled = true;
          btn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i>';
          try {
            await fetchJSON(`/api/services/deemix/queue/${id}/retry`, { method: "POST" });
            await fetchAndRender(container, signal, state);
          } catch (err) {
            showToast(`Retry failed: ${err.message}`, "error");
            btn.disabled = false;
            btn.innerHTML = '<i class="fa-solid fa-rotate"></i>';
          }
        } else if (act === "restart") {
          const localId = parseInt(btn.dataset.id, 10);
          const url = btn.dataset.url;
          if (!url) return;
          btn.disabled = true;
          btn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i>';
          try {
            if (localId) {
              // Has local DB entry — call retryDownload (UUID-based)
              await fetchJSON(`/api/services/deemix/queue/${localId}/retry`, {
                method: "POST",
              });
            } else {
              // Remote-only — re-add the URL
              await fetchJSON("/api/services/deemix/queue", {
                method: "POST",
                body: JSON.stringify({ url }),
              });
            }
            showToast("Re-download triggered", "success");
            await fetchAndRender(container, signal, state);
          } catch (err) {
            showToast(`Re-download failed: ${err.message}`, "error");
            btn.disabled = false;
            btn.innerHTML = '<i class="fa-solid fa-arrows-rotate"></i>';
          }
        } else if (act === "delete") {
          if (!confirm("Remove this item?")) return;
          btn.disabled = true;
          btn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i>';
          try {
            await fetchJSON(`/api/services/deemix/queue/${id}`, { method: "DELETE" });
            await fetchAndRender(container, signal, state);
          } catch (err) {
            showToast(`Delete failed: ${err.message}`, "error");
            btn.disabled = false;
            btn.innerHTML = '<i class="fa-solid fa-trash"></i>';
          }
        }
      },
      { signal },
    );
  }
}

function showToast(message, type) {
  const existing = document.querySelector(".toast-notification");
  if (existing) existing.remove();
  const bg =
    type === "error"
      ? "var(--red, #ef4444)"
      : type === "success"
        ? "var(--green, #22c55e)"
        : "var(--accent, #6366f1)";
  const toast = document.createElement("div");
  toast.className = "toast-notification";
  toast.textContent = message;
  Object.assign(toast.style, {
    position: "fixed",
    bottom: "24px",
    right: "24px",
    background: bg,
    color: "#fff",
    padding: "12px 20px",
    borderRadius: "8px",
    fontSize: "0.9rem",
    zIndex: "9999",
    boxShadow: "0 4px 20px rgba(0,0,0,0.3)",
    transition: "opacity 0.3s ease",
    cursor: "pointer",
  });
  toast.addEventListener("click", () => toast.remove());
  document.body.appendChild(toast);
  setTimeout(() => {
    toast.style.opacity = "0";
    setTimeout(() => toast.remove(), 300);
  }, 4000);
}

export async function init(container, signal, hashParams) {
  const state = {
    page: parseInt(hashParams?.page) || 0,
    search: hashParams?.search || "",
    statusFilter: hashParams?.status || "all",
  };
  container.innerHTML = `${renderToolbar(state.search, state)}\n<div id="dq-content">${renderLoading("Loading queue…")}</div>`;
  const toolbar = container.querySelector(".toolbar");
  if (toolbar)
    wireSearchFilter(toolbar, state, () => fetchAndRender(container, signal, state));
  await fetchAndRender(container, signal, state);
}
