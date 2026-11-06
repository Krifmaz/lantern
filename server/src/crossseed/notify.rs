// Ported from cross-seed v6.13.9 (src/pushNotifier.ts), Apache-2.0.
//! Notification webhooks (`notificationWebhookUrls`): after a searchee's matches are added or saved, or fail,
//! each address gets cross-seed's JSON message by POST: `{title, body, extra: {event, name, infoHashes, trackers,
//! source, result, paused, decisions, searchee}}`. A failure is only logged. As in cross-seed, `trackers` and
//! `decisions` come sorted (its list formatting sorts them in place).

use super::clients::estimate_paused_status;
use super::config::RuntimeConfig;
use super::constants::{Decision, InjectionResult};
use super::searchee::Searchee;
use super::torrent::{locale_cmp, Metafile};
use super::utils::format_as_list;
use serde_json::{json, Value};
use std::time::Duration;

/// `PROGRAM_NAME`: the title of every message.
pub const PROGRAM_NAME: &str = "cross-seed";

/// One match's outcome: its decision, its torrent, its tracker and what was done.
#[derive(Debug, Clone)]
pub struct Notable {
    /// The decision.
    pub decision: Decision,
    /// The match's torrent.
    pub meta: Metafile,
    /// The tracker it came from.
    pub tracker: String,
    /// What was done.
    pub result: InjectionResult,
}

/// `getSearcheeSource`: `torrentClient`, `torrentFile`, `dataDir` or `virtual`.
pub fn searchee_source(s: &Searchee) -> &'static str {
    if s.save_path.is_some() {
        "torrentClient"
    } else if s.info_hash.is_some() {
        "torrentFile"
    } else if s.path.is_some() {
        "dataDir"
    } else {
        "virtual"
    }
}

/// The `searchee` part of a message.
fn searchee_json(s: &Searchee) -> Value {
    json!({
        "category": s.category,
        "tags": s.tags,
        "trackers": s.trackers,
        "length": s.length,
        "clientHost": s.client_host,
        "infoHash": s.info_hash,
        "path": s.path,
        "source": searchee_source(s),
    })
}

/// `sendResultsNotification`'s messages for one searchee: one for the matches added or saved, one for those that
/// failed (each only when there are some).
pub fn results_messages(s: &Searchee, results: &[Notable], cfg: &RuntimeConfig) -> Vec<Value> {
    let source = s.label.as_str();
    let mut out = Vec::new();
    let s_source = searchee_source(s);
    let successes: Vec<&Notable> =
        results.iter().filter(|r| matches!(r.result, InjectionResult::Success | InjectionResult::Saved)).collect();
    if let Some(first) = successes.first() {
        let name = first.meta.name.clone();
        let n = successes.len();
        let info_hashes: Vec<String> = successes.iter().map(|r| r.meta.info_hash.clone()).collect();
        let mut trackers: Vec<String> = successes.iter().map(|r| r.tracker.clone()).collect();
        trackers.sort_by(|a, b| locale_cmp(a, b));
        let fallback = [Decision::Match, Decision::MatchSizeOnly]
            .iter()
            .find_map(|d| successes.iter().find(|r| r.decision == *d && r.result == InjectionResult::Success).map(|r| r.decision))
            .unwrap_or(Decision::MatchPartial);
        let paused = successes.iter().all(|r| r.result == InjectionResult::Saved)
            || successes.iter().any(|r| estimate_paused_status(&r.meta, s, fallback, cfg));
        let injected = successes.iter().any(|r| r.result == InjectionResult::Success);
        let performed = if injected { format!("Injected{}", if paused { " (paused)" } else { "" }) } else { "Saved".to_string() };
        let mut decisions: Vec<String> = successes.iter().map(|r| r.decision.as_str().to_string()).collect();
        decisions.sort_by(|a, b| locale_cmp(a, b));
        out.push(json!({
            "title": PROGRAM_NAME,
            "body": format!(
                "{source}: {performed} {name} on {n} tracker{} by {} from {s_source}: {}",
                if n != 1 { "s" } else { "" },
                format_as_list(&decisions, true, "long", "conjunction"),
                format_as_list(&trackers, true, "long", "conjunction")
            ),
            "extra": {
                "event": "RESULTS",
                "name": name,
                "infoHashes": info_hashes,
                "trackers": trackers,
                "source": source,
                "result": if injected { InjectionResult::Success.as_str() } else { InjectionResult::Saved.as_str() },
                "paused": paused,
                "decisions": decisions,
                "searchee": searchee_json(s),
            },
        }));
    }
    let failures: Vec<&Notable> = results.iter().filter(|r| r.result == InjectionResult::Failure).collect();
    if let Some(first) = failures.first() {
        let name = first.meta.name.clone();
        let n = failures.len();
        let info_hashes: Vec<String> = failures.iter().map(|r| r.meta.info_hash.clone()).collect();
        let mut trackers: Vec<String> = failures.iter().map(|r| r.tracker.clone()).collect();
        trackers.sort_by(|a, b| locale_cmp(a, b));
        let mut decisions: Vec<String> = failures.iter().map(|r| r.decision.as_str().to_string()).collect();
        decisions.sort_by(|a, b| locale_cmp(a, b));
        out.push(json!({
            "title": PROGRAM_NAME,
            "body": format!(
                "{source}: Failed to inject {name} on {n} tracker{} by {} from {s_source}: {}",
                if n != 1 { "s" } else { "" },
                format_as_list(&decisions, true, "long", "conjunction"),
                format_as_list(&trackers, true, "long", "conjunction")
            ),
            "extra": {
                "event": "RESULTS",
                "name": name,
                "infoHashes": info_hashes,
                "trackers": trackers,
                "source": source,
                "result": first.result.as_str(),
                "paused": false,
                "decisions": decisions,
                "searchee": searchee_json(s),
            },
        }));
    }
    out
}

