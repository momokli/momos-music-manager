//! CLI subcommands for Deemix actions.

use std::str::FromStr;
use std::time::Duration;

use anyhow::Result;
use clap::Subcommand;
use sqlx::{Pool, Sqlite, SqlitePool, sqlite::SqliteConnectOptions};

use crate::deemix::{DeemixClient, models::DeemixCombinedQueueItem};

#[derive(Subcommand, Debug)]
pub enum DeemixCommand {
    Auth {
        arl: String,
        #[arg(default_value = "http://localhost:6596")]
        host: String,
    },
    Status,
    Queue,
    Add {
        url: String,
    },
    Retry {
        id: i64,
    },
    Delete {
        id: i64,
    },
}

#[allow(clippy::collapsible_if)]
fn resolve_db_url() -> String {
    if let Ok(url) = std::env::var("DATABASE_URL") {
        return url;
    }
    if let Some(config_dir) = dirs::config_dir() {
        let p = config_dir.join("momos-music-manager").join("config.toml");
        if let Ok(c) = std::fs::read_to_string(&p) {
            if let Ok(t) = c.parse::<toml::Table>() {
                if let Some(u) = t
                    .get("database")
                    .and_then(|v| v.as_table())
                    .and_then(|d| d.get("url"))
                    .and_then(|v| v.as_str())
                {
                    return u.to_string();
                }
            }
        }
    }
    "sqlite:./app.db".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Command;
    use std::env;

    fn deemix_cmd() -> clap::Command {
        DeemixCommand::augment_subcommands(Command::new("deemix"))
    }

    #[test]
    fn test_deemix_command_auth() {
        let matches = deemix_cmd()
            .try_get_matches_from(vec!["deemix", "auth", "my_arl_token"])
            .unwrap();
        let sub = matches.subcommand_matches("auth").unwrap();
        let arl: &String = sub.get_one::<String>("arl").unwrap();
        assert_eq!(arl, "my_arl_token");
    }

    #[test]
    fn test_deemix_command_auth_with_host() {
        let matches = deemix_cmd()
            .try_get_matches_from(vec![
                "deemix",
                "auth",
                "my_arl_token",
                "http://localhost:9999",
            ])
            .unwrap();
        let sub = matches.subcommand_matches("auth").unwrap();
        let host: &String = sub.get_one::<String>("host").unwrap();
        assert_eq!(host, "http://localhost:9999");
    }

    #[test]
    fn test_deemix_command_status() {
        let matches = deemix_cmd()
            .try_get_matches_from(vec!["deemix", "status"])
            .unwrap();
        assert_eq!(matches.subcommand_name(), Some("status"));
    }

    #[test]
    fn test_deemix_command_queue() {
        let matches = deemix_cmd()
            .try_get_matches_from(vec!["deemix", "queue"])
            .unwrap();
        assert_eq!(matches.subcommand_name(), Some("queue"));
    }

    #[test]
    fn test_deemix_command_add() {
        let matches = deemix_cmd()
            .try_get_matches_from(vec![
                "deemix",
                "add",
                "https://open.spotify.com/playlist/abc",
            ])
            .unwrap();
        let sub = matches.subcommand_matches("add").unwrap();
        let url: &String = sub.get_one::<String>("url").unwrap();
        assert_eq!(url, "https://open.spotify.com/playlist/abc");
    }

    #[test]
    fn test_deemix_command_retry() {
        let matches = deemix_cmd()
            .try_get_matches_from(vec!["deemix", "retry", "42"])
            .unwrap();
        let sub = matches.subcommand_matches("retry").unwrap();
        let id: i64 = *sub.get_one::<i64>("id").unwrap();
        assert_eq!(id, 42);
    }

    #[test]
    fn test_deemix_command_delete() {
        let matches = deemix_cmd()
            .try_get_matches_from(vec!["deemix", "delete", "99"])
            .unwrap();
        let sub = matches.subcommand_matches("delete").unwrap();
        let id: i64 = *sub.get_one::<i64>("id").unwrap();
        assert_eq!(id, 99);
    }

    #[test]
    fn test_resolve_db_url_from_env() {
        // Use a single test to avoid race condition with env var manipulation
        unsafe { env::set_var("DATABASE_URL", "sqlite:/tmp/test.db") };
        let url = resolve_db_url();
        assert_eq!(url, "sqlite:/tmp/test.db");
        unsafe { env::remove_var("DATABASE_URL") };

        // Now test default (no env var set)
        let url2 = resolve_db_url();
        assert_eq!(url2, "sqlite:./app.db");
    }

    #[test]
    fn test_deemix_command_enum_debug() {
        let cmd = DeemixCommand::Status;
        let debug = format!("{:?}", cmd);
        assert_eq!(debug, "Status");
    }
}

