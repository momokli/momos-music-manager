"""FastAPI Download Service - HTTP API for downloading Spotify tracks via deemix.

Endpoints:
    POST /download              - single Spotify track
    POST /download/playlist     - resolve playlist to tracks, download all
    GET  /download/{id}         - check status
    GET  /queue                 - list all tasks
    GET  /search                - Spotify search (for guest UI)
    GET  /stats                 - status counts
    GET  /health                - health check
    GET  /                      - guest request page
"""

import os
import threading
import time
import uuid
from pathlib import Path

from config_loader import Config, load_config
from fastapi import FastAPI, HTTPException, Query
from fastapi.responses import FileResponse
from fastapi.staticfiles import StaticFiles
from pipeline import DownloadPipeline, DownloadTask, PipelineStage
from pydantic import BaseModel

import db

app = FastAPI(title="Download Service", version="0.2.0")

STATIC_DIR = os.path.join(os.path.dirname(__file__), "static")
if os.path.isdir(STATIC_DIR):
    app.mount("/static", StaticFiles(directory=STATIC_DIR), name="static")

_config_path = os.environ.get("DOWNLOAD_SERVICE_CONFIG")
_config = load_config(_config_path)
Path(_config.downloads.output_dir).mkdir(parents=True, exist_ok=True)

_pipeline = DownloadPipeline(_config)


class DownloadResponse(BaseModel):
    id: str
    spotify_id: str = ""
    status: str
    title: str | None = None
    artist: str | None = None
    cover_url: str | None = None
    source: str | None = None
    file_path: str | None = None
    file_size: int | None = None
    error: str | None = None


class PlaylistResponse(BaseModel):
    playlist_name: str
    track_count: int
    tasks: list[DownloadResponse]


class QueueResponse(BaseModel):
    total: int
    ready: int
    failed: int
    pending: int
    tasks: list[DownloadResponse]


class StatsResponse(BaseModel):
    total: int
    ready: int
    failed: int
    pending: int


class HealthResponse(BaseModel):
    status: str
    deemix_arl_configured: bool
    spotify_configured: bool
    spotdl_available: bool
    output_dir: str
    db_path: str


# ── Helpers ──────────────────────────────────────────────────────────


def _to_response(row: dict) -> DownloadResponse:
    return DownloadResponse(
        id=row["id"],
        spotify_id=row.get("spotify_id", ""),
        status=row["status"],
        title=row.get("title"),
        artist=row.get("artist"),
        cover_url=row.get("cover_url"),
        source=row.get("source"),
        file_path=row.get("file_path"),
        file_size=row.get("file_size"),
        error=row.get("error"),
    )


def _run_pipeline(
    task_id: str,
    spotify_url: str,
    spotify_id: str = "",
    meta_title: str = "",
    meta_artist: str = "",
    cover_url: str = "",
    isrc: str = "",
) -> None:
    """Run full pipeline and persist results to SQLite."""
    task = DownloadTask(task_id=task_id, spotify_url=spotify_url)
    if meta_title:
        from spotify_client import TrackMetadata

        task.metadata = TrackMetadata(
            isrc=isrc or None,
            title=meta_title,
            artist=meta_artist,
            album="",
            cover_url=cover_url or None,
            duration_ms=0,
            spotify_url=spotify_url,
        )
    try:
        task.started_at = time.time()
        _pipeline.process(task)
    except Exception as e:
        task.status = PipelineStage.FAILED
        task.stage_errors.append(f"Pipeline crashed: {e}")

    db.update_task(
        task_id,
        status=task.status.value,
        title=task.metadata.title if task.metadata else None,
        artist=task.metadata.artist if task.metadata else None,
        cover_url=task.metadata.cover_url if task.metadata else cover_url or None,
        source=task.download_source or None,
        file_path=task.file_path,
        file_size=os.path.getsize(task.file_path)
        if task.file_path and os.path.exists(task.file_path)
        else None,
        error="; ".join(task.stage_errors) if task.stage_errors else None,
        completed_at=time.time(),
    )


