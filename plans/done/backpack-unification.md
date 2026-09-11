## Plan: backpack-unification

**Status**: done ✅
**Branch**: fix/issue-32
**Issue**: #32 (PR 1 — Fundament: Migration + Backpack-Konzept-Verschmelzung)
**Migration needed**: yes — `025_backpack_concept.sql`

### Description

Verschmilzt die beiden bisher getrennten „Backpack"-artigen Konzepte zu **einem**
`Backpack`:

- **(a)** in MMM markierte Spotify-Playlists (`playlist_subscriptions`,
  `is_active = 1`), und
- **(b)** Tracks, deren Tags `backpack = 1` haben (Erbe über die
  Playlist→Tag-Kette, `v_track_tags`).

Der Backpack ist die deduplizierte Vereinigung (a) ∪ (b). Sie wird in **genau
eine** Spotify-Playlist `Backpack` materialisiert, und **nur diese eine** URL
wird an deemix submittet — statt N Einzel-Submits/Scan-Vorgängen. Die zwei
garantierten Effekte (Auto-Download `stem.m4a` > `flac` > `mp3` + prune-sicher)
wirken auf die gesamte Backpack-Menge.

### Migration 025 (Bestandsüberführung, kein Datenverlust)

Additiv + idempotent; greift die bestehende `deemix_downloads`-Tabelle an:

1. Fügt `deemix_downloads.is_backpack BOOLEAN NOT NULL DEFAULT 0` hinzu (Marker
   für die eine Transport-Row).
2. Aggregiert die N legacy per-Playlist-Rows in **eine** pending Backpack-Row
   (Status: aktiver Download gewinnt → `queued`; sonst `failed`; sonst
   `completed`; `track_count_*` summiert; letzte `error_message` übernommen).
   Bis zur ersten Laufzeit-Materialisierung steht dort die Sentinel-URL
   `https://open.spotify.com/playlist/backpack`.
3. Löscht die konsolidierten Legacy-Rows.

**Upgrade-Verhalten:**

- **Subscriptions** (`playlist_subscriptions`) bleiben unangetastet und werden
  zu Backpack-Mitgliedern (Quelle a, konsistent zu `is_active`).
- **`tags.backpack`-Flags** bleiben gültig und erben weiter (Quelle b).
- **`deemix_downloads`-Rows** werden in die eine Backpack-Row konsolidiert
  (Status-Aggregat, s. o.).
- **Laufende deemix-Queue-Einträge**: werden nicht verwaist gelassen — der
  Gesamtstand fließt in die eine `is_backpack = 1`-Row. Die reale Backpack-URL
  wird erst bei der ersten Materialisierung bekannt und ersetzt die
  Sentinel-Row (`crate::backpack::record_backpack_submit`).

`deemix_downloads` ist abgeleiteter Zustand (Spiegel der deemix-Queue + User-
Aktionen); die Konsolidierung verliert daher keine Quelldaten.

### Files to modify

- `src/backpack.rs` (neu) — Aggregation (a) ∪ (b), Dedupe, Signature,
  Materialisierung als eine Spotify-Playlist, deemix-Transport (`Transport = 1`),
  `record_backpack_submit` (Sentinel-Ersatz).
- `src/lib.rs` — Modul `backpack` registrieren.
- `src/db/settings.rs` — Settings-Keys `backpack.playlist_id` /
  `backpack.playlist_url` / `backpack.signature`.
- `src/db/files.rs` — `get_backpack_pull_candidates` + `get_backpack_size_stats`
  beziehen die Backpack-Menge über `crate::backpack::get_backpack_file_ids`.
- `src/db/storage.rs` — `get_prune_candidates` schützt die Backpack-Menge
  (prune-sicher).
- `src/poller.rs` — `start_subscription_poller` entfernt das N-Einzel-Submit und
  materialisiert stattdessen den einen Backpack.
- `migrations/025_backpack_concept.sql` (neu) — Konsolidierung + `is_backpack`.

### Testing

- Unit-Tests in `src/backpack.rs`: Aggregation/Dedupe (a) ∪ (b), stabile URI-
  Auflösung, File-Mapping, Signature, Materialisierung (gemockter
  Spotify/deemix-Transport, idempotent), Sentinel-Ersatz.
- `src/db/storage.rs`: `test_prune_candidates_excludes_backpack` (prune-sicher).
- `cargo build` + `cargo test --lib` grün (718 Tests).
- deemix-Stack ownen + echter e2e-Test bleiben **Folge-PR** (Abschnitt B).
