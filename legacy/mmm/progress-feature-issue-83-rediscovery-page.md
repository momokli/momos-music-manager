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

## VERIFIER

**Datum:** 2026-10-05 · Agent: feature-dev-verifier (subagent verify-83-rediscovery)
**Commit:** 54a37fe · **Ergebnis: PASS**

### Diff-Umfang (`clanker-git show 54a37fe --stat`)

| Datei | +/- | Bewertung |
|---|---|---|
| `frontend/pages/rediscovery.js` (neu) | +502 | OK |
| `frontend/tests/rediscovery.spec.js` (neu) | +196 | OK |
| `frontend/app.js` | +1 (`PAGE_MAP: rediscovery: "rediscovery"`) | OK |
| `frontend/shared/nav.js` | +1 (`TOOLS_ITEMS` id=rediscovery, icon `fa-rotate-left`) | OK |
| `progress-feature-issue-83-rediscovery-page.md` | +254 | Pipeline-Artefakt (siehe Hinweis) |

**KEINE Backend-Änderungen** — `src/**` nicht angefasst. Keine ungewollten Config-/Test-Artefakte im Commit.
Hinweis (nicht blockierend): Die Progress-Datei ist mit im Commit; im Rahmen dieser Pipeline üblich.

### Registrierung (beide Stellen) — PASS
- `frontend/app.js:39` → `rediscovery: "rediscovery"`.
- `frontend/shared/nav.js:51` → `{ id: "rediscovery", label: "Rediscovery", icon: "fa-rotate-left" }`.
- Nav rendert `data-page="rediscovery"` (nav.js:115/132) → Test-Selektor gültig.

### Security / Qualität — PASS
- `innerHTML`: nur mit Template-Strings aus `escapeHtml`-geprüften Werten bzw. internen Helfern.
  `title`/`artist` (`escapeHtml`), `genre`/`musicalKey` (`escapeHtml`), Chips Text **und** Attribut
  (`escapeHtml`), Stats (numerisch via `formatNumber` + `escapeHtml`), Badges (`renderBadge` escapt intern),
  `renderErrorBlock`/`renderEmpty` escapen intern.
- `renderReasons()`: `data-reason="${escapeHtml(r)}"` + Textknoten `${escapeHtml(r)}` → **verbatim**
  aus API-`reasons`, keine Umtextung, kein XSS über `r`/`title`/`artist`.
- Einzige unescapte Interpolation: `data-track-id="${row.trackId}"` (Backend-Integer, kein User-String) —
  Defense-in-depth-Hinweis, kein Blocker.
- Keine `console.log/debug`, kein `debugger`, keine TODO/FIXME. Kein Secret/Token/Passwort im Diff.
- `AbortController`: `_inflight` pro `loadPreview()` neu; alter Request wird abgebrochen;
  `AbortError` wird still geschluckt (`e.name === "AbortError"` → return); zusätzlich `signal.addEventListener("abort")`
  aus `init` → `_inflight.abort()`. Kein Reload beim Filtern (`window.__rdNavMarker` unverändert, Test 4).
- Selektoren ausschließlich `#id` / `[data-*]` (kein `nth-child`) in Page und Spec.
- Keine Client-Filterung nach Pagination: Zeilenzahl == Server-Response `candidates.length` (Test 5).

### Chips & Stats — PASS
- Chips: Klasse `.reason-chip`, Attribut `data-reason` verbatim aus API-`reasons`.
- Stats: `#rd-stats` mit `data-stat="matching|withBpmAndKey|needsAnalysis|notOwned|pushedRecently"`.
  Pflicht-Chips `matching/needsAnalysis/notOwned` vorhanden.

### Unabhängiger Testlauf (eigene Ausführung, freier Port 3101)

Temporäre, NICHT committete `playwright.rd-verify.config.js` (Port 3101, eigene DB `rd-verify.db`,
`retries:0`), danach gelöscht. Wurzel-FS: **71G frei**.

- `npx playwright test tests/rediscovery.spec.js` → **6 passed (10.7s)** — ohne Retries.
- Volle Frontend-Suite → **76 passed (35.7s)** — keine Regression durch `app.js`/`nav.js`.
- `cargo build` → grün (`Finished dev profile in 4.93s`).
- Aufräumen verifiziert: `clanker-git status -sb` sauber (keine Temp-Artefakte, Config/DB entfernt).

