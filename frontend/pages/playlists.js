/**
 * playlists.js — All playlists page.
 *
 * Lists playlists from connected streaming services with pagination,
 * filtering by service/search, subscription management, sync actions,
 * Deemix download integration, and tag creation.
 *
 * Toolbar is rendered once and preserved across re-renders to
 * keep the search input stable (no focus loss on reload).
 *
 * URL hash params:
 *   #playlists?search=foo&service=spotify&page=0&sort=name&order=asc
 */

import { fetchJSON } from "../shared/api.js";
import {
  escapeHtml,
  renderLoading,
  renderErrorBlock,
  showToast,
} from "../shared/components.js";
import { renderSearchInput, wireSearchFilter } from "../shared/search-filter.js";
import {
  getPageSize,
  renderPageSizeSelector,
  sortableTh,
  wireSortableHeaders,
  wirePageSizeSelector,
  updateHash,
  parseHash,
} from "../shared/crud.js";
import {
  loadColumnConfig,
  renderColumnConfigTrigger,
  renderColumnHeaders,
  renderColumnCells,
  wireColumnResize,
  wireColumnDragReorder,
  wireConfigTrigger,
} from "../shared/column-config.js";
import { renderActionsPanel } from "../shared/actions-panel.js";

/* ------------------------------------------------------------------ */
/*  Constants                                                          */
/* ------------------------------------------------------------------ */

const SVC = {
  spotify: ["fab fa-spotify", "Spotify"],
  soundcloud: ["fab fa-soundcloud", "SoundCloud"],
  youtube: ["fab fa-youtube", "YouTube"],
  deemix: ["fa-solid fa-download", "Deemix"],
};

const SVC_CLS = {
  spotify: "service-badge spotify",
  soundcloud: "service-badge soundcloud",
  youtube: "service-badge youtube",
  deemix: "service-badge deemix",
};

const SERVICE_OPTIONS = [
  { value: "all", label: "All" },
  { value: "spotify", label: "Spotify" },
  { value: "soundcloud", label: "SoundCloud" },
  { value: "youtube", label: "YouTube" },
  { value: "deemix", label: "Deemix" },
];

const HASH_DEFAULTS = {
  sort: "",
  order: "asc",
  search: "",
  service: "all",
  page: 0,
  untaggedOnly: false,
  mismatchOnly: false,
  selectedServices: [],
  pmvCategories: [],
  pmvAggregate: "",
};

const HASH_SCHEMA = {
  page: { type: "number", default: 0 },
  search: { type: "string", default: "" },
  sort: { type: "string", default: "" },
  order: { type: "string", default: "asc" },
  service: { type: "string", default: "all" },
  untaggedOnly: { type: "boolean", default: false },
  mismatchOnly: { type: "boolean", default: false },
  selectedServices: { type: "array", default: [] },
  pmvCategories: { type: "array", default: [] },
  pmvAggregate: { type: "string", default: "" },
};

/* ------------------------------------------------------------------ */
/*  Column model (for column-config.js)                                 */
/* ------------------------------------------------------------------ */

const PLAYLISTS_COLUMNS = [
  { id: "name", label: "Name", sortable: true, sortKey: "name", defaultWidth: 180 },
  {
    id: "service",
    label: "Service",
    sortable: true,
    sortKey: "service",
    defaultWidth: 80,
  },
  {
    id: "tracks",
    label: "Tracks",
    sortable: true,
    sortKey: "track_count",
    defaultWidth: 80,
  },
  {
    id: "imported",
    label: "Imported",
    sortable: true,
    sortKey: "imported_at",
    defaultWidth: 80,
  },
  {
    id: "updated",
    label: "Updated",
    sortable: true,
    sortKey: "updated_at",
    defaultWidth: 80,
  },
  { id: "tags", label: "Tags", sortable: false, defaultWidth: 140 },
  { id: "deemix", label: "Deemix", sortable: false, defaultWidth: 100 },
  { id: "sync", label: "Sync", sortable: false, defaultWidth: 80 },
  { id: "subscribe", label: "Subscribe", sortable: false, defaultWidth: 80 },
  { id: "view", label: "View", sortable: false, defaultWidth: 60 },
  { id: "actions", label: "Actions", sortable: false, defaultWidth: 120 },
];

/* ------------------------------------------------------------------ */
/*  Cell helpers                                                       */
/* ------------------------------------------------------------------ */

function sBadge(s) {
  return `<span class="${SVC_CLS[s] || "service-badge"}"><i class="${(SVC[s] || ["", ""])[0]}"></i> ${(SVC[s] || [s, s])[1]}</span>`;
}

function tagCell(t) {
  if (!t)
    return `<span class="status-badge" style="background:rgba(245,158,11,0.1);color:var(--yellow)"><i class="fas fa-exclamation-triangle"></i> No tag</span>`;
  return `<span class="tag-badge font-mono" style="background:var(--accent-bg);color:var(--accent);border:1px solid var(--border)"><i class="fas fa-check-circle" style="color:var(--green)"></i> ${escapeHtml(t)}</span>`;
}

