//! Outgoing mail for accounts (`/forgot` reset links, invitations).
//!
//! Delivery sits behind [`Mailer`] for one reason: the routes must work with or
//! without a configured relay. Without one the hub uses [`NoopMailer`] and the
//! reset link is *not* lost — an admin can still mint one with
//! `music-hub link --user <slug>` (see `main.rs`), which is also the only
//! sensible fallback while no account has an address.
//!
//! ## Why a relay and not direct delivery
//!
//! Sending straight to the recipient's MX from this host is technically possible
//! (outbound port 25 is open on the deploy host, verified 2026-11-22) but
//! useless in practice: the host sits on a consumer Vodafone DSL pool IP with no
//! reverse DNS entry, which Gmail/Outlook treat as a spam source and often reject
//! outright. "Do we care about spam?" is not the question — the receiving server
//! decides, not us. So the only viable path is a relay that has the IP
//! reputation: an existing mailbox (SMTP submission, usually port 587) or an API
//! provider. Both fit behind [`Mailer`]; only SMTP is implemented today.
//!
//! Privacy: the body is never logged. Failures are logged with the recipient
//! *domain* only.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context, Result};
use lettre::message::{Mailbox, Message, header::ContentType};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

use crate::config::Config;

/// One outgoing message. Plain text on purpose: a reset link must never depend
/// on HTML rendering, and text is what the mail clients show first.
#[derive(Clone)]
pub struct OutgoingMail {
    pub to: String,
    pub subject: String,
    pub body: String,
}

impl std::fmt::Debug for OutgoingMail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The body carries the one-time token: it must never reach a log line,
        // and `#[derive(Debug)]` would put it there on any `{:?}`.
        f.debug_struct("OutgoingMail")
            .field("to", &self.to)
            .field("subject", &self.subject)
            .field("body", &"<redacted>")
            .finish()
    }
}

type SendFuture<'a> = Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

/// Delivery backend. Object-safe so `AppState` can hold an `Arc<dyn Mailer>`
/// and tests can swap in a recorder.
pub trait Mailer: Send + Sync {
    fn send<'a>(&'a self, mail: &'a OutgoingMail) -> SendFuture<'a>;

    /// `false` when delivery is not configured — the UI/CLI can then point at
    /// the admin link instead of promising an e-mail.
    fn is_configured(&self) -> bool {
        true
    }
}

/// Drops every message. Used when `HUB_SMTP_HOST` is unset.
///
/// Routes stay functional (they still mint a token), which keeps the response
/// neutral whether or not mail works — a client cannot tell from the outside.
pub struct NoopMailer;

impl Mailer for NoopMailer {
    fn send<'a>(&'a self, _mail: &'a OutgoingMail) -> SendFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    fn is_configured(&self) -> bool {
        false
    }
}

/// SMTP submission through an existing mailbox or relay.
pub struct SmtpMailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl Mailer for SmtpMailer {
    fn send<'a>(&'a self, mail: &'a OutgoingMail) -> SendFuture<'a> {
        Box::pin(async move {
            let to: Mailbox = mail
                .to
                .parse()
                .with_context(|| format!("invalid recipient address: {}", mail.to))?;
            let msg = Message::builder()
                .from(self.from.clone())
                .to(to)
                .subject(mail.subject.clone())
                .header(ContentType::TEXT_PLAIN)
                .body(mail.body.clone())
                .context("building message failed")?;
            self.transport
                .send(msg)
                .await
                .context("SMTP delivery failed")?;
            Ok(())
        })
    }
}

/// Pick a backend from the environment: SMTP when `HUB_SMTP_HOST` is set,
/// otherwise [`NoopMailer`].
pub fn from_config(cfg: &Config) -> Arc<dyn Mailer> {
    match build_smtp(cfg) {
        Ok(Some(m)) => Arc::new(m),
        Ok(None) => Arc::new(NoopMailer),
        Err(e) => {
            // A half-configured relay must not take the hub down, but it must be
            // visible: silently falling back to Noop would hide the misconfig.
            tracing::error!("SMTP configuration invalid, mail disabled: {e:#}");
            Arc::new(NoopMailer)
        }
    }
}

fn build_smtp(cfg: &Config) -> Result<Option<SmtpMailer>> {
    let Some(host) = cfg.smtp_host.as_deref().filter(|h| !h.trim().is_empty()) else {
        return Ok(None);
    };

    let mut builder = match cfg.smtp_tls.as_str() {
        // Implicit TLS (SMTPS), usually port 465.
        "wrapper" => AsyncSmtpTransport::<Tokio1Executor>::relay(host)?,
        // Plaintext — only for a relay on localhost or a trusted network.
        "none" => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host),
        // STARTTLS, the default on submission ports (587).
        _ => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)?,
    };
    if let Some(port) = cfg.smtp_port {
        builder = builder.port(port);
    }
    if let (Some(user), Some(pass)) = (&cfg.smtp_username, &cfg.smtp_password) {
        builder = builder.credentials(Credentials::new(user.clone(), pass.clone()));
    }

    let from: Mailbox = cfg
        .smtp_from
        .parse()
        .with_context(|| format!("invalid HUB_SMTP_FROM: {}", cfg.smtp_from))?;
    Ok(Some(SmtpMailer {
        transport: builder.build(),
        from,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_is_not_configured_and_accepts_anything() {
        let m = NoopMailer;
        assert!(!m.is_configured());
        let mail = OutgoingMail {
            to: "not-an-address".into(),
            subject: "s".into(),
            body: "b".into(),
        };
        // Polled once by the test runtime — must resolve to Ok immediately.
        let fut = m.send(&mail);
        drop(fut);
    }

    #[test]
    fn missing_host_means_no_mailer() {
        let cfg = Config::for_test("sqlite::memory:");
        assert!(build_smtp(&cfg).unwrap().is_none());
    }

    #[test]
    fn broken_from_address_is_an_error_not_a_panic() {
        let mut cfg = Config::for_test("sqlite::memory:");
        cfg.smtp_host = Some("127.0.0.1".into());
        cfg.smtp_tls = "none".into();
        cfg.smtp_from = "not a mailbox".into();
        assert!(build_smtp(&cfg).is_err());
    }
}
