//! Tonies sync run (#502): the **execution** half of the M2 sync. The pure
//! planning lives in [`super::sync`] and is deliberately untouched here — this
//! module only orchestrates the API writes, the bounded transcoding poll, and
//! the run log (`hub_tonie_syncs`).
//!
//! Guarantees:
//! * [`claim_run`] inserts a `status='running'` row **before** any API write, so
//!   even a run that fails halfway leaves a log entry (`status='error'` +
//!   `error`).
//! * At most one live run per binding ([`Claim::Busy`]) and a minimum spacing
//!   between runs ([`Claim::RateLimited`]).
//! * The transcoding poll is bounded ([`poll_transcoding`]) — it never waits
//!   forever.
//! * No credential ever reaches a log line, an error message, or `skipped_json`:
//!   errors are the client's status-level messages, and the run log stores only
//!   titles/ISRCs/reasons.

use std::collections::HashSet;
use std::future::Future;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

use super::Chapter;
use super::bindings::Binding;
use super::client::ToniesClient;
use super::sync::{
    DesiredTrack, DiffKind, SkipReason, SkippedTrack, dedupe_by_isrc, greedy_fit, ms_to_seconds,
    plan_diff,
};
use crate::config::Config;
use crate::music_api;

/// A run still marked `running` and younger than this is considered live.
pub const STALE_SECS: i64 = 900;
/// Minimum spacing between two runs of the same binding, in seconds.
pub const MIN_INTERVAL_SECS: i64 = 30;
/// Default transcoding poll: attempts × interval.
pub const POLL_ATTEMPTS: u32 = 12;
/// Default seconds between two transcoding polls.
pub const POLL_INTERVAL_SECS: u64 = 5;
/// How many run-log rows the UI shows per binding.
pub const LIST_LIMIT: i64 = 5;

/// `status` while a run is in flight.
pub const STATUS_RUNNING: &str = "running";
/// Finished, chapters were uploaded/patched.
pub const STATUS_OK: &str = "ok";
/// Nothing to do — the Tonie already matches the plan.
pub const STATUS_NOOP: &str = "noop";
/// Uploads succeeded, but transcoding did not settle within the poll budget.
pub const STATUS_TIMEOUT: &str = "timeout";
/// The run aborted; `error` explains why (status-level, no credential).
pub const STATUS_ERROR: &str = "error";

// ── run log: view model ─────────────────────────────────────────────────────

/// One `skipped_json` entry. Display-only; never carries a credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedJson {
    pub title: String,
    pub isrc: Option<String>,
    /// Stable machine token (`no_vault` | `no_isrc` | `duplicate_isrc` | `cap_reached`).
    pub reason: String,
}

impl SkippedJson {
    /// German label for the reason token.
    pub fn label(&self) -> &'static str {
        reason_label(&self.reason)
    }
}

/// Stable machine token for a [`SkipReason`].
pub fn reason_str(r: SkipReason) -> &'static str {
    match r {
        SkipReason::NoVault => "no_vault",
        SkipReason::NoIsrc => "no_isrc",
        SkipReason::DuplicateIsrc => "duplicate_isrc",
        SkipReason::CapReached => "cap_reached",
    }
}

/// German label for a reason token.
pub fn reason_label(reason: &str) -> &'static str {
    match reason {
        "no_vault" => "nicht im Vault",
        "no_isrc" => "ohne ISRC",
        "duplicate_isrc" => "doppelt",
        "cap_reached" => "passten nicht mehr",
        _ => "übersprungen",
    }
}

/// One `hub_tonie_syncs` row, as the UI renders it.
#[derive(Debug, Clone, Default)]
pub struct SyncRow {
    pub id: i64,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub status: String,
    pub chapters_before: Option<i64>,
    pub chapters_after: Option<i64>,
    pub seconds: Option<i64>,
    pub added: Option<i64>,
    pub removed: Option<i64>,
    pub skipped: Vec<SkippedJson>,
    pub error: Option<String>,
}

impl SyncRow {
    /// Chapters on the Tonie after the run.
    pub fn chapters(&self) -> i64 {
        self.chapters_after.unwrap_or(0)
    }