function syncCell(v) {
  return v === null
    ? `<em style="color:var(--text-muted)">Never</em>`
    : `<span style="color:var(--text-muted)">${escapeHtml(v)}</span>`;
}

/** Show a subscription bell icon (green = subscribed, muted = not subscribed) */
function subCell(sub) {
  if (sub) {
    return `<span class="status-badge" style="background:rgba(34,197,94,0.1);color:var(--green)" title="Subscribed — polls every ${sub.pollIntervalSecs}s"><i class="fas fa-bell"></i></span>`;
  }
  return `<span style="color:var(--text-muted)" title="Not subscribed"><i class="far fa-bell"></i></span>`;
}

function deemixCell(r) {
  const status = r.deemixStatus;
  const restartBtn = status
    ? `<button class="btn btn-sm btn-icon" data-act="deemix-restart" data-deemix-id="${r.deemixId || ""}" data-name="${escapeHtml(r.name)}" data-id="${r.id}" title="Re-download via deemix"><i class="fa-solid fa-arrows-rotate"></i></button>`
    : "";
  const addBtn = `<button class="btn btn-sm btn-icon" data-act="deemix-add" data-id="${r.id}" data-name="${escapeHtml(r.name)}" title="Add to Deemix download queue"><i class="fa-solid fa-plus"></i></button>`;

  if (!status) return addBtn;
  if (status === "queued") {
    return `<span class="status-badge" style="background:rgba(245,158,11,0.1);color:var(--yellow)"><i class="fa-solid fa-clock"></i> Queued</span> ${restartBtn}`;
  }
  if (status === "downloading") {
    return `<span class="status-badge" style="background:rgba(59,130,246,0.1);color:var(--blue, #3b82f6)"><i class="fa-solid fa-spinner fa-spin"></i> DL</span> ${restartBtn}`;
  }
  if (status === "completed") {
    return `<span class="status-badge" style="background:rgba(34,197,94,0.1);color:var(--green)"><i class="fa-solid fa-check"></i></span> ${restartBtn}`;
  }
  if (status === "failed" && r.deemixId) {
    return `<button class="btn btn-sm btn-icon" data-act="deemix-retry" data-deemix-id="${r.deemixId}" data-id="${r.id}" data-name="${escapeHtml(r.name)}" title="Retry download"><i class="fa-solid fa-rotate"></i></button> ${restartBtn}`;
  }
  return `<span class="status-badge" style="background:rgba(245,158,11,0.1);color:var(--yellow)"><i class="fa-solid fa-clock"></i> ${escapeHtml(status)}</span> ${restartBtn}`;
}

function viewTracksCell(r) {
  return `<a href="#tracks?playlistId=${r.id}&playlistName=${encodeURIComponent(r.name)}" class="btn btn-sm btn-icon" title="View tracks"><i class="fa-solid fa-list-music"></i></a>`;
}

function actions(r) {
  let b = "";
  if (r.tag)
    b += `<button class="btn btn-sm btn-purple" data-act="edit-tag" data-id="${r.id}" title="Edit tag"><i class="fas fa-pencil-alt"></i></button> `;
  else
    b += `<button class="btn btn-sm btn-green" data-act="create-tag" data-id="${r.id}" title="Create tag from playlist name"><i class="fas fa-tag"></i></button> `;

  if (r.sub) {
    b += `<button class="btn btn-sm btn-red" data-act="unsubscribe" data-sub-id="${r.sub.id}" data-id="${r.id}" title="Unsubscribe"><i class="fas fa-bell-slash"></i></button> `;
  } else {
    b += `<button class="btn btn-sm" data-act="subscribe" data-id="${r.id}" data-service="${r.svc}" data-playlist-id="${r.playlistId}" title="Subscribe (poll for changes)"><i class="fas fa-bell"></i></button> `;
  }

  return (
    b +
    `<button class="btn btn-sm" data-act="sync" data-id="${r.id}" data-service="${r.svc}" data-playlist-id="${r.playlistId}" title="Sync now"><i class="fas fa-sync"></i></button>`
  );
}

/* ------------------------------------------------------------------ */
/*  Cell renderers (for column-config.js)                               */
/* ------------------------------------------------------------------ */

const PLAYLISTS_CELL_RENDERERS = {
  name: (r) => escapeHtml(r.name),
  service: (r) => sBadge(r.svc),
  tracks: (r) => {
    const mismatch = r.l !== r.r;
    return `<span class="${mismatch ? "diff-badge" : "font-mono"}">${r.l} / ${r.r}</span>`;
  },
  imported: (r) =>
    r.importedAt
      ? `<span class="font-mono text-xs">${new Date(r.importedAt * 1000).toLocaleDateString()}</span>`
      : '<span class="text-muted">\u2014</span>',
  updated: (r) =>
    r.updatedAt
      ? `<span class="font-mono text-xs">${new Date(r.updatedAt * 1000).toLocaleDateString()}</span>`
      : '<span class="text-muted">\u2014</span>',
  tags: (r) => tagCell(r.tag),
  deemix: (r) => deemixCell(r),
  sync: (r) => syncCell(r.sync),
  subscribe: (r) => subCell(r.sub),
  view: (r) => viewTracksCell(r),
  actions: (r) => actions(r),
};

