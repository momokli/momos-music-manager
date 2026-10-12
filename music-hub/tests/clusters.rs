//! Tag-cluster tests (#261–#268).

mod common;

use music_hub::clusters;

#[tokio::test]
async fn clusters_page_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .get(app.url("/tag-clusters"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/login");
}

#[tokio::test]
async fn rebuild_groups_variant_tags_and_page_shows_them() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("zoe", "pw").await;
    for (slug, name) in [("dark", "dark"), ("darker", "darker"), ("peak", "peak")] {
        sqlx::query(
            "INSERT INTO hub_tags (owner_user_id, slug, name, type, created_at)
             VALUES (?1, ?2, ?3, 'MOOD', '2026-01-01T00:00:00+00:00')",
        )
        .bind(uid)
        .bind(slug)
        .bind(name)
        .execute(&app.pool)
        .await
        .unwrap();
    }

    let made = clusters::rebuild(&app.pool, "MOOD").await.unwrap();
    assert_eq!(made, 1, "dark + darker cluster; peak alone");

    let members: Vec<(String, String)> = sqlx::query_as(
        "SELECT c.label, t.name FROM hub_tag_clusters c
           JOIN hub_tag_cluster_members m ON m.cluster_id = c.id
           JOIN hub_tags t ON t.id = m.tag_id
          WHERE c.tag_type = 'MOOD' ORDER BY t.name",
    )
    .fetch_all(&app.pool)
    .await
    .unwrap();
    let names: Vec<String> = members.iter().map(|(_, n)| n.clone()).collect();
    assert_eq!(names, vec!["dark".to_string(), "darker".to_string()]);

    let resp = app
        .client()
        .get(app.url("/tag-clusters?type=MOOD"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.unwrap();
    assert!(html.contains("Tag-Cluster"), "heading missing");
    assert!(html.contains("dark"), "cluster label missing");
}
