# Progress — Issue #347: Tag-task UI

Repo: momokli/momos-music-manager · Milestone: hub-tagging-0.27.0 (#33)
Base: main (9c841de) · Branch: feature/issue-347-tag-task-ui · PR target: main
Deliverable: pushed branch + open PR against `main`, body contains `Closes #347`.

## Issue (Spec)
**"Tag-task UI"** — "Card-based tagging queue for the daily/weekly task."

Milestone outcome: a track is *tagged-complete* when the 5 describing types are set
(or user-forced); a daily/weekly tagging task surfaces uncomplete tracks
(most-played first) in a **card-based queue**; per-type "describes the song" toggles
refine the semantics.

Order: #345 → #346 → **#347** → #348.
- #345 (DONE, merged PR #401 / 14fc0eb): `hub_track_tag_done`, `tagdone.rs`.
- #346 (DONE, merged PR #402 / 9c841de): backend task — `046_tag_task.sql`,
  `src/tagtasks.rs` (`Cadence`, `period_start`, `candidates`, `task_for`),
  `GET /api/hub/tag-task?cadence=daily|weekly`, settings `ENGINE_TAG_TASK_CADENCE`,
  `ENGINE_TAG_TASK_LIMIT`.
- **#347 = this issue: the card UI** consuming the #346 task. #348 = per-type toggle.

## Recon (verbindlich)
- Server-rendered askama templates in `music-hub/templates/`, handlers in
  `music-hub/src/*.rs` (axum Router per module, merged in `api.rs`).
- Card/queue precedent: `src/tagqueue.rs` (`/tag-queue`), templates `queue.html`
  + `queue_detail.html`, shared partials `_partials/tag_clouds.html`,
  `_partials/rec_tags.html`, `_partials/track_tag_form.html`.
- Current user: `crate::web::current_user(&st, &headers)`; nav via
  `crate::ui::nav(&st,&headers,"<key>")` (redirect `/login` when None).
  Nav links live in `templates/base.html` (~line 1057 `Tag-Queue`).
- #346 API handler `api.rs::tag_task` shows exactly how to build the card list:
  `tagtasks::task_for(&pool,user_id,cadence,now,limit)` then join
  `hub_tag_task_items` → `hub_tracks` (title, artists), `missing` is a CSV string.
- Manual completion: `tagdone::set_done` / `clear_done` on
  `hub_track_tag_done(user_id,track_id)`; `tagdone::status(...)`.

## Stage status
| Stage     | Status  | Result |
|-----------|---------|--------|
| recon     | done    | siehe oben |
| planner   | pending | |
| setup     | pending | |
| developer | pending | |
| verifier  | pending | |
| tester    | pending | |
| reviewer  | pending | |
