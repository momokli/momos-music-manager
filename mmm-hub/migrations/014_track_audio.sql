-- Audio analysis beyond catalog features: EffNet embeddings (similarity) and
-- local BPM/key analysis (Essentia/Traktor), separate from hub_track_features
-- (ReccoBeats/FreqBlog). Additive.
CREATE TABLE hub_track_embeddings (
    track_id     INTEGER PRIMARY KEY REFERENCES hub_tracks(id) ON DELETE CASCADE,
    model        TEXT NOT NULL DEFAULT 'discogs-effnet',
    dims         INTEGER NOT NULL,
    embedding    BLOB NOT NULL,               -- f32 little-endian, length dims*4
    from_preview INTEGER NOT NULL DEFAULT 0,  -- 1 = computed from a 30s Deezer preview
    created_at   TEXT
);

CREATE TABLE hub_track_analysis (
    track_id    INTEGER PRIMARY KEY REFERENCES hub_tracks(id) ON DELETE CASCADE,
    bpm         REAL,
    key         TEXT,        -- e.g. "A-Minor"
    camelot     TEXT,        -- e.g. "8A"
    source      TEXT NOT NULL, -- 'essentia' | 'traktor' | 'analyzer'
    analyzed_at TEXT
);
CREATE INDEX idx_hub_track_analysis_camelot ON hub_track_analysis(camelot);

-- Unified view: catalog features + local analysis + whether an embedding exists.
CREATE VIEW v_track_audio AS
SELECT
    t.id AS track_id,
    f.bpm       AS feat_bpm,
    f.camelot   AS feat_camelot,
    a.bpm       AS local_bpm,
    a.key       AS local_key,
    a.camelot   AS local_camelot,
    a.source    AS local_source,
    (e.track_id IS NOT NULL) AS has_embedding,
    COALESCE(a.bpm, f.bpm)         AS bpm,
    COALESCE(a.camelot, f.camelot) AS camelot
FROM hub_tracks t
LEFT JOIN hub_track_features  f ON f.track_id = t.id
LEFT JOIN hub_track_analysis  a ON a.track_id = t.id
LEFT JOIN hub_track_embeddings e ON e.track_id = t.id;