    /// Content duration after the run, in whole minutes.
    pub fn minutes(&self) -> i64 {
        self.seconds.unwrap_or(0) / 60
    }

    /// Uploaded chapters.
    pub fn added_count(&self) -> i64 {
        self.added.unwrap_or(0)
    }

    /// Dropped chapters.
    pub fn removed_count(&self) -> i64 {
        self.removed.unwrap_or(0)
    }

    /// Short German status label.
    pub fn status_label(&self) -> String {
        match self.status.as_str() {
            STATUS_OK => "ok".to_string(),
            STATUS_NOOP => "keine Änderung".to_string(),
            STATUS_TIMEOUT => "Transcoding läuft".to_string(),
            STATUS_ERROR => "Fehler".to_string(),
            STATUS_RUNNING => "läuft".to_string(),
            other => other.to_string(),
        }
    }

    /// `YYYY-MM-DD HH:MM` (local text form of the stored RFC3339 instant).
    pub fn started_label(&self) -> String {
        short_ts(&self.started_at)
    }

    /// Wall-clock duration of the run, or empty while it is still running.
    pub fn duration_label(&self) -> String {
        let (Some(start), Some(end)) = (
            chrono::DateTime::parse_from_rfc3339(&self.started_at).ok(),
            self.finished_at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()),
        ) else {
            return String::new();
        };
        let secs = (end - start).num_seconds().max(0);
        format!("{secs}s")
    }
}

fn short_ts(s: &str) -> String {
    if s.len() >= 16 {
        format!("{} {}", &s[..10], &s[11..16])
    } else {
        s.to_string()
    }
}

// ── run log: store ──────────────────────────────────────────────────────────

/// Outcome of one run, written by [`finish_run`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOutcome {
    pub status: String,
    pub chapters_before: i64,
    pub chapters_after: i64,
    /// Content duration on the Tonie after the run, in seconds.
    pub seconds: i64,
    pub added: i64,
    pub removed: i64,
    pub skipped: Vec<SkippedTrack>,
    pub error: Option<String>,
}

impl RunOutcome {
    /// The `skipped_json` payload (titles + reasons only).
    pub fn skipped_json(&self) -> Vec<SkippedJson> {
        self.skipped
            .iter()
            .map(|s| SkippedJson {
                title: s.title.clone(),
                isrc: s.isrc.clone(),
                reason: reason_str(s.reason).to_string(),
            })
            .collect()
    }
}

/// Whether [`claim_run`] started a run or refused one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// A `running` row was inserted; carries its id.
    Started(i64),
    /// Another run of this binding is still live.
    Busy,
    /// The last run of this binding is too recent.
    RateLimited,
}

/// Try to claim the right to run a sync for `binding_id`.
///
/// The `running` row is inserted **before** any API write, so every run has a
/// log entry — a later failure only flips it to `error`.
pub async fn claim_run(pool: &SqlitePool, binding_id: i64) -> Result<Claim> {
    let now = chrono::Utc::now();
    let stale_cutoff = (now - chrono::Duration::seconds(STALE_SECS)).to_rfc3339();
    let rate_cutoff = (now - chrono::Duration::seconds(MIN_INTERVAL_SECS)).to_rfc3339();

    // Reap a `running` row that never finished (crash, kill) so the partial
    // UNIQUE index below cannot be blocked forever by a stale row. Only rows
    // older than STALE_SECS are reaped; a genuinely live run is left alone.
    sqlx::query(
        "UPDATE hub_tonie_syncs
            SET status = ?2, finished_at = ?3, error = ?4
          WHERE binding_id = ?1 AND status = 'running' AND started_at <= ?5",
    )
    .bind(binding_id)
    .bind(STATUS_ERROR)
    .bind(now.to_rfc3339())
    .bind("Lauf abgebrochen (stale)")
    .bind(&stale_cutoff)
    .execute(pool)
    .await
    .context("Tonies-Sync-Status konnte nicht gelesen werden")?;

    let live: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_tonie_syncs
          WHERE binding_id = ?1 AND status = 'running' AND started_at > ?2",
    )
    .bind(binding_id)
    .bind(&stale_cutoff)
    .fetch_one(pool)
    .await
    .context("Tonies-Sync-Status konnte nicht gelesen werden")?;
    if live > 0 {
        return Ok(Claim::Busy);
    }

    let last: Option<String> = sqlx::query_scalar(
        "SELECT started_at FROM hub_tonie_syncs
          WHERE binding_id = ?1 ORDER BY started_at DESC LIMIT 1",
    )
    .bind(binding_id)
    .fetch_optional(pool)
    .await
    .context("Tonies-Sync-Status konnte nicht gelesen werden")?;
    if let Some(last) = last && last > rate_cutoff {
        return Ok(Claim::RateLimited);
    }

    // Atomic claim: the partial UNIQUE index (migration 059) allows at most one
    // `running` row per binding, so a racing INSERT loses with a
    // UNIQUE-constraint error instead of starting a second parallel run.
    let inserted = sqlx::query_scalar::<_, i64>(
        "INSERT INTO hub_tonie_syncs (binding_id, started_at, status)
         VALUES (?1, ?2, 'running') RETURNING id",
    )
    .bind(binding_id)
    .bind(now.to_rfc3339())
    .fetch_one(pool)
    .await;

    match inserted {
        Ok(id) => Ok(Claim::Started(id)),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Ok(Claim::Busy),
        Err(e) => Err(e).context("Tonies-Sync-Lauf konnte nicht angelegt werden"),
    }
}

