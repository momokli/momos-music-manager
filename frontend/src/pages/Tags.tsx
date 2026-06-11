import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import TagsSection from "../components/TagsSection";
import {
  useTagCategories,
  useTagEnergyLevels,
  useTagsByCategory,
  useTagBundles,
  useDynamicBundles,
  type ApiTag,
  type TagWithEnergy,
  type BundleTag,
  type DynamicBundle,
} from "../api/tags";

/* ── Constants ─────────────────────────────────────────────────── */

const SECTION_CONFIG: Record<
  string,
  { label: string; icon: string; prefix: string }
> = {
  "energy-curve": { label: "Energy Curve", icon: "fa-wave-square", prefix: "P" },
  mood: { label: "Mood", icon: "fa-heart", prefix: "M" },
  vibe: { label: "Vibe", icon: "fa-sparkles", prefix: "V" },
  genre: { label: "Genre", icon: "fa-guitar", prefix: "G" },
  merkmal: { label: "Merkmal", icon: "fa-hashtag", prefix: "E" },
  setlist: { label: "Setlist", icon: "fa-list-music", prefix: "S" },
  bundles: { label: "Bundles", icon: "fa-layer-group", prefix: "" },
};

/* ── Sub-components ───────────────────────────────────────────── */

/** Horizontal bar showing energy level with visual bar. */
function EnergyBar({
  tag,
  maxEnergy = 5,
  barWidth = 200,
}: {
  tag: TagWithEnergy;
  maxEnergy?: number;
  barWidth?: number;
}) {
  const level = tag.energy_level ?? 0;
  const fraction = maxEnergy > 0 ? level / maxEnergy : 0;
  const fillWidth = Math.round(fraction * barWidth);

  return (
    <div className="energy-row" data-energy-tag>
      <span className="energy-tag-name">{tag.tag_name}</span>
      <span className="energy-value" data-energy={level}>
        {level}
      </span>
      <div
        className="energy-bar-track"
        style={{
          width: barWidth,
          height: 10,
          background: "var(--bg-tertiary, #2a2a3a)",
          borderRadius: 5,
          overflow: "hidden",
        }}
      >
        <div
          className="energy-bar-fill"
          style={{
            width: fillWidth,
            height: "100%",
            background: "var(--accent, #6366f1)",
            borderRadius: 5,
            transition: "width 0.3s",
          }}
        />
      </div>
    </div>
  );
}

/** A small chip for a tag name. */
function TagChip({ tag }: { tag: ApiTag }) {
  return (
    <span className="tag-chip" data-tag-chip>
      {tag.name}
    </span>
  );
}

/** Add tag input with typeahead (UI only — no backend call). */
function AddTagInput({ placeholder }: { placeholder?: string }) {
  const [value, setValue] = useState("");

  return (
    <input
      className="input-text input-sm add-tag-input"
      type="text"
      placeholder={placeholder ?? "Add tag..."}
      value={value}
      data-add-tag-input
      onChange={(e) => setValue(e.target.value)}
    />
  );
}

/* ── Section Components ───────────────────────────────────────── */

function EnergyCurveSection() {
  const { data: levels = [], isLoading } = useTagEnergyLevels();

  return (
    <TagsSection sectionKey="energy-curve" dataSection="energy-curve" title="Energy Curve" icon="fa-wave-square">
      <div className="energy-curve-body">
        {isLoading && (
          <div className="loading">
            <div className="spinner" />
          </div>
        )}
        {!isLoading && levels.length === 0 && (
          <p className="text-muted" style={{ padding: "var(--space-3)" }}>
            No energy levels configured yet.
          </p>
        )}
        {levels.map((tag) => (
          <EnergyBar key={tag.tag_id} tag={tag} />
        ))}
      </div>
    </TagsSection>
  );
}

