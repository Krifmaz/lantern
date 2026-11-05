//! Warnings sent outside Lantern: an ntfy topic, a Discord webhook and email (SMTP), whichever are set under
//! Settings → Accounts → Alerts. Each risk is sent once, again when it gets worse, and again after it cleared and
//! came back; a risk that stays red is sent again once a day.

use super::{Level, Risk};
use crate::AppState;
use kit_server_core::{p, Db, RowExt};
use serde_json::{json, Map, Value};

/// How long a red risk waits before it is sent again, in seconds.
const RESEND_DANGER: f64 = super::DAY;

/// The alert channels that are set.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Channels {
    /// the ntfy topic's address
    pub ntfy_url: String,
    /// the ntfy access token, "" for none
    pub ntfy_token: String,
    /// the Discord webhook's address
    pub discord: String,
    /// the email channel, when an address and a mail server are set
    pub email: Option<Email>,
}

/// How warning emails are sent.
#[derive(Clone, Default, PartialEq)]
pub struct Email {
    /// who gets them
    pub to: String,
    /// who sends them
    pub from: String,
    /// the mail server
    pub host: String,
    /// its port (0: the usual one for `security`)
    pub port: u16,
    /// `starttls`, `tls` or `none`
    pub security: String,
    /// the mail user, "" to send without signing in
    pub user: String,
    /// the mail password
    pub password: String,
}

impl std::fmt::Debug for Email {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Email")
            .field("to", &self.to)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("security", &self.security)
            .finish()
    }
}

impl Email {
    /// The port to use: the one set, else 587 (STARTTLS), 465 (TLS) or 25.
    pub fn port(&self) -> u16 {
        match (self.port, self.security.as_str()) {
            (p, _) if p > 0 => p,
            (_, "tls") => 465,
            (_, "none") => 25,
            _ => 587,
        }
    }
}

impl Channels {
    /// The channels from the settings ([`super::alert_settings`]).
    pub fn from_settings(s: &Map<String, Value>) -> Channels {
        let t = |k: &str| s.get(k).and_then(Value::as_str).unwrap_or_default().trim().to_string();
        let (to, host) = (t("alert_email_to"), t("smtp_host"));
        let email = (!to.is_empty() && !host.is_empty()).then(|| Email {
            from: if t("smtp_from").is_empty() { t("smtp_user") } else { t("smtp_from") },
            to,
            host,
            port: s.get("smtp_port").and_then(Value::as_u64).unwrap_or(0).min(65535) as u16,
            security: if t("smtp_security").is_empty() { "starttls".into() } else { t("smtp_security") },
            user: t("smtp_user"),
            password: s.get("smtp_password").and_then(Value::as_str).unwrap_or_default().to_string(),
        });
        Channels { ntfy_url: t("alert_ntfy_url"), ntfy_token: t("alert_ntfy_token"), discord: t("alert_discord_webhook"), email }
    }

    /// Whether any channel is set.
    pub fn any(&self) -> bool {
        !self.ntfy_url.is_empty() || !self.discord.is_empty() || self.email.is_some()
    }

    /// Sends `title` and `text` to every channel; for each one, its name and `Ok`, or why it failed in words.
    pub async fn send(&self, title: &str, text: &str, urgent: bool) -> Vec<(&'static str, Result<(), String>)> {
        let mut out = Vec::new();
        let http = reqwest::Client::builder().timeout(std::time::Duration::from_secs(20)).build().unwrap_or_default();
        if !self.ntfy_url.is_empty() {
            let mut req = http
                .post(&self.ntfy_url)
                .header("Title", title)
                .header("Priority", if urgent { "high" } else { "default" })
                .header("Tags", if urgent { "rotating_light" } else { "warning" })
                .body(text.to_string());
            if !self.ntfy_token.is_empty() {
                req = req.bearer_auth(&self.ntfy_token);
            }
            out.push(("ntfy", answer(req.send().await, "ntfy")));
        }
        if !self.discord.is_empty() {
            let body = json!({ "username": "Lantern", "content": format!("**{title}**\n{text}") });
            out.push(("Discord", answer(http.post(&self.discord).json(&body).send().await, "Discord")));
        }
        if let Some(e) = &self.email {
            out.push(("email", send_mail(e, title, text).await));
        }
        out
    }
}

/// `Ok` for a 2xx answer, else why not in words.
fn answer(r: reqwest::Result<reqwest::Response>, who: &str) -> Result<(), String> {
    match r {
        Ok(r) if r.status().is_success() => Ok(()),
        Ok(r) => Err(format!("{who} refused the message (it answered {}). Check the address and token.", r.status().as_u16())),
        Err(e) => Err(format!("{who} couldn't be reached: {e}.")),
    }
}

/// The email for `title` and `text`; why not, in words, when an address can't be used.
pub fn mail(e: &Email, title: &str, text: &str) -> Result<lettre::Message, String> {
    let from: lettre::message::Mailbox = e
        .from
        .parse()
        .map_err(|_| format!("The sender address \"{}\" can't be used. Set From to an address like lantern@example.com.", e.from))?;
    let to: lettre::message::Mailbox =
        e.to.parse().map_err(|_| format!("The address \"{}\" can't be used. Set Email to an address like you@example.com.", e.to))?;
    lettre::Message::builder()
        .from(from)
        .to(to)
        .subject(title)
        .header(lettre::message::header::ContentType::TEXT_PLAIN)
        .body(text.to_string())
        .map_err(|e| format!("The email can't be made: {e}."))
}