/**
 * Render the full toolbar HTML (called once on init).
 */
function renderToolbar(state) {
  return `<div class="filter-panel" id="playlists-filter-panel">
    <div class="filter-panel-header">
      ${renderSearchInput("playlists", state.search)}
      <button class="btn btn-primary" id="playlists-create-tag"><i class="fas fa-tag"></i> Create Tags</button>
      <button class="filter-panel-toggle" id="playlists-filter-toggle" title="Toggle filters">
        <i class="fas fa-chevron-up chevron"></i>
      </button>
    </div>
    <div class="filter-panel-body">
      <div class="filter-panel-scroll" style="display:grid;grid-template-columns:1fr 1fr;gap:var(--space-2) var(--space-4);">
        <div>
          <div class="filter-section-header" style="margin-top:0"><i class="fas fa-music"></i> Playlist Info</div>
        </div>
        <div>
          <div class="filter-section-header" style="margin-top:0"><i class="fas fa-tag"></i> Classification</div>
          <div class="filter-row">
            <span class="filter-row-label toggleable" data-filter="service">Service</span>
            <div class="filter-group service-filter-group">
              <button class="filter-btn${(state.selectedServices || []).includes("spotify") ? " active" : ""}" data-value="spotify"><i class="fab fa-spotify"></i></button>
              <button class="filter-btn${(state.selectedServices || []).includes("soundcloud") ? " active" : ""}" data-value="soundcloud"><i class="fab fa-soundcloud"></i></button>
              <button class="filter-btn${(state.selectedServices || []).includes("youtube") ? " active" : ""}" data-value="youtube"><i class="fab fa-youtube"></i></button>
              <button class="filter-btn${(state.selectedServices || []).includes("deemix") ? " active" : ""}" data-value="deemix"><i class="fas fa-download"></i></button>
            </div>
          </div>
          <div class="filter-row">
            <span class="filter-row-label toggleable" data-filter="pmv">PMV</span>
            <div class="filter-group" id="playlists-pmv-cat-btns" style="flex-wrap:wrap">
              <button class="filter-btn${(state.pmvCategories || []).includes("p") ? " active" : ""}" data-value="p" title="Has Phase tags">P</button>
              <button class="filter-btn${(state.pmvCategories || []).includes("m") ? " active" : ""}" data-value="m" title="Has Mood tags">M</button>
              <button class="filter-btn${(state.pmvCategories || []).includes("v") ? " active" : ""}" data-value="v" title="Has Vibe tags">V</button>
            </div>
            <span class="pmv-sep">|</span>
            <div class="filter-group" id="playlists-pmv-agg-btns" style="flex-wrap:wrap">
              <button class="filter-btn${state.pmvAggregate === "full" ? " active" : ""}" data-value="full" title="Has all three categories">Full</button>
              <button class="filter-btn${state.pmvAggregate === "partial" ? " active" : ""}" data-value="partial" title="Has at least one category">Partial</button>
              <button class="filter-btn${state.pmvAggregate === "none" ? " active" : ""}" data-value="none" title="Has no PMV categories">None</button>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>`;
}

/* ------------------------------------------------------------------ */
/*  Content render (re-rendered on each fetch)                         */
/* ------------------------------------------------------------------ */

function renderBody(data, state) {
  const { playlists } = data;
  const totalCount = data._total ?? playlists.length;
  const totalPages = Math.max(1, Math.ceil(totalCount / state.pageSize));
  const untaggedTotal = playlists.filter((p) => !p.tag).length;
  const pageId = "playlists";

  const config = loadColumnConfig("playlists", PLAYLISTS_COLUMNS);
  const theadHtml = renderColumnHeaders(config, PLAYLISTS_COLUMNS, state, sortableTh);

  const rowsHtml = playlists
    .map((r) => {
      const mismatch = r.l !== r.r;
      return `<tr class="${mismatch ? "row-mismatch" : ""}" ${mismatch ? 'title="Local vs Remote differ"' : ""}>
        ${renderColumnCells(config, PLAYLISTS_COLUMNS, PLAYLISTS_CELL_RENDERERS, r)}
      </tr>`;
    })
    .join("");

  const visibleColCount = config.filter((c) => c.visible).length;

  const stats = `<div class="stats-row">
    <div class="stats-group">
      <button class="btn btn-sm btn-icon" id="playlists-refresh" title="Refresh"><i class="fa-solid fa-rotate"></i></button>
      <strong>${totalCount.toLocaleString()}</strong> playlists
      <span style="margin:0 6px;color:var(--text-subtle);">\u00b7</span>
      <strong>${untaggedTotal}</strong> without tags
      ${renderPageSizeSelector(state.pageSize)}
    ${renderColumnConfigTrigger()}
    ${
      state.layoutMode
        ? '<button class="btn btn-sm btn-primary" id="playlists-layout-btn" style="margin-left:8px"><i class="fas fa-check"></i> Done</button>'
        : '<button class="btn btn-sm" id="playlists-layout-btn" style="margin-left:8px"><i class="fas fa-arrows-alt"></i> Modify Column Layout</button>'
    }
  </div>
</div>`;

  const tableHtml = `<div class="table-wrap"><table class="data-table" id="pl-tbl">
    <thead><tr>${theadHtml}</tr></thead>
    <tbody>${rowsHtml}</tbody>
  </table></div>`;

  const pagination = `<div class="pagination" id="${pageId}-pagination">
    <button class="pagination-btn" id="${pageId}-prev" ${state.page === 0 ? "disabled" : ""}><i class="fa-solid fa-chevron-left"></i></button>
    <span class="pagination-info" id="${pageId}-info">Page ${state.page + 1} of ${totalPages}</span>
    <button class="pagination-btn" id="${pageId}-next" ${state.page >= totalPages - 1 ? "disabled" : ""}><i class="fa-solid fa-chevron-right"></i></button>
  </div>`;

  return `${stats}\n${tableHtml}\n${pagination}`;
}

