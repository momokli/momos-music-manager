# Progress — Issue #373 `docs: deprecation docs`

**Milestone**: mmm-sunset-1.19.0 · **Base**: `main` · **PR-Ziel**: `main`
**Branch**: `feat/deprecation-docs-373`
**Ziel**: Deprecation docs — „Point users from MMM to Hub + Kellner." Nutzer von MMM
auf den neuen Stack (Hub + Kellner, plus music-api) umleiten.

## Vorbefund (Triage-Gültigkeit)

- Kein offener/merged PR referenziert #373. #371 (freeze + FROZEN.md) und #372
  (data migration) sind gemergt/CLOSED — die Grundlagen existieren (README-Banner,
  `legacy/mmm/FROZEN.md`, Sunset-Hinweis in `docs/legacy/mmm-README.md`).
- **Es fehlt aber eine eigenständige, nutzerorientierte Deprecation-/Umstiegsseite,
  die gebündelt auf Hub + Kellner zeigt.** Deshalb: bauen (kein ALREADY-DONE).

## Scope (docs-only — wichtig wegen freeze guard)

Der `mmm-freeze.yml`-Guard failt jeden PR, der **außer `*.md`** irgendetwas unter
`legacy/mmm/` anfasst. Daher **kein** Touch von `legacy/mmm/site/index.html` o. Ä.

Deliverable:
1. Neue Datei `docs/DEPRECATION.md` — nutzerorientierte Sunset-Seite:
   - „MMM ist sunset" + was das heißt
   - Wohin: **music-hub** (Library/Tags/Playlists/Digging), **kellner** (Backups,
     Traktor-Upload, Downloads auf Mac/PC), **music-api** (Downloads/Metadaten)
   - Umstiegs-Schritte (Daten migriert → siehe #372; `legacy/mmm/FROZEN.md`)
   - Verweis auf `docs/ROADMAP.md`, `docs/ARCHITECTURE.md`
2. `README.md` — „Legacy: MMM — sunset"-Abschnitt verlinkt `docs/DEPRECATION.md`.
3. `docs/legacy/mmm-README.md` — Sunset-Banner um Link auf `docs/DEPRECATION.md` ergänzen.
4. `docs/ROADMAP.md` — optionaler Verweis auf `docs/DEPRECATION.md` bei `mmm (sunset)`.

## Stages

- [x] setup — Branch `feat/deprecation-docs-373` von `origin/main`
- [x] developer — Docs schreiben
- [x] verifier — PASS (Links, freeze-konform, Inhalt, Commit-Titel geprüft)
- [ ] developer — push + PR (`Closes #373`)

## Log

- setup: Branch `feat/deprecation-docs-373` von `origin/main` (Base-Commit 74eb149) angelegt; Working Tree sauber (nur untracked progress-*/plan-*/target/-Dateien).
- verifier: **PASS**. Scope exakt (M README.md, M docs/ROADMAP.md, M docs/legacy/mmm-README.md, ?? docs/DEPRECATION.md). Keine Änderung unter `legacy/mmm/` → freeze-guard-regex (`^legacy/mmm/` minus `*.md`) hat 0 Treffer. Alle 11 relativen Links resolven (`test -e` grün). Hub + Kellner + music-api genannt, read-only-bis-Migration (#372) korrekt. Titel `docs(mmm): …` matcht pr-quality-Typen (`docs`). Hinweis: `(#373)` im Titel ist KEIN Closing-Keyword → `Closes #373` muss in den PR-Body.
- developer: `docs/DEPRECATION.md` neu (Sunset + „Wohin statt MMM"-Tabelle Hub/Kellner/music-api + Umstieg); README-Abschnitt „Legacy: MMM — sunset", `docs/legacy/mmm-README.md`-Banner und `docs/ROADMAP.md` (`mmm (sunset)`) verlinken `DEPRECATION.md`. Nur die 4 erlaubten Dateien berührt, nichts unter `legacy/mmm/`. Kein Commit.
