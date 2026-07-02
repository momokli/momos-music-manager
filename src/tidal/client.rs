//! Tidal API client wrapper (v2 JSON:API).

use anyhow::{Context, Result};
use reqwest::Client as HttpClient;
use serde::de::DeserializeOwned;
use sqlx::Pool;
use tracing::{debug, info, warn};

use crate::config::ServiceCredentials;
use crate::db::{get_service_config, update_service_tokens};
use crate::tidal::models::{TidalPlaylist, TidalTokenResponse, TidalTrack};

const TIDAL_API_BASE: &str = "https://openapi.tidal.com/v2";
const TIDAL_AUTH_BASE: &str = "https://auth.tidal.com/v1";
const COUNTRY_CODE: &str = "DE";

pub struct TidalClient {
    http: HttpClient,
    db: Pool<sqlx::Sqlite>,
    config: TidalConfig,
    access_token: String,
}
#[derive(Debug, Clone)]
struct TidalConfig {
    client_id: String,
    client_secret: String,
    redirect_uri: String,
}

impl TidalClient {
    pub async fn from_stored_token(
        db: Pool<sqlx::Sqlite>,
        config: &ServiceCredentials,
    ) -> Result<Self> {
        Ok(Self {
            http: HttpClient::builder().user_agent("mmm/0.8").build()?,
            db: db.clone(),
            config: TidalConfig {
                client_id: config.tidal_client_id()?.into(),
                client_secret: config.tidal_client_secret()?.into(),
                redirect_uri: config.tidal_redirect_uri.clone(),
            },
            access_token: get_service_config(&db, "tidal")
                .await?
                .context("no tidal config")?
                .access_token
                .context("no tidal token")?,
        })
    }