async fn connect_db() -> Result<Pool<Sqlite>> {
    let u = resolve_db_url();
    Ok(SqlitePool::connect_with(
        SqliteConnectOptions::from_str(&u)?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5))
            .synchronous(sqlx::sqlite::SqliteSynchronous::Normal),
    )
    .await?)
}

async fn load_deemix(
    db: &Pool<Sqlite>,
) -> Result<(DeemixClient, Option<crate::deemix::DeemixLoginResponse>)> {
    use sqlx::Row;
    let r = sqlx::query(
        "SELECT access_token, metadata_json FROM service_config WHERE service='deemix'",
    )
    .fetch_optional(db)
    .await?
    .ok_or_else(|| anyhow::anyhow!("Deemix not configured"))?;
    let arl: String = r.try_get("access_token")?;
    let host = r
        .try_get::<Option<String>, _>("metadata_json")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s.as_str()).ok())
        .and_then(|v| v.get("host").and_then(|h| h.as_str().map(String::from)))
        .unwrap_or_else(|| "http://localhost:6596".to_string());
    let c = DeemixClient::new(&host, db.clone());
    let l = c.login_arl(&arl).await.ok();
    Ok((c, l))
}

pub async fn run(cmd: DeemixCommand) -> Result<()> {
    match cmd {
        DeemixCommand::Auth { arl, host } => {
            let db = connect_db().await?;
            let c = DeemixClient::new(&host, db.clone());
            let r = c.login_arl(&arl).await?;
            let m = serde_json::json!({"host": host});
            let now = chrono::Utc::now().timestamp();
            sqlx::query("INSERT INTO service_config(service,access_token,metadata_json,is_connected,last_checked,updated_at,created_at)VALUES('deemix',?,?,1,?,?,COALESCE((SELECT created_at FROM service_config WHERE service='deemix'),?))ON CONFLICT(service)DO UPDATE SET access_token=excluded.access_token,metadata_json=excluded.metadata_json,is_connected=1,last_checked=excluded.last_checked,updated_at=excluded.updated_at").bind(&arl).bind(m.to_string()).bind(now).bind(now).bind(now).execute(&db).await?;
            println!("✅ Connected as {} @ {}", r.user.name, host);
        }
        DeemixCommand::Status => {
            let db = connect_db().await?;
            let (c, l) = load_deemix(&db).await?;
            if let Some(ref r) = l {
                println!("User: {}", r.user.name);
            }
            match c.get_queue().await {
                Ok(q) => println!("Queue: {} items", q.len()),
                Err(e) => println!("Queue error: {e}"),
            }
        }
        DeemixCommand::Queue => {
            let db = connect_db().await?;
            let (c, _) = load_deemix(&db).await?;
            #[allow(clippy::type_complexity)]
            let local: Vec<(i64, String, Option<String>, String, i64, i64, Option<String>, Option<i64>, Option<i64>)> = sqlx::query_as("SELECT id,spotify_playlist_url,playlist_name,status,track_count_total,track_count_downloaded,error_message,created_at,updated_at FROM deemix_downloads ORDER BY updated_at DESC").fetch_all(&db).await.unwrap_or_default();
            let remote = c.get_queue().await.unwrap_or_default();
            let mut combined: Vec<DeemixCombinedQueueItem> = local
                .into_iter()
                .map(
                    |(id, url, name, st, total, dl, err, cr, up)| DeemixCombinedQueueItem {
                        id: Some(id),
                        uuid: None,
                        spotify_playlist_url: Some(url),
                        playlist_name: name,
                        status: st,
                        track_count_total: total,
                        track_count_downloaded: dl,
                        error_message: err,
                        created_at: cr,
                        updated_at: up,
                        title: None,
                        artist: None,
                        progress: 0,
                    },
                )
                .collect();
            for (uuid, item) in &remote {
                let url = format!("https://open.spotify.com/playlist/{}", item.id);
                let st = match item.status.as_str() {
                    "completed" | "withErrors" => "completed",
                    "downloading" => "downloading",
                    _ => "queued",
                };
                if let Some(e) = combined
                    .iter_mut()
                    .find(|c| c.spotify_playlist_url.as_deref() == Some(&url))
                {
                    e.uuid = Some(uuid.clone());
                    e.status = st.to_string();
                    e.track_count_total = item.size;
                    e.track_count_downloaded = item.downloaded;
                    e.progress = item.progress;
                    e.title = Some(item.title.clone());
                    e.artist = Some(item.artist.clone());
                } else {
                    combined.push(DeemixCombinedQueueItem {
                        id: None,
                        uuid: Some(uuid.clone()),
                        spotify_playlist_url: Some(url),
                        playlist_name: Some(item.title.clone()),
                        status: st.to_string(),
                        track_count_total: item.size,
                        track_count_downloaded: item.downloaded,
                        error_message: None,
                        created_at: None,
                        updated_at: None,
                        title: Some(item.title.clone()),
                        artist: Some(item.artist.clone()),
                        progress: item.progress,
                    });
                }
            }
            println!("{}", serde_json::to_string_pretty(&combined)?);
        }
        DeemixCommand::Add { url } => {
            let db = connect_db().await?;
            let (c, _) = load_deemix(&db).await?;
            let now = chrono::Utc::now().timestamp();
            sqlx::query("INSERT INTO deemix_downloads(spotify_playlist_url,status,created_at,updated_at)VALUES(?,'queued',?,?)ON CONFLICT(spotify_playlist_url)DO UPDATE SET status='queued',error_message=NULL,updated_at=excluded.updated_at").bind(&url).bind(now).bind(now).execute(&db).await?;
            c.add_to_queue(&url).await?;
            println!("Added: {url}");
        }
        DeemixCommand::Retry { id } => {
            let db = connect_db().await?;
            let (c, _) = load_deemix(&db).await?;
            let url: Option<String> =
                sqlx::query_scalar("SELECT spotify_playlist_url FROM deemix_downloads WHERE id=?")
                    .bind(id)
                    .fetch_optional(&db)
                    .await
                    .ok()
                    .flatten()
                    .ok_or_else(|| anyhow::anyhow!("Item {id} not found"))?;
            let now = chrono::Utc::now().timestamp();
            sqlx::query("UPDATE deemix_downloads SET status='queued',error_message=NULL,updated_at=?WHERE id=?").bind(now).bind(id).execute(&db).await?;
            let url_str = url.as_deref().unwrap_or("");
            let mut retried = false;
            if let Ok(queue) = c.get_queue().await {
                for (uuid, item) in &queue {
                    if format!("https://open.spotify.com/playlist/{}", item.id) == url_str {
                        c.retry_download(uuid).await?;
                        println!("Retried: {url_str}");
                        retried = true;
                        break;
                    }
                }
            }
            if !retried {
                c.add_to_queue(url_str).await?;
                println!("Re-added: {url_str}");
            }
        }
        DeemixCommand::Delete { id } => {
            let db = connect_db().await?;
            let r = sqlx::query("DELETE FROM deemix_downloads WHERE id=?")
                .bind(id)
                .execute(&db)
                .await?;
            if r.rows_affected() == 0 {
                anyhow::bail!("Item {id} not found");
            }
            println!("Deleted: {id}");
        }
    }
    Ok(())
}
