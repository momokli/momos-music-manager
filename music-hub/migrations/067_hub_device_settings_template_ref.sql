-- Per-device template reference (#669, slice C3): a device may point at one row
-- of the comment-template library (`hub_comment_templates`, migration 066)
-- instead of an inline free-text `comment_template`.
--
-- Purely additive: one nullable column. `NULL` = no reference, so every existing
-- device keeps its previous behaviour (legacy free text, then the default).
-- `ON DELETE SET NULL` means deleting a referenced template clears the
-- reference, letting the resolution chain fall back to the legacy free text or
-- the default template.

ALTER TABLE hub_device_settings
  ADD COLUMN comment_template_id INTEGER
  REFERENCES hub_comment_templates(id) ON DELETE SET NULL;