function MoodSection() {
  const { data: tags = [], isLoading } = useTagsByCategory("Mood");

  return (
    <TagsSection sectionKey="mood" dataSection="mood" title="Mood" icon="fa-heart">
      <div className="tag-chips-grid">
        {isLoading && (
          <div className="loading">
            <div className="spinner" />
          </div>
        )}
        {!isLoading && tags.map((tag) => (
          <TagChip key={tag.id} tag={tag} />
        ))}
      </div>
      <div className="add-tag-row">
        <AddTagInput placeholder="Add mood..." />
      </div>
    </TagsSection>
  );
}

function VibeSection() {
  const { data: tags = [], isLoading } = useTagsByCategory("Vibe");

  return (
    <TagsSection sectionKey="vibe" dataSection="vibe" title="Vibe" icon="fa-sparkles">
      <div className="tag-chips-grid">
        {isLoading && (
          <div className="loading">
            <div className="spinner" />
          </div>
        )}
        {!isLoading && tags.map((tag) => (
          <TagChip key={tag.id} tag={tag} />
        ))}
      </div>
      <div className="add-tag-row">
        <AddTagInput placeholder="Add vibe..." />
      </div>
    </TagsSection>
  );
}

function GenreSection() {
  const { data: tags = [], isLoading } = useTagsByCategory("Genre");

  return (
    <TagsSection sectionKey="genre" dataSection="genre" title="Genre" icon="fa-guitar">
      <div className="tag-chips-grid">
        {isLoading && (
          <div className="loading">
            <div className="spinner" />
          </div>
        )}
        {!isLoading && tags.map((tag) => (
          <TagChip key={tag.id} tag={tag} />
        ))}
      </div>
      <div className="add-tag-row">
        <AddTagInput placeholder="Add genre..." />
      </div>
    </TagsSection>
  );
}

function MerkmalSection() {
  const { data: tags = [], isLoading } = useTagsByCategory("Merkmal");

  return (
    <TagsSection sectionKey="merkmal" dataSection="merkmal" title="Merkmal" icon="fa-hashtag">
      <p className="text-muted" style={{ marginBottom: "var(--space-2)", fontSize: "0.85rem" }}>
        Freeform characteristics — create any tag you like.
      </p>
      <div className="tag-chips-grid">
        {isLoading && (
          <div className="loading">
            <div className="spinner" />
          </div>
        )}
        {!isLoading && tags.length === 0 && (
          <p className="text-muted" style={{ padding: "var(--space-2) 0" }}>
            No Merkmal tags yet.
          </p>
        )}
        {tags.map((tag) => (
          <TagChip key={tag.id} tag={tag} />
        ))}
      </div>
      <div className="add-tag-row">
        <AddTagInput placeholder="Add characteristic..." />
      </div>
    </TagsSection>
  );
}