function renderEmptyBody(search) {
  const config = loadColumnConfig("playlists", PLAYLISTS_COLUMNS);
  const theadHtml = renderColumnHeaders(
    config,
    PLAYLISTS_COLUMNS,
    { sort: "", order: "asc" },
    sortableTh,
  );
  const visibleColCount = config.filter((c) => c.visible).length;

  return `
    <div class="stats-row">
      <div class="stats-group">
        <button class="btn btn-sm btn-icon" id="playlists-refresh" title="Refresh"><i class="fa-solid fa-rotate"></i></button>
        <strong>0</strong> playlists
        ${renderColumnConfigTrigger()}
      </div>
    </div>
    <div class="table-wrap"><table class="data-table">
      <thead><tr>${theadHtml}</tr></thead>
      <tbody><tr><td colspan="${visibleColCount}"><div class="text-center text-muted" style="padding:32px">No playlists found. Import playlists from your connected services to get started.</div></td></tr></tbody>
    </table></div>`;
}

/* ------------------------------------------------------------------ */
/*  Build params                                                       */
/* ------------------------------------------------------------------ */

function buildParams(state) {
  const params = new URLSearchParams();
  params.set("limit", String(state.pageSize));
  params.set("offset", String(state.page * state.pageSize));
  if (state.sort) params.set("sort", state.sort);
  if (state.order) params.set("order", state.order);
  if (state.service && state.service !== "all") params.set("service", state.service);
  if (state.search) params.set("search", state.search);
  if (state.untaggedOnly) params.set("untagged", "true");
  if (state.mismatchOnly) params.set("mismatch", "true");
  return params;
}

/* ------------------------------------------------------------------ */
/*  Client-side filter helpers                                         */
/* ------------------------------------------------------------------ */

/**
 * Extract PMV categories present in a playlist's comment bracket.
 * Returns { p: bool, m: bool, v: bool }.
 * Playlists currently don't have comment data, so this always returns
 * all-false until the backend provides comment/PMV info.
 */
function pmvFromComment(comment) {
  if (!comment) return { p: false, m: false, v: false };
  const m = comment.match(/^\[([PMV_]+)\]/);
  if (!m) return { p: false, m: false, v: false };
  return {
    p: m[1].includes("P"),
    m: m[1].includes("M"),
    v: m[1].includes("V"),
  };
}

/**
 * Apply client-side filters to a playlist list.
 * Filters: service (multi-select OR), PMV categories/aggregate.
 */
function applyClientFilters(playlists, state) {
  let result = playlists;

  // Service filter (multi-select OR)
  if (state.selectedServices && state.selectedServices.length > 0) {
    result = result.filter((p) => state.selectedServices.includes(p.svc));
  }

  // PMV filter — reads comment bracket if available
  if (state.pmvCategories && state.pmvCategories.length > 0) {
    result = result.filter((p) => {
      const pmv = pmvFromComment(p.comment || "");
      return state.pmvCategories.some((c) => pmv[c]);
    });
  } else if (state.pmvAggregate) {
    result = result.filter((p) => {
      const pmv = pmvFromComment(p.comment || "");
      const hasAny = pmv.p || pmv.m || pmv.v;
      const hasAll = pmv.p && pmv.m && pmv.v;
      switch (state.pmvAggregate) {
        case "full":
          return hasAll;
        case "partial":
          return hasAny;
        case "none":
          return !hasAny;
        default:
          return true;
      }
    });
  }

  return result;
}

/* ------------------------------------------------------------------ */
/*  Fetch + Render cycle                                               */
/* ------------------------------------------------------------------ */

/**
 * Replace the content area (#playlists-content) with the given HTML.
 */
function setContent(html) {
  const el = document.getElementById("playlists-content");
  if (el) el.innerHTML = html;
}

/**
 * Fetch /api/playlists, /api/tags, /api/playlists/subscriptions in parallel,
 * adapt the data, then render.
 */
