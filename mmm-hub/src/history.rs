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
use axum::extract::{Query, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use serde::Deserialize;
use sqlx::{QueryBuilder, Row, SqlitePool};

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
    /// Sort field for the runs table (whitelisted; default `started`).
    rsort: Option<String>,
    /// Sort direction for the runs table (`asc`/`desc`).
    rdir: Option<String>,
    /// Sort field for the events table (whitelisted; default `at`).
    esort: Option<String>,
    /// Sort direction for the events table (`asc`/`desc`).
    edir: Option<String>,
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
    s_r_source: crate::table::SortHead,
    s_r_user: crate::table::SortHead,
    s_r_status: crate::table::SortHead,
    s_r_started: crate::table::SortHead,
    s_r_finished: crate::table::SortHead,
    s_e_at: crate::table::SortHead,
    s_e_source: crate::table::SortHead,
    s_e_entity_type: crate::table::SortHead,
    s_e_entity_ref: crate::table::SortHead,
    s_e_change: crate::table::SortHead,
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

/// Build a sortable header for a two-table page. `table::sort_head` emits the
/// generic `sort=`/`dir=` params, so we first drop this table's own
/// `<prefix>sort`/`<prefix>dir` from the preserved query (the sibling table's
/// params stay intact), then rewrite the emitted params back to the prefix —
/// letting the runs (`r`) and events (`e`) tables sort independently.
fn prefixed_sort_head(
    raw: &str,
    prefix: &str,
    field: &str,
    label: &str,
    cur_field: &str,
    cur_dir: &str,
) -> crate::table::SortHead {
    let own_sort = format!("{prefix}sort");
    let own_dir = format!("{prefix}dir");
    let cleaned: String = raw
        .split('&')
        .filter(|kv| {
            let k = kv.split('=').next().unwrap_or("");
            !kv.is_empty() && k != own_sort && k != own_dir
        })
        .collect::<Vec<_>>()
        .join("&");
    let mut head = crate::table::sort_head(&cleaned, field, label, cur_field, cur_dir);
    let next_dir = if cur_field == field && cur_dir.eq_ignore_ascii_case("asc") {
        "desc"
    } else {
        "asc"
    };
    let enc = urlencoding::encode(field);
    let generic = format!("sort={enc}&dir={next_dir}");
    let prefixed = format!("{prefix}sort={enc}&{prefix}dir={next_dir}");
    if let Some(pos) = head.href.rfind(generic.as_str()) {
        head.href.replace_range(pos..pos + generic.len(), &prefixed);
    }
    head
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
    RawQuery(raw): RawQuery,
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
    let allowed_runs: &[(&str, &str)] = &[
        ("source", "source"),
        ("user", "user_slug"),
        ("status", "status"),
        ("started", "started_at"),
        ("finished", "finished_at"),
    ];
    let (rsort, rdir) = crate::table::resolve_sort(
        f.rsort.as_deref(),
        f.rdir.as_deref(),
        allowed_runs,
        "started",
        "desc",
    );
    let mut rq: QueryBuilder<sqlx::Sqlite> = QueryBuilder::new(
        "SELECT * FROM (SELECT r.id AS id, r.source AS source, u.slug AS user_slug,
                r.status AS status, r.started_at AS started_at,
                COALESCE(r.finished_at, '') AS finished_at,
                COALESCE(r.stats, '') AS stats
           FROM hub_import_runs r
           JOIN hub_users u ON u.id = r.user_id
          WHERE 1 = 1",
    );
    if !source.is_empty() {
        rq.push(" AND r.source = ").push_bind(source.clone());
    }
    if !user.is_empty() {
        rq.push(" AND u.slug = ").push_bind(user.clone());
    }
    rq.push(") WHERE 1 = 1");
    // Fuzzy full-text over every visible text column (order-independent).
    crate::table::push_fuzzy(&mut rq, &q, &["source", "user_slug", "status", "stats"]);
    crate::table::order_by(&mut rq, &rsort, &rdir, allowed_runs, "started");
    rq.push(" LIMIT 500");
    let run_rows = rq.build().fetch_all(&st.pool).await.unwrap_or_default();
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
    let allowed_events: &[(&str, &str)] = &[
        ("at", "at"),
        ("source", "source"),
        ("entity_type", "entity_type"),
        ("entity_ref", "entity_ref"),
        ("change", "change"),
    ];
    let (esort, edir) = crate::table::resolve_sort(
        f.esort.as_deref(),
        f.edir.as_deref(),
        allowed_events,
        "at",
        "desc",
    );
    let mut eq: QueryBuilder<sqlx::Sqlite> = QueryBuilder::new(
        "SELECT * FROM (SELECT e.at AS at, e.source AS source, u.slug AS user_slug,
                e.entity_type AS entity_type, e.entity_ref AS entity_ref,
                e.change AS change, COALESCE(e.before, '') AS before_val,
                COALESCE(e.after, '') AS after_val
           FROM hub_import_events e
           JOIN hub_users u ON u.id = e.user_id
          WHERE 1 = 1",
    );
    if !source.is_empty() {
        eq.push(" AND e.source = ").push_bind(source.clone());
    }
    if !user.is_empty() {
        eq.push(" AND u.slug = ").push_bind(user.clone());
    }
    eq.push(") WHERE 1 = 1");
    // Fuzzy full-text over every visible text column (order-independent).
    crate::table::push_fuzzy(
        &mut eq,
        &q,
        &[
            "at",
            "source",
            "user_slug",
            "entity_type",
            "entity_ref",
            "change",
            "before_val",
            "after_val",
        ],
    );
    crate::table::order_by(&mut eq, &esort, &edir, allowed_events, "at");
    eq.push(" LIMIT 500");
    let event_rows = eq.build().fetch_all(&st.pool).await.unwrap_or_default();
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

    let rawq = raw.as_deref().unwrap_or("");
    let s_r_source = prefixed_sort_head(rawq, "r", "source", "Quelle", &rsort, &rdir);
    let s_r_user = prefixed_sort_head(rawq, "r", "user", "User", &rsort, &rdir);
    let s_r_status = prefixed_sort_head(rawq, "r", "status", "Status", &rsort, &rdir);
    let s_r_started = prefixed_sort_head(rawq, "r", "started", "Start", &rsort, &rdir);
    let s_r_finished = prefixed_sort_head(rawq, "r", "finished", "Ende", &rsort, &rdir);
    let s_e_at = prefixed_sort_head(rawq, "e", "at", "Zeit", &esort, &edir);
    let s_e_source = prefixed_sort_head(rawq, "e", "source", "Quelle", &esort, &edir);
    let s_e_entity_type = prefixed_sort_head(rawq, "e", "entity_type", "Typ", &esort, &edir);
    let s_e_entity_ref = prefixed_sort_head(rawq, "e", "entity_ref", "Referenz", &esort, &edir);
    let s_e_change = prefixed_sort_head(rawq, "e", "change", "Änderung", &esort, &edir);
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
        s_r_source,
        s_r_user,
        s_r_status,
        s_r_started,
        s_r_finished,
        s_e_at,
        s_e_source,
        s_e_entity_type,
        s_e_entity_ref,
        s_e_change,
    })
}
