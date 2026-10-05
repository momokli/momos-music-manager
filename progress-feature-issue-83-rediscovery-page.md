# Progress — Issue #83: Frontend #rediscovery page

**Repo:** momokli/momos-music-manager
**Branch:** feature/issue-83-rediscovery-page (base: origin/main @ 1bcc6af)
**PR target:** main · Milestone 1.15.0 · Closes #83
**Bot identity:** clanker-git / clanker-gh (momo-clanker[bot])

## Dispatch validity check

- No merged PR references #83; no `frontend/pages/rediscovery.js` exists on origin/main.
- Backend prerequisites ARE merged and on main:
  - #114 `GET /api/rediscovery/stats` (merge 1bcc6af)
  - #113 `GET /api/rediscovery/candidates` (merge 6da4728)
  - #112 seed scenario `rediscovery` (in `src/db/testing.rs`, registered in
    `src/api/infrastructure.rs:514` as `"rediscovery" => seed_rediscovery_scenario`)
- => Issue is NOT done. Normal pipeline.

## Backend API contract (already on main, do NOT change)

`src/api/rediscovery.rs`:
- `GET /api/rediscovery/candidates` — camelCase query params:
  `touchedBeforeDays` (def 365), `maxPlaylists` (opt), `likedOnly` (bool),
  `excludeBackpack` (def true), `excludePushedSinceDays` (def 180, 0/null = off),
  `playCountMax` (opt), `notPlayedSinceDays` (opt), `requireBpm`, `requireKey`,
  `bpmMin`, `bpmMax` (f64), `keys` (csv), `genres` (csv), `limit` (def 50,
  clamped 1..MAX_LIMIT), `offset` (def 0), `seed` (opt u64), `sort` (def
  "oldest-touched").
  Valid sorts: `oldest-touched`, `forgotten`, `bpm`, `artist`, `random`.
  Response `data`: `{ candidates: CandidateRow[], total, limit, offset }`.
  `CandidateRow` camelCase: `trackId, spotifyId, title, artist, playlistCount,
  likedAt, lastAddedAt, touchedYearsAgo, bpm, musicalKey, genre, inBackpack,
  owned, reasons: string[]`.
- `GET /api/rediscovery/stats` — SAME facet params (pagination ignored).
  Response `data`: `{ matching, withBpmAndKey, needsAnalysis, notOwned,
  pushedRecently, byBpmBucket, byLiked: {liked, unliked} }`.
- Note: issue text says stats has `matching/needsAnalysis/notOwned` — those are
  the required chips; extra fields may be shown but not required.

`reasons` values produced by backend (must render as chips, verbatim):
`last-touched-<YYYY-MM-DD>`, `only-in-1-playlist` / `in-<N>-playlists`,
`pushed-<YYYY-MM-DD>` / `never-pushed`, `liked`, `low-play-count`,
`not-played-since-<YYYY-MM-DD>`, `bpm-in-range`, `key-match:<key>`,
`genre-match:<genre>`, `not-in-backpack`.

## Registrations required (BOTH, else hash-router 404s to dashboard)

- `frontend/app.js` `PAGE_MAP`: add `rediscovery: "rediscovery"`.
- `frontend/shared/nav.js` `TOOLS_ITEMS`: add
  `{ id: "rediscovery", label: "Rediscovery", icon: ... }` (e.g. `fa-rotate-left`).

## Frontend conventions to reuse

- `init(container, signal, hashParams)` export; module-level `state`; `render()`;
  `wireEvents()` (see `frontend/pages/daily.js`).
- Helpers: `fetchJSON` (`shared/api.js`), `escapeHtml`/`showToast`/`renderBadge`/
  `renderTable`/`td`/`renderEmpty`/`renderErrorBlock` (`shared/components.js`),
  `formatBPM`/`formatNumber`/`formatDate` (`shared/format.js`).
- Selectors MUST be `#id` / `[data-*]`, never nth-child.
- Paging must be server-side (send `limit`/`offset`; never client-filter after
  pagination) — project rule.
- Live-preview: facet change (debounced) re-fetches candidates+stats with abort
  via signal; no reload.
- Generate button + order-missing action: placeholder / no action this issue.

## Definition of Done (from issue, must all hold)

- [ ] `#rediscovery` loads via hash router, appears in Tools menu
- [ ] Facet change updates preview (server-side, no reload)
- [ ] Reason-chips == API `reasons`
- [ ] Counts bar shows stats values
- [ ] No `pageerror` on load/filter
- [ ] Pagination works, server-side correct
- [ ] No client-side filtering after pagination
- [ ] Playwright `frontend/tests/rediscovery.spec.js` uses seed scenario
      `{ scenario: "rediscovery" }`; `cargo build` + tests pass; `npx playwright test`

