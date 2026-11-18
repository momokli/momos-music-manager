-- Tonies: in welcher Reihenfolge eine Playlist den Tonie fuellt.
--
-- `playlist`      - wie in Spotify sortiert (Spalte `position`, Default)
-- `newest`        - die zuletzt hinzugefuegten Tracks fuellen den Tonie;
--                   Wiedergabe in derselben Reihenfolge (neueste zuerst)
-- `newest_chrono` - dieselbe Auswahl, Wiedergabe aber in Reihenfolge des
--                   Hinzufuegens (aelteste zuerst) - fuer Hoerspiel-Serien
--
-- Additiv: bestehende Bindings bleiben bei `playlist`.

ALTER TABLE hub_tonie_bindings ADD COLUMN order_by TEXT NOT NULL DEFAULT 'playlist';

SELECT 'Migration 061 applied: tonie binding track order' AS status;
