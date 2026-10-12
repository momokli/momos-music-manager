-- Group kind + semantic role, used by the tagging/scoring/similarity engine.
--   kind: 'class' classifying groups (Mood, Vibe, Phase, Genre, Attribute, Merkmal)
--         'sort'  sorting groups (Rumpelkiste, Setlist)
--   role: optional semantic marker on top of the kind:
--         '' | 'rumpelkiste' | 'setlist' | 'genre' | 'phase'
-- Additive only; backfill by well-known group names (case-insensitive).
ALTER TABLE hub_tag_groups ADD COLUMN kind TEXT NOT NULL DEFAULT 'class';
ALTER TABLE hub_tag_groups ADD COLUMN role TEXT NOT NULL DEFAULT '';

UPDATE hub_tag_groups SET kind = 'sort', role = 'rumpelkiste'
 WHERE lower(trim(name)) IN ('rumpelkiste', 'rumpel');
UPDATE hub_tag_groups SET kind = 'sort', role = 'setlist'
 WHERE lower(trim(name)) IN ('setlist', 'setlists');
UPDATE hub_tag_groups SET role = 'genre'
 WHERE lower(trim(name)) IN ('genre', 'genres');
UPDATE hub_tag_groups SET role = 'phase'
 WHERE lower(trim(name)) IN ('phase', 'phase/energy', 'phase / energy', 'energy');

CREATE INDEX idx_hub_tag_groups_kind ON hub_tag_groups(kind);
