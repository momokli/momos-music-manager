# Progress — Issue #363: hub_devices registry (Hub side)

- Repo: momokli/momos-music-manager
- Base: main @ 92c580a (docs(kellner): lock §10 decisions in ADR-081 (#361) #450)
- Branch: feat/363-hub-devices-registry
- Milestone: kellner-0.1.0

## Gate status
- #361 (ADR Kellner architecture) → **CLOSED** (ADR-081 merged as #450). §10.2 resolved:
  Hub-side `hub_devices` device-token registry. Blocker from earlier comment is cleared.

## Scope (plan §3 hub-side + issue #363)
Hub-side `music-hub` crate (NOT `kellner/`):
- Migration `051_hub_devices.sql` — table `hub_devices`
- Endpoints: `POST /api/hub/devices` (register/re-pair → per-device token),
  `GET /api/hub/devices` (list user's devices), `DELETE /api/hub/devices/{id}` (revoke).
- Pairing approval page `GET /kellner/anheuern` (logged-in user approves → bind to user).
- Token stored hashed (`token_hash`), 040?/0600 semantics; token invalidated after binding.
- Rust model/queries + tests.

## Plan
- Plan written: `plan-issue-363.md` (6 user stories, write-set, pairing model).

## Baseline (Stage 2 setup)
- Branch: `feat/363-hub-devices-registry` @ `92c580a` (up to date with origin/main); worktree clean,
  nur untracked `plan-issue-363.md` + `progress-feat-363-hub-devices-registry.md`.
- `cargo build` (music-hub) → **EXIT 0** (Finished dev profile in 31.13s; 1 dead_code-Warnung
  in `src/web.rs` TrackPage — vorbestehend, nicht durch #363 verursacht).
- `cargo test -p music-hub --no-run` → **EXIT 0** (alle Test-Harness-Binaries kompiliert).
- Migrationen: letzte ist `050_track_features_metadata_check.sql` → **nächste freie Nummer = 051**;
  `051_hub_devices.sql` existiert noch NICHT (erwartet).
- Hinweis: `music-hub` ist eigenständige Crate (kein Workspace am Repo-Root) → cargo-Kommandos
  aus `music-hub/` ausführen.

## Stage log
- [x] 1 planner (plan-issue-363.md)
- [x] 2 setup — Branch/Baseline grün
- [x] 3 developer — Stories 1–6 implementiert; `cargo build` EXIT 0,
  `cargo test -p music-hub` EXIT 0 (alle Suites grün, u. a. `tests/devices.rs` 9 Tests grün)
- [ ] 4 verifier
- [ ] 5 tester
- [ ] 6 developer (PR)
- [ ] 7 reviewer
