//! Lantern dressed as a download client, so any app that supports one can add Lantern instead. Each app signs in
//! with its own name and Lantern's API key; Lantern registers it on first sight, takes its grabs, sends them to the
//! real client through the hub, and answers in the format the app expects.
//!
//! | Module | Speaks |
//! | --- | --- |
//! | [`qbittorrent`] | qBittorrent's Web API v2 (`/api/v2/…`) |
//! | [`sabnzbd`] | SABnzbd's API (`/api`, `/sabnzbd/api`) |
//! | [`nzbget`] | NZBGet's JSON-RPC (`/jsonrpc`) |

pub mod nzbget;
pub mod qbittorrent;
pub mod sabnzbd;

use super::apps::App;
use super::Hub;
use kit_server_core::{p, Db, Result, RowExt};
use serde_json::{json, Value};

/// The categories apps use out of the box, in small letters (Sonarr `tv`, Radarr `movies`, Lidarr `music`, Readarr
/// `readarr`); each is offered with a capital too.
pub const COMMON_CATEGORIES: [&str; 9] = ["tv", "movies", "music", "books", "sonarr", "radarr", "lidarr", "readarr", "audiobooks"];

impl Hub {
    /// The app named `name` that connects through an emulated client `source`, registered when it is new.
    pub fn emulated_app(&self, name: &str, source: &str) -> Result<App> {
        let name = name.trim();
        let name = if name.is_empty() { source } else { name };
        if let Some(r) = self.db.one("SELECT * FROM hub_apps WHERE name=?", p![name])? {
            let a = App::from_row(&r);
            self.seen(a.id);
            return Ok(a);
        }
        let (app, problem) = self.register(&json!({ "name": name, "source": source }))?;
        if let Some(p) = problem {
            tracing::warn!("{p}");
        }
        Ok(app)
    }

    /// The categories app `id` made in its emulated client (`tv-sonarr`).
    pub fn ext_categories(&self, id: i64) -> Vec<String> {
        self.db
            .one("SELECT ext_categories FROM hub_apps WHERE id=?", p![id])
            .ok()
            .flatten()
            .and_then(|r| serde_json::from_str::<Vec<String>>(&r.s("ext_categories")).ok())
            .unwrap_or_default()
    }

    /// The categories an emulated client offers `apps`: the ones they made, their own, and the usual defaults of
    /// Sonarr, Radarr, Lidarr and Readarr, since apps check that their category exists before they save the
    /// client, and Lantern takes any category anyway.
    pub fn offered_categories(&self, apps: &[super::apps::App]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for a in apps {
            out.extend(self.ext_categories(a.id));
            out.push(a.category.clone());
        }
        // apps compare names exactly, and some defaults start with a capital (Radarr's NZBGet `Movies`)
        for c in COMMON_CATEGORIES {
            out.push(c.to_string());
            let mut cap = c.chars();
            if let Some(first) = cap.next() {
                out.push(first.to_uppercase().chain(cap).collect());
            }
        }
        let mut seen = std::collections::HashSet::new();
        out.retain(|c| !c.is_empty() && seen.insert(c.clone()));
        out
    }

    /// Adds (`keep`) or drops category `name` of app `id`'s emulated client.
    pub fn set_ext_category(&self, id: i64, name: &str, keep: bool) -> Result<()> {
        let mut list = self.ext_categories(id);
        list.retain(|c| c != name);
        if keep && !name.trim().is_empty() {
            list.push(name.trim().to_string());
        }
        list.sort();
        self.db.execute("UPDATE hub_apps SET ext_categories=? WHERE id=?", p![Value::from(list).to_string(), id])?;
        Ok(())
    }
}
