//! Deemix integration module
//!
//! HTTP client for the deemix-pyweb web API (default port 6595).
//! Deemix is web-UI-only: ARL + host are stored in the `service_config` table.
//! Cookie-based auth with auto-re-auth on 401.

pub mod cli;
pub mod client;
pub mod models;

// Re-export commonly used types
pub use client::DeemixClient;
pub use models::*;
