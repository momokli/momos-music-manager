//! Spotify integration module
//!
//! This module contains the Spotify API client and sync worker for mirroring
//! Spotify playlists and tracks to the local database.

pub mod client;
pub mod models;
pub mod replay;
pub mod retry;
pub mod sync_worker;

// Re-export commonly used types
pub use client::SpotifyClient;
// pub use models::*;
// pub use sync_worker::SpotifySyncWorker;