## PLAN

### Betroffene Dateien
- `frontend/pages/rediscovery.js` (neu) — SPA-Seite, `export async function init(container, signal, hashParams)`.
- `frontend/app.js` — nur `PAGE_MAP` erweitern: `rediscovery: "rediscovery",` (Router importiert `./pages/${pageId}.js` dynamisch, keine weitere Map nötig).
- `frontend/shared/nav.js` — `TOOLS_ITEMS` ergänzen: `{ id: "rediscovery", label: "Rediscovery", icon: "fa-rotate-left" }`.
- `frontend/tests/rediscovery.spec.js` (neu).

### State (modul-level)
`state = { facets: {...}, page: 0, limit: 50, total: 0, loading: false, rows: [], stats: null, sort: "oldest-touched", seed: "" }`. Zusätzlich `_container`, `_pageSignal` (aus init) und `_inflight` (eigener AbortController für Requests).

### Formularfelder (DOM-IDs, camelCase-Query-Params)
Alle optionalen Felder weglassen statt leer senden.
- `#rd-touched-before-days` (number, def 365) → `touchedBeforeDays`
- `#rd-max-playlists` (number, leer) → `maxPlaylists`
- `#rd-liked-only` (checkbox) → `likedOnly=true`
- `#rd-exclude-backpack` (checkbox, default checked) → `excludeBackpack`
- `#rd-exclude-pushed-since-days` (number, def 180; `0` = aus/omit) → `excludePushedSinceDays`
- `#rd-play-count-max` (number, leer) → `playCountMax`
- `#rd-not-played-since-days` (number, leer) → `notPlayedSinceDays`
- `#rd-require-bpm` / `#rd-require-key` (checkbox) → `requireBpm` / `requireKey`
- `#rd-bpm-min` / `#rd-bpm-max` (number step 0.1) → `bpmMin` / `bpmMax`
- `#rd-keys` / `#rd-genres` (text, csv) → `keys` / `genres`
- `#rd-limit` (number, def 50) → `limit`
- `#rd-sort` (select: oldest-touched|forgotten|bpm|artist|random) → `sort`
- `#rd-seed` (number, optional) → `seed`
- `#rd-generate` (button) — Issue-Placeholder: `disabled` bzw. `showToast("Not implemented yet","info")`.

### Kennzahlen-Leiste
`#rd-stats` mit `<span data-stat="matching|withBpmAndKey|needsAnalysis|notOwned|pushedRecently">`. Werte aus `stats` per `formatNumber`. `byLiked`/`byBpmBucket` optional (nicht Pflicht).

### Preview-Tabelle (`#rd-preview`)
Spalten: Title, Artist, Playlists (`playlistCount`), Last added (`formatDate(lastAddedAt)`), Touched (`touchedYearsAgo`), BPM (`formatBPM`), Key, Genre, Liked (`renderBadge`), Backpack (`inBackpack` Badge), Reasons (Chips).
- Body-Zeilen: `<tr data-track-id="${trackId}">`; leere Werte via `placeholderUnset`.
- Leerzustand: `renderEmpty(...)` innerhalb `#rd-preview`; Fehler: `renderErrorBlock`.

### Chip-Rendering
`reasons` verbatim als `<span class="reason-chip" data-reason="${escapeHtml(r)}">${escapeHtml(r)}</span>`; Klassifizierung nur über Präfix (`last-touched-*`, `in-*/-playlist(s)`, `pushed-*`/`never-pushed`, `liked`, `low-play-count`, `not-played-since-*`, `bpm-in-range`, `key-match:*`, `genre-match:*`, `not-in-backpack`) per `data-reason`, kein Umtexten.

### Pagination (server-seitig, Pflicht)
- `limit` aus `#rd-limit`, `offset = state.page * limit`; Query immer mitsenden.
- Prev/Next via `Pagination` aus `components.js` (bindings prev/next/info/total/showing → `#rd-prev`,`#rd-next`,`#rd-page-info`,`#rd-total`,`#rd-showing`).
- Bei jeder Facettenänderung: `state.page = 0`; Page-Buttons nutzen `state.total` aus Response.
- Kein client-seitiges Filtern nach Pagination.

### Debounce / Abort
- Ein Delegated `input`/`change`-Listener auf dem Form-Container; `clearTimeout` + 300 ms Debounce → `loadPreview()`.
- `loadPreview()` bricht `_inflight` ab, erstellt neuen `AbortController` und fetcht parallel `candidates` + `stats` mit `{ signal }`; bei `state.page=0` Stats mitnehmen, bei Paging nur candidates.
- `init`-`signal` abonnieren (`signal.addEventListener("abort", …)`) und `_inflight.abort()`; `AbortError` still schlucken. Kein Reload beim Filtern.

