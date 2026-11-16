//! Vendored from Rusteer — MIT License.
//! Upstream: https://github.com/xScherpschutter/Rusteer (v0.1.0, commit de6561a0, 2026-03-09).
//! Copyright (c) 2026 xScherpschutter. See ./LICENSE.
//!
//! Adopted: `crypto`, `api/gateway` (trimmed, status-aware), `error`.
//! Everything else in this module is original music-api code.
//!
//! Track download only: no tagging, no album/playlist/batch, no CLI.

mod crypto;
mod error;

pub use error::Error;
