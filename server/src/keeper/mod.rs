//! The account keeper: for each account in the vault, the site's rules (minimum ratio, hit-and-runs, how often you
//! must sign in or be active, when the login or cookie runs out) and the account's numbers (ratio, upload,
//! download, seed time, bonus points), and what is at risk soon.
//!
//! - An account linked to an indexer knows on its own when Lantern last signed in (any search or test that
//!   worked) and when it last grabbed something; [`keep_alive`] signs in again before a deadline comes near.
//! - The numbers come from the account's page (typed in, or sent by a script through the API); each change is kept
//!   in `keeper_history` for the trend.
//! - [`risks`] says, in words, what will break a rule within the warning days; [`tick`] puts that in the
//!   `accounts` notice banner and sends it once through ntfy, Discord or email ([`alerts`]).

pub mod alerts;
pub mod api;

use crate::AppState;
use kit_server_core::{err, p, Db, Result, Row, RowExt, SqliteDb};
use serde::Serialize;
use serde_json::{json, Map, Value};

/// A day, in seconds.
pub const DAY: f64 = 86400.0;

/// The keeper's tables: `keeper_accounts` (rules, numbers and links, one row per vault item), `keeper_history`
/// (the numbers each time they changed) and `keeper_alerts` (which warning was sent, so it is sent once).
pub fn migrate(db: &SqliteDb) -> anyhow::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS keeper_accounts (
            vault_id TEXT PRIMARY KEY,
            indexer_id INTEGER,
            keep_alive INTEGER NOT NULL DEFAULT 1,
            min_ratio REAL NOT NULL DEFAULT 0,
            max_hnr INTEGER NOT NULL DEFAULT 0,
            hnr_hours REAL NOT NULL DEFAULT 0,
            login_days INTEGER NOT NULL DEFAULT 0,
            activity_days INTEGER NOT NULL DEFAULT 0,
            login_expires REAL,
            cookie_expires REAL,
            uploaded REAL,
            downloaded REAL,
            ratio REAL,
            seed_hours REAL,
            bonus REAL,
            hnr_count INTEGER,
            stats_at REAL,
            signed_in_at REAL,
            active_at REAL,
            keep_alive_at REAL,
            keep_alive_error TEXT NOT NULL DEFAULT ''
        );
        CREATE TABLE IF NOT EXISTS keeper_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            vault_id TEXT NOT NULL,
            at REAL NOT NULL,
            uploaded REAL,
            downloaded REAL,
            ratio REAL,
            seed_hours REAL,
            bonus REAL,
            hnr_count INTEGER
        );
        CREATE INDEX IF NOT EXISTS keeper_history_by_account ON keeper_history(vault_id, at);
        CREATE TABLE IF NOT EXISTS keeper_alerts (
            vault_id TEXT NOT NULL,
            key TEXT NOT NULL,
            level TEXT NOT NULL,
            sent_at REAL NOT NULL,
            PRIMARY KEY (vault_id, key)
        );",
    )
    .map_err(|e| anyhow::anyhow!("The account keeper's tables can't be made: {}", e.detail))
}

/// The rules of the account's site, as the owner typed them (0 or `None`: no such rule).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Rules {
    /// the lowest ratio the site allows
    pub min_ratio: f64,
    /// how many hit-and-runs the site allows before it acts, 0 for no limit
    pub max_hnr: i64,
    /// how long each torrent must seed to not count as a hit-and-run, in hours (shown as a reminder)
    pub hnr_hours: f64,
    /// sign in at least every this many days, or the account is closed
    pub login_days: i64,
    /// download or upload something at least every this many days
    pub activity_days: i64,
    /// when the site's login (or invite period, or account) runs out, seconds since 1970
    pub login_expires: Option<f64>,
    /// when the saved cookie runs out, seconds since 1970
    pub cookie_expires: Option<f64>,
}

