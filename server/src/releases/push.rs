//! Where a release a filter takes goes: straight to Sonarr, Radarr, Lidarr, Readarr or Whisparr (their
//! `release/push` API, which grabs it through the app's own download client), or to a download client through
//! the download hub ([`DownloadHub`], which Lantern's [`crate::hub::Hub`] is). A client push becomes a grab of one
//! of the hub's apps (the action's `client`, else the app "Releases", registered at the first push), so it gets
//! that app's category and folder, progress, retries and callbacks like any other grab.

use super::filter::Action;
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// A release to download.
#[derive(Debug, Clone, Default)]
pub struct Grab {
    /// The release's name.
    pub title: String,
    /// Its download link (with the passkey: never logged).
    pub download_url: String,
    /// Its page on the site.
    pub info_url: String,
    /// Bytes, when known.
    pub size: Option<u64>,
    /// `torrent` or `usenet`.
    pub protocol: String,
    /// The tracker it came from.
    pub source_name: String,
    /// The hub app whose category and folder it gets (empty: "Releases").
    pub client: String,
    /// The category it gets.
    pub category: String,
    /// Add it paused.
    pub paused: bool,
}

/// A boxed future, the answer of a [`DownloadHub`].
pub type HubFuture<'a> = Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;

/// The download hub as releases see it: it takes a release and gives it to a download client.
pub trait DownloadHub: Send + Sync + 'static {
    /// Starts downloading `grab`; what happened in words, or why it couldn't.
    fn add<'a>(&'a self, grab: &'a Grab) -> HubFuture<'a>;
}

/// The hub app a client push goes to when the filter names none.
pub const DEFAULT_APP: &str = "Releases";

impl DownloadHub for crate::hub::Hub {
    fn add<'a>(&'a self, grab: &'a Grab) -> HubFuture<'a> {
        Box::pin(async move {
            let name = if grab.client.trim().is_empty() { DEFAULT_APP } else { grab.client.trim() };
            let app = match self.app_named(name) {
                Ok(a) => a,
                Err(_) if name == DEFAULT_APP => {
                    self.register(&json!({ "name": DEFAULT_APP, "media": "other", "source": "releases" })).map_err(|e| e.detail)?.0
                }
                Err(e) => return Err(e.detail),
            };
            let g = self
                .grab(&app, &json!({ "url": grab.download_url, "title": grab.title, "protocol": grab.protocol }))
                .map_err(|e| e.detail)?;
            Ok(format!("Sent to the download hub for {} (grab {})", app.name, g.id))
        })
    }
}

/// A hub that takes nothing: every push says download clients aren't set up (tests, and before the hub starts).
pub struct NoHub;

impl DownloadHub for NoHub {
    fn add<'a>(&'a self, _: &'a Grab) -> HubFuture<'a> {
        Box::pin(async {
            Err("Lantern's download hub isn't running, so this release can't go to a download client. Send it to Sonarr, Radarr, Lidarr or Readarr instead"
                .to_string())
        })
    }
}

/// The app's name as people know it.
pub fn app_name(app: &str) -> &'static str {
    match app {
        "sonarr" => "Sonarr",
        "radarr" => "Radarr",
        "lidarr" => "Lidarr",
        "readarr" => "Readarr",
        "whisparr" => "Whisparr",
        _ => "The app",
    }
}

/// The push address of `app` at `base` (`/api/v3/release/push`, `/api/v1/…` for Lidarr and Readarr).
pub fn push_url(app: &str, base: &str) -> String {
    let v = if app == "lidarr" || app == "readarr" { "v1" } else { "v3" };
    format!("{}/api/{v}/release/push", base.trim_end_matches('/'))
}

/// Sends `grab` to the app of `action`; what happened in words, or why it couldn't.
pub async fn to_arr(http: &reqwest::Client, action: &Action, grab: &Grab) -> Result<String, String> {
    let name = app_name(&action.app);
    let body = json!({
        "title": grab.title,
        "downloadUrl": grab.download_url,
        "infoUrl": grab.info_url,
        "size": grab.size.unwrap_or(0),
        "indexer": grab.source_name,
        "downloadProtocol": grab.protocol,
        "protocol": grab.protocol,
        "publishDate": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    });
    let resp = http
        .post(push_url(&action.app, &action.url))
        .header("X-Api-Key", &action.api_key)
        .timeout(Duration::from_secs(30))
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| format!("{name} at {} can't be reached: {}", action.url, e.without_url()))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if status.as_u16() == 401 {
        return Err(format!("{name} turned down the API key. Copy it again from {name}'s Settings → General"));
    }
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !status.is_success() {
        let said = v.get("message").or_else(|| v.get(0).and_then(|e| e.get("errorMessage"))).and_then(Value::as_str).unwrap_or("");
        return Err(format!("{name} answered {}{}", status.as_u16(), if said.is_empty() { String::new() } else { format!(": {said}") }));
    }
    let decisions: Vec<Value> = match v {
        Value::Array(a) => a,
        Value::Object(_) => vec![v],
        _ => Vec::new(),
    };
    let rejected: Vec<String> = decisions
        .iter()
        .filter(|d| d.get("approved").and_then(Value::as_bool) == Some(false) || d.get("rejected").and_then(Value::as_bool) == Some(true))
        .flat_map(|d| {
            d.get("rejections").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().map(|r| match r {
                Value::String(s) => s,
                other => other.get("reason").and_then(Value::as_str).unwrap_or("").to_string(),
            })
        })
        .filter(|r| !r.is_empty())
        .collect();
    if decisions
        .iter()
        .any(|d| d.get("approved").and_then(Value::as_bool) == Some(false) || d.get("rejected").and_then(Value::as_bool) == Some(true))
    {
        return Err(format!(
            "{name} doesn't want it{}",
            if rejected.is_empty() { String::new() } else { format!(": {}", rejected.join("; ")) }
        ));
    }
    Ok(format!("Sent to {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_addresses() {
        assert_eq!(push_url("sonarr", "http://sonarr:8989/"), "http://sonarr:8989/api/v3/release/push");
        assert_eq!(push_url("lidarr", "http://lidarr:8686"), "http://lidarr:8686/api/v1/release/push");
    }

    #[tokio::test]
    async fn without_the_hub_a_client_push_says_so() {
        assert!(NoHub.add(&Grab::default()).await.unwrap_err().contains("download hub"));
    }
}
