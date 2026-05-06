/**
 * nav.js — Top navigation bar component.
 *
 * Renders brand + nav links into <nav id="main-nav">.
 * Call renderNav(currentPageId) at app.js level.
 */

const NAV_SECTIONS = [
  {
    label: "Overview",
    items: [{ id: "dashboard", label: "Dashboard", icon: "fa-gauge-high" }],
  },
  {
    label: "Library",
    items: [
      { id: "files", label: "Files", icon: "fa-music" },
      { id: "tracks", label: "Tracks", icon: "fa-stream" },
      { id: "playlists", label: "Playlists", icon: "fa-list" },
      { id: "tags", label: "Tags", icon: "fa-tag" },
      { id: "tag-categories", label: "Tag Categories", icon: "fa-layer-group" },
    ],
  },
  {
    label: "Services",
    items: [
      { id: "services", label: "Services", icon: "fa-cloud" },
      { id: "tasks", label: "Tasks", icon: "fa-list-check" },
      { id: "folders", label: "Folders", icon: "fa-folder" },
      { id: "deemix-queue", label: "Deemix Queue", icon: "fa-download" },
      { id: "traktor-import", label: "Traktor Import", icon: "fa-compact-disc" },
    ],
  },
];

const TOOLS_ITEMS = [
  { id: "auto-categorize", label: "Auto-Categorize", icon: "fa-brain" },
  { id: "digging", label: "Digging Curator", icon: "fa-bullseye" },
  { id: "data", label: "Import/Export", icon: "fa-database" },
];

/**
 * Render the top navigation bar (brand + nav links) into #main-nav.
 * @param {string} currentPageId
 */
export function renderNav(currentPageId) {
  const navEl = document.getElementById("main-nav");
  if (!navEl) return;

  const linkHtml = (item) => `
    <a href="#${item.id}" class="topnav-link${item.id === currentPageId ? " active" : ""}" data-page="${item.id}">
      <i class="fa-solid ${item.icon}"></i>
      <span>${item.label}</span>
    </a>`;

  const navLinksHtml = NAV_SECTIONS.flatMap((section) => section.items)
    .concat(TOOLS_ITEMS)
    .map(linkHtml)
    .join("");

  navEl.innerHTML = `
    <div class="topnav-brand">
      <img src="logo_cutted_out.png" alt="" class="brand-logo-icon" />
      <img src="logo-text-only.png" alt="momo\'s music manager" class="brand-logo-text" />
    </div>
    <div class="topnav-links">
      ${navLinksHtml}
    </div>
  `;
}

/**
 * Update the active nav link without full re-render.
 * @param {string} pageId
 */
export function setActiveNav(pageId) {
  document.querySelectorAll(".topnav-link").forEach((link) => {
    link.classList.toggle("active", link.dataset.page === pageId);
  });
}
