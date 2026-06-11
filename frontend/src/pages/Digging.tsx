import { useState, useEffect, useRef, useCallback } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  searchTags,
  getDiggingSuggestions,
  type ApiTag,
  type DiggingSuggestion,
  type DiggingSuggestResponse,
} from "../api/digging";

/* ─── Helpers ────────────────────────────────────── */

function camelotClass(compat: string): string {
  switch (compat) {
    case "perfect":
      return "badge-camelot perfect";
    case "good":
      return "badge-camelot good";
    default:
      return "badge-camelot ok";
  }
}

function formatBpm(bpm: number | null): string {
  if (bpm === null || bpm === undefined) return "—";
  return Math.round(bpm).toString();
}

function formatKey(key: string | null): string {
  if (!key) return "—";
  return key;
}

/* ─── Tag Search Typeahead ────────────────────────── */

function TagSearchInput({
  onSelect,
}: {
  onSelect: (tag: ApiTag) => void;
}) {
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState(false);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [debouncedQuery, setDebouncedQuery] = useState("");

  useEffect(() => {
    if (debounceRef.current) clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => {
      setDebouncedQuery(query);
    }, 150);
    return () => {
      if (debounceRef.current) clearTimeout(debounceRef.current);
    };
  }, [query]);

  const { data: tags = [] } = useQuery({
    queryKey: ["tags", "search", debouncedQuery],
    queryFn: () => searchTags(debouncedQuery),
    enabled: debouncedQuery.length >= 1,
  });

  const handleSelect = (tag: ApiTag) => {
    onSelect(tag);
    setQuery("");
    setOpen(false);
  };

  return (
    <div className="tag-search-wrap" style={{ position: "relative" }}>
      <i className="fa-solid fa-search" />
      <input
        className="input-text input-search"
        type="text"
        placeholder="Search tags…"
        value={query}
        data-digging-tag-search
        onChange={(e) => {
          setQuery(e.target.value);
          setOpen(true);
        }}
        onFocus={() => {
          if (query) setOpen(true);
        }}
        onBlur={() => {
          // Delay closing so click on dropdown item registers
          setTimeout(() => setOpen(false), 200);
        }}
      />
      <div
        className={`tag-dropdown ${open && tags.length > 0 ? "open" : ""}`}
        data-digging-tag-dropdown
      >
        {tags.map((tag) => (
          <div
            key={tag.id}
            className="tag-dropdown-item"
            onMouseDown={(e) => {
              e.preventDefault();
              handleSelect(tag);
            }}
          >
            <span className="tag-dropdown-name">{tag.name}</span>
            <span className="tag-dropdown-cat">{tag.category}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

/* ─── Suggestion Card ────────────────────────────── */

function SuggestionCard({
  suggestion,
  rank,
  isPlaying,
  isInStaging,
  onPlay,
  onAddToStaging,
}: {
  suggestion: DiggingSuggestion;
  rank: number;
  isPlaying: boolean;
  isInStaging: boolean;
  onPlay: () => void;
  onAddToStaging: () => void;
}) {
  return (
    <div className="suggestion-card" data-digging-suggestion>
      <div className="sugg-rank">#{rank}</div>
      <div className="sugg-body">
        <div className="sugg-title">{suggestion.title}</div>
        <div className="sugg-artist">{suggestion.artist}</div>

        <div className="sugg-badges">
          <span className="badge badge-bpm" data-field="bpm">
            {formatBpm(suggestion.bpm)} BPM
          </span>
          <span className="badge badge-key" data-field="key">
            {formatKey(suggestion.musicalKey)}
          </span>
          <span
            className={camelotClass(suggestion.camelotCompatibility)}
            data-camelot-compat
          >
            {suggestion.camelotCompatibility}
          </span>
        </div>

        {/* Audio player */}
        <div className="audio-player">
          <button
            className="btn-play btn-play-sm"
            data-action="play"
            onClick={onPlay}
          >
            <i className={`fa-solid ${isPlaying ? "fa-pause" : "fa-play"}`} />
          </button>
          <span className="time-display">
            {isPlaying ? "Playing…" : "Click to play"}
          </span>
        </div>
      </div>
      <div className="sugg-actions">
        {isInStaging ? (
          <span className="badge badge-plays">In staging</span>
        ) : (
          <button
            className="btn btn-sm btn-primary"
            data-action="add-to-staging"
            onClick={onAddToStaging}
          >
            <i className="fa-solid fa-plus" /> Add
          </button>
        )}
      </div>
    </div>
  );
}

/* ─── Main Digging Page ──────────────────────────── */

export default function Digging() {
  const [selectedTag, setSelectedTag] = useState<ApiTag | null>(null);
  const [shouldSearch, setShouldSearch] = useState(false);
  const [bpmTolerance, setBpmTolerance] = useState(8);
  const [staging, setStaging] = useState<DiggingSuggestion[]>([]);
  // Track which suggestion fileId is currently "playing"
  const [playingFileId, setPlayingFileId] = useState<number | null>(null);

  // Reset shouldSearch when tag changes
  useEffect(() => {
    setShouldSearch(false);
  }, [selectedTag]);

  const { data, isLoading, error } = useQuery({
    queryKey: ["digging", "suggest", selectedTag?.id, bpmTolerance],
    queryFn: (): Promise<DiggingSuggestResponse> => {
      return getDiggingSuggestions(selectedTag!.name, bpmTolerance, 10);
    },
    enabled: !!selectedTag && shouldSearch,
  });

  const suggestions = data?.suggestions ?? [];

  const handleFindSimilar = useCallback(() => {
    if (selectedTag) {
      setShouldSearch(true);
    }
  }, [selectedTag]);

  const handlePlay = useCallback((fileId: number) => {
    setPlayingFileId((prev) => (prev === fileId ? null : fileId));
  }, []);

  const handleAddToStaging = useCallback((suggestion: DiggingSuggestion) => {
    setStaging((prev) => {
      if (prev.some((s) => s.fileId === suggestion.fileId)) return prev;
      return [...prev, suggestion];
    });
  }, []);

  return (
    <div data-page="digging">
      <div className="page-header">
        <h1>
          <i className="fa-solid fa-bullseye" /> Digging
        </h1>
        <span className="subtitle">Discover tracks by tags and energy</span>
      </div>

      <div className="digging-2pane">
        {/* ── Left pane: Seeds & Staging ── */}
        <div className="pane pane-ladder">
          {/* Tag search */}
          <div className="browser-search-bar">
            <TagSearchInput onSelect={setSelectedTag} />
            <button
              className="btn btn-primary btn-sm"
              data-action="find-similar"
              disabled={!selectedTag}
              onClick={handleFindSimilar}
            >
              <i className="fa-solid fa-magnifying-glass" /> Find Similar
            </button>
          </div>

          {/* Selected tag chip */}
          {selectedTag && (
            <div className="tag-chips" style={{ marginBottom: "0.75rem" }}>
              <span className="tag-chip" data-digging-tag-chip>
                {selectedTag.name}
                <span
                  className="tag-chip-x"
                  onClick={() => {
                    setSelectedTag(null);
                    setShouldSearch(false);
                  }}
                >
                  <i className="fa-solid fa-xmark" />
                </span>
              </span>
            </div>
          )}

          {/* BPM range slider (only shown when suggestions exist) */}
          {suggestions.length > 0 && (
            <div className="browser-bpm-display">
              <span>BPM ±</span>
              <input
                type="range"
                min="1"
                max="30"
                value={bpmTolerance}
                data-bpm-range
                onChange={(e) => setBpmTolerance(Number(e.target.value))}
              />
              <span className="bpm-range-num">{bpmTolerance}</span>
            </div>
          )}

          {/* ── Staging area ── */}
          {staging.length > 0 && (
            <div className="staging-section">
              <div className="staging-header">
                <h3>
                  <i className="fa-solid fa-box" /> Staging
                </h3>
                <span className="badge badge-plays" data-staging-count>
                  {staging.length} track{staging.length !== 1 ? "s" : ""}
                </span>
              </div>

              {staging.map((s) => (
                <div
                  key={s.fileId}
                  className="suggestion-card"
                  style={{ padding: "0.5rem 0.75rem" }}
                >
                  <div className="sugg-body">
                    <div className="sugg-title">{s.title}</div>
                    <div className="sugg-artist">{s.artist}</div>
                  </div>
                  <button
                    className="btn btn-sm btn-red"
                    onClick={() =>
                      setStaging((prev) =>
                        prev.filter((x) => x.fileId !== s.fileId),
                      )
                    }
                  >
                    <i className="fa-solid fa-trash" />
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>

        {/* ── Right pane: Suggestions ── */}
        <div className="pane pane-browser">
          {isLoading && (
            <div className="digging-loading">
              <div className="spinner" />
              <span>Finding similar tracks…</span>
            </div>
          )}

          {error && (
            <div className="error-message">
              Failed to load suggestions: {(error as Error).message}
            </div>
          )}

          {!selectedTag && !isLoading && (
            <div className="empty-state">
              <div className="empty-icon">
                <i className="fa-solid fa-bullseye" />
              </div>
              <h3>Select a seed tag</h3>
              <p>Search for a tag above, then click "Find Similar" to discover tracks.</p>
            </div>
          )}

          {selectedTag && !shouldSearch && !isLoading && (
            <div className="empty-state">
              <div className="empty-icon">
                <i className="fa-solid fa-hand-pointer" />
              </div>
              <h3>Ready to dig</h3>
              <p>Click "Find Similar" to discover tracks matching "{selectedTag.name}".</p>
            </div>
          )}

          {suggestions.length === 0 && shouldSearch && !isLoading && !error && (
            <div className="empty-state">
              <div className="empty-icon">
                <i className="fa-solid fa-empty-set" />
              </div>
              <h3>No suggestions found</h3>
              <p>Try a different tag or widen the BPM range.</p>
            </div>
          )}

          {suggestions.map((s, i) => (
            <SuggestionCard
              key={s.fileId}
              suggestion={s}
              rank={i + 1}
              isPlaying={playingFileId === s.fileId}
              isInStaging={staging.some((st) => st.fileId === s.fileId)}
              onPlay={() => handlePlay(s.fileId)}
              onAddToStaging={() => handleAddToStaging(s)}
            />
          ))}
        </div>
      </div>
    </div>
  );
}
