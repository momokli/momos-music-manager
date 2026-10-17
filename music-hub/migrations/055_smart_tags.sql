-- Smart-Tags / Dynamic Lenses (#298): ein Tag, dessen Track-Menge zur *Laufzeit*
-- aus einer gespeicherten Filterdefinition abgeleitet wird — nie materialisiert.
--
-- Ein Smart-Tag ist eine normale Zeile in `hub_tags` plus genau EINE Zeile hier
-- (1:1, `tag_id` als Primary Key). Die bloße Existenz dieser Zeile markiert den
-- Tag als "dynamisch"; es gibt bewusst KEIN Flag an `hub_tags` (keine zweite,
-- desynchronisierbare Quelle). `tags::rebuild()` fasst Smart-Tags nicht an, weil
-- sie weder `hub_tag_sources` noch `hub_track_tag_manual`/`_archive` haben.
--
-- `filter_json`: versioniertes Filterobjekt, z. B.
--   {"version":1,"tag_ids":[12,15],"bpm_min":124.0,"bpm_max":138.0,
--    "energy_min":6.0,"limit":100,"sort":"newest"}
-- Additiv, append-only.

CREATE TABLE hub_smart_tags (
    tag_id      INTEGER PRIMARY KEY REFERENCES hub_tags(id) ON DELETE CASCADE,
    filter_json TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_hub_smart_tags_created
    ON hub_smart_tags(created_at);

SELECT 'Migration 055 applied: smart tags (dynamic lenses)' AS status;
