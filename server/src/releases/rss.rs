//! RSS sources: a feed address (a tracker's RSS, any Torznab feed) or the newest releases of one of Lantern's own
//! indexers, looked at every few minutes. Only items not seen before go to the filters; the first look at a new
//! source only notes what is there, so adding a feed never grabs its whole backlog.

use super::{Found, Releases, Source};
use crate::indexers::release::{Protocol, Query, Release};
use std::time::Duration;

/// Starts the loop that looks at every due RSS and indexer source (every 30 seconds it checks which are due).
pub fn spawn(releases: Releases) {
    tokio::spawn(async move {
        let mut rounds = 0u64;
        loop {
            if let Ok(list) = releases.sources() {
                let now = crate::auth::now();
                for s in list.into_iter().filter(|s| s.enabled && s.kind != "irc" && s.last_at + s.interval_min as f64 * 60.0 <= now) {
                    // noted at once, so a slow feed isn't looked at twice
                    releases.source_result(s.id, &s.last_error);
                    let r = releases.clone();
                    tokio::spawn(async move {
                        let err = poll(&r, &s).await.err().unwrap_or_default();
                        r.source_result(s.id, &err);
                    });
                }
            }
            rounds += 1;
            if rounds.is_multiple_of(2880) {
                releases.forget_old();
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
}

/// Looks at source `s` once; how many new releases went to the filters, or the problem in words.
pub async fn poll(releases: &Releases, s: &Source) -> Result<usize, String> {
    let items = fetch(releases, s).await?;
    let first = !releases.has_seen(s.id);
    let mut new = Vec::new();
    for r in items {
        let key = if r.guid.is_empty() { r.title.clone() } else { r.guid.clone() };
        if releases.see(s.id, &key) && !first {
            new.push(r);
        }
    }
    let n = new.len();
    // oldest first, as they appeared
    for r in new.into_iter().rev() {
        let f = found(releases, s, r)?;
        let rel = releases.clone();
        tokio::spawn(async move {
            rel.handle(f).await;
        });
    }
    Ok(n)
}

/// The items of `s` now.
async fn fetch(releases: &Releases, s: &Source) -> Result<Vec<Release>, String> {
    if s.kind == "indexer" {
        let ix = releases.indexers.get(s.indexer_id).map_err(|_| "The indexer of this source was removed".to_string())?;
        return releases.indexers.search_one(&ix, &Query { rss: true, ..Query::default() }, "releases").await;
    }
    let resp = reqwest::Client::new()
        .get(&s.url)
        .header("User-Agent", concat!("Lantern/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("The feed at {} can't be reached: {}", crate::indexers::http::strip_secrets(&s.url), e.without_url()))?;
    if !resp.status().is_success() {
        return Err(format!("The feed at {} answered {}", crate::indexers::http::strip_secrets(&s.url), resp.status().as_u16()));
    }
    let text = resp.text().await.map_err(|e| format!("The feed couldn't be read: {}", e.without_url()))?;
    if let Some(e) = crate::indexers::newznab::api_error(&text) {
        return Err(format!("The feed said: {e}"));
    }
    Ok(crate::indexers::newznab::parse_rss(&text, Protocol::Torrent))
}

/// A feed item as a release for the filters.
fn found(releases: &Releases, s: &Source, r: Release) -> Result<Found, String> {
    let mut download_url = if r.link.is_empty() { r.magnet.clone() } else { r.link.clone() };
    if s.kind == "indexer" && !download_url.starts_with("magnet:") {
        // the site's link may need Lantern's sign-in: apps get Lantern's signed link instead
        let base = releases.settings.str("public_url").trim().trim_end_matches('/').to_string();
        if base.is_empty() {
            return Err("Set Lantern's address for apps (Settings → General) so apps can download these releases through Lantern".into());
        }
        let (link, sig) = releases.signer.sign(r.indexer_id, &r.link);
        let file: String = url::form_urlencoded::byte_serialize(r.title.as_bytes()).collect();
        let key: String = url::form_urlencoded::byte_serialize(releases.settings.str("api_key").as_bytes()).collect();
        download_url = format!("{base}/{}/download?apikey={key}&link={link}&sig={sig}&file={file}", r.indexer_id);
    }
    let freeleech = r.freeleech();
    Ok(Found {
        title: r.title.clone(),
        source_id: s.id,
        source_name: s.name.clone(),
        protocol: if r.protocol == Protocol::Usenet { "usenet".into() } else { "torrent".into() },
        download_url,
        info_url: r.details.clone(),
        size: r.size,
        category: r.categories.first().and_then(|c| crate::indexers::categories::name_of(*c)).map(str::to_string).unwrap_or_default(),
        freeleech,
        freeleech_percent: r.download_factor.map(|d| ((1.0 - d.clamp(0.0, 1.0)) * 100.0).round() as u32),
        uploader: String::new(),
        tags: r.genre.split(',').map(|g| g.trim().to_string()).filter(|g| !g.is_empty()).collect(),
        year: r.year.and_then(|y| u32::try_from(y).ok()),
        ..Found::default()
    })
}
