-- Backpack source limit (#654, slice B4): an optional per-item cap on how many
-- tracks a single source contributes to a backpack. NULL = unlimited.
--
-- Applied by the resolver *per source*, after that source's own ordering — it is
-- a per-source truncation, never a global cut. Generic across kinds: while the UI
-- only exposes it for lenses ("neueste 100"), the resolver honours it for any
-- item that has a value set.
ALTER TABLE hub_backpack_items ADD COLUMN max_tracks INTEGER;
