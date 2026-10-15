# Progress — Issue #364 `feat(kellner): Traktor collection.nml auto-upload`

Repo: momokli/momos-music-manager · Branch: `feature/issue-364-traktor-upload` · Base: `origin/main` @ 35c837e
PR target: `main` · PR body MUST contain `Closes #364`.

## Gate-Check (erledigt — Dispatch ist gültig)

- ADR **#361** CLOSED (2026-10-15); ADR-081 in `docs/DECISIONS.md` locked.
- **#363** `hub_devices` registry CLOSED (PR #451, merge `e40db43`).
- **#362** kellner device pairing CLOSED (PR #452, merge `35c837e`).
- §10.5 Entscheidung: **token-gated `POST /api/hub/traktor/import`** (Bearer device token); hub CLI `import-traktor` bleibt.
- Der frühere Blocker-Kommentar (04:10) ist damit **überholt** — #364 ist nicht mehr decision-gated.

## Scope (aus plans/kellner/issues.md, P1-Leaves P1-1..P1-5)

| Leaf | Was | Write set |
| ---- | --- | --------- |
| P1-1 | `feat(hub): POST /api/hub/traktor/import` (device-token-gated), reuse existing traktor import | neues `music-hub/src/traktor_api.rs`, `music-hub/src/api.rs`, `music-hub/src/lib.rs`, `music-hub/src/devices.rs` (Helfer) |
| P1-2 | `feat(kellner): collection.nml detection + watch` (macOS/Windows default paths, config override) | `kellner/src/sync/{mod,traktor}.rs` (neu), `kellner/src/config.rs` |
| P1-3 | `feat(kellner): Traktor upload client` (Bearer device token, mtime-change trigger, bounded retry) | `kellner/src/client/{mod,hub}.rs` (neu), `kellner/src/state.rs` (neu), `kellner/src/main.rs` |
| P1-4 | `test(hub): traktor import integration tests` | `music-hub/tests/traktor_import.rs` (neu) + fixture |
| P1-5 | `test(kellner): traktor upload integration (mock hub)` | `kellner/tests/traktor_upload.rs` (neu) |

## Ist-Stand im Branch (Base 35c837e)

