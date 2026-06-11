import { useState, useCallback, useMemo } from "react";
import { useTracks, type ApiServiceTrack } from "../api/tracks";
import { getServiceMeta } from "../api/playlists";

/* ─── Constants ──────────────────────────────────── */

const PAGE_SIZE = 50;

const MINOR_KEYS = Array.from({ length: 12 }, (_, i) => `${i + 1}m`);
const MAJOR_KEYS = Array.from({ length: 12 }, (_, i) => `${i + 1}d`);
const ALL_KEYS = [...MINOR_KEYS, ...MAJOR_KEYS];

const SERVICES = ["spotify", "soundcloud", "youtube", "local"] as const;

/* ─── Sort column definition ─────────────────────── */

interface SortColumn {
  id: string;
  label: string;
  sortable: boolean;
  sortKey?: string;
}

const COLUMNS: SortColumn[] = [
  { id: "title", label: "Title", sortable: true, sortKey: "title" },
  { id: "artist", label: "Artist", sortable: true, sortKey: "artist" },
  { id: "service", label: "Service", sortable: true, sortKey: "service" },
  { id: "album", label: "Album", sortable: true, sortKey: "album" },
  { id: "bpm", label: "BPM", sortable: false },
  { id: "key", label: "Key", sortable: false },
  { id: "rating", label: "★", sortable: false },
  { id: "plays", label: "Plays", sortable: false },
  { id: "duration", label: "Duration", sortable: true, sortKey: "duration_ms" },
  { id: "playlists", label: "Playlists", sortable: false },
  { id: "imported", label: "Imported", sortable: true, sortKey: "imported_at" },
];

/* ─── Helpers ────────────────────────────────────── */

function formatCount(n: number): string {
  return n.toLocaleString();
}

function formatDuration(ms: number | null): string {
  if (!ms) return "—";
  const sec = Math.round(ms / 1000);
  const m = Math.floor(sec / 60);
  const s = sec % 60;
  return `${m}:${s.toString().padStart(2, "0")}`;
}

function formatBpm(bpm: number | null): string {
  if (bpm === null || bpm === undefined) return "—";
  return Math.round(bpm).toString();
}

function formatKey(key: string | null): string {
  if (!key) return "—";
  return key;
}

function formatTimestamp(ts: number | null): string {
  if (!ts) return "—";
  const d = new Date(ts * 1000);
  return d.toLocaleDateString();
}

function formatRating(rating: number | null): string {
  if (!rating) return "—";
  return "★".repeat(rating);
}

