# Plan: runner-release-flow

**Status**: in-progress
**Branch**: — (Fortsetzung in Folge-Sessions; Arbeit findet in `openclaw-deploy` + `main` statt)
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: no
**Last Updated**: 2026-10-17

## Warum diese Datei existiert

Zwei Dinge sind gleichzeitig aufgeschlagen und die Session wurde unterbrochen:

1. Die Runner haben sich in einen **Release-Deadlock + eine Versions-Regression** gelaufen (gefixt, siehe unten).
2. Ein **Infra-Umzug** nimmt `music-hub` / `music-api` vom Netz — alles, was Live-Verifikation oder Deploy braucht, ist blockiert.

Diese Datei hält den verifizierten Stand und die offenen Schritte fest, damit eine spätere
Session ohne Kontextverlust weitermacht.

## Stand (verifiziert 2026-10-17)

| Was | Wert |
| --- | --- |
| `main` | `ff3a3c3` (nach #494) |
| `music-hub` Version | **0.30.0**; Tags `hub-v0.29.0` (`030ed3d`), `hub-v0.30.0` (`71dac80`) |
| Offene PRs | **keine** |
| Runner-Host | `projectmellon.de` (= `planet`): `openclaw-gateway` + `openclaw-node` **active**, Ticks alle 5 min |
| Fokus-Milestone | **keiner** (`runner-focus-milestone.sh` exit 4 — kein freigegebener Milestone mit Arbeit) |
| Log-Flut | **behoben**: 0 Probe-Zeilen/min (vorher ~58) |
| Prod | `music-hub` / `music-api` **down** (Umzug) |

## Heute erledigt

### 1. Log-Flut „Worker disk-space probe failed" (56k Zeilen/Tag)

- Ursache: **41 verwaiste `worker_session_placements`** (`session_key LIKE '%:dashboard:%'`,
  `state='active'`) in `/srv/openclaw/state/openclaw.sqlite`, die auf **nicht mehr existierende**
  Worker-Workspaces zeigten (`/home/momo/.openclaw/node-host/gateway-d6e8d748…/workspaces/…`).
  Der Disk-Probe fasst jede Minute jedes `active`-Placement an → 1 ERROR-Zeile/Session/min.
- Fix: Rows gelöscht (FK-Cascades vorher geprüft, Kind-Tabellen 0 Zeilen);
  Backup `/srv/openclaw/backups/openclaw.sqlite.pre-placement-purge{,-wal,-shm}`.
- **Zwei Irrwege (nicht wiederholen):**
  - Gateway-Restart räumt die Rows **nicht** (sie sind persistiert und kommen sofort wieder).
  - `LogFilterPatterns=` filtert **das Journal nicht** (systemd schließt damit nur
    syslog/kmsg/console/wall aus). Das Drop-in wurde wieder entfernt.
- Verifikation:
  ```bash
  journalctl --user -u openclaw-gateway --since "60 seconds ago" | grep -c "disk-space probe failed"   # → 0
  ```

### 2. Lokale Arbeit in `main`

| PR | Issue | Anmerkung |
| --- | --- | --- |
| #471 feat(hub): Spotify-Queue/Transport/Kontext | #475 | Mocks only, kein Live-Lauf |
| #472 fix(kellner): `CARGO_TARGET_DIR` in install/smoke | #476 | |
| #474 ci(kellner): per-OS-Release-Artefakte | #477 | Rebase-Konflikt in `plans/README.md` gelöst; alle 7 Matrix-Legs grün |
| #473 fix(music-api): Deezer-ARL-Tier | #233 | **vom Gate autonom** gemergt (APPROVE + CLEAN) |
| #494 chore(repo): Progress-Artefakte raus aus git | #493 | |
| #490 chore(release): 0.29.0 (Versions-Fix) | — | `chore(release)`-Titel = Issue-Referenz-Pflicht entfällt |

### 3. Engine-Fixes in `openclaw-deploy` (committed + auf `planet` deployt)

| Commit | Fix |
| --- | --- |
| `2b51deb` | Gate-Tick brach bei „kein Fokus-Milestone" ab, **bevor** er Release-PRs ansah → Fokus ist jetzt optional |
| `5402147` | Release-Decision sagte „NIE mergen" → jetzt „Mergen + Taggen (auto), `release:human-merge` ist das Opt-out" |
| `3148dba` | Release-Stack nach dem Basis-Merge **retargeten** (`baseRefName == main`), sonst stiller No-Op-Release |
| `8e47e3c` | Release-Version = **aktuelle Cargo.toml + 1 Minor**, nicht der Milestone-Titel |
| `bd3cf3a` | `close-milestones` schloss freigegebene Milestones **vor** ihrem Release-PR → überspringt sie jetzt |
| `d182deb` | Release-PR nur auf **offene Release-PRs** stapeln (nicht auf beliebige PRs); Decision-Basis ist verbindlich |

Zusätzlich: `config/agents/orchestrator.md` — Progress-Datei ist Scratch und wird nie committet
(deployt via `sudo bash scripts/sync-agent-personas.sh momo`).

**Deploy-Weg:** lokales `~/dev/openclaw-deploy` → `scp` nach `/home/momo/openclaw-deploy/…`
(kein Git-Clone auf dem Host) → `bash scripts/install-runner-engine.sh` →
`cd /home/momo/openclaw-deploy && bash scripts/automations-apply.sh`.

### 4. Versions-Regression repariert

- Der Auto-Release nahm die Version aus dem Milestone-Titel; Retro-Milestones
  (`hub-tag-clusters-0.18.0` usw.) liegen **unter** der ausgelieferten 0.28 →
  `music-hub/Cargo.toml` fiel von **0.28.0 auf 0.18.0**, Tag `hub-v0.18.0`.
- Reparatur: #490 auf **0.29.0** (Cargo.toml/lock + CHANGELOG-Überschrift), Tag `hub-v0.18.0`
  gelöscht, `hub-v0.29.0` gesetzt; die falsch versionierten Release-PRs **#482/#484/#487/#489
  geschlossen** (#491 wurde von GitHub geschlossen, als die Basis-Branch wegfiel → durch #492 ersetzt).
- Danach lief es autonom: **#492 → 0.30.0**, Tag `hub-v0.30.0`, Milestone #25 geschlossen.

### 5. Repo-Hygiene

- 15 lokale Branches (alle mit gemergtem PR) gelöscht, verwaister Worktree
  `worktrees/…/silky-lantern` entfernt, `main` auf origin gezogen.
- `progress-*.md`-Artefakte: `.gitignore` (`/progress-*.md`) + 12 Dateien aus `main` entfernt (#494).

## Offene Schritte (in dieser Reihenfolge)

1. **#480 fixen** — echter Bug aus 0.29/0.30, komplett offline machbar.
   `ClusterQuery` deserialisiert `type_`, Template (`templates/tag_clusters.html:19`) und
   Rebuild-Redirect (`src/clusters.rs:380`) senden `type` → typisierte Cluster sind über die UI
   **nicht erreichbar**. Failing test first: `GET /tag-clusters?type=MOOD&scope=mine` muss rendern.
2. **#26/#29/#41 wieder öffnen** (optional, gibt dem Runner Arbeit während Prod down ist):
   sie wurden vom alten `close-milestones`-Bug **vor** ihrem Release geschlossen → der Runner
   cutet dann autonom 0.31.0 / 0.32.0 / 0.33.0.
3. **Engine-Nacharbeit:** der Release-PR soll den Inhalt von `[Unreleased]` in den neuen
   Versionsabschnitt **übernehmen** (sonst driftet `music-hub/CHANGELOG.md`; aktuell stehen die
   Features von `hub-playlist-sync-0.20.0` / `hub-smartplay-0.23.0` / `hub-tag-hygiene-0.30.0`
   noch unter `[Unreleased]`, obwohl sie im 0.30.0-Artefakt stecken).
4. **Milestone-/Issue-Buchhaltung** (Entscheidungen nötig, siehe unten).
5. **Nach dem Umzug**: `main` (= 0.30.0) auf hub/api deployen → Smoke (Track-Download-Toast,
   `/now-playing` mit BPM/Key, `/tag-clusters?type=…`, ARL-Tier) → dann die geparkten
   Live-Verifikationen: Spotify-Queue (#471/#475, bisher nur Mocks), `music-api` ARL (#233),
   kellner-Pairing, Playlist-Poller.

## Befunde zur Milestone-/Issue-Buchhaltung

**Geschlossene Milestones mit noch offenen Issues** (die Issues sind damit für den Runner
unerreichbar — Fokus arbeitet nur auf offenen Milestones):

| Milestone | offene Issues |
| --- | --- |
| #1 `1.14.0` (MMM) | #50 [epic] Rediscovery — Library signals |
| #24 `hub-tag-clusters-0.18.0` | #261 [epic] Tag-Cluster, #263 [research] Embedding-Provider |
| #25 `hub-tag-types-0.19.0` | #273 [epic] Tag-Typen |
| #31 `hub-recs-0.25.0` | #335 [research] Embedding provider |

- **Duplikat:** #263 („Embedding-Provider für Tag-Namen (lokal vs. hosted)") ≡ #335
  („Embedding provider for tag names (local vs hosted)") — eines schließen.
- **Legacy-Milestones offen:** #3 `1.16.0` (6), #4 `1.17.0` (8), #5 `1.18.0` (4) = 18 Issues
  für die **sunset** MMM-App (Rediscovery/Traktor-Farm). `docs/ROADMAP.md` nennt kein Rediscovery
  → als Buchhaltung schließen oder bewusst als Backlog behalten (Entscheidung).
- **Ohne Milestone (Backlog, 24 Issues):** u. a. **#480** (Bug), #49 (Spotify-Retry-Sturm),
  #47 (Rolling-Kanal), #27 (UI Overhaul), #33 (uralter Blocker), #40–#42 (Backpack),
  #394–#396 (Source-Provider), #279, #301, #383, #288, Epics #203/#292/#54, Roadmap #8–#12.
- **Nicht freigeben:** #39 `infra-deploy-1.0.0` — alle drei Issues sind `question`-gated, ein
  Fokus darauf **parkt die Queue** (schon einmal passiert).
- Fokus-Kandidaten mit Arbeit: #12 `hub-data-0.7.0` (3), #14 `hub-audio-0.9.0` (5),
  #19 `hub-tasks-0.14.0` (1) — alle **nicht** freigegeben.

## Akzeptanz

- [ ] Keine offenen PRs; `main` grün
- [ ] #480 geschlossen, Integrationstest beweist den Fix
- [ ] Keine geschlossenen Milestones mit offenen Nicht-Epic-Issues
- [ ] `[Unreleased]` in `music-hub/CHANGELOG.md` ist leer bzw. nur echte Nach-Release-Einträge
- [ ] Runner hat wieder einen freigegebenen Fokus (oder bewusst keinen)
- [ ] Nach dem Umzug: hub + api deployt, Smoke grün, die geparkten Live-Verifikationen erledigt

## Referenzen

- Runner-Betrieb: `~/.agents/skills/openclaw-runners/SKILL.md`
- Engine + Prompts: `~/dev/openclaw-deploy` (`scripts/runner-*.sh`, `config/automations/`, `config/agents/`)
- Deploy auf dem Host: `/home/momo/openclaw-deploy/…`, Engine in `~/.local/bin/runner-*.sh`
- Logs: `/srv/openclaw/logs/mmm-tick.log`
- State-DB-Backup: `/srv/openclaw/backups/openclaw.sqlite.pre-placement-purge*`
