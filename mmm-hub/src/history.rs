//! Import history — a self-contained ledger + change timeline for every import
//! source. Plan H1–H3 (issues #224–#226).
//!
//! * **H1** [`start_run`] / [`finish_run`] write one `hub_import_runs` row per
//!   import (spotify sync, traktor upload, later soundcloud/youtube) with
//!   timestamps, a status and a free-form JSON `stats` blob.
//! * **H2** [`record_event`] records a single change; [`diff_membership`] and
//!   [`diff_scalar`] are the two helpers importers use to turn a pair of
//!   snapshots (a set and a scalar) into `added`/`removed`/`changed` events.
//! * **H3** `GET /history` renders the runs (per source/user, with status +
//!   stats) and the change timeline in two bounded, dense tables
//!   (`overflow-auto hub-scroll-y`), filterable by `q`/`source`/`user`.
//!
//! The module owns no importer wiring — the coordinator calls these helpers from
//! the ingest paths. Nothing here depends on `tags`/`web`/`pages`.

use askama::Template;
use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use serde::Deserialize;
use sqlx::{Row, SqlitePool};

use crate::api::AppState;

// ── H1 + H2: recording API (used by the importers) ──────────────────────────

/// Start an import run. Returns the new run id. `status` is `running` until
/// [`finish_run`] is called.
pub async fn start_run(pool: &SqlitePool, user_id: i64, source: &str) -> anyhow::Result<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_import_runs (user_id, source, started_at, status, stats)
         VALUES (?1, ?2, ?3, 'running', '{}') RETURNING id",
    )
    .bind(user_id)
    .bind(source)
    .bind(chrono::Utc::now().to_rfc3339())
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Finish a run: stamp `finished_at`, set the terminal `status`
/// (`ok` | `error` | `partial` | …) and store `stats` as JSON.
pub async fn finish_run(
    pool: &SqlitePool,
    run_id: i64,
    status: &str,
    stats: &serde_json::Value,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE hub_import_runs
            SET status = ?1, finished_at = ?2, stats = ?3
          WHERE id = ?4",
    )
    .bind(status)
    .bind(chrono::Utc::now().to_rfc3339())
    .bind(stats.to_string())
    .bind(run_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record one change event for a run. `change` must be one of
/// `added` | `removed` | `changed` (enforced by the DB CHECK constraint).
#[allow(clippy::too_many_arguments)]
pub async fn record_event(
    pool: &SqlitePool,
    run_id: i64,
    user_id: i64,
    source: &str,
    entity_type: &str,
    entity_ref: &str,
    change: &str,
    before: Option<&str>,
    after: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO hub_import_events
            (run_id, user_id, source, entity_type, entity_ref, change, before, after, at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(run_id)
    .bind(user_id)
    .bind(source)
    .bind(entity_type)
    .bind(entity_ref)
    .bind(change)
    .bind(before)
    .bind(after)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

/// Diff two snapshots of a **set of ids** (e.g. playlist membership) and record
/// `added` (in `new_ids`, not `old_ids`) and `removed` (in `old_ids`, not
/// `new_ids`) events. `entity_ref` is the stringified id; the id is also the
/// `after` (added) / `before` (removed) value.
pub async fn diff_membership(
    pool: &SqlitePool,
    run_id: i64,
    user_id: i64,
    source: &str,
    entity_type: &str,
    old_ids: &[i64],
    new_ids: &[i64],
) -> anyhow::Result<()> {
    let old: std::collections::HashSet<i64> = old_ids.iter().copied().collect();
    let new: std::collections::HashSet<i64> = new_ids.iter().copied().collect();

    let mut added: Vec<i64> = new.difference(&old).copied().collect();
    let mut removed: Vec<i64> = old.difference(&new).copied().collect();
    added.sort_unstable();
    removed.sort_unstable();

    for id in added {
        let v = id.to_string();
        record_event(
            pool,
            run_id,
            user_id,
            source,
            entity_type,
            &v,
            "added",
            None,
            Some(&v),
        )
        .await?;
    }
    for id in removed {
        let v = id.to_string();
        record_event(
            pool,
            run_id,
            user_id,
            source,
            entity_type,
            &v,
            "removed",
            Some(&v),
            None,
        )
        .await?;
    }
    Ok(())
}

/// Diff a **scalar value** (e.g. a traktor playcount/rating) and record a single
/// `changed` event, but only when the value actually differs.
pub async fn diff_scalar(
    pool: &SqlitePool,
    run_id: i64,
    user_id: i64,
    source: &str,
    entity_type: &str,
    entity_ref: &str,
    old: Option<&str>,
    new: Option<&str>,
) -> anyhow::Result<()> {
    if old == new {
        return Ok(());
    }
    record_event(
        pool,
        run_id,
        user_id,
        source,
        entity_type,
        entity_ref,
        "changed",
        old,
        new,
    )
    .await
}

// ── H3: history page ────────────────────────────────────────────────────────

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/history", get(history_page))
        .with_state(state)
}

fn render<T: Template>(tpl: &T) -> Response {
    match tpl.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Template-Fehler: {e}"),
        )
            .into_response(),
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct HistoryFilter {
    /// Free-text search over source / entity_type / entity_ref / stats.
    q: Option<String>,
    /// Exact source match (empty = all).
    source: Option<String>,
    /// Exact user slug match (empty = all).
    user: Option<String>,
}

#[derive(Template)]
#[template(path = "history.html")]
struct HistoryPage {
    nav: crate::ui::Nav,
    flash: String,
    q: String,
    sources: Vec<SourceOption>,
    users: Vec<UserOption>,
    runs: Vec<RunRow>,
    events: Vec<EventRow>,
    run_count: usize,
    event_count: usize,
}

struct SourceOption {
    value: String,
    selected: bool,
}

struct UserOption {
    value: String,
    selected: bool,
}

struct RunRow {
    id: i64,
    source: String,
    user: String,
    status: String,
    status_class: String,
    started: String,
    finished: String,
    stats: String,
}

struct EventRow {
    at: String,
    source: String,
    user: String,
    entity_type: String,
    entity_ref: String,
    change: String,
    change_class: String,
    before: String,
    after: String,
}

/// Badge class for a run status (precomputed — askama must not compare inside
/// an attribute).
fn status_class(status: &str) -> String {
    match status {
        "ok" => "hub-badge-ok".to_string(),
        "error" => "hub-badge-no".to_string(),
        _ => String::new(),
    }
}

/// Badge class for a change kind.
fn change_class(change: &str) -> String {
    match change {
        "added" => "hub-badge-ok".to_string(),
        "removed" => "hub-badge-no".to_string(),
        _ => String::new(),
    }
}

async fn history_page(
    State(st): State<AppState>,
    Query(f): Query<HistoryFilter>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "history").await else {
        return Redirect::to("/login").into_response();
    };
    let q = f.q.unwrap_or_default().trim().to_string();
    let source = f.source.unwrap_or_default().trim().to_string();
    let user = f.user.unwrap_or_default().trim().to_string();

    // Filter option lists (server-side; distinct across all runs).
    let sources: Vec<SourceOption> = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT source FROM hub_import_runs ORDER BY source",
    )
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|value| SourceOption {
        selected: value == source,
        value,
    })
    .collect();

    let users: Vec<UserOption> = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT u.slug FROM hub_import_runs r
           JOIN hub_users u ON u.id = r.user_id
          ORDER BY u.slug",
    )
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|value| UserOption {
        selected: value == user,
        value,
    })
    .collect();

    // Runs. Empty filter strings disable that clause (all binds are always set).
    let run_rows = sqlx::query(
        "SELECT r.id, r.source, u.slug AS user_slug, r.status,
                r.started_at, COALESCE(r.finished_at, '') AS finished_at,
                COALESCE(r.stats, '') AS stats
           FROM hub_import_runs r
           JOIN hub_users u ON u.id = r.user_id
          WHERE (?1 = '' OR r.source = ?1)
            AND (?2 = '' OR u.slug = ?2)
            AND (?3 = '' OR r.source LIKE '%' || ?3 || '%'
                        OR u.slug LIKE '%' || ?3 || '%'
                        OR COALESCE(r.stats, '') LIKE '%' || ?3 || '%')
          ORDER BY r.started_at DESC, r.id DESC
          LIMIT 500",
    )
    .bind(&source)
    .bind(&user)
    .bind(&q)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();
    let runs: Vec<RunRow> = run_rows
        .iter()
        .map(|r| {
            let status: String = r.get("status");
            RunRow {
                status_class: status_class(&status),
                id: r.get("id"),
                source: r.get("source"),
                user: r.get("user_slug"),
                status,
                started: r.get("started_at"),
                finished: r.get("finished_at"),
                stats: r.get("stats"),
            }
        })
        .collect();

    // Change timeline.
    let event_rows = sqlx::query(
        "SELECT e.at, e.source, u.slug AS user_slug, e.entity_type, e.entity_ref,
                e.change, COALESCE(e.before, '') AS before_val,
                COALESCE(e.after, '') AS after_val
           FROM hub_import_events e
           JOIN hub_users u ON u.id = e.user_id
          WHERE (?1 = '' OR e.source = ?1)
            AND (?2 = '' OR u.slug = ?2)
            AND (?3 = '' OR e.entity_ref LIKE '%' || ?3 || '%'
                        OR e.entity_type LIKE '%' || ?3 || '%'
                        OR e.source LIKE '%' || ?3 || '%'
                        OR COALESCE(e.before, '') LIKE '%' || ?3 || '%'
                        OR COALESCE(e.after, '') LIKE '%' || ?3 || '%')
          ORDER BY e.at DESC, e.id DESC
          LIMIT 500",
    )
    .bind(&source)
    .bind(&user)
    .bind(&q)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();
    let events: Vec<EventRow> = event_rows
        .iter()
        .map(|r| {
            let change: String = r.get("change");
            EventRow {
                change_class: change_class(&change),
                at: r.get("at"),
                source: r.get("source"),
                user: r.get("user_slug"),
                entity_type: r.get("entity_type"),
                entity_ref: r.get("entity_ref"),
                change,
                before: r.get("before_val"),
                after: r.get("after_val"),
            }
        })
        .collect();

    let run_count = runs.len();
    let event_count = events.len();
    render(&HistoryPage {
        nav,
        flash: String::new(),
        q,
        sources,
        users,
        runs,
        events,
        run_count,
        event_count,
    })
}
