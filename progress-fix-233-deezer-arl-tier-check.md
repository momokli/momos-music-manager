# Progress — Issue #233: fix(music-api): FLAC-Download wird nicht durchgeführt

Repo: momokli/momos-music-manager
Branch: fix/233-deezer-arl-tier-check
Basis: main · PR-Ziel: main
Fokus-Milestone: music-api-0.2.0 (#23)

## Dispatch-Validierung (2026-10-16)

Kein PR schließt #233 (`closes/fixes #233`) — kein gemergter Fix. Dispatch gültig.

Root-Cause (aus Issue-Kommentaren, 2026-10-10):
- Server-Config korrekt: `DEEMIX_BITRATE=9`, deemix `maxBitrate: 9`.
- ARL ist **Deezer Free** (`OFFER: Deezer Free`, `lossless:false`, `high:false`).
- → deemix fällt auf 128 kbps zurück. Kein Code-Bug in der Bitrate-Logik.

## Deliverable (Code-Fix, wie im Issue gefordert)
1. music-api prüft beim Start den Deezer-ARL-Tier und **loggt/warnt**, wenn
   `OFFER != Premium/HiFi` (kein FLAC möglich).
2. Neuer geschützter Endpoint `GET /account` liefert den Tier
   (offer, lossless/high/low, userId) — und `/health` bleibt public.
3. Config-Validierung: Warnung, wenn `DEEMIX_BITRATE=9` angefordert ist, der
   Tier aber kein lossless erlaubt.

## Stages

- [x] plan (feature-dev-planner)
      Befund: Vorarbeit-Commit b1ee571 existiert (gw-light `deezer.getUserData`,
      parst `OFFER_NAME` + `web_sound_quality.lossless`), aber NICHT in main gemergt
      (nur auf origin/feat/music-api-fallback|url-orders). Lücken: hardkodierte URL
      (nicht mockbar), kein /account, keine Bitrate-Validierung.
      Plan: ArlTier-Struct + pure parse_user_data + fetch_arl_tier (mockbar via
      DEEZER_GW_BASE), AppState.arl_tier Cache, init_arl_tier (gespawnt, nie Boot-Block),
      GET /account (protected), Bitrate-Validierung.
- [x] setup (feature-dev-setup)
- [x] develop (feature-dev-developer)
- [ ] verify (feature-dev-verifier)
- [x] test (feature-dev-tester)
- [ ] pr (feature-dev-developer)
- [ ] review (feature-dev-reviewer)

## Spec (aus Plan, für Developer)

### 1. `music-api/src/deezer.rs`
- `pub struct ArlTier { pub offer: String, pub lossless: bool, pub high: bool, pub low: bool, pub user_id: Option<i64> }`
- `impl ArlTier { pub fn can_stream_flac(&self) -> bool { self.lossless } }`
- `pub fn parse_user_data(body: &serde_json::Value) -> Option<ArlTier>` — pur, tolerant:
  `results.OFFER_NAME` (Default "unknown"), `results.USER.OPTIONS.web_sound_quality.{lossless,high,low}`
  (Default false), `results.USER.USER_ID` (Default None). Kein Panic bei fehlenden Feldern.
- `pub async fn fetch_arl_tier(http, gw_base, arl) -> anyhow::Result<Option<ArlTier>>`:
  leere/whitespace ARL → `Ok(None)` OHNE HTTP. Sonst GET
  `{gw_base}?method=deezer.getUserData&api_version=1.0&api_token=***` mit Header
  `Cookie: arl=<arl>` + User-Agent. Interner `tokio::time::timeout(~5s)`. Body NIE loggen.
- Unit-Tests: HiFi (lossless true), Free (lossless false), fehlende Felder (Defaults/kein Panic),
  leere ARL → Ok(None) ohne Netz.

### 2. Config + AppState + Startup
- `config.rs`: `pub deezer_gw_base: String` — env `DEEZER_GW_BASE`, Default
  `https://www.deezer.com/ajax/gw-light.php`, trailing-slash getrimmt (analog deezer_base).
- `lib.rs`: `AppState.arl_tier: Arc<tokio::sync::RwLock<Option<deezer::ArlTier>>>`.
- `lib.rs`: `pub async fn init_arl_tier(state: &Arc<AppState>)`: ARL leer → warn + logs.push warn;
  sonst fetch → cache; Some → info; None/Err → warn (Netzfehler geschluckt, KEIN Abbruch).
  Darin Bitrate-Validierung: `deemix_bitrate==9 && !tier.can_stream_flac()` →
  `warn!("DEEMIX_BITRATE=9 requested but ARL tier '{offer}' has no lossless — downloads fall back")` + logs.push warn.
- `main.rs`: Config/AppState-Literale ergänzen; `tokio::spawn(init_arl_tier(state.clone()))`
  NACH state-Aufbau, NIE `.await` im Boot-Pfad.

### 3. API
- `api.rs` router(): `.route("/account", get(get_account))` (protected).
- Handler: cached tier aus state.arl_tier; wenn None und ARL gesetzt → on-demand fetch mit
  kurzem Timeout, dann cachen. Response JSON:
  `{ offer, lossless, high, low, userId, canStreamFlac }`.
  ARL nicht gesetzt → 409 `{"error":"DEEMIX_ARL not configured"}`.
  Fetch-Fehler → 502 `{"error":...}`. `/health` bleibt public/unchanged.

### 4. Tests (music-api/tests/api.rs)
- Harness Config um `deezer_gw_base` ergänzen (Default tote URL `http://127.0.0.1:1`), damit
  bestehende Tests unberührt bleiben. AppState-Literal ergänzen (`arl_tier`).
- gw-light-Mock (kleiner TCP-Listener wie im bestehenden Harness für deezer/analyzer) liefert
  ein getUserData-Payload; Integrationstest: `/account` ohne Token → 401; mit Token + Mock
  → 200 mit offer/lossless; ARL leer → 409.

### Doku
- `music-api/README.md` kurz: `/account`, `DEEZER_GW_BASE`, FLAC braucht Premium/HiFi-ARL.

## Log
- setup: Branch fix/233-deezer-arl-tier-check @ f071e02 (origin/main), Baseline cargo check grün.

### Stage develop (2026-10-16, feature-dev-developer)
- `deezer.rs`: `ArlTier` (+ `can_stream_flac`), pure `parse_user_data`, mockbarer
  `fetch_arl_tier` (5s Timeout, leere ARL → Ok(None) ohne HTTP, Body nie geloggt),
  4 Unit-Tests.
- `config.rs`: `deezer_gw_base` (env `DEEZER_GW_BASE`, Default gw-light, trailing-slash getrimmt).
- `lib.rs`: `AppState.arl_tier` (RwLock-Cache) + `init_arl_tier` (gespawnt, Netzfehler
  geschluckt, Bitrate-Validierung `DEEMIX_BITRATE=9 && !lossless` → warn + logs.push).
- `main.rs`: AppState-Literal + `tokio::spawn(init_arl_tier(&state))` NACH state-Aufbau,
  nie `.await` im Boot-Pfad.
- `api.rs`: protected `GET /account` (+ Handler, cache-first, on-demand fetch,
  409 ohne ARL, 502 bei Fetch-Fehler). `/health` unverändert public.
- `tests/api.rs`: Harness um `deezer_gw_base`/`arl_tier` erweitert (Default tote URL),
  gw-light-Mock + 3 Integrationstests (401/200+Cache/409).
- `README.md` + `.env.example`: `/account`, `DEEZER_GW_BASE`, FLAC braucht Premium/HiFi-ARL.
- `cargo test --manifest-path music-api/Cargo.toml` → **120 passed, 0 failed**
  (40 lib + 80 integration).

### Stage test (2026-10-16, feature-dev-tester)
- `cargo test --manifest-path music-api/Cargo.toml` auf planet → **122 passed, 0 failed**
  (40 lib + 82 integration, 0 ignored).
- Integration/E2E über `build_router` mit gw-light-Mock (`music-api/tests/api.rs`):
  - `GET /account` ohne Bearer → **401** (`account_requires_auth`) ✓
  - Bearer + Mock `Deezer Free` (lossless=false) → **200**, canStreamFlac=false
    (`account_reports_free_tier_without_flac`, **neu ergänzt**) ✓
  - Bearer + Mock HiFi (lossless=true) → **200**, canStreamFlac=true
    (`account_reports_hi_tier_from_gw_light`, inkl. Cache-Hit) ✓
  - ARL nicht gesetzt → **409** (`account_without_arl_is_conflict`) ✓
  - gw-light down/Timeout → **502**
    (`account_returns_bad_gateway_when_gw_light_unreachable`, **neu ergänzt**) ✓
- Neue Tests: 2 (Free-Tier-Payload + erreichbar/unerreichbarer gw-light-Mock), 74 Zeilen.
- Commit: `12f8842` (momo-clanker[bot]).
- Findings: keine Blocker. Request-Pfad über `build_router` verhält sich wie spezifiziert;
  cache-first + on-demand-Fetch korrekt, 409/502-Pfade sauber getrennt.

### Stage setup (2026-10-16, feature-dev-setup)
- `clanker-git fetch origin` — origin/main aktuell (kein neuer Commit).
- Branch `fix/233-deezer-arl-tier-check` von `origin/main` erstellt und ausgecheckt.
- Base-Commit: f071e02a30410e6ffa1aef3950f0eb3999fa1b1f
  (`fix(hub): deliver toasts with non-ASCII text (#469) (#470)`).
- Baseline `cargo check --manifest-path music-api/Cargo.toml` → **exit 0**, grün
  (`Finished dev profile ... in 16.61s`). Kein Blocker, kein Issue nötig.
