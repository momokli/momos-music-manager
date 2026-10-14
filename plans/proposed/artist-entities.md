# Plan: artist entities (atomic artists, no flattening)

**Status**: proposed
**Branch**: `feat/artist-entities`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: yes — `045_hub_artist_entities.sql`

### Description

Give **artists a first-class DB representation**. Today the hub flattens
Spotify's atomic `artists[]` into one comma-joined `hub_tracks.artists` string at
ingest (`ingest.rs::upsert_track`) and **throws the artist ids away**. This plan
stops that:

- every artist is a **row in `hub_artists`** (with its Spotify id),
- track ↔ artist is a **many-to-many link** (`hub_track_artists`), and
- we **crawl the Spotify artist API** to enrich/fill artist entries.

No string splitting in application code — the split happens **once**, in the
backfill migration, for historical rows only.

---

### Why (the bug that surfaced this)

Artist names are plaintext everywhere (Übersicht, track page, search, albums,
downloads, …) and not linked to the existing `/artists` + `/artist/{name}`
pages. Linking correctly (per artist, for multi-artist tracks) needs the
**atomic** artist list — which the DB currently doesn't hold.

---

### Data model — `migrations/045_hub_artist_entities.sql`

Aim: a **normalized (3NF) music-DB mirror** — artists and albums are first-class
entities, tracks reference them by FK / M:N, provider ids live in their own tables,
and we store **as much of the Spotify payload as we can**. No flattened strings as
the source of truth.

Additive only. Mirrors the existing `hub_track_external_ids` pattern.

```sql
CREATE TABLE hub_artists (
    id            INTEGER PRIMARY KEY,
    name          TEXT NOT NULL,           -- display name (as last seen)
    sort_name     TEXT,
    image_url     TEXT,
    followers     INTEGER,
    popularity    INTEGER,
    genres        TEXT,                    -- comma-joined Spotify artist genres
    first_seen_at TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_hub_artists_name  ON hub_artists(name COLLATE NOCASE);
CREATE INDEX        idx_hub_artists_sort  ON hub_artists(sort_name);

-- Provider ids (Spotify artist id, MusicBrainz id, …) — the real identity.
CREATE TABLE hub_artist_external_ids (
    artist_id   INTEGER NOT NULL REFERENCES hub_artists(id) ON DELETE CASCADE,
    service     TEXT NOT NULL,
    external_id TEXT NOT NULL,
    url         TEXT,
    source      TEXT,
    fetched_at  TEXT,
    PRIMARY KEY (artist_id, service, external_id)
);
CREATE INDEX idx_hub_artist_ext_lookup ON hub_artist_external_ids(service, external_id);

-- Track ↔ artist (ordered: primary, feat., …).
CREATE TABLE hub_track_artists (
    track_id    INTEGER NOT NULL REFERENCES hub_tracks(id) ON DELETE CASCADE,
    artist_id   INTEGER NOT NULL REFERENCES hub_artists(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (track_id, artist_id)
);
CREATE INDEX idx_hub_track_artists_artist ON hub_track_artists(artist_id);

-- Albums as entities (track -> album is N:1), so we stop flattening `album` too.
CREATE TABLE hub_albums (
    id            INTEGER PRIMARY KEY,
    title         TEXT NOT NULL,
    album_type    TEXT,            -- album | single | compilation | appears_on
    release_date  TEXT,
    total_tracks  INTEGER,
    label         TEXT,
    image_url     TEXT,
    first_seen_at TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);
CREATE INDEX idx_hub_albums_title ON hub_albums(title COLLATE NOCASE);

CREATE TABLE hub_album_external_ids (
    album_id    INTEGER NOT NULL REFERENCES hub_albums(id) ON DELETE CASCADE,
    service     TEXT NOT NULL,
    external_id TEXT NOT NULL,
    url         TEXT,
    source      TEXT,
    fetched_at  TEXT,
    PRIMARY KEY (album_id, service, external_id)
);
CREATE INDEX idx_hub_album_ext_lookup ON hub_album_external_ids(service, external_id);

ALTER TABLE hub_tracks ADD COLUMN album_id INTEGER REFERENCES hub_albums(id);
CREATE INDEX idx_hub_tracks_album_id ON hub_tracks(album_id);
```

