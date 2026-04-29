import { fetchJSON } from "./shared/api.js";
import {
  useErrorBanner,
  renderLoading,
  renderEmpty,
  renderErrorBlock,
  renderTable,
  td,
  Pagination,
  initSearchBar,
} from "./shared/components.js";
import { formatDuration } from "./shared/format.js";
import { renderNav } from "./shared/nav.js";

renderNav("tracks");

const ITEMS_PER_PAGE = 50;

let currentSearch = "";
let currentService = "all";
let loadToken = 0;

const tracksContent = document.getElementById("tracks-content");
const errorBanner = useErrorBanner(document.getElementById("error-message"));
const searchInput = document.getElementById("search-input");
const searchBtn = document.getElementById("search-btn");
const refreshBtn = document.getElementById("refresh-btn");
const serviceFilter = document.getElementById("service-filter");

const pagination = new Pagination({
  itemsPerPage: ITEMS_PER_PAGE,
  initialPage: 0,
  showFirstLast: true,
  bindings: {
    prev: "prev-page-btn",
    next: "next-page-btn",
    first: "first-page-btn",
    last: "last-page-btn",
    info: "page-info",
    total: "total-tracks",
    showing: "showing-tracks",
  },
  onPageChange: (page, offset) => {
    loadTracks();
  },
});

async function loadTracks() {
  const token = ++loadToken;
  pagination.setLoading(true);
  tracksContent.innerHTML = renderLoading("Loading tracks...");

  try {
    const params = new URLSearchParams();
    params.set("limit", String(ITEMS_PER_PAGE));
    params.set("offset", String(pagination.offset));

    if (currentSearch) {
      params.set("search", currentSearch);
    }
    if (currentService && currentService !== "all") {
      params.set("service", currentService);
    }

    const countParams = new URLSearchParams(params);
    // search is not supported in count query but we can still send it (it'll be ignored)
    // Actually the count handler uses the same TracksQuery struct with search field
    // but looking at get_tracks_count, it only filters by service. So we can send
    // the same params and the search will be ignored by the backend for count.

    const [tracksRes, countRes] = await Promise.all([
      fetchJSON(`/tracks?${params.toString()}`),
      fetchJSON(`/tracks/count?${countParams.toString()}`),
    ]);

    const tracks = tracksRes.data || [];
    const totalCount = countRes.data || 0;

    if (token !== loadToken) return;

    pagination.update(totalCount, tracks.length);
    renderTracks(tracks);
  } catch (err) {
    tracksContent.innerHTML = renderErrorBlock({
      title: "Failed to load tracks",
      detail: err.message,
      retryFn: "loadTracks()",
    });
  } finally {
    pagination.setLoading(false);
  }
}

function renderTracks(tracks) {
  if (!tracks || tracks.length === 0) {
    if (currentSearch || (currentService && currentService !== "all")) {
      tracksContent.innerHTML = renderEmpty({
        icon: "search",
        title: "No tracks found",
        message: "Try adjusting your search or filter criteria.",
        actionHtml: "",
      });
    } else {
      tracksContent.innerHTML = renderEmpty({
        icon: "stream",
        title: "No tracks yet",
        message: "Sync playlists from connected services to see tracks here.",
        actionHtml:
          '<a href="playlists.html" class="btn btn-primary"><i class="fas fa-spotify"></i> Go to Playlists</a>',
      });
    }
    return;
  }

  const headers = [
    "Title",
    "Artist",
    "Service",
    "Album",
    "Local Files",
    "Duration",
    "ISRC",
  ];

  const rowsHtml = tracks
    .map((track) => {
      const serviceClass = track.service || "unknown";
      const serviceIcon =
        {
          spotify: "fab fa-spotify",
          soundcloud: "fab fa-soundcloud",
          youtube: "fab fa-youtube",
        }[serviceClass] || "fas fa-cloud";

      const serviceBadge = `<span class="service-badge ${serviceClass}"><i class="${serviceIcon}"></i> ${serviceClass}</span>`;

      const filesHtml =
        track.localFiles && track.localFiles.length > 0
          ? track.localFiles
              .map(
                (ft) =>
                  `<span class="badge" style="background:rgba(99,102,241,0.15);color:#a5b4fc;border:1px solid rgba(99,102,241,0.3);margin-right:4px;">${ft}</span>`,
              )
              .join("")
          : '<span class="text-muted">—</span>';

      const duration = formatDuration(
        track.durationMs ? Math.floor(track.durationMs / 1000) : null,
      );

      return `<tr>
        ${td(escapeHtml(track.title || "Untitled"))}
        ${td(escapeHtml(track.artist || "—"))}
        ${td(serviceBadge)}
        ${td(escapeHtml(track.album || "—"))}
        ${td(filesHtml)}
        ${td(duration)}
        ${td(escapeHtml(track.isrc || "—"))}
      </tr>`;
    })
    .join("");

  tracksContent.innerHTML = renderTable(headers, rowsHtml);
}

function escapeHtml(str) {
  if (typeof str !== "string") return str;
  return str
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

// Search bar (shared component handles ENTER, button click, Cmd+F, Escape)
const searchBar = initSearchBar({
  onSearch: (term) => {
    currentSearch = term;
    pagination.reset();
  },
});

function filterByService() {
  currentService = serviceFilter.value;
  pagination.reset();
}

// ── Init ──────────────────────────────────────

document.addEventListener("DOMContentLoaded", () => {
  refreshBtn?.addEventListener("click", () => {
    currentSearch = searchInput.value.trim();
    currentService = serviceFilter.value;
    pagination.reset();
  });
  serviceFilter?.addEventListener("change", filterByService);

  loadTracks();
});