/// The account's numbers from its page on the site.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Stats {
    /// uploaded, in bytes
    pub uploaded: Option<f64>,
    /// downloaded, in bytes
    pub downloaded: Option<f64>,
    /// the ratio the site shows (else uploaded ÷ downloaded)
    pub ratio: Option<f64>,
    /// total seed time, in hours
    pub seed_hours: Option<f64>,
    /// bonus points
    pub bonus: Option<f64>,
    /// hit-and-runs the site counts now
    pub hnr_count: Option<i64>,
    /// when these numbers were read
    pub at: Option<f64>,
}

impl Stats {
    /// The ratio: the site's, else uploaded ÷ downloaded; `None` without the numbers.
    pub fn ratio(&self) -> Option<f64> {
        self.ratio.or(match (self.uploaded, self.downloaded) {
            (Some(u), Some(d)) if d > 0.0 => Some(u / d),
            _ => None,
        })
    }
}

/// How bad a risk is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// it will break a rule within the warning days
    Warn,
    /// it breaks a rule now, or within a day
    Danger,
}

impl Level {
    /// The level's name, as kept in `keeper_alerts`.
    pub fn name(self) -> &'static str {
        match self {
            Level::Warn => "warn",
            Level::Danger => "danger",
        }
    }
}

/// One thing that puts an account at risk.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Risk {
    /// which rule: `login`, `activity`, `login_expires`, `cookie_expires`, `ratio`, `hnr`, `unknown_login`
    pub key: &'static str,
    /// how bad it is
    pub level: Level,
    /// days until the deadline (below 0: passed), when the rule has one
    pub days_left: Option<f64>,
    /// what is wrong and what to do, in a sentence that names the account
    pub text: String,
}

/// An account as the keeper sees it: the vault item's name and the keeper's row.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Account {
    /// the vault item's id
    pub vault_id: String,
    /// the vault item's name
    pub name: String,
    /// the indexer it signs in through, when linked
    pub indexer_id: Option<i64>,
    /// whether Lantern signs in on its own to keep it alive
    pub keep_alive: bool,
    /// the site's rules
    pub rules: Rules,
    /// the numbers from the site
    pub stats: Stats,
    /// when Lantern (or you) last signed in, seconds since 1970
    pub signed_in_at: Option<f64>,
    /// when the account last downloaded something, seconds since 1970
    pub active_at: Option<f64>,
    /// when Lantern last tried to sign in on its own
    pub keep_alive_at: Option<f64>,
    /// why that last try failed, "" when it worked
    pub keep_alive_error: String,
}

/// `n` days in words: "today", "1 day", "3 days".
fn days_words(n: f64) -> String {
    let d = n.abs().floor() as i64;
    match d {
        0 => "today".into(),
        1 => "1 day".into(),
        d => format!("{d} days"),
    }
}

