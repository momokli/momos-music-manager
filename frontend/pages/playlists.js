import { renderLoading, renderErrorBlock, showToast } from "../shared/components.js";
import { fetchJSON } from "../shared/api.js";
import {
  renderSearchInput,
  renderFilterGroup,
  wireSearchFilter,
} from "../shared/search-filter.js";

const SVC = {
  spotify: ["fab fa-spotify", "Spotify"],
  soundcloud: ["fab fa-soundcloud", "SoundCloud"],
  youtube: ["fab fa-youtube", "YouTube"],
};
const SVC_CLS = {
  spotify: "service-badge spotify",
  soundcloud: "service-badge soundcloud",
  youtube: "service-badge youtube",
};

const SVC_OPTIONS = [
  { value: "all", label: "All" },
  { value: "spotify", label: "Spotify" },
  { value: "soundcloud", label: "SoundCloud" },
  { value: "youtube", label: "YouTube" },
];

function sBadge(s) {
  return `<span class="${SVC_CLS[s]}"><i class="${SVC[s][0]}"></i> ${SVC[s][1]}</span>`;
}

function tagCell(t) {
  if (!t)
    return `<span class="status-badge" style="background:rgba(245,158,11,0.1);color:var(--yellow)"><i class="fas fa-exclamation-triangle"></i> No tag</span>`;
  return `<span class="tag-badge font-mono" style="background:var(--accent-bg);color:var(--accent);border:1px solid var(--border)"><i class="fas fa-check-circle" style="color:var(--green)"></i> ${t}</span>`;
}

function syncCell(v) {
  return v === null
    ? `<em style="color:var(--text-muted)">Never</em>`
    : `<span style="color:var(--text-muted)">${v}</span>`;
}

/** Show a subscription bell icon (green = subscribed, muted = not subscribed) */
function subCell(sub) {
  if (sub) {
    return `<span class="status-badge" style="background:rgba(34,197,94,0.1);color:var(--green)" title="Subscribed — polls every ${sub.pollIntervalSecs}s"><i class="fas fa-bell"></i></span>`;
  }
  return `<span style="color:var(--text-muted)" title="Not subscribed"><i class="far fa-bell"></i></span>`;
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

function rows(d) {
  return d
    .map((r) => {
      const mismatch = r.l !== r.r;
      return `<tr class="${mismatch ? "row-mismatch" : ""}" ${mismatch ? 'title="Local vs Remote differ"' : ""}>
    <td style="width:3%"></td>
    <td><span class="font-medium">${esc(r.name)}</span></td>
    <td>${sBadge(r.svc)}</td>
    <td style="text-align:center">${subCell(r.sub)}</td>
    <td><span class="${mismatch ? "diff-badge" : ""}">${r.l}</span></td>
    <td><span class="${mismatch ? "diff-badge" : ""}">${r.r}</span></td>
    <td>${syncCell(r.sync)}</td>
    <td>${tagCell(r.tag)}</td>
    <td>${actions(r)}</td>
  </tr>`;
    })
    .join("");
}

function esc(s) {
  if (!s) return "";
  const d = document.createElement("div");
  d.textContent = s;
  return d.innerHTML;
}

function renderPlaylists(container, playlists, total, untaggedTotal, state, signal) {
  const totalPages = Math.max(1, Math.ceil(total / state.pageSize));
  const currentPage = state.page + 1;

  container.innerHTML = `
<div class="toolbar">
  ${renderSearchInput("playlists", state.search)}
  ${renderFilterGroup("service", SVC_OPTIONS, state.service)}
  <label class="checkbox-label"><input type="checkbox" data-sf-filter="untaggedOnly" ${state.untaggedOnly ? "checked" : ""}> Untagged only</label>
  <label class="checkbox-label"><input type="checkbox" data-sf-filter="mismatchOnly" ${state.mismatchOnly ? "checked" : ""}> Mismatch only</label>

  <button class="btn btn-green" id="pl-create-tags"><i class="fas fa-tag"></i> Create Tags</button>
</div>

<div class="stats-row">
  <div class="stats-group">
    <button class="btn btn-sm btn-icon" id="playlists-refresh" title="Refresh"><i class="fa-solid fa-rotate"></i></button>
    <strong>${total}</strong> playlists
    <span style="margin:0 6px;color:var(--text-subtle);">·</span>
    <strong>${untaggedTotal}</strong> without tags
  </div>
</div>

<div class="table-wrap"><table class="data-table" id="pl-tbl">
  <thead><tr><th style="width:3%"></th><th style="width:22%">Name</th><th style="width:8%">Service</th><th style="width:4%;text-align:center">Sub</th><th style="width:5%">Local</th><th style="width:5%">Remote</th><th style="width:12%">Last Synced</th><th style="width:16%">Tag</th><th style="width:25%">Actions</th></tr></thead>
  <tbody>${rows(playlists)}</tbody>
</table></div>

<div class="pagination">
  <button class="pagination-btn" id="pp2" ${state.page === 0 ? "disabled" : ""}><i class="fas fa-chevron-left"></i></button>
  <span class="pagination-info">Page ${currentPage} of ${totalPages}</span>
  <button class="pagination-btn" id="pn2" ${state.page >= totalPages - 1 ? "disabled" : ""}><i class="fas fa-chevron-right"></i></button>
</div>`;

  /* ---- Wire up events ---- */

  const reload = () => loadPlaylists(container, signal, state);

  // Unified search + filter wiring (debounced) — replaces manual filter/search handlers
  const toolbar = container.querySelector(".toolbar");
  if (toolbar) {
    wireSearchFilter(toolbar, state, () => loadPlaylists(container, signal, state));
  }

  // Create Tags — creates tags for all untagged playlists in one shot
  const createTagsBtn = container.querySelector("#pl-create-tags");
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
          reload();
        } catch (err) {
          showToast(`Failed to create tags: ${err.message}`, "error");
          createTagsBtn.disabled = false;
          createTagsBtn.innerHTML = '<i class="fas fa-tag"></i> Create Tags';
        }
      },
      { signal },
    );
  }

  // Refresh
  const refBtn = container.querySelector("#playlists-refresh");
  if (refBtn) {
    refBtn.addEventListener("click", () => reload(), { signal });
  }

  // Pagination: previous
  const prevBtn = container.querySelector("#pp2");
  if (prevBtn && !prevBtn.disabled) {
    prevBtn.addEventListener(
      "click",
      () => {
        if (state.page > 0) {
          state.page--;
          reload();
        }
      },
      { signal },
    );
  }

  // Pagination: next
  const nextBtn = container.querySelector("#pn2");
  if (nextBtn && !nextBtn.disabled) {
    nextBtn.addEventListener(
      "click",
      () => {
        state.page++;
        reload();
      },
      { signal },
    );
  }

  // Action buttons (delegated on table)
  const tbl = container.querySelector("#pl-tbl");
  if (tbl) {
    tbl.addEventListener(
      "click",
      async (e) => {
        const b = e.target.closest("[data-act]");
        if (!b) return;

        const act = b.dataset.act;
        const id = parseInt(b.dataset.id, 10);
        const playlist = playlists.find((p) => p.id === id);
        if (!playlist) return;

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
              body: JSON.stringify({ name: playlist.name, categoryId: defaultCat.id }),
            });
            showToast(`Tag "${playlist.name}" created`, "success");
            reload();
          } catch (err) {
            showToast(`Failed to create tag: ${err.message}`, "error");
          }
        } else if (act === "edit-tag") {
          showToast("Edit tag: navigate to Tags page", "info");
        } else if (act === "subscribe") {
          const svc = b.dataset.service || playlist.svc;
          const plId = b.dataset.playlistId || playlist.playlistId;
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
            showToast(`Subscribed to "${playlist.name}"`, "success");
            reload();
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
            showToast(`Unsubscribed from "${playlist.name}"`, "success");
            reload();
          } catch (err) {
            showToast(`Unsubscribe failed: ${err.message}`, "error");
            b.disabled = false;
            b.innerHTML = '<i class="fas fa-bell-slash"></i>';
          }
        } else if (act === "sync") {
          const svc = b.dataset.service || playlist.svc;
          const plId = b.dataset.playlistId || playlist.playlistId;
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
            showToast(`Sync started for "${playlist.name}"`, "success");
            setTimeout(() => reload(), 2000);
          } catch (err) {
            showToast(`Sync failed: ${err.message}`, "error");
            b.disabled = false;
            b.innerHTML = '<i class="fas fa-sync"></i>';
          }
        }
      },
      { signal },
    );
  }
}