# ── Endpoints ────────────────────────────────────────────────────────


@app.get("/")
def serve_request_page():
    index_path = os.path.join(STATIC_DIR, "request.html")
    if os.path.isfile(index_path):
        return FileResponse(index_path)
    return {"message": "Download Service API"}


@app.get("/search")
def search_tracks(q: str = Query(..., min_length=2), limit: int = Query(default=3, le=5)):
    """Search Spotify + YouTube + SoundCloud. Interleaved: S1,Y1,SC1,S2,Y2,SC2,..."""
    from spotify_client import SpotifyClient
    spotify = []
    try:
        s = SpotifyClient(_config.spotify)
        for t in s.search_tracks(q, limit):
            tid = t.spotify_url.replace("https://open.spotify.com/track/", "")
            spotify.append({"id": tid, "source": "spotify", "title": t.title, "artist": t.artist, "coverUrl": t.cover_url, "durationMs": t.duration_ms, "spotifyUrl": t.spotify_url})
    except Exception:
        pass

    youtube = _search_youtube(q, limit)
    soundcloud = _search_soundcloud(q, limit)

    # Interleave: S1, Y1, SC1, S2, Y2, SC2, ...
    results = []
    for i in range(max(len(spotify), len(youtube), len(soundcloud))):
        if i < len(spotify): results.append(spotify[i])
        if i < len(youtube): results.append(youtube[i])
        if i < len(soundcloud): results.append(soundcloud[i])
    return {"results": results}



def _search_soundcloud(query: str, limit: int) -> list[dict]:
    """Search SoundCloud via yt-dlp."""
    import json as _json, subprocess
    try:
        r = subprocess.run(["python3", "-m", "yt_dlp", "scsearch"+str(limit)+":"+query, "--flat-playlist", "-J", "--no-playlist"], capture_output=True, text=True, timeout=15, env={**__import__("os").environ, "PATH": "/srv/download-service/.venv/bin:"+__import__("os").environ.get("PATH","")})
        if r.returncode != 0: return []
        data = _json.loads(r.stdout)
        items = []
        for e in (data.get("entries") or [])[:limit]:
            items.append({"id": e.get("id",""), "source": "soundcloud", "title": e.get("title",""), "artist": e.get("uploader","") or e.get("channel",""), "coverUrl": (e.get("thumbnails") or [{}])[0].get("url",""), "durationMs": int(e.get("duration",0)*1000) if e.get("duration") else 0, "sourceUrl": e.get("webpage_url","") or e.get("url","")})
        return items
    except Exception:
        return []

def _search_youtube(query: str, limit: int) -> list[dict]:
    """Search YouTube via yt-dlp."""
    import json as _json, subprocess
    try:
        r = subprocess.run(["yt-dlp", "ytsearch"+str(limit)+":"+query, "--flat-playlist", "-j", "--no-playlist"], capture_output=True, text=True, timeout=15)
        if r.returncode != 0: return []
        items = []
        for line in r.stdout.strip().split("\n"):
            if not line: continue
            try: v = _json.loads(line)
            except: continue
            thumb = (v.get("thumbnails") or [{}])[0].get("url", "")
            items.append({"id": v.get("id",""), "source": "youtube", "title": v.get("title",""), "artist": v.get("channel","") or v.get("uploader",""), "coverUrl": thumb, "durationMs": int(v.get("duration",0)*1000) if v.get("duration") else 0, "sourceUrl": "https://www.youtube.com/watch?v="+v.get("id","") if v.get("id") else ""})
        return items
    except Exception:
        return []


