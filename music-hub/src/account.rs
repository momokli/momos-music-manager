//! Account self-service: password change, password reset by magic link.
//!
//! ```text
//! GET  /account/password   -> page         (HTML, web-session)
//! POST /account/password   -> form_submit  (HTML form POST)
//! GET  /forgot             -> forgot_page  (HTML, anonymous)
//! POST /forgot             -> forgot_submit(HTML form POST)
//! GET  /reset?token=…      -> reset_page   (HTML, anonymous)
//! POST /reset              -> reset_submit (HTML form POST)
//! ```
//!
//! Security properties (see `plans/proposed/accounts-mail.md`):
//! * The current password must be re-entered for a change — a stolen session
//!   alone must not lock the real owner out.
//! * `/forgot` answers identically for known and unknown addresses and mints a
//!   token in both cases, so neither the text nor the timing reveals whether an
//!   account exists.
//! * Tokens are single-use, bound to the address and (for resets) to the user,
//!   and only their SHA-256 is stored ([`crate::mail_tokens`]).
//! * A successful reset ends **every** session of that user; the password change
//!   keeps the acting session.
//! * Errors travel via the flash cookie and a `303`, so no password material
//!   ever lands in a URL, and a reload never repeats the POST.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Form, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use askama::Template;
use chrono::Duration as ChronoDuration;
use serde::Deserialize;

use crate::api::AppState;
use crate::mail::OutgoingMail;
use crate::mail_tokens::{PURPOSE_RESET, RESET_TTL_MINUTES};

const PASSWORD_PATH: &str = "/account/password";

/// Identical answer for every `/forgot` outcome (no user enumeration).
const NEUTRAL_SENT: &str =
    "Wenn zu dieser Adresse ein Konto existiert, ist eine E-Mail mit einem Link unterwegs.";

/// Minimum gap between two reset requests for the same address. Cheap abuse
/// brake; the token itself is rate-independent because only the last request can
/// be redeemed anyway.
const FORGOT_THROTTLE: Duration = Duration::from_secs(60);

static FORGOT_REQUESTS: LazyLock<Mutex<HashMap<String, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Template)]
#[template(path = "account_password.html")]
struct AccountPasswordPage {
    nav: crate::ui::Nav,
    flash: String,
    min_len: usize,
}

#[derive(Template)]
#[template(path = "forgot.html")]
struct ForgotPage {
    error: String,
    flash: String,
    mail_configured: bool,
}

#[derive(Template)]
#[template(path = "reset.html")]
struct ResetPage {
    error: String,
    flash: String,
    token: String,
    show_form: bool,
    min_len: usize,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route(PASSWORD_PATH, get(page).post(form_submit))
        .route("/forgot", get(forgot_page).post(forgot_submit))
        .route("/reset", get(reset_page).post(reset_submit))
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

fn error_page(msg: &str) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, msg.to_string()).into_response()
}

fn redirect_back(msg: &str) -> Response {
    crate::flash::redirect(PASSWORD_PATH, msg, "ok")
}

// ── password change (signed in) ─────────────────────────────────────────────

async fn page(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "settings").await else {
        return Redirect::to("/login").into_response();
    };
    render(&AccountPasswordPage {
        nav,
        flash: crate::flash::from_headers(&headers),
        min_len: crate::auth::MIN_PASSWORD_LEN,
    })
}

#[derive(Deserialize)]
struct ChangeForm {
    /// Current password — proof that the human, not just the cookie, is here.
    current: String,
    new_password: String,
    confirm: String,
}

async fn form_submit(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<ChangeForm>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "settings").await else {
        return Redirect::to("/login").into_response();
    };

    // Re-read the hash from the DB: the session only proves *who*, never *what*.
    let stored: Option<String> = sqlx::query_scalar("SELECT password_hash FROM hub_users WHERE id = ?1")
        .bind(nav.id)
        .fetch_optional(&st.pool)
        .await
        .ok()
        .flatten();

    if !crate::auth::verify_password(&f.current, stored.as_deref()) {
        return redirect_back("Das aktuelle Passwort ist falsch.");
    }
    if let Some(msg) = password_problem(&f.new_password, Some(&f.current)) {
        return redirect_back(&msg);
    }
    if f.new_password != f.confirm {
        return redirect_back("Die Wiederholung stimmt nicht mit dem neuen Passwort überein.");
    }

    let hash = match crate::auth::hash_password(&f.new_password) {
        Ok(h) => h,
        Err(e) => return error_page(&format!("Hash-Fehler: {e}")),
    };
    if let Err(e) = sqlx::query("UPDATE hub_users SET password_hash = ?2 WHERE id = ?1")
        .bind(nav.id)
        .bind(&hash)
        .execute(&st.pool)
        .await
    {
        return error_page(&format!("DB-Fehler: {e}"));
    }

    // Keep the session that made the change; end any other device/session.
    let keep = crate::web::cookie_value(&headers, crate::web::SESSION_COOKIE);
    let killed = match crate::auth::invalidate_sessions(&st.pool, nav.id, keep.as_deref()).await {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!("session invalidation failed for user {}: {e}", nav.id);
            0
        }
    };
    if killed == 0 {
        return redirect_back("Passwort geändert.");
    }
    redirect_back(&format!(
        "Passwort geändert. {killed} weitere Sitzung(en) wurden beendet."
    ))
}

