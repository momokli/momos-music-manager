import { useState, useCallback } from "react";
import { usePlaylists, useToggleArchive, getServiceMeta, type Playlist } from "../api/playlists";

/* ─── Constants ──────────────────────────────────── */

const PAGE_SIZE = 50;

/* ─── Helpers ────────────────────────────────────── */

function formatCount(n: number): string {
  return n.toLocaleString();
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

/* ─── Service Badges (handles comma-separated `services` field) ─── */

function PlaylistServiceBadges({ services }: { services: string | null }) {
  const serviceList = services
    ? services.split(",").map((s) => s.trim()).filter(Boolean)
    : [];

  // Deduplicate
  const unique = [...new Set(serviceList)];

  return (
    <span style={{ display: "inline-flex", gap: "4px", flexWrap: "wrap" }}>
      {unique.map((svc) => (
        <ServiceBadge key={svc} service={svc} />
      ))}
    </span>
  );
}

/* ─── Playlist Row ───────────────────────────────── */

function PlaylistRow({
  playlist,
  onToggleArchive,
}: {
  playlist: Playlist;
  onToggleArchive: (id: number) => void;
}) {
  return (
    <tr>
      <td>
        <strong>{playlist.name}</strong>
        {playlist.description && (
          <div className="text-muted text-xs" style={{ marginTop: "2px" }}>
            {playlist.description}
          </div>
        )}
      </td>
      <td className="text-center">
        <span className="badge badge-plays">{formatCount(playlist.trackCount)}</span>
      </td>
      <td>
        <PlaylistServiceBadges services={playlist.services || playlist.service} />
      </td>
      <td className="text-center">
        <span className="archive-toggle-btn" data-action="toggle-archive">
          <button
            className="btn btn-sm btn-icon"
            style={{
              color: playlist.archiveDeleted
                ? "var(--text-muted, #94a3b8)"
                : "var(--accent, #6366f1)",
            }}
            onClick={() => onToggleArchive(playlist.id)}
            title={playlist.archiveDeleted ? "Restore from archive" : "Archive playlist"}
          >
            <i
              className={`fa-solid ${
                playlist.archiveDeleted ? "fa-box-archive" : "fa-box-open"
              }`}
            />
          </button>
        </span>
      </td>
      <td className="text-xs text-muted">{timeAgo(playlist.updatedAt)}</td>
    </tr>
  );
}

/* ─── Main Lists Page ────────────────────────────── */

export default function Lists() {
  const [page, setPage] = useState(1);
  const [search, setSearch] = useState("");
  const [debouncedSearch, setDebouncedSearch] = useState("");

  // Debounce search input
  const [debounceTimer, setDebounceTimer] = useState<ReturnType<typeof setTimeout> | null>(null);

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

  const { data, isLoading, error } = usePlaylists({
    page,
    pageSize: PAGE_SIZE,
    search: debouncedSearch || undefined,
  });

  const toggleArchiveMutation = useToggleArchive();

  const handleToggleArchive = useCallback(
    (id: number) => {
      toggleArchiveMutation.mutate(id);
    },
    [toggleArchiveMutation],
  );

  const playlists = data?.playlists ?? [];
  const total = data?.total ?? 0;
  const totalPages = Math.ceil(total / PAGE_SIZE);

  return (
    <div data-page="lists">
      <div className="page-header">
        <h1>
          <i className="fa-solid fa-list" /> Lists
        </h1>
        <span className="subtitle">All playlists — {formatCount(total)} total</span>
      </div>

      {/* ── Search ── */}
      <div className="toolbar">
        <div className="search-wrap">
          <i className="fa-solid fa-search" />
          <input
            className="input-text input-search"
            type="text"
            placeholder="Search playlists…"
            value={search}
            data-lists-search
            onChange={(e) => handleSearchChange(e.target.value)}
          />
        </div>
        <span className="text-muted text-sm">
          {formatCount(playlists.length)} shown
        </span>
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
          <h3>Failed to load playlists</h3>
          <p>{(error as Error).message}</p>
        </div>
      )}

      {/* ── Table ── */}
      {!isLoading && !error && (
        <>
          {playlists.length === 0 ? (
            <div className="empty-state">
              <div className="empty-icon">
                <i className="fa-solid fa-list" />
              </div>
              <h3>No playlists found</h3>
              <p>
                {debouncedSearch
                  ? "Try a different search term."
                  : "Add a service or create a local playlist to get started."}
              </p>
            </div>
          ) : (
            <div className="table-wrap">
              <table className="data-table">
                <thead>
                  <tr>
                    <th>Name</th>
                    <th className="text-center">Tracks</th>
                    <th>Service</th>
                    <th className="text-center">Archive</th>
                    <th>Updated</th>
                  </tr>
                </thead>
                <tbody>
                  {playlists.map((pl) => (
                    <PlaylistRow
                      key={pl.id}
                      playlist={pl}
                      onToggleArchive={handleToggleArchive}
                    />
                  ))}
                </tbody>
              </table>
            </div>
          )}

          {/* ── Pagination ── */}
          {totalPages > 1 && (
            <div className="pagination">
              <button
                className="pagination-btn"
                disabled={page <= 1}
                onClick={() => setPage((p) => Math.max(1, p - 1))}
              >
                <i className="fa-solid fa-chevron-left" />
              </button>
              {Array.from({ length: totalPages }, (_, i) => i + 1)
                .filter((p) => {
                  // Show first, last, and pages around current
                  return (
                    p === 1 ||
                    p === totalPages ||
                    Math.abs(p - page) <= 2
                  );
                })
                .map((p, idx, arr) => {
                  // Ellipsis
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