/// A number without a trailing `.0` ("1", "0.85").
fn num(x: f64) -> String {
    let s = format!("{x:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The level of a deadline `left` days away: danger within a day (or passed), warn within `warn_days`.
fn deadline_level(left: f64, warn_days: f64) -> Option<Level> {
    if left <= 1.0 {
        Some(Level::Danger)
    } else if left <= warn_days {
        Some(Level::Warn)
    } else {
        None
    }
}

/// What puts account `a` at risk at `now`, warning `warn_days` ahead; the worst first.
pub fn risks(a: &Account, now: f64, warn_days: f64) -> Vec<Risk> {
    let mut out = Vec::new();
    let name = &a.name;
    let deadline = |key: &'static str, at: f64, ahead: &str, passed: &str| {
        let left = (at - now) / DAY;
        deadline_level(left, warn_days).map(|level| {
            let text = if left < 0.0 {
                format!("{name}: {passed} {} ago.", days_words(left))
            } else if left < 1.0 {
                format!("{name}: {ahead} today.")
            } else {
                format!("{name}: {ahead} within {}.", days_words(left))
            };
            Risk { key, level, days_left: Some(left), text }
        })
    };
    let r = &a.rules;
    if r.login_days > 0 {
        match a.signed_in_at {
            Some(at) => out.extend(deadline(
                "login",
                at + r.login_days as f64 * DAY,
                &format!("sign in on the site, or the account is closed for inactivity (the rule is {} days)", r.login_days),
                &format!("the {}-day sign-in deadline passed", r.login_days),
            )),
            None => out.push(Risk {
                key: "unknown_login",
                level: Level::Warn,
                days_left: None,
                text: format!(
                    "{name}: Lantern doesn't know when you last signed in. Link it to its indexer, or press Signed in today after you sign in."
                ),
            }),
        }
    }
    if r.activity_days > 0 {
        if let Some(at) = a.active_at.or(a.signed_in_at) {
            out.extend(deadline(
                "activity",
                at + r.activity_days as f64 * DAY,
                &format!("download or seed something, the site wants activity every {} days", r.activity_days),
                &format!("the {}-day activity deadline passed", r.activity_days),
            ));
        }
    }
    if let Some(at) = r.login_expires {
        out.extend(deadline("login_expires", at, "the account's login runs out; renew it on the site", "the account's login ran out"));
    }
    if let Some(at) = r.cookie_expires {
        out.extend(deadline(
            "cookie_expires",
            at,
            "the saved cookie runs out; sign in with your browser and save the new cookie",
            "the saved cookie ran out",
        ));
    }
    if r.min_ratio > 0.0 {
        if let Some(ratio) = a.stats.ratio() {
            if ratio < r.min_ratio {
                out.push(Risk {
                    key: "ratio",
                    level: Level::Danger,
                    days_left: None,
                    text: format!(
                        "{name}: the ratio {} is under the site's minimum of {}. Seed more, or download only freeleech.",
                        num(ratio),
                        num(r.min_ratio)
                    ),
                });
            } else if ratio < r.min_ratio * 1.1 {
                out.push(Risk {
                    key: "ratio",
                    level: Level::Warn,
                    days_left: None,
                    text: format!(
                        "{name}: the ratio {} is close to the site's minimum of {}. Seed more before you download.",
                        num(ratio),
                        num(r.min_ratio)
                    ),
                });
            }
        }
    }
    if r.max_hnr > 0 {
        let n = a.stats.hnr_count.unwrap_or(0);
        let seed = if r.hnr_hours > 0.0 { format!(" Seed each torrent for {} hours.", num(r.hnr_hours)) } else { String::new() };
        if n >= r.max_hnr {
            out.push(Risk {
                key: "hnr",
                level: Level::Danger,
                days_left: None,
                text: format!("{name}: {n} hit-and-runs, the site allows {}. Seed them until they clear.{seed}", r.max_hnr),
            });
        } else if n > 0 && n + 1 >= r.max_hnr {
            out.push(Risk {
                key: "hnr",
                level: Level::Warn,
                days_left: None,
                text: format!("{name}: {n} hit-and-runs, one more reaches the site's limit of {}.{seed}", r.max_hnr),
            });
        }
    }
    out.sort_by(|a, b| b.level.cmp(&a.level).then(a.days_left.unwrap_or(f64::MAX).total_cmp(&b.days_left.unwrap_or(f64::MAX))));
    out
}

/// An optional number of a row (`None` for NULL).
fn opt(r: &Row, k: &str) -> Option<f64> {
    r.get(k).and_then(Value::as_f64)
}

/// The keeper's row of vault item `vault_id`, `None` when it has none yet.
fn row(db: &SqliteDb, vault_id: &str) -> Result<Option<Row>> {
    db.one("SELECT * FROM keeper_accounts WHERE vault_id=?", p![vault_id])
}

/// The latest time an indexer worked (any search, test or grab), and the latest grab, from its history.
fn indexer_times(db: &SqliteDb, indexer_id: i64) -> Result<(Option<f64>, Option<f64>)> {
    let r = db.one(
        "SELECT MAX(at) AS seen, MAX(CASE WHEN kind='grab' THEN at END) AS grabbed FROM indexer_history WHERE indexer_id=? AND ok=1",
        p![indexer_id],
    )?;
    Ok(r.map(|r| (opt(&r, "seen"), opt(&r, "grabbed"))).unwrap_or((None, None)))
}

/// The later of two optional times.
fn later(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// Account `vault_id` named `name`, from its row (defaults when it has none) and its indexer's history.
fn account(db: &SqliteDb, vault_id: &str, name: &str) -> Result<Account> {
    let r = row(db, vault_id)?.unwrap_or_default();
    let indexer_id = r.get("indexer_id").and_then(Value::as_i64);
    let (seen, grabbed) = match indexer_id {
        Some(id) => indexer_times(db, id)?,
        None => (None, None),
    };
    Ok(Account {
        vault_id: vault_id.to_string(),
        name: name.to_string(),
        indexer_id,
        keep_alive: r.get("keep_alive").map(|_| r.b("keep_alive")).unwrap_or(true),
        rules: Rules {
            min_ratio: r.f("min_ratio"),
            max_hnr: r.i("max_hnr"),
            hnr_hours: r.f("hnr_hours"),
            login_days: r.i("login_days"),
            activity_days: r.i("activity_days"),
            login_expires: opt(&r, "login_expires"),
            cookie_expires: opt(&r, "cookie_expires"),
        },
        stats: Stats {
            uploaded: opt(&r, "uploaded"),
            downloaded: opt(&r, "downloaded"),
            ratio: opt(&r, "ratio"),
            seed_hours: opt(&r, "seed_hours"),
            bonus: opt(&r, "bonus"),
            hnr_count: r.get("hnr_count").and_then(Value::as_i64),
            at: opt(&r, "stats_at"),
        },
        signed_in_at: later(opt(&r, "signed_in_at"), seen),
        active_at: later(opt(&r, "active_at"), grabbed),
        keep_alive_at: opt(&r, "keep_alive_at"),
        keep_alive_error: r.s("keep_alive_error"),
    })
}

/// Every account in the vault, as the keeper sees it.
pub fn accounts(st: &AppState) -> Result<Vec<Account>> {
    st.vault
        .list()?
        .iter()
        .map(|it| account(&st.db, it["id"].as_str().unwrap_or_default(), it["name"].as_str().unwrap_or_default()))
        .collect()
}

/// Account `vault_id`; 404 when the vault has no such item.
pub fn get(st: &AppState, vault_id: &str) -> Result<Account> {
    let item = st.vault.get(vault_id)?;
    account(&st.db, vault_id, item["name"].as_str().unwrap_or_default())
}

/// Account `a` for the API: its rules, numbers, ratio and risks now.
pub fn public(a: &Account, now: f64, warn_days: f64) -> Value {
    let mut v = serde_json::to_value(a).unwrap_or_else(|_| json!({}));
    v["ratio"] = json!(a.stats.ratio());
    v["risks"] = json!(risks(a, now, warn_days));
    v
}

/// Makes sure vault item `vault_id` has a keeper row.
fn ensure_row(db: &SqliteDb, vault_id: &str) -> Result<()> {
    db.execute("INSERT OR IGNORE INTO keeper_accounts (vault_id) VALUES (?)", p![vault_id])?;
    Ok(())
}

/// A time from a request: seconds since 1970, or a `YYYY-MM-DD` date (midnight UTC); `Ok(None)` for null or "".
pub fn time_of(v: &Value, what: &str) -> Result<Option<f64>> {
    match v {
        Value::Null => Ok(None),
        Value::Number(n) => Ok(n.as_f64()),
        Value::String(s) if s.trim().is_empty() => Ok(None),
        Value::String(s) => chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|d| Some(d.and_utc().timestamp() as f64))
            .ok_or_else(|| err(400, format!("{what} must be a date like 2026-12-31."))),
        _ => Err(err(400, format!("{what} must be a date like 2026-12-31."))),
    }
}

/// A number of at least 0 from a request; `Ok(None)` for null or "".
fn number_of(v: &Value, what: &str) -> Result<Option<f64>> {
    let n = match v {
        Value::Null => return Ok(None),
        Value::String(s) if s.trim().is_empty() => return Ok(None),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        Value::Number(n) => n.as_f64(),
        _ => None,
    };
    match n {
        Some(n) if n.is_finite() && n >= 0.0 => Ok(Some(n)),
        _ => Err(err(400, format!("{what} must be a number of 0 or more."))),
    }
}

/// Changes the rules and links of account `vault_id`: only what `body` has (`indexer_id`, `keep_alive`, the
/// [`Rules`] fields). The account as answered.
pub fn set_rules(st: &AppState, vault_id: &str, body: &Value) -> Result<Account> {
    st.vault.get(vault_id)?;
    ensure_row(&st.db, vault_id)?;
    let Some(m) = body.as_object() else { return Err(err(400, "Send the rules as an object.")) };
    let mut sets: Vec<(&str, Value)> = Vec::new();
    for (k, v) in m {
        let (col, val) = match k.as_str() {
            "indexer_id" => {
                let id = match v {
                    Value::Null => None,
                    v => Some(v.as_i64().ok_or_else(|| err(400, "Choose the indexer from the list."))?),
                };
                if let Some(id) = id {
                    st.indexers.get(id)?;
                }
                ("indexer_id", json!(id))
            }
            "keep_alive" => ("keep_alive", json!(v.as_bool().unwrap_or(false))),
            "min_ratio" => ("min_ratio", json!(number_of(v, "The minimum ratio")?.unwrap_or(0.0))),
            "max_hnr" => ("max_hnr", json!(number_of(v, "The hit-and-run limit")?.unwrap_or(0.0) as i64)),
            "hnr_hours" => ("hnr_hours", json!(number_of(v, "The seed time")?.unwrap_or(0.0))),
            "login_days" => ("login_days", json!(number_of(v, "Sign in every")?.unwrap_or(0.0) as i64)),
            "activity_days" => ("activity_days", json!(number_of(v, "Be active every")?.unwrap_or(0.0) as i64)),
            "login_expires" => ("login_expires", json!(time_of(v, "The login's end")?)),
            "cookie_expires" => ("cookie_expires", json!(time_of(v, "The cookie's end")?)),
            other => return Err(err(400, format!("The account keeper has no rule called {other}."))),
        };
        sets.push((col, val));
    }
    for (col, val) in sets {
        // the column names come from the list above, never from the request
        st.db.execute(&format!("UPDATE keeper_accounts SET {col}=? WHERE vault_id=?"), p![val, vault_id])?;
    }
    get(st, vault_id)
}

/// Saves the numbers of account `vault_id` from `body` (`uploaded`, `downloaded` in bytes, `ratio`, `seed_hours`,
/// `bonus`, `hnr_count`; and `signed_in`/`active` `true` for "today"), keeping a copy in the history.
pub fn set_stats(st: &AppState, vault_id: &str, body: &Value) -> Result<Account> {
    st.vault.get(vault_id)?;
    ensure_row(&st.db, vault_id)?;
    let now = crate::auth::now();
    let old = get(st, vault_id)?.stats;
    let pick = |k: &str, what: &str, was: Option<f64>| -> Result<Option<f64>> {
        match body.get(k) {
            None => Ok(was),
            Some(v) => number_of(v, what),
        }
    };
    let s = Stats {
        uploaded: pick("uploaded", "Uploaded", old.uploaded)?,
        downloaded: pick("downloaded", "Downloaded", old.downloaded)?,
        ratio: pick("ratio", "The ratio", old.ratio)?,
        seed_hours: pick("seed_hours", "Seed time", old.seed_hours)?,
        bonus: pick("bonus", "Bonus points", old.bonus)?,
        hnr_count: pick("hnr_count", "Hit-and-runs", old.hnr_count.map(|n| n as f64))?.map(|n| n as i64),
        at: Some(now),
    };
    let numbers_sent = ["uploaded", "downloaded", "ratio", "seed_hours", "bonus", "hnr_count"].iter().any(|k| body.get(*k).is_some());
    if numbers_sent {
        st.db.execute(
            "UPDATE keeper_accounts SET uploaded=?, downloaded=?, ratio=?, seed_hours=?, bonus=?, hnr_count=?, stats_at=? WHERE vault_id=?",
            p![s.uploaded, s.downloaded, s.ratio, s.seed_hours, s.bonus, s.hnr_count, now, vault_id],
        )?;
        st.db.execute(
            "INSERT INTO keeper_history (vault_id, at, uploaded, downloaded, ratio, seed_hours, bonus, hnr_count) VALUES (?,?,?,?,?,?,?,?)",
            p![vault_id, now, s.uploaded, s.downloaded, s.ratio, s.seed_hours, s.bonus, s.hnr_count],
        )?;
    }
    if body.get("signed_in").and_then(Value::as_bool) == Some(true) {
        st.db.execute("UPDATE keeper_accounts SET signed_in_at=? WHERE vault_id=?", p![now, vault_id])?;
    }
    if body.get("active").and_then(Value::as_bool) == Some(true) {
        st.db.execute("UPDATE keeper_accounts SET active_at=? WHERE vault_id=?", p![now, vault_id])?;
    }
    get(st, vault_id)
}

/// The numbers of account `vault_id` each time they changed, oldest first (the last `limit`).
pub fn history(st: &AppState, vault_id: &str, limit: i64) -> Result<Vec<Value>> {
    let rows = st.db.query(
        "SELECT at, uploaded, downloaded, ratio, seed_hours, bonus, hnr_count FROM (SELECT * FROM keeper_history WHERE vault_id=? ORDER BY at DESC LIMIT ?) ORDER BY at",
        p![vault_id, limit.clamp(1, 1000)],
    )?;
    Ok(rows.into_iter().map(Value::Object).collect())
}

/// Signs in to account `a` through its indexer now (a test search), and notes when and whether it worked.
pub async fn sign_in(st: &AppState, a: &Account) -> Result<()> {
    let Some(id) = a.indexer_id else {
        return Err(err(400, format!("{} isn't linked to an indexer, so Lantern can't sign in to it. Choose its indexer first.", a.name)));
    };
    ensure_row(&st.db, &a.vault_id)?;
    let r = st.indexers.test(id).await;
    let why = r.as_ref().err().map(|e| e.detail.clone()).unwrap_or_default();
    st.db.execute(
        "UPDATE keeper_accounts SET keep_alive_at=?, keep_alive_error=? WHERE vault_id=?",
        p![crate::auth::now(), why, a.vault_id],
    )?;
    match r {
        Ok(_) => {
            tracing::info!("Keeper: signed in to {}", a.name);
            Ok(())
        }
        Err(e) => {
            tracing::warn!("Keeper: signing in to {} failed: {}", a.name, e.detail);
            Err(err(502, format!("Signing in to {} didn't work: {}", a.name, e.detail)))
        }
    }
}

/// Whether account `a` is due for a sign-in at `now`: linked, kept alive, not signed in for `every_days` (or for
/// a third of its sign-in rule, when that is sooner), and not tried in the last 6 hours.
pub fn due(a: &Account, now: f64, every_days: f64) -> bool {
    if !a.keep_alive || a.indexer_id.is_none() {
        return false;
    }
    let mut every = every_days.max(0.25);
    if a.rules.login_days > 0 {
        every = every.min(a.rules.login_days as f64 / 3.0);
    }
    let stale = a.signed_in_at.is_none_or(|at| now - at >= every * DAY);
    let tried_lately = a.keep_alive_at.is_some_and(|at| now - at < 6.0 * 3600.0);
    stale && !tried_lately
}

/// Signs in to every account that is [`due`]; how many it tried.
pub async fn keep_alive(st: &AppState) -> usize {
    let Ok(all) = accounts(st) else { return 0 };
    let now = crate::auth::now();
    let every = st.settings.int("keeper_keepalive_days").max(1) as f64;
    let mut n = 0;
    for a in all.iter().filter(|a| due(a, now, every)) {
        let _ = sign_in(st, a).await;
        n += 1;
    }
    n
}

/// Every risk of every account now, with the account's id.
pub fn all_risks(st: &AppState) -> Result<Vec<(String, Risk)>> {
    let now = crate::auth::now();
    let warn = st.settings.int("keeper_warn_days").max(1) as f64;
    let mut all: Vec<(String, Risk)> =
        accounts(st)?.iter().flat_map(|a| risks(a, now, warn).into_iter().map(|r| (a.vault_id.clone(), r))).collect();
    all.sort_by(|(_, a), (_, b)| b.level.cmp(&a.level).then(a.days_left.unwrap_or(f64::MAX).total_cmp(&b.days_left.unwrap_or(f64::MAX))));
    Ok(all)
}

/// The accounts notice's sentence for the risks, `None` when no account is at risk.
pub fn notice_text(risks: &[(String, Risk)]) -> Option<String> {
    match risks {
        [] => None,
        [(_, one)] => Some(format!("{} Open Accounts for the details.", one.text)),
        many => {
            let mut ids: Vec<&str> = many.iter().map(|(id, _)| id.as_str()).collect();
            ids.sort_unstable();
            ids.dedup();
            let n = ids.len();
            let first = &many[0].1.text;
            if n == 1 {
                Some(format!("{first} And {} more for this account. Open Accounts for the details.", many.len() - 1))
            } else {
                Some(format!("{n} accounts are at risk. The most urgent: {first} Open Accounts for the details."))
            }
        }
    }
}

/// One round of the keeper: keeps logins alive, then sends each new or worse risk through the alert channels.
pub async fn tick(st: &AppState) {
    keep_alive(st).await;
    match all_risks(st) {
        Ok(risks) => alerts::send_new(st, &risks).await,
        Err(e) => tracing::warn!("Keeper: the accounts can't be read: {}", e.detail),
    }
}

/// The settings of the alert channels as an object, for [`alerts::Channels::from_settings`].
pub fn alert_settings(st: &AppState) -> Map<String, Value> {
    let keys = [
        "alert_ntfy_url",
        "alert_ntfy_token",
        "alert_discord_webhook",
        "alert_email_to",
        "smtp_host",
        "smtp_security",
        "smtp_user",
        "smtp_password",
        "smtp_from",
    ];
    let mut m: Map<String, Value> = keys.iter().map(|k| (k.to_string(), json!(st.settings.str(k)))).collect();
    m.insert("smtp_port".into(), json!(st.settings.int("smtp_port")));
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An account named Site with `rules`, signed in at `signed` days ago.
    fn site(rules: Rules, signed: Option<f64>) -> Account {
        Account { name: "Site".into(), rules, signed_in_at: signed.map(|d| 1000.0 * DAY - d * DAY), keep_alive: true, ..Account::default() }
    }

    const NOW: f64 = 1000.0 * DAY;

    #[test]
    fn sign_in_deadlines_warn_ahead_then_turn_red() {
        let r = Rules { login_days: 30, ..Rules::default() };
        assert!(risks(&site(r.clone(), Some(10.0)), NOW, 7.0).is_empty());
        let w = risks(&site(r.clone(), Some(25.0)), NOW, 7.0);
        assert_eq!(w.len(), 1);
        assert_eq!((w[0].key, w[0].level), ("login", Level::Warn));
        assert_eq!(w[0].text, "Site: sign in on the site, or the account is closed for inactivity (the rule is 30 days) within 5 days.");
        let d = risks(&site(r.clone(), Some(29.5)), NOW, 7.0);
        assert_eq!(d[0].level, Level::Danger);
        assert!(d[0].text.ends_with("today."), "{}", d[0].text);
        let late = risks(&site(r.clone(), Some(33.0)), NOW, 7.0);
        assert_eq!(late[0].text, "Site: the 30-day sign-in deadline passed 3 days ago.");
        let unknown = risks(&site(r, None), NOW, 7.0);
        assert_eq!(unknown[0].key, "unknown_login");
    }

    #[test]
    fn ratio_hit_and_runs_and_expiry_dates() {
        let mut a = site(
            Rules { min_ratio: 1.0, max_hnr: 3, hnr_hours: 72.0, cookie_expires: Some(NOW + 3.0 * DAY), ..Rules::default() },
            Some(0.0),
        );
        a.stats = Stats { uploaded: Some(85.0), downloaded: Some(100.0), hnr_count: Some(2), ..Stats::default() };
        let r = risks(&a, NOW, 7.0);
        let keys: Vec<_> = r.iter().map(|r| (r.key, r.level)).collect();
        assert_eq!(keys, [("ratio", Level::Danger), ("cookie_expires", Level::Warn), ("hnr", Level::Warn)]);
        assert_eq!(r[0].text, "Site: the ratio 0.85 is under the site's minimum of 1. Seed more, or download only freeleech.");
        assert_eq!(r[2].text, "Site: 2 hit-and-runs, one more reaches the site's limit of 3. Seed each torrent for 72 hours.");
        a.stats = Stats { ratio: Some(1.05), hnr_count: Some(3), ..Stats::default() };
        let r = risks(&a, NOW, 7.0);
        assert_eq!(
            r.iter().map(|r| (r.key, r.level)).collect::<Vec<_>>(),
            [("hnr", Level::Danger), ("cookie_expires", Level::Warn), ("ratio", Level::Warn)]
        );
        a.stats = Stats { ratio: Some(2.0), ..Stats::default() };
        a.rules.cookie_expires = Some(NOW + 30.0 * DAY);
        assert!(risks(&a, NOW, 7.0).is_empty());
    }

    #[test]
    fn keep_alive_signs_in_before_the_rule_comes_near() {
        let mut a = site(Rules::default(), Some(2.0));
        assert!(!due(&a, NOW, 3.0), "not linked");
        a.indexer_id = Some(1);
        assert!(!due(&a, NOW, 3.0));
        assert!(due(&a, NOW, 1.0));
        a.rules.login_days = 6;
        assert!(due(&a, NOW, 3.0), "a third of a 6-day rule is 2 days");
        a.keep_alive_at = Some(NOW - 3600.0);
        assert!(!due(&a, NOW, 3.0), "tried an hour ago");
        a.keep_alive_at = None;
        a.keep_alive = false;
        assert!(!due(&a, NOW, 3.0));
    }

    #[test]
    fn the_banner_names_the_most_urgent_risk() {
        assert_eq!(notice_text(&[]), None);
        let risk = |t: &str| Risk { key: "login", level: Level::Warn, days_left: None, text: t.into() };
        assert_eq!(notice_text(&[("a".into(), risk("A: x."))]).unwrap(), "A: x. Open Accounts for the details.");
        let two = [("a".into(), risk("A: x.")), ("b".into(), risk("B: y."))];
        assert_eq!(notice_text(&two).unwrap(), "2 accounts are at risk. The most urgent: A: x. Open Accounts for the details.");
        assert_eq!(time_of(&json!("2026-12-31"), "x").unwrap(), Some(1798675200.0));
        assert!(time_of(&json!("31/12/2026"), "x").is_err());
    }
}
