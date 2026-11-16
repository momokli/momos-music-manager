# Plan: Apple Music als Ingest- und Metadaten-Quelle

**Status**: proposed
**Branch**: `feat/apple-music-source`
**Ready for review**: no
**Depends on**: Apple Developer Program ($99/Jahr) + MusicKit-Key (`.p8`) — dieselbe
Mitgliedschaft entsperrt die macOS-Notarization (`docs/RELEASE-ROADMAP.md` M4)
**Migration needed**: yes (`music-hub/migrations/056_apple_music_service.sql`)

Faktenblatt mit Quellen: [`music-hub/docs/apple-music.md`](../../music-hub/docs/apple-music.md).
Dieses Dokument entscheidet; das Faktenblatt belegt.

---

## 0. Entscheidung in einem Absatz

Der Hub bekommt **Apple Music als Playlist- und Metadaten-Quelle**, nicht als
Acquire-Quelle. Apple liefert kein Audio (FairPlay) und keine Audio-Features (kein
BPM/Key/Energy) — music-api bleibt damit unverändert der einzige Downloader und die
einzige BPM/Key-Autorität (ADR-065/079). Der Weg ist: Apple-Playlist → Tracks → **ISRC** →
bestehender Tag-Layer → bestehender `POST /orders`-Pfad. Alles unterhalb der ISRC ist
heute schon gebaut und wird nicht angefasst.

## 1. Scope

**In v1**

1. **Öffentliche Apple-Playlist per URL** importieren (Developer-Token genügt, kein
   User-Token, keine OAuth-Zustimmung). Wiederverwendung der bestehenden `/import`-Seite.
2. **User-Bibliothek lesen** — der **Hub** holt Library-Playlists + deren Tracks mit dem
   User-Token (derselbe Client wie für den Listening-Feed). Playlists werden damit
   Tag-Quellen. MusicKit braucht es nur zum **Beschaffen** des Tokens, nicht zum Lesen.
3. **ISRC-Nachlauf** für Library-Tracks (Apple liefert auf Library-Objekten kein ISRC).
4. **Metadaten in den Vault** (Titel/Artist/Album/Dauer/Explicit/Artwork) über den
   bestehenden `POST /metadata`.

5. **Listening-Feed** (der eigentliche Nutzen für den Handy-Fall): der Worker pollt
   `GET /v1/me/recent/played/tracks` und schreibt nach `hub_listening_history`
   (`source='apple_music'`, Spalte existiert seit `042_hub_listening_history.sql:11`),
   danach `crawl::touch_tracks` → ISRC → Vault (BPM/Key) → Order (FLAC).

**Zwei getrennte Fragen — nicht vermischen.**

1. **Wer beschafft den Token?** Braucht einen Browser: MusicKit JS (offiziell) oder
   Copy-Paste aus den DevTools (geliehen). **Antwort v1: kellner**, weil dort schon ein
   Device-Token und der Browser des Nutzers sind.
2. **Wer benutzt ihn?** Ein HTTP-Header — ortsunabhängig. **Antwort: der Hub**, weil nur
   er 24/7 läuft (Handy, Mac aus).

Diese Trennung macht die Route austauschbar (§3b): derselbe Worker benutzt einen Token,
egal ob er per `curl`, per kellner-Push oder später per MusicKit-Dialog entstanden ist.

**Der Hub hält den einzigen Apple-Client** (Library-Playlists, `recent/played`, Katalog-Hop)
und benutzt dafür einen Token pro Nutzer. kellner hält **keinen** Apple-Code — es
beschafft und transportiert nur den Token. Kein HTML-Scraping, keine zweite
Implementierung derselben API.

**Bewusst nicht in v1** (Ideen, keine Zusagen)

- Playlist **in** Apple schreiben (`POST /v1/me/library/playlists`) — symmetrisch zum
  Spotify-Push (`migrations/054_playlist_push_status.sql`), eigenes Ticket.
- Apple-Now-Playing, Apple-Ratings/Loved, Storefront-Fallback, Artist-Crawl über Apple.
- Apple Music **Feed** als Bulk-Quelle: die Feed-Bedingungen verbieten ausdrücklich die
  Nutzung für interne Systeme — nachgelesen in `music-hub/docs/apple-music.md` §6.
- Previews als Analyse-Ersatz: 30-s-Ausschnitt, kein Offset/Dauer-Feld, für BPM/Key
  unbrauchbar.

## 2. Faktenlage (verifiziert, nicht angenommen)

