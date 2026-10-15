//! Audio tag helpers (issue #367, leaves P3-1…P3-4).
//!
//! `tag::comment` renders the resolved backpack metadata into a deterministic
//! Comment string and writes it into a file's primary tag via `lofty`. The
//! write is a no-op when the render already matches, so an unchanged track's
//! mtime is preserved (and its canonical content hash is unaffected).

pub mod comment;
