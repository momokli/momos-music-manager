"""Pipeline Orchestrator - chains Stage 1->2->3->4."""

import os
import threading
import time
from dataclasses import dataclass, field
from enum import Enum

from config_loader import Config
from deemix_client import DeemixDownloader
from spotdl_client import SpotdlDownloader
from spotify_client import SpotifyClient, TrackMetadata
from tagger import tag_track

MIN_FILE_SIZE = 500_000  # 500KB minimum for a real MP3
DOWNLOAD_SEM = threading.Semaphore(3)  # max 3 concurrent CLI downloads


class PipelineStage(str, Enum):
    PENDING = "pending"
    STAGE1_METADATA = "stage1_metadata"
    STAGE2_DEEMIX = "stage2_deemix"
    STAGE3_SPOTDL = "stage3_spotdl"
    STAGE4_TAGGING = "stage4_tagging"
    READY = "ready"
    FAILED = "failed"


@dataclass
class DownloadTask:
    task_id: str
    spotify_url: str
    status: PipelineStage = PipelineStage.PENDING
    metadata: TrackMetadata | None = None
    download_source: str = ""
    file_path: str | None = None
    stage_errors: list[str] = field(default_factory=list)
    started_at: float = 0.0
    completed_at: float | None = None


class DownloadPipeline:
    def __init__(self, config: Config) -> None:
        self._spotify = SpotifyClient(config.spotify)
        self._deemix = DeemixDownloader(config.deemix, config.downloads)
        self._spotdl = SpotdlDownloader(config.spotdl, config.downloads)

    def process(self, task: DownloadTask) -> DownloadTask:
        task.started_at = time.time()

        # Stage 1: Metadata (skip if pre-populated)
        if task.metadata is None:
            try:
                task.status = PipelineStage.STAGE1_METADATA
                task.metadata = self._spotify.get_track(task.spotify_url)
                print(
                    f"[pipeline] Stage 1 OK: '{task.metadata.artist} - {task.metadata.title}'"
                )
            except Exception as e:
                task.stage_errors.append(f"Stage 1 (Spotify): {e}")
                task.status = PipelineStage.FAILED
                print(f"[pipeline] Stage 1 FAILED: {e}")
                return task
        else:
            print(
                f"[pipeline] Stage 1 SKIP: '{task.metadata.artist} - {task.metadata.title}'"
            )

        # Stage 2: Deezer via deemix
        if task.metadata.isrc:
            try:
                task.status = PipelineStage.STAGE2_DEEMIX
                deezer_track = self._deemix.lookup_isrc(task.metadata.isrc)
                if deezer_track is not None:
                    print(f"[pipeline] Stage 2 ISRC found: {deezer_track.deezer_url}")
                else:
                    # ISRC failed, try artist+title search
                    deezer_track = self._deemix.search_track(
                        task.metadata.artist, task.metadata.title
                    )
                    if deezer_track:
                        print(f"[pipeline] Stage 2 search found: {deezer_track.deezer_url}")
                    else:
                        task.stage_errors.append("Stage 2: not on Deezer (ISRC + search)")
                        print("[pipeline] Stage 2 SKIP: not on Deezer")

                if deezer_track:
                    DOWNLOAD_SEM.acquire()
                    try:
                        task.file_path = self._deemix.download(deezer_track.deezer_url)
                    finally:
                        DOWNLOAD_SEM.release()
                    if task.file_path:
                        task.download_source = "deemix"
                        print(f"[pipeline] Stage 2 OK: {task.file_path}")
            except Exception as e:
                task.stage_errors.append(f"Stage 2 (deemix): {e}")
                print(f"[pipeline] Stage 2 FAILED: {e}")
        else:
            task.stage_errors.append("Stage 2: no ISRC")
            print("[pipeline] Stage 2 SKIP: no ISRC")

        # Stage 3: YouTube via spotDL (fallback)
        if task.file_path is None:
            try:
                task.status = PipelineStage.STAGE3_SPOTDL
                DOWNLOAD_SEM.acquire()
                try:
                    task.file_path = self._spotdl.download(task.spotify_url)
                finally:
                    DOWNLOAD_SEM.release()
                if task.file_path:
                    task.download_source = "spotdl"
                    print(f"[pipeline] Stage 3 OK: {task.file_path}")
                else:
                    task.stage_errors.append("Stage 3: no file produced")
                    print("[pipeline] Stage 3 FAILED: no file")
            except Exception as e:
                task.stage_errors.append(f"Stage 3 (spotDL): {e}")
                print(f"[pipeline] Stage 3 FAILED: {e}")

        # Stage 4: Tagging + verify
        if task.file_path:
            try:
                task.status = PipelineStage.STAGE4_TAGGING
                meta = task.metadata
                assert meta is not None
                final_path = tag_track(
                    task.file_path,
                    title=meta.title,
                    artist=meta.artist,
                    album=meta.album,
                    isrc=meta.isrc,
                    cover_url=meta.cover_url,
                )
                task.file_path = final_path

                # Verify file not corrupt
                fsize = os.path.getsize(final_path)
                if fsize < MIN_FILE_SIZE:
                    os.remove(final_path)
                    task.file_path = None
                    task.stage_errors.append(
                        f"verify: only {fsize}B (min {MIN_FILE_SIZE})"
                    )
                    print(f"[pipeline] CORRUPT: {final_path} is {fsize}B - deleted")
            except Exception as e:
                task.stage_errors.append(f"Stage 4 (tagging): {e}")
                print(f"[pipeline] Stage 4 SOFT FAIL: {e}")

        # Final status
        if task.file_path:
            task.status = PipelineStage.READY
            task.completed_at = time.time()
            print(f"[pipeline] DONE ({task.download_source}, {_elapsed(task):.1f}s)")
        else:
            task.status = PipelineStage.FAILED
            task.completed_at = time.time()
            print(f"[pipeline] FAILED after {_elapsed(task):.1f}s: {task.stage_errors}")

        return task


def _elapsed(task: DownloadTask) -> float:
    end = task.completed_at or time.time()
    return end - task.started_at
