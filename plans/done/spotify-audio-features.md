## Plan: spotify-audio-features

**Status**: done ✅
**Branch**: `feat/spotify-audio-features-comparison`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: yes — `008_spotify_audio_features.sql`

### Description

Fetch Spotify Audio Features (tempo/BPM, key, mode, danceability, energy, valence, acousticness, instrumentalness, liveness, speechiness, loudness, time_signature) during track sync. Convert Spotify's pitch-class+mode notation to Camelot wheel notation for direct comparison with Traktor's key annotations. Add a comparison API endpoint showing Traktor vs Spotify BPM/Key side-by-side with match/mismatch summary.

### Files modified

- `migrations/008_spotify_audio_features.sql` — new migration: 12 audio features columns on `service_tracks`
- `src/spotify/models.rs` — `AudioFeatures` struct, `spotify_key_to_camelot()` conversion, extended `TrackInfo`
- `src/spotify/client.rs` — `get_audio_features_batch()` method (batches of 100)
- `src/spotify/sync_worker.rs` — `update_audio_features_batch()` method; injected audio features fetch after track sync in `sync_tracks_for_playlist`
- `src/db.rs` — `ServiceTrack` extended with 12 audio features columns; `update_track_audio_features()`; `KeyComparisonRow`/`KeyComparisonSummary` types; `get_key_comparison()`
- `src/api.rs` — `GET /api/files/key-comparison?tag=X&limit=N` endpoint
- `src/global_poller.rs`, `src/poller.rs` — added `audio_features: None` to episode `TrackInfo` constructors
- `frontend/pages/key-comparison.js` — new comparison page with tag typeahead, summary cards, sortable table
- `frontend/pages/track-detail.js` — new detail page showing all metadata for a single track
- `frontend/app.js` — registered `"key-comparison"` and `"track-detail"` routes
- `frontend/shared/nav.js` — added "Key Comparison" to TOOLS_ITEMS
- `frontend/style.css` — `.kc-*` and `.detail-*` styles

### Acceptance Criteria

- [x] Spotify sync fetches audio features in batches of 100 after track storage
- [x] `spotify_key_to_camelot()` maps all 24 keys (12 minor + 12 major)
- [x] All 12 audio features stored on `service_tracks` (tempo, key_raw, mode, key_camelot, danceability, energy, valence, acousticness, instrumentalness, liveness, speechiness, loudness, time_signature)
- [x] `GET /api/files/key-comparison?tag=X` returns side-by-side Traktor vs Spotify BPM/Key
- [x] Summary shows match/mismatch counts for BPM (±1 tolerance) and Key (exact Camelot match)
- [x] Works for files with no Spotify link (skipped gracefully)
- [x] Skip audio features in replay mode (cache mode)
- [x] Audio features fetch is non-fatal — tracks are stored even if features fail
- [x] Web UI at `#key-comparison` with tag typeahead, summary cards, sortable table, ✓/✗ indicators
- [x] Backend compiles (`cargo build`)
- [ ] Fresh DB: migrations 001→008 run cleanly
- [ ] Test with live data: sync a playlist, open `#key-comparison`, pick a tag

---

