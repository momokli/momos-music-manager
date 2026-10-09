//! Shared UI shell data for the askama templates.
//!
//! Every rendered page extends `base.html`, which references `nav` (topbar,
//! user menu, active state) and `flash` (a one-shot message). `nav(...)`
//! resolves both from the session; `None` means "not logged in" and the handler
//! should redirect to `/login`.

use axum::http::HeaderMap;
use sqlx::Row;

use crate::api::AppState;

/// Topbar + user-menu data every shell page needs.
#[derive(Clone, Debug, Default)]
pub struct Nav {
    pub id: i64,
    pub slug: String,
    /// Which nav item is active: `dashboard` | `playlists` | `search` | `sql`.
    pub active: String,
    pub spotify_connected: bool,
    pub spotify_label: String,
}

impl Nav {
    pub fn is_active(&self, key: &str) -> bool {
        self.active == key
    }
}

/// Resolve the shell nav for the current session, or `None` when not logged in.
pub async fn nav(st: &AppState, headers: &HeaderMap, active: &str) -> Option<Nav> {
    let (user_id, slug) = crate::web::current_user(st, headers).await?;

    let row = sqlx::query(
        "SELECT display_name, access_token FROM hub_service_accounts
          WHERE user_id = ?1 AND service = 'spotify'",
    )
    .bind(user_id)
    .fetch_optional(&st.pool)
    .await
    .ok()
    .flatten();

    let (spotify_connected, spotify_label) = match row {
        Some(r) => {
            let access: Option<String> = r.get("access_token");
            let name: Option<String> = r.get("display_name");
            (
                access.as_deref().map(|a| !a.is_empty()).unwrap_or(false),
                name.unwrap_or_default(),
            )
        }
        None => (false, String::new()),
    };

    Some(Nav {
        id: user_id,
        slug,
        active: active.to_string(),
        spotify_connected,
        spotify_label,
    })
}
