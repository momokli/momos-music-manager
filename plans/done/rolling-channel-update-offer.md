# Plan: rolling-channel-update-offer

**Status**: done
**Branch**: `fix/rolling-channel-update-offer`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: no

---

## 1. Symptom

Ein Client auf dem **rolling**-Kanal meldete „Status: Up to date / Version
v1.11.0", obwohl Stunden zuvor ein neuer `main`-Build publiziert worden war
(`latest-main` → `1.11.0-dev+0f5dde5a`, Run `35593873546`, fertig
2026-09-21T11:42Z). Kanalwechsel + Neustart änderten nichts.

## 2. Ursache (zwei unabhängige Vergleichsfehler)

Ort: `src/autoupdate/verify.rs` → `fetch_update_info` Schritt 4.

```rust
let same_precedence_new_sha = latest == current
    && settings.channel == UpdateChannel::Rolling
    && latest.to_string() != settings.current_version;
if !(latest > current || same_precedence_new_sha) { return Ok(None); }
```

1. **`Ord` vergleicht die Build-Metadata.** SemVer sagt „precedence-equal" für
   `1.11.0-dev+shaA` vs. `1.11.0-dev+shaB` — `cmp_precedence` ignoriert die
   Metadata auch korrekt. Der Updater nutzt aber `latest > current`, also das
   **abgeleitete** `Ord` von `semver::Version`, und das vergleicht die
   Build-Metadata **lexikographisch** mit (`impls.rs:107`, `BuildMetadata: Ord`).
   Ein frisch gepushter Commit, dessen SHA niedriger sortiert als der laufende,
   ergibt damit `latest < current` → „up to date". Reproduziert:
   `1.11.0-dev+0f5dde5a < 1.11.0-dev+f0000000`.
2. **Kanalwechsel vergleicht über die Pre-Release-Grenze.** Ein *Release*-Build
   (`1.11.0`), der den Rolling-Kanal wählt, vergleicht `1.11.0-dev+<sha>` mit
   `1.11.0`: jede Pre-Release-Version steht **unter** ihrem Release, also
   `latest > current == false` → „up to date", obwohl der laufende Build gar
   kein Build des gewählten Kanals ist. Der Kanalwechsel war damit faktisch
   wirkungslos (genau das Symptom „rolling zieht nicht").

## 3. Fix

`fetch_update_info` vergleicht jetzt Basisversion und Zeichenkette getrennt:

| Fall | Ergebnis |
|---|---|
| `latest` hat **ältere** Basisversion (`major.minor.patch`) | kein Update (kein Silent-Downgrade) |
| **neuere** Basisversion | Update |
| gleiche Basis, **Kanalwechsel** (Build gehört nicht zum gewählten Kanal) | Update, sofern andere Versionszeichenkette |
| gleiche Basis, **rolling** | Update, sofern andere Versionszeichenkette (Rolling trackt `main`, der SHA ist kein Ordnungskriterium) |
| gleiche Basis, **release** | reine SemVer-Precedence (`latest > current`) |

Der `ChannelMismatch`-Guard (Schritt 3) bleibt unverändert: er greift nur, wenn
die Quelle den *anderen* Kanal ausliefert als gewählt.

## 4. Tests

`src/autoupdate/verify.rs` (alle grün, `cargo test --lib autoupdate::` → 88):

- `rolling_offers_new_dev_build_whose_sha_sorts_lower` — Regression (a):
  `1.11.0-dev+f0000000` → Update auf `1.11.0-dev+0f5dde5a`.
- `release_build_on_rolling_channel_offers_same_base_dev_build` — Regression (b):
  `1.11.0` auf rolling → Update auf `1.11.0-dev+0f5dde5a`.
- `release_build_on_rolling_channel_ignores_older_base_dev_build` — kein
  Downgrade (`1.11.0` vs. `1.10.0-dev+x`).
- `dev_build_on_rolling_channel_ignores_older_base_dev_build` — Rolling bleibt
  bei älterer Basis „up to date".
- `dev_build_on_release_channel_offers_stable_release` — Spiegelrichtung.
- Bestehende Tests (`dev_build_updates_to_newer_dev_sha`,
  `dev_build_same_sha_is_uptodate`, `explicit_switch_*`) unverändert grün.

## 5. Live-Beweis (echter Feed, Release-Binary)

```
$ MMM_VERSION=1.11.0 MOMOS_AUTOUPDATE_CHANNEL=rolling ./momos-music-manager update check
Update available: v1.11.0-dev+0f5dde5a (current: v1.11.0)
  artifact: momos-music-manager-1.11.0-dev+0f5dde5a-linux-x64.tar.gz
  sha256:   18980dbeb944d6fb2438373c9ac796385d6ff258ca194469835fab43598401f3

$ MMM_VERSION=1.11.0-dev+ffffffff ./momos-music-manager update check
Update available: v1.11.0-dev+0f5dde5a (current: v1.11.0-dev+ffffffff)
```

Mit dem alten Code (gestashte `verify.rs`, gleicher Feed):

```
$ MMM_VERSION=1.11.0 MOMOS_AUTOUPDATE_CHANNEL=rolling ./momos-music-manager update check
Up to date (v1.11.0)
```

## 6. Artefakt-/Download-Pfad (mitgeprüft, kein zweiter Fehler)

Der Fix greift nur, wenn das versionierte Artefakt auch existiert — der
Updater lädt ausschließlich `momos-music-manager-<version>-<os-arch>.<ext>`:

- `SHA256SUMS` auf `latest-main` listet den versionierten macOS-Eintrag
  (`c3be8ab3…  momos-music-manager-1.11.0-dev+0f5dde5a-macos-universal.dmg`)
  zusätzlich zum stabilen `Momo-s-Music-Manager-latest.dmg`.
- `HEAD` auf beide macOS-URLs → **200** (Redirect auf die Release-Assets).
- Receiver-/Health-Pfade unberührt.

Damit ist der Mac-Client nach dem Fix nicht nur „informiert": der Download ist
erreichbar. `update apply` installiert unter macOS per DMG-Self-Install
(`~/Downloads` + `.app`-Ersetzung) und meldet `restartNeeded` — bestehendes
Verhalten, unverändert.

## 7. Nicht enthalten

- Kein Umbau des Auto-Apply-Breakers oder der Intervall-Logik.
- Keine Änderung an der Artefakt-Auflösung (versionierter Name bleibt).
