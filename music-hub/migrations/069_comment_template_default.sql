-- The system `Standard` comment template follows the typed tag order (#690).
--
-- Before #690 the default body was `{tags} {bpm} {key} {energy}`. The export and
-- preview now default to `{types} {tags}`; this migration aligns the seeded
-- system row. A body an admin (or a user) has already edited is left untouched:
-- the WHERE clause only matches the exact legacy default.
--
-- Additive + idempotent: a re-run matches nothing once updated.

UPDATE hub_comment_templates
   SET body = '{types} {tags}',
       updated_at = '2026-01-01T00:00:00+00:00'
 WHERE owner_user_id IS NULL
   AND name = 'Standard'
   AND body = '{tags} {bpm} {key} {energy}';