/// Write the outcome of `run_id` and mirror it onto the binding
/// (`last_sync_at`/`last_sync_status`, clearing `dirty_since`).
pub async fn finish_run(pool: &SqlitePool, run_id: i64, outcome: &RunOutcome) -> Result<()> {
    let finished = chrono::Utc::now().to_rfc3339();
    let skipped_json =
        serde_json::to_string(&outcome.skipped_json()).unwrap_or_else(|_| "[]".to_string());
    // Only a run that actually reconciled the Tonie clears the "unsynced"
    // signal. `error`/`timeout` wrote nothing, so `dirty_since` must stay put.
    let reconciled = matches!(outcome.status.as_str(), STATUS_OK | STATUS_NOOP);

    let mut tx = pool
        .begin()
        .await
        .context("Tonies-Sync-Abschluss konnte nicht gestartet werden")?;

    sqlx::query(
        "UPDATE hub_tonie_syncs
            SET finished_at = ?2, status = ?3, chapters_before = ?4, chapters_after = ?5,
                seconds = ?6, added = ?7, removed = ?8, skipped_json = ?9, error = ?10
          WHERE id = ?1",
    )
    .bind(run_id)
    .bind(&finished)
    .bind(&outcome.status)
    .bind(outcome.chapters_before)
    .bind(outcome.chapters_after)
    .bind(outcome.seconds)
    .bind(outcome.added)
    .bind(outcome.removed)
    .bind(&skipped_json)
    .bind(outcome.error.as_deref())
    .execute(&mut *tx)
    .await
    .context("Tonies-Sync-Abschluss konnte nicht gespeichert werden")?;

    sqlx::query(
        "UPDATE hub_tonie_bindings
            SET last_sync_at = ?2, last_sync_status = ?3,
                dirty_since = CASE WHEN ?4 THEN NULL ELSE dirty_since END,
                updated_at = ?2
          WHERE id = (SELECT binding_id FROM hub_tonie_syncs WHERE id = ?1)",
    )
    .bind(run_id)
    .bind(&finished)
    .bind(&outcome.status)
    .bind(reconciled)
    .execute(&mut *tx)
    .await
    .context("Tonies-Binding-Status konnte nicht aktualisiert werden")?;

    tx.commit()
        .await
        .context("Tonies-Sync-Abschluss konnte nicht bestätigt werden")?;
    Ok(())
}

