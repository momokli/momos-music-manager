//! Deterministic fixtures for integration tests (issue M1-4).
//!
//! Seeds three users (`alice`, `bob`, `carol`) with hand-crafted overlaps so
//! the overlap views can be asserted with exact row counts and field values:
//!
//! * `t_all`   — liked by all three users          (shared by 3)
//! * `t_two`   — liked by alice + bob              (shared by exactly 2)
//! * `t_pl`    — in alice's + bob's playlists      (shared by exactly 2, no like)
//! * `t_alice` — liked by alice only
//! * `t_bob`   — liked by bob only
//! * `t_carol` — liked by carol only, also in carol's playlist
//!
//! Derived expectations (asserted by `tests/views.rs`):
//! * `hub_v_shared_tracks`        → 3 rows (`t_all`, `t_two`, `t_pl`)
//! * `hub_v_user_overlap`         → alice↔bob 3, alice↔carol 1, bob↔carol 1
//! * `hub_v_track_playlists`      → 4 rows
//! * `hub_v_track_presence`       → 12 rows (8 likes + 4 memberships)

use anyhow::Result;
use sqlx::SqlitePool;

/// Ids of the seeded rows, keyed by the fixture names above (for assertions).
#[derive(Debug, Clone)]
pub struct Seed {
    pub alice: i64,
    pub bob: i64,
    pub carol: i64,
    pub t_all: i64,
    pub t_two: i64,
    pub t_pl: i64,
    pub t_alice: i64,
    pub t_bob: i64,
    pub t_carol: i64,
    pub pl_alice: i64,
    pub pl_bob: i64,
    pub pl_carol: i64,
}

const LOADED_AT: &str = "2026-01-01T00:00:00+00:00";

async fn user(pool: &SqlitePool, slug: &str) -> Result<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_users (slug, display_name, created_at) VALUES (?1, ?1, ?2)
         ON CONFLICT(slug) DO UPDATE SET display_name = excluded.display_name RETURNING id",
    )
    .bind(slug)
    .bind(LOADED_AT)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

async fn track(pool: &SqlitePool, sid: &str, title: &str, artists: &str) -> Result<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_tracks (service, service_track_id, title, artists, first_seen_at)
         VALUES ('spotify', ?1, ?2, ?3, ?4)
         ON CONFLICT(service, service_track_id) DO UPDATE SET title = excluded.title RETURNING id",
    )
    .bind(sid)
    .bind(title)
    .bind(artists)
    .bind(LOADED_AT)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

async fn like(pool: &SqlitePool, user_id: i64, track_id: i64) -> Result<()> {
    sqlx::query(
        "INSERT INTO hub_liked_tracks (user_id, track_id, liked_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(user_id, track_id) DO UPDATE SET liked_at = excluded.liked_at",
    )
    .bind(user_id)
    .bind(track_id)
    .bind(LOADED_AT)
    .execute(pool)
    .await?;
    Ok(())
}

async fn playlist(
    pool: &SqlitePool,
    user_id: i64,
    pid: &str,
    name: &str,
    owned: bool,
    owner: &str,
    collaborative: bool,
) -> Result<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_playlists (user_id, service, playlist_id, name, is_liked, items_available, is_owned, owner_name, collaborative, fetched_at)
         VALUES (?1, 'spotify', ?2, ?3, 0, 1, ?4, ?5, ?6, ?7)
         ON CONFLICT(user_id, service, playlist_id) DO UPDATE SET name = excluded.name, is_owned = excluded.is_owned, owner_name = excluded.owner_name, collaborative = excluded.collaborative RETURNING id",
    )
    .bind(user_id)
    .bind(pid)
    .bind(name)
    .bind(owned as i64)
    .bind(owner)
    .bind(collaborative as i64)
    .bind(LOADED_AT)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

async fn membership(pool: &SqlitePool, playlist_id: i64, track_id: i64, pos: i64) -> Result<()> {
    sqlx::query(
        "INSERT INTO hub_playlist_tracks (playlist_id, track_id, position, added_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(playlist_id, track_id) DO UPDATE SET position = excluded.position",
    )
    .bind(playlist_id)
    .bind(track_id)
    .bind(pos)
    .bind(LOADED_AT)
    .execute(pool)
    .await?;
    Ok(())
}

/// Insert the deterministic fixture set into an already-migrated pool.
pub async fn seed(pool: &SqlitePool) -> Result<Seed> {
    let alice = user(pool, "alice").await?;
    let bob = user(pool, "bob").await?;
    let carol = user(pool, "carol").await?;

    let t_all = track(pool, "t_all", "Shared Anthem", "Aaa").await?;
    let t_two = track(pool, "t_two", "Two Users", "Bbb").await?;
    let t_pl = track(pool, "t_pl", "Playlist Only", "Ccc").await?;
    let t_alice = track(pool, "t_alice", "Alice Only", "Ddd").await?;
    let t_bob = track(pool, "t_bob", "Bob Only", "Eee").await?;
    let t_carol = track(pool, "t_carol", "Carol Only", "Fff").await?;

    // Likes: t_all ×3, t_two ×2, then one unique per user.
    for uid in [alice, bob, carol] {
        like(pool, uid, t_all).await?;
    }
    like(pool, alice, t_two).await?;
    like(pool, bob, t_two).await?;
    like(pool, alice, t_alice).await?;
    like(pool, bob, t_bob).await?;
    like(pool, carol, t_carol).await?;

    // Playlists: alice owns "Deep House", bob follows a "Deep House", carol owns "Techno".
    // alice + bob share t_pl, so a track page shows both an own and a followed playlist.
    let pl_alice = playlist(pool, alice, "pl-alice", "Deep House", true, "Alice", false).await?;
    let pl_bob = playlist(pool, bob, "pl-bob", "Deep House", false, "Bob", false).await?;
    let pl_carol = playlist(pool, carol, "pl-carol", "Techno", true, "Carol", false).await?;

    // A collaborative playlist shared by alice + bob (same Spotify id across users).
    playlist(pool, alice, "pl-shared", "Shared Collab", true, "Alice", true).await?;
    playlist(pool, bob, "pl-shared", "Shared Collab", true, "Alice", true).await?;

    membership(pool, pl_alice, t_all, 0).await?;
    membership(pool, pl_alice, t_pl, 1).await?;
    membership(pool, pl_bob, t_pl, 0).await?;
    membership(pool, pl_carol, t_carol, 0).await?;

    Ok(Seed {
        alice,
        bob,
        carol,
        t_all,
        t_two,
        t_pl,
        t_alice,
        t_bob,
        t_carol,
        pl_alice,
        pl_bob,
        pl_carol,
    })
}