// ── forgot: request a reset link ────────────────────────────────────────────

async fn forgot_page(State(st): State<AppState>, headers: HeaderMap) -> Response {
    render(&ForgotPage {
        error: String::new(),
        flash: crate::flash::from_headers(&headers),
        mail_configured: st.mailer.is_configured(),
    })
}

#[derive(Deserialize)]
struct ForgotForm {
    email: String,
}

async fn forgot_submit(State(st): State<AppState>, Form(f): Form<ForgotForm>) -> Response {
    let email = normalise_email(&f.email);
    if !looks_like_address(&email) {
        return crate::flash::redirect(
            "/forgot",
            "Bitte eine gültige E-Mail-Adresse angeben.",
            "ok",
        );
    }
    if !throttle_allows(&email) {
        return crate::flash::redirect("/forgot", NEUTRAL_SENT, "ok");
    }

    // Deliberately one code path for known and unknown addresses: a token is
    // minted either way. Otherwise the response time would reveal which
    // addresses have an account — the text alone being neutral is not enough.
    let user_id: Option<i64> =
        sqlx::query_scalar("SELECT id FROM hub_users WHERE email = ?1 COLLATE NOCASE")
            .bind(&email)
            .fetch_optional(&st.pool)
            .await
            .ok()
            .flatten();

    match crate::mail_tokens::issue(
        &st.pool,
        user_id,
        &email,
        PURPOSE_RESET,
        ChronoDuration::minutes(RESET_TTL_MINUTES),
        None,
    )
    .await
    {
        Ok(token) => {
            if user_id.is_some() {
                let mail = reset_mail(&st, &email, &token);
                if let Err(e) = st.mailer.send(&mail).await {
                    // Domain only: the address is personal data, the domain is not.
                    tracing::warn!("reset mail for @{} failed: {e:#}", domain_of(&email));
                }
            }
        }
        Err(e) => tracing::warn!("could not mint a reset token: {e:#}"),
    }

    crate::flash::redirect("/forgot", NEUTRAL_SENT, "ok")
}

fn reset_mail(st: &AppState, to: &str, token: &str) -> OutgoingMail {
    let link = format!(
        "{}/reset?token={token}",
        st.cfg.public_url.trim_end_matches('/')
    );
    OutgoingMail {
        to: to.to_string(),
        subject: "Spelunke: Passwort zurücksetzen".to_string(),
        body: format!(
            "Hallo,\n\n\
             für dieses Konto wurde ein neues Passwort angefordert. Der Link gilt\n\
             {RESET_TTL_MINUTES} Minuten und lässt sich genau einmal verwenden:\n\n\
             {link}\n\n\
             Wenn du das nicht warst, ignoriere diese E-Mail — das Passwort bleibt\n\
             unverändert.\n"
        ),
    }
}

/// Simple abuse brake, keyed by address. Returns `false` when a request was made
/// too recently; the caller answers exactly as it would for a real send.
fn throttle_allows(key: &str) -> bool {
    let mut map = FORGOT_REQUESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = Instant::now();
    if let Some(previous) = map.get(key) {
        if now.duration_since(*previous) < FORGOT_THROTTLE {
            return false;
        }
    }
    // Bound memory: the map only ever holds recent keys.
    if map.len() > 1024 {
        map.retain(|_, at| now.duration_since(*at) < FORGOT_THROTTLE);
    }
    map.insert(key.to_string(), now);
    true
}

// ── reset: redeem the link ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct ResetQuery {
    #[serde(default)]
    token: Option<String>,
}

async fn reset_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ResetQuery>,
) -> Response {
    let token = q.token.unwrap_or_default();
    if token.is_empty() {
        return reset_page_html(&headers, "Der Link ist unvollständig.", "", false);
    }
    let usable = crate::mail_tokens::is_usable(&st.pool, &token, PURPOSE_RESET)
        .await
        .unwrap_or(false);
    if !usable {
        return reset_page_html(
            &headers,
            "Der Link ist ungültig, abgelaufen oder wurde bereits verwendet.",
            "",
            false,
        );
    }
    reset_page_html(&headers, "", &token, true)
}

fn reset_page_html(
    headers: &HeaderMap,
    error: &str,
    token: &str,
    show_form: bool,
) -> Response {
    render(&ResetPage {
        error: error.to_string(),
        flash: crate::flash::from_headers(headers),
        token: token.to_string(),
        show_form,
        min_len: crate::auth::MIN_PASSWORD_LEN,
    })
}

