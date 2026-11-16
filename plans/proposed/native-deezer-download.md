# Plan: native-deezer-download

**Status**: in progress (M1 #509, M2 #510 gemergt; **M3 = deemix ersatzlos entfernen**, offen)
**Branch**: `feat/native-deezer-download`
**Ready for review**: yes
**Depends on**: nothing (Rusteer ist MIT, unsere `deezer.rs`-Metadaten sind schon nativ)
**Migration needed**: nein (nutzt bestehende `store_objects`/`sat_track_download`)

> **Ziel**: music-api lädt Deezer-Audio **selbst** (128/320/FLAC). **deemix wird ersatzlos
> entfernt** — kein Sidecar, kein Fallback, kein Node, kein Docker, kein `/api/getQueue`-Polling.
> yt-dlp bleibt für YT/SoundCloud.

## Ausgangslage (verifiziert 2026-11-16)

| Teil                              | Wer                                                                                    | Fundstelle                           |
| --------------------------------- | -------------------------------------------------------------------------------------- | ------------------------------------ |
| Metadaten / ID-Mapping / ARL-Tier | **music-api nativ** (gw-light `deezer.getUserData`, `lookup_isrc`, `deemix_track_url`) | `music-api/src/deezer.rs:62,155,203` |
| Audio-Bytes (Deezer)              | **externer deemix-Dienst** — nur `/api/loginArl`, `/api/addToQueue`, `/api/getQueue`   | `music-api/src/deemix.rs:65,85,113`  |
| YT/SoundCloud                     | yt-dlp als Subprozess                                                                  | `music-api/src/ytdlp.rs:31`          |
| Orchestrierung                    | deemix-Queue → Poll, yt-dlp-Fallback                                                   | `worker.rs:137,284,436,391`          |

Der Sidecar ist heute **der** Betriebs-Blocker: auf dem neuen Host ist Docker `masked`, kein
Listener auf `DEEMIX_URL` (`:6599` — im Compose existiert der Port nicht, nur 6595/96/97/6600),
Store unverändert seit 16.10.

## Prior Art (geprüft 2026-11-16)

| Projekt                                                   | Was                                                                                                                              | Lizenz      | Reife                                                   | Fazit                                 |
| --------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------- | ----------- | ------------------------------------------------------- | ------------------------------------- |
| **[Rusteer](https://github.com/xScherpschutter/Rusteer)** | Lib + CLI: ARL, Track/Album/Playlist, **MP3 128/320 + FLAC**, Tagging, Cover, eigener `crypto/` (blowfish, aes, ctr, md-5, sha1) | **MIT**     | `0.1.0`, Push 2026-03, 2 Stars, **nicht auf crates.io** | **Basis** (vendoren)                  |
| `deezer_downloader` (crates.io)                           | download + decrypt                                                                                                               | NOASSERTION | 2021, 2.1k Downloads                                    | nur Referenz                          |
| `deezer-rs`                                               | nur Metadaten                                                                                                                    | —           | —                                                       | nein                                  |
| `pleezer`                                                 | headless **Player** (Deezer Connect)                                                                                             | —           | reif                                                    | nein (kein Download)                  |
| deemix / deemix-remastered                                | JS bzw. **Electron-GUI** (Fuses gehärtet)                                                                                        | GPL-3.0     | —                                                       | nur Protokoll-Referenz, **kein Code** |

## Test (verifiziert 2026-11-16, echter ARL, Track 3135556)

| Qualität  | Format | Bitrate    | Größe (226 s) |
| --------- | ------ | ---------- | ------------- |
| `mp3-128` | MP3    | 131,7 kbps | 3,73 MB       |
| `mp3-320` | MP3    | 323,8 kbps | 9,16 MB       |
| `flac`    | FLAC   | 896,8 kbps | 25,4 MB       |

- **Alle drei Qualitäten funktionieren** (Rusteer-Protokoll ist aktuell).
- **Kein ISRC-Tag** in der Datei — für uns zwingend nachzurüsten (Store/Vault ist ISRC-keyed).
- Der ARL meldet `OFFER_NAME: Deezer Student` mit `web_sound_quality.lossless = true`.
- Nebenbefund: der **deployte** API-Build (`api-v0.4.0-20-g6136db6`) hat **kein** `/account`
  (HTTP 404) — der ARL-Tier-Gate aus #473 ist also noch nicht live.

## Warum yt-dlp extern bleibt (geprüft 2026-11-16)

| Ansatz                                           | Stand                                                                                                                      | Fazit                                                 |
| ------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------- |
| `rusty_ytdl` (pure Rust, YouTube-Suche+Download) | Crate `0.7.4` von **2024-08**, Repo zuletzt **2026-01** (185 ★, 10 offene Issues); kann region-locked/private Videos nicht | möglich, aber die Drift läge bei uns                  |
| `rustube` (pure Rust)                            | Repo **2024-07**, 274 ★, 25 offene Issues                                                                                  | ruht                                                  |
| `yt-dlp` (Rust-Wrapper um das CLI)               | Crate `2.10.0`, Repo **2026-11**                                                                                           | aktiv gepflegt — und genau das Muster, das wir nutzen |

YouTube ändert Signaturen / `po_token` / SABR laufend, SoundCloud ebenfalls. Ein eigener Extractor
wäre ein Dauerprojekt ohne Produktnutzen. Entscheidender Unterschied zum Deezer-Fall: yt-dlp ist
**kein Dienst** (kein Port, kein Daemon, kein Node), sondern ein von uns gesteuerter Prozess —
bereits hinter `ytdlp.rs:31` gekapselt. Ein späterer Wechsel auf eine Rust-Library wäre ein
Modul-Tausch, keine Architekturänderung.

## Entscheidung

**Rusteer vendoren** (MIT-Notice behalten), nicht als crates.io-Dependency (nicht publiziert,
2 Stars, `0.1.0` → wir wären die Maintainer). Ablage: `music-api/src/deezer_dl/` mit eigenem
`crypto/`-Modul; der Aufruf bleibt hinter dem bestehenden Worker-Interface.

Unsere Seite bleibt unverändert: Queue/Prioritäten, Store/Vault, ISRC-Mapping, Tagging-Normalisierung,
yt-dlp-Fallback. **`deemix.rs` wird gelöscht** — es gibt genau einen Download-Pfad (native), keinen
Schalter, keinen Fallback (Entscheidung 2026-11-16).

## Was „deemix raus" konkret mitnimmt (verifiziert 2026-11-16)

`grep -rc deemix music-api/src music-api/tests` → **187 Treffer** (`worker.rs` 84, `config.rs` 16, `tests/api.rs` 25, `api.rs` 6; `deemix.rs` selbst 170 Zeilen). Zwei Stellen sind nicht bloß Löschung, sondern **Ersatz**:

| Abhängigkeit                                                                                   | Warum sie nicht einfach mitgelöscht werden darf                                                                                                                 | Ersatz                                                                                                                                                                                                                                                                                  |
| ---------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `GET /queue` (`api.rs:427`)                                                                    | enthält einen Sidecar-Zweig, der die **live deemix-Queue** liefert; die Hub-Ops-UI liest den Endpoint über `music-hub/src/music_ops.rs:19` (`/music-api/queue`) | der native Zweig existiert seit #517 (antwortet aus der DB) — der Sidecar-Zweig fliegt raus, mit `backend`/`sidecarConfigured`                                                                                                                                                          |
| Artist-Order-Phase (`advance_crawl_leads` → `submit_artist_order` → `ingest_artist_downloads`) | existiert **ausschließlich** über `deemix::add_to_queue(artist_url)` plus Dateisuche in `deemix_download_dir`                                                   | entfällt mit dem Sidecar: `crawl:deezer`-Leads erzeugt im Repo nur `music-api/tests/api.rs:2113`, hub und kellner **nicht**. Wenn `plans/proposed/hub-crawl-everything.md` Diskografien will, kommt das als eigener Schritt (Deezer artist → albums → tracks → ISRCs in die Work-Queue) |

`DEEMIX_ARL` ist **keine** deemix-Abhängigkeit: die native Session authentifiziert sich damit (`worker.rs:150,162`, `lib.rs:103`). Die Variable wird nur umbenannt, das Secret bleibt dasselbe.

## Meilensteine

| M                 | Inhalt                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           | Akzeptanz                                                                                                                 |
| ----------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| **M1**            | `deezer_dl`-Modul (vendored Rusteer): ARL-Session, `download_track(id, quality)` → Bytes + Datei; Fehlerbilder (invalid ARL, kein Lossless-Tier, 403/404)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        | Unit-Tests für URL-/Key-Bau + Fehler-Mapping; Live-Matrix 128/320/flac grün (manuell)                                     |
| **M2**            | Integration in den Worker: statt `deemix::add_to_queue` direkt laden → `store_objects`/`sat_track_download`; **ISRC-Tag** setzen (`tags.rs`); Status/Timings im Run-Log                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          | Bestehende Worker-Tests grün; ein Track landet mit korrektem ISRC im Store                                                |
| **M3**            | **deemix ersatzlos entfernen**: `deemix.rs` löschen; Sidecar-Queue-Polling und die Artist-Order-Phase (die nur über `deemix::add_to_queue(artist_url)` existiert) raus; **#517 zurückbauen** (das hat die Optionalität gerade eingebaut: `DOWNLOAD_BACKEND`-Default `auto`, `sidecarConfigured`/`backend` in `/health` + `/worker`, Hub-Badge „Quelle", `DEEMIX_URL`-Default); `GET /queue` auf den nativen Zweig reduzieren; `DOWNLOAD_BACKEND` ersatzlos streichen; Env entkoppeln (`DEEMIX_ARL` → `DEEZER_ARL`, `DEEMIX_BITRATE` → Qualitätswahl, `DEEMIX_URL`/`DEEMIX_DOWNLOAD_DIR` streichen); `deploy/deemix-api.compose.yml` + Deemix-Schritte in `setup.sh` löschen; README/`.env.example`/`AGENT.md`/`docs/DECISIONS.md` auf den einen Pfad umschreiben | `grep -ri deemix music-api/src` leer; Downloads laufen ohne jede deemix-Config; `/music-api/queue` zeigt die native Queue |
| **M4** (optional) | **Qobuz** als Lossless-Quelle (unverschlüsselte FLACs, nur signierte Requests) — echtes Hi-Res ohne Decrypt                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      | Spike-Ergebnis + Entscheidung; nur mit vorhandenem Abo                                                                    |

## Risiken

- **Protokoll-Drift**: Deezer-Internals ändern sich; Rusteer ist klein → Fork/Vendor heißt, wir
  patchen selbst. Es gibt **keinen** Sidecar mehr als Netz — verbleibender Fallback ist yt-dlp
  (anderer Provider), sonst schlägt der Download sichtbar fehl.
- **Lizenz-Hygiene**: MIT (Rusteer) einhalten; aus deemix(-remastered, GPL-3.0) **keinen** Code
  übernehmen, nur Verhalten ableiten.
- **ARL-Handling**: Tokens laufen ab (der ARL in der lokalen Shell war abgelaufen, der auf dem
  Server nicht) → klare Fehlermeldung + Hinweis statt stillem Fallback. Tier-Gate aus #473 nutzen.
- **Kein ISRC im Tag**: muss M2 setzen, sonst findet der Vault die Datei nicht.

## Akzeptanz (Plan)

- [ ] Downloads funktionieren **ohne** jede deemix-Installation (kein Container, kein Port, kein Node)
- [ ] Qualitätsmatrix 128/320/flac gegen echten ARL belegt (manuell, dokumentiert)
- [ ] ISRC-Tag in MP3 und FLAC vorhanden; Vault findet die Datei über ISRC
- [ ] `grep -ri deemix music-api/src` ist leer (Code, Config, Deploy, Doku) — es gibt keinen Pfad-Schalter mehr
- [ ] Hub-Ops-UI (`/music-api/queue`) zeigt die native Work-Queue statt der deemix-Queue
- [ ] `DEEMIX_ARL` ist zu `DEEZER_ARL` geworden; der alte Name wird mit Warnung weiter akzeptiert (kein Bruch beim Prod-Deploy)
- [ ] Keine Secrets in Logs (ARL nie geloggt, auch nicht in Fehlermeldungen)
- [ ] MIT-/Fremd-Lizenzhinweise im Repo vorhanden
