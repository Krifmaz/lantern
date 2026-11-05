//! The download-client layer: one interface over every program that downloads for Lantern. Apps never see which
//! client a grab went to; the hub picks one by protocol and priority.
//!
//! | Kind | Protocol | Module |
//! | --- | --- | --- |
//! | `qbittorrent` (the default for torrents) | torrent | [`qbittorrent`] |
//! | `transmission` | torrent | [`transmission`] |
//! | `deluge` | torrent | [`deluge`] |
//! | `rtorrent` | torrent | [`rtorrent`] (over [`xmlrpc`]) |
//! | `sabnzbd` (the default for Usenet) | usenet | [`sabnzbd`] |
//! | `nzbget` | usenet | [`nzbget`] |

pub mod deluge;
pub mod nzbget;
pub mod qbittorrent;
pub mod rtorrent;
pub mod sabnzbd;
pub mod transmission;
pub mod xmlrpc;

use crate::indexers::release::Protocol;
use kit_server_core::{Row, RowExt};
use serde::Serialize;
use std::time::Duration;

/// Every kind of download client Lantern talks to, with the protocol it downloads and the name people see.
pub const KINDS: [(&str, Protocol, &str); 6] = [
    ("qbittorrent", Protocol::Torrent, "qBittorrent"),
    ("transmission", Protocol::Torrent, "Transmission"),
    ("deluge", Protocol::Torrent, "Deluge"),
    ("rtorrent", Protocol::Torrent, "rTorrent"),
    ("sabnzbd", Protocol::Usenet, "SABnzbd"),
    ("nzbget", Protocol::Usenet, "NZBGet"),
];

/// The kinds new grabs of each protocol go to first when no client says otherwise.
pub const DEFAULTS: [&str; 2] = ["qbittorrent", "sabnzbd"];

/// The protocol a kind of client downloads; `None` for a kind Lantern doesn't know.
pub fn protocol_of(kind: &str) -> Option<Protocol> {
    KINDS.iter().find(|(k, _, _)| *k == kind).map(|(_, p, _)| *p)
}

/// A download client as stored (`download_clients`).
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Its id.
    pub id: i64,
    /// One of [`KINDS`].
    pub kind: String,
    /// The name people see.
    pub name: String,
    /// Its web address (`http://qbittorrent:8080`).
    pub url: String,
    /// The user name it signs in with (all but SABnzbd and Deluge).
    pub username: String,
    /// The password it signs in with (all but SABnzbd). Never sent back by the API.
    pub password: String,
    /// The API key (SABnzbd). Never sent back by the API.
    pub api_key: String,
    /// Where the client sees Lantern's downloads folder (`/data/downloads`); empty when it sees it at the same path.
    pub root: String,
    /// Used for new grabs at all.
    pub enabled: bool,
    /// Lower is used first (1–50).
    pub priority: i64,
    /// When it was added.
    pub added: f64,
}

impl ClientConfig {
    /// The client from its database row.
    pub fn from_row(r: &Row) -> ClientConfig {
        ClientConfig {
            id: r.i("id"),
            kind: r.s("kind"),
            name: r.s("name"),
            url: r.s("url"),
            username: r.s("username"),
            password: r.s("password"),
            api_key: r.s("api_key"),
            root: r.s("root"),
            enabled: r.b("enabled"),
            priority: r.i("priority"),
            added: r.f("added"),
        }
    }

    /// Torrent or Usenet.
    pub fn protocol(&self) -> Protocol {
        protocol_of(&self.kind).unwrap_or(Protocol::Torrent)
    }

    /// What changes the running client: a new one is made when this differs.
    pub fn fingerprint(&self) -> String {
        format!("{}|{}|{}|{}|{}", self.kind, self.url, self.username, self.password, self.api_key)
    }
}

/// What is sent to a client.
#[derive(Debug, Clone)]
pub enum Content {
    /// A magnet link.
    Magnet(String),
    /// A `.torrent` file.
    Torrent(Vec<u8>),
    /// An `.nzb` file.
    Nzb(Vec<u8>),
}

