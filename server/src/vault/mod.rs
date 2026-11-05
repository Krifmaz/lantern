//! The account vault: tracker and Usenet logins, passwords, cookies, passkeys, API keys, 2FA seeds and invite
//! notes, encrypted at rest.
//!
//! How it is locked:
//!
//! - One random 32-byte **data key** seals every item with XChaCha20-Poly1305 ([`crypto`]), bound to the item's id.
//! - The data key itself is sealed with a **key-encryption key** that argon2id makes from the vault secret: the
//!   `APP_VAULT_PASSPHRASE` environment variable when it is set, else the random key in `<data>/vault.key` (made at
//!   the first start, readable by the server's user only). The sealed data key and the argon2id salt live in
//!   `vault_meta`; the data key is only ever in memory.
//! - With the passphrase, a copy of the data folder alone opens nothing. With the key file, keep `vault.key` with
//!   your backups of the data folder, or the saved logins can't be opened again.
//!
//! What is shown: an item's name, kind, site and its plain fields (username, email, Usenet host) are listed;
//! its secrets (password, cookie, passkey, keys, 2FA seed, notes) only by name, never their values. Their values
//! leave the server only through [`Vault::reveal`] (the owner, after typing their password again) and inside an
//! encrypted backup ([`Vault::export`]). Lantern's own parts read them with [`Vault::secrets`]. Values are kept in
//! [`Secret`], which never prints itself, so they can't end up in a log.

pub mod api;
pub mod crypto;
pub mod totp;

use crypto::{b64, unb64, KdfParams, Key};
use kit_server_core::{err, p, Db, Result, Row, RowExt, SqliteDb};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use zeroize::Zeroizing;

/// A secret value: wiped from memory when dropped, and shown as `••••••` by `{:?}`, so it never reaches a log.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    /// A secret holding `s`.
    pub fn new(s: String) -> Secret {
        Secret(Zeroizing::new(s))
    }

    /// The value itself, for the one place that needs it (signing in to a site).
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("••••••")
    }
}

/// The kinds of account the vault keeps: (id, words).
pub const KINDS: &[(&str, &str)] = &[
    ("tracker", "Tracker account"),
    ("usenet_provider", "Usenet provider"),
    ("usenet_indexer", "Usenet indexer"),
    ("other", "Other account"),
];

/// One field an item can have.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct FieldDef {
    /// the field's name in the API
    pub key: &'static str,
    /// the field's words in the web app
    pub label: &'static str,
    /// whether its value is a secret: never listed, only revealed
    pub secret: bool,
    /// the kinds of item that offer it
    pub kinds: &'static [&'static str],
    /// a line of help under the field, "" for none
    pub help: &'static str,
}

/// Every kind.
const ALL: &[&str] = &["tracker", "usenet_provider", "usenet_indexer", "other"];

/// The fields of the vault's items, in the order the web app shows them.
pub const FIELDS: &[FieldDef] = &[
    FieldDef { key: "username", label: "Username", secret: false, kinds: ALL, help: "" },
    FieldDef { key: "email", label: "Email", secret: false, kinds: ALL, help: "" },
    FieldDef { key: "host", label: "Server", secret: false, kinds: &["usenet_provider"], help: "news.example.com" },
    FieldDef { key: "port", label: "Port", secret: false, kinds: &["usenet_provider"], help: "563 for SSL, 119 without" },
    FieldDef { key: "connections", label: "Connections", secret: false, kinds: &["usenet_provider"], help: "" },
    FieldDef { key: "password", label: "Password", secret: true, kinds: ALL, help: "" },
    FieldDef {
        key: "cookie",
        label: "Cookie",
        secret: true,
        kinds: &["tracker", "usenet_indexer"],
        help: "The Cookie header of a signed-in browser, for sites with a captcha.",
    },
    FieldDef {
        key: "passkey",
        label: "Passkey",
        secret: true,
        kinds: &["tracker"],
        help: "The personal key in your torrent and announce links.",
    },
    FieldDef { key: "api_key", label: "API key", secret: true, kinds: &["tracker", "usenet_indexer", "other"], help: "" },
    FieldDef { key: "rss_key", label: "RSS key", secret: true, kinds: &["tracker"], help: "" },
    FieldDef {
        key: "totp_seed",
        label: "2FA seed",
        secret: true,
        kinds: &["tracker", "usenet_indexer", "other"],
        help: "The text under the QR code when you set up two-factor sign-in, or its otpauth:// link.",
    },
    FieldDef { key: "invites", label: "Invite notes", secret: true, kinds: ALL, help: "Invites you have and who you invited." },
    FieldDef { key: "notes", label: "Notes", secret: true, kinds: ALL, help: "" },
];