/// The last `limit` runs of a binding, newest first.
pub async fn list_syncs(pool: &SqlitePool, binding_id: i64, limit: i64) -> Result<Vec<SyncRow>> {
    let rows = sqlx::query(
        "SELECT id, started_at, finished_at, status, chapters_before, chapters_after,
                seconds, added, removed, skipped_json, error
           FROM hub_tonie_syncs
          WHERE binding_id = ?1
          ORDER BY started_at DESC, id DESC
          LIMIT ?2",
    )
    .bind(binding_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("Tonies-Sync-Log konnte nicht gelesen werden")?;

    Ok(rows
        .into_iter()
        .map(|r| SyncRow {
            id: r.get("id"),
            started_at: r.get("started_at"),
            finished_at: r.get("finished_at"),
            status: r.get("status"),
            chapters_before: r.get("chapters_before"),
            chapters_after: r.get("chapters_after"),
            seconds: r.get("seconds"),
            added: r.get("added"),
            removed: r.get("removed"),
            skipped: r
                .get::<Option<String>, _>("skipped_json")
                .and_then(|s| serde_json::from_str::<Vec<SkippedJson>>(&s).ok())
                .unwrap_or_default(),
            error: r.get("error"),
        })
        .collect())
}

// ── readiness ───────────────────────────────────────────────────────────────

/// Vault-readiness for a set of ISRCs (one `music_api::status` call each).
/// Unknown/unreachable ISRCs are simply not "ready".
pub async fn readiness(cfg: &Config, isrcs: impl IntoIterator<Item = String>) -> HashSet<String> {
    let mut set = HashSet::new();
    for isrc in isrcs {
        if isrc.is_empty() || set.contains(&isrc) {
            continue;
        }
        if let Ok(st) = music_api::status(cfg, &isrc).await && st.ready() {
            set.insert(isrc);
        }
    }
    set
}

// ── execution ───────────────────────────────────────────────────────────────

/// Poll budget for the transcoding wait.
#[derive(Debug, Clone, Copy)]
pub struct SyncOptions {
    pub poll_attempts: u32,
    pub poll_interval: Duration,
}

impl Default for SyncOptions {
    fn default() -> Self {
        SyncOptions {
            poll_attempts: POLL_ATTEMPTS,
            poll_interval: Duration::from_secs(POLL_INTERVAL_SECS),
        }
    }
}

/// Result of waiting for the Tonies backend to finish transcoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transcoding {
    Ready,
    Timeout,
}

/// Wait (bounded) until no chapter reports `transcoding == true`.
///
/// A tonie with no transcoding flag / no chapters is immediately ready. After
/// `attempts` polls it gives up and returns [`Transcoding::Timeout`] — it never
/// waits forever.
pub async fn poll_transcoding(
    client: &mut ToniesClient,
    household: &str,
    ctid: &str,
    attempts: u32,
    interval: Duration,
) -> Transcoding {
    for attempt in 0..attempts.max(1) {
        if let Ok(t) = client.get_creativetonie(household, ctid).await {
            let busy = t.chapters.iter().any(|c| c.transcoding == Some(true));
            if !busy {
                return Transcoding::Ready;
            }
        }
        if attempt + 1 < attempts.max(1) {
            tokio::time::sleep(interval).await;
        }
    }
    Transcoding::Timeout
}

/// Upload one track and register it as a chapter. Strict order:
/// `request_upload` → `fetch_bytes` → `put_multipart` → `add_chapter`.
async fn upload_one<F, Fut>(
    client: &mut ToniesClient,
    household: &str,
    ctid: &str,
    track: &DesiredTrack,
    isrc: &str,
    fetch_bytes: &F,
) -> Result<Chapter>
where
    F: Fn(&str) -> Fut,
    Fut: Future<Output = Result<Vec<u8>>>,
{
    let ticket = client.request_upload().await?;
    let bytes = fetch_bytes(isrc)
        .await
        .context("Audio-Bytes konnten nicht geladen werden")?;
    let filename = format!("{isrc}.mp3");
    client
        .put_multipart(&ticket, &filename, "audio/mpeg", &bytes)
        .await?;
    client
        .add_chapter(household, ctid, &track.title, &ticket.file_id)
        .await?;
    Ok(Chapter {
        id: None,
        title: Some(track.title.clone()),
        file: Some(ticket.file_id.clone()),
        seconds: Some(ms_to_seconds(track.duration_ms)),
        transcoding: None,
        chapter_type: None,
    })
}