- `music-hub/src/devices.rs`: `hash_token`, `constant_time_eq`, `Device`, `/api/hub/devices` (POST/GET/DELETE), `/kellner/anheuern`. `token_hash` = Device-Token-Hash nach Bindung. `last_seen_at` laut Migration „touched by device-gated routes (#364+)".
- **Keine device-gated Route existiert** — Bearer-Auth-Schicht muss #364 bauen.
- `music-hub/src/traktor.rs`: `import_str(pool, user_id, xml) -> Result<Stats>` — wiederverwenden.
- `kellner/`: nur `config.rs`, `main.rs`, `pairing.rs`. **Kein** `state.rs`, `sync/`, `client/`; kein sqlx/rusqlite. `TraktorUpload` = Stub.

## Plan (Planner-Stage, verifizierte Signaturen)

**Hub**
- Auth neu: Bearer raw device token → `devices::hash_token` → Zeile in `hub_devices` mit `user_id IS NOT NULL AND revoked_at IS NULL AND paired_at IS NOT NULL`. Neue pub-Helfer in `devices.rs` (`find_bound_by_token_hash`, `touch_last_seen`); bestehende Layer nicht umbauen.
- Neues Modul `music-hub/src/traktor_api.rs` mit `pub fn router(state) -> Router`, Route `POST /api/hub/traktor/import`; in `api.rs::router` mergen, in `lib.rs` `pub mod traktor_api;`.
- Body: raw NML-Bytes (`axum::body::Bytes`), Größen-Cap (~16 MiB → 413), enthält `<NML` sonst 400.
- Wiederverwenden `traktor::import_str`; 200 `{"ok":true,"stats":{…}}`; ungebunden/revoked/unbekannt ⇒ 401.
- CLI `import-traktor` bleibt unangetastet (ADR-081 §10.5).

**kellner**
- `config.rs`: neu `traktor_nml: Option<PathBuf>` (+ env `KELLNER_TRAKTOR_NML`), `traktor_poll_secs: Option<u64>` (+ env `KELLNER_TRAKTOR_POLL_SECS`). Pfad-Override (kein Feature-Toggle) — Leaf P1-2 fordert ihn für Linux.
- Neu `sync/{mod,traktor}.rs`: Detection macOS `~/Music/Native Instruments/Traktor*/collection.nml`, Windows `%USERPROFILE%\Documents\Native Instruments\Traktor*/collection.nml`, sonst config-Override; kein Panic wenn fehlend.
- Neu `client/{mod,hub}.rs`: `POST {hub}/api/hub/traktor/import` mit `Authorization: Bearer <device_token>` (`pairing::auth_header`), bounded retry/backoff.
- Neu `state.rs`: minimaler Sync-State-Store (JSON unter `~/.config/kellner/`, serde_json vorhanden; **kein** sqlx/rusqlite) für `last_upload_mtime` → unverändertes NML ⇒ kein Upload.
- `main.rs`: `TraktorUpload(TraktorUploadArgs { collection_nml, once, interval_secs })` statt Stub.

**Tests**: `music-hub/tests/traktor_import.rs` + Fixture NML (P1-4); `kellner/tests/traktor_upload.rs` Mock-Hub (P1-5).

## Stage-Status (Source of Truth)

| Stage | Status | Ergebnis |
| ----- | ------ | -------- |
| Gate-Check | done | siehe oben |
| 1 Planner | done | Plan oben |
| 2 Setup | done | Branch ok (0/0 vs origin/main, clean außer untracked progress), cargo check hub+kellner EXIT 0, alle Anker vorhanden |
| 3 Developer | done | alle 5 Leaves implementiert; `music-hub cargo test` EXIT 0 (4 neue traktor_import-Tests grün), `kellner cargo test` EXIT 0 (2 neue traktor_upload-Tests grün) |
| 4 Verifier | pending | |
| 5 Tester | pending | |
| 6 PR | pending | |
| 7 Reviewer | pending | |

## Setup-Baseline (2026-10-15, planet, base 35c837e)

- Branch: `feature/issue-364-traktor-upload`; Arbeitsbaum sauber außer untracked `progress-issue-364.md`; `rev-list --left-right --count origin/main...HEAD` = 0/0.
- `cd music-hub && cargo check` → **EXIT 0** (1 pre-existing warning: unused fields `tags`/`me` in web.rs). Log: `/tmp/issue364-hub-check.log`.
- `cd kellner && cargo check` → **EXIT 0** (keine Warnungen). Log: `/tmp/issue364-kellner-check.log`.
- Anker verifiziert (alle vorhanden):
  - `music-hub/src/devices.rs`: `hash_token` (L54), `constant_time_eq` (L67) ✅
  - `music-hub/src/traktor.rs`: `pub async fn import_str` (L59) ✅
  - `music-hub/src/api.rs`: `pub fn router` + `.merge(...)`-Kette (L27+) ✅ (traktor_api-Merge noch NICHT vorhanden — erwartet, P1-1)
  - `music-hub/src/lib.rs`: `traktor_api` **fehlt** — erwartet, wird in P1-1 angelegt
  - `kellner/src/config.rs`: `struct DeviceIdentity` (L54), `device_token` (L57) ✅
  - `kellner/src/pairing.rs`: `pub fn auth_header` (L158), `Bearer` ✅
  - `kellner/src/main.rs`: `enum Command` (L29), `Command::TraktorUpload` (L37, noch Stub via `not_yet`) ✅
- Keine Blocker.

## Regeln

- Bot-Identity: `clanker-git` / `clanker-gh`, **nie** nacktes git/gh.
- Rust-Builds/Tests für `kellner`/`music-hub` auf **planet** (`exec host=node`).
- Ein Issue = ein Branch = ein PR. `Closes #364` im PR-Body.
- Kein Commit auf `main`.
