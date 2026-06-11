import { useDashboardStats, useServices, useRecentFiles, ServiceConnection } from "../api/dashboard";

/* ── Helpers ─────────────────────────────────────────── */

const SERVICE_META: Record<string, { icon: string; label: string }> = {
  spotify: { icon: "fa-brands fa-spotify", label: "Spotify" },
  soundcloud: { icon: "fa-brands fa-soundcloud", label: "SoundCloud" },
  youtube: { icon: "fa-brands fa-youtube", label: "YouTube" },
};

function serviceMeta(service: string) {
  return (
    SERVICE_META[service] ?? {
      icon: "fa-solid fa-circle",
      label: service.charAt(0).toUpperCase() + service.slice(1),
    }
  );
}

function connectionBadge(conn: ServiceConnection) {
  if (!conn.configured) {
    return <span className="status-badge unconfigured">unconfigured</span>;
  }
  return conn.connected ? (
    <span className="status-badge connected">connected</span>
  ) : (
    <span className="status-badge disconnected">disconnected</span>
  );
}

function formatCount(n: number): string {
  return n.toLocaleString();
}

/* ── Sub-components ──────────────────────────────────── */

function StatCard({
  label,
  value,
  icon,
  statKey,
}: {
  label: string;
  value: number | string;
  icon: string;
  statKey: string;
}) {
  return (
    <div className="stat-card" data-stat={statKey}>
      <div className="stat-card-header">
        <span className="stat-card-label">{label}</span>
        <div className="stat-card-icon" style={{ background: "var(--accent-bg, rgba(99,102,241,0.1))" }}>
          <i className={`fa-solid ${icon}`} style={{ color: "var(--accent, #6366f1)" }} />
        </div>
      </div>
      <div className="stat-card-value">{formatCount(typeof value === "number" ? value : 0)}</div>
      <div className="stat-card-footer">Total in library</div>
    </div>
  );
}

function ServiceStatusCard({ conn }: { conn: ServiceConnection }) {
  const meta = serviceMeta(conn.service);
  return (
    <div className="service-card">
      <div className="service-card-header">
        <span className="service-card-name">
          <i className={meta.icon} /> {meta.label}
        </span>
        {connectionBadge(conn)}
      </div>
      <div className="service-card-stats">
        <span>{formatCount(conn.tracksLocal)} tracks</span>
        <span>{formatCount(conn.playlistsLocal)} playlists</span>
      </div>
    </div>
  );
}

/* ── Main Dashboard Component ────────────────────────── */

export default function Dashboard() {
  const { data: stats, isLoading: statsLoading, error: statsError } = useDashboardStats();
  const { data: services, isLoading: servicesLoading } = useServices();
  const { data: recentFiles, isLoading: recentLoading } = useRecentFiles();

  return (
    <div data-page="dashboard">
      {/* ── Stats Grid ──────────────────────────────── */}
      <section className="stats-grid">
        <StatCard label="Files" value={stats?.files ?? 0} icon="fa-file-audio" statKey="files" />
        <StatCard label="Tracks" value={stats?.tracks ?? 0} icon="fa-music" statKey="tracks" />
        <StatCard label="Playlists" value={stats?.playlists ?? 0} icon="fa-list" statKey="playlists" />
        <StatCard label="Tags" value={stats?.tags ?? 0} icon="fa-tags" statKey="tags" />
      </section>

      {/* ── Loading / Error ─────────────────────────── */}
      {statsLoading && (
        <div className="loading fade-in">
          <div className="spinner" />
        </div>
      )}
      {statsError && (
        <div className="error-block fade-in">
          <div className="error-icon">
            <i className="fa-solid fa-circle-exclamation" />
          </div>
          <h3>Failed to load dashboard</h3>
          <p>{(statsError as Error).message}</p>
        </div>
      )}

      {/* ── Service Status ──────────────────────────── */}
      {services && services.length > 0 && (
        <section className="service-cards" data-service-status>
          <h2 className="section-title" style={{ marginBottom: "var(--space-4)" }}>
            <i className="fa-solid fa-cloud" style={{ marginRight: "var(--space-2)" }} />
            Service Status
          </h2>
          {services.map((conn) => (
            <ServiceStatusCard key={conn.service} conn={conn} />
          ))}
        </section>
      )}

      {/* ── Recent Activity ─────────────────────────── */}
      {recentFiles && recentFiles.length > 0 && (
        <section className="fade-in" data-recent-activity style={{ marginTop: "var(--space-6)" }}>
          <h2 className="section-title" style={{ marginBottom: "var(--space-4)" }}>
            <i className="fa-solid fa-clock-rotate-left" style={{ marginRight: "var(--space-2)" }} />
            Recent Activity
          </h2>
          <div className="dash-card" style={{ padding: "var(--space-3) var(--space-4)" }}>
            {recentFiles.slice(0, 5).map((file) => (
              <div
                key={file.id}
                className="dash-service-row"
                style={{ justifyContent: "space-between" }}
              >
                <span>
                  <i
                    className={`fa-solid ${getFileIcon(file.fileType)}`}
                    style={{ marginRight: "var(--space-2)", color: "var(--text-muted)" }}
                  />
                  <strong>{file.title}</strong>
                  {file.artist ? <span style={{ color: "var(--text-muted)" }}> — {file.artist}</span> : null}
                </span>
                <span style={{ color: "var(--text-subtle)", fontSize: "0.75rem" }}>
                  {file.lastScanned
                    ? timeAgo(file.lastScanned * 1000)
                    : ""}
                </span>
              </div>
            ))}
          </div>
        </section>
      )}

      {/* ── Quick Actions ───────────────────────────── */}
      <section className="action-grid" style={{ marginTop: "var(--space-6)" }}>
        <button
          className="action-card"
          data-action="sync-all"
          onClick={() => {
            // Trigger a full sync for all configured services
            services?.forEach((conn) => {
              if (conn.configured) {
                fetch(`/api/services/${conn.service}/sync`, { method: "POST" }).catch(() => {});
              }
            });
          }}
        >
          <div className="action-card-icon" style={{ background: "rgba(99,102,241,0.1)" }}>
            <i className="fa-solid fa-cloud-arrow-up" style={{ color: "var(--accent)" }} />
          </div>
          <div className="action-card-label">Sync All</div>
          <div className="action-card-desc">Refresh all services</div>
        </button>

        <a href="#/tracks" className="action-card" data-action="go-to-files">
          <div className="action-card-icon" style={{ background: "rgba(34,197,94,0.1)" }}>
            <i className="fa-solid fa-music" style={{ color: "#22c55e" }} />
          </div>
          <div className="action-card-label">Go to Tracks</div>
          <div className="action-card-desc">Browse all tracks</div>
        </a>
      </section>
    </div>
  );
}

/* ── Utility helpers ─────────────────────────────────── */

function getFileIcon(fileType: string): string {
  switch (fileType?.toLowerCase()) {
    case "mp3":
      return "fa-file-audio";
    case "flac":
      return "fa-file-audio";
    case "wav":
      return "fa-file-audio";
    case "m4a":
      return "fa-file-audio";
    case "aiff":
      return "fa-file-audio";
    default:
      return "fa-file";
  }
}

function timeAgo(ts: number): string {
  if (!ts) return "";
  const diff = Date.now() - ts;
  if (diff < 0) return "just now";
  const mins = Math.floor(diff / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d ago`;
  return new Date(ts).toLocaleDateString();
}
