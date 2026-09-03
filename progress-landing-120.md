# Progress: Landing Page v1.2.0 angleichen (feature/landing-120)

Ausgangslage: Page deployed von 58cb378 (v1.2.0). Versionstexte korrekt.
Ziel: Download-Sektion + Feature-Grid + Galerie an 1.2.0-Stand angleichen,
Workflow-Fix deploy-pages@v4 → @v5.

- [x] Stage 0: Worktree `feature/landing-120` von origin/main (58cb378)
- [x] Stage 1: Analyse site/index.html, Screenshot-Quellen, Workflow
- [x] Stage 2: index.html anpassen (Download 2-Kanal, +3 Features, +2 Galerie)
- [x] Stage 3: Screenshots kopieren (docs/screenshots → site/screenshots)
- [x] Stage 4: pages.yml deploy-pages@v5
- [x] Stage 5: Verifikation
  - 20/20 Download-URLs HTTP 200 (v1.2.0 + latest-main, Assets + .sha256)
  - Tag-/Page-Links (v1.2.0, latest-main, releases, RELEASE-ROADMAP) HTTP 200
  - Alle 8 lokal referenzierten Bilder vorhanden (6 Galerie + 2 Logo)
  - HTML-Tag-Balance ok, YAML parst
- [x] Stage 6: Commit, Push, PR — https://github.com/momokli/momos-music-manager/pull/22
