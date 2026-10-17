//! Tag-task UI (#347) — the card-based tagging queue for the daily/weekly
//! task built in #346.
//!
//! `GET /tag-task` renders the *persisted* task (`crate::tagtasks::task_for`)
//! as a card list, most-played first. Each card shows the track head, its
//! `rank`/`plays`, the still-missing describing families as chips, an inline
//! tag form and a manual `done`/`undo` toggle. The task is **not** recomputed;
//! only its stored items are joined with `hub_tracks`.
//!
//! Cards whose track is manually marked done are suppressed at render time so
//! `done` has a visible effect despite the period-frozen task.

use askama::Template;
use axum::Router;
use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use serde::Deserialize;

use crate::api::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/tag-task", get(tag_task_page))
        .route("/tag-task/{id}/tag", post(tag_task_tag))
        .route("/tag-task/{id}/done", post(tag_task_done))
        .route("/tag-task/{id}/undone", post(tag_task_undone))
        .with_state(state)
}

fn render<T: Template>(t: &T) -> Response {
    match t.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Template-Fehler: {e}"),
        )
            .into_response(),
    }
}

/// Resolve the effective cadence exactly like `api.rs::tag_task`: an explicit
/// `?cadence=` wins, otherwise the `engine_tag_task_cadence` setting, otherwise
/// `Daily`. Unknown values fall back to `Daily`.
async fn cadence_for(st: &AppState, raw: Option<&str>) -> crate::tagtasks::Cadence {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(v) => crate::tagtasks::Cadence::parse(v).unwrap_or(crate::tagtasks::Cadence::Daily),
        None => crate::settings::get(&st.pool, crate::settings::ENGINE_TAG_TASK_CADENCE)
            .await
            .as_deref()
            .and_then(crate::tagtasks::Cadence::parse)
            .unwrap_or(crate::tagtasks::Cadence::Daily),
    }
}

/// The `engine_tag_task_limit` setting, default 50, non-positive ignored.
async fn limit_for(st: &AppState) -> i64 {
    crate::settings::get(&st.pool, crate::settings::ENGINE_TAG_TASK_LIMIT)
        .await
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(50)
}

#[derive(Deserialize, Default)]
struct PageQuery {
    cadence: Option<String>,
    /// "1" = only tracks in my playlists.
    mine: Option<String>,
    /// "1" = show the skipped tracks instead of the task cards.
    skipped: Option<String>,
}

/// True when the request came from htmx (so we return a fragment, not a redirect).
fn is_htmx(headers: &HeaderMap) -> bool {
    headers.get("HX-Request").is_some()
}

struct Chip {
    label: String,
    icon: String,
}

struct CardView {
    id: i64,
    rank: i64,
    title: String,
    artists_html: String,
    plays: i64,
    missing: Vec<Chip>,
    /// Tags already resolved on the track, so a freshly added tag is visible
    /// without a full reload.
    tags: Vec<String>,
    /// Whether the track can be streamed (music-api configured + ISRC present).
    playable: bool,
    /// Unified tag input (#449): action URL (with cadence), htmx target, scope.
    tag_action: String,
    tag_target: String,
    /// Legacy recommendation scope for the tag input `data-scope`.
    rec_scope_param: String,
    /// Engine tag scope (#267) for the tag input `data-tag-scope`.
    tag_scope: String,
    my_types: Vec<(String, String)>,
}

/// One task item joined with its track — the row shape `build_cards`/`card_for`
/// share.
#[derive(sqlx::FromRow)]
struct ItemRow {
    track_id: i64,
    rank: i64,
    plays: i64,
    missing: String,
    title: String,
    isrc: String,
}

/// A single card, rendered standalone for the htmx in-place update.
#[derive(Template)]
#[template(path = "_partials/tag_task_card.html")]
struct TagTaskCard {
    card: CardView,
}

#[derive(Template)]
#[template(path = "tag_task.html")]
struct TagTaskPage {
    nav: crate::ui::Nav,
    flash: String,
    cadence: String,
    cadence_daily: bool,
    cadence_weekly: bool,
    mine: bool,
    skipped: bool,
    cards: Vec<CardView>,
    skipped_tracks: Vec<SkippedRow>,
}

/// A track the user skipped (for the "übersprungene" view).
struct SkippedRow {
    id: i64,
    title: String,
    artists_html: String,
}

/// Map a missing describing *family* (e.g. `MOOD`, `PHASE`) to a display chip.
///
/// `PHASE` is a collapsed family (`PHASE_1..5`) that `TagType::parse` does not
/// accept directly, so it gets a dedicated label; unknown names fall back to
/// their raw form.
fn family_chip(fam: &str) -> Chip {
    let up = fam.trim().to_ascii_uppercase();
    if let Some(t) = crate::tagtypes::TagType::parse(&up) {
        return Chip {
            label: t.label().to_string(),
            icon: t.icon().to_string(),
        };
    }
    if up == "PHASE" {
        return Chip {
            label: "Phase".to_string(),
            icon: "⚡".to_string(),
        };
    }
    Chip {
        label: up,
        icon: "•".to_string(),
    }
}

