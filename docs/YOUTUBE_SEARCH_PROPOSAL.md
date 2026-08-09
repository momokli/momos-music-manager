# Proposal: YouTube + Spotify Search

## What

Guest search page shows results from BOTH Spotify and YouTube, deduplicated,
with platform badges. Guest picks the exact version they want.

## Backend: `/search` returns both sources

```
GET /search?q=Dancing+Queen&limit=10

{
  "results": [
    {
      "source": "spotify",
      "id": "4euAGZTszWPrriggYK0HG9",
      "title": "Dancing Queen",
      "artist": "ABBA",
      "coverUrl": "https://i.scdn.co/...",
      "durationMs": 232000,
      "spotifyUrl": "https://open.spotify.com/track/...",
      "sourceUrl": "https://open.spotify.com/track/..."
    },
    {
      "source": "youtube",
      "id": "xFrGuyw1V8s",
      "title": "ABBA - Dancing Queen (Official Music Video)",
      "artist": "ABBA",
      "coverUrl": "https://i.ytimg.com/vi/xFrGuyw1V8s/hqdefault.jpg",
      "durationMs": 234000,
      "spotifyUrl": null,
      "sourceUrl": "https://www.youtube.com/watch?v=xFrGuyw1V8s"
    }
  ]
}
```

## YouTube search via yt-dlp

Already installed. Command:

```bash
yt-dlp "ytsearch5:Dancing Queen" --flat-playlist -j --no-playlist
```

Output per result:
```json
{
  "id": "xFrGuyw1V8s",
  "title": "ABBA - Dancing Queen (Official Music Video)",
  "duration": 234.0,
  "channel": "ABBA",
  "thumbnails": [{"url": "...", "height": 360, "width": 480}]
}
```

Parse: `title` → split on ` - ` or use `channel` for artist.
`thumbnails[0]["url"]` → coverUrl.
`duration` → durationMs (×1000).

## Deduplication

YouTube and Spotify results often overlap. Strategy:
- Show all, sorted by source (Spotify first, then YouTube)
- OR deduplicate by normalized title+artist (if both have same song, prefer Spotify)
- Simple v1: just show both, user picks

## Frontend changes

- Badge: `SPOTIFY` (green) or `YOUTUBE` (red)
- YouTube results show video thumbnail
- "Want" button sends `sourceUrl` to `/download` — backend figures out how to download

## Download routing

If `source=youtube`: skip deemix, go straight to spotDL with the YouTube URL.

If `source=spotify`: normal pipeline (ISRC → deemix → spotDL fallback).

## Files touched

| File | Change |
|---|---|
| `download-service/main.py` | `/search` endpoint: add yt-dlp call, merge results |
| `download-service/static/request.html` | Badge, cover fallback for YouTube |

## Estimate

~30 lines of Python, ~5 lines of JS. 15 minutes.
