import { useState, useEffect, useRef } from "react";
import { useQuery } from "@tanstack/react-query";
import { searchTags, type ApiTag } from "../api/digging";
import { useDailyGenerate, type DailyGenerateResponse } from "../api/daily";

/* ─── Helpers ────────────────────────────────────── */

function formatDate(epochSecs: number): string {
  if (!epochSecs) return "";
  const d = new Date(epochSecs * 1000);
  const now = new Date();
  const yesterday = new Date(now);
  yesterday.setDate(yesterday.getDate() - 1);

  if (d.toDateString() === now.toDateString()) return "Today";
  if (d.toDateString() === yesterday.toDateString()) return "Yesterday";
  return d.toLocaleDateString();
}

function spotifyStatusLabel(status: string): string {
  switch (status) {
    case "not_configured":
      return "(Spotify not configured)";
    case "no_tracks":
      return "(No tracks to push)";
    case "failed":
      return "(Spotify push failed — check server logs)";
    default:
      return "(Spotify push skipped)";
  }
}

const HISTORY_KEY = "daily-history";
const MAX_HISTORY = 20;

interface HistoryEntry {
  playlistName: string;
  trackCount: number;
  spotifyUrl?: string;
  generatedAt: number;
}

function loadHistory(): HistoryEntry[] {
  try {
    const raw = localStorage.getItem(HISTORY_KEY);
    return raw ? JSON.parse(raw) : [];
  } catch {
    return [];
  }
}

