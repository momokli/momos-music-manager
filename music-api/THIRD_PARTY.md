# Third-Party Notices

This file records code that was sourced from third-party projects and vendored
into `music-api`.

## Rusteer (Deezer downloader) — MIT License

- **Upstream:** https://github.com/xScherpschutter/Rusteer
- **Pin:** commit `de6561a0d747c51d4e9d1e0db979234d2997f88f` (2026-03-09), v0.1.0
- **License:** MIT — `Copyright (c) 2026 xScherpschutter`
  (full text vendored at `music-api/src/deezer_dl/LICENSE`, included as required
  by the MIT license)
- **Vendored as:** module tree `music-api/src/deezer_dl/` (not a sub-crate).

### Files adopted

| Upstream | Vendored location | Changes |
|---|---|---|
| `src/crypto/mod.rs` | `music-api/src/deezer_dl/crypto.rs` | Trimmed FLAC debug helpers; `crate::` → `crate::deezer_dl::`; in-memory decrypt added |
| `src/api/gateway.rs` | `music-api/src/deezer_dl/gateway.rs` | Trimmed to track download; status-aware; **no ARL field/getter** |
| `src/error.rs` | `music-api/src/deezer_dl/error.rs` | Ported to `thiserror 1`; status-aware variants |

### Upstream files deliberately **not** adopted

- `src/tagging.rs` (would pull `lofty 0.22`; out of scope for M1)
- `src/api/public.rs`, `src/models/*`, `src/converters/mod.rs`, `src/bin/rusteer-cli.rs`
- Album/playlist/batch/streaming paths of `src/rusteer.rs`

### Notes on other licenses

No code was taken from deemix / deemix-remastered (GPL-3.0); those projects are
invoked as external processes only.
