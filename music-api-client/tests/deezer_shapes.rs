//! Pure `serde_json` shape tests for the Deezer/ISRC types — no network.
//!
//! These pin the exact JSON the `music-api` service sends (camelCase, nullable
//! `seconds`, optional `isrc`/`durationMs`) so a field rename cannot slip in.

use music_api_client::{DeezerAlbum, DeezerAlbumSummary, DeezerAlbumTrack, DeezerArtistAlbums, IsrcState};

#[test]
fn artist_albums_page_deserialises_camelcase() {
    let page: DeezerArtistAlbums = serde_json::from_str(
        r#"{"albums":[{"albumId":"123","title":"X","seconds":3600,
            "releaseDate":"2024-01-01","recordType":"album","coverUrl":"https://c/1.jpg"}],
            "total":1,"next":null}"#,
    )
    .expect("deserialise artist albums page");
    assert_eq!(page.albums.len(), 1);
    assert_eq!(page.albums[0].album_id, "123");
    assert_eq!(page.albums[0].title, "X");
    assert_eq!(page.albums[0].seconds, Some(3600));
    assert_eq!(page.albums[0].release_date.as_deref(), Some("2024-01-01"));
    assert_eq!(page.albums[0].record_type.as_deref(), Some("album"));
    assert_eq!(page.total, Some(1));
    assert!(page.next.is_none());
}

#[test]
fn album_with_tracks_and_null_seconds() {
    let album: DeezerAlbum = serde_json::from_str(
        r#"{"albumId":"9","title":"A","seconds":null,
            "tracks":[{"isrc":"USAAA0000009","title":"T","durationMs":210000,"position":1}]}"#,
    )
    .expect("deserialise album");
    assert_eq!(album.album_id, "9");
    assert_eq!(album.seconds, None);
    assert_eq!(album.tracks.len(), 1);
    assert_eq!(album.tracks[0].isrc.as_deref(), Some("USAAA0000009"));
    assert_eq!(album.tracks[0].duration_ms, 210000);
    assert_eq!(album.tracks[0].position, Some(1));
}

#[test]
fn album_track_tolerates_missing_isrc_and_duration() {
    let track: DeezerAlbumTrack =
        serde_json::from_str(r#"{"title":"T"}"#).expect("minimal album track");
    assert!(track.isrc.is_none());
    assert_eq!(track.duration_ms, 0);
    assert!(track.position.is_none());
}

#[test]
fn isrc_state_deserialises_camelcase_and_helpers() {
    let st: IsrcState = serde_json::from_str(
        r#"{"isrc":"USAAA0000009","state":"ready","deezerId":"9",
            "sourceFormat":"flac","formats":["flac","mp3-320"]}"#,
    )
    .expect("deserialise isrc state");
    assert_eq!(st.deezer_id.as_deref(), Some("9"));
    assert_eq!(st.source_format.as_deref(), Some("flac"));
    assert!(st.known());
    assert!(st.ready());
    assert_eq!(st.formats_str(), "flac, mp3-320");
}

#[test]
fn isrc_state_error_is_not_known() {
    let st: IsrcState =
        serde_json::from_str(r#"{"isrc":"X","error":"unknown ISRC"}"#).expect("error state");
    assert!(!st.known());
    assert!(!st.ready());
}

#[test]
fn album_summary_seconds_missing_is_none() {
    // Cold catalogue: the field is simply absent (doc from #621).
    let s: DeezerAlbumSummary = serde_json::from_str(r#"{"albumId":"1","title":"X"}"#)
        .expect("cold-catalogue album summary");
    assert_eq!(s.seconds, None);
}
