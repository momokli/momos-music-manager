-- Per-user "work settings": the last filter/sort query per list view, so
-- navigating away and back restores where you were.
CREATE TABLE hub_view_state (
    user_id    INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    view       TEXT NOT NULL,
    query      TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (user_id, view)
);
