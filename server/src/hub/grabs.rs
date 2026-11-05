//! Grabs: a release an app asked for, from the moment it is asked until its files are there.
//!
//! ```text
//! queued ──send──▶ waiting ─▶ downloading ⇄ paused ─▶ seeding ─▶ completed
//!   ▲  │                                         │
//!   └──┴─ retrying (sending or the client failed) ┴─▶ failed (after the last attempt)
//! ```
//!
//! [`Hub::tick`] does one step for every grab: sends the queued ones whose turn has come, asks each client where
//! its downloads are, then cleans up what apps imported (see [`super::cleanup`]). Sending again waits 30 seconds, 2, 10, 30 then 60 minutes.

use super::apps::App;
use super::clients::{AddRequest, ClientConfig, Content, Item, ItemState};
use super::{paths, text, Hub};
use crate::indexers::cardigann::{looks_like_nzb, Grabbed};
use crate::indexers::release::Protocol;
use kit_server_core::{err, p, Db, Result, Row, RowExt};
use serde_json::{json, Value};
use std::collections::HashMap;

/// The grabs Lantern still follows in a client.
pub const IN_CLIENT: [&str; 4] = ["waiting", "downloading", "paused", "seeding"];

/// How a grab's link names a release file kept in the data folder (`lantern-file:12.torrent`).
pub const FILE_LINK: &str = "lantern-file:";

/// Seconds before sending again, after the 1st, 2nd … failure.
const BACKOFF: [f64; 5] = [30.0, 120.0, 600.0, 1800.0, 3600.0];

/// How long a download may be missing from its client right after it was sent (the client may not list it yet).
const GRACE: f64 = 120.0;

/// A grab, as stored (`hub_grabs`).
#[derive(Debug, Clone)]
pub struct Grab {
    /// Its id.
    pub id: i64,
    /// The app that asked.
    pub app_id: i64,
    /// The release's name.
    pub title: String,
    /// `torrent` or `usenet` (as far as known before the file is fetched).
    pub protocol: String,
    /// The indexer it comes from (0: an address the app gave).
    pub indexer_id: i64,
    /// The release's link (the indexer's, a magnet, or the address the app gave).
    pub link: String,
    /// The download client it went to (0: not sent yet).
    pub client_id: i64,
    /// The client's id for it (info hash, nzo id; empty until known).
    pub client_ref: String,
    /// The app's category when it was sent.
    pub category: String,
    /// `queued`, `waiting`, `downloading`, `paused`, `seeding`, `completed`, `failed` or `removed`.
    pub state: String,
    /// How much is done, 0 to 1.
    pub progress: f64,
    /// Its size in bytes.
    pub size: i64,
    /// Bytes per second now.
    pub speed: i64,
    /// Seconds left.
    pub eta: i64,
    /// Its file or folder, as Lantern sees it (once done).
    pub path: String,
    /// Its files as Lantern sees them, with their sizes (once done).
    pub files: Vec<(String, i64)>,
    /// The last failure, in words.
    pub error: String,
    /// How many times it was sent or failed.
    pub attempts: i64,
    /// Not sent again before this time.
    pub next_try: f64,
    /// When it last went to a client.
    pub sent: f64,
    /// When the app asked.
    pub added: f64,
    /// When it last changed.
    pub updated: f64,
    /// When its files were all there (or it failed for good).
    pub finished: f64,
    /// Uploaded divided by downloaded, as its client last said (torrents).
    pub ratio: f64,
    /// Seconds it has seeded, as its client last said (torrents).
    pub seed_secs: i64,
    /// When its app said it imported the files (0: not yet).
    pub imported: f64,
    /// The torrent's info hash in lower-case hex (empty: Usenet, or not known yet).
    pub infohash: String,
    /// The category the app asked for through an emulated client (`tv-sonarr`); empty for Lantern's own API.
    pub ext_category: String,
}