| Fakt                                                                                                                              | Konsequenz                                                                                                                    | Beleg                                                                                    |
| --------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------- |
| `hub_service_accounts.service` hat `CHECK (service IN ('spotify','soundcloud','youtube'))`                                        | Tabellen-Rebuild nötig; SQLite kann CHECK nicht `ALTER`n                                                                      | `music-hub/migrations/001_hub_schema.sql:20`                                             |
| Nichts referenziert `hub_service_accounts` per FK/View                                                                            | Rebuild ist gefahrlos **mit** aktivierten FKs                                                                                 | `grep -rn hub_service_accounts music-hub/migrations/` → nur 001, 005; geprüft 2026-11-16 |
| sqlx-SQLite ignoriert `-- no-transaction` und beginnt immer eine Transaktion                                                      | `PRAGMA foreign_keys=OFF` ist im 12-Step **nicht** nutzbar → Rebuild muss ohne FK-Abschaltung funktionieren (tut er, s. o.)   | `sqlx-sqlite-0.8.6/src/migrate.rs:136`                                                   |
| Apple-Library-Songs tragen **kein** `isrc`, nur `playParams.catalogId`                                                            | ISRC-Auflösung ist ein eigener Pipeline-Schritt, kein Attribut-Zugriff                                                        | `music-hub/docs/apple-music.md` §3                                                       |
| Batch-Limits: `filter[isrc]` = 25, `ids` = 300                                                                                    | 50 Playlists × 100 Tracks ≈ **68 Requests** im empfohlenen Weg (nicht 5 000)                                                  | ebd. §3                                                                                  |
| Music User Token: ~6 Monate, **kein** Refresh. Für aus Cookies gezogene Tokens widersprechen sich die Quellen (Tage vs. 6 Monate) | Reconnect-UX ist Pflicht; Laufzeit **messen** (401-Zeitstempel), nicht annehmen                                               | ebd. §1 + `plans/proposed/apple-music-source.md` §3b                                     |
| Developer-Token: ES256-JWT, `exp` ≤ 15 777 000 s                                                                                  | serverseitig aus `.p8` signierbar, kurz halten (7 Tage)                                                                       | ebd. §1                                                                                  |
| Rate-Limits sind **nicht** dokumentiert (nur 429)                                                                                 | Drosselung + Backoff einplanen, keine Quota-Zahlen erfinden                                                                   | ebd. §4                                                                                  |
| `needs_reconnect` meldet reconnect, sobald `refresh_token` leer/NULL ist                                                          | Apple (ohne Refresh-Token) würde **dauerhaft** „reconnect" zeigen → muss service-abhängig werden                              | `music-hub/src/api.rs:536-539`                                                           |
| `crawl::order_touched` filtert `isrc IS NOT NULL`                                                                                 | `touch_tracks` vor der ISRC-Auflösung ist ein stiller No-Op → Auflösung **vor** Touch                                         | `music-hub/src/crawl.rs:116-118`                                                         |
| `web::disconnect` ignoriert den Pfad-Parameter und hardcodet `'spotify'`                                                          | eine Zeile fixen deckt Apple mit ab                                                                                           | `music-hub/src/web.rs:3519, 3524`                                                        |
| `LOOP_NAMES` ist ein fixes 5er-Array                                                                                              | neuer Loop braucht einen zusätzlichen Namen, sonst nicht pausierbar/sichtbar                                                  | `music-hub/src/worker.rs:35`                                                             |
| music-api-Vault hat **keine** Track-ExtID-Satellitentabelle (nur Artist/Album)                                                    | Apple-Song-IDs bleiben im Hub (`hub_track_external_ids`); Vault-Erweiterung wäre totes Gewicht                                | `grep -rn sat_track_external music-api/src/` → 0 Treffer                                 |
| Vault-`record_source` ist im `/metadata`-Pfad auf `"hub"` hartcodiert (3 Stellen)                                                 | ohne Fix kann der Vault nie `applemusic` als Quelle zeigen                                                                    | `music-api/src/vault.rs:741, 753, 768`                                                   |
| `sat_track_meta` hat **keine** `genre`-Spalte; `source` wird bei jedem Push überschrieben                                         | Genre wird heute still verworfen; `source` darf **nicht** als Apple-Provenienz missbraucht werden (überschreibt BPM-Herkunft) | `music-api/src/vault.rs:151-154, 339-345`; `music-api/src/db.rs:991`                     |

## 3. Interface-Verträge (gelten, wenn gebaut wird)

- **Service-String im Hub: `apple_music`** (`hub_service_accounts`, `hub_playlists`,
  `hub_tracks`, `hub_track_external_ids`).
