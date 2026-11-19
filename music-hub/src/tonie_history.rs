//! Tonie-Verlauf (#604, Slice U4): was auf einer Figur liegt, was vorher lief
//! und welche Alben als gehoert gelten.
//!
//! Die Seite liest ausschliesslich die Datenbank — `hub_tonie_bindings`,
//! `hub_tonie_syncs` und `hub_tonie_albums_seen`. Kein Tonies-Abruf, kein
//! Netzwerk: der Verlauf bleibt sichtbar, auch wenn das Konto gerade nicht
//! verbunden ist.
//!
//! Alles ist user-scoped. Bindings und Ledger werden nur ueber `nav.id`
//! gelesen, ein fremder Tonie oder ein fremdes Album taucht hier nie auf —
//! `unlisten`/`reset` treffen ebenfalls nur die Zeilen des angemeldeten Users.

use askama::Template;
use axum::Router;
use axum::extract::{Form, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use serde::Deserialize;

use crate::api::AppState;
use crate::tonies::bindings::SeenAlbum;
use crate::tonies::run::{
    LIST_LIMIT, STATUS_ERROR, STATUS_NOOP, STATUS_OK, STATUS_RUNNING, STATUS_TIMEOUT,
};
use crate::tonies::{Binding, SyncRow};

/// Wie viele gehoerte Alben die Liste zeigt.
pub const SEEN_LIMIT: i64 = 100;

/// `GET /tonies/history` plus the two ledger mutations. Own router so the page
/// does not grow `web.rs` any further.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/tonies/history", get(history_page))
        .route("/tonies/history/unlisten", post(unlisten_post))
        .route("/tonies/history/reset", post(reset_post))
        .with_state(state)
}

// ── view model ──────────────────────────────────────────────────────────────

/// One figure with its run history.
struct FigureHistory {
    /// Tonie id — also the stable HTML anchor (`#figure-{id}`).
    tonie_id: String,
    /// Display name; falls back to the id when the binding has none.
    name: String,
    /// Source in short form, e.g. `Alben · mp3-128`.
    source_label: String,
    /// What physically sits on the figure (newest run that reported a state).
    current: Option<String>,
    /// The most recent run, however it ended.
    last: Option<String>,
    /// CSS modifier for [`Self::last`] (`is-ok` | `is-warn` | `is-err` | `is-run`).
    last_class: &'static str,
    /// The last [`LIST_LIMIT`] runs, newest first.
    runs: Vec<SyncRow>,
}

/// One entry of the "Gehört" list.
struct SeenRow {
    album_id: String,
    title: String,
    last_used_at: String,
}

impl From<SeenAlbum> for SeenRow {
    fn from(a: SeenAlbum) -> Self {
        SeenRow {
            title: if a.album_title.trim().is_empty() {
                "(unbekannt)".to_string()
            } else {
                a.album_title
            },
            album_id: a.album_id,
            last_used_at: a.last_used_at,
        }
    }
}

#[derive(Template)]
#[template(path = "tonie_history.html")]
struct TonieHistoryPage {
    nav: crate::ui::Nav,
    flash: String,
    figures: Vec<FigureHistory>,
    seen: Vec<SeenRow>,
}

// ── handlers ────────────────────────────────────────────────────────────────

async fn history_page(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "tonies").await else {
        return Redirect::to("/login").into_response();
    };
    let uid = nav.id;

    let figures = match load_figures(&st, uid).await {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("Tonie-Verlauf fuer User {uid} nicht lesbar: {e:#}");
            return error_page("Verlauf konnte nicht geladen werden");
        }
    };

    // A broken ledger must not hide the run history above it.
    let seen = match crate::tonies::list_seen_albums(&st.pool, uid, SEEN_LIMIT).await {
        Ok(rows) => rows.into_iter().map(SeenRow::from).collect(),
        Err(e) => {
            tracing::warn!("Tonie-Album-Ledger fuer User {uid} nicht lesbar: {e:#}");
            Vec::new()
        }
    };

    render(&TonieHistoryPage {
        nav,
        flash: crate::flash::from_headers(&headers),
        figures,
        seen,
    })
}

/// `POST /tonies/history/unlisten` — put one album back on the queue.
async fn unlisten_post(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<UnlistenForm>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "tonies").await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let album_id = f.album_id.trim();
    let (msg, kind) = if album_id.is_empty() {
        ("Kein Album angegeben.".to_string(), "err")
    } else {
        match crate::tonies::unlisten_album(&st.pool, nav.id, album_id).await {
            Ok(()) => ("Album wieder auf der Liste".to_string(), "ok"),
            Err(e) => (format!("Fehler: {e}"), "err"),
        }
    };
    back_to_history(&msg, kind)
}

/// `POST /tonies/history/reset` — clear the user's whole listened ledger.
async fn reset_post(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "tonies").await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let (msg, kind) = match crate::tonies::reset_seen(&st.pool, nav.id).await {
        Ok(n) => (format!("{n} Album(e) zurueckgesetzt"), "ok"),
        Err(e) => (format!("Fehler: {e}"), "err"),
    };
    back_to_history(&msg, kind)
}

#[derive(Deserialize)]
struct UnlistenForm {
    #[serde(default)]
    album_id: String,
}

/// PRG: the toast rides on the 303 as `HX-Trigger` (for htmx) and as a
/// `hub_flash` cookie (for a plain form submit) so both paths show it once.
fn back_to_history(msg: &str, kind: &str) -> Response {
    let resp = crate::flash::redirect("/tonies/history", msg, kind);
    crate::toast::apply(resp, msg, kind)
}

// ── data ────────────────────────────────────────────────────────────────────

