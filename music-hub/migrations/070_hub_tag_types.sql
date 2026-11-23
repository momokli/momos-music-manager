-- 1) hub_tag_types anlegen
CREATE TABLE hub_tag_types (
    id INTEGER PRIMARY KEY,
    owner_user_id INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    label TEXT NOT NULL,
    abbrev TEXT NOT NULL,
    icon TEXT NOT NULL DEFAULT '',
    sort_order INTEGER NOT NULL DEFAULT 0,
    describes INTEGER NOT NULL DEFAULT 1,
    weight REAL NOT NULL DEFAULT 1.0,
    created_at TEXT NOT NULL,
    UNIQUE (owner_user_id, key)
);

CREATE INDEX idx_hub_tag_types_owner ON hub_tag_types(owner_user_id);
CREATE UNIQUE INDEX idx_hub_tag_types_owner_abbrev ON hub_tag_types(owner_user_id, abbrev) WHERE abbrev <> '';

-- 2) Seed-Funktion (SQLite unterstützt keine Prozeduren → CTE + INSERT OR IGNORE)
--    Wird aus Rust aufgerufen: seed_types(pool, user_id)
--    Hier als Vorlage für den Rust-Code (siehe Schritt 2).

-- 3) CHECK-Constraint aus hub_tags.type entfernen
--    SQLite: Tabelle kopieren, umbenennen, Indizes/Views neu anlegen.
--    Erst abhängige Views droppen, dann Tabelle neu anlegen.
DROP VIEW IF EXISTS hub_v_track_tags;

CREATE TABLE hub_tags_new (
    id            INTEGER PRIMARY KEY,
    owner_user_id INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    slug          TEXT NOT NULL COLLATE NOCASE,
    name          TEXT NOT NULL,
    type          TEXT,  -- KEIN CHECK mehr
    created_at    TEXT NOT NULL,
    sync_playlist INTEGER NOT NULL DEFAULT 0,
    UNIQUE (owner_user_id, slug)
);

-- Daten kopieren
INSERT INTO hub_tags_new (id, owner_user_id, slug, name, type, created_at, sync_playlist)
SELECT id, owner_user_id, slug, name, type, created_at, sync_playlist
FROM hub_tags;

-- Alte Tabelle droppen, neue umbenennen
DROP TABLE hub_tags;
ALTER TABLE hub_tags_new RENAME TO hub_tags;

-- Indizes neu anlegen (aus 015_tag_per_user.sql + 036_tag_types.sql)
CREATE INDEX idx_hub_tags_owner ON hub_tags(owner_user_id);
CREATE INDEX idx_hub_tags_type ON hub_tags(type);

-- Views neu anlegen (aus 015_tag_per_user.sql)
CREATE VIEW hub_v_track_tags AS
SELECT rt.track_id, t.id AS tag_id, t.name AS tag, t.slug, t.owner_user_id
  FROM hub_track_resolved_tags rt
  JOIN hub_tags t ON t.id = rt.tag_id;

-- 4) Backfill-Steuerung: Einmalig für bestehende User
--    Gesetzt wird das Flag in hub_settings (global, nicht pro User!),
--    weil der Backfill einmalig für ALLE bestehenden User läuft.
--    Neuer User: Seed passiert bei Signup (Code-seitig).
INSERT OR IGNORE INTO hub_settings (key, value, updated_at)
VALUES ('tag_types_backfill_done', '0', datetime('now'));

-- 5) Backfill-Logik (wird aus Rust aufgerufen, nicht in Migration):
--    IF (SELECT value FROM hub_settings WHERE key = 'tag_types_backfill_done') = '0'
--       THEN FOR EACH user_id IN (SELECT id FROM hub_users)
--            CALL seed_types(pool, user_id)
--       UPDATE hub_settings SET value = '1' WHERE key = 'tag_types_backfill_done'