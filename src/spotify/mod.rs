//! Spotify integration module
//!
//! This module contains the Spotify API client and sync worker for mirroring
//! Spotify playlists and tracks to the local database.

pub mod client;
pub mod models;
pub mod sync_worker;

// Re-export commonly used types
// NOTE: Re-exports currently unused; add back when external consumers need them
// pub use client::SpotifyClient;
// pub use models::*;
// pub use sync_worker::SpotifySyncWorker;
