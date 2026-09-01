# Plan: Nachhaltiges Versioning-Konzept

Branch: `feat/versioning-concept` · Basis: `main` @ 4eaa1d9 · Stand: 2026-08-31
Scope: NUR Versioning/CI/Doku. Offene PRs #1, #2 und M-Issues #8–#12 bleiben unangetastet.

---

## 0. Ist-Analyse (verifiziert am 2026-08-31)

| Bereich | Befund |
|---|---|
| `Cargo.toml` | `version = "1.0.1"` — einzige Versionsquelle heute |
| Versionsanzeige | `src/main.rs:21` (clap `--version`), `src/main.rs:678` (Startup-Log), `src/api/infrastructure.rs:48` (`GET /api/version`), `src/telemetry/mod.rs:236`, `src/autoupdate/verify.rs:84` (User-Agent), `src/autoupdate/verify.rs:162` (current_version), `src/autoupdate/mod.rs:27` (Recovery) — alle via `env!("CARGO_PKG_VERSION")` |
| Webapp | Kein Trunk. Statisches `frontend/` wird per rust-embed eingebettet; `frontend/app.js:140` zeigt `v<version>` aus `/api/version` im Footer |
| Landing Page | `site/index.html:47,256` hardcodiert `v1.0.1` (Hero + Footer) — wird via `pages.yml` deployed |
| Packaging | `scripts/package-linux.sh`, `scripts/package-windows.ps1`, `scripts/package-macos.sh` lesen die Version via `cargo metadata` (Cargo.toml) |
| CI | `.github/workflows/build-all.yml`: on push main / tags `v*` / PR / dispatch. Publish-Job: main → `latest-main` (prerelease, versionierte + stabile Namen, `--clobber`), Tags → Release mit `--generate-notes`. Manifest-Version wird per `sed -E 's/momos-music-manager-([0-9]+\.[0-9]+\.[0-9]+)-.*/\1/'` aus Dateinamen extrahiert (**bricht mit dev-Versionen**) |
| Autoupdater | `src/autoupdate/*` (M6, PR #14 gemerged): lädt `SHA256SUMS` + `.minisig` von `DEFAULT_BASE_URL` = `…/releases/download/latest-main`, verifiziert Ed25519, löst Artefakt über **stabile** Namen `momos-music-manager-latest-<os-arch>.<ext>` auf, Version via `manifest.version_for()` (zerlegt am **ersten** `-` — **bricht mit `1.1.0-dev+<sha>`**), semver-Vergleich `latest <= current → UpToDate` (**bricht rolling dev-updates**: `-dev+shaA` vs `-dev+shaB` sind precedence-gleich) |
| Docker/Makefile | Nicht vorhanden → nicht betroffen |
| Releases (remote, verifiziert via `gh`) | `v1.1.0` (non-prerelease): Assets heißen `momos-music-manager-1.0.1-*` (10 Assets + `SHA256SUMS` + `SHA256SUMS.minisig`) · `latest-main` (prerelease): 24 Assets, **ohne** `SHA256SUMS.minisig` (Manifest unsigned → Autoupdater lehnt aktuell ab!), plus Legacy-Müll `Momo.s-Music-Manager-latest.dmg`, `Momo.s-Music-Manager-v1.0.1-main-423b105.dmg`, `Momo.s-Music-Manager-v1.0.1-main-f444ca9.dmg` |

**Live-Problem (Nebenbefund):** `latest-main` hat kein signiertes Manifest. Der erste main-Push mit dem neuen Workflow (und gesetztem `MINISIGN_SECRET_KEY`) behebt das automatisch; als Verifikationskriterium in US5 aufnehmen.

---

## 1. Versioning-Schema

### 1.1 Release-Build (Tag `v*`)

**Formel:** `VERSION = TAG ohne führendes "v"` — z. B. `v1.1.0` → `1.1.0`.

**Entscheidung: Der Tag ist die Wahrheit, nicht `Cargo.toml`.** Begründung:
- CI schreibt nie zurück ins Repo (kein Write-Back-Token auf den Default-Branch, kein zusätzlicher Commit/Tag-Drift, keine Race zwischen Tag und Bump-Commit).
- Die `v*`-Trigger-Konvention in `build-all.yml` garantiert, dass jeder Tag-Build eine semantische Version hat.
- `Cargo.toml` bleibt die **Basis für dev-Builds** (siehe 1.2) und wird per Runbook (Abschnitt 4) vor dem Tagen auf die Release-Version gehoben — „lose synchron", bewusst nicht CI-erzwungen. Ein vergessener Bump bricht nichts: Der Tag-Build ignoriert ihn, dev-Builds bleiben nur bis zum nächsten Bump auf der alten Basis (harmlos, da dev-Updates über SHA-Vergleich laufen, siehe 3.3).

### 1.2 Dev-Build (rolling main)

**Formel:** `VERSION = <Cargo.toml-Version>-dev+<sha8>` — z. B. `1.1.0-dev+4eaa1d9`.

- **Basis:** `cargo metadata`-Version (nach Runbook = letzte Release-Version). Konkret, deterministisch, ohne GitHub-API/Netzwerk im CI.
- **Suffix:** `-dev+` + `git rev-parse --short=8 HEAD`.
- **SemVer-Konformität:** `1.1.0-dev+4eaa1d9` → pre-release `dev`, build-metadata `4eaa1d9`. Precedence: `1.1.0-dev+*` < `1.1.0` (dev ist *kleiner* als das Release), und alle `1.1.0-dev+*` sind **precedence-gleich** (build-metadata wird von SemVer ignoriert). Genau diese Eigenschaft wird in 3.3 für den Rolling-Vergleich genutzt.
- **Verworfene Alternative „nächste Minor"** (`<major>.<minor+1>.0-dev+sha`): bräuchte Kenntnis des letzten Releases im CI (GitHub-API/Tag-Lookup) → mehr Moving Parts, nicht deterministisch offline. Cargo.toml-Basis ist einfacher und nach dem Bump vor dem Tag identisch.

### 1.3 Mechanik (Stack: Rust + statisches Frontend, kein Trunk/Docker)

1. **`build.rs`** (neben dem bestehenden `rerun-if-changed=frontend/`):
   - `MMM_VERSION`-Env-Var lesen; Fallback `CARGO_PKG_VERSION` (wird von Cargo an Build-Scripts gesetzt).
   - `println!("cargo:rustc-env=MMM_VERSION={version}")` + `println!("cargo:rerun-if-env-changed=MMM_VERSION")` (erzwingt Rebuild bei Änderung).
2. **Alle `env!("CARGO_PKG_VERSION")`-Stellen → `env!("MMM_VERSION")`** (Dateien siehe Ist-Analyse; `MMM_VERSION` ist durch den Fallback im build.rs immer gesetzt → `env!` bleibt safe).
3. **Neu `scripts/resolve-version.sh`** — einzige Quelle für die CI-Version:
   - `GITHUB_REF == refs/tags/v*` → `echo "${GITHUB_REF#refs/tags/v}"` (Tag-SemVer)
   - `GITHUB_REF == refs/heads/main` (oder sonstiger Branch/PR/dispatch) → `<cargo-metadata-version>-dev+<git rev-parse --short=8 HEAD>`
   - Lokal ohne `GITHUB_REF`: dev-Formel (identisch mit main-Zweig).
4. **Packaging-Skripte:** `VERSION="${MMM_VERSION:-$(cargo metadata …)}"` in `package-linux.sh`, `package-windows.ps1`, `package-macos.sh` (inkl. `Info.plist`-`CFBundleVersion`/`CFBundleShortVersionString`, DMG-Dateiname, `VERSION`-Datei im Archiv — die fließen bereits aus `$VERSION`).
5. **Webapp/Landing:** `frontend/` zeigt die Version dynamisch über `/api/version` → keine Änderung nötig. `site/index.html` (statisch): hardcodierte `v1.0.1`-Stellen auf `v1.1.0` anheben (einmalig, siehe US7; bei Releases per Runbook pflegen).

---

## 2. CI-Anpassung (`build-all.yml`)

### 2.1 Asset-Namens-Schemata (exakt)

| Kanal | Schema | Beispiel |
|---|---|---|
| Release (Tag `v1.2.0`) | `momos-music-manager-<semver>-<os-arch>.<ext>` (+ `.sha256` je Asset) | `momos-music-manager-1.2.0-linux-x64.tar.gz` |
| Dev (main, SHA `abc1234`, Basis `1.1.0`) | `momos-music-manager-<basis>-dev+<sha8>-<os-arch>.<ext>` (+ `.sha256`) | `momos-music-manager-1.1.0-dev+abc1234-linux-x64.tar.gz` |
| Dev, stabile Namen (nur `latest-main`, für Landing Page + Autoupdater) | `momos-music-manager-latest-<os-arch>.<ext>`, `Momo-s-Music-Manager-latest.dmg` (+ `.sha256`) — **unverändert** | `momos-music-manager-latest-linux-x64.tar.gz` |
| Beide | `SHA256SUMS` (aggregiert) + `SHA256SUMS.minisig` (signiert) | — |

`+` ist auf allen Zielplattformen und in GitHub-URLs sicher (kein Windows-Verbotzeichen).

### 2.2 Workflow-Änderungen (pro Job)

1. **Jeder Build-Job** (linux-x64, linux-arm64, windows-x64, windows-arm64, macos-universal): vor dem Packaging-Schritt einen Step einfügen:
   `echo "MMM_VERSION=$(bash scripts/resolve-version.sh)" >> "$GITHUB_ENV"`
   (Windows-Runner: bash ist via Git Bash vorhanden; `resolve-version.sh` nutzt nur `cargo metadata` + `git`, kein Bash-Spezifisches).
2. **macos-universal Job:** Rename-Step nutzt `"${MMM_VERSION}"` statt `cargo metadata`-Abfrage (der `cp`-Befehl bleibt, nur `VERSION` kommt aus der Env).
3. **Publish-Job:**
   - `VERSION` nicht mehr per `sed`-Regex aus Dateinamen extrahieren (bricht mit `-dev+`), sondern `VERSION="$(bash scripts/resolve-version.sh)"`.
   - main-Zweig: `latest-main` sicherstellen (unverändert), versionierte dev-Assets + stabile Namen + SHA256SUMS + `.minisig` uploaden, `--clobber` (wie heute).
   - **Neu (empfohlen): Cleanup-Step nach erfolgreichem Upload** — entfernt Assets aus `latest-main`, die nicht zum aktuellen Schema gehören:
     - versionierte Assets (`momos-music-manager-<version>-<os-arch>.<ext>`), deren Version ≠ `$VERSION`,
     - Legacy-Namen `Momo.s-Music-Manager-*` (Punkt statt Bindestrich, `main-<sha>`-Varianten — 3 Assets liegen aktuell dort).
     - **Behalten:** stabile Namen (`*-latest-*`, `Momo-s-Music-Manager-latest.dmg*`), `SHA256SUMS`, `SHA256SUMS.minisig`, aktuelle versionierte Assets.
     - Umsetzung: `gh release view latest-main --json assets -q '.assets[].name'` filtern, dann `gh release delete-asset latest-main <name> --yes`.
   - Tag-Zweig: Release erstellen (unverändert), nur versionierte SemVer-Assets + SHA256SUMS + `.minisig` (keine stabilen Namen) — Upload-Liste explizit wie bisher, `--clobber`.
   - Minisign-Flow (Hard-Fail ohne `MINISIGN_SECRET_KEY`, Self-Verify gegen `scripts/minisign.pub`) unverändert.
   - **Kleine Erweiterung:** Publish-Bedingung um `workflow_dispatch` auf `main` ergänzen (`github.event_name == 'push' || github.event_name == 'workflow_dispatch'`, weiterhin `ref == main` oder Tag) → ermöglicht Test-Publishes ohne Push.

### 2.3 Nachteile/Alternativen (benannt)

- **`gh release upload --clobber`**: idempotent, überschreibt gleichnamige Assets — Standard für rolling `latest-main`. Nachteil: alte versionierte dev-Assets akkumulieren (Wachstum; M2-Problem, Roadmap kennt es). → Mit Cleanup-Step (2.2) entschärft.
- **Cleanup-Nachteil:** Ein Client, der zwischen Manifest-Fetch und Asset-Download läuft, kann einen 404 auf ein gerade gelöschtes altes Asset bekommen → `UpdateError::HttpStatus`, beim nächsten Lauf selbstheilend (Rolling-Kanal akzeptiert das; im Code-Review-Doc notieren).
- **Alternative „eigenes Release pro Commit"**: saubere History, aber Release-Spam + `releases/latest`-Kollisionen → verworfen.
- **Alternative „nur stabile Namen + SHA256SUMS in latest-main, keine versionierten dev-Assets"**: weniger Müll, aber keine SHA-Provenienz pro Build → verworfen (SHA im Namen ist der Kern des Konzepts).

---

## 3. Autoupdater-Kanal-Logik (`src/autoupdate/`)

### 3.1 Kanaldefinition

- **Kanal eines Builds = Eigenschaft seiner Version:** Version mit pre-release (`-dev+`) → **Dev-Build**; ohne → **Release-Build**.
- **Kanal einer Quelle = Basis-URL:** Dev → `latest-main` (Default bleibt `DEFAULT_BASE_URL`); Release → **`https://github.com/momokli/momos-music-manager/releases/latest`** (neue Konstante `DEFAULT_RELEASE_BASE_URL`; GitHub leitet auf das neueste Non-Prerelease-Release um → zeigt nach dem v1.1.0-Repair auf v1.1.0; reqwest folgt Redirects standardmäßig). Explizite `MOMOS_AUTOUPDATE_BASE_URL`/`[autoupdate] base_url` überschreibt weiterhin (dokumentieren).

### 3.2 Konkrete Änderungen in `src/autoupdate/verify.rs`

1. **`UpdateSettings::from_config`:** `current_version` einmal parsen; Parse-Fehler → `UpdateError::CurrentVersion` (fail fast). Default-Basis-URL abhängig vom Kanal: `current.pre.is_empty() ? DEFAULT_RELEASE_BASE_URL : DEFAULT_BASE_URL`.
2. **`manifest.rs::version_for`** (Parsing-Fix): Statt `split_once('-')` am Anfang: Suffix ab dem **letzten** Vorkommen von `-<os_arch>.` abschneiden, Prefix `momos-music-manager-` strippen, Rest = vollständige Versionszeichenkette (`1.1.0-dev+abc1234`), `Version::parse`. Die `-latest-`-Entry scheitert weiterhin am Parse („latest") und wird übersprungen.
3. **Artefakt-Auflösung:** `stable_artifact_name()` nicht mehr als Lookup verwenden; stattdessen aus der geparsten Version ableiten: `format!("momos-music-manager-{}-{}.{}", latest, os_arch, ext)` (existiert auf beiden Kanälen; `Version::to_string()` erhält build-metadata). Der stabile Name bleibt nur noch für den Download-Pfad der Landing Page relevant (CI-seitig).
4. **Kanal-Guards** (in `fetch_update_info`, nach Manifest-Verifikation, vor dem Versionsvergleich):
   - `cur_dev != new_dev` (dev-Build trifft Release-Version oder umgekehrt) → **ablehnen**: `Err(UpdateError::ChannelMismatch { current_version, available_version })` (neue Error-Variante). Dev-Builds updaten **nie** automatisch auf Stable-Releases und umgekehrt.
5. **Vergleich** (ersetzt `latest <= current → None`):
   - `latest > current` → `UpdateAvailable`
   - `latest == current` (precedence) **und** `cur_dev` **und** `latest.to_string() != settings.current_version` → `UpdateAvailable` (neuer SHA im rolling Kanal; build-metadata ist precedence-ignorant, daher String-Vergleich)
   - sonst → `None` (`UpToDate`)
6. **Status:** neue Variante `UpdateStatus::ChannelMismatch`; `check()` mappt `UpdateError::ChannelMismatch` darauf; `apply()` propagiert den Fehler.
7. **`src/main.rs` CLI:** neue Match-Arms in `update check` (Hinweis: „Dev-Build – Stable-Release bitte manuell installieren" bzw. „Release-Build – nur semver-Releases werden automatisch installiert") und `update apply`-Fehlerausgabe. `update status` zeigt weiterhin Kanal = `base_url`.

### 3.3 Tests (bestehende in `verify.rs`/`manifest.rs` anpassen + neu)

- `version_for` parst `momos-music-manager-1.1.0-dev+abc1234-linux-x64.tar.gz` → `1.1.0-dev+abc1234`.
- Dev-Client (current `1.1.0-dev+abc1234`) + Manifest-Version `1.1.0-dev+def5678` → `UpdateAvailable` (rolling).
- Dev-Client + Manifest-Version `1.1.0` (Release) → `ChannelMismatch`.
- Release-Client (`1.1.0`) + Manifest-Version `1.1.0-dev+…` → `ChannelMismatch`.
- Release-Client `1.0.1` + Manifest `1.1.0` → `UpdateAvailable`, `artifact_name == "momos-music-manager-1.1.0-linux-x64.tar.gz"`.
- Bestehende Tests: `artifact_name`-Erwartungen von `-latest-` auf versionierte Namen umstellen (Mock-Manifeste enthalten versionierte Zeilen bereits).

---

## 4. Doku

1. **Neu `docs/versioning.md`:** Schema-Tabellen (1.1/1.2), Kanalmodell (3.1), Asset-Namen (2.1).
2. **Release-Runbook (in `docs/versioning.md`):**
   - `CHANGELOG.md` pflegen; `Cargo.toml` auf Release-Version heben + Commit + Push auf main;
   - `site/index.html` Footer/Hero-Version anheben (gleicher Commit);
   - Tag erstellen: `git tag v1.2.0 && git push origin v1.2.0` → `build-all.yml` baut + publiziert (Assets `momos-music-manager-1.2.0-*`, signiertes Manifest);
   - Verifikation: `gh release view v1.2.0 --json assets -q '.assets[].name'`, `gh release view v1.2.0 -p SHA256SUMS.minisig` vorhanden.
   - **Warnung:** Nie `build-all.yml` per `workflow_dispatch` auf einem alten Tag ausführen (alter Code + neuer Workflow → Assets `1.2.0` mit Binaries, die `1.0.1` melden = Fehllabeling). Für Alt-Tag-Repairs existiert `repair-release.yml` (Abschnitt 5).
3. **Dev-Build-Erklärung (in `docs/versioning.md`):** rolling main → `latest-main`, `-dev+<sha8>`, stabile Namen, Autoupdater verweigert Kanalwechsel.
4. **README.md:**
   - „Naming"-Abschnitt (~Z.141): Schema + `docs/versioning.md`-Link.
   - Autoupdater-Abschnitt (~Z.237): Kanäle erklären (Dev → `latest-main`, Release → `releases/latest`), Kanal-Guards.
   - curl-Beispiele (Z.166–169): hardcodierte `1.0.1`-Namen → stabile `momos-music-manager-latest-linux-x64.tar.gz`.
   - `update status`-Ausgabe erwähnen (zeigt Kanal).
5. **`.env.example`:** Kommentar ergänzen — Default-Basis-URL ist kanalabhängig (Dev: `latest-main`, Release: `releases/latest`).
6. **`CHANGELOG.md`:** Unreleased-Eintrag (Versioning-Schema, Kanäle, CI).
7. **`site/index.html`:** `v1.0.1` → `v1.1.0` (Hero Z.47, Footer Z.256).

---

## 5. v1.1.0-Nachbesserung

### 5.1 Analyse

- `v1.1.0` (derzeit `releases/latest`, non-prerelease) trägt Assets `momos-music-manager-1.0.1-*` + `SHA256SUMS` (1.0.1-Namen, signiert). Ursache: Build las die Version aus `Cargo.toml` (1.0.1).
- **Auswirkung:** Jeder Release-Kanal-Updater (nach 3.1) würde auf `releases/latest` die Version `1.0.1` aus dem Manifest lesen → „up to date" trotz v1.1.0-Tag; manuelle Downloads sind verwirrend benannt. Funktional selbstheilend ab dem nächsten Release (v1.1.1/v1.2.0), aber das Flaggschiff-Release bleibt inkonsistent.
- **Grenze der Nachbesserung:** Die *eingebettete* Versionszeichenkette (`--version`, `/api/version`) der v1.1.0-Binaries bleibt `1.0.1` — sie wurde beim Tag-Build mit altem Code (Cargo.toml 1.0.1, kein MMM_VERSION) gebacken und kann ohne Rebuild nicht korrigiert werden. Ein Rebuild vom Tag ist **nicht** möglich (alter build.rs ignoriert MMM_VERSION → wieder 1.0.1). Das behebt sich erst mit dem nächsten Release. → Asset-Repair ist Kosmetik/Konsistenz, keine inhaltliche Korrektur; das wird im Plan und in der Doku explizit so kommuniziert.

### 5.2 Empfehlung: **Jetzt reparieren** (per CI-Workflow)

Begründung: Der Reparatur-Aufwand ist klein und einmalig; er macht `v1.1.0` sofort korrekt für die neue Release-Kanal-Logik (`version_for` → `1.1.0`, Artefakt `momos-music-manager-1.1.0-*` vorhanden) und räumt die verwirrendste Stelle für die ersten Nutzer auf. Die Alternative „nächstem Release überlassen" ist vertretbar, lässt aber das aktuelle `releases/latest` inkonsistent — und genau dahin zeigt der Release-Kanal.

**Neue Datei `.github/workflows/repair-release.yml`** (workflow_dispatch, Input `tag` required, z. B. `v1.1.0`; läuft vom Default-Branch → hat den neuen Code + `scripts/minisign.pub`):

1. `actions/checkout@v4` (main, nur für `scripts/minisign.pub`).
2. minisign installieren (pinned, wie in `build-all.yml`).
3. Schritt (env: `GITHUB_TOKEN`, `MINISIGN_SECRET_KEY`):
   - Stale-Liste vor dem Download merken: `gh release view "$TAG" --json assets -q '.assets[].name' | grep '^momos-music-manager-1\.0\.1' > /tmp/stale.txt`
   - `gh release download "$TAG" -p 'momos-music-manager-*' -D repair/` (versionierte Assets, ohne stabile Namen)
   - Umbenennen: je Asset `momos-music-manager-1.0.1-*` → `momos-music-manager-${TAG#v}-*` (sed über die Versionsstelle; `.sha256`-Dateien mit umbenennen oder verwerfen)
   - Per-Asset-Checksums neu erzeugen: `sha256sum momos-music-manager-1.1.0-*.<ext> > <asset>.sha256`
   - Aggregat neu erzeugen: `cat momos-music-manager-1.1.0-*.sha256 > SHA256SUMS`
   - Signieren (identischer Ablauf wie in `build-all.yml`): `base64 -d` → `minisign -S -s … -m SHA256SUMS` → Self-Verify `minisign -V -p scripts/minisign.pub` → Schlüsseldatei löschen. Hard-Fail ohne Secret.
   - Upload: `gh release upload "$TAG" momos-music-manager-1.1.0-* SHA256SUMS SHA256SUMS.minisig --clobber`
   - Löschen: `while read -r a; do gh release delete-asset "$TAG" "$a" --yes; done < /tmp/stale.txt` (alle alten `1.0.1`-Assets inkl. `.sha256`).
4. Verifikation: `gh release view v1.1.0 --json assets -q '.assets[].name'` → nur noch `momos-music-manager-1.1.0-*` + SHA256SUMS(+.minisig); `minisign -V -p scripts/minisign.pub -m SHA256SUMS` ok.

**Reiner `gh`-Weg ohne CI (dokumentieren, nicht empfehlen):** Download → umbenennen → `gh release upload v1.1.0 … --clobber` → `gh release delete-asset … --yes`. Nachteil: `SHA256SUMS`/`.minisig` bleiben auf den alten Namen → Release-Kanal-Updater bekommt `ArtifactNotFound` für `momos-music-manager-1.1.0-*` (Manifest nennt 1.0.1) und kann v1.1.0 nicht als Update anbieten, bis das nächste Release erscheint. → Nur die CI-Variante stellt auch das Manifest konsistent.

**Nicht tun:** `build-all.yml` auf dem v1.1.0-Tag re-runnen (siehe 4.2-Warnung) — erzeugt entweder wieder 1.0.1-Namen (alter Workflow) oder fehllabelte Assets (neuer Workflow + alter Code).

---

## 6. User Stories / Tasks (Reihenfolge = Abhängigkeiten)

### US1 — Versions-Injektion in den Rust-Build
- **AK:** `MMM_VERSION=9.9.9-dev+abc1234 cargo build` → `--version`/`/api/version` melden `9.9.9-dev+abc1234`; ohne Env → `Cargo.toml`-Version; `scripts/resolve-version.sh` liefert für `refs/tags/v1.2.0` → `1.2.0`, für `refs/heads/main` → `<basis>-dev+<sha8>`, sonst dev-Formel (mit Beispieldurchläufen im Commit dokumentiert).
- **Dateien:** `build.rs`, `Cargo.toml` (version = 1.1.0), `scripts/resolve-version.sh` (neu), `src/main.rs` (2 Stellen), `src/api/infrastructure.rs`, `src/telemetry/mod.rs`
- **LoC:** ~35

### US2 — Packaging-Skripte versionieren aus MMM_VERSION
- **AK:** `MMM_VERSION=1.1.0-dev+abc1234 ./scripts/package-linux.sh` → `target/momos-music-manager-1.1.0-dev+abc1234-linux-x64.tar.gz` + `.sha256`; `VERSION`-Datei im Archiv enthält die volle Versionszeichenkette; analog ps1 (zip) und macos (DMG-Name + Info.plist); Fallback ohne Env = `cargo metadata`.
- **Dateien:** `scripts/package-linux.sh`, `scripts/package-windows.ps1`, `scripts/package-macos.sh`
- **LoC:** ~15

### US3 — Manifest-Parsing für dev-Versionen
- **AK:** `version_for("linux-x64")` parst `momos-music-manager-1.1.0-dev+abc1234-linux-x64.tar.gz` → `1.1.0-dev+abc1234`; `-latest-`-Einträge weiterhin übersprungen; bestehende Tests grün + neuer Unit-Test.
- **Dateien:** `src/autoupdate/manifest.rs`
- **LoC:** ~10

### US4 — Autoupdater-Kanäle
- **AK:** `from_config` wählt Default-Basis-URL kanalabhängig (dev → `latest-main`, release → `releases/latest`); Artefakt-Auflösung über versionierte Namen; Guards (dev↔release → `ChannelMismatch`); rolling-Vergleich (precedence-gleich + String-Diff → UpdateAvailable); neue CLI-Ausgaben in `update check`/`apply`; alle Unit-Tests (neu + angepasst) grün; `cargo test` gesamt grün.
- **Dateien:** `src/autoupdate/verify.rs`, `src/autoupdate/mod.rs`, `src/autoupdate/platform.rs` (nur falls Signaturberührung), `src/main.rs` (Match-Arms), `src/config.rs` (Doku-Kommentar Default), `.env.example`
- **LoC:** ~90

### US5 — CI build-all.yml
- **AK:** Build-Jobs exportieren `MMM_VERSION`; macos-Rename nutzt Env; Publish-Job nutzt `resolve-version.sh` (kein sed); main → versionierte dev-Assets + stabile Namen + signiertes Manifest mit `--clobber` + Cleanup (stale versionierte + `Momo.s-*`-Legacy); Tag → semver-Assets ohne stabile Namen; Dispatch auf main publiziert; minisign-Hard-Fail unverändert. Verifikation: manueller Dispatch auf `feat/versioning-concept`-Testlauf (oder dry-run `resolve-version.sh` + `actionlint`) + **erster echter main-Push publiziert signiertes `latest-main`-Manifest** (behebt das Live-Problem aus Abschnitt 0; Asset-Liste enthält keine `Momo.s-*`-Legacy mehr).
- **Dateien:** `.github/workflows/build-all.yml`
- **LoC:** ~60

### US6 — v1.1.0-Repair
- **AK:** `repair-release.yml` existiert; Dispatch mit `tag=v1.1.0` benennt Assets auf `momos-music-manager-1.1.0-*` um, regeneriert `.sha256` + `SHA256SUMS`, signiert (Secret) und löscht alle `1.0.1`-Assets; `gh release view v1.1.0` zeigt nur 1.1.0-Namen; `minisign -V` ok.
- **Dateien:** `.github/workflows/repair-release.yml` (neu)
- **LoC:** ~70

### US7 — Doku
- **AK:** `docs/versioning.md` (Schema, Kanäle, Release-Runbook inkl. Cargo.toml-/site-Bump + Tag-Befehlen, Dev-Erklärung, Repair-Anleitung, Warnung Dispatch-auf-Tag); README-Naming-/Autoupdater-Abschnitte + curl-Beispiele auf `-latest-`-Namen; `.env.example`-Kommentar; CHANGELOG-Eintrag; `site/index.html` zeigt `v1.1.0`.
- **Dateien:** `docs/versioning.md` (neu), `README.md`, `.env.example`, `CHANGELOG.md`, `site/index.html`
- **LoC:** ~120 (davon ~80 Doku-Text)

---

## Kernentscheidungen (Kurzfassung)

1. **Tag = Wahrheit für Releases** (`v1.1.0` → `1.1.0`); `Cargo.toml` wird per Runbook lose synchron gehalten und ist die dev-Basis.
2. **Dev = `<Cargo.toml>-dev+<sha8>`** (SemVer-valid; `-dev+` = pre-release + build-metadata).
3. **Mechanik:** `build.rs` injiziert `MMM_VERSION` (env, Fallback Cargo.toml); alle `env!("CARGO_PKG_VERSION")`-Stellen umgestellt; `scripts/resolve-version.sh` als einzige CI-Versionsquelle; Packaging-Skripte lesen `MMM_VERSION`.
4. **Kanäle:** Dev → `latest-main`, Release → `releases/latest`; Manifest/Artefakt über versionierte Namen; Kanal-Guards verhindern dev↔release-Autoupdates; rolling-Vergleich über String-Diff bei gleicher Precedence.
5. **v1.1.0:** jetzt reparieren via neuem `repair-release.yml` (umbenennen + neu signieren + alte Assets löschen); eingebettete Version bleibt bis zum nächsten Release 1.0.1 (dokumentierte Grenze).
6. **Kein Docker/Trunk, keine Berührung von PRs #1/#2 und M-Issues #8–#12; `pages.yml` unverändert.**
