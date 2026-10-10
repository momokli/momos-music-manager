# MMM Hub — Data Model & Spotify Ingest

> Written against `plans/mmm-hub/00-interface.md` (frozen contract). Table, view
> and column **names** are taken verbatim from §5; anything this document _adds_
> (extra columns, one extra index, timestamp conventions) is called out inline and
> again in §7. Anything the contract fixes that I had to _interpret_ (the meaning
> of `playlist_id` inside a view, `track_id` in URLs) is flagged as an open
> question in §7 rather than silently assumed.
>
> **Revised against the post-February-2026 Spotify Web API** (see `00-interface.md`
> §1a and `verification.md`): playlist contents now come from
> `GET /playlists/{id}/items` and are **owner/collaborator-only**, the batch
> endpoints are gone (full track objects are embedded instead), ISRC is
> nullable/non-unique and **not** an identity, and refresh tokens expire 6 months
> after the original authorization.

Scope: M1 data model (migration `001`), the four overlap views, seed fixtures,
and the M3 Spotify ingest algorithm. `hub_sessions` is **not** created here — the
contract defers it to migration `002`, which is owned by `auth.md`. This document
assumes `hub_users.id` is an `INTEGER PRIMARY KEY` (rowid alias) exactly as
`auth.md` §2.1 assumes for its `hub_sessions.user_id` FK.

---

## 1. Storage conventions

These conventions apply to every `hub_` table in this document. They are chosen
to match the sibling `auth.md` (which already fixed `hub_sessions` to ISO-8601
TEXT timestamps), so the whole `hub.db` stays internally consistent.

| Concern        | Choice                                                                                                                                                   | Rationale                                                                                                                                                                                                                            |
| -------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Timestamps     | `TEXT` ISO-8601 UTC, default `(strftime('%Y-%m-%dT%H:%M:%fZ','now'))`                                                                                    | Matches `auth.md`'s `hub_sessions`; lexicographic sort == chronological sort; Spotify already hands us ISO-8601 (`added_at`, `liked_at`), so no lossy conversion. **Departure from the client**, which uses `INTEGER` `unixepoch()`. |
| Booleans       | `INTEGER NOT NULL DEFAULT 0 CHECK (x IN (0,1))`                                                                                                          | SQLite idiom; matches the client's `BOOLEAN … DEFAULT FALSE`.                                                                                                                                                                        |
| Surrogate keys | `INTEGER PRIMARY KEY` (rowid alias)                                                                                                                      | Matches the client and `auth.md`'s expected FK type.                                                                                                                                                                                 |
| FKs            | Explicit `REFERENCES … ON DELETE …`                                                                                                                      | Real enforced FKs. The pool **must** set `PRAGMA foreign_keys = ON` per connection (also flagged in `deployment.md` §3.2).                                                                                                           |
| IDs            | `hub_tracks.id`, `hub_playlists.id`, `hub_users.id` are **local** surrogates. Remote identities are the `service_track_id` / `playlist_id` TEXT columns. | Remote ids are never assumed to be integers.                                                                                                                                                                                         |

Remote Spotify timestamps (`added_at`, `liked_at`) are stored as the ISO-8601
string Spotify returns (e.g. `2021-03-05T09:16:31Z`); they are nullable because
`playlist_items`/`saved_tracks` may omit them.

---

## 2. Migration 001 DDL (full SQL)

File: `mmm-hub/migrations/001_hub_schema.sql`. Additive, hub-owned chain. Six
tables, their indexes, and the four views from §3 all live in this one file so
M1 is a single migration (contract §1: "one net-new migration where possible").

