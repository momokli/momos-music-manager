//! Byte-identical parity: the Hub preview and the kellner write path must
//! produce the same Comment bytes (issue #670, slice C4).
//!
//! Both sides render through [`comment_template::render`] with identical
//! [`TrackFacts`]; this test proves that the render output survives the real
//! file round-trip unchanged (no `lofty` mangling / no second truncation) and
//! that it matches the fixed golden string the Hub preview fragment shows.
//!
//! kellner has no library target, so the file write mirrors `tag::comment`
//! (`lofty::read_from_path` → `set_comment` → `save_to_path`) with the
//! dev-dependency `lofty`.

use std::path::PathBuf;

use comment_template::{TrackFacts, render};
use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::tag::Tag;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Write `comment` into the file's primary tag (mirrors `kellner::tag::comment`).
fn write_comment(path: &std::path::Path, comment: &str) {
    let mut tagged = lofty::read_from_path(path).expect("read");
    let tag_type = tagged.primary_tag_type();
    let tag = match tagged.primary_tag_mut() {
        Some(t) => t,
        None => {
            tagged.insert_tag(Tag::new(tag_type));
            tagged.primary_tag_mut().expect("inserted tag")
        }
    };
    if comment.is_empty() {
        tag.remove_comment();
    } else {
        tag.set_comment(comment.to_string());
    }
    tagged
        .save_to_path(path, WriteOptions::default())
        .expect("save");
}

fn facts(
    types: &[&str],
    tags: &[&str],
    source_ids: &[&str],
    bpm: Option<f64>,
    key: Option<&str>,
    energy: Option<f64>,
) -> TrackFacts {
    TrackFacts {
        types: types.iter().map(|s| s.to_string()).collect(),
        tags: tags.iter().map(|s| s.to_string()).collect(),
        source_ids: source_ids.iter().map(|s| s.to_string()).collect(),
        bpm,
        key: key.map(str::to_string),
        energy,
    }
}

/// The rendered Comment written into the file equals the Hub preview bytes
/// (both `comment_template::render`) and the fixed golden string.
#[test]
fn preview_bytes_equal_written_file_bytes() {
    let cases: &[(&str, TrackFacts, &str)] = &[
        (
            "{tags} {bpm} {key} {energy}",
            facts(&[], &["Techno", "Peak Time"], &[], Some(128.0), Some("8A"), Some(7.0)),
            "Techno Peak-Time BPM-128 KEY-8A ENERGY-7",
        ),
        (
            "{tags}",
            facts(&[], &[], &[], None, None, None),
            "",
        ),
        (
            "{key} / {bpm}",
            facts(&[], &["House"], &[], Some(127.5), Some("A-Minor"), None),
            "KEY-A-Minor / BPM-128",
        ),
        (
            "{types} {tags} {source_ids}",
            facts(
                &["M", "P"],
                &["Techno"],
                &["sp:abc", "yt:def"],
                None,
                None,
                None,
            ),
            "[MP] Techno sp:abc yt:def",
        ),
    ];

    for (body, f, expected) in cases {
        // The Hub preview fragment renders through the same function.
        let preview = render(body, f);
        assert_eq!(&preview, expected, "golden mismatch for {body:?}");

        // The kellner write path: same bytes land in (and read back from) the file.
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("parity.flac");
        std::fs::copy(fixture("test.flac"), &dest).expect("copy fixture");
        write_comment(&dest, &preview);

        let read_back = lofty::read_from_path(&dest)
            .expect("read back")
            .primary_tag()
            .and_then(|t| t.comment())
            .map(|c| c.to_string())
            .unwrap_or_default();
        assert_eq!(
            read_back.as_bytes(),
            preview.as_bytes(),
            "file bytes differ from preview for {body:?}"
        );
    }
}

/// A >512-char render already carries the crate's `...` marker; the write path
/// must not cut it a second time.
#[test]
fn truncation_marker_survives_the_write() {
    let body = format!("{{tags}} {}", "x ".repeat(300));
    let rendered = render(&body, &facts(&[], &["Techno"], &[], None, None, None));
    assert_eq!(rendered.chars().count(), 512);
    assert!(rendered.ends_with("..."));

    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("long.mp3");
    std::fs::copy(fixture("test.mp3"), &dest).expect("copy fixture");
    write_comment(&dest, &rendered);

    let read_back = lofty::read_from_path(&dest)
        .expect("read back")
        .primary_tag()
        .and_then(|t| t.comment())
        .map(|c| c.to_string())
        .unwrap_or_default();
    assert_eq!(read_back, rendered, "no second truncation on write");
    assert!(read_back.ends_with("..."));
}
