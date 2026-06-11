import { useState, useRef, useEffect } from "react";
import {
  useBackpackTags,
  useBackpackSizeStats,
  useSyncBackpack,
  useTaskPoll,
  type BackpackSizeStats,
} from "../api/backpack";

/* ─── Helpers ────────────────────────────────────── */

function formatBytes(bytes: number): string {
  if (!bytes || bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}

function formatEta(secs: number | null): string {
  if (secs == null || secs <= 0 || !isFinite(secs)) return "";
  if (secs < 60) return " (< 1 min)";
  const mins = Math.round(secs / 60);
  if (mins < 60) return ` (~${mins} min)`;
  const hours = Math.floor(mins / 60);
  const remain = mins % 60;
  if (remain === 0) return ` (~${hours}h)`;
  return ` (~${hours}h ${remain}min)`;
}

/* ─── Sync Progress Overlay ──────────────────────── */

function SyncProgress({
  taskId,
  onComplete,
}: {
  taskId: string;
  onComplete: () => void;
}) {
  const { data: task, isError } = useTaskPoll(taskId);

  const status = task?.status;
  useEffect(() => {
    if (status === "completed" || status === "failed" || status === "cancelled") {
      const timer = setTimeout(onComplete, 2000);
      return () => clearTimeout(timer);
    }
  }, [status, onComplete]);

  if (isError || status === "failed" || status === "cancelled") {
    return (
      <div className="backpack-sync-status error">
        <i className="fa-solid fa-circle-exclamation" /> {task?.progress || "Sync failed"}
      </div>
    );
  }

  if (status === "completed") {
    return (
      <div className="backpack-sync-status done">
        <i className="fa-solid fa-check-circle" /> Sync complete
      </div>
    );
  }

  return (
    <div className="backpack-sync-status running">
      <i className="fa-solid fa-spinner fa-spin" /> {task?.progress || "Syncing..."}
    </div>
  );
}

/* ─── Size Stats Bar ─────────────────────────────── */

function SizeStatsCards({ stats }: { stats: BackpackSizeStats }) {
  const [rateInfo, setRateInfo] = useState<{
    lastLocalBytes: number;
    lastTime: number;
  }>({ lastLocalBytes: 0, lastTime: 0 });

  // Track rate from polled stats
  useEffect(() => {
    if (stats.localBytes > 0) {
      setRateInfo((prev) => {
        if (prev.lastTime === 0) {
          return { lastLocalBytes: stats.localBytes, lastTime: Date.now() };
        }
        return prev;
      });
    }
  }, [stats.localBytes]);

  const percent =
    stats.targetBytes > 0
      ? Math.round((stats.localBytes / stats.targetBytes) * 100)
      : 100;

  let etaSecs: number | null = null;
  if (stats.needsPullBytes > 0 && rateInfo.lastTime > 0) {
    const elapsed = (Date.now() - rateInfo.lastTime) / 1000;
    if (elapsed > 2) {
      const pulled = stats.localBytes - rateInfo.lastLocalBytes;
      if (pulled > 0) {
        const rate = pulled / elapsed;
        etaSecs = stats.needsPullBytes > 0 ? stats.needsPullBytes / rate : 0;
      }
    }
  }

  return (
    <div className="backpack-size-cards">
      <div className="backpack-size-card">
        <div className="backpack-size-value">{formatBytes(stats.localBytes)}</div>
        <div className="backpack-size-bar">
          <div className="backpack-size-bar-fill" style={{ width: `${percent}%` }} />
        </div>
        <div className="backpack-size-label">On Disk ({percent}%)</div>
      </div>
      <div className="backpack-size-card">
        <div className="backpack-size-value">{formatBytes(stats.targetBytes)}</div>
        <div className="backpack-size-label">Target (fully synced)</div>
      </div>
      {stats.needsPullBytes > 0 ? (
        <div className="backpack-size-remaining">
          <span className="backpack-pulse-dot" /> {formatBytes(stats.needsPullBytes)}{" "}
          remaining to pull
          {etaSecs ? formatEta(etaSecs) : ""}
        </div>
      ) : (
        <div
          className="backpack-size-done"
          style={{
            color: "var(--green)",
            textAlign: "center",
            marginTop: "0.5rem",
            fontSize: "0.9rem",
          }}
        >
          ✓ Fully synced
        </div>
      )}
    </div>
  );
}

/* ─── Tag Card ───────────────────────────────────── */

function BackpackTagCard({
  name,
  icon,
  fileCount,
}: {
  name: string;
  icon: string | null;
  fileCount: number;
}) {
  return (
    <div className="backpack-tag-card">
      <span className="backpack-tag-icon">
        <i className={`${icon || "fa-solid fa-tag"}`} />
      </span>
      <span className="backpack-tag-name">{name}</span>
      <span className="backpack-tag-count">
        {fileCount} {fileCount === 1 ? "track" : "tracks"}
      </span>
    </div>
  );
}

/* ─── Main Backpack Page ─────────────────────────── */

export default function Backpack() {
  const {
    data: tags = [],
    isLoading: tagsLoading,
    isError: tagsError,
    error: tagsErr,
  } = useBackpackTags();
  const { data: sizeStats, isLoading: statsLoading } = useBackpackSizeStats();

  const syncMutation = useSyncBackpack();
  const [syncTaskId, setSyncTaskId] = useState<string | null>(null);

  const totalTracks = tags.reduce((sum, t) => sum + (t.fileCount || 0), 0);

  const handleSync = () => {
    syncMutation.mutate(undefined, {
      onSuccess: (data) => {
        if (data.taskId) {
          setSyncTaskId(data.taskId);
        }
      },
    });
  };

  const handleSyncComplete = () => {
    setSyncTaskId(null);
  };

  return (
    <div data-page="backpack">
      <div className="page-header">
        <h1>
          <i className="fa-solid fa-box" /> Backpack
        </h1>
      </div>

      {/* ── Sync Progress ── */}
      {syncTaskId && <SyncProgress taskId={syncTaskId} onComplete={handleSyncComplete} />}
      {syncMutation.isError && (
        <div className="error-message">Sync failed: {syncMutation.error.message}</div>
      )}

      {/* ── Size Stats ── */}
      {!statsLoading && sizeStats && sizeStats.trackCount > 0 && (
        <SizeStatsCards stats={sizeStats} />
      )}

      {/* ── Summary Bar ── */}
      <div className="backpack-summary">
        <div className="backpack-stat">
          <span className="backpack-stat-value">{tags.length}</span>
          <span className="backpack-stat-label">Tags</span>
        </div>
        <div className="backpack-stat">
          <span className="backpack-stat-value">{totalTracks}</span>
          <span className="backpack-stat-label">Tracks</span>
        </div>
      </div>

      {/* ── Action Buttons ── */}
      <div
        className="backpack-actions"
        style={{ display: "flex", gap: "0.75rem", marginBottom: "1rem" }}
      >
        <button
          className="btn btn-primary"
          data-action="sync-backpack"
          disabled={syncMutation.isPending || tags.length === 0}
          onClick={handleSync}
        >
          {syncMutation.isPending ? (
            <>
              <i className="fa-solid fa-spinner fa-spin" /> Syncing...
            </>
          ) : (
            <>
              <i className="fa-solid fa-sync" /> Sync Backpack
            </>
          )}
        </button>
        <button
          className="btn btn-outline"
          data-action="pull-missing"
          disabled={syncMutation.isPending || tags.length === 0}
          onClick={handleSync}
        >
          <i className="fa-solid fa-download" /> Pull Missing
        </button>
      </div>

      {/* ── Backpack Tags Section ── */}
      {tagsLoading ? (
        <div className="loading">
          <div className="spinner" />
        </div>
      ) : tagsError ? (
        <div className="error-message">
          <i className="fa-solid fa-triangle-exclamation" /> Failed to load:{" "}
          {(tagsErr as Error).message}
        </div>
      ) : tags.length === 0 ? (
        <div className="text-muted" style={{ padding: "1rem" }}>
          No backpack tags. Toggle "Backpack" on a tag in the Tags page.
        </div>
      ) : (
        <div className="backpack-section" data-backpack-tags>
          <h2 className="section-title">
            <i className="fa-solid fa-tags" /> Backpack Tags
          </h2>
          <div className="backpack-tags-list">
            {tags.map((tag) => (
              <BackpackTagCard
                key={tag.id}
                name={tag.name}
                icon={tag.categoryIcon}
                fileCount={tag.fileCount}
              />
            ))}
          </div>
        </div>
      )}

      {/* ── Track Status Section ── */}
      {sizeStats && sizeStats.tagCount > 0 && (
        <div
          className="backpack-section"
          data-track-status
          style={{ marginTop: "1.5rem" }}
        >
          <h2 className="section-title">
            <i className="fa-solid fa-file-audio" /> Sync Status
          </h2>
          <div className="backpack-tag-card">
            <span className="backpack-tag-name">
              <i className="fa-solid fa-hard-drive" /> Local files
            </span>
            <span className="backpack-tag-count">
              {formatBytes(sizeStats.localBytes)}
            </span>
          </div>
          <div className="backpack-tag-card">
            <span className="backpack-tag-name">
              <i className="fa-solid fa-cloud" /> Remote pending
            </span>
            <span className="backpack-tag-count">
              {formatBytes(sizeStats.needsPullBytes)}
            </span>
          </div>
        </div>
      )}
    </div>
  );
}
