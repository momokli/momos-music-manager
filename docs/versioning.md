# Versioning, Channels & Releases

Dieses Dokument beschreibt das nachhaltige Versioning-Konzept: wie die
Version eines Builds bestimmt wird, welche Kanäle der Autoupdater kennt und
wie ein Release ausgerollt wird. Stand: 2026-09-02.

## 1. Version eines Builds

| Build-Typ | Auslöser | Formel | Beispiel |
|---|---|---|---|
| **Release** | Tag `v*` auf `main` | Tag ohne führendes `v` | `v1.2.0` → `1.2.0` |
| **Dev** (rolling) | Push auf `main` (CI), jeder lokale Build | `<Cargo.toml-Version>-dev+<sha8>` | `1.1.0-dev+4eaa1d93` |

**Der Tag ist die Wahrheit für Releases.** Die CI liest die Version nie aus
`Cargo.toml`, sondern aus `scripts/resolve-version.sh` — der einzigen
Versionsquelle für CI und Packaging:

- `GITHUB_REF == refs/tags/v*` → Tag-SemVer (z. B. `v1.1.0` → `1.1.0`)
- `GITHUB_REF == refs/heads/main` (oder kein `GITHUB_REF`, z. B. lokal) →
  `<cargo-metadata-Version>-dev+<git rev-parse --short=8 HEAD>`

`Cargo.toml` bleibt die Basis für Dev-Builds und wird per Runbook (unten)
„lose synchron" vor dem Tagen auf die Release-Version gehoben. Ein
vergessener Bump bricht nichts: Der Tag-Build ignoriert ihn, Dev-Builds
bleiben nur bis zum nächsten Bump auf der alten Basis (harmlos, da
Dev-Updates über den SHA-Vergleich laufen, siehe Abschnitt 3).

### Mechanik

- `build.rs` liest die Env-Var `MMM_VERSION` (Fallback:
  `CARGO_PKG_VERSION`) und setzt `cargo:rustc-env=MMM_VERSION` — damit ist
  `env!("MMM_VERSION")` überall verfügbar (CLI `--version`, `/api/version`,
  Startup-Log, Telemetrie, Autoupdater).
- Die Packaging-Skripte (`scripts/package-{linux,macos}.sh`,
  `scripts/package-windows.ps1`) bevorzugen `MMM_VERSION` und fallen lokal
  auf `cargo metadata` zurück. Damit enthalten Dateinamen, `Info.plist` und
  die `VERSION`-Datei im Archiv die volle Versionszeichenkette inkl.
  Build-Metadaten (`1.1.0-dev+abc1234`).
- SemVer-Konformität: `1.1.0-dev+4eaa1d93` = pre-release `dev` +
  build-metadata `4eaa1d93`. Precedence: `1.1.0-dev+*` < `1.1.0`; alle
  `1.1.0-dev+*` sind untereinander **precedence-gleich** (build-metadata
  wird von SemVer ignoriert) — genau das nutzt der rolling Vergleich.

## 2. Asset-Namen in Releases

| Kanal | Schema | Beispiel |
|---|---|---|
| Release (Tag `v1.2.0`) | `momos-music-manager-<semver>-<os-arch>.<ext>` (+ `.sha256`) | `momos-music-manager-1.2.0-linux-x64.tar.gz` |
| Dev (main, SHA `abc1234`, Basis `1.1.0`) | `momos-music-manager-<basis>-dev+<sha8>-<os-arch>.<ext>` (+ `.sha256`) | `momos-music-manager-1.1.0-dev+abc1234-linux-x64.tar.gz` |
| Dev, stabile Namen (nur `latest-main`, für Landing Page + Autoupdater) | `momos-music-manager-latest-<os-arch>.<ext>`, `Momo-s-Music-Manager-latest.dmg` (+ `.sha256`) | `momos-music-manager-latest-linux-x64.tar.gz` |
| Beide | `SHA256SUMS` (aggregiert) + `SHA256SUMS.minisig` (signiert) | — |

Das `+` ist auf allen Zielplattformen und in GitHub-URLs sicher. `latest-main`
wird bei jedem main-Push per `--clobber` aktualisiert; ein Cleanup-Step
entfernt anschließend alte versionierte Assets (Version ≠ aktuell) und
Legacy-Namen (`Momo.s-Music-Manager-*`). Stabile Namen, `SHA256SUMS` und
`SHA256SUMS.minisig` bleiben erhalten.