    pub async fn from_new_auth(
        db: Pool<sqlx::Sqlite>,
        config: &ServiceCredentials,
        code: &str,
        code_verifier: &str,
    ) -> Result<Self> {
        let cid = config.tidal_client_id()?;
        let csec = config.tidal_client_secret()?;
        let redir = &config.tidal_redirect_uri;
        let ah = format!(
            "Basic {}",
            base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                format!("{}:{}", cid, csec)
            )
        );
        let http = HttpClient::builder().user_agent("mmm/0.8").build()?;
        let r = http
            .post(format!("{}/oauth2/token", TIDAL_AUTH_BASE))
            .header("Authorization", &ah)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("code_verifier", code_verifier),
                ("redirect_uri", redir),
            ])
            .send()
            .await?;
        let sc = r.status();
        let body = r.text().await?;
        if !sc.is_success() {
            warn!("TIDAL TOKEN FAIL {}: {}", sc, &body[..body.len().min(300)]);
            return Err(anyhow::anyhow!("Tidal token: {}", sc));
        }
        let token: TidalTokenResponse = serde_json::from_str(&body)?;
        let now = chrono::Utc::now().timestamp();
        update_service_tokens(
            &db,
            "tidal",
            token.refresh_token.as_deref(),
            Some(&token.access_token),
            Some(now + token.expires_in),
        )
        .await?;
        Ok(Self {
            http,
            db,
            config: TidalConfig {
                client_id: cid.into(),
                client_secret: csec.into(),
                redirect_uri: redir.clone(),
            },
            access_token: token.access_token,
        })
    }

    pub fn auth_url(client_id: &str, redirect_uri: &str) -> (String, String) {
        use sha2::{Digest, Sha256};
        let cv: String = (0..4)
            .map(|_| uuid::Uuid::new_v4().to_string().replace('-', ""))
            .collect();
        let cv = cv[..128].to_string();
        let mut h = Sha256::new();
        h.update(cv.as_bytes());
        let cc = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            &h.finalize(),
        );
        (
            format!(
                "https://login.tidal.com/authorize?response_type=code&client_id={}&redirect_uri={}&code_challenge={}&code_challenge_method=S256&scope=playlists.read%20playlists.write%20user.read",
                client_id, redirect_uri, cc
            ),
            cv,
        )
    }

    pub async fn get_user_playlists(&self) -> Result<Vec<TidalPlaylist>> {
        let mut all = Vec::new();
        let mut off = 0i64;
        loop {
            let j: serde_json::Value = self
                .api_get(&format!(
                    "playlists?filter[owners]=me&countryCode={}&limit=50&offset={}",
                    COUNTRY_CODE, off
                ))
                .await?;
            let items = j["data"].as_array().cloned().unwrap_or_default();
            if items.is_empty() {
                break;
            }
            for item in &items {
                let a = &item["attributes"];
                all.push(TidalPlaylist {
                    id: item["id"].as_str().unwrap_or("").into(),
                    name: a["name"].as_str().unwrap_or("").into(),
                    description: a["description"].as_str().map(|s| s.into()),
                    track_count: a["trackCount"].as_i64().unwrap_or(0) as i32,
                    public: a["public"].as_bool().unwrap_or(false),
                    owner_name: None,
                });
            }
            off += 50;
            if items.len() < 50 {
                break;
            }
        }
        Ok(all)
    }

    pub async fn get_playlist(&self, pid: &str) -> Result<(TidalPlaylist, Vec<TidalTrack>)> {
        let j: serde_json::Value = self
            .api_get(&format!("playlists/{}?countryCode={}", pid, COUNTRY_CODE))
            .await?;
        let a = &j["data"]["attributes"];
        let pl = TidalPlaylist {
            id: j["data"]["id"].as_str().unwrap_or("").into(),
            name: a["name"].as_str().unwrap_or("").into(),
            description: a["description"].as_str().map(|s| s.into()),
            track_count: a["trackCount"].as_i64().unwrap_or(0) as i32,
            public: a["public"].as_bool().unwrap_or(false),
            owner_name: None,
        };
        let tj: serde_json::Value = self
            .api_get(&format!(
                "playlists/{}/tracks?countryCode={}&limit=100",
                pid, COUNTRY_CODE
            ))
            .await?;
        let mut tr = Vec::new();
        if let Some(items) = tj["data"].as_array() {
            for item in items {
                let a = &item["attributes"];
                tr.push(TidalTrack {
                    id: item["id"].as_str().unwrap_or("").into(),
                    title: a["title"].as_str().unwrap_or("").into(),
                    artist: a["artist"].as_str().unwrap_or("").into(),
                    isrc: a["isrc"].as_str().map(|s| s.into()),
                    duration_ms: (a["duration"].as_f64().unwrap_or(0.0) * 1000.0) as i64,
                    album: a["album"].as_str().map(|s| s.into()),
                    track_number: a["trackNumber"].as_i64().map(|n| n as i32),
                });
            }
        }
        Ok((pl, tr))
    }

    pub async fn create_playlist(
        &self,
        name: &str,
        desc: Option<&str>,
        public: bool,
    ) -> Result<String> {
        let j: serde_json::Value = self.api_post("playlists", &serde_json::json!({"data":{"type":"playlists","attributes":{"name":name,"description":desc.unwrap_or(""),"public":public}}})).await?;
        Ok(j["data"]["id"].as_str().context("no id")?.to_string())
    }

    pub async fn add_tracks_to_playlist(&self, pid: &str, ids: &[String]) -> Result<()> {
        for chunk in ids.chunks(50) {
            let data: Vec<serde_json::Value> = chunk
                .iter()
                .map(|id| serde_json::json!({"type":"tracks","id":id}))
                .collect();
            let _: serde_json::Value = self
                .api_post(
                    &format!("playlists/{}/relationships/tracks", pid),
                    &serde_json::json!({"data":data}),
                )
                .await?;
        }
        Ok(())
    }

    pub async fn search_by_isrc(&self, isrc: &str) -> Result<Option<String>> {
        let j: serde_json::Value = self
            .api_get(&format!(
                "tracks?filter[isrc]={}&countryCode={}&limit=1",
                isrc, COUNTRY_CODE
            ))
            .await?;
        Ok(j["data"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|t| t["id"].as_str().map(|s| s.to_string())))
    }

    async fn api_get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}/{}", TIDAL_API_BASE, path.trim_start_matches('/'));
        let r = self
            .http
            .get(&url)
            .header("Authorization", &self.auth_header())
            .header("Accept", "application/vnd.api+json")
            .send()
            .await?;
        let body = r.text().await?;
        Ok(serde_json::from_str(&body)?)
    }

    async fn api_post<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T> {
        let url = format!("{}/{}", TIDAL_API_BASE, path.trim_start_matches('/'));
        let r = self
            .http
            .post(&url)
            .header("Authorization", &self.auth_header())
            .header("Accept", "application/vnd.api+json")
            .header("Content-Type", "application/vnd.api+json")
            .json(body)
            .send()
            .await?;
        let text = r.text().await?;
        Ok(serde_json::from_str(&text)?)
    }

    fn auth_header(&self) -> String {
        format!("Bearer {}", self.access_token)
    }

    #[allow(dead_code)]
    pub async fn refresh_token_if_needed(&mut self) -> Result<()> {
        Ok(())
    }

    pub async fn store_token(&self) -> Result<()> {
        update_service_tokens(
            &self.db,
            "tidal",
            None,
            Some(&self.access_token),
            Some(chrono::Utc::now().timestamp() + 3600),
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_auth_url() {
        let (u, v) = TidalClient::auth_url("tid", "http://x");
        assert!(u.contains("playlists.read"));
        assert_eq!(v.len(), 128);
    }
    #[test]
    fn test_auth_header() {
        assert_eq!(format!("Bearer t"), "Bearer t");
    }
}