async function loadPlaylists(container, signal, state) {
  const params = new URLSearchParams();
  params.set("limit", String(state.pageSize));
  params.set("offset", String(state.page * state.pageSize));
  if (state.service && state.service !== "all") params.set("service", state.service);
  if (state.search) params.set("search", state.search);
  if (state.untaggedOnly) params.set("untagged", "true");
  if (state.mismatchOnly) params.set("mismatch", "true");

  const [plResp, tagsResp, subsResp] = await Promise.all([
    fetchJSON(`/api/playlists?${params}`, { signal }),
    fetchJSON("/api/tags", { signal }),
    fetchJSON("/api/playlists/subscriptions", { signal }),
  ]);
  if (signal.aborted) return;

  // Build tag lookup: tag name (lowercase) -> tag name
  const tagLookup = {};
  for (const t of tagsResp.data) {
    tagLookup[t.name.toLowerCase()] = t.name;
  }

  // Build subscription lookup: key = "service:playlist_id" -> subscription object
  const subLookup = {};
  const subscriptions = subsResp.data?.subscriptions || [];
  for (const s of subscriptions) {
    subLookup[`${s.service}:${s.playlist_id}`] = s;
  }

  const playlists = plResp.data.playlists || [];
  const total = plResp.data.total || playlists.length;

  // Untagged count for current result set
  const untaggedTotal = playlists.filter((p) => !tagLookup[p.name.toLowerCase()]).length;

  const adapted = playlists.map((p) => {
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
    };
  });

  renderPlaylists(container, adapted, total, untaggedTotal, state, signal);
}

export async function init(container, signal) {
  container.innerHTML = renderLoading("Loading playlists…");

  // Single state object — mutable, lives across renders
  const state = {
    page: 0,
    pageSize: 10,
    service: "all",
    search: "",
    untaggedOnly: false,
    mismatchOnly: false,
  };

  try {
    await loadPlaylists(container, signal, state);
  } catch (err) {
    if (err.name === "AbortError" || signal.aborted) return;
    container.innerHTML = renderErrorBlock({
      title: "Failed to load playlists",
      detail: err.message,
      retryFn: "location.reload()",
    });
  }
}