/// The longest value a field takes (a site's cookies can be long), in bytes.
pub const MAX_VALUE: usize = 64 * 1024;

/// What a saved secret is listed as in forms that send it back: a value equal to it keeps the saved one.
pub const MASK: &str = "••••••";

/// The field `key`, `None` when there is none of that name.
pub fn field(key: &str) -> Option<&'static FieldDef> {
    FIELDS.iter().find(|f| f.key == key)
}

/// Where the vault's secret comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeySource {
    /// the `APP_VAULT_PASSPHRASE` environment variable
    Passphrase,
    /// `<data>/vault.key`
    File,
}

/// The vault; clones share it.
#[derive(Clone)]
pub struct Vault {
    /// the shared state
    inner: Arc<Inner>,
}

/// What the clones of a [`Vault`] share.
struct Inner {
    /// the database holding `vault_meta` and `vault_items`
    db: SqliteDb,
    /// the data key, or why the vault can't be opened, in words
    key: std::result::Result<Key, String>,
    /// where the vault's secret came from
    source: KeySource,
}

/// The associated text the data key is sealed with.
const KEY_AAD: &[u8] = b"lantern-vault-data-key";

/// The associated text an item is sealed with: its id, so a sealed value can't be moved to another item.
fn item_aad(id: &str) -> Vec<u8> {
    format!("lantern-vault-item:{id}").into_bytes()
}

/// The vault's tables: `vault_meta` (the argon2id settings and the sealed data key) and `vault_items`.
pub fn migrate(db: &SqliteDb) -> anyhow::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS vault_meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS vault_items (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            name TEXT NOT NULL,
            site TEXT NOT NULL DEFAULT '',
            plain TEXT NOT NULL DEFAULT '{}',
            secrets TEXT NOT NULL DEFAULT '',
            data TEXT NOT NULL,
            created_at REAL,
            updated_at REAL,
            revealed_at REAL
        );",
    )
    .map_err(|e| anyhow::anyhow!("The vault's tables can't be made: {}", e.detail))
}

/// The value kept under `k` in `vault_meta`.
fn meta(db: &SqliteDb, k: &str) -> Result<Option<String>> {
    Ok(db.one("SELECT v FROM vault_meta WHERE k=?", p![k])?.map(|r| r.s("v")))
}

