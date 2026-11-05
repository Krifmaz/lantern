//! Apps that download through Lantern. An app registers with its name, media type and import path; Lantern gives
//! it a category (its name as a slug) and the folder `<downloads folder>/<category>`, made at once. Registering
//! again with the same name updates the app, so an app can register at every start.

use super::{paths, text, Hub};
use kit_server_core::{err, p, Db, Result, Row, RowExt};
use serde_json::{json, Value};

/// The media types an app can say it handles.
pub const MEDIA: [&str; 5] = ["tv", "movie", "music", "book", "other"];

/// A registered app.
#[derive(Debug, Clone)]
pub struct App {
    /// Its id.
    pub id: i64,
    /// Its name (unique, any case).
    pub name: String,
    /// `tv`, `movie`, `music`, `book` or `other`.
    pub media: String,
    /// Its category in every download client, and its folder's name.
    pub category: String,
    /// Where the app sees its folder (empty: at the same path as Lantern).
    pub import_path: String,
    /// Where Lantern posts its events (empty: none).
    pub callback_url: String,
    /// How it connects: `native` (Lantern's API), or the client it thinks it talks to.
    pub source: String,
    /// The last webhook failure, in words (empty: the last one went through).
    pub callback_error: String,
    /// When it registered.
    pub added: f64,
    /// When it last asked Lantern something.
    pub last_seen: f64,
}

impl App {
    /// The app from its database row.
    pub fn from_row(r: &Row) -> App {
        App {
            id: r.i("id"),
            name: r.s("name"),
            media: r.s("media"),
            category: r.s("category"),
            import_path: r.s("import_path"),
            callback_url: r.s("callback_url"),
            source: r.s("source"),
            callback_error: r.s("callback_error"),
            added: r.f("added"),
            last_seen: r.f("last_seen"),
        }
    }

    /// The standard categories its searches use when they name none.
    pub fn categories(&self) -> Vec<u32> {
        match self.media.as_str() {
            "tv" => vec![5000],
            "movie" => vec![2000],
            "music" => vec![3000],
            "book" => vec![7000],
            _ => Vec::new(),
        }
    }
}

impl Hub {
    /// Every app, by name.
    pub fn apps(&self) -> Result<Vec<App>> {
        Ok(self.db.query("SELECT * FROM hub_apps ORDER BY name COLLATE NOCASE", p![])?.iter().map(App::from_row).collect())
    }

    /// App `id`; 404 when there is none.
    pub fn app(&self, id: i64) -> Result<App> {
        self.db
            .one("SELECT * FROM hub_apps WHERE id=?", p![id])?
            .map(|r| App::from_row(&r))
            .ok_or_else(|| err(404, "There is no app with that id"))
    }

    /// The app a request names, by id or by name; 404 when there is none.
    pub fn app_named(&self, which: &str) -> Result<App> {
        let which = which.trim();
        if which.is_empty() {
            return Err(err(400, "Say which app is asking (its id or name, as `app`)"));
        }
        if let Ok(id) = which.parse::<i64>() {
            return self.app(id);
        }
        self.db
            .one("SELECT * FROM hub_apps WHERE name=?", p![which])?
            .map(|r| App::from_row(&r))
            .ok_or_else(|| err(404, format!("There is no app named {which}. Register it first.")))
    }

    /// Lantern's folder for `app` (`<downloads folder>/<category>`).
    pub fn folder(&self, app: &App) -> String {
        format!("{}/{}", self.root(), app.category)
    }

