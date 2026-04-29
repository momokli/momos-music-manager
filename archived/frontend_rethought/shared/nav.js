/**
 * nav.js — Navigation bar component.
 *
 * Call renderNav(pageId) at module load time on every page.
 * <nav id="main-nav"></nav> must exist in the HTML.
 *
 * Usage:
 *   import { renderNav } from './shared/nav.js';
 *   renderNav('files');
 */

const NAV_ITEMS = [
  { id: "index", label: "Home", icon: "fa-house", href: "index.html" },
  { id: "services", label: "Services", icon: "fa-server", href: "services.html" },
  { id: "files", label: "Files", icon: "fa-music", href: "files.html" },
  { id: "tracks", label: "Tracks", icon: "fa-stream", href: "tracks.html" },
  {
    id: "playlists",
    label: "Playlists",
    icon: "fa-spotify",
    href: "playlists.html",
    iconBrand: true,
  },
  { id: "folders", label: "Folders", icon: "fa-folder", href: "folders.html" },
  { id: "tags", label: "Tags", icon: "fa-tag", href: "tags.html" },
  {
    id: "tag-categories",
    label: "Categories",
    icon: "fa-layer-group",
    href: "tag-categories.html",
  },
  { id: "tasks", label: "Tasks", icon: "fa-list-check", href: "tasks.html" },
];

const TOOLS_ITEMS = [
  {
    id: "tags-from-playlists",
    label: "Create Tags from Playlists",
    icon: "fa-wand-magic-sparkles",
    href: "tags-from-playlists.html",
  },
  {
    id: "auto-categorize",
    label: "Auto-Categorize",
    icon: "fa-robot",
    href: "auto-categorize.html",
  },
  {
    id: "bulk-import-tags",
    label: "Bulk Import Tags",
    icon: "fa-inbox",
    href: "bulk-import-tags.html",
  },
];

export function renderNav(currentPageId) {
  const navEl = document.getElementById("main-nav");
  if (!navEl) return;

  const iconPrefix = (brand) => (brand ? "fab" : "fas");

  const itemsHtml = NAV_ITEMS.map((item) => {
    const active = item.id === currentPageId ? " active" : "";
    const prefix = iconPrefix(item.iconBrand);
    return `<a href="${item.href}" class="nav-item${active}" data-nav="${item.id}">
      <i class="${prefix} ${item.icon}"></i>
      <span class="nav-label">${item.label}</span>
    </a>`;
  }).join("");

  const activeTool = TOOLS_ITEMS.find((t) => t.id === currentPageId);
  const dropdownLabel = activeTool ? activeTool.label : "Tools";
  const dropdownActive = activeTool ? " active" : "";

  const toolsHtml = TOOLS_ITEMS.map((item) => {
    const active = item.id === currentPageId ? " active" : "";
    return `<a href="${item.href}" class="nav-dd-item${active}" data-nav="${item.id}">
      <i class="fas ${item.icon}"></i> ${item.label}
    </a>`;
  }).join("");

  const html = `
    <div class="main-nav" role="navigation" aria-label="Main navigation">
      <div class="main-nav-inner">
        <div class="nav-scroll">
          ${itemsHtml}
        </div>
        <div class="nav-divider"></div>
        <div class="nav-dd">
          <span class="nav-item nav-dd-toggle${dropdownActive}" tabindex="0" role="button" aria-haspopup="true">
            <i class="fas fa-tools"></i>
            <span class="nav-label">${dropdownLabel}</span>
            <i class="fas fa-chevron-down nav-chevron"></i>
          </span>
          <div class="nav-dd-menu" role="menu">
            ${toolsHtml}
          </div>
        </div>
      </div>
    </div>
  `;

  navEl.innerHTML = html;

  const toggle = navEl.querySelector(".nav-dd-toggle");
  if (toggle) {
    toggle.addEventListener("click", (e) => e.preventDefault());
  }
}