```sql
-- 001_hub_schema.sql — MMM Hub core data model (M1).
-- Additive, hub-owned chain; never touches the client chain in ../migrations/.
-- Timestamps are ISO-8601 UTC TEXT (see plans/mmm-hub/data-and-ingest.md §1).

-- ── hub_users ────────────────────────────────────────────────────────────────
-- Generic hub account, keyed by the OIDC subject. Integer PK is the type the
-- sibling auth.md assumes for hub_sessions.user_id.
CREATE TABLE hub_users (
    id            INTEGER PRIMARY KEY,
    oidc_subject  TEXT    NOT NULL UNIQUE,   -- OIDC 'sub' claim
    display_name  TEXT,
    email         TEXT,
    created_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
-- UNIQUE(oidc_subject) already provides the lookup index; no extra index needed.

-- ── hub_service_accounts ─────────────────────────────────────────────────────
-- One linked streaming account per (user, service). v1 stores Spotify OAuth
-- tokens; the CHECK already admits the later providers the contract names.
CREATE TABLE hub_service_accounts (
    id              INTEGER PRIMARY KEY,
    user_id         INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    service         TEXT    NOT NULL CHECK (service IN ('spotify','soundcloud','youtube')),
    remote_user_id  TEXT,                    -- Spotify GET /me -> id
    display_name    TEXT,                    -- Spotify GET /me -> display_name
    access_token    TEXT,
    refresh_token   TEXT,
    token_expiry    TEXT,                    -- ISO-8601 UTC
    authorized_at   TEXT,                    -- ISO-8601 UTC of the ORIGINAL Spotify
                                             -- authorization; starts the 6-month
                                             -- refresh-token clock (§4.1). Additive
                                             -- to the contract's §5 column list.
    reconnect_required INTEGER NOT NULL DEFAULT 0 CHECK (reconnect_required IN (0,1)),
                                             -- set on 400 invalid_grant; cleared only
                                             -- by a fresh authorize flow (§4.1, §6.1)
    scopes          TEXT,                    -- space-separated scope string
    connected_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    UNIQUE (user_id, service)
);
CREATE INDEX idx_hub_service_accounts_user_id ON hub_service_accounts(user_id);
CREATE INDEX idx_hub_service_accounts_service ON hub_service_accounts(service);

-- ── hub_tracks ───────────────────────────────────────────────────────────────
-- GLOBAL track identity, shared across users. Canonical id is first-seen.
CREATE TABLE hub_tracks (
    id               INTEGER PRIMARY KEY,
    service          TEXT    NOT NULL CHECK (service IN ('spotify','soundcloud','youtube')),
    service_track_id TEXT    NOT NULL,
    isrc             TEXT,
    title            TEXT,
    artists          TEXT,                   -- comma-joined display string
    album            TEXT,
    duration_ms      INTEGER,
    explicit         INTEGER NOT NULL DEFAULT 0 CHECK (explicit IN (0,1)),
    image_url        TEXT,                   -- largest album image
    first_seen_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    UNIQUE (service, service_track_id)
);
-- UNIQUE(service, service_track_id) is the primary dedup index.
-- ISRC is a *nullable, non-unique secondary attribute* (see §4.9) — never an
-- identity/merge key: Spotify returns several track ids that share one ISRC.
CREATE INDEX idx_hub_tracks_isrc            ON hub_tracks(isrc);
CREATE INDEX idx_hub_tracks_service_isrc    ON hub_tracks(service, isrc);

-- ── hub_playlists ────────────────────────────────────────────────────────────
-- Per-user playlist. The SAME Spotify playlist linked by two users is two rows
-- (different user_id) — name matching across users is done in the views.
CREATE TABLE hub_playlists (
    id           INTEGER PRIMARY KEY,
    user_id      INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    service      TEXT    NOT NULL CHECK (service IN ('spotify','soundcloud','youtube')),
    playlist_id  TEXT    NOT NULL,           -- remote (Spotify) playlist id
    name         TEXT,
    description  TEXT,
    is_liked     INTEGER NOT NULL DEFAULT 0 CHECK (is_liked IN (0,1)),
    track_count  INTEGER,                     -- remote tracks.total
    snapshot_id  TEXT,                        -- remote snapshot for change detection
    fetched_at   TEXT,                        -- last successful track fetch (ISO-8601)
    UNIQUE (user_id, service, playlist_id)
);
CREATE INDEX idx_hub_playlists_user_id             ON hub_playlists(user_id);
CREATE INDEX idx_hub_playlists_service_playlist_id ON hub_playlists(service, playlist_id);
CREATE INDEX idx_hub_playlists_name                ON hub_playlists(name);

-- ── hub_playlist_tracks ──────────────────────────────────────────────────────
-- Membership. PK(playlist_id, track_id) collapses a track occurring twice in the
-- same playlist into one membership row (acceptable for metadata-only v1; the
-- last-seen position wins). Both FKs cascade so deleting a playlist or a track
-- cleans membership up automatically.
CREATE TABLE hub_playlist_tracks (
    playlist_id INTEGER NOT NULL REFERENCES hub_playlists(id) ON DELETE CASCADE,
    track_id    INTEGER NOT NULL REFERENCES hub_tracks(id)     ON DELETE CASCADE,
    position    INTEGER,
    added_at    TEXT,                          -- ISO-8601 from Spotify, nullable
    PRIMARY KEY (playlist_id, track_id)
);
CREATE INDEX idx_hub_playlist_tracks_track_id ON hub_playlist_tracks(track_id);

-- ── hub_liked_tracks ─────────────────────────────────────────────────────────
-- Likes are modelled as their OWN relation, not a synthetic playlist (§4.5).
CREATE TABLE hub_liked_tracks (
    user_id  INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    track_id INTEGER NOT NULL REFERENCES hub_tracks(id) ON DELETE CASCADE,
    liked_at TEXT,                             -- ISO-8601 from Spotify, nullable
    PRIMARY KEY (user_id, track_id)
);
CREATE INDEX idx_hub_liked_tracks_track_id ON hub_liked_tracks(track_id);
```

Notes on choices the contract left open:

- **`hub_tracks` has no `updated_at`/`last_seen_at`.** Every ingest run touches
  the row (metadata refresh) but the meaningful timestamps live on the
  memberships (`hub_playlist_tracks.added_at`, `hub_liked_tracks.liked_at`) and
  on `hub_playlists.fetched_at`. Adding a `last_seen_at` later is a non-breaking
  migration if needed.
- **`hub_liked_tracks.liked_at`** is the Spotify `added_at` of the saved-track
  entry — the closest thing Spotify exposes to a like timestamp.
- **Two additive columns on `hub_service_accounts`** for the 6-month
  refresh-token expiry (§4.1): `authorized_at` (original authorization time) and
  `reconnect_required` (set on `invalid_grant`). Both are additions to the
  contract's §5 column list; no contract column is renamed or removed (flagged
  in §7).
- **No tombstone/deleted columns.** Remote deletions are handled by hard deletes
  plus `ON DELETE CASCADE` (§6.5); v1 is metadata-only and does not need history.

`hub_sessions` is created in `002_hub_sessions.sql` (owned by `auth.md` §2.1) and
references `hub_users(id) ON DELETE CASCADE`.

---

## 3. View DDL (full SQL) and semantics

All four views read from the base tables directly. `track_id` is always the local
`hub_tracks.id`; `user_id` is always `hub_users.id`. In `hub_v_track_presence` /
`hub_v_track_playlists`, `playlist_id` is the **local** `hub_playlists.id`, and an
added `service_playlist_id` column carries the remote Spotify id for consumers
that need it (flagged in §7).

### 3.1 `hub_v_track_presence`

Every `(track, user, source)` presence. One row per playlist membership plus one
row per like. This is the atomic fact table the other three views aggregate.

```sql
CREATE VIEW hub_v_track_presence AS
SELECT
    hpt.track_id        AS track_id,
    hp.user_id          AS user_id,
    'playlist'          AS source,
    hp.id               AS playlist_id,
    hp.playlist_id      AS service_playlist_id,   -- added: remote Spotify id
    hp.name             AS playlist_name
FROM hub_playlist_tracks hpt
JOIN hub_playlists hp ON hp.id = hpt.playlist_id
UNION ALL
SELECT
    hlt.track_id        AS track_id,
    hlt.user_id         AS user_id,
    'liked'             AS source,
    NULL                AS playlist_id,
    NULL                AS service_playlist_id,
    NULL                AS playlist_name
FROM hub_liked_tracks hlt;
```

