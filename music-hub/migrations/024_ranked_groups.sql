-- Ranked groups: a group can be marked "ranked" and its tags ordered on a 1..5
-- scale (energy level, e.g. Phase: start=1 ... peak=5). MMM stores this in
-- `tag_energy_levels` (per tag); here the rank is per (tag, group) so a tag can
-- rank differently across groups, and the group carries the "ranked" flag.
ALTER TABLE hub_tag_groups ADD COLUMN ranked INTEGER NOT NULL DEFAULT 0;
ALTER TABLE hub_group_tags ADD COLUMN rank INTEGER;