/// Build one render-ready card from a joined task-item row: live missing
/// families, the resolved tags, and whether it is streamable.
async fn card_from(
    st: &AppState,
    user_id: i64,
    r: ItemRow,
    music_api: bool,
    cadence: &str,
    my_types: &[(String, String)],
) -> CardView {
    // Display reactivity (US3): keep the task's families as the base, but drop
    // those a tag applied during the period already resolved.
    let stored: Vec<String> = r
        .missing
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    let stored_refs: Vec<&str> = stored.iter().map(|s| s.as_str()).collect();
    let live = crate::tagdone::status(&st.pool, user_id, r.track_id, &stored_refs)
        .await
        .missing;
    let missing: Vec<Chip> = live.iter().map(|f| family_chip(f)).collect();
    let tags: Vec<String> = sqlx::query_scalar(
        "SELECT t.name FROM hub_track_resolved_tags rt
           JOIN hub_tags t ON t.id = rt.tag_id
          WHERE rt.track_id = ?1 ORDER BY t.name COLLATE NOCASE LIMIT 100",
    )
    .bind(r.track_id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();
    CardView {
        id: r.track_id,
        rank: r.rank,
        title: r.title,
        artists_html: String::new(),
        plays: r.plays,
        missing,
        tags,
        playable: music_api && !r.isrc.trim().is_empty(),
        tag_action: format!("/tag-task/{}/tag?cadence={}", r.track_id, cadence),
        tag_target: "closest .hub-tag-task-card".to_string(),
        rec_scope_param: String::new(),
        tag_scope: String::new(),
        my_types: my_types.to_vec(),
    }
}

/// The render-ready card list from the persisted task: join stored items with
/// `hub_tracks`, drop manually-done tracks (and, with `mine`, tracks outside the
/// user's playlists), map `missing` to chips.
async fn build_cards(
    st: &AppState,
    user_id: i64,
    task_id: i64,
    cadence: &str,
    mine: bool,
) -> Vec<CardView> {
    let rows = sqlx::query_as::<_, ItemRow>(
        "SELECT i.track_id, i.rank, i.plays, i.missing,
                COALESCE(t.title,'') AS title, COALESCE(t.isrc,'') AS isrc
           FROM hub_tag_task_items i
           LEFT JOIN hub_tracks t ON t.id = i.track_id
          WHERE i.task_id = ?1
          ORDER BY i.rank ASC, i.track_id ASC",
    )
    .bind(task_id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();

    let music_api = st.cfg.music_api_token.is_some();
    let my_types: Vec<(String, String)> = crate::tagtypes::options()
        .into_iter()
        .map(|(value, label, _icon)| (value.to_string(), label.to_string()))
        .collect();

    // Tracks in my playlists (for the "mine" filter).
    let mine_ids: std::collections::HashSet<i64> = if mine {
        sqlx::query_scalar::<_, i64>(
            "SELECT DISTINCT hpt.track_id FROM hub_playlist_tracks hpt
               JOIN hub_playlists hp ON hp.id = hpt.playlist_id
              WHERE hp.user_id = ?1",
        )
        .bind(user_id)
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect()
    } else {
        Default::default()
    };

    let mut cards = Vec::new();
    for r in rows {
        if crate::tagdone::is_done(&st.pool, user_id, r.track_id).await {
            continue;
        }
        if mine && !mine_ids.contains(&r.track_id) {
            continue;
        }
        cards.push(card_from(st, user_id, r, music_api, cadence, &my_types).await);
    }
    let ids: Vec<i64> = cards.iter().map(|c| c.id).collect();
    let mut html = crate::artists::artists_html_for_tracks(&st.pool, &ids).await;
    for c in cards.iter_mut() {
        c.artists_html = html.remove(&c.id).unwrap_or_default();
    }
    cards
}

/// Rebuild a single card (for the htmx in-place update after a tag/done action).
/// Ensures the task exists (generating it if needed) so a tag POST works even
/// before the page has been loaded.
async fn card_for(
    st: &AppState,
    user_id: i64,
    track_id: i64,
    cadence: crate::tagtasks::Cadence,
) -> Option<CardView> {
    let limit = limit_for(st).await;
    let task = crate::tagtasks::task_for(&st.pool, user_id, cadence, chrono::Utc::now(), limit)
        .await
        .ok()?;
    let row = sqlx::query_as::<_, ItemRow>(
        "SELECT i.track_id, i.rank, i.plays, i.missing,
                COALESCE(t.title,'') AS title, COALESCE(t.isrc,'') AS isrc
           FROM hub_tag_task_items i
           LEFT JOIN hub_tracks t ON t.id = i.track_id
          WHERE i.track_id = ?1 AND i.task_id = ?2",
    )
    .bind(track_id)
    .bind(task.id)
    .fetch_optional(&st.pool)
    .await
    .ok()
    .flatten()?;
    let music_api = st.cfg.music_api_token.is_some();
    let my_types: Vec<(String, String)> = crate::tagtypes::options()
        .into_iter()
        .map(|(value, label, _icon)| (value.to_string(), label.to_string()))
        .collect();
    let mut card = card_from(st, user_id, row, music_api, cadence.as_str(), &my_types).await;
    let mut html = crate::artists::artists_html_for_tracks(&st.pool, &[card.id]).await;
    card.artists_html = html.remove(&card.id).unwrap_or_default();
    Some(card)
}

/// The tracks the user has skipped (for the "übersprungene" view), newest first.
async fn build_skipped(st: &AppState, user_id: i64) -> Vec<SkippedRow> {
    let rows = sqlx::query_as::<_, (i64, String)>(
        "SELECT t.id, COALESCE(t.title,'') FROM hub_track_tag_done d
           JOIN hub_tracks t ON t.id = d.track_id
          WHERE d.user_id = ?1
          ORDER BY d.marked_at DESC LIMIT 500",
    )
    .bind(user_id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();
    let ids: Vec<i64> = rows.iter().map(|(id, _)| *id).collect();
    let mut html = crate::artists::artists_html_for_tracks(&st.pool, &ids).await;
    rows.into_iter()
        .map(|(id, title)| SkippedRow {
            id,
            title,
            artists_html: html.remove(&id).unwrap_or_default(),
        })
        .collect()
}

async fn tag_task_page(
    State(st): State<AppState>,
    Query(q): Query<PageQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "tag-task").await else {
        return Redirect::to("/login").into_response();
    };
    let cadence = cadence_for(&st, q.cadence.as_deref()).await;
    let mine = q.mine.as_deref() == Some("1");
    let skipped = q.skipped.as_deref() == Some("1");
    let limit = limit_for(&st).await;
    let task = match crate::tagtasks::task_for(&st.pool, nav.id, cadence, chrono::Utc::now(), limit)
        .await
    {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Task-Fehler: {e}"),
            )
                .into_response();
        }
    };
    let cards = build_cards(&st, nav.id, task.id, cadence.as_str(), mine).await;
    let skipped_tracks = if skipped {
        build_skipped(&st, nav.id).await
    } else {
        Vec::new()
    };
    render(&TagTaskPage {
        nav,
        flash: String::new(),
        cadence: cadence.as_str().to_string(),
        cadence_daily: cadence == crate::tagtasks::Cadence::Daily,
        cadence_weekly: cadence == crate::tagtasks::Cadence::Weekly,
        mine,
        skipped,
        cards,
        skipped_tracks,
    })
}

