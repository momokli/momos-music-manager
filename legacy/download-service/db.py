"""SQLite task store - persistent download tracking."""

import os
import sqlite3
import threading
import time
from dataclasses import dataclass

DB_PATH = os.environ.get(
    "DOWNLOAD_DB_PATH", os.path.join(os.path.dirname(__file__), "downloads.db")
)

_lock = threading.Lock()


def get_db() -> sqlite3.Connection:
    db = sqlite3.connect(DB_PATH)
    db.row_factory = sqlite3.Row
    db.execute("PRAGMA journal_mode=WAL")
    db.execute("PRAGMA busy_timeout=5000")
    _migrate(db)
    return db


def _migrate(db: sqlite3.Connection) -> None:
    db.execute("""
        CREATE TABLE IF NOT EXISTS tasks (
            id TEXT PRIMARY KEY,
            spotify_id TEXT,
            youtube_id TEXT,
            soundcloud_id TEXT,
            spotify_url TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            title TEXT,
            artist TEXT,
            album TEXT,
            cover_url TEXT,
            isrc TEXT,
            source TEXT,
            file_path TEXT,
            file_size INTEGER,
            error TEXT,
            retries INTEGER DEFAULT 0,
            created_at REAL NOT NULL,
            completed_at REAL
        )
    """)
    db.execute("CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status)")
    db.execute("CREATE INDEX IF NOT EXISTS idx_tasks_spotify_id ON tasks(spotify_id)")
    db.execute("CREATE INDEX IF NOT EXISTS idx_tasks_created ON tasks(created_at)")
    db.commit()


def insert_task(
    task_id: str,
    spotify_url: str,
    spotify_id: str = "",
    title: str = "",
    artist: str = "",
    cover_url: str = "",
    isrc: str = "",
) -> None:
    with _lock:
        db = get_db()
        db.execute(
            """
            INSERT OR REPLACE INTO tasks (id, spotify_id, spotify_url, status,
                title, artist, cover_url, isrc, created_at)
            VALUES (?, ?, ?, 'pending', ?, ?, ?, ?, ?)
        """,
            (
                task_id,
                spotify_id,
                spotify_url,
                title,
                artist,
                cover_url,
                isrc,
                time.time(),
            ),
        )
        db.commit()
        db.close()


def update_task(task_id: str, **kwargs) -> None:
    if not kwargs:
        return
    sets = [f"{k} = ?" for k in kwargs]
    vals = list(kwargs.values()) + [task_id]
    with _lock:
        db = get_db()
        db.execute(f"UPDATE tasks SET {', '.join(sets)} WHERE id = ?", vals)
        db.commit()
        db.close()


def get_task(task_id: str) -> dict | None:
    db = get_db()
    row = db.execute("SELECT * FROM tasks WHERE id = ?", (task_id,)).fetchone()
    db.close()
    return dict(row) if row else None


def list_tasks(status: str | None = None, limit: int = 200) -> list[dict]:
    db = get_db()
    if status:
        rows = db.execute(
            "SELECT * FROM tasks WHERE status = ? ORDER BY created_at DESC LIMIT ?",
            (status, limit),
        ).fetchall()
    else:
        rows = db.execute(
            "SELECT * FROM tasks ORDER BY created_at DESC LIMIT ?", (limit,)
        ).fetchall()
    db.close()
    return [dict(r) for r in rows]


def count_by_status() -> dict[str, int]:
    db = get_db()
    rows = db.execute(
        "SELECT status, COUNT(*) as cnt FROM tasks GROUP BY status"
    ).fetchall()
    db.close()
    return {r["status"]: r["cnt"] for r in rows}


def task_exists(spotify_id: str) -> bool:
    db = get_db()
    row = db.execute(
        "SELECT 1 FROM tasks WHERE spotify_id = ? AND status = 'ready'", (spotify_id,)
    ).fetchone()
    db.close()
    return row is not None


def deduplicate_tasks() -> int:
    """Remove duplicate ready tasks (same spotify_id), keep newest."""
    with _lock:
        db = get_db()
        # Delete older duplicates
        db.execute("""
            DELETE FROM tasks WHERE id IN (
                SELECT t1.id FROM tasks t1
                JOIN tasks t2 ON t1.spotify_id = t2.spotify_id
                WHERE t1.spotify_id != ''
                  AND t1.status = 'ready'
                  AND t2.status = 'ready'
                  AND t1.created_at < t2.created_at
            )
        """)
        removed = db.total_changes
        db.commit()
        db.close()
        return removed
