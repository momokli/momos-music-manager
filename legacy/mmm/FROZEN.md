# MMM is frozen — status: `frozen / sunset`

> **Momo's Music Manager (MMM)** is **sunset**. This app is feature-frozen and only
> kept alive until the data migration to the music stack completes.

## What this means

- **No new features.** Do **not** add features to `legacy/mmm/src/`,
  `legacy/mmm/frontend/` or `legacy/mmm/migrations/`.
- **Bug fixes only** — and only those needed to keep MMM **running** until the
  migration is done (startup, crashes, data integrity, migration-critical paths).
- **Docs/changelog** under `legacy/mmm/` (e.g. `CHANGELOG.md`, `*.md`) are allowed.

## Where the future is

The replacement lives in the music stack — build new functionality there, not in MMM:

- **music-hub** — multi-user web UI + DB (the new home of the library).
- **music-api** — the authority: downloads, object store, BPM/key/metadata catalogue.
- **docs/** — [`docs/ROADMAP.md`](../../docs/ROADMAP.md),
  [`docs/ARCHITECTURE.md`](../../docs/ARCHITECTURE.md) and
  [`docs/legacy/mmm-README.md`](../../docs/legacy/mmm-README.md) for MMM-era context.

## Enforcement

The `.github/workflows/mmm-freeze.yml` **MMM freeze guard** fails any PR that touches
the MMM feature surface (`src/`, `frontend/`, `migrations/`, `Cargo.toml`, `Cargo.lock`,
`build.rs`).

**Escape hatch:** a migration-critical fix can be unblocked by adding the PR label
**`mmm:unfreeze`**. That label is the *only* way past the guard — use it deliberately.
