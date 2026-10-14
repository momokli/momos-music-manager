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
use sqlx::Row;

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
}

struct Chip {
    label: String,
    icon: String,
}

#[derive(Clone)]
struct TypeOption {
    value: &'static str,
    label: &'static str,
}

struct CardView {
    id: i64,
    rank: i64,
    title: String,
    artists_html: String,
    plays: i64,
    missing: Vec<Chip>,
    my_types: Vec<TypeOption>,
}

#[derive(Template)]
#[template(path = "tag_task.html")]
struct TagTaskPage {
    nav: crate::ui::Nav,
    flash: String,
    cadence: String,
    cadence_daily: bool,
    cadence_weekly: bool,
    cards: Vec<CardView>,
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

/// Build the render-ready card list from the persisted task: join stored items
/// with `hub_tracks`, drop manually-done tracks, map `missing` to chips.
async fn build_cards(st: &AppState, user_id: i64, task_id: i64) -> Vec<CardView> {
    let rows = sqlx::query(
        "SELECT i.track_id, i.rank, i.plays, i.missing,
                COALESCE(t.title,'') AS title, COALESCE(t.artists,'') AS artists
           FROM hub_tag_task_items i
           LEFT JOIN hub_tracks t ON t.id = i.track_id
          WHERE i.task_id = ?1
          ORDER BY i.rank ASC, i.track_id ASC",
    )
    .bind(task_id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();

    let my_types: Vec<TypeOption> = crate::tagtypes::options()
        .into_iter()
        .map(|(value, label, _icon)| TypeOption { value, label })
        .collect();

    let mut cards = Vec::new();
    for r in rows {
        let track_id: i64 = r.get("track_id");
        if crate::tagdone::is_done(&st.pool, user_id, track_id).await {
            continue;
        }
        let missing_raw: String = r.get("missing");
        // Display reactivity (US3): keep the task's families as the base, but
        // drop those a tag applied during the period already resolved, so a
        // freshly tagged family disappears from the card. The card set/order
        // still comes from the persisted (frozen) task.
        let stored: Vec<String> = missing_raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        let stored_refs: Vec<&str> = stored.iter().map(|s| s.as_str()).collect();
        let live = crate::tagdone::status(&st.pool, user_id, track_id, &stored_refs)
            .await
            .missing;
        let missing: Vec<Chip> = live.iter().map(|f| family_chip(f)).collect();
        cards.push(CardView {
            id: track_id,
            rank: r.get("rank"),
            title: r.get("title"),
            artists_html: String::new(),
            plays: r.get("plays"),
            missing,
            my_types: my_types.clone(),
        });
    }
    let ids: Vec<i64> = cards.iter().map(|c| c.id).collect();
    let mut html = crate::artists::artists_html_for_tracks(&st.pool, &ids).await;
    for c in cards.iter_mut() {
        c.artists_html = html.remove(&c.id).unwrap_or_default();
    }
    cards
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
    let cards = build_cards(&st, nav.id, task.id).await;
    render(&TagTaskPage {
        nav,
        flash: String::new(),
        cadence: cadence.as_str().to_string(),
        cadence_daily: cadence == crate::tagtasks::Cadence::Daily,
        cadence_weekly: cadence == crate::tagtasks::Cadence::Weekly,
        cards,
    })
}

#[derive(Deserialize)]
struct TagForm {
    name: String,
    #[serde(default, rename = "type")]
    tag_type: Option<String>,
}

/// Apply a tag from a card through the shared pipeline, then redirect back to
/// the queue. Mirrors `tagqueue::queue_tag` (ensure → set type → tag_track).
async fn tag_task_tag(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<TagForm>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "tag-task").await else {
        return Redirect::to("/login").into_response();
    };
    if !f.name.trim().is_empty() {
        if let Ok(tag_id) = crate::tags::ensure_tag(&st.pool, nav.id, &f.name).await {
            if let Some(ty) = f
                .tag_type
                .as_deref()
                .and_then(crate::tagtypes::TagType::parse)
            {
                let _ = crate::tagtypes::set(&st.pool, nav.id, tag_id, Some(ty)).await;
            }
            let _ = crate::tags::tag_track(&st.pool, nav.id, id, tag_id).await;
        }
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
    Redirect::to("/tag-task").into_response()
}