fn error_outcome(
    skipped: Vec<SkippedTrack>,
    chapters_before: i64,
    seconds: i64,
    message: String,
) -> RunOutcome {
    RunOutcome {
        status: STATUS_ERROR.to_string(),
        chapters_before,
        chapters_after: chapters_before,
        seconds,
        added: 0,
        removed: 0,
        skipped,
        error: Some(message),
    }
}

/// Run one sync: plan (pure) → upload the difference → bounded transcoding
/// poll. Always returns an outcome; failures land in `status='error'` + `error`
/// so the caller can log them.
///
/// `is_ready` decides vault readiness and `fetch_bytes` supplies the audio
/// bytes, so this can be exercised without a network.
#[allow(clippy::too_many_arguments)]
pub async fn run_sync<R, F, Fut>(
    client: &mut ToniesClient,
    household: &str,
    binding: &Binding,
    desired: Vec<DesiredTrack>,
    is_ready: R,
    fetch_bytes: F,
    opts: &SyncOptions,
) -> RunOutcome
where
    R: Fn(&str) -> bool,
    F: Fn(&str) -> Fut,
    Fut: Future<Output = Result<Vec<u8>>>,
{
    // 1. pure planning: dedupe, then greedy fit against the binding capacity.
    let (deduped, mut skipped) = dedupe_by_isrc(desired);
    let fit = greedy_fit(&deduped, binding.cap_seconds, |isrc| is_ready(isrc));
    skipped.extend(fit.skipped.iter().cloned());

    // 2. current state of the Tonie (also the diff basis).
    let current = match client.get_creativetonie(household, &binding.tonie_id).await {
        Ok(t) => t,
        Err(e) => return error_outcome(skipped, 0, fit.seconds, format!("{e}")),
    };
    let chapters_before = current.chapters.len() as i64;
    let diff = plan_diff(&current.chapters, &fit.tracks);

    // 3. nothing to do — no API write at all.
    if diff.kind == DiffKind::Unchanged {
        return RunOutcome {
            status: STATUS_NOOP.to_string(),
            chapters_before,
            chapters_after: chapters_before,
            seconds: fit.seconds,
            added: 0,
            removed: 0,
            skipped,
            error: None,
        };
    }

    // 4. upload each new chapter, in plan order.
    let mut uploaded: Vec<Chapter> = Vec::with_capacity(diff.upload.len());
    for track in &diff.upload {
        let Some(isrc) = track.isrc.as_deref() else {
            skipped.push(SkippedTrack {
                title: track.title.clone(),
                isrc: None,
                reason: SkipReason::NoIsrc,
            });
            continue;
        };
        match upload_one(
            client,
            household,
            &binding.tonie_id,
            track,
            isrc,
            &fetch_bytes,
        )
        .await
        {
            Ok(ch) => uploaded.push(ch),
            Err(e) => return error_outcome(skipped, chapters_before, fit.seconds, format!("{e}")),
        }
    }

    // 5. on a full replace, re-assert the whole list (reused prefix + uploads).
    if diff.kind == DiffKind::Replace {
        let mut target = diff.reuse.clone();
        target.extend(uploaded.iter().cloned());
        if let Err(e) = client
            .patch_chapters(household, &binding.tonie_id, &target)
            .await
        {
            return error_outcome(skipped, chapters_before, fit.seconds, format!("{e}"));
        }
    }

    // 6. bounded transcoding poll.
    let transcoding = poll_transcoding(
        client,
        household,
        &binding.tonie_id,
        opts.poll_attempts,
        opts.poll_interval,
    )
    .await;
    let status = match transcoding {
        Transcoding::Ready => STATUS_OK,
        Transcoding::Timeout => STATUS_TIMEOUT,
    };

    let chapters_after = diff.reuse.len() as i64 + uploaded.len() as i64;
    let removed = (chapters_before - diff.reuse.len() as i64).max(0);
    RunOutcome {
        status: status.to_string(),
        chapters_before,
        chapters_after,
        seconds: fit.seconds,
        added: uploaded.len() as i64,
        removed,
        skipped,
        error: None,
    }
}