@app.post("/download", response_model=DownloadResponse)
def download(request: dict) -> DownloadResponse:
    url = request.get("url", "")
    if not url:
        raise HTTPException(status_code=400, detail="url required")

    tid = str(uuid.uuid4())
    sid = url.replace("https://open.spotify.com/track/", "").split("?")[0]

    # Check if already downloaded
    if db.task_exists(sid):
        raise HTTPException(status_code=409, detail="Already downloaded")

    db.insert_task(tid, url, spotify_id=sid)
    threading.Thread(target=_run_pipeline, args=(tid, url, sid), daemon=True).start()
    return DownloadResponse(id=tid, spotify_id=sid, status="pending")


@app.post("/download/playlist", response_model=PlaylistResponse)
def download_playlist(request: dict) -> PlaylistResponse:
    url = request.get("url", "")
    if not url:
        raise HTTPException(status_code=400, detail="url required")

    from spotify_client import SpotifyClient

    spotify = SpotifyClient(_config.spotify)
    try:
        tracks = spotify.get_playlist_tracks(url)
    except Exception as e:
        raise HTTPException(status_code=502, detail=f"Failed to fetch playlist: {e}")
    if not tracks:
        raise HTTPException(status_code=404, detail="Playlist empty or not found")

    # Get playlist name
    pid = spotify._extract_id(url)
    try:
        data = spotify._get(f"/v1/playlists/{pid}?fields=name")
        playlist_name = data.get("name", pid)
    except Exception:
        playlist_name = pid

    tasks = []
    for t in tracks:
        sid = t.spotify_url.replace("https://open.spotify.com/track/", "").split("?")[0]
        if db.task_exists(sid):
            continue
        tid = str(uuid.uuid4())
        db.insert_task(
            tid,
            t.spotify_url,
            spotify_id=sid,
            title=t.title,
            artist=t.artist,
            cover_url=t.cover_url or "",
            isrc=t.isrc or "",
        )
        threading.Thread(
            target=_run_pipeline,
            args=(
                tid,
                t.spotify_url,
                sid,
                t.title,
                t.artist,
                t.cover_url or "",
                t.isrc or "",
            ),
            daemon=True,
        ).start()
        tasks.append(
            DownloadResponse(
                id=tid, spotify_id=sid, status="pending", title=t.title, artist=t.artist
            )
        )

    return PlaylistResponse(
        playlist_name=playlist_name, track_count=len(tasks), tasks=tasks
    )


@app.get("/download/{task_id}", response_model=DownloadResponse)
def download_status(task_id: str) -> DownloadResponse:
    row = db.get_task(task_id)
    if not row:
        raise HTTPException(status_code=404, detail="Task not found")
    return _to_response(row)


@app.get("/queue", response_model=QueueResponse)
def queue() -> QueueResponse:
    tasks = db.list_tasks(limit=500)
    counts = db.count_by_status()
    return QueueResponse(
        total=len(tasks),
        ready=counts.get("ready", 0),
        failed=counts.get("failed", 0),
        pending=counts.get("pending", 0)
        + counts.get("stage1_metadata", 0)
        + counts.get("stage2_deemix", 0)
        + counts.get("stage3_spotdl", 0)
        + counts.get("stage4_tagging", 0),
        tasks=[_to_response(r) for r in tasks],
    )


@app.get("/stats", response_model=StatsResponse)
def stats() -> StatsResponse:
    counts = db.count_by_status()
    return StatsResponse(
        total=sum(counts.values()),
        ready=counts.get("ready", 0),
        failed=counts.get("failed", 0),
        pending=sum(v for k, v in counts.items() if k not in ("ready", "failed")),
    )


@app.get("/health", response_model=HealthResponse)
def health() -> HealthResponse:
    import shutil

    return HealthResponse(
        status="ok",
        deemix_arl_configured=bool(_config.deemix.arl),
        spotify_configured=bool(
            _config.spotify.client_id and _config.spotify.client_secret
        ),
        spotdl_available=shutil.which(_config.spotdl.executable) is not None,
        output_dir=_config.downloads.output_dir,
        db_path=db.DB_PATH,
    )
