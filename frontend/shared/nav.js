/**
 * nav.js — Top navigation bar component.
 *
 * Renders brand + dropdown-grouped nav links into <nav id="main-nav">.
 * Sections with >1 item become dropdown menus. Single-item sections
 * (Overview → Dashboard) render as direct links.
 *
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
  { id: "tag-curation", label: "Tag Curation", icon: "fa-sitemap" },
  { id: "auto-categorize", label: "Auto-Categorize", icon: "fa-brain" },
  { id: "digging", label: "Digging Curator", icon: "fa-bullseye" },
  { id: "data", label: "Import/Export", icon: "fa-database" },
  { id: "key-comparison", label: "Key Comparison", icon: "fa-scale-balanced" },
  { id: "storage", label: "Storage", icon: "fa-hdd" },
  { id: "backpack", label: "Backpack", icon: "fa-box" },
  { id: "daily", label: "Daily", icon: "fa-calendar-day" },
  { id: "tag-bundles", label: "Tag Bundles", icon: "fa-layer-group" },
  { id: "dynamic-bundles", label: "Dynamic Bundles", icon: "fa-filter-list" },
];

/** @type {string | null} - id of the currently open dropdown section, if any */
let openDropdownId = null;

/**
 * Render the top navigation bar (brand + nav links) into #main-nav.
 * @param {string} currentPageId
 */
export function renderNav(currentPageId) {
  const navEl = document.getElementById("main-nav");
  if (!navEl) return;

  // Build the sections HTML
  const sectionsHtml = NAV_SECTIONS.map((section) =>
    renderSection(section, currentPageId),
  ).join("");

  // Tools section
  const toolsHtml = renderDropdownSection("tools", "Tools", TOOLS_ITEMS, currentPageId);

  navEl.innerHTML = `
    <div class="topnav-brand">
      <img src="logo_cutted_out.png" alt="" class="brand-logo-icon" />
      <img src="logo-text-only.png" alt="momo's music manager" class="brand-logo-text" />
    </div>
    <div class="topnav-links" id="topnav-links">
      ${sectionsHtml}
      ${toolsHtml}
    </div>
    <span class="topnav-version" id="app-version"></span>
  `;

  // Wire up dropdown toggle clicks
  wireDropdowns(navEl);
}

/**
 * Render a single section. If it has exactly 1 item, render as a direct link.
 * Otherwise render as a dropdown trigger + menu.
 */
function renderSection(section, currentPageId) {
  if (section.items.length === 1) {
    // Single item — direct link
    const item = section.items[0];
    return renderLink(item, currentPageId);
  }
  return renderDropdownSection(
    section.label.toLowerCase(),
    section.label,
    section.items,
    currentPageId,
  );
}

/**
 * Render a direct nav link (for single-item sections).
 */
function renderLink(item, currentPageId) {
  const isActive = item.id === currentPageId;
  return `
    <a href="#${item.id}" class="topnav-link${isActive ? " active" : ""}" data-page="${item.id}">
      <i class="fa-solid ${item.icon}"></i>
      <span>${item.label}</span>
    </a>`;
}

/**
 * Render a dropdown section (trigger + menu).
 */
function renderDropdownSection(sectionId, label, items, currentPageId) {
  const hasActiveChild = items.some((item) => item.id === currentPageId);
  const isOpen = openDropdownId === sectionId;

  const itemsHtml = items
    .map((item) => {
      const isActive = item.id === currentPageId;
      return `
        <a href="#${item.id}" class="topnav-dropdown-item${isActive ? " active" : ""}" data-page="${item.id}">
          <i class="fa-solid ${item.icon}"></i>
          <span>${item.label}</span>
        </a>`;
    })
    .join("");

  return `
    <div class="topnav-dropdown${isOpen ? " open" : ""}" data-dropdown="${sectionId}">
      <button class="topnav-dropdown-trigger${hasActiveChild ? " active" : ""}" data-dropdown-trigger="${sectionId}">
        <span>${label}</span>
        <i class="fa-solid fa-chevron-down topnav-chevron"></i>
      </button>
      <div class="topnav-dropdown-menu">
        ${itemsHtml}
      </div>
    </div>`;
}

/**
 * Wire up click handlers for dropdown toggles and outside-click closing.
 */
function wireDropdowns(navEl) {
  const linksContainer = navEl.querySelector("#topnav-links");
  if (!linksContainer) return;

  // Delegate clicks on dropdown triggers
  linksContainer.addEventListener("click", (e) => {
    const trigger = e.target.closest("[data-dropdown-trigger]");
    if (!trigger) return;

    e.preventDefault();
    e.stopPropagation();

    const sectionId = trigger.dataset.dropdownTrigger;
    toggleDropdown(sectionId);
  });

  // Close dropdowns when clicking anywhere else
  document.addEventListener("click", (e) => {
    if (!e.target.closest(".topnav-dropdown")) {
      closeAllDropdowns();
    }
  });

  // Close dropdowns on Escape
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      closeAllDropdowns();
    }
  });

  // Close dropdowns on hash change (user clicked a link or navigated)
  window.addEventListener("hashchange", () => {
    closeAllDropdowns();
  });
}

/**
 * Toggle a dropdown open/closed.
 */
function toggleDropdown(sectionId) {
  if (openDropdownId === sectionId) {
    closeAllDropdowns();
    return;
  }
  closeAllDropdowns();
  openDropdownId = sectionId;

  const dropdown = document.querySelector(`[data-dropdown="${sectionId}"]`);
  if (dropdown) {
    dropdown.classList.add("open");
  }
}

/**
 * Close all open dropdowns.
 */
function closeAllDropdowns() {
  if (openDropdownId === null) return;
  const dropdown = document.querySelector(`[data-dropdown="${openDropdownId}"]`);
  if (dropdown) {
    dropdown.classList.remove("open");
  }
  openDropdownId = null;
}

/**
 * Update the active nav link without full re-render.
 * Highlights direct links, dropdown items, and dropdown triggers.
 * @param {string} pageId
 */
export function setActiveNav(pageId) {
  // Direct links
  document.querySelectorAll(".topnav-link").forEach((link) => {
    link.classList.toggle("active", link.dataset.page === pageId);
  });

  // Dropdown items
  document.querySelectorAll(".topnav-dropdown-item").forEach((item) => {
    item.classList.toggle("active", item.dataset.page === pageId);
  });

  // Dropdown triggers — highlight if any child is active
  document.querySelectorAll(".topnav-dropdown-trigger").forEach((trigger) => {
    const dropdown = trigger.closest(".topnav-dropdown");
    if (!dropdown) return;
    const hasActive =
      dropdown.querySelector(`.topnav-dropdown-item[data-page="${pageId}"]`) !== null;
    trigger.classList.toggle("active", hasActive);
  });
}
