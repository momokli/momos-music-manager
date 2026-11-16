# Plan: native-deezer-download

**Status**: proposed
**Branch**: `feat/native-deezer-download`
**Ready for review**: yes
**Depends on**: nothing (Rusteer ist MIT, unsere `deezer.rs`-Metadaten sind schon nativ)
**Migration needed**: nein (nutzt bestehende `store_objects`/`sat_track_download`)

> **Ziel**: music-api lädt Deezer-Audio **selbst** (128/320/FLAC) — ohne deemix-Sidecar, ohne
> Node, ohne Docker, ohne `/api/getQueue`-Polling. Der Sidecar bleibt als Fallback, yt-dlp bleibt
> für YT/SoundCloud.

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
yt-dlp-Fallback. `deemix.rs` bleibt als **optionaler Fallback**, wenn `DEEMIX_URL` gesetzt ist.

## Meilensteine

| M                 | Inhalt                                                                                                                                                                  | Akzeptanz                                                                             |
| ----------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| **M1**            | `deezer_dl`-Modul (vendored Rusteer): ARL-Session, `download_track(id, quality)` → Bytes + Datei; Fehlerbilder (invalid ARL, kein Lossless-Tier, 403/404)               | Unit-Tests für URL-/Key-Bau + Fehler-Mapping; Live-Matrix 128/320/flac grün (manuell) |
| **M2**            | Integration in den Worker: statt `deemix::add_to_queue` direkt laden → `store_objects`/`sat_track_download`; **ISRC-Tag** setzen (`tags.rs`); Status/Timings im Run-Log | Bestehende Worker-Tests grün; ein Track landet mit korrektem ISRC im Store            |
| **M3**            | Sidecar entkoppeln: `DEEMIX_URL` optional, Queue-Polling nur im Sidecar-Modus; Health zeigt „native" vs. „sidecar"                                                      | Ohne Docker laufen Downloads; mit `DEEMIX_URL` unverändert als Fallback               |
| **M4** (optional) | **Qobuz** als Lossless-Quelle (unverschlüsselte FLACs, nur signierte Requests) — echtes Hi-Res ohne Decrypt                                                             | Spike-Ergebnis + Entscheidung; nur mit vorhandenem Abo                                |

## Risiken

- **Protokoll-Drift**: Deezer-Internals ändern sich; Rusteer ist klein → Fork/Vendor heißt, wir
  patchen selbst. Abgefedert durch yt-dlp-Fallback und den optionalen Sidecar.
- **Lizenz-Hygiene**: MIT (Rusteer) einhalten; aus deemix(-remastered, GPL-3.0) **keinen** Code
  übernehmen, nur Verhalten ableiten.
- **ARL-Handling**: Tokens laufen ab (der ARL in der lokalen Shell war abgelaufen, der auf dem
  Server nicht) → klare Fehlermeldung + Hinweis statt stillem Fallback. Tier-Gate aus #473 nutzen.
- **Kein ISRC im Tag**: muss M2 setzen, sonst findet der Vault die Datei nicht.

## Akzeptanz (Plan)

- [ ] Downloads funktionieren **ohne** Docker/Node (deemix-Sidecar gestoppt, Store bekommt Dateien)
- [ ] Qualitätsmatrix 128/320/flac gegen echten ARL belegt (manuell, dokumentiert)
- [ ] ISRC-Tag in MP3 und FLAC vorhanden; Vault findet die Datei über ISRC
- [ ] Mit gesetzter `DEEMIX_URL` verhält sich alles wie heute (Fallback-Pfad getestet)
- [ ] Keine Secrets in Logs (ARL nie geloggt, auch nicht in Fehlermeldungen)
- [ ] MIT-/Fremd-Lizenzhinweise im Repo vorhanden