impl Grab {
    /// The grab from its database row.
    pub fn from_row(r: &Row) -> Grab {
        let files: Vec<(String, i64)> = serde_json::from_str(&r.s("files")).unwrap_or_default();
        Grab {
            id: r.i("id"),
            app_id: r.i("app_id"),
            title: r.s("title"),
            protocol: r.s("protocol"),
            indexer_id: r.i("indexer_id"),
            link: r.s("link"),
            client_id: r.i("client_id"),
            client_ref: r.s("client_ref"),
            category: r.s("category"),
            state: r.s("state"),
            progress: r.f("progress"),
            size: r.i("size"),
            speed: r.i("speed"),
            eta: r.i("eta"),
            path: r.s("path"),
            files,
            error: r.s("error"),
            attempts: r.i("attempts"),
            next_try: r.f("next_try"),
            sent: r.f("sent"),
            added: r.f("added"),
            updated: r.f("updated"),
            finished: r.f("finished"),
            ratio: r.f("ratio"),
            seed_secs: r.i("seed_secs"),
            imported: r.f("imported"),
            infohash: r.s("infohash"),
            ext_category: r.s("ext_category"),
        }
    }

    /// The mark it carries in a client that keeps tags (`lantern-12`).
    pub fn tag(&self) -> String {
        format!("lantern-{}", self.id)
    }
}

/// Where a grab's release comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// A release of indexer `id`, at its link.
    Indexer(i64, String),
    /// A magnet link.
    Magnet(String),
    /// A `.torrent` or `.nzb` at an address.
    Url(String),
}

/// The token a hub search hands out for a release: `<indexer>.<link>.<signature>`.
pub fn token(signer: &crate::indexers::LinkSigner, indexer: i64, link: &str) -> String {
    let (l, sig) = signer.sign(indexer, link);
    format!("{indexer}.{l}.{sig}")
}

impl Hub {
    /// Where the release of a grab request comes from: a search's `token`, or `url` / `download_url` (a magnet,
    /// one of Lantern's own download links, or any address of a `.torrent` or `.nzb`).
    pub fn source_of(&self, body: &Value) -> Result<Source> {
        if let Some(t) = text(body, "token").filter(|t| !t.is_empty()) {
            let mut parts = t.splitn(3, '.');
            let (Some(id), Some(link), Some(sig)) = (parts.next(), parts.next(), parts.next()) else {
                return Err(err(400, "This grab token is not one Lantern made. Search again and use the new one."));
            };
            let id: i64 = id.parse().map_err(|_| err(400, "This grab token is not one Lantern made. Search again and use the new one."))?;
            let link = self
                .signer
                .open_link(id, link, sig)
                .ok_or_else(|| err(400, "This grab token is not one Lantern made. Search again and use the new one."))?;
            return Ok(if link.starts_with("magnet:") { Source::Magnet(link) } else { Source::Indexer(id, link) });
        }
        let url = text(body, "url").or_else(|| text(body, "download_url")).unwrap_or_default();
        if url.is_empty() {
            return Err(err(400, "Say what to download: a search result's token, or a url (a magnet or a .torrent or .nzb address)"));
        }
        if url.starts_with("magnet:") {
            return Ok(Source::Magnet(url));
        }
        let Ok(u) = url::Url::parse(&url) else { return Err(err(400, "The address to download is not a web address or a magnet link")) };
        if !matches!(u.scheme(), "http" | "https") {
            return Err(err(400, "The address to download must start with http://, https:// or magnet:"));
        }
        // one of Lantern's own download links (`/{id}/download?link=&sig=`, `/api/v1/indexer/{id}/download`)
        let segs: Vec<&str> = u.path_segments().map(|s| s.collect()).unwrap_or_default();
        if segs.len() >= 2 && segs[segs.len() - 1] == "download" {
            if let Ok(id) = segs[segs.len() - 2].parse::<i64>() {
                let q: HashMap<String, String> = u.query_pairs().into_owned().collect();
                if let (Some(l), Some(s)) = (q.get("link"), q.get("sig")) {
                    if let Some(link) = self.signer.open_link(id, l, s) {
                        return Ok(if link.starts_with("magnet:") { Source::Magnet(link) } else { Source::Indexer(id, link) });
                    }
                }
            }
        }
        Ok(Source::Url(url))
    }