`UNION ALL` (not `UNION`) is correct: the two sub-selects are already
source-tagged and can never produce an identical row across the union, so the
extra dedup pass of `UNION` would only cost work. A track a user both liked _and_
put in two playlists yields three presence rows — that is intentional; the
aggregating views below dedupe to "distinct users".

### 3.2 `hub_v_track_playlists`

Playlist-only projection (no likes). Used to answer "which playlists is this
track in, and whose?".

```sql
CREATE VIEW hub_v_track_playlists AS
SELECT
    hpt.track_id AS track_id,
    hp.user_id   AS user_id,
    hp.id        AS playlist_id,
    hp.name      AS playlist_name
FROM hub_playlist_tracks hpt
JOIN hub_playlists hp ON hp.id = hpt.playlist_id;
```

Because likes live in `hub_liked_tracks`, `hub_playlists.is_liked` is always `0`
in v1 and this view is simply "all playlist memberships" — no filter needed. If a
future provider exposes likes as a real playlist, the filter `WHERE
hp.is_liked = 0` would be added; the column is kept for exactly that.

### 3.3 `hub_v_shared_tracks` — "≥ 2 distinct users"

```sql
CREATE VIEW hub_v_shared_tracks AS
SELECT
    track_id,
    COUNT(*)              AS user_count,
    GROUP_CONCAT(user_id) AS user_ids
FROM (
    SELECT DISTINCT track_id, user_id
    FROM hub_v_track_presence
    ORDER BY track_id, user_id
)
GROUP BY track_id
HAVING COUNT(*) >= 2;
```

Why this is correct:

- The inner `SELECT DISTINCT track_id, user_id` collapses _all_ the ways one user
  can touch a track (liked + N playlist memberships) into a **single** row. This
  is the whole point: the outer `COUNT(*)` then counts **distinct users**, not
  presence rows. Without the `DISTINCT`, a track a user liked _and_ playlisted
  and that another user playlisted would report `user_count = 3` for two users.
- `HAVING COUNT(*) >= 2` enforces the contract's "≥ 2 distinct users" at the
  user level, after dedup — never at the raw-presence level.
- `user_ids` is a comma-separated list (e.g. `1,2`). SQLite's `GROUP_CONCAT`
  order is not _formally_ guaranteed even with an ordered subquery; the
  `ORDER BY track_id, user_id` makes it deterministic in practice, and the test
  suite must compare it as a **parsed set**, not a string (§5). `user_count`
  equals the number of elements in `user_ids`.
- `DISTINCT` does not defeat the `ORDER BY`: SQLite feeds the aggregate in the
  subquery's order.

### 3.4 `hub_v_user_overlap` — pairwise overlap without double counting

```sql
CREATE VIEW hub_v_user_overlap AS
SELECT
    a.user_id AS user_a_id,
    b.user_id AS user_b_id,
    COUNT(*)  AS shared_tracks
FROM (SELECT DISTINCT track_id, user_id FROM hub_v_track_presence) a
JOIN (SELECT DISTINCT track_id, user_id FROM hub_v_track_presence) b
  ON a.track_id = b.track_id
 AND a.user_id < b.user_id
GROUP BY a.user_id, b.user_id;
```

Why this is correct:

- **One row per unordered pair.** The join predicate `a.user_id < b.user_id`
  emits each pair exactly once: `(1,2)` matches, `(2,1)` cannot, and `a.user_id =
b.user_id` (a user "overlapping" themselves) is excluded. Without it, every
  pair would appear twice — once as `(1,2)` and once as `(2,1)` — and every user
  would have a spurious self-pair.
- **No double counting within a pair.** Both sides are the `DISTINCT (track,
user)` subquery, so each shared track contributes exactly one joined row. A
  track that user A has via two playlists still counts once.
- **`COUNT(*)` = number of shared tracks** for that pair. It equals the count of
  `hub_v_shared_tracks` rows whose `user_ids` contains both users, but is
  computed directly from presence so it needs no string parsing.
- The view lists pairs with **≥ 1** shared track. "≥ 2 users" from §3.3 is the
  track-level notion; a pair is any two users sharing at least one track,
  including a track that only those two have.

The two `DISTINCT` subqueries are logically the inverse of `hub_v_shared_tracks`'s
inner query; an implementation may factor them into a common CTE if desired, but
SQLite views cannot share a CTE across definitions, so each view repeats it.

---

## 4. Spotify ingest algorithm (M3)

Entry point `ingest/spotify.rs`, one sync per user, single-flight (in-memory
per-user lock; a concurrent `POST /sync` returns the running task, not a second
ingest). All Spotify calls go through the user's `hub_service_accounts` row.

Scopes requested at link time: `playlist-read-private`,
`playlist-read-collaborative`, `user-library-read`, and `user-read-private`
(for `GET /me`). `user-read-email` is **not** requested: post-Feb-2026 `GET /me`
no longer returns `email` (nor `country`/`product`/`followers`), and the hub gets
the user's email from the OIDC login instead.

### 4.1 Build the client and refresh tokens

