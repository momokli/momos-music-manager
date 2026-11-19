-- Artist-Name an der Album-Bindung (#620).
--
-- Die Bindung speicherte den Artist bisher nur als ID (`source_id`), sodass die
-- UI `Deezer-Artist #<id>` zeigte. Ab jetzt wird der beim Setzen bekannte Name
-- additiv als `source_name` mitgeschrieben; fehlt er, faellt die UI auf die ID
-- zurueck.
--
-- Additiv: nullable, ohne Default -> NULL fuer alle Bestandszeilen. Kein
-- Backfill, kein Table-Rebuild.

ALTER TABLE hub_tonie_bindings ADD COLUMN source_name TEXT;

SELECT 'Migration 064 applied: tonie binding source name' AS status;
