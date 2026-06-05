//! Database layer — domain-specific query modules.
//!
//! Types in `types.rs`. Queries in per-domain files.
//! `pub use` re-exports ensure `crate::db::*` is backward compatible
//! — callers still write `crate::db::get_files` regardless of
//! which sub-module it lives in.

pub mod connection;
pub mod files;
pub mod folders;
pub mod playlists;
pub mod schema;
pub mod storage;
pub mod tags;
pub mod tracks;
pub mod types;

// Re-export everything so crate::db::* remains backward compatible.
pub use connection::*;
pub use files::*;
pub use folders::*;
pub use playlists::*;
pub use schema::*;
pub use storage::*;
pub use tags::*;
pub use tracks::*;
pub use types::*;
