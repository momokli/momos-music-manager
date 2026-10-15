//! Local detection + change-watch helpers (issue #364).
//!
//! `sync::traktor` locates the Traktor `collection.nml` and reports its mtime so
//! the upload loop can skip unchanged files. `sync::backup` walks the music
//! collection and mirrors it into music-api's object store (#365).
//! `sync::downloads` orders ISRCs at music-api, polls until ready and files the
//! delivered bytes flat into the collection (#366).

pub mod backup;
pub mod backpack;
pub mod downloads;
pub mod traktor;
