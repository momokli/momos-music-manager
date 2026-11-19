#!/usr/bin/env python3
"""Offline music-api stub for the hub screenshot env.

Serves the Deezer metadata routes the hub's album pages call, with a fixed
PAW-Patrol-like fixture set, plus locally generated SVG covers so a screenshot
never shows a broken image.

    MUSIC_API_PORT=18710 python3 scripts/dev/mock-music-api.py

Routes (the bearer token the hub sends is ignored):
    GET /health
    GET /deezer/artist/{id}/albums?limit&index
    GET /deezer/search/artist?q
    GET /deezer/album/{id}
    GET /cover/{id}.svg
"""

import json
import os
import re
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

PORT = int(os.environ.get("MUSIC_API_PORT", "18710"))

ARTIST_ID = "15167501"

# (album_id, title, release_date, seconds, tracks, color)
ALBUMS = [
    ("1137915301", "Folge 459: Die Robo-Ente", "2026-11-13", 1174, 12, "#1f6feb"),
    (
        "1137915302",
        "Folge 458: Wilder Ritt auf dem Dino",
        "2026-11-06",
        1290,
        13,
        "#8250df",
    ),
    ("1137915303", "Folge 457: Einsatz im Schnee", "2026-10-30", 980, 10, "#0a7d4f"),
    ("1137915304", "Folge 456: Der grosse Sturm", "2026-10-23", 1310, 14, "#bf3989"),
    ("1137915305", "Folge 455: Rettung im Hafen", "2026-10-16", 1105, 11, "#bc4c00"),
]

BY_ID = {a[0]: a for a in ALBUMS}


def album_summary(album):
    album_id, title, release, seconds, tracks, _ = album
    return {
        "albumId": album_id,
        "title": title,
        "releaseDate": release,
        "recordType": "album",
        "coverUrl": f"http://127.0.0.1:{PORT}/cover/{album_id}.svg",
        "nbTracks": tracks,
        # The real music-api does not send a duration in the list yet (#621);
        # the mock does, so the page can be screenshotted without N detail calls.
        "seconds": seconds,
    }


def album_detail(album):
    album_id, title, release, seconds, tracks, _ = album
    per = max(1, seconds // max(1, tracks))
    return {
        "albumId": album_id,
        "title": title,
        "releaseDate": release,
        "recordType": "album",
        "coverUrl": f"http://127.0.0.1:{PORT}/cover/{album_id}.svg",
        "seconds": seconds,
        "tracks": [
            {
                "isrc": f"DEVMOCK{album_id}{i:03d}",
                "title": f"Kapitel {i:02d}",
                "durationMs": per * 1000,
                "position": i,
            }
            for i in range(1, tracks + 1)
        ],
    }


def cover_svg(album_id):
    _, title, _, _, _, color = BY_ID.get(album_id, ("", "Album", "", 0, 0, "#444"))
    label = re.sub(r"^Folge ", "", title).split(":")[0]
    return f"""<svg xmlns="http://www.w3.org/2000/svg" width="300" height="300">
  <rect width="300" height="300" fill="{color}"/>
  <circle cx="150" cy="128" r="74" fill="rgba(255,255,255,0.18)"/>
  <text x="150" y="146" font-family="Helvetica,Arial" font-size="54" font-weight="700"
        fill="#fff" text-anchor="middle">{label}</text>
  <text x="150" y="232" font-family="Helvetica,Arial" font-size="19"
        fill="rgba(255,255,255,0.85)" text-anchor="middle">PAW Patrol</text>
</svg>"""


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):  # keep the terminal quiet
        pass

    def _send(self, code, body, ctype="application/json"):
        raw = body.encode() if isinstance(body, str) else body
        self.send_response(code)
        self.send_header("content-type", ctype)
        self.send_header("content-length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        url = urlparse(self.path)
        path = url.path.rstrip("/")

        if path == "/health":
            return self._send(200, json.dumps({"status": "ok"}))

        m = re.fullmatch(r"/deezer/artist/([^/]+)/albums", path)
        if m:
            limit = int(parse_qs(url.query).get("limit", ["50"])[0])
            albums = [album_summary(a) for a in ALBUMS][:limit]
            return self._send(
                200, json.dumps({"albums": albums, "total": len(ALBUMS), "next": None})
            )

        if path == "/deezer/search/artist":
            q = parse_qs(url.query).get("q", [""])[0].strip().lower()
            hits = []
            if q in ("paw patrol", "paw", "patrol", ""):
                hits.append(
                    {
                        "artistId": ARTIST_ID,
                        "name": "PAW Patrol",
                        "coverUrl": f"http://127.0.0.1:{PORT}/cover/1137915301.svg",
                    }
                )
            return self._send(200, json.dumps({"artists": hits}))

        m = re.fullmatch(r"/deezer/album/([^/]+)", path)
        if m:
            album = BY_ID.get(m.group(1))
            if not album:
                return self._send(404, json.dumps({"error": "unknown album"}))
            return self._send(200, json.dumps(album_detail(album)))

        m = re.fullmatch(r"/cover/([^/]+)\.svg", path)
        if m:
            return self._send(200, cover_svg(m.group(1)), "image/svg+xml")

        self._send(404, json.dumps({"error": "not found", "path": path}))


if __name__ == "__main__":
    print(f"mock music-api on http://127.0.0.1:{PORT}", flush=True)
    ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