**Backfill (same migration, one-time):** create `hub_artists` rows from the
existing distinct `hub_tracks.artists` strings _split_ on `; , & feat./ft.` (the
same delimiters `artists.rs::split_artists` uses — name-only, no ids), then insert
`hub_track_artists` links; likewise `hub_albums` from distinct `hub_tracks.album`
strings + `album_id`. This is the **only** place splitting is allowed.

`hub_tracks.artists` **stays** as a denormalized display cache (kept in sync at
ingest) so nothing else breaks while we migrate consumers to the entity tables.

---

### Ingest — `ingest.rs`

`upsert_track` currently does `t["artists"][].name.join(", ")`. Change to:

1. Upsert each `t["artists"][]` atomically: `{ id, name }` -->
   - upsert `hub_artists` (by spotify external id when present, else by name),
   - insert/refresh `hub_artist_external_ids` (service `spotify`, `external_id = id`),
   - insert `hub_track_artists` (position = array index).
2. Keep writing the joined `artists` string (display cache).

Non-Spotify sources (`upsert_track_from_ytdlp`, SoundCloud) have **no artist
ids** --> create name-keyed artists (external id absent). Documented limitation.

New helper: `ingest::upsert_track_artists(pool, track_id, &json_artists)`.

---

### Reads / links

- Track detail + list handlers fetch the track's artists from `hub_track_artists`
  as `Vec<{ id, name }>` and pass them to templates.
- Every artist name renders as `<a href="/artist/{id}">{name}</a>` (numeric id,
  collision-safe; keep a redirect `/artist/{name}` → `/artist/{id}` for old links).
- The existing `/artists` list and `/artist/{id}` profile keep working, now keyed
  by the entity (name-based `artist_page` logic reads the link table instead of
  splitting `hub_tracks.artists`).

---

### Crawl (Spotify artist API)

A background job (reuse the `worker.rs` enrichment loop, budgeted like the rest):

- `GET /v1/artists/{id}` --> name, image, followers, popularity, genres
  (populated on `hub_artists`).
- Optional (later slice, ties into `plans/proposed/hub-crawl-everything.md`):
  `/artists/{id}/albums` + `/artists/{id}/top-tracks` to ingest **new tracks**
  (the "artist depth"/"crawl everything" direction).
- Triggered when a new artist is first seen (leave-one's queue), rate-limited.

---

### Acceptance criteria

- [ ] Migration `045` runs clean on a fresh DB and on the live DB (existing rows
      backfilled into `hub_artists` + `hub_track_artists`)
- [ ] Ingest stores atomic artists: a Spotify track with 2 artists ⇒ 2
      `hub_artists` rows (with spotify external ids) + 2 link rows
- [ ] No application-code string splitting (grep: only the migration splits)
- [ ] Artist names are links **everywhere** (Übersicht, track page, search,
      albums, listening, downloads, tag, playlist, queue, …) → `/artist/{id}`
- [ ] `/artists` + `/artist/{id}` work by entity; `/artist/{name}` redirects
- [ ] Artist crawl populates name/image/followers/popularity/genres
- [ ] `cargo build`, `cargo test`, `npx playwright test` pass (Playwright: artist
      link present + navigates on the track page)
- [ ] Task history / budget respected (no unbounded Spotify calls)

### Phasing (one PR each)

1. **P0 — model + atomic ingest + backfill + links** (migration 045; no crawl yet).
2. **P1 — artist crawl** (`GET /artists/{id}` enrichment).
3. **P2 — artist depth / crawl-everything** (albums/top-tracks → new tracks).

### Open decisions (RESOLVED)

- **URLs are id-based**: `/artist/{id}`, `/album/{id}` (numeric, collision-safe);
  `/artist/{name}` and `/album/{name}` get a 301/redirect for old links.
- `hub_tracks.artists` / `hub_tracks.album` **stay as display caches** (kept in
  sync at ingest) during the migration; the entity tables are the source of truth.
- Spotify artist/album crawl eagerness: **on first seen** by default, budgeted.

The direction: **a proper music-DB mirror** (3NF) — artist + album + track
entities, provider ids in side tables, storing as much of the Spotify object as
the API gives us.
