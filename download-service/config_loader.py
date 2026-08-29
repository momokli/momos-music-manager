"""Config loader - reads from TOML and environment variables.

Priority (highest first):
  1. Environment variables (DOWNLOAD_DEEMIX_ARL, DOWNLOAD_SPOTIFY_CLIENT_ID, ...)
  2. TOML file (~/.config/download-service/config.toml or script-dir)
  3. Defaults
"""

import os
import sys
from dataclasses import dataclass, field
from pathlib import Path

if sys.version_info >= (3, 11):
    import tomllib
else:
    import tomli as tomllib  # type: ignore


@dataclass
class DeemixConfig:
    arl: str = ""
    base_url: str = "http://localhost:6595"


@dataclass
class SpotifyConfig:
    client_id: str = ""
    client_secret: str = ""


@dataclass
class SpotdlConfig:
    executable: str = "spotdl"
    cookie_file: str = ""


@dataclass
class DownloadsConfig:
    output_dir: str = "/opt/download-service/downloads/tracks"
    deemix_bitrate: str = "320"
    spotdl_bitrate: str = "128"
    timeout_seconds: int = 180


@dataclass
class Config:
    deemix: DeemixConfig = field(default_factory=DeemixConfig)
    spotify: SpotifyConfig = field(default_factory=SpotifyConfig)
    spotdl: SpotdlConfig = field(default_factory=SpotdlConfig)
    downloads: DownloadsConfig = field(default_factory=DownloadsConfig)


def load_config(path: str | None = None) -> Config:
    """Load configuration.

    Priority (highest first):
      1. Env vars: DOWNLOAD_DEEMIX_ARL, DOWNLOAD_SPOTIFY_CLIENT_ID, ...
      2. TOML file (via path, or script-dir, or ~/.config/download-service/)
      3. Defaults
    """
    config = Config()

    # Step 1: Load from TOML file
    if path:
        paths = [Path(path)]
    else:
        paths = [
            Path(__file__).parent / "config.toml",
            Path.home() / ".config" / "download-service" / "config.toml",
        ]

    for p in paths:
        if p.exists():
            _apply_toml(config, tomllib.loads(p.read_text()))
            break

    # Step 2: Override with environment variables
    _apply_env(config)

    # Validate
    if not config.deemix.arl:
        raise ValueError(
            "deemix.arl is required. Set in ~/.config/download-service/config.toml "
            "or DOWNLOAD_DEEMIX_ARL env var."
        )
    if not config.spotify.client_id or not config.spotify.client_secret:
        raise ValueError(
            "spotify.client_id and spotify.client_secret are required. "
            "Set in config.toml or DOWNLOAD_SPOTIFY_CLIENT_ID / DOWNLOAD_SPOTIFY_CLIENT_SECRET env vars."
        )

    return config


def _apply_toml(config: Config, data: dict) -> None:
    if "deemix" in data:
        config.deemix.arl = data["deemix"].get("arl", config.deemix.arl)
        config.deemix.base_url = data["deemix"].get("base_url", config.deemix.base_url)
    if "spotify" in data:
        config.spotify.client_id = data["spotify"].get(
            "client_id", config.spotify.client_id
        )
        config.spotify.client_secret = data["spotify"].get(
            "client_secret", config.spotify.client_secret
        )
    if "spotdl" in data:
        config.spotdl.executable = data["spotdl"].get(
            "executable", config.spotdl.executable
        )
        config.spotdl.cookie_file = data["spotdl"].get(
            "cookie_file", config.spotdl.cookie_file
        )
    if "downloads" in data:
        config.downloads.output_dir = data["downloads"].get(
            "output_dir", config.downloads.output_dir
        )
        config.downloads.deemix_bitrate = data["downloads"].get(
            "deemix_bitrate", config.downloads.deemix_bitrate
        )
        config.downloads.spotdl_bitrate = data["downloads"].get(
            "spotdl_bitrate", config.downloads.spotdl_bitrate
        )
        config.downloads.timeout_seconds = data["downloads"].get(
            "timeout_seconds", config.downloads.timeout_seconds
        )


def _apply_env(config: Config) -> None:
    """Override config with environment variables (highest priority)."""
    for var, target, field in [
        ("DOWNLOAD_DEEMIX_ARL", config.deemix, "arl"),
        ("DOWNLOAD_SPOTIFY_CLIENT_ID", config.spotify, "client_id"),
        ("DOWNLOAD_SPOTIFY_CLIENT_SECRET", config.spotify, "client_secret"),
        ("DOWNLOAD_SPOTDL_COOKIE_FILE", config.spotdl, "cookie_file"),
        ("DOWNLOAD_OUTPUT_DIR", config.downloads, "output_dir"),
    ]:
        val = os.environ.get(var)
        if val:
            setattr(target, field, val)

    # Numeric env vars
    if os.environ.get("DOWNLOAD_TIMEOUT_SECONDS"):
        try:
            config.downloads.timeout_seconds = int(
                os.environ["DOWNLOAD_TIMEOUT_SECONDS"]
            )
        except ValueError:
            pass