function timeAgo(ts: number | null): string {
  if (!ts) return "—";
  const diff = Date.now() - ts * 1000;
  if (diff < 0) return "just now";
  const mins = Math.floor(diff / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d ago`;
  return new Date(ts * 1000).toLocaleDateString();
}

/* ─── Service Badge ──────────────────────────────── */

function ServiceBadge({ service }: { service: string }) {
  const meta = getServiceMeta(service);
  return (
    <span className={`service-badge ${meta.cssClass}`} data-service-badge>
      <i className={meta.icon} /> {meta.label}
    </span>
  );
}

/* ─── Key Toggle Button ─────────────────────────── */

function KeyToggle({
  keyName,
  selected,
  onToggle,
}: {
  keyName: string;
  selected: boolean;
  onToggle: (k: string) => void;
}) {
  const isMinor = keyName.endsWith("m");
  return (
    <button
      className={`key-toggle ${isMinor ? "minor" : "major"}${selected ? " active" : ""}`}
      onClick={() => onToggle(keyName)}
      title={keyName}
      data-key-btn
    >
      {keyName}
    </button>
  );
}

/* ─── Track Row ─────────────────────────────────── */

function TrackRow({ track }: { track: ApiServiceTrack }) {
  return (
    <tr>
      <td>
        <strong>{track.title}</strong>
      </td>
      <td>{track.artist}</td>
      <td>
        <ServiceBadge service={track.service} />
      </td>
      <td className="text-muted">{track.album || "—"}</td>
      <td className="text-center">{formatBpm(track.bpm)}</td>
      <td className="text-center">{formatKey(track.musicalKey)}</td>
      <td className="text-center">{formatRating(track.rating)}</td>
      <td className="text-center">
        {track.playCount != null ? formatCount(track.playCount) : "—"}
      </td>
      <td className="text-center">{formatDuration(track.durationMs)}</td>
      <td>
        {track.playlistNames.length > 0
          ? track.playlistNames.slice(0, 3).join(", ")
          : "—"}
      </td>
      <td className="text-xs text-muted">{timeAgo(track.importedAt)}</td>
    </tr>
  );
}

/* ─── Sortable Header ───────────────────────────── */

function SortHeader({
  column,
  sort,
  order,
  onSort,
}: {
  column: SortColumn;
  sort: string;
  order: string;
  onSort: (key: string) => void;
}) {
  if (!column.sortable) {
    return <th>{column.label}</th>;
  }

  const sortKey = column.sortKey || column.id;
  const isActive = sort === sortKey;
  const dir = isActive && order === "asc" ? "asc" : "desc";

  return (
    <th
      className={`sortable ${isActive ? `sorted-${dir}` : ""}`}
      onClick={() => onSort(sortKey)}
      data-sort-key={sortKey}
    >
      {column.label}
      {isActive && (
        <i
          className={`fa-solid ${order === "asc" ? "fa-arrow-up-wide-short" : "fa-arrow-down-wide-short"}`}
          style={{ marginLeft: "4px", fontSize: "0.75em" }}
        />
      )}
    </th>
  );
}

/* ─── Main Tracks Page ──────────────────────────── */

export default function Tracks() {
  const [page, setPage] = useState(1);
  const [search, setSearch] = useState("");
  const [debouncedSearch, setDebouncedSearch] = useState("");
  const [sort, setSort] = useState("");
  const [order, setOrder] = useState("asc");
  const [selectedServices, setSelectedServices] = useState<string[]>([]);
  const [bpmMin, setBpmMin] = useState("");
  const [bpmMax, setBpmMax] = useState("");
  const [selectedKeys, setSelectedKeys] = useState<string[]>([]);

  // Debounce search input
  const [debounceTimer, setDebounceTimer] = useState<ReturnType<
    typeof setTimeout
  > | null>(null);

  const handleSearchChange = useCallback(
    (value: string) => {
      setSearch(value);
      if (debounceTimer) clearTimeout(debounceTimer);
      const timer = setTimeout(() => {
        setDebouncedSearch(value);
        setPage(1);
      }, 300);
      setDebounceTimer(timer);
    },
    [debounceTimer],
  );

  // Build query params for API
  const queryParams = useMemo(() => {
    const params: Record<string, unknown> = {
      page,
      pageSize: PAGE_SIZE,
      search: debouncedSearch || undefined,
      sort: sort || undefined,
      order: order || undefined,
    };

    if (selectedServices.length > 0) {
      params.services = selectedServices.join(",");
    }

    const bpmMinNum = bpmMin ? parseFloat(bpmMin) : 0;
    const bpmMaxNum = bpmMax ? parseFloat(bpmMax) : 300;
    if (bpmMinNum > 0) params.bpmMin = bpmMinNum;
    if (bpmMaxNum < 300) params.bpmMax = bpmMaxNum;

    if (selectedKeys.length > 0) {
      params.keys = selectedKeys.join(",");
    }

    return params as Parameters<typeof useTracks>[0];
  }, [
    page,
    debouncedSearch,
    sort,
    order,
    selectedServices,
    bpmMin,
    bpmMax,
    selectedKeys,
  ]);

  const { data, isLoading, error } = useTracks(queryParams);

  const tracks = data?.tracks ?? [];
  const total = data?.total ?? 0;
  const totalPages = Math.ceil(total / PAGE_SIZE);

  // Sort handler
  const handleSort = useCallback(
    (key: string) => {
      if (sort === key) {
        setOrder((prev) => (prev === "asc" ? "desc" : "asc"));
      } else {
        setSort(key);
        setOrder("asc");
      }
      setPage(1);
    },
    [sort],
  );

  // Service toggle
  const toggleService = useCallback((service: string) => {
    setSelectedServices((prev) => {
      if (prev.includes(service)) {
        return prev.filter((s) => s !== service);
      }
      return [...prev, service];
    });
    setPage(1);
  }, []);

  // Key toggle
  const toggleKey = useCallback((keyName: string) => {
    setSelectedKeys((prev) => {
      if (prev.includes(keyName)) {
        return prev.filter((k) => k !== keyName);
      }
      return [...prev, keyName];
    });
    setPage(1);
  }, []);

  return (
    <div data-page="tracks">
      {/* ── Page Header ── */}
      <div className="page-header">
        <h1>
          <i className="fa-solid fa-stream" /> Tracks
        </h1>
        <span className="subtitle">
          Unified track browser — {formatCount(total)} total
        </span>
      </div>

      {/* ── Toolbar ── */}
      <div className="toolbar">
        <div className="search-wrap">
          <i className="fa-solid fa-search" />
          <input
            className="input-text input-search"
            type="text"
            placeholder="Search tracks…"
            value={search}
            data-tracks-search
            onChange={(e) => handleSearchChange(e.target.value)}
          />
        </div>
        <span className="text-muted text-sm">{formatCount(tracks.length)} shown</span>
      </div>

      {/* ── Filter Panel ── */}
      <div className="filter-panel" data-filter-panel>
        {/* Service Filter */}
        <div className="filter-row" data-filter="services">
          <span className="filter-label">
            <i className="fa-solid fa-cloud" /> Service
          </span>
          <div className="filter-btn-group">
            {SERVICES.map((svc) => {
              const meta = getServiceMeta(svc);
              const active = selectedServices.includes(svc);
              return (
                <button
                  key={svc}
                  className={`filter-btn ${active ? "active" : ""}`}
                  data-service-btn={svc}
                  data-active={active ? "true" : undefined}
                  onClick={() => toggleService(svc)}
                >
                  <i className={meta.icon} /> {meta.label}
                </button>
              );
            })}
          </div>
        </div>

        {/* BPM Filter */}
        <div className="filter-row" data-filter="bpm">
          <span className="filter-label">
            <i className="fa-solid fa-gauge-high" /> BPM
          </span>
          <div className="bpm-range-inputs">
            <input
              className="input-text input-sm"
              type="number"
              min="0"
              max="300"
              placeholder="Min"
              value={bpmMin}
              onChange={(e) => {
                setBpmMin(e.target.value);
                setPage(1);
              }}
              data-bpm-min
            />
            <span className="bpm-separator">—</span>
            <input
              className="input-text input-sm"
              type="number"
              min="0"
              max="300"
              placeholder="Max"
              value={bpmMax}
              onChange={(e) => {
                setBpmMax(e.target.value);
                setPage(1);
              }}
              data-bpm-max
            />
          </div>
        </div>

        {/* Key Filter */}
        <div className="filter-row" data-filter="keys">
          <span className="filter-label">
            <i className="fa-solid fa-music" /> Key
          </span>
          <div className="key-grid">
            <div className="key-row">
              {ALL_KEYS.slice(0, 12).map((k) => (
                <KeyToggle
                  key={k}
                  keyName={k}
                  selected={selectedKeys.includes(k)}
                  onToggle={toggleKey}
                />
              ))}
            </div>
            <div className="key-row">
              {ALL_KEYS.slice(12).map((k) => (
                <KeyToggle
                  key={k}
                  keyName={k}
                  selected={selectedKeys.includes(k)}
                  onToggle={toggleKey}
                />
              ))}
            </div>
          </div>
        </div>
      </div>

      {/* ── Loading ── */}
      {isLoading && (
        <div className="loading fade-in">
          <div className="spinner" />
        </div>
      )}

      {/* ── Error ── */}
      {error && (
        <div className="error-block fade-in">
          <div className="error-icon">
            <i className="fa-solid fa-circle-exclamation" />
          </div>
          <h3>Failed to load tracks</h3>
          <p>{(error as Error).message}</p>
        </div>
      )}

      {/* ── Table ── */}
      {!isLoading && !error && (
        <>
          {tracks.length === 0 ? (
            <div className="empty-state">
              <div className="empty-icon">
                <i className="fa-solid fa-stream" />
              </div>
              <h3>No tracks found</h3>
              <p>
                {debouncedSearch
                  ? "Try a different search term or adjust your filters."
                  : "Add a service or import tracks to get started."}
              </p>
            </div>
          ) : (
            <div className="table-wrap">
              <table className="data-table">
                <thead>
                  <tr>
                    {COLUMNS.map((col) => (
                      <SortHeader
                        key={col.id}
                        column={col}
                        sort={sort}
                        order={order}
                        onSort={handleSort}
                      />
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {tracks.map((track) => (
                    <TrackRow key={track.id} track={track} />
                  ))}
                </tbody>
              </table>
            </div>
          )}

          {/* ── Pagination ── */}
          {totalPages > 1 && (
            <div className="pagination" data-pagination>
              <button
                className="pagination-btn"
                disabled={page <= 1}
                onClick={() => setPage((p) => Math.max(1, p - 1))}
              >
                <i className="fa-solid fa-chevron-left" />
              </button>
              {Array.from({ length: totalPages }, (_, i) => i + 1)
                .filter((p) => {
                  return p === 1 || p === totalPages || Math.abs(p - page) <= 2;
                })
                .map((p, idx, arr) => {
                  if (idx > 0 && p - arr[idx - 1] > 1) {
                    return (
                      <span key={`ellipsis-${p}`} className="pagination-info">
                        …
                      </span>
                    );
                  }
                  return (
                    <button
                      key={p}
                      className={`pagination-btn${p === page ? " active" : ""}`}
                      onClick={() => setPage(p)}
                    >
                      {p}
                    </button>
                  );
                })}
              <button
                className="pagination-btn"
                disabled={page >= totalPages}
                onClick={() => setPage((p) => Math.min(totalPages, p + 1))}
              >
                <i className="fa-solid fa-chevron-right" />
              </button>
            </div>
          )}
        </>
      )}
    </div>
  );
}