    /// Registers an app from `body` (`name`, `media`, `import_path`, `callback_url`, `source`), or updates the one
    /// with that name; the app and, when its folder couldn't be made, why.
    pub fn register(&self, body: &Value) -> Result<(App, Option<String>)> {
        let name = text(body, "name").unwrap_or_default();
        if name.is_empty() {
            return Err(err(400, "Enter the app's name"));
        }
        let media = text(body, "media").or_else(|| text(body, "media_type")).map(|m| m.to_lowercase());
        if let Some(m) = &media {
            if !MEDIA.contains(&m.as_str()) {
                return Err(err(400, "The media type must be tv, movie, music, book or other"));
            }
        }
        let callback = text(body, "callback_url");
        if let Some(c) = callback.as_deref().filter(|c| !c.is_empty()) {
            if !url::Url::parse(c).is_ok_and(|u| matches!(u.scheme(), "http" | "https")) {
                return Err(err(400, "The callback address must start with http:// or https://"));
            }
        }
        let import = text(body, "import_path").map(|p| paths::clean(&p));
        let now = crate::auth::now();
        let id = match self.db.one("SELECT * FROM hub_apps WHERE name=?", p![name])? {
            Some(r) => {
                let cur = App::from_row(&r);
                self.db.execute(
                    "UPDATE hub_apps SET media=?, import_path=?, callback_url=?, source=?, last_seen=? WHERE id=?",
                    p![
                        media.unwrap_or(cur.media),
                        import.unwrap_or(cur.import_path),
                        callback.unwrap_or(cur.callback_url),
                        text(body, "source").filter(|s| !s.is_empty()).unwrap_or(cur.source),
                        now,
                        cur.id
                    ],
                )?;
                cur.id
            }
            None => {
                let base = paths::slug(&name);
                let mut category = base.clone();
                let mut n = 2;
                while self.db.one("SELECT id FROM hub_apps WHERE category=?", p![category])?.is_some() {
                    category = format!("{base}-{n}");
                    n += 1;
                }
                self.db.execute(
                    "INSERT INTO hub_apps (name, media, category, import_path, callback_url, source, added, last_seen) VALUES (?,?,?,?,?,?,?,?)",
                    p![
                        name,
                        media.unwrap_or_else(|| "other".into()),
                        category,
                        import.unwrap_or_default(),
                        callback.unwrap_or_default(),
                        text(body, "source").filter(|s| !s.is_empty()).unwrap_or_else(|| "native".into()),
                        now,
                        now
                    ],
                )?
            }
        };
        let app = self.app(id)?;
        let folder = self.folder(&app);
        let problem = std::fs::create_dir_all(&folder)
            .err()
            .map(|e| format!("Lantern couldn't make the folder {folder}: {e}. Check that the downloads folder is mounted and writable."));
        Ok((app, problem))
    }

    /// Changes app `id`: `media`, `import_path`, `callback_url` (the name and category stay).
    pub fn update_app(&self, id: i64, body: &Value) -> Result<App> {
        let cur = self.app(id)?;
        let mut b = body.clone();
        b["name"] = json!(cur.name);
        self.register(&b)?;
        self.app(id)
    }

    /// Removes app `id` and its grabs' records (the downloads stay in the clients).
    pub fn remove_app(&self, id: i64) -> Result<()> {
        self.app(id)?;
        self.db.execute("DELETE FROM hub_apps WHERE id=?", p![id])?;
        self.db.execute("DELETE FROM hub_grabs WHERE app_id=?", p![id])?;
        Ok(())
    }

    /// Notes that app `id` asked something now.
    pub fn seen(&self, id: i64) {
        let _ = self.db.execute("UPDATE hub_apps SET last_seen=? WHERE id=?", p![crate::auth::now(), id]);
    }

    /// An app as the API shows it, with its folder in Lantern's view and its own, and its grab counts.
    pub fn app_json(&self, a: &App) -> Value {
        let counts =
            self.db.query("SELECT state, COUNT(*) AS n FROM hub_grabs WHERE app_id=? GROUP BY state", p![a.id]).unwrap_or_default();
        let mut by_state = serde_json::Map::new();
        for r in counts {
            by_state.insert(r.s("state"), json!(r.i("n")));
        }
        let folder = self.folder(a);
        json!({
            "id": a.id, "name": a.name, "media": a.media, "category": a.category, "folder": folder,
            "import_path": if a.import_path.is_empty() { folder.clone() } else { a.import_path.clone() },
            "callback_url": a.callback_url, "callback_error": a.callback_error, "source": a.source,
            "added": a.added, "last_seen": a.last_seen, "grabs": by_state,
        })
    }
}