### Issue-DoD

- [x] `#rediscovery` lädt via Hash-Router, erscheint im Tools-Menü (Test 1)
- [x] Facet-Änderung aktualisiert Preview server-seitig, kein Reload (Test 4)
- [x] Reason-Chips == API `reasons` verbatim (Test 3)
- [x] Counts-Bar zeigt Stats-Werte (Test 2)
- [x] Kein `pageerror` bei Load/Filter/Paging (Tests 1–6)
- [x] Pagination server-seitig korrekt (`limit`/`offset`, Test 5)
- [x] Keine client-seitige Filterung nach Pagination (Test 5)
- [x] Playwright-Spec nutzt Seed `{ scenario: "rediscovery" }`
- [~] Backend `cargo test`: keine Backend-Änderung → nicht relevant; `cargo build` grün. (Setup-Blocker
  ENOSPC aus Baseline dokumentiert → Issue #112; bei diesem Lauf nicht mehr auftretend.)

### Offene Punkte / Hinweise (nicht blockierend)
- `renderRow`: `data-track-id` unescapt (Integer) — optional `escapeHtml` für Konsistenz.
- `state.error` wird lazy am Modulende initialisiert — funktioniert, Stilhinweis.
- `#rd-generate` ist Placeholder per Plan (`showToast("Not implemented yet","info")`).

### Entscheidung
**PASS** → weiter zu Test. Keine Blocker.

## TESTER

**Datum:** 2026-10-05 · Agent: feature-dev-tester (subagent test-83-rediscovery)
**Commit:** 54a37fe · **Ergebnis: PASS**

### Vorgehen (unabhängig, eigener Lauf)
- Port 3001 war auch in diesem Lauf durch Docker belegt (`ss` bestätigt LISTEN auf `:3001`).
  Daher temporäre, NICHT committete `frontend/playwright.rd-tester.config.js` auf Port **3101**
  mit eigener DB (`test-rd-tester.db`), `retries: 0`; danach restlos gelöscht.
- `frontend/node_modules` vorhanden, `target/debug/momos-music-manager` gecacht → `cargo build` grün
  (`Finished dev profile in 0.16s`). Wurzel-FS: 71G frei.

### E2E-Läufe
| Lauf | Ergebnis |
|---|---|
| `npx playwright test tests/rediscovery.spec.js` | **6 passed (10.8s)**, 0 Retries |
| volle Suite `npx playwright test` | **76 passed (36.4s)**, keine Regression |

### Edge-Cases (zusätzlich manuell, eigener Server + curl / Temp-Spec)
Alle Seed-Vorgaben via `POST /api/testing/seed {scenario:"rediscovery"}`.

1. **Stats-Konsistenz `matching == candidates.total` bei gleichen Facetten** — OK
   (default 8==8; `touchedBeforeDays=1&likedOnly=true` 9==9; `touchedBeforeDays=0` 10==10;
   `requireBpm=true&requireKey=true` 7==7 mit `withBpmAndKey=7`).
2. **Kombination `touchedBeforeDays`+`likedOnly`** — OK (9 Treffer, konsistent in beiden Endpoints).
3. **Pagination letzte Seite** — `limit=3`, `total=8` → letzter `offset=6`, `len==2==min(limit,total-offset)`;
   UI: `limit=4` → 3 Seiten, letzte Seite zeigt exakt 2 Zeilen, `#rd-page-info` = "Page 3 of 3",
   `#rd-next` disabled. **Kein Client-Filter** (Zeilenzahl == Server-Seitenlänge).
4. **Offset > total** → leeres `candidates`-Array, `total` unverändert (kein Fehler).
5. **Leeres Ergebnis → `renderEmpty`** — `#rd-genres=__no_such_genre__` → `#rd-preview .empty-state`
   sichtbar, 0 `tbody tr`, kein `pageerror`.
6. **Kein `pageerror`** in allen 6 Spec-Tests + beiden Edge-Tests (Load/Filter/Paging/Empty).

### DoD aus User-Sicht
- [x] `#rediscovery` via Hash-Router + Tools-Menü erreichbar
- [x] Facet-Änderung aktualisiert Preview server-seitig, kein Reload (`__rdNavMarker` stabil)
- [x] Reason-Chips verbatim == API `reasons` (`data-reason`)
- [x] Counts-Bar zeigt Stats-Werte
- [x] Pagination server-seitig (`limit`/`offset`), keine Client-Filterung nach Pagination
- [x] Kein `pageerror`

### Aufräumen
- Entfernt: `frontend/playwright.rd-tester.config.js`, `frontend/tests/rd-edge-tester.spec.js`,
  `test-rd-tester.db*`. **Kein** Produktivcode geändert. Port 3101 wieder frei.
- `clanker-git status -sb`: nur diese Progress-Datei modifiziert (Pipeline-Artefakt), sonst sauber.

### Blocker
Keine. Bekannter, nicht-blockierender Umgebungsfaktor: Host-Port 3001 dauerhaft durch Docker
belegt (vgl. Setup/Issue #112) → E2E braucht freien Port; in dieser Pipeline dokumentiert/umgangen.

## REVIEWER

**Datum:** 2026-10-05 · Agent: feature-dev-reviewer (subagent review-83-rediscovery)
**PR:** #115 (OPEN, base main, head feature/issue-83-rediscovery-page, HEAD 54a37fe)
**Ergebnis: APPROVE** — kein Merge durch diesen Task (kein re-review-Auftrag).

### Umfang
`clanker-git diff origin/main...HEAD --stat` → **954 insertions(+), 0 deletions**, nur
Frontend + Progress:

| Datei | +/- | Bewertung |
|---|---|---|
| `frontend/pages/rediscovery.js` (neu) | +502 | sauber, kommentiert, konsistent zu `daily.js` |
| `frontend/tests/rediscovery.spec.js` (neu) | +196 | 6 Tests, Seed `{scenario:"rediscovery"}` |
| `frontend/app.js` | +1 | `PAGE_MAP.rediscovery` |
| `frontend/shared/nav.js` | +1 | `TOOLS_ITEMS` id=rediscovery, icon `fa-rotate-left` |
| `progress-*.md` | +254 | Pipeline-Artefakt (Hinweis, nicht blockierend) |

**Keine Backend-Änderung** (`src/**` unangetastet). Diff-Umfang angemessen, keine Fremdartefakte.

### Issue-DoD — vollständig erfüllt
- [x] `#rediscovery` lädt via Hash-Router; im Tools-Menü (`app.js:39` PAGE_MAP + `nav.js` TOOLS_ITEMS, `data-page="rediscovery"`).
- [x] Facettenänderung aktualisiert Preview server-seitig ohne Reload: 300 ms Debounce → `loadPreview()` → GET `/api/rediscovery/candidates`; kein `location`-Zugriff, `window.__rdNavMarker` stabil (Spec-Test 4).
- [x] Reason-Chips == API `reasons`: `renderReasons()` schreibt Wert verbatim in `data-reason` **und** Textknoten (beide `escapeHtml`), kein Umtexten; Präfix-Klassifizierung nur via CSS/Attribut. Abgleich gegen `src/api/rediscovery.rs:491-545`: `last-touched-*`/`only-in-1-playlist`/`in-<N>-playlists`/`pushed-*`/`never-pushed`/`liked`/`low-play-count`/`not-played-since-*`/`bpm-in-range`/`key-match:*`/`genre-match:*`/`not-in-backpack` — deckungsgleich.
- [x] Kennzahlen-Leiste zeigt `stats`-Werte: `#rd-stats [data-stat=matching|withBpmAndKey|needsAnalysis|notOwned|pushedRecently]`, Keys == `RediscoveryStatsResponse` (camelCase), Pflicht-Chips `matching/needsAnalysis/notOwned` vorhanden.
- [x] Keine `pageerror`: Sammel-Assertion in allen 6 Tests, verifiziert 6/76 passed.
- [x] Pagination server-seitig: `limit`/`offset` immer gesendet (`offset = page*limit`), `total` aus Response; letzte Seite/Offset>total korrekt (Tester-Edge-Cases).
- [x] Kein Client-Filter nach Pagination: DOM-Zeilenzahl == `candidates.length` (Spec-Test 5: 5 == 5, Seite 2 == `min(limit,total-offset)`).

### Risiken / Regressionen — geprüft, keine Blocker
- **Registrierungen:** PAGE_MAP + TOOLS_ITEMS beide vorhanden → kein Router-404. `renderDropdownSection` rendert `data-page`-Attribut → Spec-Selektor gültig.
- **Backend:** keine Änderung; nur bestehende Endpoints konsumiert (Query-Params camelCase, `excludePushedSinceDays=0` = off, `touchedBeforeDays` default 365).
- **Escaping/XSS:** `escapeHtml` auf title/artist/genre/musicalKey, Chip-Text+Attribut, Stats (`formatNumber`→`escapeHtml`), `label`; Badges/`renderEmpty`/`renderErrorBlock` escapen intern. Einzige unescapte Interpolation `data-track-id="${row.trackId}"` ist Backend-Integer (`i64`) und **konsistent mit bestehendem Code** (`tracks.js:222`, `digging.js:654`) — kein Blocker.
- **AbortController:** pro `loadPreview()` neuer `_inflight`, alter Request abgebrochen; zusätzlich `init`-`signal`-`abort` → `_inflight.abort()`; `AbortError` still geschluckt; kein Reload.
- **`#rd-generate`:** reiner Placeholder (`showToast("Not implemented yet","info")`), **kein** fetch/Playlist-Anlegen → Issue-Vorgabe "nichts darf Playlists anlegen" erfüllt. Order-missing-Aktion ebenfalls nicht implementiert.
- **Selektoren:** ausschließlich `#id`/`[data-*]`, kein `nth-child`.
- **Stilkonsistenz:** gleiche Struktur wie `daily.js` (`export async function init(container, signal, hashParams)`, modul-level `state`, `render()`/`wireEvents()`, Helfer aus `shared/*`). Lesbar, idiomatisch.

### PR-Body-Referenz
Body beginnt mit **`Closes #83`** (nicht `Refs`) → Issue wird bei Merge automatisch geschlossen. ✅

### Nicht blockierende Hinweise
- `state.error` wird am Modulende lazy initialisiert (funktioniert; Stilhinweis aus Verifier).
- `updatePagination()` setzt `_pagination.page` direkt; während eines laufenden Fetches werden Prev/Next nicht disabled (reine UX-Politur, außerhalb DoD).
- Progress-Datei ist mit committet (Pipeline-Konvention).

### Entscheidung
**APPROVE** — alle DoD-Punkte erfüllt, keine Blocker, keine Backend-Regression. Merge erfolgt durch die Triage/Re-Review-Instanz (`mergeStateStatus: BLOCKED` = erwartet, Freigabe steht aus). DoD-Häkchen als PR-Kommentar gepostet.

## REWORK — mmm-pr-gate REQUEST_CHANGES (2026-10-05)

Gate-Blocker adressiert:

1. **Screenshots (Pflicht, Frontend-Regel)** — 6 PNGs in `docs/screenshots/` aus der echten
   App + Seed `rediscovery` (Chromium headless, isolierter Port, frische DB):
   `rediscovery-overview.png` (Form + Stats + Preview + Pagination),
   `-facets.png`, `-stats.png` (8/7/0/2/1), `-preview-chips.png` (Reason-Chips),
   `-pagination.png` / `-pagination-page2.png` (Page 2 of 3, Showing 6-10, Total 12).
   Capture-Spec/Config waren temporär und wurden wieder entfernt.
2. **README** — Route `#rediscovery` in Tabelle „SPA Pages" ergänzt.
3. **CHANGELOG** — Eintrag `#83` unter `[Unreleased]/Added`.

Optionale Politur umgesetzt: `state.error` im state-Literal deklariert;
Prev/Next während Fetch via `Pagination.setLoading` disabelt + `#rd-loading`-Spinner;
doppelte `input`+`change`-Listener auf einen delegierten `input`-Listener gestrafft.

Verifikation: volle Frontend-Suite 76 passed (inkl. 6 rediscovery-Specs), `node --check` grün.
