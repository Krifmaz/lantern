// Ported from autobrr v1.90.0 (internal/domain/action.go), GPL-2.0-or-later.
//! Actions: what happens to a release a filter takes. Each action has a type (a download client, an *arr, a
//! watch folder, a program, a web address) and the settings that type reads; the text fields can hold macros
//! (`{{ .TorrentName }}`), filled in from the release just before the action runs ([`Action::parse_macros`]).
//!
//! Lantern adds one type of its own, [`DOWNLOAD_HUB`]: the release goes to the download hub, which picks the
//! client and gives the download the category and folder of the hub app named in [`Action::category`].

use super::downloader::Downloader;
use super::macros::Macro;
use super::release::Release;
use serde::{Deserialize, Serialize};

/// Lantern's own action type: hand the release to the download hub.
pub const DOWNLOAD_HUB: &str = "DOWNLOAD_HUB";

/// Every action type autobrr has, then Lantern's [`DOWNLOAD_HUB`].
pub const ACTION_TYPES: [&str; 21] = [
    "TEST",
    "EXEC",
    "QBITTORRENT",
    "DELUGE_V1",
    "DELUGE_V2",
    "RTORRENT",
    "TRANSMISSION",
    "PORLA",
    "ARIA2",
    "WATCH_FOLDER",
    "WEBHOOK",
    "RADARR",
    "SONARR",
    "LIDARR",
    "WHISPARR",
    "WHISPARR_V3",
    "READARR",
    "SPORTARR",
    "SABNZBD",
    "NZBGET",
    DOWNLOAD_HUB,
];

/// For serde: leave out a false flag (Go's `omitempty`).
fn is_false(b: &bool) -> bool {
    !*b
}

/// For serde: leave out a zero (Go's `omitempty`).
fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// For serde: leave out a zero (Go's `omitempty`).
fn is_zero_f(n: &f64) -> bool {
    *n == 0.0
}

/// One action of a filter (autobrr's `Action`, with its JSON names).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Action {
    /// Its id (0 before it is saved).
    pub id: i64,
    /// The name people see.
    pub name: String,
    /// One of [`ACTION_TYPES`].
    #[serde(rename = "type")]
    pub kind: String,
    /// Runs at all.
    pub enabled: bool,
    /// EXEC: the program.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub exec_cmd: String,
    /// EXEC: its arguments, split like a shell would but never run by one (macros).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub exec_args: String,
    /// WATCH_FOLDER: the folder, or a full `.torrent` path (macros).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub watch_folder: String,
    /// qBittorrent's category, the *arr download client's, SABnzbd's and NZBGet's; the hub app (macros).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub category: String,
    /// qBittorrent's tags, comma separated (macros).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tags: String,
    /// The label (Deluge, rTorrent, Transmission's labels, Porla's preset) (macros).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// Where the files go (macros).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub save_path: String,
    /// qBittorrent: where unfinished files go (macros).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub download_path: String,
    /// Add it paused.
    #[serde(skip_serializing_if = "is_false")]
    pub paused: bool,
    /// Add it even when the client's rules say it is busy.
    #[serde(skip_serializing_if = "is_false")]
    pub ignore_rules: bool,
    /// qBittorrent: download the first and last pieces first.
    #[serde(skip_serializing_if = "is_false")]
    pub first_last_piece_prio: bool,
    /// Don't check the files' data (qBittorrent; Deluge 2's seed mode).
    #[serde(skip_serializing_if = "is_false")]
    pub skip_hash_check: bool,
    /// `ORIGINAL`, `SUBFOLDER_NONE` or `SUBFOLDER_CREATE`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub content_layout: String,
    /// Upload limit in KiB/s.
    #[serde(skip_serializing_if = "is_zero")]
    pub limit_upload_speed: i64,
    /// Download limit in KiB/s.
    #[serde(skip_serializing_if = "is_zero")]
    pub limit_download_speed: i64,
    /// Stop seeding at this ratio.
    #[serde(skip_serializing_if = "is_zero_f")]
    pub limit_ratio: f64,
    /// Stop seeding after this many minutes.
    #[serde(skip_serializing_if = "is_zero")]
    pub limit_seed_time: i64,
    /// qBittorrent: `MAX` or `MIN` in the queue; rTorrent: `LOW`, `NORMAL` or `HIGH`; empty: as the client does.
    #[serde(rename = "priority", skip_serializing_if = "String::is_empty")]
    pub priority_layout: String,
    /// rTorrent: the ruTorrent ratio group (view) to put it in.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ratio_group: String,
    /// Don't re-announce until a tracker answers.
    #[serde(rename = "reannounce_skip", skip_serializing_if = "is_false")]
    pub re_announce_skip: bool,
    /// Remove it when the re-announces never work.
    #[serde(rename = "reannounce_delete", skip_serializing_if = "is_false")]
    pub re_announce_delete: bool,
    /// Seconds between re-announces.
    #[serde(rename = "reannounce_interval", skip_serializing_if = "is_zero")]
    pub re_announce_interval: i64,
    /// The most re-announces.
    #[serde(rename = "reannounce_max_attempts", skip_serializing_if = "is_zero")]
    pub re_announce_max_attempts: i64,
    /// WEBHOOK: the address it posts to.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub webhook_host: String,
    /// WEBHOOK: stored; autobrr's webhook action doesn't read it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub webhook_type: String,
    /// WEBHOOK: stored; autobrr's webhook action always posts.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub webhook_method: String,
    /// WEBHOOK: the JSON it posts (macros).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub webhook_data: String,
    /// WEBHOOK: taken but not kept, as autobrr's database has no place for it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub webhook_headers: Vec<String>,
    /// The *arrs: the id of their own download client to use (replaces the client's setting).
    #[serde(skip_serializing_if = "is_zero")]
    pub external_download_client_id: i64,
    /// The *arrs: the name of their own download client to use (replaces the client's setting).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub external_download_client: String,
    /// The filter it belongs to.
    #[serde(skip_serializing_if = "is_zero")]
    pub filter_id: i64,
    /// The download client it uses ([`Downloader`] id).
    #[serde(skip_serializing_if = "is_zero")]
    pub client_id: i64,
    /// The download client itself, when loaded with it.
    #[serde(skip_serializing_if = "Option::is_none", skip_deserializing)]
    pub client: Option<Downloader>,
}