    /// Queues a grab for `app` from `body` (`token` or `url`, `title`, `protocol`); it is sent at the next step.
    pub fn grab(&self, app: &App, body: &Value) -> Result<Grab> {
        let source = self.source_of(body)?;
        let given = text(body, "protocol").map(|p| p.to_lowercase());
        let (indexer_id, link, protocol) = match &source {
            Source::Indexer(id, link) => {
                let ix = self.indexers.get(*id)?;
                (*id, link.clone(), protocol_name(ix.protocol()).to_string())
            }
            Source::Magnet(m) => (0, m.clone(), "torrent".to_string()),
            Source::Url(u) => {
                let guess = if u.to_lowercase().contains(".nzb") { "usenet" } else { "torrent" };
                (0, u.clone(), given.filter(|g| g == "usenet" || g == "torrent").unwrap_or_else(|| guess.to_string()))
            }
        };
        let title = text(body, "title").filter(|t| !t.is_empty()).unwrap_or_else(|| title_of(&link));
        let now = crate::auth::now();
        let id = self.db.execute(
            "INSERT INTO hub_grabs (app_id, title, protocol, indexer_id, link, category, state, added, updated) VALUES (?,?,?,?,?,?,'queued',?,?)",
            p![app.id, title, protocol, indexer_id, link, app.category, now, now],
        )?;
        self.seen(app.id);
        if let Some(h) = super::torrent::magnet_hash(&link) {
            self.set(id, &[("infohash", json!(h))])?;
        }
        let g = self.get_grab(id)?;
        self.emit("grab.queued", app, self.grab_json(&g, app));
        Ok(g)
    }

    /// Queues a grab for `app` of `content` an app handed over itself (an emulated client's upload or magnet),
    /// in the category `ext_category` the app asked for. A file is kept in the data folder until the grab is
    /// removed, so it can be sent again.
    pub fn grab_content(&self, app: &App, content: Content, title: &str, ext_category: &str) -> Result<Grab> {
        let (protocol, infohash) = match &content {
            Content::Magnet(m) => ("torrent", super::torrent::magnet_hash(m).unwrap_or_default()),
            Content::Torrent(b) => ("torrent", super::torrent::info_hash(b).unwrap_or_default()),
            Content::Nzb(_) => ("usenet", String::new()),
        };
        let title = if title.trim().is_empty() {
            match &content {
                Content::Magnet(m) => title_of(m),
                _ => "Release".to_string(),
            }
        } else {
            title.trim().to_string()
        };
        let now = crate::auth::now();
        let id = self.db.execute(
            "INSERT INTO hub_grabs (app_id, title, protocol, category, state, infohash, ext_category, added, updated) VALUES (?,?,?,?,'queued',?,?,?,?)",
            p![app.id, title, protocol, app.category, infohash, ext_category, now, now],
        )?;
        let link = match &content {
            Content::Magnet(m) => m.clone(),
            Content::Torrent(b) | Content::Nzb(b) => {
                let name = format!("{id}.{}", if protocol == "usenet" { "nzb" } else { "torrent" });
                let dir = self.data_dir.join("grabs");
                std::fs::create_dir_all(&dir)
                    .and_then(|_| std::fs::write(dir.join(&name), b))
                    .map_err(|e| err(507, format!("Lantern couldn't keep the release file in its data folder: {e}")))?;
                format!("{FILE_LINK}{name}")
            }
        };
        self.set(id, &[("link", json!(link))])?;
        self.seen(app.id);
        let g = self.get_grab(id)?;
        self.emit("grab.queued", app, self.grab_json(&g, app));
        Ok(g)
    }

    /// Grab `id`; 404 when there is none.
    pub fn get_grab(&self, id: i64) -> Result<Grab> {
        self.db
            .one("SELECT * FROM hub_grabs WHERE id=?", p![id])?
            .map(|r| Grab::from_row(&r))
            .ok_or_else(|| err(404, "There is no grab with that id"))
    }

    /// The newest grabs, of app `app` and in `state` when given.
    pub fn grabs(&self, app: Option<i64>, state: Option<&str>, limit: i64) -> Result<Vec<Grab>> {
        let rows = self.db.query(
            "SELECT * FROM hub_grabs WHERE (? = 0 OR app_id = ?) AND (? = '' OR state = ?) ORDER BY id DESC LIMIT ?",
            p![app.unwrap_or(0), app.unwrap_or(0), state.unwrap_or(""), state.unwrap_or(""), limit.clamp(1, 1000)],
        )?;
        Ok(rows.iter().map(Grab::from_row).collect())
    }

