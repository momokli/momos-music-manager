# Progress: feature/files-stem-missing-filter

## Status: COMPLETE

## Feature
Filter-Button **„stem missing"** in der `/files`-View (Momo's Music Manager, `momokli/momos-music-manager`):
Zeigt alle Nicht-Stem-Dateien (flac/mp3/wav/…), deren Track **kein** `stem.m4a` mit gleicher ISRC hat.
Kombinierbar mit allen bestehenden Filtern (backup, on-disk, type, tags, PMV, …), inkl. inversem „Has"-Zustand.

## Repo
- https://github.com/momokli/momos-music-manager
- Branch: `feature/files-stem-missing-filter` (von main, f11820d)
- PR: https://github.com/momokli/momos-music-manager/pull/2 (OPEN, MERGEABLE)
- Commit: `7fac506` feat(files): add 'stem missing' filter to /files view

## Implementierung
- **Server-seitiger Filter** (AGENT.md-Prinzip: server-side filtering auf paginierten Seiten).
- DB-Schema: unverändert — Stem-Verknüpfung über `files.isrc` + `file_type='stem.m4a'` (bestehende `has_stem`-Logik).
- `stemMissing` Query-Param (`FilesQuery` + `FilesFilterAll`), SQL in allen 3 Filtern-Buildern
  (`build_files_filter_sql`, `get_files`, `get_files_count`):
  - `true`: `file_type != 'stem.m4a' AND NOT EXISTS (stem.m4a mit gleicher non-empty ISRC)`
  - `false`: `file_type = 'stem.m4a' OR EXISTS (stem.m4a mit gleicher ISRC)` ("Has"-Button)
- Frontend: neue Filter-Zeile „Stem Missing" (All/Missing/Has) in `frontend/pages/files.js`,
  URL-Hash-State (`stemMissing`), server-side params inkl. select-all Filter-Body.

## Geänderte Dateien
- `src/api/files.rs` (Query-Structs + 3 SQL-Builder)
- `frontend/pages/files.js` (Filter-Row, Handler, Params, Hash-Schema/State)
- `tests/api_files.rs` (+4 Integrationstests: Filter, Invers, Kombination, Count)
- `frontend/tests/files.spec.js` (+3 Playwright E2E-Tests)
- `Cargo.toml` (macOS-only Deps `tray-icon`/`objc2*` hinter `cfg(target_os="macos")` → Linux-Build möglich)
- `src/main.rs` (serve: SqliteConnectOptions mit create_if_missing/WAL/busy_timeout 30s — fixt „database is locked" beim Seeding)
- `README.md`, `CHANGELOG.md` (kurz ergänzt)

## Build / Tests
- `cargo build`: OK (Rust 1.98 via rustup; system cargo 1.65 zu alt für edition 2024)
- `cargo test`: 787 passed; einzige Failures: prä-existente flaky Races in `api_storage` (`*_rejects_concurrent`, ~50% pass) — unabhängig von dieser Änderung
- metaflac für 2 Lib-Tests lokal bereitgestellt (Umgebungs-Dependency, kein Code-Change)
- Playwright E2E: 36/36 passed (inkl. 3 neuer Stem-Missing-Tests);
  dazu Chromium-Systemlibs + fontconfig/fonts lokal bereitgestellt (Container ohne root)
