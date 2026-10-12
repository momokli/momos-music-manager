"""Spotify Client Credentials Flow + Track Metadata (Stage 1).

Uses the Client Credentials flow (server-to-server, no user login needed).
Token is cached and auto-refreshed.
"""

import base64
import time
from dataclasses import dataclass

import httpx
from config_loader import SpotifyConfig


@dataclass
class TrackMetadata:
    """Metadata extracted from Spotify for a track."""

    isrc: str | None
    title: str
    artist: str
    album: str
    cover_url: str | None
    duration_ms: int
    spotify_url: str


class SpotifyClient:
    """Authenticated Spotify API client using Client Credentials."""

    def __init__(self, config: SpotifyConfig) -> None:
        self._client_id = config.client_id
        self._client_secret = config.client_secret
        self._token: str | None = None
        self._token_expires: float = 0.0
        self._http = httpx.Client(timeout=15.0)

    # ── Public API ──────────────────────────────────────────────────

    def get_track(self, track_id: str) -> TrackMetadata:
        """Fetch track metadata from Spotify.

        Args:
            track_id: Spotify track ID (e.g., '4cOdK2wGLETKBW3PvgPWqT')
                      or full URI ('spotify:track:4cOdK2wGLETKBW3PvgPWqT')
        """
        tid = self._extract_id(track_id)
        data = self._get(f"/v1/tracks/{tid}")
        return TrackMetadata(
            isrc=data.get("external_ids", {}).get("isrc"),
            title=data["name"],
            artist=data["artists"][0]["name"] if data.get("artists") else "Unknown",
            album=data.get("album", {}).get("name", "Unknown"),
            cover_url=_best_image(data.get("album", {}).get("images", [])),
            duration_ms=data.get("duration_ms", 0),
            spotify_url=data.get("external_urls", {}).get("spotify", ""),
        )

    def search_tracks(self, query: str, limit: int = 10) -> list[TrackMetadata]:
        """Search for tracks on Spotify."""
        params = {"q": query, "type": "track", "limit": str(limit)}
        data = self._get("/v1/search", params=params)
        items = data.get("tracks", {}).get("items", [])
        return [
            TrackMetadata(
                isrc=item.get("external_ids", {}).get("isrc"),
                title=item["name"],
                artist=item["artists"][0]["name"] if item.get("artists") else "Unknown",
                album=item.get("album", {}).get("name", "Unknown"),
                cover_url=_best_image(item.get("album", {}).get("images", [])),
                duration_ms=item.get("duration_ms", 0),
                spotify_url=item.get("external_urls", {}).get("spotify", ""),
            )
            for item in items
        ]

    def get_playlist_tracks(self, playlist_id: str) -> list[TrackMetadata]:
        """Fetch all tracks from a Spotify playlist (handles pagination)."""
        pid = self._extract_id(playlist_id)
        tracks: list[TrackMetadata] = []
        url: str | None = f"/v1/playlists/{pid}/tracks"

        while url:
            data = self._get(url, {"limit": "100"} if url.endswith("/tracks") else None)
            for item in data.get("items", []):
                track = item.get("track")
                if track is None or track.get("id") is None:
                    continue
                tracks.append(
                    TrackMetadata(
                        isrc=track.get("external_ids", {}).get("isrc"),
                        title=track["name"],
                        artist=track["artists"][0]["name"]
                        if track.get("artists")
                        else "Unknown",
                        album=track.get("album", {}).get("name", "Unknown"),
                        cover_url=_best_image(track.get("album", {}).get("images", [])),
                        duration_ms=track.get("duration_ms", 0),
                        spotify_url=track.get("external_urls", {}).get("spotify", ""),
                    )
                )
            url = data.get("next")

        return tracks

    # ── Auth helpers ────────────────────────────────────────────────

    def _ensure_token(self) -> str:
        """Get a valid access token, refreshing if expired."""
        if self._token and time.time() < self._token_expires - 60:
            return self._token
        self._refresh_token()
        assert self._token is not None
        return self._token

    def _refresh_token(self) -> None:
        """Fetch a new access token via Client Credentials flow."""
        credentials = base64.b64encode(
            f"{self._client_id}:{self._client_secret}".encode()
        ).decode()
        resp = self._http.post(
            "https://accounts.spotify.com/api/token",
            headers={"Authorization": f"Basic {credentials}"},
            data={"grant_type": "client_credentials"},
        )
        resp.raise_for_status()
        data = resp.json()
        self._token = data["access_token"]
        self._token_expires = time.time() + data.get("expires_in", 3600)

    def _get(self, path: str, params: dict | None = None) -> dict:
        """Make an authenticated GET request to the Spotify API."""
        token = self._ensure_token()
        resp = self._http.get(
            f"https://api.spotify.com{path}",
            headers={"Authorization": f"Bearer {token}"},
            params=params,
        )
        resp.raise_for_status()
        return resp.json()

    @staticmethod
    def _extract_id(track_ref: str) -> str:
        """Extract bare track ID from various Spotify formats."""
        t = track_ref.strip()
        # spotify:track:ID
        if ":" in t:
            parts = t.split(":")
            t = parts[-1]
        # https://open.spotify.com/track/ID
        if "/" in t:
            t = t.split("/")[-1]
        # Remove query params
        if "?" in t:
            t = t.split("?")[0]
        return t


def _best_image(images: list[dict]) -> str | None:
    """Return the URL of the best (largest) image."""
    if not images:
        return None
    # Spotify returns images sorted largest-first
    return images[0]["url"]