async function fetchAndRender(container, signal, state) {
  setContent(renderLoading("Loading playlists…"));

  try {
    // Build server-side params, then remove pagination — we paginate
    // client-side after applying client-side filters (service, PMV).
    const params = buildParams(state);
    const fetchParams = new URLSearchParams(params);
    fetchParams.delete("limit");
    fetchParams.delete("offset");

    const [plResp, tagsResp, subsResp] = await Promise.all([
      fetchJSON(`/api/playlists?${fetchParams}`, { signal }),
      fetchJSON("/api/tags", { signal }),
      fetchJSON("/api/playlists/subscriptions", { signal }),
    ]);
    if (signal.aborted) return;

    // Build tag lookup: tag name (lowercase) -> tag name
    const tagLookup = {};
    for (const t of tagsResp.data || []) {
      tagLookup[t.name.toLowerCase()] = t.name;
    }

    // Build subscription lookup: key = "service:playlist_id" -> subscription object
    const subLookup = {};
    const subscriptions = subsResp.data?.subscriptions || [];
    for (const s of subscriptions) {
      subLookup[`${s.service}:${s.playlist_id}`] = s;
    }

    let rawPlaylists = plResp.data.playlists || plResp.data || [];

    let adapted = rawPlaylists.map((p) => {
      const key = `${p.service}:${p.playlistId}`;
      return {
        id: p.id,
        name: p.name,
        svc: p.service,
        playlistId: p.playlistId,
        sub: subLookup[key] || null,
        l: p.localTrackCount ?? 0,
        r: p.remoteTrackCount ?? 0,
        sync: null,
        tag: tagLookup[p.name.toLowerCase()] || null,
        deemixStatus: p.deemixStatus || null,
        deemixId: p.deemixId || null,
        importedAt: p.importedAt || p.imported_at || null,
        updatedAt: p.updatedAt || p.updated_at || null,
      };
    });

    // Apply client-side filters (service multi-select, PMV)
    adapted = applyClientFilters(adapted, state);

    // Client-side pagination
    const total = adapted.length;
    const paged = adapted.slice(
      state.page * state.pageSize,
      (state.page + 1) * state.pageSize,
    );

    const data = {
      _total: total,
      playlists: paged,
    };

    // Empty state (no playlists in DB at all)
    if (paged.length === 0 && total === 0) {
      setContent(renderEmptyBody(state.search));
      wireContentEvents(container, signal, state);
      return;
    }

    setContent(renderBody(data, state));
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
        title: "Failed to load playlists",
        detail: err.message,
        retryFn: "window.location.hash='#playlists'",
      }),
    );
  }
}

/* ------------------------------------------------------------------ */
/*  Toolbar event wiring (called once on init)                         */
/* ------------------------------------------------------------------ */