/// Whether any of `fields` names any of `macros` (autobrr's `containsAnyMacro`).
pub fn contains_any_macro(fields: &[&str], macros: &[&str]) -> bool {
    fields.iter().filter(|f| !f.is_empty()).any(|f| macros.iter().any(|m| f.contains(m)))
}

impl Action {
    /// Whether the action sends the torrent file itself, so the release is fetched once before the actions
    /// run (autobrr's `NeedsTorrentDownloaded`).
    pub fn needs_torrent_downloaded(&self) -> bool {
        matches!(
            self.kind.as_str(),
            "QBITTORRENT" | "DELUGE_V1" | "DELUGE_V2" | "RTORRENT" | "TRANSMISSION" | "PORLA" | "ARIA2" | "WATCH_FOLDER"
        )
    }

    /// The fields whose macros are filled in and could name the torrent file.
    fn macro_fields(&self) -> [&str; 5] {
        [&self.exec_args, &self.watch_folder, &self.save_path, &self.download_path, &self.webhook_data]
    }

    /// Whether a macro needs the torrent written to a file (autobrr's `CheckMacrosNeedTorrentTmpFile`).
    pub fn check_macros_need_torrent_tmp_file(&self, release: &Release) -> bool {
        if !release.torrent_tmp_file.is_empty() {
            return false;
        }
        contains_any_macro(&self.macro_fields(), &["TorrentPathName", "TorrentTmpFile"])
    }

    /// Whether the action or a macro needs the torrent's bytes (autobrr's `CheckMacrosNeedRawDataBytes`).
    pub fn check_macros_need_raw_data_bytes(&self, release: &Release) -> bool {
        if !release.torrent_data_raw_bytes.is_empty() {
            return false;
        }
        if self.kind == "WATCH_FOLDER" {
            return true;
        }
        contains_any_macro(&self.macro_fields(), &["TorrentPathName", "TorrentTmpFile", "TorrentDataRawBytes", "TorrentHash"])
    }