function SetlistSection() {
  const { data: tags = [], isLoading } = useTagsByCategory("Setlist");

  return (
    <TagsSection sectionKey="setlist" dataSection="setlist" title="Setlist" icon="fa-list-music">
      {isLoading && (
        <div className="loading">
          <div className="spinner" />
        </div>
      )}
      {!isLoading && tags.length === 0 && (
        <p className="text-muted" style={{ padding: "var(--space-3)" }}>
          No setlist tags yet. Setlist tags are auto-created from playlist names.
        </p>
      )}
      {!isLoading && tags.length > 0 && (
        <table className="setlist-table">
          <thead>
            <tr>
              <th>Tag name</th>
              <th>Files</th>
              <th>
                <i className="fa-solid fa-box" title="Backpack" />
              </th>
            </tr>
          </thead>
          <tbody>
            {tags.map((tag) => (
              <tr key={tag.id}>
                <td>
                  <span className="setlist-tag-name">{tag.name}</span>
                </td>
                <td>
                  <span className="setlist-file-count">{tag.fileCount}</span>
                </td>
                <td>
                  <button
                    className="btn-icon btn-backpack"
                    data-action="toggle-backpack"
                    title={tag.backpack ? "Remove from backpack" : "Add to backpack"}
                  >
                    <i
                      className={`fa-solid ${tag.backpack ? "fa-box" : "fa-box-open"}`}
                    />
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </TagsSection>
  );
}

function BundlesSection() {
  const [showForm, setShowForm] = useState(false);
  const { data: staticBundles = [], isLoading: staticLoading } = useTagBundles();
  const { data: dynamicBundles = [], isLoading: dynamicLoading } =
    useDynamicBundles();

  return (
    <TagsSection
      sectionKey="bundles"
      dataSection="bundles"
      title="Bundles"
      icon="fa-layer-group"
      headerAction={
        <button
          className="btn btn-sm btn-primary"
          data-action="new-bundle"
          onClick={() => setShowForm((s) => !s)}
        >
          <i className="fa-solid fa-plus" /> New Bundle
        </button>
      }
    >
      {/* Static bundles */}
      <div className="bundle-subsection" data-bundle-type="static">
        <h3 className="bundle-subsection-title">
          <i className="fa-solid fa-link" /> Static Bundles
        </h3>
        {staticLoading && (
          <div className="loading">
            <div className="spinner" />
          </div>
        )}
        {!staticLoading && staticBundles.length === 0 && (
          <p className="text-muted" style={{ padding: "var(--space-2) 0" }}>
            No static bundles yet. Create one to group tags together.
          </p>
        )}
        {staticBundles.map((bundle) => (
          <div key={bundle.id} className="bundle-card">
            <span className="bundle-name">{bundle.name}</span>
            <span className="badge badge-plays">{bundle.memberCount} members</span>
          </div>
        ))}
      </div>

      {/* Dynamic bundles */}
      <div className="bundle-subsection" data-bundle-type="dynamic">
        <h3 className="bundle-subsection-title">
          <i className="fa-solid fa-sliders" /> Dynamic Bundles
        </h3>
        {dynamicLoading && (
          <div className="loading">
            <div className="spinner" />
          </div>
        )}
        {!dynamicLoading && dynamicBundles.length === 0 && (
          <p className="text-muted" style={{ padding: "var(--space-2) 0" }}>
            No dynamic bundles yet. Create one with filter criteria.
          </p>
        )}
        {dynamicBundles.map((bundle) => (
          <div key={bundle.id} className="bundle-card">
            <span className="bundle-name">{bundle.name}</span>
            <span className="badge badge-plays">
              {bundle.matching_file_count} files
            </span>
          </div>
        ))}
      </div>

      {/* New Bundle Form (UI only) */}
      {showForm && (
        <div className="bundle-form" data-bundle-form>
          <h4>Create New Bundle</h4>
          <div className="bundle-form-fields">
            <label className="field-label">
              Name
              <input className="input-text" type="text" placeholder="Bundle name..." />
            </label>
            <label className="field-label">
              Type
              <select className="input-text">
                <option value="static">Static</option>
                <option value="dynamic">Dynamic</option>
              </select>
            </label>
          </div>
          <div className="bundle-form-actions">
            <button className="btn btn-primary btn-sm">
              <i className="fa-solid fa-check" /> Create
            </button>
            <button
              className="btn btn-ghost btn-sm"
              onClick={() => setShowForm(false)}
            >
              Cancel
            </button>
          </div>
        </div>
      )}
    </TagsSection>
  );
}

/* ── Main Tags Page ───────────────────────────────────────────── */

export default function TagsPage() {
  return (
    <div data-page="tags">
      <div className="page-header">
        <h1>
          <i className="fa-solid fa-tags" /> Tags
        </h1>
        <span className="subtitle">
          Manage system tag categories — energy, mood, vibe, genre, merkmal,
          setlist, and bundles
        </span>
      </div>

      <div className="tags-sections-stack">
        <EnergyCurveSection />
        <MoodSection />
        <VibeSection />
        <GenreSection />
        <MerkmalSection />
        <SetlistSection />
        <BundlesSection />
      </div>
    </div>
  );
}