/// Every binding of `uid` together with its last runs. `list_syncs` is scoped
/// by `binding_id`, and those bindings all belong to `uid`.
async fn load_figures(st: &AppState, uid: i64) -> anyhow::Result<Vec<FigureHistory>> {
    let bindings = crate::tonies::list_bindings(&st.pool, uid).await?;
    let mut out = Vec::with_capacity(bindings.len());
    for b in bindings {
        // A damaged log must not blank the whole page.
        let runs = crate::tonies::list_syncs(&st.pool, b.id, LIST_LIMIT)
            .await
            .unwrap_or_default();
        out.push(figure_history(b, runs));
    }
    Ok(out)
}

fn figure_history(b: Binding, runs: Vec<SyncRow>) -> FigureHistory {
    // `chapters_after`/`seconds` are written for every finished run — including
    // `error` (they mirror `chapters_before`), so the newest row that carries
    // them is exactly the state on the figure. A `running` row has neither.
    let current = runs
        .iter()
        .find(|r| r.chapters_after.is_some())
        .map(current_line);
    let last = runs.first().map(last_line);
    let last_class = status_class(runs.first().map(|r| r.status.as_str()).unwrap_or(""));

    FigureHistory {
        name: b
            .tonie_name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or(&b.tonie_id)
            .to_string(),
        tonie_id: b.tonie_id.clone(),
        source_label: format!(
            "{} · {}",
            crate::tonies::source_kind_label(&b.source_kind),
            b.format
        ),
        current,
        last,
        last_class,
        runs,
    }
}

/// `2 Kapitel · 71 min · Stand 2026-11-18 09:12`
fn current_line(r: &SyncRow) -> String {
    format!(
        "{} Kapitel · {} min · Stand {}",
        r.chapters(),
        r.minutes(),
        r.started_label()
    )
}

/// `2026-11-18 09:12 · ok · 2 Kapitel · 71 min · +2 · -1`
fn last_line(r: &SyncRow) -> String {
    let mut s = format!("{} · {}", r.started_label(), r.status_label());
    if r.chapters_after.is_some() {
        s.push_str(&format!(
            " · {} Kapitel · {} min",
            r.chapters(),
            r.minutes()
        ));
        if r.added_count() > 0 {
            s.push_str(&format!(" · +{}", r.added_count()));
        }
        if r.removed_count() > 0 {
            s.push_str(&format!(" · -{}", r.removed_count()));
        }
    }
    s
}

/// CSS modifier for a run status; unknown tokens read as an error.
fn status_class(status: &str) -> &'static str {
    match status {
        STATUS_OK | STATUS_NOOP => "is-ok",
        STATUS_RUNNING => "is-run",
        STATUS_TIMEOUT => "is-warn",
        STATUS_ERROR => "is-err",
        _ => "is-err",
    }
}

// ── rendering ───────────────────────────────────────────────────────────────

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

fn error_page(msg: &str) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, msg.to_string()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(status: &str, chapters_after: Option<i64>) -> SyncRow {
        SyncRow {
            id: 1,
            started_at: "2026-11-18T09:12:33+00:00".to_string(),
            finished_at: Some("2026-11-18T09:12:41+00:00".to_string()),
            status: status.to_string(),
            chapters_before: Some(1),
            chapters_after,
            seconds: Some(4260),
            added: Some(2),
            removed: Some(1),
            skipped: Vec::new(),
            error: None,
        }
    }

    #[test]
    fn current_line_names_chapters_minutes_and_when() {
        assert_eq!(
            current_line(&row(STATUS_OK, Some(2))),
            "2 Kapitel · 71 min · Stand 2026-11-18 09:12"
        );
    }

    #[test]
    fn last_line_omits_figures_while_running() {
        let running = last_line(&row(STATUS_RUNNING, None));
        assert_eq!(running, "2026-11-18 09:12 · läuft");
        assert!(!running.contains("Kapitel"));
    }

    #[test]
    fn last_line_keeps_added_and_removed() {
        let ok = last_line(&row(STATUS_OK, Some(2)));
        assert!(ok.contains("2 Kapitel · 71 min"), "{ok}");
        assert!(ok.ends_with("+2 · -1"), "{ok}");
    }

    #[test]
    fn status_classes_are_stable() {
        assert_eq!(status_class(STATUS_OK), "is-ok");
        assert_eq!(status_class(STATUS_NOOP), "is-ok");
        assert_eq!(status_class(STATUS_RUNNING), "is-run");
        assert_eq!(status_class(STATUS_TIMEOUT), "is-warn");
        assert_eq!(status_class(STATUS_ERROR), "is-err");
        assert_eq!(status_class("weird"), "is-err");
        assert_eq!(status_class(""), "is-err");
    }

    #[test]
    fn a_running_run_is_never_the_current_state() {
        let running = row(STATUS_RUNNING, None);
        let finished = row(STATUS_NOOP, Some(3));
        let current = [running, finished]
            .iter()
            .find(|r| r.chapters_after.is_some())
            .map(current_line);
        assert_eq!(
            current.as_deref(),
            Some("3 Kapitel · 71 min · Stand 2026-11-18 09:12")
        );
    }

    #[test]
    fn unnamed_binding_falls_back_to_the_tonie_id() {
        let b = Binding {
            id: 7,
            tonie_id: "ct9".to_string(),
            tonie_name: Some("  ".to_string()),
            source_kind: "albums".to_string(),
            format: "mp3-128".to_string(),
            ..Default::default()
        };
        let f = figure_history(b, Vec::new());
        assert_eq!(f.name, "ct9");
        assert_eq!(f.source_label, "Alben · mp3-128");
        assert!(f.current.is_none());
        assert!(f.last.is_none());
        assert_eq!(f.last_class, "is-err");
    }
}