impl Content {
    /// Torrent or Usenet.
    pub fn protocol(&self) -> Protocol {
        match self {
            Content::Nzb(_) => Protocol::Usenet,
            _ => Protocol::Torrent,
        }
    }
}

/// One download to start.
#[derive(Debug, Clone)]
pub struct AddRequest {
    /// What to download.
    pub content: Content,
    /// The release's name.
    pub title: String,
    /// The app's category.
    pub category: String,
    /// Where the files go, as the client sees it.
    pub save_path: String,
    /// The mark that finds this download again (`lantern-12`).
    pub tag: String,
}

/// Where a download is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemState {
    /// Waiting its turn in the client.
    Queued,
    /// Downloading.
    Downloading,
    /// Paused by someone.
    Paused,
    /// Downloaded and still sharing (torrents).
    Seeding,
    /// Downloaded and done.
    Completed,
    /// Failed in the client.
    Failed,
}

impl ItemState {
    /// Whether every file is there.
    pub fn done(self) -> bool {
        matches!(self, ItemState::Seeding | ItemState::Completed)
    }
}

/// A download as a client reports it.
#[derive(Debug, Clone)]
pub struct Item {
    /// The client's id for it (info hash, nzo id).
    pub id: String,
    /// Lantern's mark on it, when the client keeps one (`lantern-12`).
    pub tag: String,
    /// Its name.
    pub name: String,
    /// Where it is.
    pub state: ItemState,
    /// How much is done, 0 to 1.
    pub progress: f64,
    /// Its size in bytes.
    pub size: i64,
    /// Bytes per second now.
    pub speed: i64,
    /// Seconds left (0: unknown or done).
    pub eta: i64,
    /// Its file or folder, as the client sees it.
    pub path: String,
    /// The folder its files are in, as the client sees it.
    pub save_path: String,
    /// Uploaded divided by downloaded (torrents).
    pub ratio: f64,
    /// Seconds spent seeding (torrents).
    pub seed_secs: i64,
    /// Why it failed, in words.
    pub error: String,
}

/// A running client, made from its [`ClientConfig`].
pub enum Backend {
    /// qBittorrent's Web API.
    Qbittorrent(qbittorrent::Qbittorrent),
    /// Transmission's RPC.
    Transmission(transmission::Transmission),
    /// Deluge's Web UI JSON-RPC.
    Deluge(deluge::Deluge),
    /// rTorrent's XML-RPC.
    Rtorrent(rtorrent::Rtorrent),
    /// SABnzbd's API.
    Sabnzbd(sabnzbd::Sabnzbd),
    /// NZBGet's JSON-RPC.
    Nzbget(nzbget::Nzbget),
}

impl Backend {
    /// The running client for `cfg`; an error in words for a kind Lantern doesn't know.
    pub fn new(cfg: &ClientConfig) -> Result<Backend, String> {
        let http = http_client(true);
        match cfg.kind.as_str() {
            "qbittorrent" => Ok(Backend::Qbittorrent(qbittorrent::Qbittorrent::new(cfg, http))),
            "transmission" => Ok(Backend::Transmission(transmission::Transmission::new(cfg, http_client(false)))),
            "deluge" => Ok(Backend::Deluge(deluge::Deluge::new(cfg, http))),
            "rtorrent" => Ok(Backend::Rtorrent(rtorrent::Rtorrent::new(cfg, http_client(false)))),
            "sabnzbd" => Ok(Backend::Sabnzbd(sabnzbd::Sabnzbd::new(cfg, http))),
            "nzbget" => Ok(Backend::Nzbget(nzbget::Nzbget::new(cfg, http_client(false)))),
            k => Err(format!("Lantern doesn't know the download client kind {k}")),
        }
    }

