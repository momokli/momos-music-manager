//! HTTP surface for T2: health, login/logout, protected placeholder home, 404.

use std::sync::Arc;

use askama::Template;
use axum::extract::{Form, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::auth;
use crate::config::Config;
use crate::templates::{ErrorTemplate, HomeTemplate, LoginTemplate};

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub config: Arc<Config>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/login", get(login_form).post(login_submit))
        .route("/logout", get(logout).post(logout))
        .route("/", get(index))
        .fallback(not_found)
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

async fn not_found() -> Response {
    let mut resp = render(&ErrorTemplate {
        message: "Diese Seite gibt es nicht.".to_string(),
    });
    *resp.status_mut() = StatusCode::NOT_FOUND;
    resp
}

/// Unprotected liveness probe backed by a real DB query.
async fn health(State(st): State<AppState>) -> Response {
    match sqlx::query("SELECT 1").execute(&st.pool).await {
        Ok(_) => Json(serde_json::json!({ "status": "ok" })).into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "status": "down" })),
        )
            .into_response(),
    }
}

async fn login_form(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if auth::current_user(&st.pool, &headers).await.is_some() {
        return Redirect::to("/").into_response();
    }
    render(&LoginTemplate {
        error: String::new(),
    })
}

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
}

async fn login_submit(
    State(st): State<AppState>,
    Form(form): Form<LoginForm>,
) -> Response {
    let user: Option<(i64, String)> =
        sqlx::query_as("SELECT id, password_hash FROM users WHERE slug = ?1")
            .bind(form.username.trim())
            .fetch_optional(&st.pool)
            .await
            .ok()
            .flatten();

    let ok = user
        .as_ref()
        .map(|(_, hash)| auth::verify_password(&form.password, hash))
        .unwrap_or(false);

    let Some((user_id, _)) = user.filter(|_| ok) else {
        let mut resp = render(&LoginTemplate {
            error: "Benutzername oder Passwort falsch.".to_string(),
        });
        *resp.status_mut() = StatusCode::UNAUTHORIZED;
        return resp;
    };

    match auth::create_session(&st.pool, user_id).await {
        Ok(token) => auth::with_cookie(
            Redirect::to("/").into_response(),
            auth::session_cookie(&token, st.config.cookie_secure),
        ),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Session konnte nicht angelegt werden.",
        )
            .into_response(),
    }
}

async fn logout(State(_st): State<AppState>) -> Response {
    auth::with_cookie(
        Redirect::to("/login").into_response(),
        auth::clear_cookie(),
    )
}

async fn index(State(st): State<AppState>, headers: HeaderMap) -> Response {
    match auth::require_user(&st.pool, &headers).await {
        Ok((_, slug)) => render(&HomeTemplate { user_slug: slug }),
        Err(redirect) => redirect,
    }
}