    /// A grab as its app sees it: paths in the app's own view. Which client holds it is not said.
    pub fn grab_json(&self, g: &Grab, app: &App) -> Value {
        let folder = self.folder(app);
        let view = |p: &str| if p.is_empty() { String::new() } else { paths::to_app(p, &folder, &app.import_path) };
        json!({
            "id": g.id, "app_id": g.app_id, "app": app.name, "title": g.title, "protocol": g.protocol,
            "indexer_id": g.indexer_id, "category": g.category, "state": g.state, "progress": g.progress,
            "size": g.size, "speed": g.speed, "eta": g.eta, "path": view(&g.path),
            "files": g.files.iter().map(|(p, s)| json!({ "path": view(p), "size": s })).collect::<Vec<_>>(),
            "error": g.error, "attempts": g.attempts, "next_try": g.next_try, "added": g.added,
            "updated": g.updated, "finished": g.finished, "ratio": g.ratio, "seed_secs": g.seed_secs,
            "imported": g.imported, "infohash": g.infohash, "seeded": matches!(g.state.as_str(), "seeding" | "completed") && self.seeded(g),
        })
    }

    /// Sends failed grab `id` again from the start.
    pub fn retry(&self, id: i64) -> Result<Grab> {
        let g = self.get_grab(id)?;
        if g.state != "failed" {
            return Err(err(409, "Only a failed grab can be tried again"));
        }
        self.db.execute(
            "UPDATE hub_grabs SET state='queued', attempts=0, next_try=0, error='', client_id=0, client_ref='', finished=0, updated=? WHERE id=?",
            p![crate::auth::now(), id],
        )?;
        let g = self.get_grab(id)?;
        if let Ok(app) = self.app(g.app_id) {
            self.emit("grab.queued", &app, self.grab_json(&g, &app));
        }
        Ok(g)
    }

    /// Stops grab `id`: removes it from its client (and its files when `delete_files`) and marks it removed.
    pub async fn remove_grab(&self, id: i64, delete_files: bool) -> Result<Grab> {
        let g = self.get_grab(id)?;
        if g.client_id != 0 && IN_CLIENT.contains(&g.state.as_str()) {
            if let Ok(cfg) = self.client(g.client_id) {
                let backend = self.backend(&cfg)?;
                let item_id = if g.client_ref.is_empty() { self.find_ref(&cfg, &g).await } else { Some(g.client_ref.clone()) };
                if let Some(item_id) = item_id {
                    backend.remove(&item_id, delete_files).await.map_err(|e| err(502, e))?;
                }
            }
        }
        self.set(id, &[("state", json!("removed")), ("speed", json!(0)), ("eta", json!(0))])?;
        if let Some(name) = g.link.strip_prefix(FILE_LINK) {
            let _ = std::fs::remove_file(self.data_dir.join("grabs").join(name));
        }
        let g = self.get_grab(id)?;
        if let Ok(app) = self.app(g.app_id) {
            self.emit("grab.removed", &app, self.grab_json(&g, &app));
        }
        Ok(g)
    }

    /// The client's id of grab `g` found by its tag, when the client lists it.
    async fn find_ref(&self, cfg: &ClientConfig, g: &Grab) -> Option<String> {
        let items = self.backend(cfg).ok()?.items().await.ok()?;
        items.into_iter().find(|i| i.tag == g.tag()).map(|i| i.id)
    }

    /// Sets `fields` of grab `id` and its `updated` time.
    fn set(&self, id: i64, fields: &[(&str, Value)]) -> Result<()> {
        let mut sql = String::from("UPDATE hub_grabs SET updated=?");
        let mut params = p![crate::auth::now()];
        for (k, v) in fields {
            sql.push_str(&format!(", {k}=?"));
            params.push(v.clone());
        }
        sql.push_str(" WHERE id=?");
        params.push(json!(id));
        self.db.execute(&sql, params)?;
        Ok(())
    }

    /// One step: sends the grabs whose turn has come, then follows every download in the clients. Two steps
    /// never run at once.
    pub async fn tick(&self) {
        let _guard = self.ticking.lock().await;
        if let Err(e) = self.send_due().await {
            tracing::warn!("download hub: {}", e.detail);
        }
        if let Err(e) = self.follow().await {
            tracing::warn!("download hub: {}", e.detail);
        }
        if let Err(e) = self.clean_up().await {
            tracing::warn!("download hub: {}", e.detail);
        }
    }

