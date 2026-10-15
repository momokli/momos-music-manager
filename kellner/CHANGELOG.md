# Changelog

All notable changes to `kellner` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the project aims to
use [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0]

First shippable slice of the local, cross-platform music-stack companion.

### Added

- **Pairing** (#362) — `kellner pair` registers this device with the hub and
  persists the device token (`0600`, redacted in logs).
- **Traktor upload** (#364) — `kellner traktor-upload` detects the local
  `collection.nml`, watches it for mtime changes and uploads it to the
  device-token-gated hub route.
- **Backpack upkeep** (#367) — `kellner backpack` exports the active backpacks,
  orders + fetches missing ISRCs flat, writes the deterministic Comment tag and
  soft-prunes tracks that left every active backpack.
- **Backup** (#365) — `kellner backup` walks the library, canonicalises each
  audio file (Comment cleared), and mirrors missing objects to music-api's
  content-addressed object store.
- **Downloads** (#366) — `kellner downloads --isrc …` orders, polls and files
  ISRCs flat into the collection; a status report without arguments.
- **Service + tray** (#368) — `kellner run` (power-aware background loop),
  `kellner service install|uninstall|status` (launchd / systemd user unit,
  Task Scheduler hint on Windows) and an opt-in `tray` menu-bar widget.
- **Packaging + tests** (#370) — the end-to-end harness (`tests/e2e.rs`), the
  install / offline-smoke scripts (`scripts/`), the deploy reference templates
  (`deploy/`) and a CI smoke step.

### Documentation

- README / AGENT / ROADMAP / plan lifecycle closed out for the 0.1.0 slice.

[0.1.0]: https://github.com/momokli/momos-music-manager/releases/tag/kellner-v0.1.0
