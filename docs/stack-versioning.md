# Versioning & Build Traceability (music stack)

> Every binary and service in the stack reports **which git commit it was built
> from**, so a running deploy is always traceable. Applies to **music-hub**,
> **music-api** and **kellner**. (The old single-app scheme is in the MMM-era
> [`docs/versioning.md`](versioning.md); this file supersedes it for the stack.)

## Version string

| Build | Source | Example |
| --- | --- | --- |
| **Release** | git tag `v*` on `main` (tag without leading `v`) | `v1.16.0` -> `1.16.0` |
| **Dev / rolling** | `<Cargo.toml version>-dev+<sha8>` | `0.1.0-dev+602650f8` |

Additional raw metadata is always embedded:

| Field | Meaning |
| --- | --- |
| `VERSION` | the string above (env override `<CRATE>_VERSION` wins) |
| `GIT_SHA` | `git rev-parse --short=8 HEAD` (or `unknown`) |
| `GIT_DESCRIBE` | `git describe --tags --always --dirty` (or `unknown`) |
| `LONG_VERSION` | `VERSION (GIT_DESCRIBE)` - the long `--version` form |

## How it works

Each crate has a `build.rs` that captures git state **at build time**:

    build.rs -> cargo:rustc-env=BUILD_VERSION / BUILD_GIT_SHA / BUILD_GIT_DESCRIBE
             -> env!("BUILD_VERSION") etc. in the crate

Precedence: `<CRATE>_VERSION` env (release/CI) -> `GIT_SHA`/`GIT_DESCRIBE` env ->
`git rev-parse` / `git describe` in the crate dir -> `CARGO_PKG_VERSION` / `unknown`.

**It never fails without git.** The deploy ships a `tar` with **no `.git`**, so the
deploy loop passes `GIT_SHA` / `GIT_DESCRIBE` explicitly (see
[`music-hub/AGENT.md`](../music-hub/AGENT.md) section Deploy loop); CI/tag builds
set `<CRATE>_VERSION` from the tag.

## Where to see it

| Surface | music-hub | music-api | kellner |
| --- | --- | --- | --- |
| CLI | `music-hub --version` (`-V` short, `--version` long) | `music-api --version` | `kellner --version` |
| HTTP | `GET /api/hub/health` -> `{version,gitSha,describe}` | `GET /health` -> same | - |
| Web UI | footer shows `music-hub <VERSION>` | - | - |
| Logs | startup `starting music-hub version=... git_sha=...` | startup `music-api listening on ... version=...` | - |
| Code | `music_hub::{VERSION,GIT_SHA,GIT_DESCRIBE,LONG_VERSION}` | `music_api::{VERSION,GIT_SHA,GIT_DESCRIBE}` | `env!("BUILD_VERSION")` |

## Releases

The **tag is the truth** for releases; `Cargo.toml` versions are only the dev
basis (kept loosely in sync). A missed bump is harmless - dev builds carry the
`+SHA` and are compared by SHA, not by the Cargo version.

## Tags & SHAs

Git tags are semver on `main` (`git describe` reads e.g. `v1.15.2-180-g602650f`).
The tag names the release; the embedded `GIT_SHA`/`GIT_DESCRIBE` pin the exact
commit of every dev/rolling build.