/// Execute one binding end-to-end: build the Tonies client, resolve the
/// household, read the desired tracks from the bound source (playlist, tag or
/// backpack, #504), check vault readiness, then run the sync (Tonies +
/// music-api; **never** Spotify).
///
/// Never fails: any error becomes a `status='error'` outcome (credential-free)
/// that the caller writes to the run log. Shared by the web handler
/// (`/tonies/{id}/sync`) and the worker auto-sync, so behaviour is identical.
pub async fn execute_binding(
    pool: &SqlitePool,
    cfg: &Config,
    uid: i64,
    binding: &Binding,
) -> RunOutcome {
    let fail = |msg: String| RunOutcome {
        status: STATUS_ERROR.to_string(),
        error: Some(msg),
        ..Default::default()
    };

    let mut client = match crate::tonies::client_for(pool, cfg, uid).await {
        Ok(c) => c,
        Err(e) => return fail(e.message),
    };

    let household = match crate::tonies::load_account(pool, uid)
        .await
        .ok()
        .flatten()
        .and_then(|a| a.household_id)
        .filter(|h| !h.is_empty())
    {
        Some(h) => h,
        None => match crate::tonies::resolve_household(&mut client).await {
            Some(h) => h,
            None => return fail("Household konnte nicht ermittelt werden".to_string()),
        },
    };

    let desired = match super::sync::desired_for_source(
        pool,
        &binding.source_kind,
        binding.source_id,
    )
    .await
    {
        Ok(d) => d,
        Err(e) => return fail(format!("Quelle konnte nicht aufgelöst werden: {e}")),
    };
    if desired.is_empty() {
        return fail(format!(
            "Quelle ist leer oder nicht mehr vorhanden ({})",
            binding.source_kind
        ));
    }

    // Readiness: one music-api status call per distinct ISRC.
    let isrcs: Vec<String> = desired.iter().filter_map(|t| t.isrc.clone()).collect();
    let ready = readiness(cfg, isrcs).await;

    // Byte source: the real music-api download, mapped via `binding.format`.
    let cfg = cfg.clone();
    let format = binding.format.clone();
    let fetch = move |isrc: &str| {
        let cfg = cfg.clone();
        let format = format.clone();
        let isrc = isrc.to_string();
        async move { crate::music_api::file_bytes(&cfg, &isrc, &format).await }
    };

    let opts = SyncOptions::default();
    run_sync(
        &mut client,
        &household,
        binding,
        desired,
        |isrc| ready.contains(isrc),
        fetch,
        &opts,
    )
    .await
}

