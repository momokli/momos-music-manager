-- Group importance/weight: how much a match via a tag in this group counts
-- (e.g. "mood" matters more than "rumpelkiste" — but both still count). Used by
-- the digging/similarity scoring. Editable per group (owner) and by the owning
-- collective's members on the collective page.
ALTER TABLE hub_tag_groups ADD COLUMN weight REAL NOT NULL DEFAULT 0;
