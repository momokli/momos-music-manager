# Progress: Nachhaltiges Versioning-Konzept

Branch: `feat/versioning-concept`
Basis: `main` @ 4eaa1d9 (ci: fail publish when MINISIGN_SECRET_KEY missing (#15))

## Ziel
Dev-Build = rolling main, Commit-SHA eingebacken (z.B. `1.1.0-dev+<sha8>`)
Release-Build = Tag auf main (v*) → semantische Version aus Tag (z.B. `v1.1.0` → `1.1.0`)
+ konsistente Asset-Namen im CI, Versionsanzeige in App/CLI, Autoupdater-Kanal-Logik.

## Kontext (Stand 2026-08-31)
- Cargo.toml version = 1.0.1; Assets heißen `momos-music-manager-1.0.1-*`, obwohl Release v1.1.0 getaggt ist → Diskrepanz, die das Konzept behebt.
- v1.1.0-Tag existiert nur remote; lokal nur v1.0.0 + latest-main.
- Autoupdater PR #14 (M6 v1, minisign-signiert, atomarer Swap) ist gemerged (9d0e29e).
- CI: build-all.yml baut on-tag; rolling latest-main Release existiert (manuell/separat). pages.yml = GitHub Pages.
- Offene PRs #1 (tag roundtrip), #2 (STEMS filter) NICHT anfassen.
- Kein Scope-Creep: nur Versioning/CI/Doku; M-Issues #8-#12 nicht anfassen.

## Stages
- [x] 0. Orchestrator: Repo gelesen (Cargo.toml 1.0.1, build-all.yml + pages.yml, Tags lokal v1.0.0/latest-main, remote +v1.1.0)
- [x] 1. feature-dev-planner: FERTIG (2. Run) — PLAN-versioning-concept.md liegt vor (233 Z., US1–US7, ist-verifiziert, kein Nachdesign nötig). Konsistenz-Check 2026-08-31: alle referenzierten Dateien vorhanden, Cargo.toml=1.0.1, main@4eaa1d9, gh-Auth momokli ok.
- [x] 2. feature-dev-setup: Branch `feat/versioning-concept` erstellt (Basis main @ 4eaa1d9), Build-Baseline PASS (rustup stable 1.98.0 nachinstalliert, System-Rust 1.63 konnte Edition 2024 nicht parsen; cargo metadata → 1.0.1; cargo test --lib: 473 passed / 2 failed — umgebungsbedingt, `metaflac` fehlt)
- [x] 3. feature-dev-developer: Implementierung (build.rs/env, CI, Autoupdater-Kanäle, Doku) — FERTIG 2026-08-31: US1–US7 implementiert & committet (8 Commits auf 6a08fe7: Cargo.lock-Sync, Packaging-MMM_VERSION, Manifest-dev-Parsing, Kanal-Logik, CI resolve-version+Cleanup, repair-release.yml, Doku). cargo build grün; cargo test --lib: 481 passed / 2 failed (bekannte metaflac-Umgebungsfails test_write_comment_to_file_*, dokumentiert, nicht gefixt). resolve-version.sh verifiziert (tag→1.2.0, main/lokal→1.1.0-dev+<sha8>); --version meldet 1.1.0 (Fallback). Working Tree sauber (nur PLAN/progress untracked). Nicht gepusht.
- [ ] 4. feature-dev-verifier: Quality Gate (Diff, Security)
- [ ] 5. feature-dev-tester: Integration/E2E Tests
- [ ] 6. feature-dev-developer: PR erstellen
- [ ] 7. feature-dev-reviewer: Final Review

## Retries
- Verify/Test/Review FAIL → zurück zu Developer (max 2 retries)
