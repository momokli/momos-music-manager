"""Deezer API + deemix CLI wrapper (Stage 2).

ISRC lookup via public Deezer REST API, then delegates download to
the deemix CLI (bambanah/deemix fork).
"""

import subprocess
from dataclasses import dataclass

import httpx
from config_loader import DeemixConfig, DownloadsConfig


@dataclass
class DeezerTrack:
    """Deezer track info from the public API."""

    deezer_id: int
    title: str
    artist: str
    album: str
    cover_url: str
    duration_seconds: int
    isrc: str
    deezer_url: str


class DeemixDownloader:
    """Downloads tracks from Deezer via the deemix CLI."""

    def __init__(self, deemix_cfg: DeemixConfig, dl_cfg: DownloadsConfig) -> None:
        self._bitrate = dl_cfg.deemix_bitrate
        self._output_dir = dl_cfg.output_dir
        self._timeout = dl_cfg.timeout_seconds
        self._http = httpx.Client(timeout=10.0)

    # ── Public API ──────────────────────────────────────────────────

    def lookup_isrc(self, isrc: str) -> DeezerTrack | None:
        resp = self._http.get(f"https://api.deezer.com/track/isrc:{isrc}")
        if resp.status_code == 404:
            return None
        resp.raise_for_status()
        data = resp.json()
        if "error" in data:
            return None
        return self._parse(data)

    def search_track(self, artist: str, title: str) -> DeezerTrack | None:
        """Fallback: search Deezer by artist+title when ISRC fails."""
        import urllib.parse
        q = urllib.parse.quote(f"{artist} {title}")
        resp = self._http.get(f"https://api.deezer.com/search?q={q}&limit=5")
        resp.raise_for_status()
        data = resp.json()
        if not data.get("data"):
            return None
        for track in data["data"]:
            if track["artist"]["name"].lower() == artist.lower():
                return self._parse(track)
        return self._parse(data["data"][0])

    def _parse(self, data: dict) -> DeezerTrack:
        return DeezerTrack(
            deezer_id=data["id"],
            title=data["title"],
            artist=data["artist"]["name"],
            album=data["album"]["title"],
            cover_url=data["album"]["cover_big"],
            duration_seconds=data["duration"],
            isrc=data.get("isrc", ""),
            deezer_url=data["link"],
        )

    def download(self, deezer_url: str) -> str | None:
        """Download a track from Deezer via the deemix CLI.

        Returns the path to the downloaded file, or None on failure.
        """
        output_dir = self._output_dir
        cmd = [
            "python3",
            "-m",
            "deemix",
            "--bitrate",
            self._bitrate,
            "--path",
            output_dir,
            deezer_url,
        ]

        try:
            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=self._timeout,
            )
        except subprocess.TimeoutExpired:
            print(f"[deemix] TIMEOUT after {self._timeout}s: {deezer_url}")
            return None
        except FileNotFoundError:
            print(
                "[deemix] ERROR: deemix not found. Is it installed? (pip install deemix)"
            )
            return None

        if result.returncode != 0:
            stderr = result.stderr.strip()
            stdout = result.stdout.strip()
            error_msg = stderr or stdout or "unknown error"
            # ARL expired → clear signal for retry
            if "NotLoggedIn" in error_msg or "arl" in error_msg.lower():
                print(f"[deemix] ARL EXPIRED: {error_msg[:200]}")
            else:
                print(f"[deemix] FAILED (rc={result.returncode}): {error_msg[:300]}")
            return None

        # deemix output: "Artist - Title (FLAC 44100Hz 24bit).flac" or ".mp3"
        # Find the downloaded file by parsing stdout
        output = result.stdout.strip() + result.stderr.strip()
        return _parse_downloaded_path(output, self._output_dir, deezer_url)


def _parse_downloaded_path(stdout: str, output_dir: str, deezer_url: str) -> str | None:
    """Find the downloaded file by scanning the output directory."""
    import glob
    import os
    import time

    now = time.time()
    candidates = []

    # First try: parse stdout for a clear filename
    for line in stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        # Look for lines like Rick Astley - Never Gonna Give You Up.mp3
        if line.endswith(".mp3") or line.endswith(".flac") or line.endswith(".m4a"):
            if not line.startswith("["):  # skip deemix progress lines
                if "/" not in line:  # just a filename, not a path
                    candidate = os.path.join(output_dir, line)
                    if os.path.exists(candidate):
                        return candidate

    # Second try: newest file in output dir (within last 60s)
    for ext in ("*.mp3", "*.flac", "*.m4a"):
        for path in glob.glob(os.path.join(output_dir, ext)):
            try:
                mtime = os.path.getmtime(path)
                if now - mtime < 60:
                    candidates.append((mtime, path))
            except OSError:
                pass

    if candidates:
        candidates.sort(reverse=True)
        return candidates[0][1]

    return None
