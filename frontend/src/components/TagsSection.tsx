import { useState, useEffect, type ReactNode } from "react";

interface TagsSectionProps {
  /** Unique key for localStorage persistence (e.g. "energy-curve", "mood") */
  sectionKey: string;
  /** The data-tags-section attribute value */
  dataSection: string;
  /** Section heading text */
  title: string;
  /** Optional icon class (fa-solid fa-...) */
  icon?: string;
  /** Whether the section starts collapsed */
  defaultCollapsed?: boolean;
  /** Children to render inside the collapsible content area */
  children: ReactNode;
  /** Extra action button rendered in the header (e.g. [+ New]) */
  headerAction?: ReactNode;
}

const STORAGE_PREFIX = "tags-section-";

function getStored(key: string): boolean | null {
  try {
    const val = localStorage.getItem(`${STORAGE_PREFIX}${key}-collapsed`);
    if (val === "true") return true;
    if (val === "false") return false;
  } catch {
    // localStorage unavailable
  }
  return null;
}

function setStored(key: string, collapsed: boolean) {
  try {
    localStorage.setItem(`${STORAGE_PREFIX}${key}-collapsed`, String(collapsed));
  } catch {
    // localStorage unavailable
  }
}

export default function TagsSection({
  sectionKey,
  dataSection,
  title,
  icon,
  defaultCollapsed = false,
  children,
  headerAction,
}: TagsSectionProps) {
  const [collapsed, setCollapsed] = useState<boolean>(() => {
    const stored = getStored(sectionKey);
    return stored !== null ? stored : defaultCollapsed;
  });

  useEffect(() => {
    setStored(sectionKey, collapsed);
  }, [sectionKey, collapsed]);

  return (
    <section
      className="tags-section"
      data-tags-section={dataSection}
    >
      <div className="tags-section-header">
        <button
          className="tags-section-toggle"
          data-section-toggle
          onClick={() => setCollapsed((c) => !c)}
          aria-expanded={!collapsed}
          aria-label={collapsed ? `Expand ${title}` : `Collapse ${title}`}
        >
          <i
            className={`fa-solid fa-chevron-right${collapsed ? "" : " rotate-90"}`}
            style={{ transition: "transform 0.2s" }}
          />
        </button>
        <h2 className="tags-section-title">
          {icon && <i className={`fa-solid ${icon}`} style={{ marginRight: "var(--space-2)" }} />}
          {title}
        </h2>
        {headerAction && (
          <div className="tags-section-action">{headerAction}</div>
        )}
      </div>
      {!collapsed && (
        <div className="tags-section-body">
          {children}
        </div>
      )}
    </section>
  );
}
