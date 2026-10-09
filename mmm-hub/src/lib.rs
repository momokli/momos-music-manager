//! MMM Hub — multi-user Spotify ingest + exploration over one shared SQLite DB.
//!
//! The crate is exposed as a library so integration tests (`tests/`) can build
//! the router against a temporary DB. The binary (`main.rs`) is a thin CLI on
//! top of it.

pub mod api;
pub mod config;
pub mod db;
pub mod ingest;
pub mod music_api;
pub mod pages;
pub mod spotify;
pub mod web;
pub mod worker;
