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
import { renderNav } from "./shared/nav.js";

renderNav("tags");

const ITEMS_PER_PAGE = 50;

let allTags = [];
let filteredTags = [];
let loadToken = 0;

const contentEl = document.getElementById("tags-content");
const errorBanner = useErrorBanner(document.getElementById("error-message"));

// Search bar (shared component handles ENTER, button click, Cmd+F, Escape)
const searchBar = initSearchBar({
  onSearch: (term) => {
    filterAndRender(term);
  },
});

// Pagination
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
    total: "total-tags",
    showing: "showing-tags",
  },
  onPageChange: () => {
    renderCurrentPage();
  },
});

function filterAndRender(searchTerm) {
  if (!searchTerm) {
    filteredTags = allTags;
  } else {
    const term = searchTerm.toLowerCase();
    filteredTags = allTags.filter((tag) => {
      return (
        (tag.name && tag.name.toLowerCase().includes(term)) ||
        (tag.category && tag.category.toLowerCase().includes(term))
      );
    });
  }
  pagination.reset();
}

async function loadTags() {
  const token = ++loadToken;
  pagination.setLoading(true);
  contentEl.innerHTML = renderLoading("Loading tags...");

  try {
    const res = await fetchJSON("/tags");
    allTags = res.data || [];
    filteredTags = allTags;

    if (token !== loadToken) return;

    pagination.update(filteredTags.length, filteredTags.length);
    renderCurrentPage();
  } catch (err) {
    if (token !== loadToken) return;
    contentEl.innerHTML = renderErrorBlock({
      title: "Failed to load tags",
      detail: err.message,
      retryFn: "loadTags()",
    });
  } finally {
    pagination.setLoading(false);
  }
}

function renderCurrentPage() {
  const start = pagination.offset;
  const end = start + ITEMS_PER_PAGE;
  const pageItems = filteredTags.slice(start, end);

  if (filteredTags.length === 0) {
    contentEl.innerHTML = renderEmpty({
      icon: "tag",
      title: "No tags found",
      message: "Tags will appear here once created.",
      actionHtml: "",
    });
    return;
  }

  const headers = ["ID", "Name", "Category", "Created"];

  const rowsHtml = pageItems
    .map((tag) => {
      const categoryHtml = tag.category
        ? `<span class="badge" style="background: var(--border); color: var(--text-secondary);">${escapeHtml(tag.category)}</span>`
        : '<span style="color: var(--text-subtle);">—</span>';
      const created = tag.createdAt
        ? new Date(tag.createdAt * 1000).toLocaleDateString()
        : "—";
      return `<tr>
        ${td(escapeHtml(String(tag.id)))}
        ${td(escapeHtml(tag.name || "—"))}
        ${td(categoryHtml)}
        ${td(created)}
      </tr>`;
    })
    .join("");

  contentEl.innerHTML = renderTable(headers, rowsHtml);
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

// Init
document.addEventListener("DOMContentLoaded", () => {
  document.getElementById("refresh-btn")?.addEventListener("click", () => {
    loadTags();
  });

  loadTags();
});
