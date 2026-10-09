//! MMM Hub — multi-user Spotify ingest + exploration over one shared SQLite DB.
//!
//! The crate is exposed as a library so integration tests (`tests/`) can build
//! the router against a temporary DB. The binary (`main.rs`) is a thin CLI on
//! top of it.

pub mod analyze;
pub mod analyzer;
pub mod api;
pub mod audio;
pub mod config;
pub mod cosine;
pub mod db;
pub mod digging;
pub mod features;
pub mod freqblog;
pub mod genres;
pub mod ingest;
pub mod lastfm;
pub mod mmm_import;
pub mod music_api;
pub mod pages;
pub mod settings;
pub mod similar;
pub mod spotify;
pub mod tags;
pub mod ui;
pub mod web;
pub mod worker;