function wireToolbarEvents(container, signal, state) {
  const filterPanel = container.querySelector("#playlists-filter-panel");

  // Unified search + filter wiring (debounced)
  if (filterPanel) {
    wireSearchFilter(filterPanel, state, () => {
      updateHash("playlists", state, HASH_DEFAULTS);
      fetchAndRender(container, signal, state);
    });
  }

  // Multi-select service filter
  const svcGroup = container.querySelector(".service-filter-group");

  function syncServiceFilterUI() {
    if (!svcGroup) return;
    svcGroup.querySelectorAll(".filter-btn").forEach((btn) => {
      btn.classList.toggle("active", state.selectedServices.includes(btn.dataset.value));
    });
  }

  if (svcGroup) {
    svcGroup.addEventListener(
      "click",
      (e) => {
        const btn = e.target.closest(".filter-btn");
        if (!btn) return;
        const v = btn.dataset.value;
        const i = state.selectedServices.indexOf(v);
        if (i >= 0) state.selectedServices.splice(i, 1);
        else state.selectedServices.push(v);
        state.page = 0;
        syncServiceFilterUI();
        updateHash("playlists", state, HASH_DEFAULTS);
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  }

  // PMV category buttons (multi-select: P, M, V) + aggregate (single-select)
  const pmvCatEl = container.querySelector("#playlists-pmv-cat-btns");
  const pmvAggEl = container.querySelector("#playlists-pmv-agg-btns");

  function syncPmvFilterUI() {
    if (pmvCatEl) {
      pmvCatEl.querySelectorAll(".filter-btn").forEach((btn) => {
        btn.classList.toggle("active", state.pmvCategories.includes(btn.dataset.value));
      });
    }
    if (pmvAggEl) {
      pmvAggEl.querySelectorAll(".filter-btn").forEach((btn) => {
        btn.classList.toggle("active", btn.dataset.value === state.pmvAggregate);
      });
    }
  }

  if (pmvCatEl) {
    pmvCatEl.addEventListener(
      "click",
      (e) => {
        const btn = e.target.closest(".filter-btn");
        if (!btn) return;
        const v = btn.dataset.value;
        const i = state.pmvCategories.indexOf(v);
        if (i >= 0) state.pmvCategories.splice(i, 1);
        else {
          state.pmvAggregate = "";
          state.pmvCategories.push(v);
        }
        state.page = 0;
        syncPmvFilterUI();
        updateHash("playlists", state, HASH_DEFAULTS);
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  }

  // PMV aggregate buttons (single-select: Full, Partial, None)
  if (pmvAggEl) {
    pmvAggEl.addEventListener(
      "click",
      (e) => {
        const btn = e.target.closest(".filter-btn");
        if (!btn) return;
        const v = btn.dataset.value;
        if (state.pmvAggregate === v) state.pmvAggregate = "";
        else {
          state.pmvCategories = [];
          state.pmvAggregate = v;
        }
        state.page = 0;
        syncPmvFilterUI();
        updateHash("playlists", state, HASH_DEFAULTS);
        fetchAndRender(container, signal, state);
      },
      { signal },
    );
  }

  // Generic toggle for data-filter labels
  const labels = filterPanel.querySelectorAll("[data-filter]");
  for (const label of labels) {
    const updateUI = () => {
      const key = label.dataset.filter + "Enabled";
      const active = state[key] !== false;
      label.classList.toggle("active", active);
      label.classList.toggle("off", !active);
      const row = label.closest(".filter-row");
      if (row) {
        const inputs = row.querySelectorAll("select, input, button, .filter-group");
        for (const el of inputs) el.classList.toggle("filter-disabled", !active);
      }
    };
    label.addEventListener("click", () => {
      const key = label.dataset.filter + "Enabled";
      if (state[key] === false) state[key] = true;
      else state[key] = false;
      state.page = 0;
      updateUI();
      updateHash("playlists", state, HASH_DEFAULTS);
      fetchAndRender(container, signal, state);
    });
    updateUI();
  }

  filterPanel.addEventListener("click", (e) => {
    const row = e.target.closest(".filter-row");
    if (!row) return;
    const label = row.querySelector("[data-filter]");
    if (!label) return;
    const key = label.dataset.filter + "Enabled";
    if (state[key] !== false) return;
    if (e.target.closest("[data-filter]")) return;
    state[key] = true;
    label.classList.add("active");
    label.classList.remove("off");
    const inputs = row.querySelectorAll("select, input, button, .filter-group");
    for (const el of inputs) el.classList.remove("filter-disabled");
    state.page = 0;
    updateHash("playlists", state, HASH_DEFAULTS);
    fetchAndRender(container, signal, state);
  });
}

/* ------------------------------------------------------------------ */
/*  Event wiring (re-wired after each content render)                  */
/* ------------------------------------------------------------------ */

function wireContentEvents(container, signal, state) {
  // Refresh button
  const refreshBtn = container.querySelector("#playlists-refresh");
  if (refreshBtn) {
    refreshBtn.onclick = () => {
      updateHash("playlists", state, HASH_DEFAULTS);
      fetchAndRender(container, signal, state);
    };
  }

  // Sortable headers
  const tbl = container.querySelector("#pl-tbl");
  if (tbl) {
    wireSortableHeaders(tbl, state, () => {
      updateHash("playlists", state, HASH_DEFAULTS);
      fetchAndRender(container, signal, state);
    });
  }

  // Page size selector
  wirePageSizeSelector(container, state, () => {
    updateHash("playlists", state, HASH_DEFAULTS);
    fetchAndRender(container, signal, state);
  });

  // Pagination: Previous
  const prevBtn = container.querySelector("#playlists-prev");
  if (prevBtn) {
    prevBtn.onclick = () => {
      if (state.page > 0) {
        state.page--;
        updateHash("playlists", state, HASH_DEFAULTS);
        fetchAndRender(container, signal, state);
      }
    };
  }

  // Pagination: Next
  const nextBtn = container.querySelector("#playlists-next");
  if (nextBtn) {
    nextBtn.onclick = () => {
      state.page++;
      updateHash("playlists", state, HASH_DEFAULTS);
      fetchAndRender(container, signal, state);
    };
  }

  // Action buttons (delegated on table)
  if (tbl) {
    tbl.addEventListener(
      "click",
      async (e) => {
        const b = e.target.closest("[data-act]");
        if (!b) return;

        const act = b.dataset.act;
        const id = parseInt(b.dataset.id, 10);

        // Build playlist info from the button's data attributes
        // (playlists array isn't in closure anymore — data-attrs are always current)
        const pl = {
          id,
          name: b.dataset.name || "",
          svc: b.dataset.service || "",
          playlistId: b.dataset.playlistId || "",
        };

        if (act === "create-tag") {
          try {
            const catResp = await fetchJSON("/api/tag-categories");
            const defaultCat = catResp.data.find((c) => c.isDefault) || catResp.data[0];
            if (!defaultCat) {
              showToast("No tag category found", "error");
              return;
            }
            await fetchJSON("/api/tags", {
              method: "POST",
              body: JSON.stringify({ name: pl.name, categoryId: defaultCat.id }),
            });
            showToast(`Tag "${pl.name}" created`, "success");
            updateHash("playlists", state, HASH_DEFAULTS);
            fetchAndRender(container, signal, state);
          } catch (err) {
            showToast(`Failed to create tag: ${err.message}`, "error");
          }
        } else if (act === "edit-tag") {
          showToast("Edit tag: navigate to Tags page", "info");
        } else if (act === "subscribe") {
          const svc = b.dataset.service;
          const plId = b.dataset.playlistId;
          if (!plId) {
            showToast("No playlist ID", "error");
            return;
          }
          b.disabled = true;
          b.innerHTML = '<i class="fas fa-spinner fa-spin"></i>';
          try {
            await fetchJSON("/api/playlists/subscriptions", {
              method: "POST",
              body: JSON.stringify({ service: svc, playlistId: plId }),
            });
            showToast("Subscribed", "success");
            updateHash("playlists", state, HASH_DEFAULTS);
            fetchAndRender(container, signal, state);
          } catch (err) {
            showToast(`Subscribe failed: ${err.message}`, "error");
            b.disabled = false;
            b.innerHTML = '<i class="fas fa-bell"></i>';
          }
        } else if (act === "unsubscribe") {
          const subId = parseInt(b.dataset.subId, 10);
          if (!subId) return;
          b.disabled = true;
          b.innerHTML = '<i class="fas fa-spinner fa-spin"></i>';
          try {
            await fetchJSON(`/api/playlists/subscriptions/${subId}`, {
              method: "DELETE",
            });
            showToast("Unsubscribed", "success");
            updateHash("playlists", state, HASH_DEFAULTS);
            fetchAndRender(container, signal, state);
          } catch (err) {
            showToast(`Unsubscribe failed: ${err.message}`, "error");
            b.disabled = false;
            b.innerHTML = '<i class="fas fa-bell-slash"></i>';
          }
        } else if (act === "sync") {
          const svc = b.dataset.service;
          const plId = b.dataset.playlistId;
          if (!plId) {
            showToast("No playlist ID available for sync", "error");
            return;
          }
          b.disabled = true;
          b.innerHTML = '<i class="fas fa-spinner fa-spin"></i>';
          try {
            const svcEndpoint =
              svc === "spotify"
                ? `/api/services/spotify/sync/playlists/${plId}/tracks`
                : `/api/services/${svc}/sync`;
            await fetchJSON(svcEndpoint, { method: "POST" });
            showToast("Sync started", "success");
            setTimeout(() => {
              updateHash("playlists", state, HASH_DEFAULTS);
              fetchAndRender(container, signal, state);
            }, 2000);
          } catch (err) {
            showToast(`Sync failed: ${err.message}`, "error");
            b.disabled = false;
            b.innerHTML = '<i class="fas fa-sync"></i>';
          }
        } else if (act === "deemix-add") {
          const url = `https://open.spotify.com/playlist/${b.dataset.playlistId}`;
          const name = b.dataset.name;
          b.disabled = true;
          b.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i>';
          try {
            await fetchJSON("/api/services/deemix/queue", {
              method: "POST",
              body: JSON.stringify({ url }),
            });
            showToast(`Added "${name}" to Deemix download queue`, "success");
            setTimeout(() => {
              updateHash("playlists", state, HASH_DEFAULTS);
              fetchAndRender(container, signal, state);
            }, 1500);
          } catch (err) {
            showToast(`Failed to add to Deemix queue: ${err.message}`, "error");
            b.disabled = false;
            b.innerHTML = '<i class="fa-solid fa-plus"></i>';
          }
        } else if (act === "deemix-restart") {
          const deemixId = b.dataset.deemixId ? parseInt(b.dataset.deemixId, 10) : null;
          const name = b.dataset.name;
          b.disabled = true;
          b.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i>';
          try {
            if (deemixId) {
              await fetchJSON(`/api/services/deemix/queue/${deemixId}/retry`, {
                method: "POST",
              });
            } else {
              const url = `https://open.spotify.com/playlist/${b.dataset.playlistId}`;
              await fetchJSON("/api/services/deemix/queue", {
                method: "POST",
                body: JSON.stringify({ url }),
              });
            }
            showToast(`Re-download triggered for "${name}"`, "success");
            setTimeout(() => {
              updateHash("playlists", state, HASH_DEFAULTS);
              fetchAndRender(container, signal, state);
            }, 1500);
          } catch (err) {
            showToast(`Re-download failed: ${err.message}`, "error");
            b.disabled = false;
            b.innerHTML = '<i class="fa-solid fa-arrows-rotate"></i>';
          }
        } else if (act === "deemix-retry") {
          const deemixId = parseInt(b.dataset.deemixId, 10);
          const name = b.dataset.name;
          b.disabled = true;
          b.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i>';
          try {
            await fetchJSON(`/api/services/deemix/queue/${deemixId}/retry`, {
              method: "POST",
            });
            showToast(`Retrying download for "${name}"`, "success");
            setTimeout(() => {
              updateHash("playlists", state, HASH_DEFAULTS);
              fetchAndRender(container, signal, state);
            }, 1500);
          } catch (err) {
            showToast(`Retry failed: ${err.message}`, "error");
            b.disabled = false;
            b.innerHTML = '<i class="fa-solid fa-rotate"></i>';
          }
        }
      },
      { signal },
    );
  }

  // Column config: resize, reorder, visibility modal
  const colConfig = loadColumnConfig("playlists", PLAYLISTS_COLUMNS);
  if (state.layoutMode) {
    wireColumnResize(container, "playlists", PLAYLISTS_COLUMNS, colConfig);
    wireColumnDragReorder(container, "playlists", PLAYLISTS_COLUMNS, colConfig, () => {
      updateHash("playlists", state, HASH_DEFAULTS);
      fetchAndRender(container, signal, state);
    });
  }
  wireConfigTrigger(container, "playlists", PLAYLISTS_COLUMNS, colConfig, () => {
    updateHash("playlists", state, HASH_DEFAULTS);
    fetchAndRender(container, signal, state);
  });

  // Layout mode toggle
  const layoutBtn = container.querySelector("#playlists-layout-btn");
  if (layoutBtn) {
    layoutBtn.onclick = () => {
      state.layoutMode = !state.layoutMode;
      document.body.classList.toggle("layout-mode", state.layoutMode);
      updateHash("playlists", state, HASH_DEFAULTS);
      fetchAndRender(container, signal, state);
    };
  }
}

/* ------------------------------------------------------------------ */
/*  Initialisation                                                     */
/* ------------------------------------------------------------------ */

/**
 * Initialise the playlists page.
 *
 * @param {HTMLElement} container — the #main-content element
 * @param {AbortSignal} signal    — abort signal for ongoing requests
 * @param {object}       hashParams — parsed hash params from app.js
 */
export async function init(container, signal, hashParams) {
  // Parse hash params into initial state
  const parsed = parseHash(hashParams || {}, HASH_SCHEMA);
  const state = {
    page: parsed.page,
    pageSize: getPageSize(),
    search: parsed.search,
    sort: parsed.sort,
    order: parsed.order,
    service: parsed.service,
    untaggedOnly: parsed.untaggedOnly,
    mismatchOnly: parsed.mismatchOnly,
    selectedServices: parsed.selectedServices || [],
    pmvCategories: parsed.pmvCategories || [],
    pmvAggregate: parsed.pmvAggregate || "",
    serviceEnabled: true,
    pmvEnabled: true,
    layoutMode: false,
  };

  // Reset layout mode on page entry
  document.body.classList.remove("layout-mode");

  // Render stable toolbar + actions panel + content wrapper ONCE
  container.innerHTML = `
    <div style="display:flex;flex-direction:column;gap:var(--space-4);">
      <div style="display:flex;gap:var(--space-4);align-items:flex-start;">
        <div style="flex:4;min-width:0;">${renderToolbar(state)}</div>
        <div class="actions-panel" style="flex:1;min-width:180px;max-width:220px;">
          <div class="actions-panel-header">
            <span><i class="fas fa-bolt"></i> Actions</span>
            <span class="actions-sel-count" id="playlists-sel-count">0</span>
          </div>
          <button class="btn btn-sm" id="playlists-actions-refresh"><i class="fas fa-rotate"></i> Refresh</button>
        </div>
      </div>
      <div id="playlists-content" style="min-height:200px;">${renderLoading("Loading playlists…")}</div>
    </div>`;

  // Wire toolbar events once (search, service filter, PMV, toggles)
  wireToolbarEvents(container, signal, state);

  // Wire filter panel collapse/expand toggle
  const toggleBtn = container.querySelector("#playlists-filter-toggle");
  const filterPanel = container.querySelector("#playlists-filter-panel");
  if (toggleBtn && filterPanel) {
    const saved = localStorage.getItem("filterPanelCollapsed_playlists");
    if (saved === "true") filterPanel.classList.add("collapsed");
    toggleBtn.addEventListener("click", () => {
      filterPanel.classList.toggle("collapsed");
      localStorage.setItem(
        "filterPanelCollapsed_playlists",
        filterPanel.classList.contains("collapsed"),
      );
    });
  }

  // Wire actions panel refresh
  import("../shared/actions-panel.js").then(({ wireActionsRefresh }) => {
    wireActionsRefresh(container, "playlists", () => {
      state.page = 0;
      return fetchAndRender(container, signal, state);
    });
  });

  // Create Tags — creates tags for all untagged playlists in one shot
  const createTagsBtn = container.querySelector("#playlists-create-tag");
  if (createTagsBtn) {
    createTagsBtn.addEventListener(
      "click",
      async () => {
        createTagsBtn.disabled = true;
        createTagsBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Creating…';
        try {
          const resp = await fetchJSON("/api/tags/create-from-playlists", {
            method: "POST",
          });
          const created = resp.data?.created ?? 0;
          if (created > 0) {
            showToast(
              `Created ${created} tag${created !== 1 ? "s" : ""} from playlists`,
              "success",
            );
          } else {
            showToast("All playlists already have tags", "info");
          }
          updateHash("playlists", state, HASH_DEFAULTS);
          fetchAndRender(container, signal, state);
        } catch (err) {
          showToast(`Failed to create tags: ${err.message}`, "error");
          createTagsBtn.disabled = false;
          createTagsBtn.innerHTML = '<i class="fas fa-tag"></i> Create Tags';
        }
      },
      { signal },
    );
  }

  // Fetch initial data
  await fetchAndRender(container, signal, state);
}
