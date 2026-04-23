//! Spotify integration module
//!
//! This module contains the Spotify API client and sync worker for mirroring
//! Spotify playlists and tracks to the local database.

pub mod client;
pub mod models;
pub mod sync_worker;

// Re-export commonly used types
#[allow(unused_imports)]
pub use client::SpotifyClient;
#[allow(unused_imports)]
pub use models::*;
#[allow(unused_imports)]
pub use sync_worker::SpotifySyncWorker;