## 3. Autoupdater-Kanäle

Der Kanal eines Builds ist eine Eigenschaft seiner **Version**: Version mit
pre-release (`-dev+`) → Dev-Build; ohne → Release-Build. Seit der
Kanal-Wahl (Settings-Seite) ist der **verfolgte** Kanal ein Setting
(`autoupdate.channel`, Default = Kanal des laufenden Builds) — der
**eingebettete** Kanal der Version bestimmt nur noch diesen Default (und die
Download-Logik der Landing Page).

| Kanal | Basis-URL (Default) |
|---|---|
| `rolling` (Default von Dev-Builds) | `…/releases/download/latest-main` (`DEFAULT_BASE_URL`) |
| `release` (Default von Release-Builds) | `https://github.com/momokli/momos-music-manager/releases/latest/download` (`DEFAULT_RELEASE_BASE_URL`; GitHub leitet auf das neueste Non-Prerelease-Release um) |

`MOMOS_AUTOUPDATE_BASE_URL` / `[autoupdate] base_url` überschreibt den
Kanal-Default.

Verhalten:

- **Kanal-Guards:** `check`/`apply` laufen gegen den **gewählten** Kanal
  (env > UI > TOML > Default = eingebetteter Kanal). Ein expliziter
  Kanalwechsel ist **kein** Fehler mehr: Wer als Dev-Build den
  Release-Kanal wählt, bekommt dort geprüfte Stable-Updates (und umgekehrt).
  Der Guard (`ChannelMismatch`) greift nur noch für **inkonsistente** Fälle:
  Wenn die Update-Quelle den *anderen* Kanal ausliefert als gewählt — etwa
  durch einen `base_url`-Override auf den falschen Feed. In `update check`
  und `update apply` mit Handlungs-Hinweis ausgegeben.
- **Rolling-Vergleich (Dev):** `latest > current` → Update verfügbar;
  precedence-gleich **und** andere Versionszeichenkette (neuer SHA) → Update
  verfügbar; sonst up to date. Release: nur `latest > current`.
- **Artefakt-Auflösung:** über den **versionierten** Namen
  (`momos-music-manager-<version>-<os-arch>.<ext>`), abgeleitet aus der
  geparsten Manifest-Version (`Version::to_string()` erhält die
  build-metadata). Der stabile `-latest-`-Name bleibt nur für die
  Download-Buttons der Landing Page relevant (CI-seitig).
- `update status` zeigt u. a. den Kanal (Basis-URL).
- **CLI vs. Server:** Die CLI (`update check|apply|status`) löst den Kanal
  nur aus Env/TOML/eingebettetem Default auf (kein DB-Zugriff) — ein in der
  Settings-UI gewählter Kanal gilt für Server-Endpoints und den Start-Check,
  nicht für die CLI (Folge-PR: CLI liest die DB).

## 4. Release-Runbook

1. `CHANGELOG.md` pflegen (Unreleased-Eintrag nach `## [Unreleased]`).
2. `Cargo.toml` auf die Release-Version heben (z. B. `1.2.0`) — derselbe
   Commit hebt auch `site/index.html` (Hero + Footer) an.
3. Commit + Push auf `main`.
4. Tag erstellen und pushen:

   ```bash
   git tag v1.2.0 && git push origin v1.2.0
   ```

   `build-all.yml` baut alle Plattformen und publiziert die Assets
   `momos-music-manager-1.2.0-*` plus signiertes Manifest.
5. Verifizieren:

   ```bash
   gh release view v1.2.0 --json assets -q '.assets[].name'
   gh release view v1.2.0 -p SHA256SUMS.minisig
   ```

**Warnung:** Nie `build-all.yml` per `workflow_dispatch` auf einem **alten
Tag** ausführen — alter Code + neuer Workflow erzeugt Assets `1.2.0` mit
Binaries, die eine andere Version melden (Fehllabeling). Für Alt-Tag-Repairs
existiert `repair-release.yml` (Abschnitt 5).

## 5. v1.1.0-Repair (Alt-Tag-Nachbesserung)

Das Release `v1.1.0` trägt Assets `momos-music-manager-1.0.1-*`, weil der
Tag-Build die Version noch aus `Cargo.toml` (1.0.1) las. Der Workflow
`.github/workflows/repair-release.yml` benennt die Assets auf den Tag um,
regeneriert `.sha256` + `SHA256SUMS`, signiert neu und löscht die alten
Assets:

```bash
# GitHub UI: Actions → Repair release assets → Run workflow
# tag: v1.1.0, old_version: 1.0.1 (Default)
```

Danach zeigt `gh release view v1.1.0 --json assets -q '.assets[].name'` nur
noch `momos-music-manager-1.1.0-*` + `SHA256SUMS`(+`.minisig`).

**Grenze:** Die *eingebettete* Versionszeichenkette (`--version`,
`/api/version`) der v1.1.0-Binaries bleibt `1.0.1` — sie wurde beim Tag-Build
mit altem Code gebacken und kann ohne Rebuild nicht korrigiert werden (ein
Rebuild vom Tag erzeugt wieder `1.0.1`, da der alte `build.rs` `MMM_VERSION`
ignoriert). Der Repair ist daher Kosmetik/Konsistenz (Asset-Namen +
signiertes Manifest), kein inhaltlicher Fix — das behebt sich erst mit dem
nächsten Release.

## 6. Update-Status in der UI

Seit v1.1.1 zeigt die **Settings-Seite** (`#settings`, Nav-Sektion
„System") den Update-Status, steuert die Autoupdater-Einstellungen und
wählt den **Update-Kanal** (`release` | `rolling`). Backend: Modul
`src/api/update.rs`, Persistenz in der SQLite-KV-Tabelle `settings`
(Migration 024).

### Endpoints

| Endpoint | Zweck |
|---|---|
| `GET /api/update/status` | Version, **effektiver Kanal** (`channel`: `rolling`/`release`, Quelle in `channelSource`, verfügbare Kanäle in `availableChannels`), effektiver Enabled-Wert + Quelle, **effektives Auto-Apply-Intervall** (`autoApplyIntervalSecs`, Quelle in `autoApplyIntervalSource`), Basis-URL des gewählten Kanals, Artifact, letzter Check (`lastCheckAt`, `lastCheckStatus`, `lastCheckResult`, `lastCheckError`), `pendingUpdate` (aus `update-state.json`), `platformSelfInstall` |
| `POST /api/update/check` | Führt einen verifizierten Check gegen den **gewählten Kanal** aus (30-s-HTTP-Timeout), persistiert das Ergebnis und gibt den frischen Status zurück. Netz-/Signaturfehler sind **kein** HTTP-Fehler: `lastCheckStatus: "error"` bei 200 |
| `POST /api/update/settings` | Body `{"autoUpdateEnabled": bool}` und/oder `{"channel": "rolling"|"release"}` und/oder `{"autoApplyIntervalSecs": <Sekunden>}` (mind. ein Feld) — persistiert Toggle, Kanal und/oder Intervall. **409**, wenn Env/TOML den Wert fixieren (es wird nichts geschrieben); **400** bei ungültigem Body oder Kanalwert. Ein Kanalwechsel löscht den Cache des letzten Checks (`autoupdate.last_check_*`) — ein Ergebnis vom alten Kanal gilt nicht für den neuen |
| `POST /api/update/apply` | Manuelles „Update now" gegen den **gewählten Kanal**: Linux/Windows → Swap; macOS → **DMG-Self-Install** (Mount → `.app`-Ersetzung → Unmount, siehe §7). Erfolg → `{outcome: "installed", restartNeeded: true}`; schlägt der Self-Install fehl → Fallback `{outcome: "downloaded", path: …}` (verifizierter DMG in `~/Downloads`). 409 bei disabled/inkonsistenter Quelle (Kanal-Mismatch), 404 wenn kein Update verfügbar |

### Precedence-Regel (Env > UI > TOML > Default)

Für **Enabled** (Default **true**):

1. `MOMOS_AUTOUPDATE_ENABLED` gesetzt **und** parsebar → gewinnt (`env`)
2. sonst UI-Wert in `settings['autoupdate.enabled']` → gewinnt (`ui`)
3. sonst `[autoupdate] enabled` aus `config.toml` → gewinnt (`toml`)
4. sonst Default **true** (`default`)

Für **Channel** (Default = Kanal des laufenden Builds: Dev-Build →
`rolling`, Release-Build → `release`):

1. `MOMOS_AUTOUPDATE_CHANNEL` gesetzt **und** parsebar
   (`"rolling"`/`"release"`) → gewinnt (`env`)
2. sonst UI-Wert in `settings['autoupdate.channel']` → gewinnt (`ui`)
3. sonst `[autoupdate] channel` aus `config.toml` → gewinnt (`toml`)
4. sonst Default (eingebetteter Kanal) (`default`)

Für **Auto-Apply-Intervall** (Default **14400 s = 4 h**, `0` = periodische
Schleife aus):

1. `MOMOS_AUTOUPDATE_INTERVAL_SECS` gesetzt **und** parsebar (Ganzzahl) →
   gewinnt (`env`)
2. sonst UI-Wert in `settings['autoupdate.interval_secs']` → gewinnt (`ui`)
3. sonst `[autoupdate] interval_secs` aus `config.toml` → gewinnt (`toml`)
4. sonst Default 4 h (`default`)

Die Status-Response enthält `enabledSource`/`channelSource`/
`autoApplyIntervalSource`; bei `env`/`toml` rendert die UI Toggle bzw.
Dropdown bzw. Intervall-Select **disabled** mit Hinweis („von
Umgebungsvariable gesetzt" / „von config.toml gesetzt"). Unparsebare
Env-Werte fallen wie bisher durch (kein Breaking Change).

### Basis-URL je Kanal

Der gewählte Kanal entscheidet über die Default-Quelle:
`rolling` → `latest-main` (Dev-Builds von `main`), `release` →
`releases/latest/download` (neuestes Stable-Release). Ein expliziter Override
(`MOMOS_AUTOUPDATE_BASE_URL` / `[autoupdate] base_url`) hat weiterhin
Vorrang — liefert er dann den *anderen* Kanal aus, meldet der
Kanal-Guard den Widerspruch (`ChannelMismatch`), statt still den falschen
Feed zu prüfen.

### Toggle-, Kanal- & Intervall-Persistenz

Toggle, Kanal und Auto-Apply-Intervall werden in SQLite persistiert
(`settings`-KV, Keys `autoupdate.enabled` (`"true"`/`"false"`),
`autoupdate.channel` (`"rolling"`/`"release"`) und
`autoupdate.interval_secs` (Sekunden als INTEGER-String)) und überleben
Neustarts. Der Start-Check in `serve()` liest die **effektiven** Werte aus
der DB (nicht mehr nur `config.autoupdate_enabled`) und persistiert sein
Ergebnis (`autoupdate.last_check_*`) — `--no-autoupdate` hat weiterhin
höchste Priorität.

### Kanalwechsel (Cross-Channel-Switch)

Die UI bietet ein Kanal-Dropdown neben dem Auto-Update-Toggle; ein Wechsel
läuft über ein **Confirm-Modal**, das erklärt, dass der nächste Check/Apply
gegen den anderen Kanal läuft und „Update now" das Binary des anderen
Kanaltyps installieren kann (z. B. Release-Binary auf einer Dev-Installation
— nach dem Neustart läuft die App als Release-Build auf dem neuen Kanal).
Der Wechsel ist **kein** ChannelMismatch-Fehler mehr — Guards gelten nur
für inkonsistente Quellen (siehe oben).

### macOS: DMG-Self-Install (Phase C)

„Update now" auf macOS installiert den verifizierten DMG jetzt selbst
(Phase C, ersetzt die v1-Limitation): `hdiutil attach` (read-only) →
`.app`-Bundle im Image suchen → `ditto` in ein Staging-Verzeichnis neben
`/Applications` (bzw. `MOMOS_AUTOUPDATE_APP_DIR`/`[autoupdate] app_dir`)
→ atomarer Tausch (bisherige Version wird zu
`Momo's Music Manager.app.updater-bak`, bei Fehlern wird sie
zurückgespielt) → `hdiutil detach`. Die alte Version bleibt als
`.updater-bak` für die manuelle Wiederherstellung stehen und wird beim
nächsten erfolgreichen Self-Install entfernt. `platformSelfInstall` ist auf
macOS damit `true`; schlägt die Installation fehl, fällt `apply` auf den
verifizierten Download nach `~/Downloads` zurück (`outcome: "downloaded"`).

### Kanal-Mismatch

Ein Kanal-Mismatch bedeutet seit der Kanal-Wahl: Die Update-Quelle liefert
den **anderen** Kanal aus als gewählt — z. B. weil `base_url` explizit auf
einen Feed zeigt, der Dev-Builds serviert, während der Release-Kanal
gewählt ist. Die UI zeigt einen Erklärtext (gewählter Kanal vs. gelieferte
Build-Art), der Apply-Button erscheint nicht (`POST /api/update/apply` →
409). Ein **expliziter Kanalwechsel über das Dropdown ist kein Fehler** —
Check/Apply laufen dann einfach gegen den anderen Kanal (siehe oben).

## 7. Auto-Apply (Phase C) — Scheduler & Self-Restart

Seit Phase C installiert der Autoupdater Updates **vollautomatisch**: Der
`serve()`-Scheduler läuft im konfigurierbaren Intervall (§6, Default 4 h,
`0` = aus) und führt pro Zyklus `check → apply → Self-Restart` aus. Die
Settings-Seite zeigt das effektive Intervall als Dropdown (Presets
Off/1 h/4 h/12 h/24 h, gesperrt bei Env/TOML-Pinning) und erklärt, dass
„Auto-Update an" automatisch anwendet und neu startet.

**Ablauf eines Zyklus** (`api::update::run_auto_apply_cycle`,
`autoupdate::update_auto`):

1. effektiver Enabled-Wert (`env > UI > TOML > default true`) — aus, wenn
   disabled;
2. in-flight-Guard: liegt ein unbestätigter Swap-Marker (`update-state.json`)
   vor, wird **nicht** erneut angewendet (kein Stapeln auf laufende
   Health-Grace/Rollback);
3. verifizierter Check gegen den gewählten Kanal (Ergebnis wird wie beim
   manuellen Check persistiert);
4. bei verfügbarer Version: Apply (`Installed` → der Versuch wird als
   `settings['autoupdate.auto_apply_state']` (JSON: `attempted_version`,
   `failures`, Zeitstempel) **vor** dem Neustart aufgezeichnet; macOS-DMG
   wird dabei selbst installiert, §6). `DownloadedOnly` (Self-Install-Fallback)
   und Fehler werden geloggt und im nächsten Zyklus erneut versucht.

**Self-Restart** (`autoupdate::restart`): Nach `Installed` beendet sich der
Prozess; die neue Version startet — je nach Kontext:

- **systemd** (`INVOCATION_ID` gesetzt): kein eigener Relauncher — der
  Service-Manager startet den Prozess neu (`Restart=always` im
  ausgelieferten Unit);
- **Linux/Windows/macOS-Dev-Binary**: detachter Relauncher (neue
  Prozessgruppe, kein stdio) wartet 2 s (Port/Datenbank-Handles des alten
  Prozesses werden frei) und führt das Binary am bisherigen Pfad erneut aus
  (dort liegt jetzt die neue Version);
- **macOS `.app`**: das ersetzte Bundle wird per LaunchServices (`open`)
  neu gestartet — nur wenn das laufende Bundle im Installations-Verzeichnis
  liegt (sonst Hinweis, kein Auto-Start einer falschen Kopie).

Nach dem Neustart greift die bestehende Health-Grace (`swap.rs`): Die neue
Version muss die Grace-Periode überleben, dann wird committet
(`auto_apply_state` wird geleert = Erfolg). Überlebt sie nicht, greift nach
`MAX_UNHEALTHY_STARTS` der Auto-Rollback — und beim Rollback-Event wird der
**Crash-Loop-Breaker** aktiviert: Derselbe Versionsstand wird nicht erneut
automatisch angewendet (frühestens, wenn eine *neuere* Version erscheint;
manuelles `update apply` geht immer). Zusammen mit dem in-flight-Guard gibt
es damit **keinen Endlos-Restart-Loop** bei fehlgeschlagenen Updates.

**Abgrenzung**: Der manuelle Pfad (Settings „Update now", CLI `update
apply`) bleibt unverändert und startet nie automatisch neu
(`restartNeeded: true` in der Response; Admin/Supervisor startet neu).
Telemetrie-PR #20 hängt an `ApplyOutcome::Installed` in `verify::apply` —
der Versionswechsel-Pfad und das Outcome-Schema sind unverändert, der
`app.updated`-Hook bleibt kompatibel und feuert inzwischen in **beiden**
Install-Zweigen (Binary-Swap und DMG-Self-Install; #20 ist gemergt).
