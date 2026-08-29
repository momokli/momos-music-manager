"""ID3 Tagging with mutagen (Stage 4).

Writes metadata and embeds cover art into downloaded tracks.
"""

import os
from pathlib import Path
from typing import Optional

from mutagen.easyid3 import EasyID3
from mutagen.id3 import APIC, ID3
from mutagen.mp3 import MP3


def tag_track(
    file_path: str,
    title: str,
    artist: str,
    album: str,
    isrc: str | None = None,
    cover_url: str | None = None,
) -> str:
    """Apply ID3 tags and optionally embed cover art into an MP3 file.

    Also normalizes the filename to 'Artist - Title.mp3' and moves to
    the output directory if needed.

    Args:
        file_path: Path to the downloaded MP3 file.
        title: Track title.
        artist: Track artist.
        album: Album name.
        isrc: ISRC code.
        cover_url: URL of cover art to download and embed.

    Returns:
        The final path of the tagged file.
    """
    if not os.path.exists(file_path):
        raise FileNotFoundError(f"File not found: {file_path}")

    path = Path(file_path)

    # Only tag MP3 files — FLAC/AAC need different handling (out of scope for now)
    if path.suffix.lower() != ".mp3":
        print(f"[tagger] Skipping non-MP3 file: {path.name}")
        return str(path)

    # Load or create ID3 tags
    try:
        audio = MP3(path, ID3=EasyID3)
    except Exception:
        audio = MP3(path)
        audio.add_tags()

    audio["title"] = title
    audio["artist"] = artist
    audio["album"] = album
    if isrc:
        audio["isrc"] = isrc
    audio.save()

    # Embed cover art if available
    if cover_url:
        _embed_cover(str(path), cover_url)

    # Rename to canonical format: "Artist - Title.mp3"
    parent = path.parent
    safe_artist = _safe_filename(artist)
    safe_title = _safe_filename(title)
    new_name = f"{safe_artist} - {safe_title}.mp3"
    new_path = parent / new_name

    if new_path == path:
        return str(path)

    if new_path.exists():
        # Canonical name exists - keep better quality (larger file wins)
        existing_size = new_path.stat().st_size
        current_size = path.stat().st_size
        if current_size > existing_size:
            new_path.unlink()
            os.rename(path, new_path)
        else:
            path.unlink()
        return str(new_path)
    else:
        os.rename(path, new_path)
        return str(new_path)


def _embed_cover(file_path: str, cover_url: str) -> None:
    """Download and embed cover art into the MP3."""
    import httpx

    try:
        resp = httpx.get(cover_url, timeout=15.0)
        resp.raise_for_status()
        cover_data = resp.content
    except Exception as e:
        print(f"[tagger] Failed to download cover: {e}")
        return

    if not cover_data:
        return

    try:
        audio = ID3(file_path)
    except Exception:
        audio = ID3()

    # Remove existing cover art
    audio.delall("APIC")

    # Determine MIME type
    mime = "image/jpeg"
    if cover_data[:4] == b"\x89PNG":
        mime = "image/png"

    audio.add(
        APIC(
            encoding=3,
            mime=mime,
            type=3,  # Cover (front)
            desc="Cover",
            data=cover_data,
        )
    )
    # v2_version=3 for broad compatibility
    audio.save(file_path, v2_version=3)


def _safe_filename(name: str) -> str:
    """Sanitize a string for use as a filename."""
    # Replace characters that are problematic in filenames
    for char in r'<>:"/\|?*':
        name = name.replace(char, "-")
    # Collapse multiple dashes/spaces
    while "  " in name:
        name = name.replace("  ", " ")
    while "--" in name:
        name = name.replace("--", "-")
    return name.strip()