    /// Sends every queued grab whose turn has come.
    async fn send_due(&self) -> Result<()> {
        let due = self.db.query("SELECT * FROM hub_grabs WHERE state='queued' AND next_try<=? ORDER BY id", p![crate::auth::now()])?;
        for g in due.iter().map(Grab::from_row) {
            let Ok(app) = self.app(g.app_id) else { continue };
            match self.send(&g, &app).await {
                Ok(()) => {
                    let g = self.get_grab(g.id)?;
                    self.emit("grab.sent", &app, self.grab_json(&g, &app));
                }
                Err(why) => self.failed(&g, &app, &why)?,
            }
        }
        Ok(())
    }

    /// Fetches grab `g`'s release and hands it to the right client, in `app`'s category and folder.
    async fn send(&self, g: &Grab, app: &App) -> std::result::Result<(), String> {
        let content = self.fetch(g).await?;
        let protocol = content.protocol();
        if g.infohash.is_empty() {
            let h = match &content {
                Content::Magnet(m) => super::torrent::magnet_hash(m),
                Content::Torrent(b) => super::torrent::info_hash(b),
                Content::Nzb(_) => None,
            };
            if let Some(h) = h {
                self.set(g.id, &[("infohash", json!(h))]).map_err(|e| e.detail)?;
            }
        }
        let cfg = self.client_for(protocol).map_err(|e| e.detail)?.ok_or_else(|| {
            format!(
                "There is no download client for {} yet. Add one under Download clients.",
                if protocol == Protocol::Usenet { "Usenet" } else { "torrents" }
            )
        })?;
        let backend = self.backend(&cfg).map_err(|e| e.detail)?;
        let folder = self.folder(app);
        let _ = std::fs::create_dir_all(&folder);
        let save_path = paths::to_client(&folder, &self.root(), &cfg.root);
        backend.ensure_category(&app.category, &save_path).await?;
        let req = AddRequest { content, title: g.title.clone(), category: app.category.clone(), save_path, tag: g.tag() };
        let item_id = backend.add(&req).await?;
        self.set(
            g.id,
            &[
                ("state", json!("waiting")),
                ("protocol", json!(protocol_name(protocol))),
                ("client_id", json!(cfg.id)),
                ("client_ref", json!(item_id)),
                ("category", json!(app.category)),
                ("sent", json!(crate::auth::now())),
                ("error", json!("")),
                ("progress", json!(0.0)),
            ],
        )
        .map_err(|e| e.detail)
    }