Read the `hub_service_accounts` row for `(user_id, 'spotify')`, construct
`rspotify::AuthCodeSpotify` with `token_refreshing = true` and the stored
tokens (same shape as the client's `SpotifyClient::from_stored_tokens`). If
`token_expiry` is within 60 s, refresh; write the new `access_token` /
`token_expiry` (and `refresh_token`, if rotated) back and bump `updated_at`.

**6-month refresh-token expiry → reconnect.** Spotify invalidates a refresh
token **6 months after the original authorization**; refreshing does **not**
reset the clock. On expiry the token endpoint returns **`400 invalid_grant`**.
This is a first-class account state, not a transient error:

- `hub_service_accounts.authorized_at` records when the authorization-code flow
  completed (the `auth.md` link callback). It is the only anchor for the clock —
  Spotify exposes **no issuance timestamp** on refresh tokens. Keep
  `authorized_at` unchanged across refreshes; reset it only when the user
  re-runs the authorize flow.
- On `400 invalid_grant` (or `401`) during refresh or any call: set
  `reconnect_required = 1`, **clear `access_token` and `refresh_token`** (both
  are dead), keep the row, and abort the run. Do **not** loop or apply backoff —
  no retry can revive an expired refresh token.
- `GET /api/hub/me` surfaces this as `needsReconnect: true` for the service
  (`connected: false`); ingest skips that user until re-linked. Clearing
  `reconnect_required` and issuing fresh tokens happens only via a new Spotify
  authorize flow (M2), which also resets `authorized_at`.
- **Pre-warn** (proposed additive field): while `reconnect_required = 0`,
  `/me` may include a `reconnectBy` timestamp (or a warning boolean) once
  `now >= authorized_at + 6 months − grace` (grace ≈ 14 days), so a user can
  re-authorize before ingest silently stops. Flagged in §7.

### 4.2 `GET /me`

`rspotify` `current_user()`. Upsert `hub_service_accounts.remote_user_id = id`,
`display_name`, `updated_at`. This anchors `remote_user_id` so the same Spotify
account cannot silently link under two hub users (a follow-up can add
`UNIQUE(service, remote_user_id)`; flagged in §7).

Post-Feb-2026 `GET /me` no longer returns `email`, `country`, `product`, or
`followers`; read **only** `id` and `display_name` here. The hub's `email` comes
from the OIDC login, and `remote_user_id` is later compared against playlist
`owner.id` to decide item access (§4.3/§4.4).

### 4.3 `GET /me/playlists` — the playlist list

- Parameters: `limit=50` (Spotify max for this endpoint), `offset` stepping by
  50 until `offset >= total` (or the response's `next` is null). `rspotify`'s
  `current_user_playlists_manual(limit, offset)` or the auto-paging stream both
  work; **use explicit offset paging** so we can check the process-wide 429
  cooldown between pages and so partial progress is obvious.
- Capture per playlist: `id.id()` (bare id — `Display` yields a URI, see the
  client's `rspotify_id_display_renders_a_uri_not_a_bare_id` test), `name`,
  `description`, `snapshot_id`, `items.total` (the playlist paging object was
  renamed `tracks`→`items` in Feb 2026), `owner.id`, and `collaborative`.
- Decide the **item-access class** here and carry it into §4.4:
  `can_read_items = (owner.id == remote_user_id) || collaborative`. Owned and
  collaborative playlists expose `items`; merely _followed_ playlists do
  **not** (§4.4).
- Upsert into `hub_playlists` keyed `(user_id, service, playlist_id)`
  (see §6.3). "Liked Songs" is **not** in this list — it is handled in §4.5.

### 4.4 `GET /playlists/{id}/items` — playlist members (owned/collaborative only)

**Access rule (Feb 2026).** The endpoint is `GET /playlists/{id}/items`
(renamed from `/tracks`) and returns `items` **only** for playlists the current
user **owns or collaborates on**. For a merely _followed_ playlist the response
carries **metadata only** (`items` absent) and the items endpoint answers
`403`.

- Branch on the `can_read_items` flag computed in §4.3:
  - **Owned / collaborative** → walk items as below.
  - **Followed** → **do not call this endpoint.** Store playlist metadata only
    (`name`, `description`, `items.total`, `snapshot_id`) and create **no**
    `hub_playlist_tracks` rows for it. Leave `fetched_at` NULL so consumers can
    tell a metadata-only playlist from a fetched one (flagged in §7). The same
    playlist owned by _another_ hub user is stored separately under that user
    and does get its memberships — the overlap model is unaffected.
- Parameters: `limit=50` (Feb-2026 default **20**, max **50**), `offset`
  stepping by 50 until the page's `total` is consumed; `additional_types=track`;
  `market=from_token` (via `Market::FromToken`). `rspotify`
  `playlist_items_manual(playlist_id, fields=None, limit, offset, market)`.
- **Response field renames**: top-level paging object `tracks`→`items`, each
  entry's track field `track`→`item` (and playlist `tracks`→`items`). The full
  track object is embedded at `items.items[].item`.
- Do **not** trim with `fields`: the default response carries the full track
  object, including `external_ids.isrc` and `album.images`; that is why no
  separate enrichment call is needed (§4.6).
- For each entry, `position = offset + index` (Spotify returns no position
  field). Take `entry.added_at` (ISO-8601) as `added_at`.
- **Filter entries**: keep only `entry.item` where `type == "track"`; **skip**
  episodes (`type == "episode"`), local files (`is_local == true`), and `None`
  (removed/unavailable). v1 is tracks-only, and local/episode entries have no
  stable `service_track_id`.
- Resolve/dedupe the track via §4.8/§4.9 and insert a membership row
  `(playlist_id_local, track_id_local, position, added_at)` with `ON CONFLICT
  (playlist_id, track_id) DO UPDATE SET position=excluded.position,
  added_at=excluded.added_at` (last-seen wins).
- Reconciliation is per playlist in one transaction: after the full page walk,
  `DELETE FROM hub_playlist_tracks WHERE playlist_id = ? AND track_id NOT IN
  (<seen>)`. Tracks the user removed disappear; the `hub_tracks` rows stay (they
  may be shared by others).

### 4.5 `GET /me/tracks` — likes

`GET /me/tracks` (scope `user-library-read`) is **unaffected** by the Feb-2026
changes and remains the canonical likes source.

- Parameters: `limit=50` (max), `offset` stepping by 50 until `total`. Each
  `SavedTrack` gives `track` (full) and `added_at`.
- Map the track via §4.8, then `INSERT INTO hub_liked_tracks(user_id, track_id,
  liked_at) … ON CONFLICT(user_id, track_id) DO UPDATE SET
  liked_at=excluded.liked_at` (refresh the timestamp on re-like).
- Reconcile the whole set: `DELETE FROM hub_liked_tracks WHERE user_id = ? AND
track_id NOT IN (<seen>)`. This is how unlikes are caught.

**Recommendation — dedicated table, not a synthetic playlist.** Model likes as
`hub_liked_tracks` (as the contract's §5 already does), i.e. presence with
`source = 'liked'` and no owning playlist. Reasons:

1. Likes have no `snapshot_id`, no name, no description, no track position;
   forcing them into `hub_playlists` means inventing a fake remote id and
   sentinel metadata. The client did exactly that (a synthetic "Liked" playlist)
   and had to bolt on `playlist_kind` (client migration `032`) to keep likes from
   distorting curated-playlist counts. The Hub can avoid that debt from day one.
2. `PRIMARY KEY(user_id, track_id)` makes the reconcile diff a trivial set
   operation and gives clean `ON CONFLICT` idempotency.
3. Presence `source='liked'` is a first-class value in the contract's
   `hub_v_track_presence`, so the views need no special-casing.

Consequently `hub_playlists.is_liked` stays `0` in v1 (kept for contract
compatibility / future providers).

### 4.6 Enrichment — embedded track objects, no batch endpoint

The batch endpoints were **removed** in Feb 2026: `GET /tracks?ids=`,
`/albums`, `/artists`, … no longer exist. **Do not call them**, and do not use
`rspotify`'s deprecated `tracks()` / `artists()` / `albums()` wrappers even
though 0.15 still exposes them (flagged in §7).

No separate enrichment step is needed: both `/playlists/{id}/items` (§4.4) and
`/me/tracks` (§4.5) embed the **full** track object, including
`external_ids.isrc` and `album.images`. Ingest maps fields straight from the
embedded object (§4.8); there is no id-batching, no cache, and no
`SimplifiedTrack` path.

If per-track data is ever needed for a single track, use `GET /tracks/{id}`
individually — but v1 avoids it entirely, both for quota and to keep ingest
one-directional (playlist/likes → DB).

### 4.7 Snapshot-aware re-sync

- `hub_playlists.snapshot_id` stores the Spotify `snapshot_id`. On each
  `/me/playlists` pass, for a **`can_read_items`** playlist already stored with
  an equal `snapshot_id`, **skip the `/playlists/{id}/items` walk** — only
  refresh `name`, `description`, `track_count` (from `items.total`),
  `fetched_at`. Because the snapshot
  changes on any content/order change, "equal snapshot ⇒ members unchanged" is
  sound. `track_count` may still differ on a snapshot-stable metadata edit
  (e.g. Spotify re-counting); we store it but do **not** re-fetch on count
  mismatch alone — snapshot is the source of truth.
- On a successful track walk, write the new `snapshot_id` and `fetched_at` in
  the same transaction as the membership changes, so a crash mid-walk leaves the
  old snapshot and the next run re-fetches (no half-applied snapshot).
- **Followed playlists** (§4.4): no items walk and no snapshot advance; only
  `name`/`description`/`track_count`/`snapshot_id` are refreshed, `fetched_at`
  stays NULL.
- Collaborative playlists update under multiple owners; `snapshot_id` still
  covers the membership state, so the skip remains valid.
- **Likes have no snapshot.** Reconcile them with a full fetch+diff every run.
  `current_user_saved_tracks_manual(None, Some(1), Some(0)).total` is a cheap
  probe; use it to skip only the degenerate `0 == stored 0` case, never as a
  general "unchanged" signal (an unlike plus a like keeps the total constant).

### 4.8 Field mapping — Spotify object → `hub_tracks`

| `hub_tracks` column | Spotify source (`FullTrack` / `SavedTrack.track`)                             |
| ------------------- | ----------------------------------------------------------------------------- |
| `service`           | constant `'spotify'`                                                          |
| `service_track_id`  | `track.id.id()` — **bare id**, not the URI                                    |
| `isrc`              | `track.external_ids.get("isrc")` (`Option<String>`)                           |
| `title`             | `track.name`                                                                  |
| `artists`           | artist names joined with `", "`                                               |
| `album`             | `track.album.name`                                                            |
| `duration_ms`       | `track.duration.num_milliseconds()`                                           |
| `explicit`          | `track.explicit` → `1`/`0`                                                    |
| `image_url`         | largest of `track.album.images` (max width, then max height; `None` if empty) |
| `first_seen_at`     | DB default on insert; **never** touched by `ON CONFLICT` update               |

`hub_playlists` mapping: `user_id`, `service='spotify'`, `playlist_id`,
`name`, `description`, `is_liked=0`, `track_count = items.total` (renamed from
`tracks.total`), `snapshot_id`, `fetched_at = now` for owned/collaborative
(and NULL for followed, §4.4).
`hub_playlist_tracks` mapping: local `playlist_id`, local `track_id`, `position`,
`added_at`.
`hub_liked_tracks` mapping: `user_id`, local `track_id`, `liked_at = added_at`.

Removed/deprecated Spotify fields are **not read**: `available_markets`,
`linked_from`, `popularity`, `preview_url`, and (from `/me`) `email`,
`country`, `product`, `followers`. `isrc` comes from `external_ids.isrc` and is
nullable (see §4.9).

### 4.9 Track dedup rules

**Single-tier identity: `(service, service_track_id)` only.**

1. Enforced by `UNIQUE(service, service_track_id)` and an atomic upsert. This is
   the workhorse: the same Spotify track across many users' playlists/likes
   resolves to one `hub_tracks` row, which is what makes the overlap views
   correct.
2. **ISRC is a nullable, non-unique secondary attribute — never an
   identity/merge key.** Spotify legitimately returns several track ids that
   share one ISRC (single vs. album vs. remaster), and some items carry none at
   all. Two Spotify track ids that share an ISRC are therefore **two distinct
   `hub_tracks` rows by design**; there is no ISRC fallback and no alias table.
   `isrc` is stored for display/search only (its indexes remain, §2).

Resolution pseudocode (inside the per-item transaction):

```
resolve_track(service, track_id, isrc) -> local track_id:
    if row = SELECT id FROM hub_tracks WHERE service=? AND service_track_id=?:
        UPDATE metadata (never service_track_id, never first_seen_at)
        return row.id
    INSERT INTO hub_tracks(service, service_track_id, isrc, title, artists,
                           album, duration_ms, explicit, image_url) VALUES (...)
    return last_insert_rowid()
```

Consequences:

- Cross-user dedup is proven on `service_track_id` — never on ISRC (see the
  seed fixture, §5).
- `isrc` may be `NULL`; `idx_hub_tracks_isrc` / `idx_hub_tracks_service_isrc`
  remain useful for ad-hoc lookups but carry **no uniqueness**.
- Within a single sync run, keep a `HashMap<service_track_id, local_id>` to
  avoid re-querying for ids repeated across pages and playlists.

---

## 5. Seed fixture spec (M1 tests, no network)

Three users, five tracks, four playlists, overlapping likes and playlists. It
deterministically exercises exactly the contract's four required situations. Ids
are fixed so tests compare exact values.

**Users**

| id  | oidc_subject | display_name | email             |
| --- | ------------ | ------------ | ----------------- |
| 1   | `sub-alice`  | Alice        | alice@example.com |
| 2   | `sub-bob`    | Bob          | bob@example.com   |
| 3   | `sub-carol`  | Carol        | carol@example.com |

**Tracks** (`hub_tracks`, all `service='spotify'`)

| id  | service_track_id  | title           | artists | isrc      | explicit | duration_ms |
| --- | ----------------- | --------------- | ------- | --------- | -------- | ----------- |
| 1   | `trk_shared_like` | Shared Like     | A       | `ISRCAAA` | 0        | 180000      |
| 2   | `trk_shared_pl`   | Shared Playlist | B       | `ISRCBBB` | 0        | 200000      |
| 3   | `trk_solo`        | Solo            | C       | `ISRCCCC` | 1        | 210000      |
| 4   | `trk_both`        | Both Sources    | D       | `ISRCDDD` | 0        | 190000      |
| 5   | `trk_iso_twin_b`  | Iso Twin (b)    | E       | `ISRCEEE` | 0        | 175000      |

**Playlists** (`hub_playlists`)

| id  | user_id | service | playlist_id   | name        | snapshot_id | track_count | is_liked |
| --- | ------- | ------- | ------------- | ----------- | ----------- | ----------- | -------- |
| 1   | 1       | spotify | `pl_alice_wh` | Warehouse   | `snapA1`    | 2           | 0        |
| 2   | 2       | spotify | `pl_bob_wh`   | Warehouse   | `snapB1`    | 2           | 0        |
| 3   | 3       | spotify | `pl_carol_ch` | Chill       | `snapC1`    | 1           | 0        |
| 4   | 1       | spotify | `pl_alice_ex` | Alice Extra | `snapD1`    | 1           | 0        |

Playlists **1 and 2 share the name `Warehouse`** (same name, different remote
ids, different owners) — the "shared by name" case.

**Playlist memberships** (`hub_playlist_tracks`)

| playlist_id | track_id | position | added_at               |
| ----------- | -------- | -------- | ---------------------- |
| 1           | 2        | 0        | `2024-01-01T00:00:00Z` |
| 1           | 4        | 1        | `2024-01-02T00:00:00Z` |
| 2           | 2        | 0        | `2024-02-01T00:00:00Z` |
| 2           | 4        | 1        | `2024-02-02T00:00:00Z` |
| 3           | 3        | 0        | `2024-03-01T00:00:00Z` |
| 4           | 1        | 0        | `2024-04-01T00:00:00Z` |

**Likes** (`hub_liked_tracks`)

| user_id | track_id | liked_at               |
| ------- | -------- | ---------------------- |
| 1       | 1        | `2024-05-01T00:00:00Z` |
| 2       | 1        | `2024-05-02T00:00:00Z` |
| 3       | 3        | `2024-05-03T00:00:00Z` |
| 1       | 4        | `2024-05-04T00:00:00Z` |

**What each requirement maps to**

| Requirement                 | Fixture evidence                                                                 | Expected view result                                                           |
| --------------------------- | -------------------------------------------------------------------------------- | ------------------------------------------------------------------------------ |
| Track liked by 2 users      | track 1 liked by users 1 and 2                                                   | `hub_v_shared_tracks`: track 1, `user_count=2`, `user_ids` set `{1,2}`         |
| Track in 2 users' playlists | track 2 in playlist 1 (user 1) and playlist 2 (user 2)                           | `hub_v_shared_tracks`: track 2, `user_count=2`, `{1,2}`                        |
| Track only one user has     | track 3 in playlist 3 + liked by user 3 only                                     | absent from `hub_v_shared_tracks`; presence rows are user 3 only               |
| Playlist shared by name     | playlists 1 and 2 both named `Warehouse`                                         | two `hub_playlists` rows; `hub_v_track_playlists` shows both names for track 2 |
| **Dedup stress**            | track 4: user 1 liked it _and_ has it in playlist 1, user 2 has it in playlist 2 | presence has 3 rows for track 4 but `user_count=2` (Alice counted once)        |

**Expected `hub_v_shared_tracks`** (order-independent; `user_ids` compared as a set)

| track_id | user_count | user_ids (set) |
| -------- | ---------- | -------------- |
| 1        | 2          | {1, 2}         |
| 2        | 2          | {1, 2}         |
| 4        | 2          | {1, 2}         |

**Expected `hub_v_user_overlap`**

| user_a_id | user_b_id | shared_tracks |
| --------- | --------- | ------------- |
| 1         | 2         | 3             |

No other pair shares a track: user 3 shares nothing with 1 or 2 (track 3 is
solo). If the API ever changes the pair ordering, sort `(user_a_id, user_b_id)`
in the test.

**Expected `hub_v_track_presence` for track 4** (the dedup case)

| track_id | user_id | source   | playlist_id | playlist_name |
| -------- | ------- | -------- | ----------- | ------------- |
| 4        | 2       | playlist | 2           | Warehouse     |
| 4        | 1       | liked    | NULL        | NULL          |
| 4        | 1       | playlist | 1           | Warehouse     |

(three rows; two distinct users). Fixture goes in `mmm-hub/src/db/testing.rs`,
mirroring the client's `seed_*_scenario` style: idempotent `INSERT OR IGNORE`
with explicit ids so tests can assert exact counts.

`trk_iso_twin_b` (track 5, `ISRCEEE`) is present but unreferenced in the base
fixture; it is the input for the dedicated "ISRC is not an identity" test, which
inserts a _second_ `service_track_id` carrying the same `ISRCEEE` and asserts a
**new** `hub_tracks` row **does** appear (§4.9).

---

## 6. Reliability

### 6.1 Rate limits / backoff (429 vs. QUOTA_EXCEEDED)

Spotify rate-limits over a **rolling 30-second window** per app and returns
HTTP **429**. Reuse the client's proven pattern: walk the `anyhow` error chain
for `rspotify::ClientError::Http` carrying HTTP 429 and read the `Retry-After`
header (seconds). Mirror `spotify/retry.rs::extract_retry_after_secs` and
`spotify/cooldown.rs`.

**Two distinct 429s — distinguish them by the body, not just the status:**

| Signal                       | `Retry-After` | Body                                                     | Meaning                                                        | Handling                                                                   |
| ---------------------------- | ------------- | -------------------------------------------------------- | -------------------------------------------------------------- | -------------------------------------------------------------------------- |
| Rate limit                   | present       | `{"error":{"status":429,"message":"Too many requests"}}` | rolling-window limit hit                                       | process-wide cooldown from `Retry-After`; abort cycle, resume next run     |
| Dev-mode **quota** exhausted | **absent**    | `…"reason":"QUOTA_EXCEEDED"`                             | lower dev-mode quota spent (counted **per developer account**) | do **not** use `Retry-After`; longer, quota-specific cooldown; abort cycle |

- On a normal rate-limit 429, record a **process-wide** cooldown
  (`note_retry_after`, monotonic, only ever extends, capped — Spotify hands out
  up to ~24 h). Do **not** sleep and retry inline: inline retries inside the
  penalty window are what keep the limit saturated.
- On a **`QUOTA_EXCEEDED`** 429 (no `Retry-After`), do **not** retry or treat it
  as a short rate limit. Abort the cycle and set a longer, distinct cooldown
  (quota refills over a much wider window; make the backstop configurable). Log
  it at `warn` distinctly, and it may set a per-service "degraded/exhausted"
  hint on `/api/hub/me` (`rspotify` 0.15 may not surface `reason` in its typed
  error — parse the response body / error string to detect it; flagged in §7).
- Abort the current sync cycle at the next page boundary; already-committed
  per-playlist transactions stand. The next run resumes.
- For transient 5xx / network errors (not 429): bounded exponential backoff,
  3 attempts, ~500 ms → 1 s → 2 s with jitter, then fail that unit and continue.

### 6.2 Partial-failure behavior

- Per-playlist transaction: a failure while walking one playlist never rolls
  back another. Playlists already processed keep their new snapshot; the failed
  one keeps its old snapshot, so the next sync re-fetches it.
- List-level failures (network down, token expired) abort the run early; the
  `hub_service_accounts` row records the error state for `/api/hub/me`.
- **Expired refresh token** (`400 invalid_grant`) is not a transient failure: it
  sets `reconnect_required = 1`, clears the dead tokens, and aborts the run
  (§4.1). Ingest for that user is skipped until re-linked.
- **`QUOTA_EXCEEDED`** aborts the run like a rate limit but under a distinct,
  longer cooldown (§6.1); partial per-playlist commits still stand.
- The sync returns a per-user summary (`playlists_synced`, `playlists_skipped`,
  `playlists_failed`, `tracks_upserted`, `likes_upserted`) and logs it; the
  contract's `POST /sync` only needs `{ started: true }`, so the summary is
  internal/for `tracing` in v1.

### 6.3 Idempotent upserts

All writes are safe to rerun:

```sql
-- track (primary identity path)
INSERT INTO hub_tracks
    (service, service_track_id, isrc, title, artists, album, duration_ms, explicit, image_url)
VALUES (?,?,?,?,?,?,?,?,?)
ON CONFLICT(service, service_track_id) DO UPDATE SET
    isrc        = COALESCE(excluded.isrc, hub_tracks.isrc),
    title       = excluded.title,
    artists     = excluded.artists,
    album       = excluded.album,
    duration_ms = excluded.duration_ms,
    explicit    = excluded.explicit,
    image_url   = COALESCE(excluded.image_url, hub_tracks.image_url);
-- first_seen_at is deliberately absent from the UPDATE list.

-- playlist
INSERT INTO hub_playlists
    (user_id, service, playlist_id, name, description, is_liked, track_count, snapshot_id, fetched_at)
VALUES (?,?,?,?,?,?,?,?,?)
ON CONFLICT(user_id, service, playlist_id) DO UPDATE SET
    name = excluded.name, description = excluded.description,
    track_count = excluded.track_count,
    snapshot_id = COALESCE(excluded.snapshot_id, hub_playlists.snapshot_id),
    fetched_at  = excluded.fetched_at;

-- membership (last-seen position wins)
INSERT INTO hub_playlist_tracks (playlist_id, track_id, position, added_at)
VALUES (?,?,?,?)
ON CONFLICT(playlist_id, track_id) DO UPDATE SET
    position = excluded.position, added_at = excluded.added_at;

-- like
INSERT INTO hub_liked_tracks (user_id, track_id, liked_at)
VALUES (?,?,?)
ON CONFLICT(user_id, track_id) DO UPDATE SET liked_at = excluded.liked_at;
```

Every upsert keys on `(service, service_track_id)` / `(user_id, service,
playlist_id)` / `(playlist_id, track_id)` / `(user_id, track_id)` — never on
ISRC (§4.9). Membership writes happen only for owned/collaborative playlists
(§4.4).

### 6.4 Concurrency

- Single-flight per `(user_id, service)`: an in-memory set of running syncs (the
  Hub has no `TaskManager` yet) rejects a second concurrent sync for the same
  user.
- SQLite: WAL mode plus `busy_timeout` on every connection (flagged in
  `deployment.md` §3.2) so concurrent reads during a sync do not error. Writes
  from different users' syncs serialize on the single writer; keep transactions
  short (per playlist, per likes page-batch).

### 6.5 Remote playlist deleted

- On each `/me/playlists` pass, compute the set of remote playlist ids. Any
  stored `hub_playlists` row for this user whose `playlist_id` is **absent**
  remotely is deleted: `DELETE FROM hub_playlists WHERE user_id=? AND service=?
  AND playlist_id NOT IN (<remote set>)`. `ON DELETE CASCADE` removes its
  `hub_playlist_tracks`. `hub_tracks` rows are untouched (other users may
  reference them).
- Likes: the full diff in §4.5 deletes `hub_liked_tracks` rows no longer present,
  same effect.
- v1 hard-deletes because there is no tombstone column and the scope is
  metadata-only; a soft-delete `deleted_at` (like the client's migration `008`)
  is the obvious later addition if "was in a playlist that vanished" ever needs
  to be queryable.

---

## 7. Open questions / risks

1. **Timestamp representation.** I adopted ISO-8601 `TEXT` everywhere to match
   `auth.md`'s `hub_sessions`; the client uses `INTEGER unixepoch()`. This is a
   deliberate hub-wide choice — please confirm the coordinator wants the hub to
   diverge from the client rather than the reverse (which would require
   `auth.md` to change).
2. **`hub_users.id` type.** I used `INTEGER PRIMARY KEY` (rowid alias) because
   `auth.md` §2.1 declares `hub_sessions.user_id INTEGER NOT NULL REFERENCES
   hub_users(id)`. If the coordinator prefers the OIDC subject as the PK, both
   documents change together.
3. **Meaning of `playlist_id` / `track_id` in views and URLs.** The contract
   fixes the column names but not whether they are local surrogates or remote
   ids. I defined them as **local** (`hub_playlists.id`, `hub_tracks.id`) and
   added `service_playlist_id` to the presence view so callers can still get the
   Spotify id. Confirm the API (`/api/hub/tracks/{id}`, `/overlap`) is expected
   to accept/return local ids; otherwise the views need a column swap.
4. **RESOLVED — ISRC is not an identity key.** Confirmed against the post-Feb-
   2026 API (`verification.md` fact 4c): `external_ids.isrc` is nullable and
   non-unique. §4.9 now dedups on `(service, service_track_id)` **only**; the
   former ISRC-fallback merge and its test are removed, and `isrc` is a stored
   secondary attribute. The one-ISRC-many-ids collision trade-off no longer
   exists (distinct ids stay distinct rows).
5. **`GROUP_CONCAT` ordering is not formally guaranteed.** I feed an ordered
   subquery and require tests to compare `user_ids` as a parsed set. If the
   coordinator wants byte-stable output, switch `user_ids` to `json_group_array`
   over a `WITH … ORDER BY` and define the JSON array as the contract.
6. **`hub_v_track_presence.playlist_id` addition.** `service_playlist_id` is an
   added column (§3.1). Additions are allowed by the contract, but the
   verification doc should not treat it as frozen.
7. **Unavailable / local / episode items are skipped.** Spotify playlist items
   can be `null`, local files, or `Episode`s. v1 stores tracks only, so presence
   is incomplete for those items. If a future requirement needs them, episodes
   need their own identity/table.
8. **`hub_playlist_tracks` collapses duplicate occurrences** of the same track in
   one playlist (PK on `(playlist_id, track_id)`). Metadata-only v1 does not care;
   position count could if "played twice" ever matters.
9. **DUPLICATE Spotify accounts across hub users.** A follow-up migration could
   add `UNIQUE(service, remote_user_id)` to `hub_service_accounts`; it is not in
   migration 001 because existing rows could violate it during early testing.
10. **Likes are fully re-fetched every sync** (no snapshot). For very large
    libraries this is the dominant quota cost. A stored `total` probe only skips
    the empty case; a real optimisation (delta on `added_at`) is out of scope.
11. **Multi-provider stretch.** The `service` CHECKs already admit
    `soundcloud`/`youtube`, but dedup/ingest is Spotify-only in v1. Cross-service
    merging would need a deliberate identity strategy (the ISRC fallback from
    item 4 is gone); likely an alias table mapping every observed
    `(service, service_track_id)` to a canonical track, deferred out of v1.
12. **RESOLVED — batch enrichment removed.** `GET /tracks?ids=` (and `/albums`,
    `/artists`, …) no longer exist post-Feb-2026; §4.6 was rewritten to rely on
    the full track object embedded in playlist-items and saved-tracks responses.
    No batch fallback remains.
13. **Followed (metadata-only) playlists have no dedicated column.** §4.4 stores
    followed playlists as metadata only and signals this with `fetched_at IS
    NULL` plus zero `hub_playlist_tracks` rows. Confirm that is enough, or add an
    explicit `items_available` / `is_followed` column to `hub_playlists`
    (additive, allowed).
14. **`/me` pre-warn field for the 6-month cliff.** §4.1 proposes an additive
    `reconnectBy` (or a warning boolean) in the `/api/hub/me` service object; the
    contract's response is a sketch, so this needs coordinator sign-off.
15. **rspotify 0.15 is pre-Feb-2026.** The pinned `rspotify` 0.15 still exposes
    the removed batch methods (`tracks()`, `artists()`, `albums()`, `user()`) and
    may not surface `reason:"QUOTA_EXCEEDED"` in its typed error. Ingest must
    avoid the removed methods and body-parse the quota case; consider bumping the
    pin to `0.16.x` (`verification.md` facts 8 and 10).

End of data-model and ingest plan. Migration `001`, the four views, the seed
fixture, and the Spotify ingest algorithm above are the concrete M1/M3 building
blocks; the open questions above are the only places where the frozen contract
left a genuine decision for the coordinator.