#[derive(Deserialize)]
struct TagForm {
    #[serde(default)]
    name: String,
    #[serde(default, rename = "type")]
    tag_type: Option<String>,
    /// Existing tag to link (the unified tag input, #449).
    #[serde(default)]
    tag_id: Option<i64>,
    /// Cadence of the task the card belongs to (carried through the htmx form).
    #[serde(default)]
    cadence: Option<String>,
}

/// Apply a tag from a card through the shared pipeline. htmx gets the re-rendered
/// card back (in-place update); a plain POST redirects to the queue.
/// Mirrors `tagqueue::queue_tag` (ensure → set type → tag_track).
async fn tag_task_tag(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<TagForm>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "tag-task").await else {
        return Redirect::to("/login").into_response();
    };
    if let Some(tag_id) = crate::tags::resolve_tag_for_add(
        &st.pool,
        nav.id,
        f.tag_id,
        Some(&f.name),
        f.tag_type.as_deref(),
    )
    .await
    {
        let _ = crate::tags::tag_track(&st.pool, &st.cfg, nav.id, id, tag_id).await;
    }
    if is_htmx(&headers) {
        let cadence = cadence_for(&st, f.cadence.as_deref()).await;
        return match card_for(&st, nav.id, id, cadence).await {
            Some(card) => render(&TagTaskCard { card }),
            None => Html(String::new()).into_response(),
        };
    }
    Redirect::to("/tag-task").into_response()
}

async fn tag_task_done(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "tag-task").await else {
        return Redirect::to("/login").into_response();
    };
    let _ = crate::tagdone::set_done(&st.pool, nav.id, id).await;
    if is_htmx(&headers) {
        // An empty outerHTML swap removes the card.
        return Html(String::new()).into_response();
    }
    Redirect::to("/tag-task").into_response()
}

async fn tag_task_undone(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "tag-task").await else {
        return Redirect::to("/login").into_response();
    };
    let _ = crate::tagdone::clear_done(&st.pool, nav.id, id).await;
    if is_htmx(&headers) {
        // An empty outerHTML swap removes the row.
        return Html(String::new()).into_response();
    }
    Redirect::to("/tag-task?skipped=1").into_response()
}
