// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/Notifications/Email/Email.cs, EmailSettings.cs), GPL-3.0.

//! `Email`: each event as a plain-text mail through the SMTP server in the settings, with STARTTLS when the
//! server offers it (`Preferred`), always encrypted (`Always`: TLS from the start on port 465, else STARTTLS) or
//! never (`Never`).

use super::senders::SendError;
use super::Event;
use crate::prowlarr::apps::provider::Definition;
use lettre::message::{Mailbox, Message};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use serde_json::Value;

/// The addresses of a list setting.
fn addresses(def: &Definition, field: &str) -> Vec<String> {
    def.setting(field).as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
}

/// Sends `ev` as a mail.
pub async fn send(def: &Definition, ev: &Event<'_>) -> Result<(), SendError> {
    let (subject, body) = match ev {
        Event::Grab(g) => ("Lantern - Release Grabbed".to_string(), g.message.clone()),
        Event::HealthIssue(f) => ("Lantern - Health Check Failure".to_string(), f.message.clone()),
        Event::HealthRestored(f) => {
            ("Lantern - Health Check Restored".to_string(), format!("The following issue is now resolved: {}", f.message))
        }
        Event::ApplicationUpdate(u) => ("Lantern - Application Updated".to_string(), u.message.clone()),
        Event::Test => (
            "Lantern - Test Notification".to_string(),
            "Success! You have properly configured your email notification settings".to_string(),
        ),
    };
    let bad = |what: &str, a: &str| SendError::new_pub("Server", format!("The {what} address '{a}' isn't one Lantern can send to."));
    let from: Mailbox = def.text("from").parse().map_err(|_| bad("From", &def.text("from")))?;
    let mut m = Message::builder().from(from).subject(subject);
    for (field, what) in [("to", "To"), ("cc", "Cc"), ("bcc", "Bcc")] {
        for a in addresses(def, field) {
            let mb: Mailbox = a.parse().map_err(|_| bad(what, &a))?;
            m = match field {
                "to" => m.to(mb),
                "cc" => m.cc(mb),
                _ => m.bcc(mb),
            };
        }
    }
    let mail = m.body(body).map_err(|e| SendError::new_pub("Server", format!("Lantern can't make the mail: {e}")))?;
    let host = def.text("server");
    let port = def.setting("port").as_i64().unwrap_or(587) as u16;
    let tls = || {
        TlsParameters::new(host.clone())
            .map_err(|e| SendError::new_pub("Server", format!("Lantern can't set up encryption for {host}: {e}")))
    };
    let tls = match def.setting("useEncryption").as_i64().unwrap_or(0) {
        2 => Tls::None,
        1 if port == 465 => Tls::Wrapper(tls()?),
        1 => Tls::Required(tls()?),
        _ => Tls::Opportunistic(tls()?),
    };
    let mut transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host.clone()).port(port).tls(tls);
    if !def.text("username").is_empty() {
        transport = transport.credentials(Credentials::new(def.text("username"), def.text("password")));
    }
    transport
        .timeout(Some(std::time::Duration::from_secs(30)))
        .build()
        .send(mail)
        .await
        .map(|_| ())
        .map_err(|e| SendError::new_pub("Server", format!("Unable to send test email: {e}")))
}
