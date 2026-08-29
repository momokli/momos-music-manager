"""spotDL CLI wrapper (Stage 3 — YouTube Fallback).

Used when Deezer doesn't have the track.
Leverages spotDL's YouTube matching algorithm.
"""

import subprocess
from pathlib import Path

from config_loader import DownloadsConfig, SpotdlConfig


class SpotdlDownloader:
    """Downloads tracks from YouTube via the spotDL CLI."""

    def __init__(self, spotdl_cfg: SpotdlConfig, dl_cfg: DownloadsConfig) -> None:
        self._executable = spotdl_cfg.executable
        self._cookie_file = spotdl_cfg.cookie_file
        self._bitrate = dl_cfg.spotdl_bitrate
        self._output_dir = dl_cfg.output_dir
        self._timeout = dl_cfg.timeout_seconds

    def download(self, spotify_url: str) -> str | None:
        """Download a track via spotDL.

        Args:
            spotify_url: Spotify track URL (open.spotify.com/track/...)

        Returns the path to the downloaded file, or None on failure.
        """
        cmd = [
            self._executable, "download",
            spotify_url,
            "--output",
            self._output_dir,
            "--bitrate",
            self._bitrate + "k",
            "--format",
            "mp3",
        ]

        if self._cookie_file:
            cmd.extend(["--cookie-file", self._cookie_file])

        try:
            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=self._timeout,
            )
        except subprocess.TimeoutExpired:
            print(f"[spotDL] TIMEOUT after {self._timeout}s: {spotify_url}")
            return None
        except FileNotFoundError:
            print(
                f"[spotDL] ERROR: '{self._executable}' not found. Is spotDL installed?"
            )
            return None

        if result.returncode != 0:
            stderr = result.stderr.strip()
            stdout = result.stdout.strip()
            error_msg = stderr or stdout or "unknown error"
            print(f"[spotDL] FAILED (rc={result.returncode}): {error_msg[:300]}")
            return None

        # spotDL outputs the file path in stdout
        output = result.stdout.strip() + result.stderr.strip()
        return _parse_downloaded_path(output, self._output_dir)


def _parse_downloaded_path(stdout: str, output_dir: str) -> str | None:
    """Try to find the downloaded file from spotDL output."""
    for line in stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        # spotDL typically prints the destination path
        if line.endswith(".mp3") and ("Downloaded" in line or output_dir in line):
            # Extract path from line like "Downloaded "Artist - Title.mp3" to /path/"
            if output_dir in line:
                idx = line.find(output_dir)
                path = line[idx:].rstrip('"').rstrip("'")
                # Find the actual file
                import glob

                candidates = glob.glob(f"{output_dir}/**/*.mp3", recursive=True)
                if candidates:
                    return max(candidates, key=lambda p: Path(p).stat().st_mtime)
                return path

    # Fallback: newest mp3 in output dir
    import glob
    import os
    import time

    candidates = []
    for path in glob.glob(f"{output_dir}/**/*.mp3", recursive=True):
        try:
            mtime = os.path.getmtime(path)
            if time.time() - mtime < 10:
                candidates.append((mtime, path))
        except OSError:
            pass

    if candidates:
        candidates.sort(reverse=True)
        return candidates[0][1]

    return None