### Playwright-Tests (`frontend/tests/rediscovery.spec.js`)
`beforeEach`: `POST /api/testing/seed { scenario: "rediscovery" }`.
1. **loads + nav**: `goto /#rediscovery`; `waitForSelector(".rediscovery-page")`; `[data-page="rediscovery"]` im Tools-Menü sichtbar; `pageerror` leer.
2. **stats bar**: `#rd-stats [data-stat="matching"]` sichtbar und numerisch; `needsAnalysis`/`notOwned` gerendert.
3. **rows + chips**: `#rd-preview tbody tr` > 0; mind. ein `.reason-chip`; Chip-Text exakt `liked` (Seed: alle 9 Tracks in Playlist 5 geliked).
4. **Filter server-seitig**: `waitForResponse(/\/api\/rediscovery\/candidates/)`-Listener vor Interaktion; `#rd-liked-only`/`#rd-bpm-min` setzen; neue Response-URL enthält geänderten Param; Marker `window.__rdNavMarker` (in init gesetzt) bleibt ⇒ kein Reload.
5. **Pagination**: `#rd-limit` auf 5; erste Response `limit=5`; `#rd-next` klicken; Request-URL `offset=5`; `#rd-page-info` = "Page 2 of …"; 5 Zeilen; keine Client-Filterung (Zeilenzahl == `limit`).
6. **kein pageerror** nach Filter+Paging (Sammel-Assertion).

## Stage log

- [ ] planner
- [ ] setup
- [x] developer
- [ ] verifier
- [ ] tester
- [ ] developer (PR)
- [ ] reviewer

## DEVELOPER LOG

**Datum:** 2026-10-05 · Agent: feature-dev-developer (subagent dev-83-rediscovery)

### Umgesetzt (keine Backend-Änderungen)

- `frontend/pages/rediscovery.js` (neu, ~18 KB): ES-Modul mit
  `export async function init(container, signal, hashParams)`.
  - Facetten-Formular mit allen Plan-Feldern/IDs (`#rd-touched-before-days`,
    `#rd-max-playlists`, `#rd-liked-only`, `#rd-exclude-backpack`,
    `#rd-exclude-pushed-since-days`, `#rd-play-count-max`,
    `#rd-not-played-since-days`, `#rd-require-bpm`, `#rd-require-key`,
    `#rd-bpm-min`, `#rd-bpm-max`, `#rd-keys`, `#rd-genres`, `#rd-limit`,
    `#rd-sort`, `#rd-seed`, `#rd-generate`).
  - Kennzahlen-Leiste `#rd-stats` mit `data-stat`-Spans: `matching`,
    `withBpmAndKey`, `needsAnalysis`, `notOwned`, `pushedRecently`
    (Werte via `formatNumber`).
  - Live-Preview `#rd-preview` via `renderTable`; Zeilen `data-track-id`,
    Reason-Chips `<span class="reason-chip" data-reason="...">` verbatim aus
    `reasons`; Spalten Title/Artist/Playlists/Last added/Touched/BPM/Key/
    Genre/Liked/Backpack/Reasons; Leerzustand `renderEmpty`, Fehler
    `renderErrorBlock`.
  - Server-seitige Pagination via `Pagination` (bindings
    `#rd-prev`/`#rd-next`/`#rd-page-info`/`#rd-total`/`#rd-showing`);
    `limit`/`offset` immer mitgesendet, kein Client-Filter nach Pagination.
  - Debounce 300 ms auf `input`/`change` des Formulars + eigener
    AbortController (`_inflight`), zusätzlich an den `init`-Abort gehängt;
    `AbortError` wird still geschluckt; kein Reload beim Filtern.
  - `#rd-generate` = Placeholder (`showToast("Not implemented yet", "info")`).
  - Selektoren ausschließlich `#id`/`[data-*]`.
- `frontend/app.js`: `PAGE_MAP` + `rediscovery: "rediscovery"`.
- `frontend/shared/nav.js`: `TOOLS_ITEMS` + `{ id: "rediscovery", label:
  "Rediscovery", icon: "fa-rotate-left" }`.
- `frontend/tests/rediscovery.spec.js` (neu): 6 Playwright-Tests,
  `beforeEach` seed `{ scenario: "rediscovery" }`.

### Testlauf

- `npm install` in `frontend/` erfolgreich (node_modules fehlte, Disk frei).
- `cargo build` → grün (gecacht).
- **Port-Konflikt:** Host-Port `3001` ist durch einen Docker-Mapping
  (`e10-grafana 0.0.0.0:3001->3000/tcp`) belegt → Playwright-`webServer`
  (reuseExistingServer:false) startet nicht. Workaround: temporäre, NICHT
  committete `playwright.rediscovery.config.js` auf Port `3101` mit eigener
  DB; danach entfernt. Empfehlung für CI/Verifier: freien Port nutzen oder
  `reuseExistingServer:true` mit isolierter DB.
