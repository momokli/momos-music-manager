import { useLocation } from "react-router-dom";

interface NavItem {
  id: string;
  icon: string;
  label: string;
  href: string;
}

interface NavSection {
  key: string;
  label: string;
  items: NavItem[];
}

const SECTIONS: NavSection[] = [
  {
    key: "workflows",
    label: "WORKFLOWS",
    items: [
      { id: "dig", icon: "fa-bullseye", label: "Dig", href: "#/digging" },
      { id: "daily", icon: "fa-calendar-day", label: "Daily", href: "#/daily" },
      { id: "backpack", icon: "fa-box", label: "Pack", href: "#/backpack" },
    ],
  },
  {
    key: "library",
    label: "LIBRARY",
    items: [
      { id: "tracks", icon: "fa-stream", label: "Tracks", href: "#/tracks" },
      { id: "lists", icon: "fa-list", label: "Lists", href: "#/lists" },
      { id: "tags", icon: "fa-tag", label: "Tags", href: "#/tags" },
    ],
  },
  {
    key: "setup",
    label: "SETTINGS",
    items: [{ id: "setup", icon: "fa-gear", label: "Setup", href: "#/setup" }],
  },
];

/**
 * Build a map from nav-item ID to the path portion of its href.
 * E.g. "dig" → "/digging"
 */
function buildPathMap(): Record<string, string> {
  const map: Record<string, string> = {};
  for (const section of SECTIONS) {
    for (const item of section.items) {
      // href is "#/digging" → extract "/digging"
      map[item.id] = item.href.replace(/^#/, "");
    }
  }
  return map;
}

const ID_TO_PATH = buildPathMap();

/**
 * Resolve the active nav-item id from the current hash-based pathname.
 * Uses useLocation().pathname which HashRouter derives from the hash.
 */
function resolveActiveItem(pathname: string): string | null {
  for (const [id, path] of Object.entries(ID_TO_PATH)) {
    if (pathname === path) {
      return id;
    }
  }
  return null;
}

export function Sidebar() {
  const location = useLocation();
  const activeItemId = resolveActiveItem(location.pathname);

  return (
    <aside className="sidebar" data-sidebar>
      <div className="sidebar-brand">
        <span className="sidebar-logo">🎵</span>
        <span className="sidebar-title">momo's</span>
      </div>

      <nav className="sidebar-nav">
        {SECTIONS.map((section) => (
          <div
            key={section.key}
            className="sidebar-section"
            data-nav-section={section.key}
          >
            <span className="sidebar-section-label">{section.label}</span>
            {section.items.map((item) => (
              <a
                key={item.id}
                href={item.href}
                className="sidebar-link"
                data-nav-item={item.id}
                data-active={activeItemId === item.id ? "true" : undefined}
              >
                <i className={`fa-solid ${item.icon}`} />
                <span>{item.label}</span>
              </a>
            ))}
          </div>
        ))}
      </nav>

      <div className="sidebar-footer">
        <span className="sidebar-version">v0.8.1</span>
      </div>
    </aside>
  );
}
