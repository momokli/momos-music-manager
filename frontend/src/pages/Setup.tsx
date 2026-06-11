import {
  useSetupServices,
  useSetupFolders,
  useSetupStorageStatus,
  useSetupTasks,
  useSetupTraktorStatus,
  useSetupDeemixQueue,
  type FolderInfo,
  type StorageStatus,
  type TaskProgress,
  type TasksResponse,
  type TraktorStatus,
  type DeemixQueueResponse,
} from "../api/setup";
import type { ServiceConnection } from "../api/dashboard";

/* ── Helpers ─────────────────────────────────────────────────── */

const SERVICE_META: Record<string, { icon: string; label: string }> = {
  spotify: { icon: "fa-brands fa-spotify", label: "Spotify" },
  soundcloud: { icon: "fa-brands fa-soundcloud", label: "SoundCloud" },
  youtube: { icon: "fa-brands fa-youtube", label: "YouTube" },
  deemix: { icon: "fa-solid fa-download", label: "Deemix" },
};

function serviceMeta(service: string) {
  return (
    SERVICE_META[service] ?? {
      icon: "fa-solid fa-circle",
      label: service.charAt(0).toUpperCase() + service.slice(1),
    }
  );
}

function statusBadge({
  configured,
  connected,
}: {
  configured: boolean;
  connected: boolean;
}) {
  if (!configured)
    return (
      <span className="status-badge unconfigured" data-status="unconfigured">
        <i className="fa-solid fa-circle" /> Configure
      </span>
    );
  return connected ? (
    <span className="status-badge connected" data-status="connected">
      <i className="fa-solid fa-circle-check" /> Connected
    </span>
  ) : (
    <span className="status-badge disconnected" data-status="disconnected">
      <i className="fa-solid fa-circle-exclamation" /> Offline
    </span>
  );
}

function taskStatusBadge(status: string) {
  const cls = status.toLowerCase();
  return (
    <span className={`status-badge ${cls}`} data-task-status>
      {status}
    </span>
  );
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(1024));
  const val = bytes / Math.pow(1024, i);
  return `${val < 10 ? val.toFixed(1) : Math.round(val)} ${units[i]}`;
}

function formatCount(n: number): string {
  return n.toLocaleString();
}

