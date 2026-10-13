//! Spelunke — multi-user Spotify ingest + exploration over one shared SQLite DB.
//!
//! The crate is exposed as a library so integration tests (`tests/`) can build
//! the router against a temporary DB. The binary (`main.rs`) is a thin CLI on
//! top of it.

/// Full build version (git-derived). Set by `build.rs`.
pub const VERSION: &str = env!("BUILD_VERSION");
/// Short git SHA of the build, or `unknown`.
pub const GIT_SHA: &str = env!("BUILD_GIT_SHA");
/// `git describe --tags --always --dirty` of the build, or `unknown`.
pub const GIT_DESCRIBE: &str = env!("BUILD_GIT_DESCRIBE");
/// `VERSION (GIT_DESCRIBE)` — for `--version --verbose` style output.
pub const LONG_VERSION: &str =
    concat!(env!("BUILD_VERSION"), " (", env!("BUILD_GIT_DESCRIBE"), ")");

pub mod albums;
pub mod analyze;
pub mod analyzer;
pub mod api;
pub mod artists;
pub mod audio;
pub mod backpacks;
pub mod clusters;
pub mod config;
pub mod cosine;
pub mod curate;
pub mod db;
pub mod digging;
pub mod features;
pub mod freqblog;
pub mod genres;
pub mod history;
pub mod ingest;
pub mod lastfm;
pub mod listening;
pub mod mmm_import;
pub mod music_api;
pub mod music_ops;
pub mod nowplaying;
pub mod pages;
pub mod recommend;
pub mod scoring;
pub mod settings;
pub mod similar;
pub mod similarity;
pub mod soundcloud;
pub mod spotify;
pub mod table;
pub mod tagdone;
pub mod tagqueue;
pub mod tags;
pub mod tagsearch;
pub mod tagtypes;
pub mod traktor;
pub mod ui;
pub mod viewstate;
pub mod web;
pub mod worker;
pub mod ytdlp;
