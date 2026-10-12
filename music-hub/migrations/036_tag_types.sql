-- Tags get ONE hard-wired type; the flexible per-user group layer is removed.
--
--   type ∈ { MOOD, GENRE, VIBE, PHASE_1..PHASE_5, MERKMAL, SETLIST, RUMPELKISTE }
--
-- "describes the song" is inherent to the type (MOOD/GENRE/VIBE/PHASE_*/MERKMAL
-- describe; SETLIST/RUMPELKISTE sort). Backfill is a straight 1:1 map from each
-- tag's group (in practice every tag had at most one group). Unmapped tags stay
-- NULL for now.

-- The legacy FK column to the groups table must go before we drop the table.
ALTER TABLE hub_tags DROP COLUMN category_id;

ALTER TABLE hub_tags ADD COLUMN type TEXT
    CHECK (type IS NULL OR type IN
      ('MOOD','GENRE','VIBE','PHASE_1','PHASE_2','PHASE_3','PHASE_4','PHASE_5',
       'MERKMAL','SETLIST','RUMPELKISTE'));

-- ── backfill from the group each tag lived in (role first, then name) ─────────

UPDATE hub_tags SET type = 'SETLIST'
 WHERE type IS NULL AND id IN (
   SELECT gt.tag_id FROM hub_group_tags gt JOIN hub_tag_groups g ON g.id = gt.group_id
    WHERE g.role = 'setlist' OR lower(trim(g.name)) IN ('setlist', 'setlists'));

UPDATE hub_tags SET type = 'RUMPELKISTE'
 WHERE type IS NULL AND id IN (
   SELECT gt.tag_id FROM hub_group_tags gt JOIN hub_tag_groups g ON g.id = gt.group_id
    WHERE g.role = 'rumpelkiste' OR lower(trim(g.name)) IN ('rumpelkiste', 'rumpel'));

UPDATE hub_tags SET type = 'MOOD'
 WHERE type IS NULL AND id IN (
   SELECT gt.tag_id FROM hub_group_tags gt JOIN hub_tag_groups g ON g.id = gt.group_id
    WHERE lower(trim(g.name)) IN ('mood', 'moods'));

UPDATE hub_tags SET type = 'VIBE'
 WHERE type IS NULL AND id IN (
   SELECT gt.tag_id FROM hub_group_tags gt JOIN hub_tag_groups g ON g.id = gt.group_id
    WHERE lower(trim(g.name)) IN ('vibe', 'vibes'));

UPDATE hub_tags SET type = 'MERKMAL'
 WHERE type IS NULL AND id IN (
   SELECT gt.tag_id FROM hub_group_tags gt JOIN hub_tag_groups g ON g.id = gt.group_id
    WHERE lower(trim(g.name)) IN ('merkmal', 'merkmale', 'attribute', 'attribut'));

UPDATE hub_tags SET type = 'GENRE'
 WHERE type IS NULL AND id IN (
   SELECT gt.tag_id FROM hub_group_tags gt JOIN hub_tag_groups g ON g.id = gt.group_id
    WHERE g.role = 'genre' OR lower(trim(g.name)) IN ('genre', 'genres'));

-- Phase: PHASE_<rank> (rank NULL / out of range -> PHASE_3).
UPDATE hub_tags SET type = 'PHASE_' || (
    SELECT CASE WHEN gt.rank BETWEEN 1 AND 5 THEN gt.rank ELSE 3 END
      FROM hub_group_tags gt JOIN hub_tag_groups g ON g.id = gt.group_id
     WHERE gt.tag_id = hub_tags.id
       AND (g.role = 'phase'
            OR lower(trim(g.name)) IN ('phase', 'phase/energy', 'phase / energy', 'energy'))
     LIMIT 1)
 WHERE type IS NULL AND id IN (
   SELECT gt.tag_id FROM hub_group_tags gt JOIN hub_tag_groups g ON g.id = gt.group_id
    WHERE g.role = 'phase'
       OR lower(trim(g.name)) IN ('phase', 'phase/energy', 'phase / energy', 'energy'));

-- ── drop the whole group subsystem ───────────────────────────────────────────

DROP TABLE IF EXISTS hub_group_tags;
DROP TABLE IF EXISTS hub_group_members;
DROP TABLE IF EXISTS hub_tag_groups;

CREATE INDEX idx_hub_tags_type ON hub_tags(type);