    /// Fills in the macros of the text fields from `release` (autobrr's `ParseMacros`); which field failed and
    /// why.
    pub fn parse_macros(&mut self, release: &Release) -> Result<(), String> {
        let m = Macro::new(release);
        let fields: [(&mut String, &str); 8] = [
            (&mut self.exec_args, "could not parse exec args"),
            (&mut self.watch_folder, "could not parse watch folder"),
            (&mut self.category, "could not parse category"),
            (&mut self.tags, "could not parse tags"),
            (&mut self.label, "could not parse label"),
            (&mut self.save_path, "could not parse save_path"),
            (&mut self.download_path, "could not parse download_path"),
            (&mut self.webhook_data, "could not parse webhook_data"),
        ];
        for (field, what) in fields {
            *field = m.parse(field).map_err(|e| format!("{what}: {e}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! autobrr's internal/domain/release_torrent_test.go: `TestAction_NeedsTorrentDownloaded`,
    //! `TestAction_CheckMacrosNeedTorrent`.
    use super::*;

    #[test]
    fn needs_torrent_downloaded() {
        let cases = [
            ("QBITTORRENT", true),
            ("DELUGE_V1", true),
            ("DELUGE_V2", true),
            ("RTORRENT", true),
            ("TRANSMISSION", true),
            ("PORLA", true),
            ("ARIA2", true),
            ("WATCH_FOLDER", true),
            ("RADARR", false),
            ("SONARR", false),
            ("SABNZBD", false),
            ("EXEC", false),
            ("WEBHOOK", false),
            ("TEST", false),
            (DOWNLOAD_HUB, false),
        ];
        for (kind, want) in cases {
            assert_eq!(Action { kind: kind.into(), ..Action::default() }.needs_torrent_downloaded(), want, "{kind}");
        }
    }

    #[test]
    fn check_macros_need_torrent() {
        let a = |f: fn(&mut Action)| {
            let mut a = Action { kind: "EXEC".into(), ..Action::default() };
            f(&mut a);
            a
        };
        let cases = [
            ("path macro in exec args needs both", a(|a| a.exec_args = "--file {{.TorrentPathName}}".into()), true, true),
            ("raw bytes macro needs contents only", a(|a| a.exec_args = "--data {{.TorrentDataRawBytes}}".into()), true, false),
            (
                "hash macro needs contents only",
                a(|a| {
                    a.kind = "WEBHOOK".into();
                    a.webhook_data = r#"{"hash":"{{.TorrentHash}}"}"#.into();
                }),
                true,
                false,
            ),
            ("path macro in watch folder is covered", a(|a| a.watch_folder = "/watch/{{.TorrentPathName}}".into()), true, true),
            ("path macro in save path is covered", a(|a| a.save_path = "/data/{{.TorrentPathName}}".into()), true, true),
            (
                "unrelated macros need nothing",
                a(|a| {
                    a.exec_args = "--name {{.TorrentName}}".into();
                    a.save_path = "/data/{{.Indexer}}".into();
                }),
                false,
                false,
            ),
        ];
        let r = Release { torrent_name: "Test.Release-GRP".into(), ..Release::default() };
        for (name, action, bytes, file) in cases {
            assert_eq!(action.check_macros_need_raw_data_bytes(&r), bytes, "{name}: raw data bytes");
            assert_eq!(action.check_macros_need_torrent_tmp_file(&r), file, "{name}: tmp file");
        }
    }

    #[test]
    fn json_is_autobrrs() {
        let a: Action = serde_json::from_value(serde_json::json!({
            "id": 3, "name": "qb", "type": "QBITTORRENT", "enabled": true, "priority": "MAX",
            "reannounce_skip": true, "reannounce_interval": 7, "client_id": 2, "webhook_headers": ["A=b"]
        }))
        .unwrap();
        assert_eq!((a.priority_layout.as_str(), a.re_announce_skip, a.re_announce_interval, a.client_id), ("MAX", true, 7, 2));
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["type"], "QBITTORRENT");
        assert!(v.get("exec_cmd").is_none(), "empty fields are left out");
        assert!(v.get("paused").is_none());
    }
}
