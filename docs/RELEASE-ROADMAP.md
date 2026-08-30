# Release & Download Roadmap

Status: **Stand 2026-08-30** · Ziel: **Landing-Page-Downloads für alle Ziel-OS
mit SHA256-Verifikation, schrittweise zu produktionsreifer Verteilung**

Dieses Dokument beschreibt die **iterative** Weiterentwicklung der
Download-/Release-Strategie von Momo's Music Manager. Jeder Milestone ist
**einzeln abarbeitbar**, liefert für sich einen sichtbaren Wert und hat eine
klare Definition of Done (DoD). Es gibt **keine „Big-Bang"-Meilensteine** —
jeder Schritt wird separat geprüft, gemerged und veröffentlicht, bevor der
nächste beginnt.

Ausgangslage (Bestandsaufnahme, 2026-08-30):

- CI ([`build-all.yml`](../.github/workflows/build-all.yml)) baut alle
  **6 Ziel-Targets** grün: Linux x64 + arm64 (tar.gz), Windows x64 + arm64
  (zip), macOS universal (DMG) — plus `.sha256` je Datei und aggregiertem
  `SHA256SUMS`.
- Jeder Push auf `main` publiziert ein rolling **`latest-main`**-Release mit
  allen Artefakten. Tags `v*` erzeugen eigene Releases.
- Die Landing-Page ([`site/`](../site/)) war bislang **macOS-zentriert**
  (ein DMG-Button, Gatekeeper-Hinweis) — Linux/Windows-Downloads fehlten.
- Detaillierte Plattform-Matrix: [`docs/PLATFORM-SUPPORT.md`](PLATFORM-SUPPORT.md).

## Überblick

