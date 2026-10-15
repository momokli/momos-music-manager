//! Local detection + change-watch helpers (issue #364).
//!
//! `sync::traktor` locates the Traktor `collection.nml` and reports its mtime so
//! the upload loop can skip unchanged files.

pub mod traktor;
