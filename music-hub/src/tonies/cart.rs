//! Tonie cart: a persistent draft of what should go on one Kreativ-Tonie
//! (`plans/proposed/tonie-album-queue.md`, §UX/IA).
//!
//! One cart per `(user, tonie)`, independent of a binding: you can fill it
//! before any source is configured, and it survives a device switch. The
//! `album_mode = basket` resolver reads exactly this list, so what the user
//! sees in the cart is what the tonie gets.

use anyhow::{Context, Result};
use sqlx::{Row, SqlitePool};

use super::bindings::AlbumItem;
use super::now_iso;

/// The cart of `tonie_id`, in user order.
pub async fn list(pool: &SqlitePool, user_id: i64, tonie_id: &str) -> Result<Vec<AlbumItem>> {
    let rows = sqlx::query(
        "SELECT album_id, IFNULL(title, '') AS album_title,
                IFNULL(seconds, 0) AS seconds, position
           FROM hub_tonie_cart_items
          WHERE user_id = ?1 AND tonie_id = ?2
          ORDER BY position, id",
    )
    .bind(user_id)
    .bind(tonie_id)
    .fetch_all(pool)
    .await
    .context("Tonie-Korb konnte nicht gelesen werden")?;

    Ok(rows
        .into_iter()
        .map(|r| AlbumItem {
            album_id: r.get("album_id"),
            album_title: r.get("album_title"),
            seconds: r.get("seconds"),
            position: r.get("position"),
        })
        .collect())
}

/// Replace the whole cart in the given order. Duplicates are dropped by the
/// unique key (first occurrence wins).
pub async fn replace_all(
    pool: &SqlitePool,
    user_id: i64,
    tonie_id: &str,
    items: &[AlbumItem],
) -> Result<()> {
    let ts = now_iso();
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM hub_tonie_cart_items WHERE user_id = ?1 AND tonie_id = ?2")
        .bind(user_id)
        .bind(tonie_id)
        .execute(&mut *tx)
        .await
        .context("Tonie-Korb konnte nicht geleert werden")?;

    for (position, item) in items.iter().enumerate() {
        sqlx::query(
            "INSERT INTO hub_tonie_cart_items
                 (user_id, tonie_id, position, album_id, title, cover_url, seconds, added_at)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7)
             ON CONFLICT(user_id, tonie_id, album_id) DO NOTHING",
        )
        .bind(user_id)
        .bind(tonie_id)
        .bind(position as i64)
        .bind(&item.album_id)
        .bind(&item.album_title)
        .bind(item.seconds)
        .bind(&ts)
        .execute(&mut *tx)
        .await
        .context("Tonie-Korb konnte nicht geschrieben werden")?;
    }
    tx.commit().await?;
    Ok(())
}

/// Append one album. Returns `false` when it is already in the cart.
#[allow(clippy::too_many_arguments)]
pub async fn add(
    pool: &SqlitePool,
    user_id: i64,
    tonie_id: &str,
    album_id: &str,
    title: &str,
    cover_url: Option<&str>,
    seconds: i64,
) -> Result<bool> {
    let next: i64 = sqlx::query_scalar(
        "SELECT IFNULL(MAX(position) + 1, 0) FROM hub_tonie_cart_items
          WHERE user_id = ?1 AND tonie_id = ?2",
    )
    .bind(user_id)
    .bind(tonie_id)
    .fetch_one(pool)
    .await
    .context("Tonie-Korb konnte nicht gelesen werden")?;

    let res = sqlx::query(
        "INSERT INTO hub_tonie_cart_items
             (user_id, tonie_id, position, album_id, title, cover_url, seconds, added_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(user_id, tonie_id, album_id) DO NOTHING",
    )
    .bind(user_id)
    .bind(tonie_id)
    .bind(next)
    .bind(album_id)
    .bind(title)
    .bind(cover_url)
    .bind(seconds)
    .bind(now_iso())
    .execute(pool)
    .await
    .context("Album konnte nicht in den Korb gelegt werden")?;

    Ok(res.rows_affected() > 0)
}

/// Empty one cart.
pub async fn clear(pool: &SqlitePool, user_id: i64, tonie_id: &str) -> Result<u64> {
    let res = sqlx::query("DELETE FROM hub_tonie_cart_items WHERE user_id = ?1 AND tonie_id = ?2")
        .bind(user_id)
        .bind(tonie_id)
        .execute(pool)
        .await
        .context("Tonie-Korb konnte nicht geleert werden")?;
    Ok(res.rows_affected())
}

/// How many albums sit in one cart (drives the badge/counter).
pub async fn count(pool: &SqlitePool, user_id: i64, tonie_id: &str) -> Result<i64> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_tonie_cart_items WHERE user_id = ?1 AND tonie_id = ?2",
    )
    .bind(user_id)
    .bind(tonie_id)
    .fetch_one(pool)
    .await
    .context("Tonie-Korb konnte nicht gezaehlt werden")
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> SqlitePool {
        let pool = crate::db::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "INSERT INTO hub_users (slug, display_name, created_at)
             VALUES ('u', 'u', '2026-01-01T00:00:00+00:00')",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn item(id: &str) -> AlbumItem {
        AlbumItem {
            album_id: id.to_string(),
            album_title: format!("Album {id}"),
            seconds: 600,
            position: 0,
        }
    }

    #[tokio::test]
    async fn cart_is_per_tonie_and_user() {
        let pool = pool().await;
        add(&pool, 1, "ct1", "a", "A", None, 600).await.unwrap();
        add(&pool, 1, "ct2", "b", "B", None, 600).await.unwrap();

        assert_eq!(list(&pool, 1, "ct1").await.unwrap().len(), 1);
        assert_eq!(list(&pool, 1, "ct2").await.unwrap().len(), 1);
        // Another user sees nothing.
        assert!(list(&pool, 2, "ct1").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn duplicate_album_is_rejected_but_kept_once() {
        let pool = pool().await;
        assert!(add(&pool, 1, "ct1", "a", "A", None, 600).await.unwrap());
        assert!(!add(&pool, 1, "ct1", "a", "A ", None, 600).await.unwrap());
        assert_eq!(count(&pool, 1, "ct1").await.unwrap(), 1);
    }

    #[tokio::test]
    async fn order_is_the_user_order() {
        let pool = pool().await;
        replace_all(&pool, 1, "ct1", &[item("x"), item("y"), item("z")])
            .await
            .unwrap();
        let ids: Vec<String> = list(&pool, 1, "ct1")
            .await
            .unwrap()
            .into_iter()
            .map(|i| i.album_id)
            .collect();
        assert_eq!(ids, vec!["x", "y", "z"]);

        replace_all(&pool, 1, "ct1", &[item("z"), item("x")])
            .await
            .unwrap();
        let ids: Vec<String> = list(&pool, 1, "ct1")
            .await
            .unwrap()
            .into_iter()
            .map(|i| i.album_id)
            .collect();
        assert_eq!(ids, vec!["z", "x"]);
    }

    #[tokio::test]
    async fn clear_only_touches_one_cart() {
        let pool = pool().await;
        add(&pool, 1, "ct1", "a", "A", None, 600).await.unwrap();
        add(&pool, 1, "ct2", "b", "B", None, 600).await.unwrap();
        assert_eq!(clear(&pool, 1, "ct1").await.unwrap(), 1);
        assert!(list(&pool, 1, "ct1").await.unwrap().is_empty());
        assert_eq!(list(&pool, 1, "ct2").await.unwrap().len(), 1);
    }
}