function timeAgo(ts: number | null | undefined): string {
  if (!ts) return "";
  const now = Date.now() / 1000;
  const diff = now - ts;
  if (diff < 0) return "just now";
  const mins = Math.floor(diff / 60);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d ago`;
  return new Date(ts * 1000).toLocaleDateString();
}

/* ── Card Wrapper ────────────────────────────────────────────── */

function SetupCard({
  cardKey,
  icon,
  title,
  children,
}: {
  cardKey: string;
  icon: string;
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div className="setup-card" data-setup-card={cardKey}>
      <div className="setup-card-header">
        <div className="setup-card-header-icon">
          <i className={`fa-solid ${icon}`} />
        </div>
        <h3 className="setup-card-title">{title}</h3>
      </div>
      <div className="setup-card-body">{children}</div>
    </div>
  );
}

/* ── Card Content Components ─────────────────────────────────── */

function ServicesContent({
  services,
  isLoading,
}: {
  services: ServiceConnection[] | undefined;
  isLoading: boolean;
}) {
  if (isLoading)
    return (
      <div className="loading">
        <div className="spinner" />
      </div>
    );
  if (!services || services.length === 0)
    return <p className="text-muted">No services configured.</p>;

  return (
    <div className="setup-services-list">
      {services.map((conn) => {
        const meta = serviceMeta(conn.service);
        return (
          <div
            className="setup-service-row"
            key={conn.service}
            data-service={conn.service}
          >
            <span className="setup-service-name">
              <i className={meta.icon} /> {meta.label}
            </span>
            <div className="setup-service-status-wrap">
              {statusBadge({
                configured: conn.configured,
                connected: conn.connected,
              })}
              {conn.configured && (
                <span className="setup-service-meta">
                  {formatCount(conn.tracksLocal)} tracks /{" "}
                  {formatCount(conn.playlistsLocal)} playlists
                </span>
              )}
            </div>
          </div>
        );
      })}
      <button
        className="btn btn-sm btn-primary setup-card-action"
        data-action="resync-all"
        style={{ marginTop: "var(--space-3)" }}
        onClick={() => {
          services.forEach((conn) => {
            if (conn.configured) {
              fetch(`/api/services/${conn.service}/sync`, {
                method: "POST",
              }).catch(() => {});
            }
          });
        }}
      >
        <i className="fa-solid fa-cloud-arrow-up" /> Resync All
      </button>
    </div>
  );
}

function FoldersContent({
  folders,
  isLoading,
}: {
  folders: FolderInfo[] | undefined;
  isLoading: boolean;
}) {
  if (isLoading)
    return (
      <div className="loading">
        <div className="spinner" />
      </div>
    );
  if (!folders || folders.length === 0)
    return <p className="text-muted">No folders configured.</p>;

  return (
    <div className="setup-folders-list">
      {folders.map((folder) => (
        <div className="setup-folder-row" key={folder.id}>
          <div className="setup-folder-info">
            <span className="setup-folder-path" title={folder.path}>
              <i className="fa-solid fa-folder-open" /> {folder.path}
            </span>
            <span className="setup-folder-meta">
              {formatCount(folder.fileCount)} files
              {folder.lastScanned
                ? ` · scanned ${timeAgo(folder.lastScanned)}`
                : " · never scanned"}
            </span>
          </div>
          <button
            className="btn btn-xs btn-icon"
            data-action="scan-folder"
            title="Scan folder"
            onClick={() => {
              fetch(`/api/folders/${folder.id}/scan`, {
                method: "POST",
              }).catch(() => {});
            }}
          >
            <i className="fa-solid fa-rotate" />
          </button>
        </div>
      ))}
    </div>
  );
}

function StorageContent({
  storage,
  isLoading,
}: {
  storage: StorageStatus | undefined;
  isLoading: boolean;
}) {
  if (isLoading)
    return (
      <div className="loading">
        <div className="spinner" />
      </div>
    );
  if (!storage) return <p className="text-muted">Storage status unavailable.</p>;

  return (
    <div className="setup-storage-stats">
      <div className="setup-stat-row" data-stat="local-files">
        <span className="setup-stat-label">
          <i className="fa-solid fa-file-audio" /> Local Files
        </span>
        <span className="setup-stat-value">{formatCount(storage.localFileCount)}</span>
      </div>
      <div className="setup-stat-row" data-stat="backed-up">
        <span className="setup-stat-label">
          <i className="fa-solid fa-cloud-arrow-up" /> Backed Up
        </span>
        <span className="setup-stat-value">{formatCount(storage.backupCount)}</span>
      </div>
      <div className="setup-stat-row">
        <span className="setup-stat-label">
          <i className="fa-solid fa-weight-hanging" /> Total Size
        </span>
        <span className="setup-stat-value">{formatBytes(storage.localSizeBytes)}</span>
      </div>
      <div className="setup-stat-row">
        <span className="setup-stat-label">
          <i className="fa-solid fa-trash-can" /> Prune Candidates
        </span>
        <span className="setup-stat-value">
          {formatCount(storage.pruneCandidateCount)}
        </span>
      </div>
      {storage.pruneCandidateBytes > 0 && (
        <p className="setup-storage-hint">
          {formatBytes(storage.pruneCandidateBytes)} reclaimable
        </p>
      )}
    </div>
  );
}

function TasksContent({
  tasksData,
  isLoading,
}: {
  tasksData: TasksResponse | undefined;
  isLoading: boolean;
}) {
  if (isLoading)
    return (
      <div className="loading">
        <div className="spinner" />
      </div>
    );
  if (!tasksData || tasksData.tasks.length === 0)
    return <p className="text-muted">No recent tasks.</p>;

  return (
    <div className="setup-tasks-list">
      {tasksData.tasks.slice(0, 5).map((task) => (
        <div className="setup-task-row" key={task.id}>
          <div className="setup-task-info">
            <span className="setup-task-type">{task.task_type}</span>
            <span className="setup-task-progress">{task.progress}</span>
          </div>
          <div className="setup-task-right">{taskStatusBadge(task.status)}</div>
        </div>
      ))}
      {tasksData.total > 5 && (
        <p className="text-muted text-sm" style={{ marginTop: "var(--space-2)" }}>
          +{tasksData.total - 5} more tasks
        </p>
      )}
    </div>
  );
}

function DataContent() {
  return (
    <div className="setup-data-actions">
      <p className="text-muted text-sm" style={{ marginBottom: "var(--space-3)" }}>
        Export or restore your database.
      </p>
      <div className="setup-btn-group">
        <button
          className="btn btn-sm btn-primary"
          data-action="export"
          onClick={() => {
            window.open("/api/dump", "_blank");
          }}
        >
          <i className="fa-solid fa-download" /> Export
        </button>
        <button
          className="btn btn-sm btn-outline"
          data-action="import"
          onClick={() => {
            const input = document.createElement("input");
            input.type = "file";
            input.accept = ".json";
            input.onchange = async () => {
              const file = input.files?.[0];
              if (!file) return;
              const form = new FormData();
              form.append("file", file);
              await fetch("/api/restore", { method: "POST", body: form });
              window.location.reload();
            };
            input.click();
          }}
        >
          <i className="fa-solid fa-upload" /> Import
        </button>
      </div>
    </div>
  );
}

function DeemixContent({
  queue,
  isLoading,
}: {
  queue: DeemixQueueResponse | undefined;
  isLoading: boolean;
}) {
  if (isLoading)
    return (
      <div className="loading">
        <div className="spinner" />
      </div>
    );
  const count = queue?.total ?? queue?.items?.length ?? 0;

  return (
    <div className="setup-deemix-info">
      <p className="setup-deemix-count">
        <strong>{count}</strong> {count === 1 ? "track" : "tracks"} in queue
      </p>
      <p className="text-muted text-sm">Manage downloads via Deemix queue page.</p>
    </div>
  );
}

function TraktorContent({
  status,
  isLoading,
}: {
  status: TraktorStatus | undefined;
  isLoading: boolean;
}) {
  if (isLoading)
    return (
      <div className="loading">
        <div className="spinner" />
      </div>
    );

  return (
    <div className="setup-traktor-info">
      {status?.path ? (
        <>
          <p className="setup-traktor-path" title={status.path}>
            <i className="fa-solid fa-file" /> {status.path}
          </p>
          <p className="text-muted text-sm">
            Last modified: {timeAgo(status.modifiedAt)}
          </p>
        </>
      ) : (
        <p className="text-muted">
          <i className="fa-solid fa-circle-exclamation" /> No collection found.
        </p>
      )}
      <button
        className="btn btn-sm btn-outline"
        data-action="import-traktor"
        style={{ marginTop: "var(--space-3)" }}
        onClick={() => {
          const input = document.createElement("input");
          input.type = "file";
          input.accept = ".nml,.xml";
          input.onchange = async () => {
            const file = input.files?.[0];
            if (!file) return;
            const form = new FormData();
            form.append("file", file);
            await fetch("/api/traktor/import", {
              method: "POST",
              body: form,
            });
            window.location.reload();
          };
          input.click();
        }}
      >
        <i className="fa-solid fa-upload" /> Import Collection
      </button>
    </div>
  );
}

function KeyComparisonContent() {
  return (
    <div className="setup-key-compare-info">
      <p className="text-muted text-sm" style={{ marginBottom: "var(--space-3)" }}>
        Compare BPM and Key between your Traktor collection and Spotify tracks.
      </p>
      <button
        className="btn btn-sm btn-primary"
        data-action="key-compare"
        onClick={() => {
          window.location.hash = "#/key-comparison";
        }}
      >
        <i className="fa-solid fa-arrow-right" /> Open Comparison
      </button>
    </div>
  );
}

/* ── Main Setup Page ────────────────────────────────────────── */

export default function Setup() {
  const { data: services, isLoading: servicesLoading } = useSetupServices();
  const { data: folders, isLoading: foldersLoading } = useSetupFolders();
  const { data: storage, isLoading: storageLoading } = useSetupStorageStatus();
  const { data: tasks, isLoading: tasksLoading } = useSetupTasks();
  const { data: traktor, isLoading: traktorLoading } = useSetupTraktorStatus();
  const { data: deemix, isLoading: deemixLoading } = useSetupDeemixQueue();

  return (
    <div data-page="setup">
      <div className="page-header">
        <h1>
          <i className="fa-solid fa-gear" /> Setup
        </h1>
        <p className="subtitle">Manage services, folders, storage, and more.</p>
      </div>

      <div className="setup-card-grid">
        <SetupCard cardKey="services" icon="fa-cloud" title="Services">
          <ServicesContent services={services} isLoading={servicesLoading} />
        </SetupCard>

        <SetupCard cardKey="folders" icon="fa-folder-tree" title="Folders">
          <FoldersContent folders={folders} isLoading={foldersLoading} />
        </SetupCard>

        <SetupCard cardKey="storage" icon="fa-hard-drive" title="Storage">
          <StorageContent storage={storage} isLoading={storageLoading} />
        </SetupCard>

        <SetupCard cardKey="tasks" icon="fa-list-check" title="Tasks">
          <TasksContent tasksData={tasks} isLoading={tasksLoading} />
        </SetupCard>

        <SetupCard cardKey="deemix" icon="fa-download" title="Deemix Queue">
          <DeemixContent queue={deemix} isLoading={deemixLoading} />
        </SetupCard>

        <SetupCard cardKey="traktor" icon="fa-compact-disc" title="Traktor Import">
          <TraktorContent status={traktor} isLoading={traktorLoading} />
        </SetupCard>

        <SetupCard cardKey="data" icon="fa-database" title="Import / Export">
          <DataContent />
        </SetupCard>

        <SetupCard cardKey="key-comparison" icon="fa-code-compare" title="Key Comparison">
          <KeyComparisonContent />
        </SetupCard>
      </div>
    </div>
  );
}
