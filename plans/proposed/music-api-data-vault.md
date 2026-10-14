# Plan: music-api as the Data Vault of music

**Status**: proposed
**Branch**: `feat/music-api-data-vault`
**Ready for review**: no
**Depends on**: `music-api` (existing order/download pipeline), `plans/proposed/artist-entities.md`, `plans/proposed/hub-crawl-everything.md`
**Migration needed**: yes — music-api vault tables (`hubs`/`links`/`satellites`)

### Description

`music-api` is the **data vault of music**: a **Data Vault 2.0** model (hubs, links,
satellites) over every musical entity we touch. Insert-only, hash-keyed, auditable —
multiple sources' truths coexist; consumers pick the value they trust.

The **hub is only a consumer** (its own DB is a read-model for the UI). It never
analyses and never owns the vault.

---

### Conventions (DV2)

- **Hash keys**: `*_hk` = `UPPER(HEX(SHA256(normalized_business_key)))` (store as TEXT).
  Business keys are normalized (`UPPER(TRIM(...))`; ISRC: strip `-`/spaces).
- **Link keys**: `*_lk` = hash of the concatenated participating hub keys (stable order).
- **`load_date`**: UTC ISO-8601 of the load; **`record_source`**: the producing source
  (`deezer`, `spotify`, `yt-dlp`, `essentia`, `traktor`, `hub`, …).
- **`hash_diff`**: SHA-256 over the satellite's descriptive columns; a new row is
  written only when it changes (insert-only, append).
- Satellites are keyed by `(parent_hk, load_date)` (or `+ record_source` when a source
  must be told apart — e.g. analysis).
- **The operational queue is NOT the vault.** The order/download pipeline
  (`orders`/`order_items`/`tracks.state`) stays operational (mutable). The vault holds
  the **data**; the worker writes **loads** into it.

---

### Hubs (business keys)

```sql
CREATE TABLE hub_track (track_hk TEXT PRIMARY KEY, isrc TEXT NOT NULL,
    load_date TEXT NOT NULL, record_source TEXT NOT NULL);
CREATE UNIQUE INDEX ux_hub_track_bk ON hub_track(isrc);

CREATE TABLE hub_artist (artist_hk TEXT PRIMARY KEY, artist_name TEXT NOT NULL,
    load_date TEXT NOT NULL, record_source TEXT NOT NULL);
CREATE UNIQUE INDEX ux_hub_artist_bk ON hub_artist(artist_name);   -- normalized name

CREATE TABLE hub_album (album_hk TEXT PRIMARY KEY, album_key TEXT NOT NULL,
    load_date TEXT NOT NULL, record_source TEXT NOT NULL);
CREATE UNIQUE INDEX ux_hub_album_bk ON hub_album(album_key);
```

> **Business keys**: track = ISRC (strip/upper). artist = normalized **name**
> (provider ids are satellites/links, below). album = normalized `title` (+ first
> release date when known). Provider-specific ids are **not** hubs (avoid a hub per
> provider) — they live as satellites/links.

### Links (relationships)

```sql
CREATE TABLE link_track_artist (track_artist_lk TEXT PRIMARY KEY,
    track_hk TEXT NOT NULL, artist_hk TEXT NOT NULL, position INTEGER NOT NULL DEFAULT 0,
    load_date TEXT NOT NULL, record_source TEXT NOT NULL);
CREATE INDEX ix_lta_track  ON link_track_artist(track_hk);
CREATE INDEX ix_lta_artist ON link_track_artist(artist_hk);

CREATE TABLE link_track_album (track_album_lk TEXT PRIMARY KEY,
    track_hk TEXT NOT NULL, album_hk TEXT NOT NULL,
    load_date TEXT NOT NULL, record_source TEXT NOT NULL);
CREATE INDEX ix_lta2_album ON link_track_album(album_hk);
```

### Satellites (descriptive, per-source, insert-only)

