# Plan: hub-spotify-queue

**Status**: proposed
**Branch**: `feat/hub-spotify-queue`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: no

### Description

Show and control the **Spotify playback queue** from the hub. The now-playing
widget (issue #293/#294) already shows _what plays right now_; this adds the
**„Als Nächstes"** list next to it and a way to push a hub track into that
queue. Spotify's player endpoints make both directions possible:

| Direction  | Call                                                           | Scope                                                           |
| ---------- | -------------------------------------------------------------- | --------------------------------------------------------------- |
| Read queue | `GET /me/player/queue` → `{ currently_playing, queue: [...] }` | `user-read-currently-playing` **or** `user-read-playback-state` |
| Append     | `POST /me/player/queue?uri=spotify:track:…&device_id=…` (204)  | `user-modify-playback-state`                                    |

All three scopes are **already requested** by `src/spotify.rs::SCOPES`, so no
re-auth is needed for accounts connected after this ships (older grants without
`user-read-playback-state` fall back to the existing reconnect prompt).

### Known limits (drive the scope — see §Non-goals)

1. **Append only.** There is no insert-at-position, no delete, no reorder.
   „Als Nächstes" = hinten anhängen (plus optional `POST /me/player/next`).
2. **Premium required** for every write (Spotify docs: _"only works for users who
   have Spotify Premium"_).
3. **Active device required.** Without one → `404 NO_ACTIVE_DEVICE`. `device_id`
   is _"not guaranteed persistent"_ (docs) — fetch per call, never cache.
4. **No push/webhooks.** Polling only, as with the current 20 s now-playing poll.
5. **The `queue` list is the manually queued „Next up" items**, not the rest of
   the current context (playlist/album). It is practically capped (~20 items).
   The UI copy must not promise a playlist lookahead.
6. **Per user**, tied to the logged-in hub user's Spotify token.

### Milestones

**M1 — Read: the queue client** — `src/nowplaying.rs`

- `QueueItem { hub_track_id, title, artists, image, duration_ms }` + `Queue`
  (`playing: Option<Now>`, `items: Vec<QueueItem>`).
- `queue(pool, cfg, user_id) -> Result<Queue>`: GET `/me/player/queue`, resolve
  each item to a hub track, **ingest unknown items on the fly** (reuse
  `ingest::ingest_spotify_item`, same as `current()`).
- `has_queue_scope()` — stored grant contains `user-read-playback-state` **or**
  `user-read-currently-playing`. Factor the scope lookup shared with `has_scope`.
- Factor the artist-line / cover-url parsing shared with `current()` into small
  helpers (no behaviour change).

**M2 — Read: the „Als Nächstes"-Widget** — `src/web.rs`, `templates/`

- Route `GET /now-playing/queue` (htmx fragment), handler `now_queue_frag` +
  askama struct `NowQueue` mirroring the `NowPlaying` widget.
- `templates/_partials/now_queue.html`: list with cover, title (link to
  `/track/{id}` when we have the track), artists, duration; „Queue ist leer"
  and reconnect/error states.
- `templates/base.html`: second sticky host `#hub-queue-host`
  (`hx-trigger="load, every 30s, hub:queue-changed from:body"`) below the
  now-playing host + CSS.
- ~~`/now` page gains the same list~~ — dropped: the queue is a **shell** widget,
  so `/now`, the track page and every other authenticated page already get it.
  `shell_hosts_the_queue_widget_on_every_page` locks that in.

**M3 — Write: push to queue** — `src/nowplaying.rs`, `src/web.rs`, `templates/`

- `add_to_queue(pool, cfg, user_id, uri) -> Result<()>`; `spotify_uri(track_id)`
  = `spotify:track:{service_track_id}` from `hub_tracks`.
- `POST /now-playing/queue/add/{id}` (track id in the path — an `hx-vals`
  JSON blob tripped askama's `}}` lexer), htmx, refreshing the queue fragment
  via a `hub:queue-changed` event + toast — same shape as `now_tag`.
- Button on `/track/{id}` (in `_partials/track_actions.html`) and on tag-queue /
  backpack / tag rows where a Spotify id exists.
- Error surfaces, not silent: `NO_ACTIVE_DEVICE`, `403 PREMIUM_REQUIRED`,
  `403` scope, `429` → actionable toast text.

**M4 — Player transport** — `src/nowplaying.rs`, `src/web.rs`, `templates/`

- ✅ `Transport { Next, Previous, Play, Pause }` + `transport()`. The method split
  is **POST** for skip and **PUT** for play/pause (Spotify separates actions from
  state changes); a wrong verb 405s live and no mock would notice, so each
  command's method is pinned in the tests.
- ✅ `POST /now-playing/transport/{cmd}` (`204` + toast + `hub:now-changed`), with
  ⏮/⏯/⏭ buttons in the widget (`_partials/now_transport.html`), offered in the
  playing _and_ the idle branch.
- Play/pause direction comes from the **rendered** state (`is_playing`), not from a
  toggle endpoint — Spotify has none, and a server-side state read would cost an
  extra request per click.
- ✅ Whole **tag** as the playback context: `POST /now-playing/play/tag/{id}` →
  `PUT /me/player/play` with `{"uris": [...]}`, button on the tag page. Track ids
  come from the tag's materialized resolution (`backpacks::tracks_for_tag`, now
  `pub(crate)`), uris from `nowplaying::spotify_uris` — deliberately the _same_
  source as the single-track path.
- ⚠️ `PLAY_URIS_CAP = 100` is **our** cap, not a documented Spotify limit (the
  reference does not state one). A larger tag is truncated and the toast says so;
  move context-sized tags to `context_uri` + playlist if that ever bites.
- ✅ The same for a **backpack**: `POST /now-playing/play/backpack/{id}`
  (`backpacks::resolve_unbounded`) with an **owner check first** — a backpack of
  another user is refused _without_ calling Spotify. Button on `/backpacks`.
- ✅ Shared tail factored: `play_tracks()` (ids → uris → play → toast) and
  `now_changed_response()`; the three write handlers now share `no_write_scope()`.

### Progress

**M1, M2 (widget only), M3, M4 (transport + tag + backpack playback) are
implemented.** M2's `/now` item was dropped as redundant — see M2 above. The only
thing left is the **live verification runbook** below.

- Deliberate deviation from the M3 sketch: the gate is a new
  `nowplaying::has_playback_write_scope` (`user-modify-playback-state`), _not_
  `curate::has_write_scope` — the latter checks the playlist-modify scopes, which
  are the wrong ones for the player.
- `hx-vals='{"track_id": {{ id }}}'` does not survive askama's lexer; the track
  id therefore travels in the path.
- `NowPlaying.playing` turned out to mean "we have a track object", **not** "it is
  playing" (the main branch hardcodes it `true`). A new explicit `is_playing` (from
  `Now.playing`) drives the play/pause choice; `play_state` stays the display text.
- ⚠️ **Unverified against the live API** — every call is exercised only against
  mocks. `POST /me/player/next|previous` in particular was _not_ re-confirmed from
  the docs (that reference page fetch 404'd); the tests pin our assumption, they do
  not prove Spotify's. A 405 on the first live click is the tell. Queue read,
  queue append, play/pause, scopes and the error shapes were checked against the
  Player reference in the docs.
- Toasts go through `web.rs::hx_trigger`/`ascii_fold`: `HeaderValue` accepts high
  bytes but `to_str()` and browsers mangle them, so German typography is
  transliterated for the header only (HTML keeps proper umlauts). `now_tag` was
  routed through the same helper — it had this latent mojibake already.
- ⚠️ Environment, not code: `CARGO_TARGET_DIR` is shared with other checkouts of this
  repo (`~/.zshrc:129` exports `$HOME/.cargo-target`), so cargo reused/replayed foreign
  artifacts — it reported `src/toast.rs` missing and a `pages.rs:1014: json_str_ascii`
  that does not exist here, and one `cargo test` linked a stale lib without the new
  `nowplaying::*` symbols. A repo file cannot override it (`CARGO_TARGET_DIR` env >
  `build.target-dir`). **Decision (2026-10-16): leave the global env alone** — verify
  with `CARGO_TARGET_DIR=/tmp/mh-verify cargo test`, or `cargo clean -p music-hub`
  when a run looks impossible. Also recorded in `music-hub/AGENT.md` §8.

### Live verification (runbook)

Everything above is exercised **only against mocks**. Before trusting it — and
before deploying — walk this list on the hub with a real account. It is short and
decides the three things mocks cannot:

```bash
# 0. The grant must carry BOTH scopes (older connections lack them):
sqlite3 hub.db "SELECT scopes FROM hub_service_accounts WHERE service='spotify';"
#    expect: user-read-currently-playing user-read-playback-state user-modify-playback-state
#    missing → reconnect Spotify (/api/hub/services/spotify/connect)
```

| #   | Do                                                         | Confirms / falsifies                                                                                                                                       |
| --- | ---------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | Open any page, watch the „Als Nächstes“ widget             | `GET /me/player/queue` shape; **is it only the manually queued items, or the playlist lookahead too?** (our UI copy assumes the former)                    |
| 2   | `＋ Queue` on a track, then look at the Spotify app        | `POST /me/player/queue` appends where we think (end of queue)                                                                                              |
| 3   | ⏭ in the now widget                                       | **`POST /me/player/next` — the one method we could not re-confirm from the docs.** A 405 in the toast = it is PUT, and `Transport::method()` needs the fix |
| 4   | ⏸ / ▶                                                      | `PUT /me/player/pause` / `/play`, and that the direction matches the rendered state                                                                        |
| 5   | `▶ Tag abspielen` on a tag >100 tracks                     | `PUT /me/player/play` + the „Die ersten 100 von N“ toast (our cap, not a documented limit)                                                                 |
| 6   | `▶ Abspielen` on a backpack                                | ditto, plus the owner check                                                                                                                                |
| 7   | Pause the phone, close every Spotify client, then click ⏭ | expect `Kein aktives Spotify-Gerät …` (404 `NO_ACTIVE_DEVICE`)                                                                                             |
| 8   | Try 1–5 on a **free** account                              | expect `Das braucht Spotify Premium.` (403 `PREMIUM_REQUIRED`)                                                                                             |

Anything that fails here is a fix to `Transport::method()` / the uri source / the
message mapping — not a redesign.

### Acceptance Criteria

- [ ] `cd music-hub && cargo build` passes; `cargo test` stays green (~150 tests)
- [ ] `GET /now-playing/queue` without login → 401; without scope → reconnect hint
- [ ] With a queued track in the mocked Spotify API the fragment lists it and
      links `/track/{id}` (unknown item is ingested)
- [ ] Empty queue renders the „Queue ist leer" state, not an empty div
- [ ] `POST /now-playing/queue/add` appends via `POST /me/player/queue` with the
      resolved `spotify:track:…` uri; verified against a mock
- [ ] `NO_ACTIVE_DEVICE` / Premium / scope-403 produce distinct, readable errors
      (no 500, no silent no-op)
- [ ] No `cargo fmt` churn
- [ ] UI copy makes the append-only + „Next up"-only limits explicit

### Non-goals

- Reordering or removing queue entries (Spotify has no such endpoint).
- „Direkt nach dem laufenden Track einfügen" — impossible via the API.
- A durable local queue mirror / DB table (remote state is read live; no
  migration).
- Replacing the 20 s now-playing poll with the queue poll (possible — the queue
  response carries `currently_playing` — but not in this change; it would alter
  the listening-history/replay-guard timing in `nowplaying::record_listen`).
- Web Playback SDK / in-browser playback.