- **Vault-`record_source`: `applemusic`** — bewusste Inkonsistenz, wie in
  `plans/proposed/unified-work-queue.md:34` vorgesehen. Genau zwei Konstanten
  (`SERVICE`, `VAULT_SOURCE`) in `src/applemusic.rs`; Umrechnung nur an der Vault-Grenze.
- **`POST /orders` bleibt unverändert.** Apple ist kein `source`-Wert für Orders. Wer das
  später ändern will, ändert den Contract — nicht dieses Feature.
- **`POST /metadata` bleibt der Weg in den Vault** (kein neuer Endpoint).
- **Kein `source`-Feld in Apple-Metadaten-Pushes** (überschreibt die Provenienz der
  BPM/Key-Werte, s. Risiko R1). Provenienz über `recordSource`.
- API-Basis injizierbar (`APPLE_MUSIC_API_BASE`) — sonst ist der Ingest nicht testbar
  (Muster: `ingest_soundcloud_public_with_api_base`, `music-hub/src/ingest.rs:964`).

### 3a. Übergabe-Contract: `spelunke.playlist-import/v1`

Der Vertrag ist der Grund, warum ein PoC-Script nicht wegwerfbar ist: Script, Reseller
und offizielle API liefern **dasselbe** JSON, der Hub kennt nur diesen einen Eingang.

```json
{
  "schema": "spelunke.playlist-import/v1",
  "source": "apple_music",
  "playlist": {
    "remote_id": "pl.f4d106fed2bd41149aaacabb233eb5eb",
    "name": "Today's Hits",
    "description": "",
    "url": "https://music.apple.com/us/playlist/todays-hits/pl.f4d106…",
    "storefront": "us",
    "fetched_at": "2026-11-16T20:00:00Z"
  },
  "tracks": [
    {
      "position": 0,
      "remote_track_id": "6822497774",
      "isrc": "USUM72501234",
      "title": "Housewife",
      "artists": "Ashe",
      "album": "Housewife - Single",
      "duration_ms": 208810,
      "explicit": false,
      "image_url": "https://is1-ssl.mzstatic.com/…/300x300bb.jpg",
      "match": { "confidence": "exact", "note": null }
    }
  ]
}
```

Regeln:

- `isrc` ist das **einzige Pflichtfeld für den Order-Flow**; `null` ist erlaubt (Track wird
  gespeichert, aber `crawl::order_touched` überspringt ihn — gewollt, kein Fehler).
- `remote_track_id` → `hub_tracks.service_track_id`; `playlist.remote_id` →
  `hub_playlists.playlist_id`. Beide müssen pro Quelle stabil sein.
- `match.confidence` ∈ `exact` (ISRC vorhanden oder vom Script verifiziert) | `fuzzy`
  (nur auf Titel/Artist/Dauer gematcht) | `none`. Der Hub **verwirft nichts** davon, aber
  `fuzzy`/`none` muss in der UI sichtbar sein, sonst ordert jemand Fehlmatches.
- `source` ist der Hub-Service-String (`apple_music`); die Vault-Umrechnung auf
  `applemusic` passiert erst beim Metadaten-Push.

Hub-Eingang (neu, klein): `POST /api/hub/import/playlist` (Session-Auth, JSON-Body) →
upsert `hub_playlists` + Tracks + `hub_playlist_tracks`, danach `crawl::touch_tracks` nur
für Tracks mit ISRC. **Keine Migration nötig**: `hub_playlists.service` und
`hub_tracks.service` haben keinen CHECK (`001_hub_schema.sql:34, 51`), und es wird keine
`hub_service_accounts`-Zeile geschrieben.

### 3b. Token: Beschaffen vs. Benutzen

**Beschaffen** braucht einen Browser. **Benutzen** braucht nur einen Header. Deshalb ist
der Token ein austauschbarer Rand — der Worker dahinter bleibt derselbe.

| Variante                    | Wer beschafft                                                                                  | Wann sinnvoll                                                                                                       |
| --------------------------- | ---------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| **kellner pusht** (v1)      | kellner liest den Token aus seiner Config und ruft `POST /apple-music/token` mit Device-Bearer | Es gibt einen Mac mit Browser, und kellner ist schon gepaart                                                        |
| **Paste im Hub** (Fallback) | Formularfeld auf `/apple-music`                                                                | kein kellner zur Hand, einmaliger Test                                                                              |
| **MusicKit JS** (später)    | derselbe Endpoint, Token kommt aus dem Apple-Dialog                                            | wenn die Route offiziell wird                                                                                       |
| **Reseller-API**            | —                                                                                              | für den **Listening-Feed ungeeignet**: `/v1/me/*` braucht einen User-Token, den nur der Nutzer selbst erzeugen kann |

