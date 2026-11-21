//! Audio tag helpers (issue #367, leaves P3-1…P3-4).
//!
//! `tag::comment` writes the resolved backpack Comment string into a file's
//! primary tag via `lofty`. The write is a no-op when the render already
//! matches, so an unchanged track's mtime is preserved (and its canonical
//! content hash is unaffected). The deterministic render semantics live in the
//! `comment-template` crate (issue #664).

pub mod comment;
