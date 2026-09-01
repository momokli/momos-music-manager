# Branch Sync: feature/files-stem-missing-filter + feat/tag-roundtrip-inbox

Ziel: Beide Feature-Branches auf Stand mit main bringen, PRs konfliktfrei.

## Status

- [x] PR #2 (feature/files-stem-missing-filter) Ausgangslage: MERGEABLE/CLEAN, 11 behind / 5 ahead
- [x] PR #1 (feat/tag-roundtrip-inbox) Ausgangslage: CONFLICTING/DIRTY, 46 behind / 4 ahead
- [x] Branch feature/files-stem-missing-filter rebased auf origin/main (konfliktfrei), gepusht (--force-with-lease)
- [x] Branch feat/tag-roundtrip-inbox rebased auf origin/main (1 Konflikt: Cargo.lock, geloest), gepusht (--force-with-lease)
- [x] Build/Tests: cargo check + cargo test beide Branches
- [x] Verifikation: PR #1 + #2 beide MERGEABLE/CLEAN, alle CI-Builds (5 Plattformen) pass

## Ergebnis

- PR #1: mergeable=MERGEABLE, mergeStateStatus=CLEAN
- PR #2: mergeable=MERGEABLE, mergeStateStatus=CLEAN

## Notizen

- metaflac fehlte lokal (2 Tests schlagen sonst fehl) → Homebrew-Bottle-Wrapper unter ~/.local/bin/metaflac installiert
- Bekannter Flaky-Test storage_backup_rejects_concurrent auf dem STEMS-Branch (pre-existing, auch vor Rebase fehlgeschlagen; De-Flake-Fix existiert nur auf dem tag-roundtrip-Branch); CI fuehrt keine Tests aus, daher kein PR-Blocker
- Rust-Toolchain 1.98 via rustup unter ~/.cargo installiert (System-Cargo 1.65 zu alt fuer edition 2024)
