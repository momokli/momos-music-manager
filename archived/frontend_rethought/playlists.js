import { fetchJSON } from "./shared/api.js";
import { formatDateTime } from "./shared/format.js";
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
import { renderNav } from "./shared/nav.js";

renderNav("playlists");

/* ── Constants ──────────────────────────── */
const ITEMS_PER_PAGE = 50;

/* ── State ──────────────────────────────── */
let currentSearch = "";
let currentService = "all";

/* ── DOM refs ───────────────────────────── */
const contentEl = document.getElementById("playlists-content");
const searchInput = document.getElementById("search-input");
const searchBtn = document.getElementById("search-btn");
const refreshBtn = document.getElementById("refresh-btn");
const serviceFilter = document.getElementById("service-filter");
const errorBanner = useErrorBanner(document.getElementById("error-message"));

/* ── Pagination ─────────────────────────── */
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
    total: "total-playlists",
    showing: "showing-playlists",
  },
  onPageChange: (page, offset) => {
    loadPlaylists();
  },
});

/* ── URL params ─────────────────────────── */
function readUrlParams() {
  const params = new URLSearchParams(window.location.search);
  const service = params.get("service");
  if (service && ["spotify", "soundcloud", "youtube"].includes(service)) {
    currentService = service;
    serviceFilter.value = service;
  }
}

/* ── Load ───────────────────────────────── */
async function loadPlaylists() {
  pagination.setLoading(true);
  contentEl.innerHTML = renderLoading("Loading playlists...");

  const params = new URLSearchParams({
    limit: String(ITEMS_PER_PAGE),
    offset: String(pagination.offset),
  });
  if (currentSearch) params.set("search", currentSearch);
  if (currentService !== "all") params.set("service", currentService);

  try {
    const res = await fetchJSON(`/playlists?${params.toString()}`);
    const data = res.data || res;
    const playlists = data.playlists || [];
    const total = data.total ?? playlists.length;

    pagination.update(total, playlists.length);

    if (playlists.length === 0 && !currentSearch && currentService === "all") {
      contentEl.innerHTML = renderEmpty({
        icon: "spotify",
        title: "No playlists yet",
        message: "Sync a service to populate your playlist library.",
        actionHtml:
          '<a href="index.html" class="btn btn-primary"><i class="fas fa-sync-alt"></i> Go to Home to Sync</a>',
      });
    } else if (playlists.length === 0) {
      contentEl.innerHTML = renderEmpty({
        icon: "search",
        title: "No playlists found",
        message: currentSearch
          ? `No playlists match "${currentSearch}".`
          : "No playlists for the selected filter.",
        actionHtml:
          '<button class="btn" onclick="window.clearFilters()"><i class="fas fa-undo"></i> Clear Filters</button>',
      });
    } else {
      contentEl.innerHTML = renderPlaylists(playlists);
    }
  } catch (err) {
    contentEl.innerHTML = renderErrorBlock({
      title: "Failed to load playlists",
      detail: err.message,
      retryFn: "loadPlaylists()",
    });
  } finally {
    pagination.setLoading(false);
  }
}

window.loadPlaylists = loadPlaylists;

/* ── Render ─────────────────────────────── */
function renderPlaylists(playlists) {
  const rows = playlists
    .map((pl) => {
      const name = pl.name || "Unnamed Playlist";
      const description = pl.description || "";
      const service = (pl.service || "").toLowerCase();
      const serviceId = pl.playlistId || "—";
      const trackCount = pl.trackCount ?? 0;
      const updatedAt = pl.updatedAt || null;

      const hasDescription = description
        ? `<br><span class="text-muted" style="font-size:0.8rem;">${description}</span>`
        : "";

      const serviceColors = {
        spotify: { cls: "service-badge spotify", icon: "fab fa-spotify" },
        soundcloud: { cls: "service-badge soundcloud", icon: "fab fa-soundcloud" },
        youtube: { cls: "service-badge youtube", icon: "fab fa-youtube" },
      };
      const sc = serviceColors[service] || {
        cls: "service-badge",
        icon: "fas fa-cloud",
      };

      const badge = `<span class="${sc.cls}"><i class="${sc.icon}"></i> ${service.charAt(0).toUpperCase() + service.slice(1)}</span>`;

      const idCell =
        serviceId !== "—"
          ? `<code style="font-family:var(--font-mono);font-size:0.82rem;color:var(--text-muted);">${serviceId}</code>`
          : "—";

      return `<tr>
      ${td(`${name}${hasDescription}`)}
      ${td(badge)}
      ${td(idCell)}
      ${td(String(trackCount))}
      ${td(updatedAt ? formatDateTime(new Date(updatedAt * 1000)) : "—")}
    </tr>`;
    })
    .join("");

  return renderTable(
    ["Playlist Name", "Service", "Service ID", "Tracks", "Last Updated"],
    rows,
  );
}

/* ── Filter helpers ─────────────────────── */
window.clearFilters = function () {
  currentSearch = "";
  currentService = "all";
  searchInput.value = "";
  serviceFilter.value = "all";
  pagination.reset();
  loadPlaylists();
};

// Search bar (shared component handles ENTER, button click, Cmd+F, Escape)
const searchBar = initSearchBar({
  onSearch: (term) => {
    currentSearch = term;
    pagination.reset();
    loadPlaylists();
  },
});

function doFilterService() {
  const val = serviceFilter.value;
  if (val === currentService) return;
  currentService = val;
  pagination.reset();
  loadPlaylists();
}

/* ── Init ───────────────────────────────── */
document.addEventListener("DOMContentLoaded", () => {
  readUrlParams();

  refreshBtn?.addEventListener("click", () => {
    loadPlaylists();
  });
  serviceFilter?.addEventListener("change", doFilterService);

  loadPlaylists();
});