| # | Milestone | Kernnutzen | Aufwand | Status |
|---|-----------|-----------|---------|--------|
| [M1](#m1--download-sektion-für-alle-6-artefakte--sha256) | Download-Sektion für alle 6 Artefakte + SHA256 (rolling) | Besucher laden jede Plattform direkt von der Landing-Page, mit Checksummen-Verifikation | S | ✅ **umgesetzt** (PR #13) |
| [M2](#m2--release-spezifische-seitenassets-pro-tag) | Release-spezifische Seiten/Assets pro Tag (`v*`) | Stabile, versionierte Downloads mit Changelog statt ewig wachsendem Rolling-Release | M | 🟡 offen |
| [M3](#m3--windows-code-signing-smartscreen) | Windows Code-Signing (SmartScreen) | Keine „Unbekannter Herausgeber"-Warnung mehr | M | ❌ offen |
| [M4](#m4--macos-notarization-gatekeeper) | macOS Notarization (Gatekeeper) | Doppelklick-Installation ohne Rechtsklick-Trick | M | ❌ offen |
| [M5](#m5--linux-appimage--flatpak) | Linux AppImage / Flatpak | Desktop-Integration (Icon, .desktop, App-Store) statt nacktem tar.gz | M | 🟡 geplant |
| [M6](#m6--autoupdater-optional) | Autoupdater (optional) | App aktualisiert sich selbst | L | ⏸️ optional |

Legende: ✅ umgesetzt · 🟡 offen/geplant · ❌ offen (blockiert/aufwendig) · ⏸️ optional

---

## M1 — Download-Sektion für alle 6 Artefakte + SHA256

**Ziel:** Die Landing-Page bietet für **alle Ziel-Plattformen** direkte
Download-Buttons aus dem rolling `latest-main`-Release — macOS Universal-DMG,
Windows x64 + arm64, Linux x64 + arm64 — jeweils mit SHA256-Checksummen-Link
und kurzer Verifikations-Anleitung. Kein „weiter zur Release-Seite" mehr nötig.

**Umgesetzt in PR #13** (Branch `feature/site-downloads-roadmap`):

- `site/index.html`: neue Download-Sektion mit 5 Paket-Karten (macOS universal,
  Windows x64/arm64, Linux x64/arm64) + SHA256-Block mit Befehlen für
  macOS/Linux/Windows + Link auf aggregiertes `SHA256SUMS`. Hero-Button
  generalisiert („Download" statt „Download for macOS").
- `site/style.css`: Styles für Plattform-Grid, Paket-Karten und Code-Block.
- `.github/workflows/build-all.yml` (nur Publish-Job): legt **stabile
  Artefakt-Namen** an (`momos-music-manager-latest-<os>-<arch>.<ext>` +
  `Momo-s-Music-Manager-latest.dmg.sha256`), damit die Download-Links der
  Landing-Page **über Versionswechsel hinweg stabil** bleiben; aggregiertes
  `SHA256SUMS` enthält jetzt auch die stabilen Einträge.
- `docs/PLATFORM-SUPPORT.md` + `README.md` + `CHANGELOG.md` aktualisiert.

**Definition of Done (M1):**

- [x] Landing-Page zeigt Download-Buttons für macOS (Universal), Windows
      (x64 + arm64), Linux (x64 + arm64).
- [x] Jeder Button verlinkt ein **stabiles** Artefakt aus `latest-main`.
- [x] Jedes Artefakt hat einen `.sha256`-Link; aggregiertes `SHA256SUMS` verlinkt.
- [x] Verifikations-Anleitung für alle 3 OS-Familien auf der Seite.
- [x] `pages.yml`-Deploy bleibt grün; kein Build-Risiko (nur Publish-Job
      erweitert, Build-Matrix unverändert).

**Bekannte Grenze (bewusst):** Die Links zeigen auf das rolling
`latest-main`-Release. Echte versionierte Releases sind **M2**.

---

## M2 — Release-spezifische Seiten/Assets pro Tag

**Ziel:** Stabile, **versionierte** Releases statt eines ewig wachsenden
Rolling-Releases. Wer eine verlässliche Version will (z. B. für
Produktiv-Setups, Rollbacks, Changelog), bekommt pro `v*`-Tag eine saubere
Release-Seite mit allen 6 Artefakten + SHA256SUMS. Die Landing-Page weist
zusätzlich auf die neueste **stabile** Version hin.

**Hintergrund / Probleme heute:**

- Das `latest-main`-Release sammelt bei jedem Versions-Bump alte versionierte
  Assets an (nur `--clobber` ersetzt Namensgleiches) — der Release wächst.
- Die Landing-Page kennt nur „latest rolling", keine Versionen/Changelog.

**Vorschläge zur Umsetzung (entscheiden + umsetzen):**

1. Aufräum-Regel für `latest-main`: beim Publish alte `momos-music-manager-<ver>-*`
   Assets löschen, sobald sie nicht mehr dem aktuellen VERSION-String entsprechen
   (per `gh release delete-asset` im Publish-Job).
2. Landing-Page: Sektion „Stabile Versionen" mit Link auf
   `releases/latest` bzw. den neuesten `v*`-Tag (oder Versions-Dropdown mit
   den letzten N Tags via kleinem JSON-Endpunkt/`release`-Link).
3. `v*`-Tags bereits erzeugt von `build-all.yml` (Release + alle Assets +
   `--generate-notes`). Prüfen, ob die Notizen die Plattform-Matrix + SHA256
   enthalten.

**Definition of Done (M2):**

- [ ] Ein `v*`-Tag erzeugt ein Release mit allen 6 Artefakten + `SHA256SUMS`.
- [ ] `latest-main` enthält nach dem nächsten Build nur noch die **aktuelle**
      Version (Aufräum-Job oder dokumentierte Regel, verifiziert im CI).
- [ ] Landing-Page verlinkt die neueste stabile Version (mit Changelog-Link).
- [ ] Mit einem echten Test-Tag (`v1.1.0` o. ä.) end-to-end verifiziert.

**Aufwand:** M — überwiegend CI/Publish-Logik + kleiner Site-Input.
**Abhängigkeiten:** keine (baut auf M1 auf).
**Referenz-Issue:** [#8](https://github.com/momokli/momos-music-manager/issues/8)

---

## M3 — Windows Code-Signing (SmartScreen)

**Ziel:** Windows-Nutzer bekommen keine „Windows hat einen unbekannten
Herausgeber blockiert"-Warnung (SmartScreen) mehr bzw. sehen zumindest einen
verifizierten Herausgeber.

**Hintergrund:** Die `.exe` ist heute unsigned → SmartScreen/Defender stufen
sie als „unbekannte App" ein (kein Malware-Befund erwartet, aber abschreckend).

**Optionen:**

1. **Azure Trusted Signing** (Microsoft, deutlich günstiger als klassische
   Zertifikate, CI-freundlich via `trustedsign`/`osslsigncode`).
2. Klassisches **OV/EV-Code-Signing-Zertifikat** (kostenpflichtig, EV baut
   Reputation schneller auf).
3. **Nicht machen** → SmartScreen-Hinweis auf der Landing-Page dokumentieren
   (Fallback, wenn kein Budget).

**Wichtig:** Signieren verändert den Binär-Hash → die `.sha256`/`SHA256SUMS`
müssen **nach** dem Signieren erzeugt werden (Reihenfolge im Publish-Job
anpassen). ZIP signieren: `osslsigncode` kann die gepackte `.exe` vor dem
Zippen signieren — Reihenfolge: sign → zip → checksum.

**Definition of Done (M3):**

- [ ] Signierte `.exe` (x64 + arm64) in `latest-main` und Tag-Releases.
- [ ] `signtool verify`/`osslsigncode verify` läuft im CI grün.
- [ ] Checksummen werden nach dem Signieren erzeugt (SHA256 korrekt).
- [ ] SmartScreen zeigt „Verifizierter Herausgeber" (oder dokumentierte
      Entscheidung für Fallback 3 mit aktualisiertem Landing-Page-Hinweis).
- [ ] `docs/PLATFORM-SUPPORT.md` aktualisiert.

**Aufwand:** M · **Blockiert durch:** Zertifikat/Azure-Konto + Secrets im Repo.
**Referenz-Issue:** [#9](https://github.com/momokli/momos-music-manager/issues/9)

---

## M4 — macOS Notarization (Gatekeeper)

**Ziel:** DMG lässt sich per Doppelklick installieren und starten — ohne den
bisherigen Rechtsklick-Öffnen-Trick. Der Gatekeeper-Hinweis auf der
Landing-Page kann dann entfernt oder stark abgeschwächt werden.

**Hintergrund:** Der DMG ist heute ad-hoc signiert, **nicht notarized**.
Gatekeeper blockiert ihn deshalb beim normalen Öffnen (siehe
[`docs/PLATFORM-SUPPORT.md`](PLATFORM-SUPPORT.md)).

**Umsetzungsskizze:**

1. Apple Developer Program (99 $/Jahr) + Developer ID Application-Zertifikat.
2. CI: Build → `codesign --options runtime` → `dmg` → `notarytool submit`
   → `stapler staple` → Checksumme **nach** dem Notarisieren erzeugen.
3. Secrets: `APPLE_ID`, `APPLE_APP_SPECIFIC_PASSWORD`, `APPLE_TEAM_ID`
   (bzw. `APPLE_API_KEY`/`APPLE_API_ISSUER` für CI-Keys).
4. Verifikation: `spctl -a -vv` → `accepted`.
5. Landing-Page: Gatekeeper-Block ersetzen durch kurzen Hinweis.

**Definition of Done (M4):**

- [ ] DMG ist notarized **und** stapled; `spctl -a -vv` meldet `accepted`.
- [ ] Frische macOS-VM (oder ehrlicher Nutzertest): Doppelklick-Installation
      ohne Rechtsklick.
- [ ] CI-Job signiert/notarisiert reproduzierbar (Secrets dokumentiert).
- [ ] Landing-Page-Hinweis aktualisiert; `docs/PLATFORM-SUPPORT.md` aktualisiert.

**Aufwand:** M · **Blockiert durch:** Apple Developer Account (kostenpflichtig).
**Referenz-Issue:** [#10](https://github.com/momokli/momos-music-manager/issues/10)

---

## M5 — Linux AppImage / Flatpak

**Ziel:** Linux-Nutzer bekommen neben dem tar.gz eine **desktop-integrierte**
Variante: Icon, `.desktop`-Eintrag, ggf. App-Store-Präsenz (Flathub).

**Hintergrund:** Das tar.gz funktioniert (headless Server + `--no-browser`),
hat aber keine Desktop-Integration. Die App hat kein natives GUI — ein
AppImage mit `--no-browser`-Start (oder Web-UI-öffnen) ist trotzdem sinnvoll
für Desktop-Nutzer.

**Optionen:**

1. **AppImage** (einfach): `appimagetool` im CI, `.desktop` + Icon + launcher
   Script (startet Server und öffnet Browser). Landing-Page-Link ergänzen.
2. **Flatpak** (mehr Aufwand): Manifest, Build im CI oder auf Flathub,
   Sandbox-Permissions (Netz, SQLite-DB-Verzeichnis). Höhere Sichtbarkeit,
   mehr Arbeit + Review-Prozess bei Flathub.
3. **Nicht machen** → tar.gz bleibt einzige Linux-Distribution (dokumentierte
   Entscheidung).

**Definition of Done (M5):**

- [ ] Entscheidung dokumentiert (AppImage vs. Flatpak vs. „bleibt tar.gz").
- [ ] Falls AppImage: CI baut es, Smoke-Test startet es headless, Artefakt +
      SHA256 landen in `latest-main` und Tag-Releases, Landing-Page-Link + Icon.
- [ ] `docs/PLATFORM-SUPPORT.md` + README aktualisiert.

**Aufwand:** M · **Abhängigkeiten:** keine (baut auf M1 auf).
**Referenz-Issue:** [#12](https://github.com/momokli/momos-music-manager/issues/12)

---

## M6 — Autoupdater (optional)

**Ziel:** Die App prüft selbst auf neue Versionen (rolling `latest-main` oder
neuester Tag) und aktualisiert sich — mit Signatur-/Checksummen-Verifikation
vor der Installation.

**Hinweis:** In [`docs/PLATFORM-SUPPORT.md`](PLATFORM-SUPPORT.md) aktuell als
„nicht geplant" markiert. Aufwand hoch (pro Plattform: Download, atomarer
Austausch, Rollback, Service-Neustart). Sollte **erst nach M3/M4** (Signing)
begonnen werden, da Update-Pipelines ohne Signaturprüfung ein Sicherheitsrisiko
sind.

**Definition of Done (M6) — nur falls priorisiert:**

- [ ] Update-Check gegen `latest-main` (Version aus Asset-Namen/`SHA256SUMS`).
- [ ] Signatur-/Checksummen-Prüfung **vor** dem Austausch (nutzt M3/M4).
- [ ] Opt-out möglich; Fehler führen zu sauberem Rollback.
- [ ] Dokumentation (README, PLATFORM-SUPPORT).

**Aufwand:** L · **Blockiert durch:** M3/M4 (Signaturprüfung) · **Status:** ⏸️ optional.
**Referenz-Issue:** [#11](https://github.com/momokli/momos-music-manager/issues/11)

---

## Reihenfolge & Regeln

1. **Stück für Stück:** Jeder Milestone ist ein eigener PR (eigener Branch),
   wird einzeln reviewed und gemerged. Keine kombinierten Mega-PRs.
2. **DoD vor Start:** Ein Milestone beginnt erst, wenn Ziel + DoD klar sind
   (stehen in diesem Dokument bzw. im jeweiligen Issue).
3. **Verify first:** Vor jedem Milestone bestätigen, dass der Vorgänger in
   `main` lebt (CI grün, Release/Seite erreichbar).
4. **Ausstieg erlaubt:** M3/M4/M5/M6 dürfen auch bewusst „nicht machen"
   (dokumentierte Entscheidung) — dann wird nur der Landing-Page-Hinweis /
   PLATFORM-SUPPORT angepasst.