    /// Signs in and asks the version: the version, or why it doesn't answer.
    pub async fn test(&self) -> Result<String, String> {
        match self {
            Backend::Qbittorrent(c) => c.test().await,
            Backend::Transmission(c) => c.test().await,
            Backend::Deluge(c) => c.test().await,
            Backend::Rtorrent(c) => c.test().await,
            Backend::Sabnzbd(c) => c.test().await,
            Backend::Nzbget(c) => c.test().await,
        }
    }

    /// Makes (or points again) the client's category `name` at `save_path` (as the client sees it). Clients
    /// without categories (Transmission, rTorrent, NZBGet) get the folder or category with each download instead.
    pub async fn ensure_category(&self, name: &str, save_path: &str) -> Result<(), String> {
        match self {
            Backend::Qbittorrent(c) => c.ensure_category(name, save_path).await,
            Backend::Deluge(c) => c.ensure_category(name, save_path).await,
            Backend::Sabnzbd(c) => c.ensure_category(name, save_path).await,
            Backend::Transmission(_) | Backend::Rtorrent(_) | Backend::Nzbget(_) => Ok(()),
        }
    }

    /// Starts a download: the client's id for it when it says one at once, else "" (found again by its tag).
    pub async fn add(&self, req: &AddRequest) -> Result<String, String> {
        match self {
            Backend::Qbittorrent(c) => c.add(req).await,
            Backend::Transmission(c) => c.add(req).await,
            Backend::Deluge(c) => c.add(req).await,
            Backend::Rtorrent(c) => c.add(req).await,
            Backend::Sabnzbd(c) => c.add(req).await,
            Backend::Nzbget(c) => c.add(req).await,
        }
    }

    /// The downloads Lantern started in this client.
    pub async fn items(&self) -> Result<Vec<Item>, String> {
        match self {
            Backend::Qbittorrent(c) => c.items().await,
            Backend::Transmission(c) => c.items().await,
            Backend::Deluge(c) => c.items().await,
            Backend::Rtorrent(c) => c.items().await,
            Backend::Sabnzbd(c) => c.items().await,
            Backend::Nzbget(c) => c.items().await,
        }
    }

    /// The files of a finished download: (path as the client sees it, size). An empty list when the client
    /// doesn't list them (the hub then reads the folder).
    pub async fn files(&self, item: &Item) -> Result<Vec<(String, i64)>, String> {
        match self {
            Backend::Qbittorrent(c) => c.files(item).await,
            Backend::Transmission(c) => c.files(item).await,
            Backend::Deluge(c) => c.files(item).await,
            Backend::Rtorrent(c) => c.files(item).await,
            Backend::Sabnzbd(_) | Backend::Nzbget(_) => Ok(Vec::new()),
        }
    }

    /// Removes a download from the client, and its files when `delete_files`.
    pub async fn remove(&self, id: &str, delete_files: bool) -> Result<(), String> {
        match self {
            Backend::Qbittorrent(c) => c.remove(id, delete_files).await,
            Backend::Transmission(c) => c.remove(id, delete_files).await,
            Backend::Deluge(c) => c.remove(id, delete_files).await,
            Backend::Rtorrent(c) => c.remove(id, delete_files).await,
            Backend::Sabnzbd(c) => c.remove(id, delete_files).await,
            Backend::Nzbget(c) => c.remove(id, delete_files).await,
        }
    }
}

/// The HTTP client a download client uses: 30 seconds per request, cookies kept when `cookies`.
pub fn http_client(cookies: bool) -> reqwest::Client {
    reqwest::Client::builder()
        .cookie_store(cookies)
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .user_agent(format!("Lantern/{}", crate::VERSION))
        .build()
        .unwrap_or_default()
}

/// A failed request in words: what Lantern tried and what to check.
pub fn unreachable(name: &str, url: &str, e: &reqwest::Error) -> String {
    if e.is_timeout() {
        format!("{name} at {url} took too long to answer. Check that it is running.")
    } else if e.is_connect() {
        format!("Lantern can't reach {name} at {url}. Check the address and that it is running.")
    } else {
        format!("{name} at {url} didn't answer properly: {e}")
    }
}

/// `url` without a slash at the end.
pub fn trim_url(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}