/// `PushNotifier.notify`: sends `message` to every address at once (5 minutes at most each); a refusal or a
/// failure is logged.
pub async fn notify(urls: &[String], message: &Value) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .user_agent(format!("Lantern/{}", crate::VERSION))
        .build()
        .unwrap_or_default();
    let sends = urls.iter().map(|url| {
        let client = client.clone();
        async move {
            match client.post(url).header("Content-Type", "application/json").body(message.to_string()).send().await {
                Ok(r) if !r.status().is_success() => {
                    let status = r.status();
                    let text = r.text().await.unwrap_or_default();
                    tracing::error!(
                        "cross-seed: {url} rejected push notification: {} {}",
                        status.as_u16(),
                        status.canonical_reason().unwrap_or("")
                    );
                    let short: String = text.chars().take(100).collect();
                    tracing::debug!("cross-seed: {url}: {short}{}", if text.chars().count() > 100 { "..." } else { "" });
                }
                Ok(_) => {}
                Err(e) => tracing::error!("cross-seed: {url} failed to send push notification: {e}"),
            }
        }
    });
    futures_util::future::join_all(sends).await;
}

/// `sendResultsNotification`: sends the messages for one searchee's results in the background.
pub fn send_results_notification(s: &Searchee, results: &[Notable], cfg: &RuntimeConfig) {
    if cfg.notification_webhook_urls.is_empty() {
        return;
    }
    let messages = results_messages(s, results, cfg);
    let urls = cfg.notification_webhook_urls.clone();
    tokio::spawn(async move {
        for m in messages {
            notify(&urls, &m).await;
        }
    });
}

/// `sendTestNotification`: cross-seed's test message (`{"title":"cross-seed","body":"Test","extra":{"event":"TEST"}}`).
pub async fn send_test_notification(urls: &[String]) {
    notify(urls, &json!({ "title": PROGRAM_NAME, "body": "Test", "extra": { "event": "TEST" } })).await;
    tracing::info!("cross-seed: Sent test notification");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crossseed::searchee::Label;
    use crate::crossseed::torrent::build;

    #[test]
    fn messages_as_cross_seed_sends_them() {
        let meta = Metafile::decode(&build("Show.S01.1080p-GRP", &[("a.mkv", 10)], "b")).unwrap();
        let s = Searchee {
            name: "Show.S01.1080p-GRP".into(),
            title: "Show.S01.1080p-GRP".into(),
            info_hash: Some("a".repeat(40)),
            save_path: Some("/dl".into()),
            client_host: Some("qbit:8080".into()),
            category: Some("tv".into()),
            length: 10,
            files: meta.files.clone(),
            label: Label::Search,
            ..Searchee::default()
        };
        let cfg = RuntimeConfig::default();
        let ok = Notable { decision: Decision::Match, meta: meta.clone(), tracker: "Site B".into(), result: InjectionResult::Success };
        let saved = Notable { decision: Decision::MatchSizeOnly, tracker: "Site A".into(), result: InjectionResult::Saved, ..ok.clone() };
        let failed = Notable { tracker: "Site C".into(), result: InjectionResult::Failure, ..ok.clone() };
        let m = results_messages(&s, &[ok, saved, failed], &cfg);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0]["title"], "cross-seed");
        assert_eq!(
            m[0]["body"],
            "search: Injected Show.S01.1080p-GRP on 2 trackers by MATCH and MATCH_SIZE_ONLY from torrentClient: Site A and Site B"
        );
        assert_eq!(m[0]["extra"]["result"], "INJECTED");
        assert_eq!(m[0]["extra"]["paused"], false, "a full match is not paused");
        assert_eq!(m[0]["extra"]["trackers"], json!(["Site A", "Site B"]), "sorted, as cross-seed's formatAsList leaves them");
        assert_eq!(m[0]["extra"]["infoHashes"], json!([meta.info_hash, meta.info_hash]));
        assert_eq!(m[0]["extra"]["searchee"]["source"], "torrentClient");
        assert_eq!(m[0]["extra"]["searchee"]["path"], Value::Null);
        assert_eq!(m[1]["body"], "search: Failed to inject Show.S01.1080p-GRP on 1 tracker by MATCH from torrentClient: Site C");
        assert_eq!(m[1]["extra"]["result"], "FAILURE");
        assert_eq!(m[1]["extra"]["paused"], false);
        assert_eq!(searchee_source(&Searchee { path: Some("/d".into()), ..Searchee::default() }), "dataDir");
        assert_eq!(searchee_source(&Searchee::default()), "virtual");
    }
}
