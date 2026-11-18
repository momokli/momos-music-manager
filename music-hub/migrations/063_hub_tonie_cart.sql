-- Tonie-Warenkorb, M2 der Album-Queue (plans/proposed/tonie-album-queue.md).
--
-- Bisher hing der Korb an der Bindung (`hub_tonie_album_items`), also an einer
-- bereits konfigurierten Figur. Ab jetzt ist der Korb ein Entwurf pro
-- (User, Figur): er existiert auch ohne Bindung, ueberlebt Logins und
-- Geraetewechsel und ist die Quelle fuer `album_mode = 'basket'`.
--
-- Additiv: die alte Tabelle bleibt liegen und wird nicht mehr beschrieben.

CREATE TABLE IF NOT EXISTS hub_tonie_cart_items (
    id        INTEGER PRIMARY KEY,
    user_id   INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    tonie_id  TEXT NOT NULL,               -- Kreativ-Tonie ctid
    position  INTEGER NOT NULL,
    album_id  TEXT NOT NULL,               -- Deezer-Album-ID
    title     TEXT,
    cover_url TEXT,
    seconds   INTEGER,
    added_at  TEXT NOT NULL,
    UNIQUE (user_id, tonie_id, album_id)
);

CREATE INDEX IF NOT EXISTS idx_hub_tonie_cart_user
    ON hub_tonie_cart_items(user_id, tonie_id, position);

-- Einmalige Uebernahme des bisherigen Bindungs-Korbs.
INSERT OR IGNORE INTO hub_tonie_cart_items
    (user_id, tonie_id, position, album_id, title, cover_url, seconds, added_at)
SELECT b.user_id, b.tonie_id, i.position, i.album_id, i.album_title, NULL, i.seconds, i.added_at
  FROM hub_tonie_album_items i
  JOIN hub_tonie_bindings b ON b.id = i.binding_id;

SELECT 'Migration 063 applied: tonie cart per (user, tonie)' AS status;