    /// The release of grab `g`: from its indexer, as a magnet, or fetched from the address the app gave.
    async fn fetch(&self, g: &Grab) -> std::result::Result<Content, String> {
        if g.link.starts_with("magnet:") {
            return Ok(Content::Magnet(g.link.clone()));
        }
        if let Some(name) = g.link.strip_prefix(FILE_LINK) {
            let bytes = std::fs::read(self.data_dir.join("grabs").join(name))
                .map_err(|_| "The release file the app sent is no longer in Lantern's data folder.".to_string())?;
            return Ok(if g.protocol == "usenet" { Content::Nzb(bytes) } else { Content::Torrent(bytes) });
        }
        let bytes = if g.indexer_id != 0 {
            match self.indexers.grab(g.indexer_id, &g.link, &g.title, "Lantern").await.map_err(|e| e.detail)? {
                Grabbed::Magnet(m) => return Ok(Content::Magnet(m)),
                Grabbed::File(b) => b,
            }
        } else {
            let mut url = g.link.clone();
            let mut got = None;
            for _ in 0..5 {
                let r = self
                    .http_no_redirect()
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| format!("Lantern couldn't download the release from {url}: {e}"))?;
                if r.status().is_redirection() {
                    let next = r.headers().get("location").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                    if next.starts_with("magnet:") {
                        return Ok(Content::Magnet(next));
                    }
                    url = url::Url::parse(&url)
                        .and_then(|b| b.join(&next))
                        .map(|u| u.to_string())
                        .map_err(|_| format!("{url} sent Lantern to an address it can't follow"))?;
                    continue;
                }
                if !r.status().is_success() {
                    return Err(format!("The release's address answered {} instead of the file.", r.status().as_u16()));
                }
                got = Some(r.bytes().await.map_err(|e| format!("The release's download stopped halfway: {e}"))?.to_vec());
                break;
            }
            got.ok_or_else(|| "The release's address sent Lantern on too many times.".to_string())?
        };
        if looks_like_nzb(&bytes) {
            Ok(Content::Nzb(bytes))
        } else if bytes.first() == Some(&b'd') {
            Ok(Content::Torrent(bytes))
        } else {
            Err("What the indexer sent is neither a .torrent nor an .nzb file. The release may be gone.".into())
        }
    }

    /// The release at `url` (a `.torrent` or `.nzb` address an app gave, or a redirect to a magnet).
    pub async fn fetch_url(&self, url: &str) -> std::result::Result<Content, String> {
        let probe = Grab { link: url.to_string(), ..self.blank() };
        self.fetch(&probe).await
    }

    /// Sets the category grab `id` shows in an emulated client.
    pub fn set_ext_category_of(&self, id: i64, category: &str) -> Result<()> {
        self.set(id, &[("ext_category", json!(category))])
    }

    /// An empty grab, for fetching an address before there is a grab.
    fn blank(&self) -> Grab {
        Grab::from_row(&Row::new())
    }

    /// An HTTP client that doesn't follow redirects (a redirect may lead to a magnet link).
    fn http_no_redirect(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(60))
            .user_agent(format!("Lantern/{}", crate::VERSION))
            .build()
            .unwrap_or_else(|_| self.http.clone())
    }

    /// Grab `g` failed with `why`: queued again after a wait, or failed for good after the last attempt.
    fn failed(&self, g: &Grab, app: &App, why: &str) -> Result<()> {
        let attempts = g.attempts + 1;
        let now = crate::auth::now();
        if attempts >= self.max_attempts() {
            self.set(
                g.id,
                &[
                    ("state", json!("failed")),
                    ("attempts", json!(attempts)),
                    ("error", json!(why)),
                    ("finished", json!(now)),
                    ("speed", json!(0)),
                    ("eta", json!(0)),
                ],
            )?;
            let g = self.get_grab(g.id)?;
            self.emit("grab.failed", app, self.grab_json(&g, app));
        } else {
            let wait = BACKOFF[((attempts - 1) as usize).min(BACKOFF.len() - 1)];
            self.set(
                g.id,
                &[
                    ("state", json!("queued")),
                    ("attempts", json!(attempts)),
                    ("error", json!(why)),
                    ("next_try", json!(now + wait)),
                    ("client_id", json!(0)),
                    ("client_ref", json!("")),
                    ("speed", json!(0)),
                    ("eta", json!(0)),
                ],
            )?;
            let g = self.get_grab(g.id)?;
            self.emit("grab.retrying", app, self.grab_json(&g, app));
        }
        Ok(())
    }

    /// Asks every client where Lantern's downloads are, and moves each grab on.
    async fn follow(&self) -> Result<()> {
        let rows = self.db.query(
            "SELECT * FROM hub_grabs WHERE state IN ('waiting','downloading','paused','seeding') AND client_id<>0 ORDER BY id",
            p![],
        )?;
        let mut by_client: HashMap<i64, Vec<Grab>> = HashMap::new();
        for g in rows.iter().map(Grab::from_row) {
            by_client.entry(g.client_id).or_default().push(g);
        }
        for (client_id, grabs) in by_client {
            let Ok(cfg) = self.client(client_id) else {
                for g in grabs {
                    if let Ok(app) = self.app(g.app_id) {
                        self.failed(&g, &app, "Its download client was removed from Lantern.")?;
                    }
                }
                continue;
            };
            let backend = self.backend(&cfg)?;
            let items = match backend.items().await {
                Ok(i) => i,
                Err(e) => {
                    tracing::warn!("download hub: {e}");
                    continue;
                }
            };
            for g in grabs {
                let Ok(app) = self.app(g.app_id) else { continue };
                let item = items.iter().find(|i| if g.client_ref.is_empty() { i.tag == g.tag() } else { i.id == g.client_ref });
                match item {
                    Some(item) => self.update_from(&g, &app, &cfg, item).await?,
                    None if g.state == "seeding" => self.set(g.id, &[("state", json!("completed")), ("speed", json!(0))])?,
                    None if crate::auth::now() - g.sent > GRACE => {
                        self.failed(&g, &app, &format!("The download disappeared from {}.", cfg.name))?;
                    }
                    None => {}
                }
            }
        }
        Ok(())
    }

    /// Moves grab `g` on from what its client says about it.
    async fn update_from(&self, g: &Grab, app: &App, cfg: &ClientConfig, item: &Item) -> Result<()> {
        let backend = self.backend(cfg)?;
        if item.state == ItemState::Failed {
            let _ = backend.remove(&item.id, true).await;
            return self.failed(g, app, &item.error);
        }
        let state = match item.state {
            ItemState::Queued => "waiting",
            ItemState::Downloading => "downloading",
            ItemState::Paused => "paused",
            ItemState::Seeding => "seeding",
            ItemState::Completed | ItemState::Failed => "completed",
        };
        let was_done = matches!(g.state.as_str(), "seeding" | "completed");
        let mut fields = vec![
            ("client_ref", json!(item.id)),
            ("state", json!(state)),
            ("progress", json!(if item.state.done() { 1.0 } else { item.progress })),
            ("size", json!(if item.size > 0 { item.size } else { g.size })),
            ("speed", json!(if item.state.done() { 0 } else { item.speed })),
            ("eta", json!(if item.state.done() { 0 } else { item.eta })),
            ("ratio", json!(item.ratio)),
            ("seed_secs", json!(item.seed_secs)),
        ];
        if item.state.done() && !was_done {
            let root = self.root();
            let mut files: Vec<(String, i64)> = backend
                .files(item)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|(p, s)| (paths::from_client(&p, &root, &cfg.root), s))
                .collect();
            let path = paths::from_client(&item.path, &root, &cfg.root);
            if files.is_empty() && !item.path.is_empty() {
                files = list_files(std::path::Path::new(&path));
            }
            fields.push(("path", json!(path)));
            fields.push(("files", json!(files)));
            fields.push(("finished", json!(crate::auth::now())));
            fields.push(("error", json!("")));
            self.set(g.id, &fields)?;
            let g = self.get_grab(g.id)?;
            self.emit("grab.completed", app, self.grab_json(&g, app));
            return Ok(());
        }
        self.set(g.id, &fields)?;
        let changed = g.state != state || (item.progress - g.progress).abs() >= 0.01;
        if changed && !was_done {
            let g = self.get_grab(g.id)?;
            self.emit("grab.progress", app, self.grab_json(&g, app));
        }
        Ok(())
    }
}

