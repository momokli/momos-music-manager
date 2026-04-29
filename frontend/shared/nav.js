/**
 * nav.js — Sidebar navigation component.
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
      { id: "traktor-import", label: "Traktor Import", icon: "fa-compact-disc" },
    ],
  },
];

const TOOLS_ITEMS = [
  { id: "auto-categorize", label: "Auto-Categorize", icon: "fa-brain" },
  { id: "digging", label: "Digging Curator", icon: "fa-bullseye" },
];

/**
 * Render the full sidebar (brand + nav links) into #main-nav.
 * @param {string} currentPageId
 */
export function renderNav(currentPageId) {
  const navEl = document.getElementById("main-nav");
  if (!navEl) return;

  const linkHtml = (item) => `
    <a href="#${item.id}" class="sidebar-link${item.id === currentPageId ? " active" : ""}" data-page="${item.id}">
      <i class="fa-solid ${item.icon}"></i>
      <span>${item.label}</span>
    </a>`;

  const sectionsHtml = NAV_SECTIONS.map(
    (section) => `
    <div class="sidebar-section">
      <div class="sidebar-section-title">${section.label}</div>
      ${section.items.map(linkHtml).join("")}
    </div>`,
  ).join("");

  const toolsHtml = `
    <div class="sidebar-section">
      <div class="sidebar-section-title">Tools</div>
      ${TOOLS_ITEMS.map(linkHtml).join("")}
    </div>`;

  navEl.innerHTML = `
    <div class="sidebar-brand">
      <div class="sidebar-brand-logos">
        <img src="logo_cutted_out.png" alt="" class="brand-logo-icon" />
        <img src="logo-text-only.png" alt="momo's music manager" class="brand-logo-text" />
      </div>
    </div>
    <div class="sidebar-nav">
      ${sectionsHtml}
      ${toolsHtml}
    </div>
  `;
}

/**
 * Update the active nav link without full re-render.
 * @param {string} pageId
 */
export function setActiveNav(pageId) {
  document.querySelectorAll(".sidebar-link").forEach((link) => {
    link.classList.toggle("active", link.dataset.page === pageId);
  });
}
