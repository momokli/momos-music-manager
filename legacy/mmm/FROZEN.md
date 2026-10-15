# MMM is frozen — status: `frozen / sunset`

> **Momo's Music Manager (MMM)** is **sunset**. This app is feature-frozen and only
> kept alive until the data migration to the music stack completes.

## What this means

- **No new features.** The whole `legacy/mmm/` tree is frozen — every path under it
  except `*.md` docs/changelog is off-limits.
- **Bug fixes only** — and only those needed to keep MMM **running** until the
  migration is done (startup, crashes, data integrity, migration-critical paths).
- **Docs/changelog only** are the one allowed change under `legacy/mmm/` — a `*.md`
  edit (e.g. `CHANGELOG.md`, `FROZEN.md`). Everything else needs the `mmm:unfreeze` label.

## Where the future is

The replacement lives in the music stack — build new functionality there, not in MMM:

- **music-hub** — multi-user web UI + DB (the new home of the library).
- **music-api** — the authority: downloads, object store, BPM/key/metadata catalogue.
- **docs/** — [`docs/ROADMAP.md`](../../docs/ROADMAP.md),
  [`docs/ARCHITECTURE.md`](../../docs/ARCHITECTURE.md) and
  [`docs/legacy/mmm-README.md`](../../docs/legacy/mmm-README.md) for MMM-era context.

## Enforcement

The `.github/workflows/mmm-freeze.yml` **MMM freeze guard** fails any PR that touches
**any** path under `legacy/mmm/`, **except** docs/changelog (`*.md`, e.g. `CHANGELOG.md`,
`FROZEN.md`).

**Escape hatch:** a migration-critical fix can be unblocked by adding the PR label
**`mmm:unfreeze`**. That label is the *only* way past the guard — use it deliberately.