**Speicherung** (`hub_service_accounts`, `service = 'apple_music'`):
`access_token` = Music-User-Token, `refresh_token` = **NULL** (es gibt keinen),
`authorized_at` = Push-Zeitpunkt, `token_expiry` = **NULL**.
Keine erfundene 180-Tage-Heuristik: die Lebensdauer ist unbekannt (§2, Quellenwiderspruch).
Die UI zeigt „läuft seit N Tagen, Ablauf unbekannt", und ein 401 setzt
`likes_status = 'reauth'` und stoppt das Polling für diesen Account.

**Messen statt annehmen:** jeder 401 wird mit Zeitstempel geloggt. Nach ein paar Zyklen
steht die echte Lebensdauer im Log — erst dann rechnet die UI damit.

Läuft der Token ab, ist Nachlegen ein Handgriff (kellner-Push oder Paste). Bei einem
Apple-Nutzer ist das kein Betriebsproblem — es ist der Preis für die Route ohne
Developer-Programm.

## 4. Hub: Migration 056 (**Pflicht** für den Token-Speicher)

`hub_service_accounts.service` hat `CHECK (service IN ('spotify','soundcloud','youtube'))`
(`001_hub_schema.sql:20`). Sobald der Hub einen Apple-Token halten soll (#534), braucht er
eine Zeile mit `service = 'apple_music'` — ohne Rebuild schlägt jedes `INSERT` fehl.
SQLite kann CHECK nicht `ALTER`n, also Tabellen-Rebuild.

Eine Datei. Reihenfolge im SQL: neu anlegen → kopieren → alt droppen → umbenennen.

1. **`hub_service_accounts` rebuilden** — **Enumeration raus, Form rein**:
   `CHECK (length(service) BETWEEN 2 AND 32 AND service NOT GLOB '*[^a-z0-9_]*'
AND substr(service, 1, 1) GLOB '[a-z]')`.
   Grund: eine Liste (`… IN ('spotify','soundcloud','youtube','apple_music')`) hätte bei
   **jeder** weiteren Quelle einen neuen Rebuild erzwungen. Die fachliche Allowlist lebt
   im Code (`playlist_import::ALLOWED_SOURCES`), nicht im Schema.
   - Spalten aus 001 **plus** die drei `ALTER`-Spalten aus `005_likes_sync.sql`
     (`likes_synced_at`, `likes_status`, `likes_error`) — fehlt eine, ist es stiller
     Datenverlust.
   - `UNIQUE (user_id, service)` neu deklarieren: sie ist das Ziel von
     `ON CONFLICT(user_id, service)` (`music-hub/src/ingest.rs:71`); ohne sie scheitert
     `store_initial_tokens` zur **Laufzeit**, nicht beim Migrieren.
   - `id` mitkopieren (Deep-Link-/Datasette-Stabilität kostet nichts).
   - `ON DELETE CASCADE` auf `hub_users` neu deklarieren.
2. Abschließendes `SELECT 'Migration 056 applied: …' as status` (Hausstil).

**GLOB-Falle** (im ersten Testlauf aufgefallen): `*` ist in SQLite-GLOB ein beliebiger
Wildcard, keine Wiederholung der Zeichenklasse. `'[a-z][a-z0-9_]*'` prüft nur die ersten
zwei Zeichen und lässt `with-dash` durch — deshalb die Negativ-Form oben.

**Keine Miss-Cache-Tabelle**: Library-Tracks ohne `playParams.catalogId` lösen gar keinen
Katalog-Request aus — ein negativer Cache wäre toter Ballast.

**Warum das gefahrlos ist** (geprüft): nichts referenziert die Tabelle per FK oder View;
sqlx führt jede Migration in _einer_ Transaktion aus (`sqlx-sqlite-0.8.6/src/migrate.rs:136`)
und ignoriert `-- no-transaction`, also ist `PRAGMA foreign_keys=OFF` keine Option — der
Rebuild funktioniert aber auch mit aktiven FKs, weil es keine Inbound-Referenz gibt.

**Regeln**: 056 nach dem ersten Deploy **nie** editieren (Checksummen, `AGENT.md:95-99`).
Der Rebuild bleibt nur solange gefahrlos, wie **keine** neue Tabelle/View
`hub_service_accounts` referenziert — das ist ab jetzt eine Design-Regel im AGENT.md.
Runbook: nach dem Deploy einmal `sqlite3 hub.db "PRAGMA foreign_key_check"`.

## 5. Hub: Module

`src/applemusic.rs` (neu, in `lib.rs` registrieren), Aufbau analog `soundcloud.rs`
(Client mit injizierbarer Base) + `spotify.rs` (Pagination):

```rust
pub const SERVICE: &str = "apple_music";
pub const VAULT_SOURCE: &str = "applemusic";
pub const DEFAULT_API_BASE: &str = "https://api.music.apple.com/v1";
pub const DEV_TOKEN_TTL_SECS: i64 = 7 * 24 * 3600;

pub fn developer_token(creds: &Creds, ttl_secs: i64, origin: Option<&str>) -> Result<String>;
pub struct Client { /* base, dev_token, user_token: Option<String> */ }

impl Client {
    pub async fn library_playlists(&self) -> Result<Vec<Value>>;
    pub async fn library_playlist_tracks(&self, lib_id: &str) -> Result<Vec<Value>>;
    pub async fn library_songs(&self) -> Result<Vec<Value>>;
    pub async fn catalog_playlist(&self, storefront: &str, id: &str) -> Result<Value>;
    pub async fn catalog_songs_by_ids(&self, storefront: &str, ids: &[String]) -> Result<Vec<Value>>;
    pub async fn catalog_songs_by_isrc(&self, storefront: &str, isrcs: &[String]) -> Result<Vec<Value>>;
}

pub enum UrlKind { PublicPlaylist { storefront: String, id: String }, Unsupported }
pub fn classify_url(url: &str) -> UrlKind;
fn catalog_id(item: &Value) -> Option<String>;  // nur Library-Objekte: playParams.catalogId
```

`src/ingest.rs`:

```rust
pub async fn ingest_apple_public_playlist(pool, cfg, slug, url) -> Result<Summary>;
pub async fn ingest_apple_public_playlist_with_client(pool, cfg, slug, url, &Client) -> Result<Summary>;
pub async fn ingest_apple_library(pool, cfg, user_id, &Client) -> Result<Summary>;
pub async fn resolve_apple_isrcs(pool, cfg, &Client, limit: usize) -> Result<usize>;
```

Datenfluss und Regeln:

- **Öffentliche Playlist**: `classify_url` → `catalog_playlist` → `hub_playlists`
  (`service='apple_music'`, `items_available=1`) → `DELETE`+Reinsert `hub_playlist_tracks`.
  Katalog-Songs tragen `isrc` → `crawl::touch_tracks` **direkt** danach wirkt.
- **Library**: `library_songs` → `hub_liked_tracks` (user-scoped DELETE, Muster
  `ingest.rs:913-920`), `library_playlists` → je Playlist Tracks → `hub_playlists`.
  ISRC bleibt hier `NULL`.
- **ISRC-Nachlauf**: `hub_tracks.service='apple_music' AND isrc IS NULL` ⋈
  `hub_track_external_ids` (Katalog-ID) → `catalog_songs_by_ids` in 100er-Chunks →
  `UPDATE hub_tracks SET isrc = COALESCE(isrc, ?)` → **danach** `touch_tracks`.
  Kein Catalog-ID → sofort Miss-Zeile, **kein** Request.
- **Dedup**: `UNIQUE(service, service_track_id)`; `service_track_id` = Apples top-level
  `id`. Library-IDs (`i.…`) erzeugen pro Nutzer eine Zeile — unvermeidbar, identisch zu
  SoundCloud; zusammengeführt wird downstream über die **ISRC**, nicht über `hub_tracks.id`.
- `hub_track_external_ids` bekommt die **Katalog**-ID (das ist die ID, auf die andere
  Dienste matchen; Index aus `021_external_id_lookup_index.sql`).
- Artists via `artists::link_track_artist_name` (Präzedenzfall SoundCloud) — Apple liefert
  keine stabilen Artist-IDs im Spotify-Shape.

## 6. Hub: Verbinden, Worker, Reconnect

**Auth-Flow** (`GET /apple-music` + `POST /apple-music/token`):

1. Server signiert den Developer-Token aus `.p8` (neue Dependency `p256`, pure Rust,
   `cargo build --locked` muss grün bleiben → `Cargo.lock` mitcommitten).
2. Seite lädt MusicKit JS vom Apple-CDN, `MusicKit.configure({developerToken, storefront})`,
   auf **Klick** `authorize()` → `musicUserToken`.
3. `POST /apple-music/token` → **Validierungs-Roundtrip** gegen
   `/v1/me/library/songs?limit=1`; 200 → speichern, 401/403 → nicht speichern + Flash.
4. Speicherung: `access_token = musicUserToken`, `refresh_token = NULL` (**bewusst**),
   `token_expiry = authorized_at + 180 d` (Heuristik, kommentieren), `scopes =
'music-user-token'`, danach `likes_status='queued'`.
5. Ins Frontend dürfen nur Developer-Token + Storefront. `.p8`, Team-/Key-ID und der
   Music-User-Token nie in HTML/Logs (Client-`Display` manuell auf `<redacted>`).

**Worker** (`worker.rs`): `LOOP_NAMES` um `"apple"` erweitern (sonst nicht pausierbar und
im `/admin`-Panel unsichtbar), eigener `tokio::spawn`-Block, ein Pass = drei Jobs:

| Job           | Auswahl                                                                                   | Cap                     |
| ------------- | ----------------------------------------------------------------------------------------- | ----------------------- |
| A `isrc`      | Tracks ohne ISRC, ohne Miss                                                               | 500/Pass, 100er-Batches |
| B `library`   | `service='apple_music' AND likes_status IN (NULL,'queued')`                               | 1 Account/Pass          |
| C `playlists` | `service='apple_music' AND enabled_for_fetch=1 AND (items_available=0 OR fetched_at alt)` | 6/Pass                  |

Job A zuerst (sonst wächst der ISRC-Rückstand und Orders passieren nie).
Apple hat kein `snapshot_id`-Äquivalent → **TTL-Refresh** statt Snapshot-Vergleich.
`enqueue_service_sync(pool, user_id, service)` als gemeinsamer Helper: die identischen
SQL-Blöcke in `api.rs:418-437` und `web.rs:806-820` werden dadurch ersetzt, statt einen
dritten zu schreiben.

**Fehlerbehandlung**:

| HTTP             | Ursache                   | Reaktion                                                                                                                                                               |
| ---------------- | ------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 401 (User-Token) | 6-Monats-Token abgelaufen | `likes_status='reauth'`, Klartext in `likes_error`, betroffene `hub_playlists.fetch_status='error'`; `access_token` **nicht** löschen (Banner braucht `authorized_at`) |
| 401 (Dev-Token)  | Key/Team falsch           | `likes_status='error'` → `/admin`-Panel, kein Reconnect nötig                                                                                                          |
| 403              | Storefront/Abo/Region     | `likes_status='error'` mit Klartext                                                                                                                                    |
| 429              | Rate-Limit                | ein bounded Retry (falls `Retry-After`), sonst 30 s und Pass beenden                                                                                                   |

**Reconnect-UX**: Zustände `NotConfigured → Disconnected → Connected → Expiring (ab 150 d)
→ Expired/Reauth`. Anzeige als Nav-Badge + Banner mit Restlaufzeit.
`needs_reconnect` (`api.rs:529-551`) wird service-abhängig: Spotify = Refresh-Token-Regel,
Apple = `authorized_at`-Alter bzw. `reauth`. **Ohne diesen Fix meldet `/api/hub/me` für
Apple dauerhaft „reconnect".** Reconnect ist kein OAuth-Redirect, sondern derselbe
Buttonklick → der Banner darf nicht auf `/api/hub/services/…/connect` zeigen.
Formulierung „erneut verbinden", nicht „erneuern": Silent-Renew ist beobachtbar, nicht
garantiert.

## 7. music-api: kleinster Eingriff

Was **ohne** Änderung funktioniert (belegt in `music-api/src/vault.rs`):
`title`/`artist`/`album` landen bereits in `sat_track_meta`; `duration_ms`/`explicit`/
`image_url` **haben Spalten**; Hubs/Links normalisieren nur Namen, ein Apple-Push trifft
dieselben Artist/Album-Hubs; `bpm`/`key`/`camelot` können nicht überschrieben werden
(`COALESCE` in `db.rs:983-990` + `Some`-Bedingung vor `load_analysis` in `vault.rs:748`).

Nötig:

1. **`MetadataItem` additiv erweitern** (`models.rs:209-228`): `duration_ms` (alias
   `durationMs`), `explicit`, `image_url` (alias `artworkUrl`), `record_source` (alias
   `recordSource`). Alle `Option` + `#[serde(default)]`; genau zwei Struct-Literale
   brechen (`worker.rs:1592`, `tests/api.rs:1869`) → `..Default::default()`.
2. **`ingest_metadata` durchreichen** (`vault.rs:733-775`): neue Felder an
   `load_track_meta*` (existiert schon für Dauer, `vault.rs:367-405`) und `record_source`
   statt des Literals `"hub"` (auch `vault.rs:741`).
3. **`vault::track_detail` härten** (`vault.rs:1128-1139`): die `hub`-View nimmt heute die
   _neueste_ `sat_track_meta`-Zeile ohne Source-Filter. Ein Teil-Push (nur Titel) würde
   `hub.durationMs`/`hub.explicit` auf `null` setzen, obwohl die Deezer-Zeile die Werte
   hat → pro Feld aus der neuesten **non-null** Zeile lesen.
4. **`/metadata`-Antwort um den Vault-Status** erweitern (`vaultUpserted`/`vaultError`):
   der Vault-Ingest ist heute best-effort und unsichtbar (`api.rs:508-515`) — für Apple
   ist der Vault das _einzige_ Ziel, also Totalverlust ohne Signal.
5. **Doku**: `music-api/README.md` §`/metadata` + §meta-Satellit.

Nicht geändert: kein neuer Endpoint, keine Track-ExtID-Tabelle (kein Leser), kein
`release_date` im Track-Push (würde den Album-Business-Key splitten, `vault.rs:77-83`),
kein `crawl:applemusic` (Leads sind im Code auf „Artist-Order → deemix" verdrahtet,
`worker.rs:1151-1166`).

## 8. Tests

Hub (`cargo test`, kein Playwright — `music-hub/AGENT.md:328`), „Apple ohne Apple" via
injizierbarer Base + In-Process-Mock (Muster `tests/ingest.rs:42-84`):

| Datei                        | Tests                                                                                                                                                                                                                                                                                                                                                                                  |
| ---------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tests/applemusic_schema.rs` | `apple_music` akzeptiert · `deezer` weiter abgelehnt · `UNIQUE(user_id,service)` überlebt · Altsplalten schreibbar · CASCADE überlebt · bestehende Account-Zeile unverändert                                                                                                                                                                                                           |
| `tests/applemusic.rs`        | `classify_url` (Playlist/Album/Müll) · öffentlicher Import setzt Playlist+Tracks+ISRC · Import idempotent · Library-Mapping (Upload ohne `catalogId` → Miss, kein Request) · Library löscht Spotify-Likes nicht · ISRC-Backfill (Treffer + Miss + **zweiter Lauf: 0 Requests**) · 401 → `reauth` · Apple-Playlists werden nie nach Spotify gepusht                                     |
| `tests/applemusic_ui.rs`     | `/apple-music` verlangt Login · Dev-Token nur im HTML wenn konfiguriert · Token-POST persistiert korrekt (`refresh_token IS NULL`, `token_expiry ≈ +180 d`) · abgelehnter Token ändert nichts · Sync-Endpoint öffnet `apple_music` (und lehnt `youtube` weiter ab) · Banner-Restlaufzeit · `/api/hub/me` meldet `needsReconnect=false` bei frischem Apple-Token (der `api.rs:538`-Bug) |
| Unit in `src/applemusic.rs`  | `developer_token` hat `alg=ES256`, `kid`, `iss`, `exp-iat ≤ 15777000`, Signatur verifizierbar mit **im Test generiertem** P-256-Key                                                                                                                                                                                                                                                    |

music-api (`music-api/tests/api.rs`, Harness vorhanden):

| Test                                                     | Assertion                                                                    |
| -------------------------------------------------------- | ---------------------------------------------------------------------------- |
| `metadata_push_writes_duration_and_explicit_to_sat_meta` | `satellites.meta[0].recordSource == "applemusic"`, Dauer/Explicit gesetzt    |
| `metadata_push_without_features_leaves_bpm_untouched`    | nach `bpm/key/source=traktor`-Push bleibt `source == "traktor"` (R1-Wächter) |
| `metadata_push_does_not_clear_hub_duration`              | Teil-Push senkt `hub.durationMs` nicht (R3-Wächter)                          |
| `metadata_push_creates_no_analysis_row_without_features` | `satellites.analysis` leer, `resolved.bpm == null`                           |
| `metadata_push_reports_vault_outcome`                    | `vaultUpserted` in der Antwort                                               |

Nicht testbar ohne echtes Apple-Konto: MusicKit-`authorize()`, Domain-/Origin-Verhalten,
echtes `playParams`-Shape → manueller Smoke-Test im Runbook.

## 9. Arbeitsschritte (Stand 2026-11-16)

**Der Hauptpfad** ist jetzt: Token in den Hub → Hub liest selbst → ISRC → music-api.
Das PoC-Script ist abgeschlossen (#522), sein Erkenntniswert steht im Faktenblatt.

| #   | Ticket                                                                            | Blocked by     |
| --- | --------------------------------------------------------------------------------- | -------------- |
| 1   | **#540** Migration 056 — `hub_service_accounts` + `apple_music` (CHECK-Rebuild)   | —              |
| 2   | **#534** Apple-Token annehmen + speichern (Device-Bearer, Validierungs-Roundtrip) | #540           |
| 3   | **#539** Apple-Bibliothek lesen (Playlists + Tracks + ISRC-Hop)                   | #534           |
| 4   | **#535** Listening-Feed → `hub_listening_history` + `needs_reconnect`-Fix         | #534           |
| 5   | **#533** music-api: Metadaten-Felder (Dauer/Explicit/Artwork/`record_source`)     | — **parallel** |
| 6   | **#538** follow-up: Artist-Link + External-IDs beim Import                        | #529 gemergt   |

**Optionaler Nebenweg** (nicht nötig für die Funktion): #529 (`POST /api/hub/import/playlist`,
gebaut) + #530 (Bearer) + #531 (Upload-Feld) + #532 (kellner-Dateitransport). Nützlich für
Tests, Fixtures und für Nutzer, die keinen Token abgeben wollen.

**Nur wenn die Route offiziell wird** (Developer Program): `p256` +
Developer-Token-Signierung, MusicKit JS auf `/apple-music` als Ersatz für den Token-Push,
Reconnect-UI mit bekannter Laufzeit. Der Worker bleibt dabei **unverändert** — er benutzt
einen Token, egal woher.

**Fertig / in Review:** #529 (Endpoint, PR), #525 (Plan + ADR + Faktenblatt, PR),
#522 (PoC, geschlossen).

## 10. Risiken

| #   | Risiko                                                                                                                                  | Gegenmaßnahme                                                                                                    |
| --- | --------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| R1  | Apple-Push mit `source:"applemusic"` überschreibt `track_metadata.source`, während `bpm`/`key` von Traktor bleiben → falsche Provenienz | Apple-Pushes senden **kein** `source`; Test-Wächter                                                              |
| R2  | `/metadata` antwortet 200, auch wenn der Vault-Ingest scheitert → stiller Totalverlust                                                  | Antwortfeld `vaultUpserted`/`vaultError`                                                                         |
| R3  | `hub`-View nimmt neueste Meta-Zeile ohne Source-Filter → Teil-Push nullt `durationMs`/`explicit`                                        | pro Feld neueste non-null Zeile                                                                                  |
| R4  | Lebensdauer des User-Tokens unbekannt (Quellen widersprechen sich: Tage vs. 6 Monate)                                                   | `reauth`-Status + 401-Zeitstempel, dann messen; `token_expiry = NULL` statt Heuristik                            |
| R5  | ISRC-Auflösung skaliert mit der Bibliotheksgröße (1 Katalog-Call je 300 IDs)                                                            | Miss-Cache mit TTL, 500 Tracks/Pass, Drosselung; bei Bedarf nur Playlist-Tracks auflösen statt ganzer Library    |
| R6  | `.p8` landet im Deploy-Tar (Deploy kopiert `music-hub/` breit)                                                                          | Key außerhalb des Baums (`/home/momo/apple/…`, `chmod 600`), `APPLE_MUSIC_KEY_PATH` absolut                      |
| R7  | 429 ohne `Retry-After` (Apple dokumentiert das nicht)                                                                                   | bounded Retry, dann Pass beenden; Loop-Takt statt Inline-Warten                                                  |
| R8  | Neue Dependency `p256` bricht `--locked` in CI                                                                                          | `Cargo.lock` mitcommitten, `cargo build --locked` lokal vor dem PR                                               |
| R9  | `service='spotify'` ist an ~54 Stellen hartcodiert                                                                                      | v1-Scope bewusst eng: Ingest + Anzeige + Tag-Quelle, **kein** Apple-Push/Now-Playing; Lücken als Issues notieren |
| R10 | Checksummen: 056 nachträglich "nur kurz" korrigiert                                                                                     | 056 einfrieren, Korrekturen als 057                                                                              |

## 11. Offene Entscheidungen

1. **Apple-Bibliothek → `hub_liked_tracks` oder Pseudo-Playlist?** Empfehlung:
   `hub_liked_tracks` (semantisch „gespeicherte Songs", speist `base_likes` der Engine) —
   Modellentscheidung, keine technische Notwendigkeit.
2. **Genre in den Vault?** `sat_track_meta` hat keine `genre`-Spalte. Entweder `ALTER TABLE
… ADD COLUMN genre` + `hash_diff` (einmalig eine neue Zeile je Track) oder Genre bleibt
   Hub-lokal. Empfehlung: weglassen in v1, sonst zwei Wahrheiten.
3. **Alle Library-Songs oder nur Playlist-Tracks auflösen?** Bei großen Bibliotheken ist
   „nur was in einer Playlist hängt" deutlich billiger und deckt den Nutzen (Tag-Quellen).
4. **`filter[isrc]` (25/Batch) für Hub-Enrichment nutzen?** Ja für gezielte Lookups
   (z. B. Track-Page), nein für Bulk — dort `ids` (300/Batch).