function saveHistory(history: HistoryEntry[]) {
  try {
    localStorage.setItem(HISTORY_KEY, JSON.stringify(history));
  } catch {
    // Ignore storage errors
  }
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
    <div className="typeahead-wrap" style={{ position: "relative", flex: 1 }}>
      <input
        type="text"
        className="input-text"
        placeholder="add tag..."
        autoComplete="off"
        value={query}
        data-daily-tag-search
        onChange={(e) => {
          setQuery(e.target.value);
          setOpen(true);
        }}
        onFocus={() => {
          if (query) setOpen(true);
        }}
        onBlur={() => {
          setTimeout(() => setOpen(false), 200);
        }}
      />
      <div
        className={`tag-dropdown ${open && tags.length > 0 ? "open" : ""}`}
        data-daily-tag-dropdown
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

/* ─── Main Daily Page ────────────────────────────── */

export default function Daily() {
  const [selectedTags, setSelectedTags] = useState<ApiTag[]>([]);
  const [bpmMin, setBpmMin] = useState(120);
  const [bpmMax, setBpmMax] = useState(140);
  const [limit, setLimit] = useState(20);
  const [excludeFullyTagged, setExcludeFullyTagged] = useState(true);
  const [history, setHistory] = useState<HistoryEntry[]>(() => loadHistory());
  const [result, setResult] = useState<DailyGenerateResponse | null>(null);

  const generateMutation = useDailyGenerate();

  const handleTagSelect = (tag: ApiTag) => {
    setSelectedTags((prev) => {
      if (prev.some((t) => t.id === tag.id)) return prev;
      return [...prev, tag];
    });
  };

  const handleRemoveTag = (tagId: number) => {
    setSelectedTags((prev) => prev.filter((t) => t.id !== tagId));
  };

  const handleBpmPreset = (min: number, max: number) => {
    setBpmMin(min);
    setBpmMax(max);
  };

  const handleGenerate = () => {
    if (selectedTags.length === 0) {
      // Could show a toast, but for now just return
      return;
    }

    generateMutation.mutate(
      {
        tags: selectedTags.map((t) => t.name),
        bpmMin: bpmMin || 0,
        bpmMax: bpmMax || 999,
        limit,
        excludeFullyTagged,
      },
      {
        onSuccess: (data) => {
          setResult(data);

          const entry: HistoryEntry = {
            playlistName: data.playlistName,
            trackCount: data.trackCount,
            spotifyUrl: data.spotifyUrl,
            generatedAt: Math.floor(Date.now() / 1000),
          };

          const updatedHistory = [entry, ...history].slice(0, MAX_HISTORY);
          setHistory(updatedHistory);
          saveHistory(updatedHistory);
        },
      },
    );
  };

  const bpmStr =
    bpmMin > 0 || bpmMax < 999
      ? `${bpmMin}\u2013${bpmMax} BPM`
      : "";

  return (
    <div data-page="daily">
      <div className="page-header">
        <h1>
          <i className="fa-solid fa-calendar-day" /> Daily Tagging Queue
        </h1>
      </div>
      <p className="daily-intro">
        Generate a narrowed Spotify playlist for on-the-go tagging.
        Listen on your phone and tag by adding tracks to tag-named playlists
        in Spotify.
      </p>

      {/* ── Form ── */}
      <div className="card daily-form">
        {/* Source Tags */}
        <div className="daily-form-row">
          <label className="daily-label">Source Tags</label>
          <div className="daily-tag-input-row">
            <TagSearchInput onSelect={handleTagSelect} />
          </div>
          {selectedTags.length > 0 && (
            <div className="tag-chips">
              {selectedTags.map((tag) => (
                <span className="tag-chip" key={tag.id} data-daily-tag={tag.name}>
                  {tag.name}
                  <span
                    className="tag-chip-x"
                    onClick={() => handleRemoveTag(tag.id)}
                  >
                    &times;
                  </span>
                </span>
              ))}
            </div>
          )}
        </div>

        {/* BPM Range */}
        <div className="daily-form-row">
          <label className="daily-label">BPM Range</label>
          <div className="daily-bpm-row">
            <input
              type="number"
              className="input-text"
              value={bpmMin}
              min={0}
              max={300}
              step={1}
              style={{ width: "80px" }}
              onChange={(e) => setBpmMin(parseInt(e.target.value, 10) || 0)}
            />
            <span className="daily-bpm-sep">&ndash;</span>
            <input
              type="number"
              className="input-text"
              value={bpmMax}
              min={0}
              max={300}
              step={1}
              style={{ width: "80px" }}
              onChange={(e) => setBpmMax(parseInt(e.target.value, 10) || 0)}
            />
            <div className="daily-bpm-presets" data-daily-bpm-presets>
              <button
                className="btn btn-sm"
                onClick={() => handleBpmPreset(120, 130)}
              >
                120&ndash;130
              </button>
              <button
                className="btn btn-sm"
                onClick={() => handleBpmPreset(130, 140)}
              >
                130&ndash;140
              </button>
              <button
                className="btn btn-sm"
                onClick={() => handleBpmPreset(140, 150)}
              >
                140&ndash;150
              </button>
              <button
                className="btn btn-sm"
                onClick={() => handleBpmPreset(145, 155)}
              >
                145&ndash;155
              </button>
              <button
                className="btn btn-sm"
                onClick={() => handleBpmPreset(150, 160)}
              >
                150&ndash;160
              </button>
            </div>
          </div>
        </div>

        {/* Track limit */}
        <div className="daily-form-row">
          <label className="daily-label">Tracks per batch</label>
          <input
            type="number"
            className="input-text"
            value={limit}
            min={5}
            max={50}
            step={5}
            style={{ width: "80px" }}
            onChange={(e) => setLimit(parseInt(e.target.value, 10) || 20)}
          />
        </div>

        {/* Exclude toggle */}
        <div className="daily-form-row">
          <label className="checkbox-label">
            <input
              type="checkbox"
              checked={excludeFullyTagged}
              onChange={(e) => setExcludeFullyTagged(e.target.checked)}
            />
            {" "}Exclude already fully tagged (has P+M+V tags)
          </label>
        </div>

        {/* Generate button */}
        <div className="daily-form-row">
          <button
            className="btn btn-primary"
            data-action="generate"
            disabled={generateMutation.isPending || selectedTags.length === 0}
            onClick={handleGenerate}
          >
            {generateMutation.isPending ? (
              <>
                <i className="fa-solid fa-spinner fa-spin" /> Generating...
              </>
            ) : (
              <>
                <i className="fa-solid fa-bolt" /> Generate Playlist
              </>
            )}
          </button>
        </div>
      </div>

      {/* ── Result ── */}
      {result && (
        <div className="card daily-result-card" data-daily-result>
          <h4>
            <i className="fa-solid fa-check-circle" style={{ color: "var(--green)" }} />{" "}
            {result.playlistName}
          </h4>
          <p>
            {result.trackCount} track{result.trackCount !== 1 ? "s" : ""}
            {bpmStr ? ` \u00b7 ${bpmStr}` : ""}
          </p>
          <div className="daily-result-actions">
            {result.spotifyUrl ? (
              <a
                href={result.spotifyUrl}
                target="_blank"
                rel="noopener"
                className="btn btn-sm daily-spotify-btn"
              >
                <i className="fa-brands fa-spotify" /> Open in Spotify
              </a>
            ) : (
              <span className="text-muted">
                {spotifyStatusLabel(result.spotifyPushStatus)}
              </span>
            )}
          </div>
        </div>
      )}

      {/* ── History ── */}
      <div className="card daily-history">
        <h3>
          <i className="fa-solid fa-history" /> History
        </h3>
        <div id="daily-history-list">
          {history.length === 0 ? (
            <p className="text-muted">No playlists generated yet.</p>
          ) : (
            history.map((h, i) => (
              <div className="daily-history-item" key={i}>
                <span className="daily-history-name">{h.playlistName}</span>
                <span className="daily-history-count">{h.trackCount} tracks</span>
                {h.spotifyUrl && (
                  <a
                    href={h.spotifyUrl}
                    target="_blank"
                    rel="noopener"
                    className="btn btn-xs daily-spotify-btn"
                    title="Open in Spotify"
                  >
                    <i className="fa-brands fa-spotify" />
                  </a>
                )}
                <span className="daily-history-date">{formatDate(h.generatedAt)}</span>
              </div>
            ))
          )}
        </div>
      </div>

      {/* ── Error state ── */}
      {generateMutation.isError && (
        <div className="error-message">
          Failed to generate playlist: {generateMutation.error.message}
        </div>
      )}
    </div>
  );
}
