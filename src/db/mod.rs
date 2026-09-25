//! Database layer — domain-specific query modules.
//!
//! Types in `types.rs`. Queries in per-domain files.
//! `pub use` re-exports ensure `crate::db::*` is backward compatible
//! — callers still write `crate::db::get_files` regardless of
//! which sub-module it lives in.

pub mod connection;
pub mod dynamic_bundles;
pub mod files;
pub mod folders;
pub mod music_api;
pub mod playlists;
pub mod schema;
pub mod settings;
pub mod storage;
pub mod tags;
pub mod testing;
pub mod tracks;
pub mod types;

// Re-export everything so crate::db::* remains backward compatible.
pub use connection::*;
pub use dynamic_bundles::*;
pub use files::*;
pub use folders::*;
pub use music_api::*;
pub use playlists::*;
pub use schema::*;
pub use settings::*;
pub use storage::*;
pub use tags::*;
pub use testing::*;
pub use tracks::*;
pub use types::*;

/// Rebuild both materialised tag-resolution tables (`file_resolved_tags` and
/// `track_resolved_tags`). Best-effort — a failure is logged, never propagated.
///
/// Call this after any mutation that changes tag resolution (tag create/rename/
/// delete, category changes, playlist delete) so that filters and comment targets
/// see the new state immediately, instead of waiting for the next background
/// refresh (maintainer cycle / folder scan / global poller).
///
/// `file_resolved_tags.tag_id` has no foreign key onto `tags`, so a deleted tag
/// leaves denormalised rows behind until this rebuild runs.
pub async fn refresh_resolved_tags(pool: &sqlx::Pool<sqlx::Sqlite>) {
    if let Err(e) = refresh_file_resolved_tags(pool).await {
        tracing::warn!("refresh_file_resolved_tags failed: {e:#}");
    }
    if let Err(e) = refresh_track_resolved_tags(pool).await {
        tracing::warn!("refresh_track_resolved_tags failed: {e:#}");
    }
}
