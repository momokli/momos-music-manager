# Progress: feature/linux-support

## Ziel
Linux-Support (Build, Paketierung, CI) + Plattform-Roadmap für alle 6 Targets
(Linux x64/arm64, Windows x64/arm64, macOS Intel/ARM).

## Status — ALLES FERTIG ✅

- [x] 1. Baseline: `cargo build --release` auf Linux grün (nur dead-code warnings)
- [x] 2. TLS auf rustls (reqwest/hf-hub/rspotify) — kein OpenSSL-System-Dep; soundcloud-rs vendored (Patch via [patch.crates-io], reqwest default-features aus)
- [x] 3. sqlx `sqlite` ist in 0.8 bereits bundled — keine Aenderung noetig
- [x] 4. scripts/package-linux.sh (tar.gz + SHA256SUMS + systemd-Unit + INSTALL.txt)
- [x] 5. scripts/package-windows.ps1 (zip + sha256, x64/arm64)
- [x] 6. .github/workflows/build-all.yml (Matrix + Publish; ersetzt main-build.yml + release.yml)
- [x] 7. docs/PLATFORM-SUPPORT.md
- [x] 8. README Linux/Windows-Sektionen, CHANGELOG
- [x] 9. src/main.rs: create_db_pool legt DB-Datei + Parent-Dir an (frischer Start überall)
- [x] 10. cargo test: 783/783 grün (metaflac via lokalem .deb, kein root nötig)
- [x] 11. Server-Smoke headless: lokaler Build + CI-Artefakt booten, /api/storage/status → 200
- [x] 12. CI: alle 5 Build-Jobs grün auf main; Publish-Job erfolgreich; Artefakte im latest-main-Release
- [x] 13. Linux-Jobs auf ubuntu-22.04 (glibc 2.35) — portabler; Cache pro Runner-Image gescoped

## CI-Fixes unterwegs
- g++-aarch64-linux-gnu für esaxx-rs (C++ Cross-Build)
- CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc (rust-lld a53-Bug)
- Publish VERSION-Parsing aus Dateiname (grep auf Binary lieferte leer)
- rust-cache per Runner-Image scopen (ubuntu-22.04 vs ubuntu-latest E0463-Mix)

## Hinweis
- Im Repo liegt ein alter Stash von feature/idle-energy-poller (Aug 24) — unangetastet.
- Kein fremder Agent aktiv; Stash-Pop-Konfusion war meinerseits, sauber zurückgebaut.