/// Sends `title` and `text` by email through `e`'s mail server.
async fn send_mail(e: &Email, title: &str, text: &str) -> Result<(), String> {
    use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
    let msg = mail(e, title, text)?;
    let builder = match e.security.as_str() {
        "tls" => AsyncSmtpTransport::<Tokio1Executor>::relay(&e.host),
        "none" => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&e.host)),
        _ => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&e.host),
    }
    .map_err(|err| format!("The mail server {} can't be used: {err}.", e.host))?;
    let mut builder = builder.port(e.port()).timeout(Some(std::time::Duration::from_secs(20)));
    if !e.user.is_empty() {
        builder = builder.credentials(lettre::transport::smtp::authentication::Credentials::new(e.user.clone(), e.password.clone()));
    }
    builder.build().send(msg).await.map(|_| ()).map_err(|err| format!("The mail server {} didn't take the email: {err}.", e.host))
}

/// Whether a risk last sent at `sent` with level `was` (none: never) should be sent now at `level`.
pub fn should_send(was: Option<(&str, f64)>, level: Level, now: f64) -> bool {
    match was {
        None => true,
        Some((was, _)) if was == "warn" && level == Level::Danger => true,
        Some(("danger", at)) if level == Level::Danger => now - at >= RESEND_DANGER,
        Some(_) => false,
    }
}

/// Sends each risk in `risks` that [`should_send`], and forgets risks that cleared (so they are sent when they
/// come back). Without a channel, nothing is sent and nothing is noted.
pub async fn send_new(st: &AppState, risks: &[(String, Risk)]) {
    let ch = Channels::from_settings(&super::alert_settings(st));
    let now = crate::auth::now();
    let kept = st.db.query("SELECT vault_id, key, level, sent_at FROM keeper_alerts", p![]).unwrap_or_default();
    for r in &kept {
        if !risks.iter().any(|(id, risk)| *id == r.s("vault_id") && risk.key == r.s("key")) {
            let _ = st.db.execute("DELETE FROM keeper_alerts WHERE vault_id=? AND key=?", p![r.s("vault_id"), r.s("key")]);
        }
    }
    if !ch.any() {
        return;
    }
    for (id, risk) in risks {
        let was = kept.iter().find(|r| r.s("vault_id") == *id && r.s("key") == risk.key).map(|r| (r.s("level"), r.f("sent_at")));
        if !should_send(was.as_ref().map(|(l, at)| (l.as_str(), *at)), risk.level, now) {
            continue;
        }
        let urgent = risk.level == Level::Danger;
        let title = if urgent { "Lantern: an account is at risk now" } else { "Lantern: an account needs you soon" };
        let sent = ch.send(title, &risk.text, urgent).await;
        for (who, r) in &sent {
            if let Err(why) = r {
                tracing::warn!("Keeper: the alert couldn't go through {who}: {why}");
            }
        }
        if sent.iter().any(|(_, r)| r.is_ok()) {
            let _ = st.db.execute(
                "INSERT OR REPLACE INTO keeper_alerts (vault_id, key, level, sent_at) VALUES (?,?,?,?)",
                p![id, risk.key, risk.level.name(), now],
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_come_from_the_settings() {
        let s: Map<String, Value> = serde_json::from_value(json!({
            "alert_ntfy_url": " https://ntfy.sh/me ", "alert_discord_webhook": "", "alert_email_to": "me@example.com",
            "smtp_host": "smtp.example.com", "smtp_security": "tls", "smtp_user": "bot@example.com", "smtp_password": "pw", "smtp_from": "", "smtp_port": 0
        }))
        .unwrap();
        let ch = Channels::from_settings(&s);
        assert_eq!(ch.ntfy_url, "https://ntfy.sh/me");
        let e = ch.email.clone().unwrap();
        assert_eq!((e.from.as_str(), e.port()), ("bot@example.com", 465));
        assert!(!format!("{e:?}").contains("pw"), "the password stays out of logs");
        assert!(mail(&e, "Title", "Text").is_ok());
        let bad = Email { from: "not an address".into(), ..e };
        assert!(mail(&bad, "Title", "Text").unwrap_err().starts_with("The sender address"));
        assert!(!Channels::default().any());
    }

    #[test]
    fn each_risk_is_sent_once_and_again_when_worse() {
        assert!(should_send(None, Level::Warn, 0.0));
        assert!(!should_send(Some(("warn", 0.0)), Level::Warn, 1e9));
        assert!(should_send(Some(("warn", 0.0)), Level::Danger, 10.0));
        assert!(!should_send(Some(("danger", 0.0)), Level::Danger, 3600.0));
        assert!(should_send(Some(("danger", 0.0)), Level::Danger, RESEND_DANGER));
        assert!(!should_send(Some(("danger", 0.0)), Level::Warn, RESEND_DANGER));
    }
}
