# Plan: unified priority work-queue + leads spider (music-api)

**Status**: proposed
**Branch**: `feat/music-api-work-queue`
**Ready for review**: no — big architectural shift, see **Open decisions**
**Depends on**: `plans/proposed/music-api-data-vault.md`, `plans/proposed/hub-crawl-everything.md`
**Migration needed**: music-api — `work_items` (+ staging tables)

### Description

**One persistent priority queue** in music-api's SQLite drives **all** background
work: **download, transcode, analyse, crawl** (per source). Each worker loop pulls
**~5 at a time** from `next(kind)`, and **`next` orders by priority**. User actions
in the hub *inherently* set the priority — nothing is "requested" by hand.

There is **no "order everything"**: every track that enters the catalog is
*inherently* a `download@prio 0`. References / now-playing / relatedness only
**raise** it.

---

### The queue

```sql
CREATE TABLE work_items (
    id          INTEGER PRIMARY KEY,
    kind        TEXT NOT NULL,      -- download|transcode|analyze|crawl:<source>
    ref         TEXT NOT NULL,      -- isrc | 'deezer:artist:10725209' | 'spotify:artist:…' | track id
    priority    INTEGER NOT NULL DEFAULT 0,
    state       TEXT NOT NULL DEFAULT 'queued',  -- queued|running|done|failed
    attempts    INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 5,
    not_before  TEXT,               -- TTL / backoff
    record_source TEXT,             -- deezer|spotify|musicbrainz|applemusic|soundcloud|hub|essentia|…
    error       TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    UNIQUE (kind, ref)
);
CREATE INDEX idx_work_next ON work_items(kind, state, priority DESC, not_before);
```

`next(kind, slots)` = `SELECT … WHERE kind=? AND state='queued' AND (not_before IS NULL OR not_before<=now) ORDER BY priority DESC, created_at ASC LIMIT ?`.
Every loop: `slots = MAX_INFLIGHT(5) − running(kind)`, then `next(kind, slots)`.

**Kinds (workers):**
- `download` — deemix/Deezer (existing pipeline).
- `transcode` — derive 320/128 from FLAC (and other-format transcodes).
- `analyze` — BPM/Key from the file (Essentia).
- `crawl:<source>` — **leads**: complete an entity via a source. One item per
  (source × entity), e.g. `crawl:spotify` / `crawl:musicbrainz` / `crawl:applemusic`
  / `crawl:soundcloud`, each with `ref = '<source>:artist:<id>'` (also album/track).

---

### Priority — the user's actions are the dial

Every work item's priority is derived from user-facing facts (same for download,
transcode, analyse **and** crawl):

| Fact | Priority |
| --- | --- |
| in the catalog, no reference (just heard / related) | 0 (or **negative** for related-only) |
| liked by any hub user, or in a user's playlist | ≥ 1 |
| +1 per additional playlist reference | +1 each |
| **currently playing** (now-playing) | **+100** |
| **related** to something listened to (features / similar) | low (e.g. −10) — *we still get it, later* |

Because the same rule applies to the **crawl** queue, playing a brand-new artist
puts *that artist's* `crawl:*` leads at +100 → the whole artist is completed fast.

---

### Leads (the spider in music-api)

A **lead** is a follow-up task: *"complete this artist via source X"*. The spider is
just a **queue of leads** (tens of thousands), processed in bounded batches,
priority-driven.

- **Seeded** when an entity is touched (a track's artists → all their source-leads)
  and, at lower priority, for **related** entities (featured artists we never play,
  similar artists).
- **Bounded**: per-source batch size, TTL per lead, caps; each source has its own
  worker class so one source's rate limits can't starve the others.
- Sources are pluggable — adding one = adding a `crawl:<source>` lead type.

---

### Staging / load layer (DWH)

User dumps (their playlists + tracks + likes) land in a **staging area** and are
loaded into the catalog as **atomic loads**:

- A user's playlist/likes are bound to that user, but every **track is atomic** →
  many users share one track row → users benefit from each other.
- Everyone **contributes to the music-api catalog**; the hub is the front end.
- The `download@0` for a newly-loaded track is inherent; references raise it.

---

### Where things live (the shift)

- **music-api = the music catalog** (Data Vault: hubs/links/satellites) + the work
  queue + the staging layer. It downloads *everything*, incl. related-only material
  at low priority → a huge on-disk archive.
- **The hub = front end**: it consumes music-api (metadata + resolved values) and
  holds *its* social layer (users, tags, playlists, collectives). It sets priorities
  by what its users do.

---

### Acceptance criteria

- [ ] `work_items` + `next(kind, slots)` with priority; `MAX_INFLIGHT` per kind
- [ ] download / transcode / analyze / crawl all run through it (no ad-hoc loops)
- [ ] No "order everything" path — a catalog track is inherently `download@0`
- [ ] Crawl leads seeded on touch; related entities at low priority
- [ ] User actions (like/playlist/now-playing) move priorities; visible via API
- [ ] Bounded, backoff, TTL; `cargo build`/`cargo test` green

### Open decisions

1. **Hub DB scope** — the hub keeps *its* social layer (users/tags/playlists/…)?
   (It must, for tags/collectives — clarify what is "front end only".)
2. **Priority tuning** — related-only = `-10` or `0`? negative base for the tail?
3. **Lead sources v1** — Spotify first, then MusicBrainz / Apple Music / SoundCloud?
4. **One queue table vs per-kind tables** — one `work_items` with `kind`, or split.
5. **Transcode as its own kind** vs folded into `download`.
