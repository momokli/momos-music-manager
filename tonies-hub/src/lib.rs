//! Tonies-Hub — eigenständiger Dienst für die Eltern-UI (ADR-083, Slice T2).
//!
//! Die Crate trennt `lib` (testbar, ohne Prozessstart) von `main` (CLI).

pub mod auth;
pub mod config;
pub mod db;
pub mod settings;
pub mod templates;
pub mod web;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