/// The vault secret: the passphrase when one is given, else the key file (made when the vault is new).
fn vault_secret(data: &Path, passphrase: Option<String>, new_vault: bool) -> std::result::Result<(Zeroizing<Vec<u8>>, KeySource), String> {
    if let Some(p) = passphrase.filter(|p| !p.is_empty()) {
        return Ok((Zeroizing::new(p.into_bytes()), KeySource::Passphrase));
    }
    let file = data.join("vault.key");
    match std::fs::read_to_string(&file) {
        Ok(s) => unb64(&s).map(|b| (Zeroizing::new(b), KeySource::File)).ok_or_else(|| {
            format!("The vault key file {} is damaged. Put back the copy from your backup to open the saved logins.", file.display())
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && new_vault => {
            let key = crypto::random_key();
            write_private(&file, &b64(&key[..]))
                .map_err(|e| format!("The vault key file {} can't be written: {e}. Check the data folder.", file.display()))?;
            Ok((Zeroizing::new(key.to_vec()), KeySource::File))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(format!(
            "The vault key file {} is missing. Put back the copy from your backup (or set APP_VAULT_PASSPHRASE if you used one) to open the saved logins.",
            file.display()
        )),
        Err(e) => Err(format!("The vault key file {} can't be read: {e}.", file.display())),
    }
}

/// Writes `text` to `file`, readable and writable by the server's user only.
fn write_private(file: &Path, text: &str) -> std::io::Result<()> {
    std::fs::write(file, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// The plain parts of an item and its secret fields, as kept and as answered.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Item {
    /// the item's id (random, 16 hex digits), the same in every backup
    pub id: String,
    /// one of [`KINDS`]
    pub kind: String,
    /// the name people see ("BeyondHD", "Newshosting")
    pub name: String,
    /// the site it is for: an indexer definition's id or an address, "" for none
    #[serde(default)]
    pub site: String,
    /// every field's value, secrets among them
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    /// when it was added, seconds since 1970
    #[serde(default)]
    pub created_at: f64,
    /// when it last changed, seconds since 1970
    #[serde(default)]
    pub updated_at: f64,
}

/// The file an encrypted backup is: its argon2id settings, nonce and the sealed items.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Backup {
    /// always `lantern-vault-backup`
    pub format: String,
    /// the file's version, 1
    pub version: u32,
    /// when it was made, RFC 3339
    pub created: String,
    /// how many items it holds (also inside, sealed; this copy is for people reading the file)
    pub items: usize,
    /// how the backup's key is made from its passphrase
    pub kdf: KdfParams,
    /// the sealed items (nonce ‖ ciphertext ‖ tag), base64
    pub data: String,
}

/// The name of a backup file's format.
pub const BACKUP_FORMAT: &str = "lantern-vault-backup";

/// The associated text a backup is sealed with.
const BACKUP_AAD: &[u8] = b"lantern-vault-backup:1";

/// The shortest backup passphrase taken.
pub const MIN_PASSPHRASE: usize = 12;

/// What a restore did (or, in a dry run, would do).
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct Restored {
    /// items that are new here
    pub added: Vec<String>,
    /// items that the backup's copy replaces
    pub updated: Vec<String>,
    /// items that are the same here and in the backup
    pub unchanged: Vec<String>,
    /// items changed here after the backup was made, kept as they are
    pub kept_newer: Vec<String>,
}

impl Vault {
    /// The vault on `db`, its key file in `data`; `passphrase` is `APP_VAULT_PASSPHRASE`. A vault that can't be
    /// opened (a missing key file, a wrong passphrase) still starts: it lists its items and says why it is locked.
    pub fn open(db: SqliteDb, data: &Path, passphrase: Option<String>) -> anyhow::Result<Vault> {
        migrate(&db)?;
        let dberr = |e: kit_server_core::Error| anyhow::anyhow!("The vault can't be read: {}", e.detail);
        let wrapped = meta(&db, "data_key").map_err(dberr)?;
        let source = if passphrase.as_deref().is_some_and(|p| !p.is_empty()) { KeySource::Passphrase } else { KeySource::File };
        let key = match vault_secret(data, passphrase, wrapped.is_none()) {
            Err(why) => Err(why),
            Ok((secret, source)) => match wrapped {
                None => {
                    let params = KdfParams::fresh();
                    let kek = crypto::derive(&secret, &params).map_err(|e| anyhow::anyhow!(e))?;
                    let key = crypto::random_key();
                    let sealed = crypto::seal(&kek, &key[..], KEY_AAD);
                    let kdf = serde_json::to_string(&params)?;
                    let source = serde_json::to_value(source)?.as_str().unwrap_or("file").to_string();
                    for (k, v) in [("kdf", kdf), ("data_key", b64(&sealed)), ("source", source)] {
                        db.execute("INSERT OR REPLACE INTO vault_meta (k, v) VALUES (?,?)", p![k, v]).map_err(dberr)?;
                    }
                    Ok(key)
                }
                Some(wrapped) => unlock(&db, &secret, &wrapped, source),
            },
        };
        if let Err(why) = &key {
            tracing::warn!("{why}");
        }
        Ok(Vault { inner: Arc::new(Inner { db, key, source }) })
    }

    /// The data key, or a 503 that says why the vault is locked.
    fn key(&self) -> Result<&Key> {
        self.inner.key.as_ref().map_err(|why| err(503, why.clone()))
    }

    /// Whether the vault is open, where its secret comes from, why it is locked, and how many items it has.
    pub fn status(&self) -> Result<Value> {
        let n = self.inner.db.one("SELECT COUNT(*) AS n FROM vault_items", p![])?.map(|r| r.i("n")).unwrap_or(0);
        Ok(json!({
            "open": self.inner.key.is_ok(),
            "locked_because": self.inner.key.as_ref().err(),
            "key_source": self.inner.source,
            "items": n,
        }))
    }

    /// Every item as listed (secrets by name only), by kind then name.
    pub fn list(&self) -> Result<Vec<Value>> {
        let rows = self.inner.db.query("SELECT * FROM vault_items ORDER BY kind, name COLLATE NOCASE", p![])?;
        Ok(rows.iter().map(listed).collect())
    }

    /// Item `id` as listed; 404 when there is none.
    pub fn get(&self, id: &str) -> Result<Value> {
        Ok(listed(&self.row(id)?))
    }

    /// The row of item `id`; 404 when there is none.
    fn row(&self, id: &str) -> Result<Row> {
        self.inner.db.one("SELECT * FROM vault_items WHERE id=?", p![id])?.ok_or_else(|| err(404, "That account isn't in the vault."))
    }

    /// Item `id` with every value opened.
    fn item(&self, id: &str) -> Result<Item> {
        let r = self.row(id)?;
        let key = self.key()?;
        let damaged = || {
            err(500, format!("The saved values of {} can't be opened: they were changed outside Lantern or the vault key is not the one they were saved with.", r.s("name")))
        };
        let sealed = unb64(&r.s("data")).ok_or_else(damaged)?;
        let plain = crypto::open(key, &sealed, &item_aad(id)).ok_or_else(damaged)?;
        let fields: BTreeMap<String, String> = serde_json::from_slice(&plain).map_err(|_| damaged())?;
        Ok(Item {
            id: id.to_string(),
            kind: r.s("kind"),
            name: r.s("name"),
            site: r.s("site"),
            fields,
            created_at: r.f("created_at"),
            updated_at: r.f("updated_at"),
        })
    }

    /// Every value of item `id`, for Lantern's own use (signing in to a site); never send them anywhere else.
    pub fn secrets(&self, id: &str) -> Result<BTreeMap<String, Secret>> {
        Ok(self.item(id)?.fields.into_iter().map(|(k, v)| (k, Secret::new(v))).collect())
    }

    /// Saves `item` (made or replaced), sealing its fields.
    fn save(&self, item: &Item) -> Result<()> {
        let key = self.key()?;
        let mut plain = Map::new();
        let mut secrets = Vec::new();
        for (k, v) in &item.fields {
            match field(k) {
                Some(f) if f.secret => secrets.push(k.as_str()),
                _ => {
                    plain.insert(k.clone(), json!(v));
                }
            }
        }
        let fields = Zeroizing::new(serde_json::to_vec(&item.fields).map_err(|e| err(500, e.to_string()))?);
        let data = b64(&crypto::seal(key, &fields, &item_aad(&item.id)));
        self.inner.db.execute(
            "INSERT INTO vault_items (id, kind, name, site, plain, secrets, data, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?)
             ON CONFLICT(id) DO UPDATE SET kind=excluded.kind, name=excluded.name, site=excluded.site, plain=excluded.plain,
             secrets=excluded.secrets, data=excluded.data, updated_at=excluded.updated_at",
            p![
                item.id,
                item.kind,
                item.name,
                item.site,
                Value::Object(plain).to_string(),
                secrets.join(","),
                data,
                item.created_at,
                item.updated_at
            ],
        )?;
        Ok(())
    }

    /// Adds an item from `body` (`{kind, name, site, fields}`); the item as listed.
    pub fn create(&self, body: &Value) -> Result<Value> {
        self.key()?;
        let now = crate::auth::now();
        let mut item = Item { id: hex::encode(crypto::random::<8>()), created_at: now, updated_at: now, ..Item::default() };
        apply(&mut item, body)?;
        self.save(&item)?;
        tracing::info!("Vault: {} added", item.name);
        self.get(&item.id)
    }

    /// Changes item `id`: only what `body` has. A secret left empty, or sent back as [`MASK`], keeps its saved
    /// value; `null` removes a field.
    pub fn update(&self, id: &str, body: &Value) -> Result<Value> {
        let mut item = self.item(id)?;
        apply(&mut item, body)?;
        item.updated_at = crate::auth::now();
        self.save(&item)?;
        tracing::info!("Vault: {} changed", item.name);
        self.get(id)
    }

    /// Removes item `id`; 404 when there is none.
    pub fn remove(&self, id: &str) -> Result<()> {
        let r = self.row(id)?;
        self.inner.db.execute("DELETE FROM vault_items WHERE id=?", p![id])?;
        tracing::info!("Vault: {} removed", r.s("name"));
        Ok(())
    }

    /// Every field of item `id` with its value, and the 2FA code now when it has a seed; notes the time it was
    /// revealed. `by` names who asked, for the log.
    pub fn reveal(&self, id: &str, by: &str) -> Result<Value> {
        let item = self.item(id)?;
        let now = crate::auth::now();
        self.inner.db.execute("UPDATE vault_items SET revealed_at=? WHERE id=?", p![now, id])?;
        tracing::info!("Vault: {} revealed to {by}", item.name);
        let code = item.fields.get("totp_seed").and_then(|s| totp::parse(s)).map(|seed| {
            let (code, left) = totp::code(&seed, now as u64);
            json!({ "code": code, "seconds_left": left })
        });
        let mut out = listed(&self.row(id)?);
        out["fields"] = json!(item.fields);
        out["totp"] = json!(code);
        Ok(out)
    }

    /// An encrypted backup of every item, locked with `passphrase` (12 characters or more).
    pub fn export(&self, passphrase: &str) -> Result<Backup> {
        self.key()?;
        if passphrase.chars().count() < MIN_PASSPHRASE {
            return Err(err(400, format!("The backup passphrase must be at least {MIN_PASSPHRASE} characters.")));
        }
        let ids: Vec<String> =
            self.inner.db.query("SELECT id FROM vault_items ORDER BY created_at", p![])?.iter().map(|r| r.s("id")).collect();
        let items = ids.iter().map(|id| self.item(id)).collect::<Result<Vec<Item>>>()?;
        let plain = Zeroizing::new(serde_json::to_vec(&json!({ "items": items })).map_err(|e| err(500, e.to_string()))?);
        let kdf = KdfParams::fresh();
        let key = crypto::derive(passphrase.as_bytes(), &kdf).map_err(|e| err(500, e))?;
        tracing::info!("Vault: backup of {} items made", items.len());
        Ok(Backup {
            format: BACKUP_FORMAT.into(),
            version: 1,
            created: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            items: items.len(),
            kdf,
            data: b64(&crypto::seal(&key, &plain, BACKUP_AAD)),
        })
    }

    /// Puts back the items of `backup` (made by [`Vault::export`]) opened with `passphrase`: new items are added, an
    /// item that is here too is replaced by the backup's copy unless it changed here after the backup. Nothing is
    /// removed. With `dry_run`, nothing changes: the answer says what would.
    pub fn restore(&self, backup: &Value, passphrase: &str, dry_run: bool) -> Result<Restored> {
        self.key()?;
        let not_backup = || err(400, "This file isn't a Lantern vault backup. Choose the .json file that Back up made.");
        let b: Backup = serde_json::from_value(backup.clone()).map_err(|_| not_backup())?;
        if b.format != BACKUP_FORMAT {
            return Err(not_backup());
        }
        if b.version != 1 {
            return Err(err(400, "This backup was made by a newer Lantern. Update Lantern, then restore it."));
        }
        let key = crypto::derive(passphrase.as_bytes(), &b.kdf).map_err(|e| err(400, format!("This backup can't be read. {e}")))?;
        let sealed = unb64(&b.data).ok_or_else(not_backup)?;
        let plain = crypto::open(&key, &sealed, BACKUP_AAD).ok_or_else(|| {
            err(400, "The passphrase doesn't open this backup. Check it and try again; it is the one typed when the backup was made.")
        })?;
        #[derive(Deserialize)]
        struct Inside {
            items: Vec<Item>,
        }
        let inside: Inside = serde_json::from_slice(&plain).map_err(|_| not_backup())?;
        let mut done = Restored::default();
        for mut item in inside.items {
            if !KINDS.iter().any(|(k, _)| *k == item.kind) || item.id.is_empty() || item.id.len() > 64 {
                return Err(not_backup());
            }
            let here = self.inner.db.one("SELECT updated_at FROM vault_items WHERE id=?", p![item.id])?;
            let label = item.name.clone();
            match here {
                None => done.added.push(label),
                Some(r) => {
                    let ours = self.item(&item.id)?;
                    if ours.kind == item.kind && ours.name == item.name && ours.site == item.site && ours.fields == item.fields {
                        done.unchanged.push(label);
                        continue;
                    }
                    if r.f("updated_at") > item.updated_at {
                        done.kept_newer.push(label);
                        continue;
                    }
                    item.created_at = ours.created_at;
                    done.updated.push(label);
                }
            }
            if !dry_run {
                self.save(&item)?;
            }
        }
        if !dry_run {
            tracing::info!("Vault: backup restored ({} added, {} replaced)", done.added.len(), done.updated.len());
        }
        Ok(done)
    }
}

/// The data key sealed in `wrapped`, opened with the key argon2id makes from `secret`; why not, in words.
fn unlock(db: &SqliteDb, secret: &[u8], wrapped: &str, source: KeySource) -> std::result::Result<Key, String> {
    let damaged = "The vault's saved key is damaged. Restore the data folder from a backup, or restore a vault backup.".to_string();
    let params: KdfParams = meta(db, "kdf").ok().flatten().and_then(|k| serde_json::from_str(&k).ok()).ok_or_else(|| damaged.clone())?;
    let kek = crypto::derive(secret, &params)?;
    let sealed = unb64(wrapped).ok_or_else(|| damaged.clone())?;
    let made_with = meta(db, "source").ok().flatten().unwrap_or_default();
    let opened = crypto::open(&kek, &sealed, KEY_AAD).ok_or_else(|| match source {
        KeySource::Passphrase if made_with == "file" => {
            "The vault can't be opened: it was made with the vault.key file, not APP_VAULT_PASSPHRASE. Remove APP_VAULT_PASSPHRASE and restart Lantern."
                .to_string()
        }
        KeySource::Passphrase => {
            "The vault can't be opened: APP_VAULT_PASSPHRASE isn't the passphrase it was made with. Set the right one and restart Lantern."
                .to_string()
        }
        KeySource::File => {
            if made_with == "passphrase" {
                "The vault can't be opened: it was made with APP_VAULT_PASSPHRASE. Set it again and restart Lantern.".to_string()
            } else {
                "The vault can't be opened: vault.key in the data folder isn't the key it was made with. Put back the right copy and restart Lantern.".to_string()
            }
        }
    })?;
    let mut key = Zeroizing::new([0u8; crypto::KEY_LEN]);
    if opened.len() != crypto::KEY_LEN {
        return Err(damaged);
    }
    key.copy_from_slice(&opened);
    Ok(key)
}

/// An item's row as listed: plain fields with their values, secrets by name only.
fn listed(r: &Row) -> Value {
    let plain: Value = serde_json::from_str(&r.s("plain")).unwrap_or_else(|_| json!({}));
    let secrets: Vec<&str> = r.s("secrets").split(',').filter(|s| !s.is_empty()).map(|s| field(s).map(|f| f.key).unwrap_or("")).collect();
    let kind = r.s("kind");
    json!({
        "id": r.s("id"),
        "kind": kind,
        "kind_label": KINDS.iter().find(|(k, _)| *k == kind).map(|(_, l)| *l).unwrap_or("Account"),
        "name": r.s("name"),
        "site": r.s("site"),
        "fields": plain,
        "secrets": secrets,
        "created_at": r.get("created_at").cloned().unwrap_or(Value::Null),
        "updated_at": r.get("updated_at").cloned().unwrap_or(Value::Null),
        "revealed_at": r.get("revealed_at").cloned().unwrap_or(Value::Null),
    })
}

/// Applies a create or change request to `item`: `kind`, `name`, `site`, and `fields`; 400 in words when
/// something isn't right.
fn apply(item: &mut Item, body: &Value) -> Result<()> {
    if let Some(k) = body.get("kind") {
        let k = k.as_str().unwrap_or_default();
        if !KINDS.iter().any(|(id, _)| *id == k) {
            return Err(err(400, "Choose what kind of account this is: tracker, Usenet provider, Usenet indexer or other."));
        }
        item.kind = k.to_string();
    }
    if item.kind.is_empty() {
        item.kind = "tracker".into();
    }
    if let Some(n) = body.get("name") {
        item.name = n.as_str().unwrap_or_default().trim().to_string();
    }
    if item.name.is_empty() {
        return Err(err(400, "Give the account a name, for example the site's name."));
    }
    if item.name.chars().count() > 120 {
        return Err(err(400, "The name is too long: keep it under 120 characters."));
    }
    if let Some(s) = body.get("site") {
        item.site = s.as_str().unwrap_or_default().trim().chars().take(500).collect();
    }
    let Some(fields) = body.get("fields") else { return Ok(()) };
    let Some(fields) = fields.as_object() else { return Err(err(400, "fields must be an object of field names and values.")) };
    for (k, v) in fields {
        let Some(f) = field(k) else {
            return Err(err(400, format!("The vault has no field called {k}.")));
        };
        let v = match v {
            Value::Null => {
                item.fields.remove(k);
                continue;
            }
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            _ => return Err(err(400, format!("{} must be text.", f.label))),
        };
        if v.len() > MAX_VALUE {
            return Err(err(400, format!("{} is too long: keep it under 64 KB.", f.label)));
        }
        let v = if f.secret { v } else { v.trim().to_string() };
        if f.secret && (v.is_empty() || v == MASK) {
            continue;
        }
        if f.key == "totp_seed" && totp::parse(&v).is_none() {
            return Err(err(
                400,
                "The 2FA seed isn't one Lantern can read. Copy the letters and numbers under the QR code, or the otpauth:// link.",
            ));
        }
        if v.is_empty() {
            item.fields.remove(k);
        } else {
            item.fields.insert(k.clone(), v);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_never_print_themselves() {
        let s = Secret::new("hunter2".into());
        assert_eq!(format!("{s:?}"), "••••••");
        assert_eq!(format!("{:?}", BTreeMap::from([("password", s.clone())])), r#"{"password": ••••••}"#);
        assert_eq!(s.expose(), "hunter2");
    }

    #[test]
    fn every_field_is_offered_by_a_kind_the_vault_knows() {
        for f in FIELDS {
            assert!(f.kinds.iter().all(|k| KINDS.iter().any(|(id, _)| id == k)), "{}", f.key);
        }
    }

    #[test]
    fn a_change_keeps_masked_and_empty_secrets() {
        let mut item = Item { name: "Site".into(), kind: "tracker".into(), ..Item::default() };
        apply(&mut item, &json!({ "fields": { "username": " me ", "password": "pw" } })).unwrap();
        apply(&mut item, &json!({ "fields": { "password": MASK } })).unwrap();
        apply(&mut item, &json!({ "fields": { "password": "" } })).unwrap();
        assert_eq!(item.fields.get("password").map(String::as_str), Some("pw"));
        assert_eq!(item.fields.get("username").map(String::as_str), Some("me"));
        apply(&mut item, &json!({ "fields": { "password": null, "username": "" } })).unwrap();
        assert!(item.fields.is_empty());
        assert!(apply(&mut item, &json!({ "fields": { "pin": "1" } })).is_err());
        assert!(apply(&mut item, &json!({ "kind": "bank" })).is_err());
        assert!(apply(&mut item, &json!({ "name": "  " })).is_err());
        assert!(apply(&mut item, &json!({ "fields": { "totp_seed": "not a seed!" } })).is_err());
    }
}
