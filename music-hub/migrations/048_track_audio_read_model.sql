-- Flat read-model of the resolved best audio values (#hub-crawl-everything, plan §6).
--
-- The hub **never analyses** and owns **no vault**. music-api is the Data Vault
-- of music: it resolves the "right" BPM/key per track by source precedence and
-- exposes it on the frozen consumer surface
-- `GET /vault/tracks?isrcs=a,b,c` →
-- `{"tracks":[{"isrc","bpm","key","camelot","energy","source","updatedAt"}]}`.
--
-- The hub pulls that resolved value (see `analysis::refresh_from_vault`) and
-- stores it here — one flat row per track, no per-source rows, no hash keys
-- (that is the vault's job). The UI reads only this read-model.
--
-- Additive only.
CREATE TABLE hub_track_audio (
    track_id    INTEGER PRIMARY KEY REFERENCES hub_tracks(id) ON DELETE CASCADE,
    bpm         REAL,
    musical_key TEXT,                   -- e.g. "A-Minor" / "11m"
    camelot     TEXT,                   -- e.g. "8A"
    energy      REAL,
    source      TEXT,                   -- source the vault resolved the value from
    updated_at  TEXT
);