- `npx playwright test tests/rediscovery.spec.js` (Port 3101) → **6 passed**.
- Voller Frontend-Suite-Lauf (Port 3101) → **76 passed** (6 neu + 70
  bestehend, keine Regression durch `app.js`/`nav.js`).

### Restore / Commit

- Temp-Artefakte entfernt (`playwright.rediscovery.config.js`,
  `test-playwright-rd.db*`).

## SETUP BASELINE

**Datum:** 2026-10-05 · Agent: feature-dev-setup

### Git / Branch
- `clanker-git status -sb` → `## feature/issue-83-rediscovery-page...origin/main`
  (nur `?? progress-feature-issue-83-rediscovery-page.md` untracked; sonst sauber)
- `clanker-git log --oneline -1` → `1bcc6af feat(#82): rediscovery stats endpoint over shared facets (#114)`
- Branch korrekt, base origin/main @ 1bcc6af bestätigt.

### Build Baseline — BLOCKIERT durch vollen Wurzel-FS
Systemzustand: `df -h /` → `/dev/md2 904G 858G 0 100% /` (**0 Byte frei**).
`free -h` → Swap 31Gi total / 31Gi used / 15Mi free.

| Befehl | Ergebnis |
|---|---|
| `cargo build` | **OK** — `Finished dev profile ... in 34.87s` (Artefakte bereits gecacht, keine neuen Writes) |
| `cargo test --no-run` | **FEHLER** — Linker: `collect2: fatal error: ld terminated with signal 7 [Bus error], core dumped` (parallel-Linking OOM/IO) |
| `cargo test --no-run -j 2` | **FEHLER** — `error: could not compile ... (test "api_daily"); Caused by: No space left on device (os error 28)` |
| `cargo check` | **FEHLER** — `failed to create directory .../target/debug/.fingerprint/... Caused by: No space left on device (os error 28)` |

→ Nur `cargo build` grün (gecacht). Test-Compile und `cargo check` scheitern an ENOSPC.
Wörtliche Kernfehler:
```
collect2: fatal error: ld terminated with signal 7 [Bus error], core dumped
No space left on device (os error 28)
```

### Frontend / Playwright
- `frontend/node_modules` existiert **nicht**; `frontend/node_modules/.bin/playwright` fehlt.
- `package.json` devDependency: `@playwright/test ^1.52.0` (nicht installiert).
- `npx playwright --version` → `1.63.0` (npx lädt transient nach `~/.npm/_npx`).
- `npx playwright test --list` → **nicht lauffähig**: zunächst `ERR_MODULE_NOT_FOUND`
  (`@playwright/test` fehlt), dann `npm error code ENOSPC ... no space left on device` (EXIT 228).
- Chromium-Browser sind gecacht (`~/.cache/ms-playwright`: chromium-1208/1223/1243).
- `frontend/playwright.config.js` + `frontend/tests/*.spec.js` vorhanden (webServer = `cargo run -- serve --port 3001`).

**Fazit:** `npx playwright test` ist hier **nicht lauffähig**, weil `@playwright/test` nicht
installiert ist und `npm install` an ENOSPC scheitert. Für DoD-Testlauf nötig: (a) FS freiräumen,
(b) `npm install` in `frontend/`.

### Disk-Verursacher (Top)
`/home/momo/repos 123G` (davon `target` 33G) · `/opt/apps 72G` · `/tmp/mmm-base 41G` ·
`/opt/backups 26G` · `/tmp/mmm-bpfix 23G` · `/tmp/mmm-rel 20G` · `/tmp/mmm 8.3G` ·
`/home/momo/Backups 6.1G` · `/tmp/936r 5.6G` · `/tmp/mm-review-109 5.5G` · `/tmp/rev103 5.4G` ·
`/tmp/mm-review-111 4.5G` · `/tmp/rev113 4.2G` · `/tmp/936v 4.2G` · `/var/log 4.9G`.

### Blocker → Issue
Vorbestehendes Issue **#112** `[planet] Wurzel-FS 100% voll — cargo test scheitert im Linker (Bus error, signal 7)`
(open, 2026-10-05T14:12Z) deckt exakt diesen Blocker ab → **Kommentar statt Duplikat**:
https://github.com/momokli/openclaw-deploy/issues/112#issuecomment-5997598607

**Baseline-Status: ROT/blockiert** — Setup kann Test-Baseline nicht grün verifizieren, bis Wurzel-FS
freigeräumt ist. `cargo build` allein grün (gecacht).