/// One-line, credential-free summary for the htmx toast.
pub fn summary(o: &RunOutcome) -> String {
    let mut msg = format!("{} Kapitel, {} min", o.chapters_after, o.seconds / 60);
    match o.status.as_str() {
        STATUS_NOOP => {
            msg.push_str(" — keine Änderung");
            return msg;
        }
        STATUS_ERROR => {
            let e = o
                .error
                .clone()
                .unwrap_or_else(|| "Unbekannter Fehler".to_string());
            msg.push_str(" — Fehler: ");
            msg.push_str(&e);
            return msg;
        }
        STATUS_TIMEOUT => msg.push_str(" · Transcoding läuft noch"),
        _ => {}
    }

    let count = |r: SkipReason| o.skipped.iter().filter(|s| s.reason == r).count();
    let mut parts = Vec::new();
    let cap = count(SkipReason::CapReached);
    let vault = count(SkipReason::NoVault);
    let no_isrc = count(SkipReason::NoIsrc);
    let dup = count(SkipReason::DuplicateIsrc);
    if cap > 0 {
        parts.push(format!("{cap} passten nicht mehr"));
    }
    if vault > 0 {
        parts.push(format!("{vault} nicht im Vault"));
    }
    if no_isrc > 0 {
        parts.push(format!("{no_isrc} ohne ISRC"));
    }
    if dup > 0 {
        parts.push(format!("{dup} doppelt"));
    }
    if !parts.is_empty() {
        msg.push_str(" — ");
        msg.push_str(&parts.join(", "));
    }
    msg
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tonies::sync::SkipReason;

    async fn pool_with_binding() -> (SqlitePool, i64) {
        let pool = crate::db::connect("sqlite::memory:").await.unwrap();
        let uid: i64 = sqlx::query_scalar(
            "INSERT INTO hub_users (slug, display_name, created_at)
             VALUES ('run-test', 'Run', '2026-01-01T00:00:00+00:00') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let bid: i64 = sqlx::query_scalar(
            "INSERT INTO hub_tonie_bindings
                 (user_id, tonie_id, tonie_name, source_kind, source_id, format, mode,
                  dirty_since, created_at, updated_at)
             VALUES (?1, 'ct1', 'Story', 'playlist', NULL, 'mp3-128', 'manual',
                     '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00',
                     '2026-01-01T00:00:00+00:00')
             RETURNING id",
        )
        .bind(uid)
        .fetch_one(&pool)
        .await
        .unwrap();
        (pool, bid)
    }

    fn outcome(status: &str, skipped: Vec<SkippedTrack>) -> RunOutcome {
        RunOutcome {
            status: status.to_string(),
            chapters_before: 0,
            chapters_after: 12,
            seconds: 3660,
            added: 12,
            removed: 0,
            skipped,
            error: None,
        }
    }

    #[test]
    fn summary_names_chapters_minutes_and_skips() {
        let skipped = vec![
            SkippedTrack {
                title: "Too Long".into(),
                isrc: Some("x".into()),
                reason: SkipReason::CapReached,
            },
            SkippedTrack {
                title: "Too Long 2".into(),
                isrc: Some("y".into()),
                reason: SkipReason::CapReached,
            },
            SkippedTrack {
                title: "Too Long 3".into(),
                isrc: Some("z".into()),
                reason: SkipReason::CapReached,
            },
            SkippedTrack {
                title: "Missing".into(),
                isrc: Some("m".into()),
                reason: SkipReason::NoVault,
            },
        ];
        let msg = summary(&outcome(STATUS_OK, skipped));
        assert_eq!(
            msg,
            "12 Kapitel, 61 min — 3 passten nicht mehr, 1 nicht im Vault"
        );
        assert!(!msg.contains("secret"));
    }

    #[test]
    fn summary_handles_noop_timeout_and_error() {
        let noop = summary(&outcome(STATUS_NOOP, vec![]));
        assert_eq!(noop, "12 Kapitel, 61 min — keine Änderung");

        let timeout = summary(&outcome(STATUS_TIMEOUT, vec![]));
        assert!(timeout.contains("12 Kapitel, 61 min"));
        assert!(timeout.contains("Transcoding läuft noch"));

        let mut err = outcome(STATUS_ERROR, vec![]);
        err.error = Some("S3-Upload antwortete 500".to_string());
        let msg = summary(&err);
        assert!(msg.contains("Fehler: S3-Upload antwortete 500"));
    }

    #[test]
    fn reason_labels_are_stable() {
        assert_eq!(reason_str(SkipReason::NoVault), "no_vault");
        assert_eq!(reason_str(SkipReason::NoIsrc), "no_isrc");
        assert_eq!(reason_str(SkipReason::DuplicateIsrc), "duplicate_isrc");
        assert_eq!(reason_str(SkipReason::CapReached), "cap_reached");
        assert_eq!(reason_label("cap_reached"), "passten nicht mehr");
        assert_eq!(reason_label("unknown"), "übersprungen");
    }

    #[tokio::test]
    async fn claim_then_finish_then_list_roundtrips() {
        let (pool, bid) = pool_with_binding().await;
        let Claim::Started(id) = claim_run(&pool, bid).await.unwrap() else {
            panic!("expected Started");
        };

        // A second claim while the run is live is refused.
        assert_eq!(claim_run(&pool, bid).await.unwrap(), Claim::Busy);

        let mut out = outcome(STATUS_OK, vec![]);
        out.chapters_before = 0;
        finish_run(&pool, id, &out).await.unwrap();

        let rows = list_syncs(&pool, bid, 5).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, STATUS_OK);
        assert_eq!(rows[0].chapters_after, Some(12));
        assert_eq!(rows[0].minutes(), 61);
        assert!(rows[0].finished_at.is_some());

        // Binding mirrors the outcome and is no longer dirty.
        let (last_at, last_status, dirty): (Option<String>, Option<String>, Option<String>) =
            sqlx::query_as(
                "SELECT last_sync_at, last_sync_status, dirty_since
                   FROM hub_tonie_bindings WHERE id = ?1",
            )
            .bind(bid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(last_at.is_some());
        assert_eq!(last_status.as_deref(), Some(STATUS_OK));
        assert!(dirty.is_none(), "dirty_since must be cleared after a run");
    }

    #[tokio::test]
    async fn recent_run_is_rate_limited() {
        let (pool, bid) = pool_with_binding().await;
        let Claim::Started(id) = claim_run(&pool, bid).await.unwrap() else {
            panic!("expected Started");
        };
        finish_run(&pool, id, &outcome(STATUS_OK, vec![]))
            .await
            .unwrap();

        // The finished run is still within MIN_INTERVAL_SECS → rate limited.
        assert_eq!(claim_run(&pool, bid).await.unwrap(), Claim::RateLimited);
    }

    #[tokio::test]
    async fn stale_running_row_does_not_block_forever() {
        let (pool, bid) = pool_with_binding().await;
        let old = (chrono::Utc::now() - chrono::Duration::seconds(STALE_SECS + 60)).to_rfc3339();
        sqlx::query(
            "INSERT INTO hub_tonie_syncs (binding_id, started_at, status)
             VALUES (?1, ?2, 'running')",
        )
        .bind(bid)
        .bind(&old)
        .execute(&pool)
        .await
        .unwrap();

        // The stale run is ignored → a fresh run may start.
        assert!(matches!(
            claim_run(&pool, bid).await.unwrap(),
            Claim::Started(_)
        ));
    }

    #[tokio::test]
    async fn error_outcome_is_persisted_with_message() {
        let (pool, bid) = pool_with_binding().await;
        let Claim::Started(id) = claim_run(&pool, bid).await.unwrap() else {
            panic!("expected Started");
        };
        let out = error_outcome(vec![], 3, 0, "S3-Upload antwortete 500".to_string());
        finish_run(&pool, id, &out).await.unwrap();

        let rows = list_syncs(&pool, bid, 5).await.unwrap();
        assert_eq!(rows[0].status, STATUS_ERROR);
        assert_eq!(rows[0].error.as_deref(), Some("S3-Upload antwortete 500"));
    }

    #[tokio::test]
    async fn running_index_blocks_a_second_live_row() {
        let (pool, bid) = pool_with_binding().await;
        let Claim::Started(_) = claim_run(&pool, bid).await.unwrap() else {
            panic!("expected Started");
        };

        // The partial UNIQUE index (migration 059) rejects a second `running`
        // row for the same binding — the storage-level race guard.
        let err = sqlx::query(
            "INSERT INTO hub_tonie_syncs (binding_id, started_at, status)
             VALUES (?1, ?2, 'running')",
        )
        .bind(bid)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&pool)
        .await
        .unwrap_err();
        match err {
            sqlx::Error::Database(e) => assert!(e.is_unique_violation()),
            other => panic!("expected unique violation, got {other:?}"),
        }

        // And a second claim is refused as Busy, never a second running row.
        assert_eq!(claim_run(&pool, bid).await.unwrap(), Claim::Busy);
        let running: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM hub_tonie_syncs WHERE binding_id = ?1 AND status = 'running'",
        )
        .bind(bid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(running, 1);
    }

    #[tokio::test]
    async fn error_run_keeps_dirty_signal() {
        let (pool, bid) = pool_with_binding().await;
        let Claim::Started(id) = claim_run(&pool, bid).await.unwrap() else {
            panic!("expected Started");
        };
        let out = error_outcome(vec![], 3, 0, "S3-Upload antwortete 500".to_string());
        finish_run(&pool, id, &out).await.unwrap();

        let (dirty, last_status): (Option<String>, Option<String>) = sqlx::query_as(
            "SELECT dirty_since, last_sync_status FROM hub_tonie_bindings WHERE id = ?1",
        )
        .bind(bid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(last_status.as_deref(), Some(STATUS_ERROR));
        assert!(
            dirty.is_some(),
            "a failed run must not clear the dirty_since signal"
        );
    }
}