/// The files under `path` (or `path` itself when it is a file) with their sizes, as Lantern sees them; empty when
/// Lantern can't read it.
pub fn list_files(path: &std::path::Path) -> Vec<(String, i64)> {
    let mut out = Vec::new();
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(meta) = std::fs::metadata(&p) else { continue };
        if meta.is_file() {
            out.push((p.to_string_lossy().replace('\\', "/"), meta.len() as i64));
        } else if meta.is_dir() {
            if let Ok(rd) = std::fs::read_dir(&p) {
                stack.extend(rd.flatten().map(|e| e.path()));
            }
        }
        if out.len() > 10_000 {
            break;
        }
    }
    out.sort();
    out
}

/// `torrent` or `usenet`.
pub fn protocol_name(p: Protocol) -> &'static str {
    match p {
        Protocol::Usenet => "usenet",
        _ => "torrent",
    }
}

/// A name for a grab from its link when the app gave none: the magnet's `dn`, else the address's last part.
fn title_of(link: &str) -> String {
    if let Some(q) = link.strip_prefix("magnet:?") {
        if let Some(dn) = url::form_urlencoded::parse(q.as_bytes()).find(|(k, _)| k == "dn") {
            return dn.1.to_string();
        }
    }
    url::Url::parse(link)
        .ok()
        .and_then(|u| u.path_segments().and_then(|mut s| s.next_back().map(str::to_string)))
        .map(|s| {
            let lower = s.to_lowercase();
            if lower.ends_with(".nzb") || lower.ends_with(".torrent") {
                s.rsplit_once('.').map(|(a, _)| a.to_string()).unwrap_or(s)
            } else {
                s
            }
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Release".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_grabs_from_their_link() {
        assert_eq!(title_of("magnet:?xt=urn:btih:abc&dn=Some+Show+S01"), "Some Show S01");
        assert_eq!(title_of("https://x.test/get/Thing.nzb?k=1"), "Thing");
        assert_eq!(title_of("https://x.test/get/12345"), "12345");
        assert_eq!(title_of("https://x.test/"), "Release");
    }

    #[test]
    fn lists_files_in_a_folder() {
        let dir = std::env::temp_dir().join(format!("lantern-list-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.mkv"), b"12").unwrap();
        std::fs::write(dir.join("sub/b.nfo"), b"1").unwrap();
        let files = list_files(&dir);
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|(p, s)| p.ends_with("/a.mkv") && *s == 2));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