```sql
CREATE TABLE sat_track_meta (track_hk TEXT NOT NULL, load_date TEXT NOT NULL,
    hash_diff TEXT NOT NULL, title TEXT, artist TEXT, album TEXT, duration_ms INTEGER,
    explicit INTEGER, image_url TEXT, record_source TEXT NOT NULL,
    PRIMARY KEY (track_hk, load_date));

CREATE TABLE sat_track_analysis (track_hk TEXT NOT NULL, load_date TEXT NOT NULL,
    hash_diff TEXT NOT NULL, bpm REAL, musical_key TEXT, camelot TEXT, energy REAL,
    analyzer TEXT NOT NULL,           -- 'essentia' | 'traktor' | 'reccobeats' | …
    record_source TEXT NOT NULL,      -- 'music-api-worker' | 'hub' | …
    PRIMARY KEY (track_hk, load_date, analyzer));

CREATE TABLE sat_track_download (track_hk TEXT NOT NULL, load_date TEXT NOT NULL,
    hash_diff TEXT NOT NULL, state TEXT, source_format TEXT, formats TEXT,
    record_source TEXT NOT NULL, PRIMARY KEY (track_hk, load_date));

CREATE TABLE sat_artist_extid (artist_hk TEXT NOT NULL, load_date TEXT NOT NULL,
    hash_diff TEXT NOT NULL, service TEXT NOT NULL, external_id TEXT NOT NULL,
    url TEXT, record_source TEXT NOT NULL,
    PRIMARY KEY (artist_hk, service, load_date));

CREATE TABLE sat_artist_meta (artist_hk TEXT NOT NULL, load_date TEXT NOT NULL,
    hash_diff TEXT NOT NULL, image_url TEXT, followers INTEGER, popularity INTEGER,
    genres TEXT, record_source TEXT NOT NULL, PRIMARY KEY (artist_hk, load_date));

CREATE TABLE sat_album_meta (album_hk TEXT NOT NULL, load_date TEXT NOT NULL,
    hash_diff TEXT NOT NULL, album_type TEXT, release_date TEXT, total_tracks INTEGER,
    label TEXT, image_url TEXT, record_source TEXT NOT NULL,
    PRIMARY KEY (album_hk, load_date));

CREATE TABLE sat_album_extid (album_hk TEXT NOT NULL, load_date TEXT NOT NULL,
    hash_diff TEXT NOT NULL, service TEXT NOT NULL, external_id TEXT NOT NULL,
    url TEXT, record_source TEXT NOT NULL, PRIMARY KEY (album_hk, service, load_date));
```

---

### The analysis satellite is the point

`sat_track_analysis` is **insert-only, one row per `(track, analyzer, load_date)`** —
so "these were the analysis results" is always visible. The consumer resolves the
"right" BPM/key by **precedence** over the latest per-analyzer rows
(default `traktor > essentia > reccobeats`, configurable). A trash value stays in the
model; it's just not chosen.

### Loaders (in `music-api`)

- **Ingest (Deezer/order/url):** write `hub_track`/`hub_artist`/`hub_album`, the links,
  and `sat_track_meta` / `sat_artist_meta` / `sat_album_meta` / `*_extid` (hashdiff-gated).
- **Analysis worker:** write `sat_track_analysis(analyzer='essentia', record_source='music-api-worker')`.
- **Download state:** snapshot into `sat_track_download` (operational `tracks.state` stays
  the live queue).

### Consumer surface (for the hub)

- `GET /vault/track/{isrc}` → hubs + latest satellites + resolved best BPM/key.
- `GET /vault/tracks?isrcs=` (batched) — same, for many.
- Keep `/metadata/{isrc}` as a thin alias of the resolved analysis (back-compat).

---

### Migration from today's tables

`tracks` (operational) stays; `track_metadata` is superseded by `sat_track_analysis`.
Backfill: from `tracks` + `track_metadata` → hubs/links/satellites (one-time, Rust
loader, **off the boot path**, batched — see the hub backfill lesson).

### Acceptance criteria

- [ ] Vault tables created (additive); fresh-DB migrations clean
- [ ] Ingest paths write hubs/links/satellites (hashdiff-gated, insert-only)
- [ ] `sat_track_analysis` receives the Essentia result (source `essentia`)
- [ ] `/vault/track/{isrc}` resolves best BPM/key by precedence
- [ ] Backfill of existing rows (off boot, batched, idempotent)
- [ ] Hub consumes the vault (no hub-side analysis, no `hub_track_analysis_sat`)
- [ ] `cargo build`/`cargo test` green (music-api + hub)

### Open decisions

- artist/album **business keys** (name vs provider id) — v1 uses normalized names +
  extid satellites.
- Whether to keep `track_metadata` as a compatibility view over the satellite.
- Cross-vault identity: linking a **Deezer artist** to a **Spotify artist** (name today;
  later ISRC co-occurrence / MusicBrainz).