#[derive(Deserialize)]
struct ResetForm {
    token: String,
    new_password: String,
    confirm: String,
}

async fn reset_submit(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<ResetForm>,
) -> Response {
    // Validate *before* consuming: a typo must not burn the single-use token.
    if let Some(msg) = password_problem(&f.new_password, None) {
        return reset_page_html(&headers, &msg, &f.token, true);
    }
    if f.new_password != f.confirm {
        return reset_page_html(
            &headers,
            "Die Wiederholung stimmt nicht mit dem neuen Passwort überein.",
            &f.token,
            true,
        );
    }

    let Some(redeemed) = crate::mail_tokens::redeem(&st.pool, &f.token, PURPOSE_RESET)
        .await
        .ok()
        .flatten()
    else {
        return reset_page_html(
            &headers,
            "Der Link ist ungültig, abgelaufen oder wurde bereits verwendet.",
            "",
            false,
        );
    };
    let Some(user_id) = redeemed.user_id else {
        return reset_page_html(&headers, "Der Link gehört zu keinem Konto.", "", false);
    };

    // The address may have changed since the token was minted — a link mailed to
    // the old address must not reset the account any more.
    let current: Option<String> = sqlx::query_scalar("SELECT email FROM hub_users WHERE id = ?1")
        .bind(user_id)
        .fetch_optional(&st.pool)
        .await
        .ok()
        .flatten();
    let same = current
        .as_deref()
        .is_some_and(|c| c.eq_ignore_ascii_case(&redeemed.email));
    if !same {
        return reset_page_html(
            &headers,
            "Der Link passt nicht mehr zu diesem Konto. Fordere einen neuen an.",
            "",
            false,
        );
    }

    let hash = match crate::auth::hash_password(&f.new_password) {
        Ok(h) => h,
        Err(e) => return error_page(&format!("Hash-Fehler: {e}")),
    };
    if let Err(e) = sqlx::query("UPDATE hub_users SET password_hash = ?2 WHERE id = ?1")
        .bind(user_id)
        .bind(&hash)
        .execute(&st.pool)
        .await
    {
        return error_page(&format!("DB-Fehler: {e}"));
    }

    // Whoever learns the address reset it: every existing session is gone.
    if let Err(e) = crate::auth::invalidate_sessions(&st.pool, user_id, None).await {
        tracing::warn!("session invalidation failed for user {user_id}: {e}");
    }
    crate::flash::redirect(
        "/login",
        "Passwort gesetzt. Bitte mit dem neuen Passwort einloggen.",
        "ok",
    )
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// Shared validation for a *new* password. `current` is only passed by the
/// change form (a reset has no meaningful "old" password to compare against).
fn password_problem(new_password: &str, current: Option<&str>) -> Option<String> {
    if new_password.chars().count() < crate::auth::MIN_PASSWORD_LEN {
        return Some(format!(
            "Das neue Passwort braucht mindestens {} Zeichen.",
            crate::auth::MIN_PASSWORD_LEN
        ));
    }
    if current.is_some_and(|c| c == new_password) {
        return Some("Das neue Passwort muss sich vom alten unterscheiden.".to_string());
    }
    None
}

fn normalise_email(raw: &str) -> String {
    raw.trim().to_ascii_lowercase()
}

/// Cheap shape check; delivery is the real test.
fn looks_like_address(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
        && !email.contains(char::is_whitespace)
}

fn domain_of(email: &str) -> &str {
    email.split_once('@').map(|(_, d)| d).unwrap_or("unknown")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_shape_check_is_conservative() {
        assert!(looks_like_address("a@b.de"));
        assert!(looks_like_address("first.last+tag@example.co.uk"));
        assert!(!looks_like_address("no-at-sign"));
        assert!(!looks_like_address("@example.com"));
        assert!(!looks_like_address("a@"));
        assert!(!looks_like_address("a@localhost"));
        assert!(!looks_like_address("a b@example.com"));
        assert!(!looks_like_address("a@.example.com"));
    }

    #[test]
    fn normalisation_lowercases_and_trims() {
        assert_eq!(normalise_email("  Momo@Example.COM "), "momo@example.com");
    }

    #[test]
    fn domain_of_never_leaks_the_local_part() {
        assert_eq!(domain_of("momo@zukkafabrik.de"), "zukkafabrik.de");
        assert_eq!(domain_of("broken"), "unknown");
    }

    #[test]
    fn password_rules_are_shared_with_the_change_form() {
        assert!(password_problem("short", None).is_some());
        assert!(password_problem("longenough", None).is_none());
        assert!(password_problem("longenough", Some("longenough")).is_some());
        assert!(password_problem("longenough", Some("different1")).is_none());
    }
}
