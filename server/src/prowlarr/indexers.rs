// Ported from Prowlarr v2.7.0.5723 (src/Prowlarr.Api.V1/Indexers/IndexerController.cs, IndexerResource.cs,
// IndexerBulkResource.cs, IndexerCapabilityResource.cs, IndexerDefaultCategoriesController.cs,
// IndexerStatusController.cs, IndexerStatusResource.cs, IndexerStatsController.cs,
// src/Prowlarr.Api.V1/ProviderControllerBase.cs, src/NzbDrone.Core/Indexers/IndexerFactory.cs,
// IndexerBaseSettings.cs, IndexerTorrentBaseSettings.cs, Settings/NoAuthTorrentBaseSettings.cs,
// Definitions/Cardigann/CardigannSettings.cs, Definitions/Cardigann/Cardigann.cs (DefaultDefinitions, RequestAction),
// Definitions/Newznab/Newznab.cs, NewznabSettings.cs, Definitions/Torznab/Torznab.cs, TorznabSettings.cs,
// HttpIndexerBase.cs (TestConnection), src/NzbDrone.Core/IndexerStats/IndexerStatisticsService.cs,
// src/NzbDrone.Core/Parser/StringUtil.cs (NormalizeTitle)), GPL-3.0.

//! Prowlarr's indexer resource, as its web app and tools use it:
//!
//! ```text
//! GET    /api/v1/indexer                    every indexer, by name (IndexerResource)
//! POST   /api/v1/indexer[?forceSave=]       add one (checked, and tested when it is on): 201
//! GET    /api/v1/indexer/{id}               one
//! PUT    /api/v1/indexer/{id}[?forceSave=]  change one (tested when on, unless forceSave): 202
//! DELETE /api/v1/indexer/{id}               remove one and its history: {}
//! PUT    /api/v1/indexer/bulk               change several: enable, app profile, priority, seeding, tags: 202
//! DELETE /api/v1/indexer/bulk               remove several: {}
//! GET    /api/v1/indexer/schema             every definition and preset, ready to fill in
//! POST   /api/v1/indexer/test[?forceTest=]  try one without saving it
//! POST   /api/v1/indexer/testall            try every enabled one: 400 when one fails
//! POST   /api/v1/indexer/action/{name}      a provider action (`getUrls`)
//! GET    /api/v1/indexer/categories         the standard categories (ParentCats)
//! GET    /api/v1/indexerstatus              the indexers on hold after failures
//! GET    /api/v1/indexerstats               queries, grabs and answer times from the history
//! ```
//!
//! Lantern keeps its indexers in its own table ([`crate::indexers`]); this module maps them to Prowlarr's resource
//! and back: Cardigann sites (`Cardigann`), Newznab and Torznab APIs. Settings are Prowlarr's `fields`; a Cardigann
//! site's own settings follow its definition (a select's value is the option's place, as in Prowlarr).

use super::apps::categories::PARENTS;
use super::apps::provider::{
    prop, read_settings, to_schema, FieldSpec, FieldType, Implementation, Initial, Options, Privacy, ProviderFailure, ValueKind, MASK,
};
use super::{api_exception, body_json, check, config, fail, validate_id, validation, ApiError, V1};
use crate::indexers::capabilities::{Capabilities, Category};
use crate::indexers::definition::{Definition as CardigannDefinition, Setting};
use crate::indexers::release::Protocol;
use crate::indexers::{limits, Indexer};
use crate::AppState;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};

/// The indexer routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/indexer", get(get_all).post(create))
        .route("/api/v1/indexer/bulk", put(bulk_update).delete(bulk_delete))
        .route("/api/v1/indexer/schema", get(schema))
        .route("/api/v1/indexer/test", post(test))
        .route("/api/v1/indexer/testall", post(test_all))
        .route("/api/v1/indexer/action/{name}", post(action))
        .route("/api/v1/indexer/categories", get(default_categories))
        .route("/api/v1/indexer/{id}", get(get_one).put(update).delete(remove))
        .route("/api/v1/indexerstatus", get(statuses))
        .route("/api/v1/indexerstats", get(stats))
}

// ---- the settings classes ----

/// `IndexerLimitsUnit`.
const LIMITS_UNIT: &[(i64, &str)] = &[(0, "Day"), (1, "Hour")];

/// A number field of `IndexerBaseSettings` or `IndexerTorrentBaseSettings`.
const fn number(order: i64, name: &'static str, label: &'static str, help: &'static str, kind: ValueKind, advanced: bool) -> FieldSpec {
    FieldSpec { order, name, label, help_text: Some(help), kind, field_type: FieldType::Number, advanced, ..FieldSpec::BASE }
}

/// `IndexerBaseSettings` (prefix `baseSettings.`), from `order` on.
const fn base_settings(order: i64) -> [FieldSpec; 3] {
    [
        number(order, "baseSettings.queryLimit", "IndexerSettingsQueryLimit", "IndexerSettingsQueryLimitHelpText", ValueKind::OptInt, true),
        number(
            order + 1,
            "baseSettings.grabLimit",
            "IndexerSettingsGrabLimit",
            "IndexerSettingsGrabLimitHelpText",
            ValueKind::OptInt,
            true,
        ),
        FieldSpec {
            order: order + 2,
            name: "baseSettings.limitsUnit",
            label: "IndexerSettingsLimitsUnit",
            help_text: Some("IndexerSettingsLimitsUnitHelpText"),
            kind: ValueKind::Int,
            field_type: FieldType::Select,
            advanced: true,
            options: Options::Enum(LIMITS_UNIT),
            default: Initial::Int(0),
            ..FieldSpec::BASE
        },
    ]
}

/// `IndexerTorrentBaseSettings` (prefix `torrentBaseSettings.`), from `order` on.
const fn torrent_settings(order: i64) -> [FieldSpec; 5] {
    [
        number(
            order,
            "torrentBaseSettings.appMinimumSeeders",
            "IndexerSettingsAppsMinimumSeeders",
            "IndexerSettingsAppsMinimumSeedersHelpText",
            ValueKind::OptInt,
            true,
        ),
        number(
            order + 1,
            "torrentBaseSettings.seedRatio",
            "IndexerSettingsSeedRatio",
            "IndexerSettingsSeedRatioHelpText",
            ValueKind::OptDouble,
            false,
        ),
        FieldSpec {
            unit: Some("minutes"),
            ..number(
                order + 2,
                "torrentBaseSettings.seedTime",
                "IndexerSettingsSeedTime",
                "IndexerSettingsSeedTimeHelpText",
                ValueKind::OptInt,
                true,
            )
        },
        FieldSpec {
            unit: Some("minutes"),
            ..number(
                order + 3,
                "torrentBaseSettings.packSeedTime",
                "IndexerSettingsPackSeedTime",
                "IndexerSettingsPackSeedTimeIndexerHelpText",
                ValueKind::OptInt,
                true,
            )
        },
        FieldSpec {
            order: order + 4,
            name: "torrentBaseSettings.preferMagnetUrl",
            label: "IndexerSettingsPreferMagnetUrl",
            help_text: Some("IndexerSettingsPreferMagnetUrlHelpText"),
            kind: ValueKind::Bool,
            field_type: FieldType::Checkbox,
            advanced: true,
            default: Initial::Bool(false),
            ..FieldSpec::BASE
        },
    ]
}

/// Joins field lists at compile time.
macro_rules! fields {
    ($($part:expr),* $(,)?) => {{
        const PARTS: &[&[FieldSpec]] = &[$(&$part),*];
        const N: usize = { let mut n = 0; let mut i = 0; while i < PARTS.len() { n += PARTS[i].len(); i += 1; } n };
        const OUT: [FieldSpec; N] = {
            let mut out = [FieldSpec::BASE; N];
            let (mut k, mut i) = (0, 0);
            while i < PARTS.len() {
                let mut j = 0;
                while j < PARTS[i].len() {
                    out[k] = PARTS[i][j];
                    k += 1;
                    j += 1;
                }
                i += 1;
            }
            out
        };
        &OUT
    }};
}

/// `CardigannSettings`: the definition (hidden), the address (picked from the definition's), the limits and the
/// torrent settings.
const CARDIGANN_FIELDS: &[FieldSpec] = fields![
    [
        FieldSpec { order: 0, name: "definitionFile", hidden: Some("hidden"), ..FieldSpec::BASE },
        FieldSpec {
            order: 1,
            name: "baseUrl",
            label: "IndexerSettingsBaseUrl",
            help_text: Some("IndexerSettingsBaseUrlHelpText"),
            field_type: FieldType::Select,
            options_action: Some("getUrls"),
            ..FieldSpec::BASE
        },
    ],
    base_settings(2),
    torrent_settings(5),
];

/// The first fields of `NewznabSettings`: the address, the API path and the API key.
const NAB_HEAD: [FieldSpec; 3] = [
    FieldSpec { order: 0, name: "baseUrl", label: "URL", ..FieldSpec::BASE },
    FieldSpec {
        order: 1,
        name: "apiPath",
        label: "IndexerSettingsApiPath",
        help_text: Some("IndexerSettingsApiPathHelpText"),
        help_tokens: &[("url", "/api")],
        advanced: true,
        default: Initial::Text("/api"),
        ..FieldSpec::BASE
    },
    FieldSpec {
        order: 2,
        name: "apiKey",
        label: "ApiKey",
        help_text: Some("IndexerNewznabSettingsApiKeyHelpText"),
        privacy: Privacy::ApiKey,
        ..FieldSpec::BASE
    },
];

/// The extra parameters and VIP expiry of `NewznabSettings`, from `order` on.
const fn nab_tail(order: i64) -> [FieldSpec; 2] {
    [
        FieldSpec {
            order,
            name: "additionalParameters",
            label: "IndexerSettingsAdditionalParameters",
            help_text: Some("IndexerNewznabSettingsAdditionalParametersHelpText"),
            advanced: true,
            ..FieldSpec::BASE
        },
        FieldSpec {
            order: order + 1,
            name: "vipExpiration",
            label: "IndexerSettingsVipExpiration",
            help_text: Some("IndexerNewznabSettingsVipExpirationHelpText"),
            default: Initial::Text(""),
            ..FieldSpec::BASE
        },
    ]
}

/// `NewznabSettings`.
const NEWZNAB_FIELDS: &[FieldSpec] = fields![NAB_HEAD, nab_tail(3), base_settings(5)];

/// `TorznabSettings`: Newznab's with the torrent settings after the API key.
const TORZNAB_FIELDS: &[FieldSpec] = fields![NAB_HEAD, torrent_settings(3), nab_tail(8), base_settings(10)];

/// The checks are made by [`validate_settings`]; the generic provider hook has nothing to add.
fn no_checks(_: &Map<String, Value>) -> Vec<ProviderFailure> {
    Vec::new()
}

/// The three kinds of indexer Lantern has, as Prowlarr's provider classes.
const IMPLEMENTATIONS: [Implementation; 3] = [
    Implementation {
        implementation: "Cardigann",
        implementation_name: "Cardigann",
        config_contract: "CardigannSettings",
        fields: CARDIGANN_FIELDS,
        validate: no_checks,
    },
    Implementation {
        implementation: "Newznab",
        implementation_name: "Newznab",
        config_contract: "NewznabSettings",
        fields: NEWZNAB_FIELDS,
        validate: no_checks,
    },
    Implementation {
        implementation: "Torznab",
        implementation_name: "Torznab",
        config_contract: "TorznabSettings",
        fields: TORZNAB_FIELDS,
        validate: no_checks,
    },
];

/// The implementation of a Lantern indexer kind (`cardigann`, `newznab`, `torznab`).
fn implementation_of_kind(kind: &str) -> &'static Implementation {
    match kind {
        "newznab" => &IMPLEMENTATIONS[1],
        "torznab" => &IMPLEMENTATIONS[2],
        _ => &IMPLEMENTATIONS[0],
    }
}

/// The Lantern kind of a Prowlarr implementation name, in any case.
fn kind_of_implementation(name: &str) -> Option<&'static str> {
    match name.to_lowercase().as_str() {
        "cardigann" => Some("cardigann"),
        "newznab" => Some("newznab"),
        "torznab" => Some("torznab"),
        _ => None,
    }
}

/// Where Lantern keeps a Newznab/Torznab field among the indexer's settings (Lantern's own names).
const NAB_KEYS: [(&str, &str); 4] =
    [("apiPath", "api_path"), ("apiKey", "apikey"), ("additionalParameters", "extra"), ("vipExpiration", "vip_expiration")];

/// The Lantern settings key of field `name`.
fn lantern_key(name: &str) -> &str {
    NAB_KEYS.iter().find(|(f, _)| *f == name).map(|(_, k)| *k).unwrap_or(name)
}

/// The value of field `f` from Lantern's settings text: a number, a switch or text as the field's kind needs.
fn value_from_text(f: &FieldSpec, text: Option<&String>) -> Value {
    let Some(t) = text.map(|t| t.trim()) else { return f.default.value() };
    match f.kind {
        ValueKind::Text => json!(t),
        ValueKind::Int => t.parse::<i64>().map(|n| json!(n)).unwrap_or_else(|_| f.default.value()),
        ValueKind::OptInt => t.parse::<i64>().map(|n| json!(n)).unwrap_or(Value::Null),
        ValueKind::Double | ValueKind::OptDouble => t.parse::<f64>().map(|n| json!(n)).unwrap_or(Value::Null),
        ValueKind::Bool => json!(t.eq_ignore_ascii_case("true") || t == "1"),
        ValueKind::IntList | ValueKind::TextList => json!(t.split(',').filter(|x| !x.is_empty()).collect::<Vec<_>>()),
    }
}

/// A field value as Lantern keeps it: text, "" for nothing.
fn text_of(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

/// An indexer's standard settings as Prowlarr's settings class holds them (field name → value).
fn standard_values(ix: &Indexer) -> Map<String, Value> {
    let imp = implementation_of_kind(&ix.kind);
    let mut out = Map::new();
    for f in imp.fields {
        let v = match f.name {
            "definitionFile" => json!(ix.definition),
            "baseUrl" if ix.base_url.is_empty() => Value::Null,
            "baseUrl" => json!(ix.base_url),
            name => value_from_text(f, ix.settings.get(lantern_key(name))),
        };
        out.insert(f.name.into(), v);
    }
    out
}

// ---- a definition's own settings ----

/// The settings of a Cardigann definition as Prowlarr reads them: its own, or a user name and password when it has
/// none, and a CAPTCHA field when its sign-in has one.
fn cardigann_settings(def: &CardigannDefinition) -> Vec<Setting> {
    let mut list = if def.settings.is_empty() {
        vec![
            Setting { name: "username".into(), label: "Username".into(), kind: "text".into(), ..Setting::default() },
            Setting { name: "password".into(), label: "Password".into(), kind: "password".into(), ..Setting::default() },
        ]
    } else {
        def.settings.clone()
    };
    if def.login.as_ref().is_some_and(|l| l.captcha.is_some()) && !list.iter().any(|s| s.kind == "cardigannCaptcha") {
        list.push(Setting {
            name: "cardigannCaptcha".into(),
            label: "CAPTCHA".into(),
            kind: "cardigannCaptcha".into(),
            ..Setting::default()
        });
    }
    list
}

/// A select setting's option keys, sorted as Prowlarr sorts them (its value is a place in this list).
fn sorted_keys(s: &Setting) -> Vec<String> {
    let mut keys: Vec<String> = s.options.keys().cloned().collect();
    keys.sort();
    keys
}

/// Whether a definition setting holds a secret Lantern never sends back.
fn secret_setting(s: &Setting) -> bool {
    !s.is_info() && s.kind != "checkbox" && s.kind != "select" && s.is_secret()
}

/// The info texts Prowlarr shows in place of some settings (`MapCardigannField`): label, text, help link.
fn info_text(kind: &str, indexer_name: &str) -> Option<(String, String, &'static str)> {
    Some(match kind {
        "info_cookie" => (
            "How to get the Cookie".into(),
            "<ol><li>Login to this tracker with your browser</li><li>If present in the login page, ensure you have the <b>Remember me</b> ticked and the <b>Log Me Out if IP Changes</b> unticked when you login</li><li>Navigate to the web site's torrent search page to view the list of available torrents for download</li><li>Open the <b>DevTools</b> panel by pressing <b>F12</b></li><li>Select the <b>Network</b> tab</li><li>Click on the <b>Doc</b> button (Chrome Browser) or <b>HTML</b> button (FireFox)</li><li>Refresh the page by pressing <b>F5</b></li><li>Click on the first row entry</li><li>Select the <b>Headers</b> tab on the Right panel</li><li>Find <b>'cookie:'</b> in the <b>Request Headers</b> section</li><li><b>Select</b> and <b>Copy</b> the whole cookie string <i>(everything after 'cookie: ')</i> and <b>Paste</b> here.</li></ol>".into(),
            "https://wiki.servarr.com/useful-tools#finding-cookies",
        ),
        "info_useragent" => (
            "How to get the User-Agent".into(),
            "<ol><li>From the same place you fetched the cookie,</li><li>Find <b>'user-agent:'</b> in the <b>Request Headers</b> section</li><li><b>Select</b> and <b>Copy</b> the whole user-agent string <i>(everything after 'user-agent: ')</i> and <b>Paste</b> here.</li></ol>".into(),
            "https://wiki.servarr.com/useful-tools#finding-cookies",
        ),
        "info_flaresolverr" => (
            "FlareSolverr Info".into(),
            "This site may use Cloudflare DDoS Protection, therefore Prowlarr requires <a href=\"https://wiki.servarr.com/prowlarr/faq#can-i-use-flaresolverr-indexers\" target=\"_blank\" rel=\"noreferrer\">FlareSolverr</a> to access it.".into(),
            "https://wiki.servarr.com/prowlarr/faq#can-i-use-flaresolverr-indexers",
        ),
        "info_category_8000" => (
            format!("About {indexer_name} Categories"),
            format!("{indexer_name} does not return categories in its search results. To sync to your apps, include 8000(Other) in your Apps' Sync Categories."),
            "https://wiki.servarr.com/prowlarr/faq#prowlarr-will-not-sync-x-indexer-to-app",
        ),
        _ => return None,
    })
}

/// One definition setting as a field (`MapCardigannField`), its value from `saved` (Lantern's settings) when given.
fn cardigann_field(s: &Setting, order: usize, indexer_name: &str, saved: Option<&BTreeMap<String, String>>) -> Value {
    let mut f = Map::new();
    f.insert("order".into(), json!(order));
    f.insert("name".into(), json!(s.name));
    f.insert("label".into(), json!(s.label));
    let mut kind = if s.kind == "text" { "textbox".to_string() } else { s.kind.clone() };
    let mut value: Value;
    let stored = |key: &str| saved.and_then(|m| m.get(key));
    if s.kind == "select" {
        let keys = sorted_keys(s);
        let options: Vec<Value> = keys.iter().enumerate().map(|(i, k)| json!({ "value": i, "name": s.options[k], "order": 0 })).collect();
        f.insert("selectOptions".into(), Value::Array(options));
        let place = |k: Option<&String>| k.and_then(|k| keys.iter().position(|x| x == k)).map(|i| i as i64).unwrap_or(-1);
        value = json!(place(s.default.as_ref()));
        if let Some(v) = stored(&s.name) {
            value = json!(place(Some(v)));
        }
    } else if s.kind == "checkbox" {
        value = json!(s.default.as_deref().is_some_and(|d| d.eq_ignore_ascii_case("true")));
        if let Some(v) = stored(&s.name) {
            value = json!(v.eq_ignore_ascii_case("true") || v == "1");
        }
    } else if let Some((label, text, link)) = info_text(&s.kind, indexer_name) {
        kind = "info".into();
        f.insert("label".into(), json!(label));
        f.insert("helpLink".into(), json!(link));
        value = json!(text);
    } else {
        value = s.default.as_ref().map(|d| json!(d)).unwrap_or(Value::Null);
        let key = if s.name == "cardigannCaptcha" { "CAPTCHA" } else { s.name.as_str() };
        if kind != "info" {
            if let Some(v) = stored(key) {
                value = if secret_setting(s) && !v.is_empty() { json!(MASK) } else { json!(v) };
            }
        }
    }
    if !value.is_null() {
        f.insert("value".into(), value);
    }
    f.insert("type".into(), json!(kind));
    f.insert("advanced".into(), json!(false));
    f.insert("privacy".into(), json!("normal"));
    f.insert("isFloat".into(), json!(false));
    Value::Object(f)
}

/// A definition setting's value from a field (`MapValue`): a select's place back to its option key, a switch as
/// `true`/`false`, else the text; a masked secret keeps the saved one.
fn setting_from_field(s: &Setting, v: &Value, saved: Option<&String>) -> String {
    let text = match v {
        Value::Null => String::new(),
        Value::String(t) => t.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        other => other.to_string(),
    };
    match s.kind.as_str() {
        "select" => {
            let keys = sorted_keys(s);
            let place = text.trim().parse::<i64>().unwrap_or(0);
            usize::try_from(place).ok().and_then(|i| keys.get(i).cloned()).or_else(|| s.default.clone()).unwrap_or_default()
        }
        "checkbox" => text.trim().eq_ignore_ascii_case("true").to_string(),
        _ if text == MASK && secret_setting(s) => saved.cloned().unwrap_or_default(),
        _ => text,
    }
}

// ---- the resource ----

/// .NET's `EncodingName` of an encoding as definitions name it.
fn encoding_name(name: &str) -> String {
    let n = name.trim().to_lowercase();
    match n.as_str() {
        "" | "utf-8" | "utf8" => "Unicode (UTF-8)",
        "windows-1250" => "Central European (Windows)",
        "windows-1251" => "Cyrillic (Windows)",
        "windows-1252" => "Western European (Windows)",
        "windows-1253" => "Greek (Windows)",
        "windows-1254" => "Turkish (Windows)",
        "windows-1255" => "Hebrew (Windows)",
        "windows-1256" => "Arabic (Windows)",
        "iso-8859-1" => "Western European (ISO)",
        "iso-8859-2" => "Central European (ISO)",
        "iso-8859-5" => "Cyrillic (ISO)",
        "iso-8859-15" => "Latin 9 (ISO)",
        "koi8-r" => "Cyrillic (KOI8-R)",
        "gb2312" => "Chinese Simplified (GB2312)",
        "gbk" => "Chinese Simplified (GBK)",
        "big5" => "Chinese Traditional (Big5)",
        "shift_jis" => "Japanese (Shift-JIS)",
        "euc-jp" => "Japanese (EUC)",
        "euc-kr" => "Korean (EUC)",
        _ => return name.to_string(),
    }
    .to_string()
}

/// Prowlarr's `NormalizeTitle` (`sortName`, `sortTitle`): words split on dots, dashes and the like, punctuation and
/// "a", "an", "the", "and", "or", "of" dropped, spaces single, small letters.
pub fn normalize_title(title: &str) -> String {
    use std::sync::OnceLock;
    static RES: OnceLock<[regex::Regex; 5]> = OnceLock::new();
    let [word, punct, common, spaces, special] = RES.get_or_init(|| {
        [
            regex::Regex::new(r"(\s|\.|,|_|-|=|'|\|)+").expect("a fixed pattern"),
            regex::Regex::new(r"[^\w\s]").expect("a fixed pattern"),
            regex::Regex::new(r"(?i)\b(a|an|the|and|or|of)\b\s?").expect("a fixed pattern"),
            regex::Regex::new(r"\s{2,}").expect("a fixed pattern"),
            regex::Regex::new(r"(\&|\:|\\|/)+").expect("a fixed pattern"),
        ]
    });
    let t = word.replace_all(title, " ");
    let t = punct.replace_all(&t, "");
    let t = common.replace_all(&t, "");
    let t = spaces.replace_all(&t, " ");
    let t = special.replace_all(&t, "");
    t.trim().to_lowercase()
}

/// A search parameter as Prowlarr's enums are written (`imdbId`, `tvMazeId`, `rId`).
fn param_name(p: &str) -> &'static str {
    match p {
        "q" => "q",
        "season" => "season",
        "ep" => "ep",
        "imdbid" => "imdbId",
        "tvdbid" => "tvdbId",
        "rid" => "rId",
        "tvmazeid" => "tvMazeId",
        "traktid" => "traktId",
        "tmdbid" => "tmdbId",
        "doubanid" => "doubanId",
        "genre" => "genre",
        "year" => "year",
        "imdbtitle" => "imdbTitle",
        "imdbyear" => "imdbYear",
        "album" => "album",
        "artist" => "artist",
        "label" => "label",
        "track" => "track",
        "title" => "title",
        "author" => "author",
        "publisher" => "publisher",
        _ => "q",
    }
}

/// A category as Prowlarr's `IndexerCategory` (id, name, its sub-categories).
pub fn category_json(c: &Category) -> Value {
    json!({ "id": c.id, "name": c.name, "subCategories": c.subs.iter().map(category_json).collect::<Vec<_>>() })
}

/// Capabilities as Prowlarr's `IndexerCapabilityResource`.
pub fn capabilities_json(c: &Capabilities) -> Value {
    // each mode once, in order (a definition may list a mode the defaults already have)
    let params = |v: &Vec<&'static str>| {
        let mut out: Vec<&str> = Vec::new();
        for p in v.iter().map(|p| param_name(p)) {
            if !out.contains(&p) {
                out.push(p);
            }
        }
        out
    };
    let mut v = json!({
        "categories": c.categories.tree(false).iter().map(category_json).collect::<Vec<_>>(),
        "supportsRawSearch": c.supports_raw_search,
        "searchParams": params(&c.search_params),
        "tvSearchParams": params(&c.tv_params),
        "movieSearchParams": params(&c.movie_params),
        "musicSearchParams": params(&c.music_params),
        "bookSearchParams": params(&c.book_params),
    });
    if let Some(m) = c.limits_max {
        v["limitsMax"] = json!(m);
    }
    if let Some(d) = c.limits_default {
        v["limitsDefault"] = json!(d);
    }
    v
}

/// `IndexerPrivacy` as written (`public`, `semiPrivate`, `private`) from a definition's `type`.
fn privacy_name(kind: &str) -> &'static str {
    match kind {
        "private" => "private",
        "public" => "public",
        _ => "semiPrivate",
    }
}

/// The description Prowlarr's Newznab and Torznab classes give.
fn nab_description(kind: &str) -> &'static str {
    if kind == "newznab" {
        "Newznab is an API search specification for Usenet"
    } else {
        "A Newznab-like api for torrents."
    }
}

/// Everything of an `IndexerResource` but its settings, for a saved indexer or a template.
struct Shape<'a> {
    id: i64,
    name: &'a str,
    kind: &'a str,
    definition_name: String,
    info_link_name: String,
    indexer_urls: Vec<String>,
    legacy_urls: Vec<String>,
    description: Option<String>,
    language: Option<String>,
    encoding: Option<String>,
    privacy: &'static str,
    caps: Capabilities,
}

/// The resource of `shape` with `fields`, `enable`, `redirect`, `priority`, app profile, download client, added
/// time and tags (`IndexerResourceMapper.ToResource`).
#[allow(clippy::too_many_arguments)]
fn resource_of(
    shape: Shape,
    fields: Vec<Value>,
    enable: bool,
    redirect: bool,
    priority: i64,
    app_profile_id: i64,
    download_client_id: i64,
    added: f64,
    tags: Vec<i64>,
) -> Value {
    let imp = implementation_of_kind(shape.kind);
    let cardigann = shape.kind == "cardigann";
    let mut v = json!({
        "name": shape.name,
        "fields": fields,
        "implementationName": imp.implementation_name,
        "implementation": imp.implementation,
        "configContract": imp.config_contract,
        "infoLink": format!("https://wiki.servarr.com/prowlarr/supported-indexers#{}", shape.info_link_name.to_lowercase().replace(' ', "-")),
        "tags": tags,
        "indexerUrls": shape.indexer_urls,
        "legacyUrls": shape.legacy_urls,
        "definitionName": shape.definition_name,
        "enable": enable,
        "redirect": redirect,
        "supportsRss": true,
        "supportsSearch": true,
        "supportsRedirect": !cardigann,
        "supportsPagination": !cardigann,
        "appProfileId": app_profile_id,
        "protocol": if shape.kind == "newznab" { "usenet" } else { "torrent" },
        "privacy": shape.privacy,
        "capabilities": capabilities_json(&shape.caps),
        "priority": priority,
        "downloadClientId": download_client_id,
        "added": if added > 0.0 { super::date(added) } else { "0001-01-01T00:00:00Z".to_string() },
        "sortName": normalize_title(shape.name),
    });
    if shape.id != 0 {
        v["id"] = json!(shape.id);
    }
    for (k, x) in [("description", shape.description), ("language", shape.language), ("encoding", shape.encoding)] {
        if let Some(x) = x {
            v[k] = json!(x);
        }
    }
    v
}

/// A saved indexer as Prowlarr's `IndexerResource`.
pub fn resource(st: &AppState, ix: &Indexer, lang: &str) -> Value {
    let imp = implementation_of_kind(&ix.kind);
    let mut fields = to_schema(imp, &standard_values(ix), lang);
    let shape = if ix.kind == "cardigann" {
        let def = st.indexers.library.get(&ix.definition);
        if let Some(d) = &def {
            fields.extend(cardigann_settings(d).iter().enumerate().map(|(i, s)| cardigann_field(s, i, &ix.name, Some(&ix.settings))));
        }
        Shape {
            id: ix.id,
            name: &ix.name,
            kind: &ix.kind,
            definition_name: ix.definition.clone(),
            info_link_name: ix.definition.clone(),
            indexer_urls: def.as_ref().map(|d| d.links.clone()).unwrap_or_default(),
            legacy_urls: def.as_ref().map(|d| d.legacylinks.clone()).unwrap_or_default(),
            description: def.as_ref().map(|d| d.description.clone()),
            language: def.as_ref().map(|d| d.language.clone()),
            encoding: def.as_ref().map(|d| encoding_name(&d.encoding)),
            privacy: def.as_ref().map(|d| privacy_name(&d.kind)).unwrap_or("private"),
            caps: st.indexers.capabilities(ix),
        }
    } else {
        Shape {
            id: ix.id,
            name: &ix.name,
            kind: &ix.kind,
            definition_name: imp.implementation_name.into(),
            info_link_name: imp.implementation_name.into(),
            indexer_urls: vec![if ix.caps_xml.is_empty() { String::new() } else { ix.base_url.clone() }],
            legacy_urls: Vec::new(),
            description: Some(nab_description(&ix.kind).into()),
            language: Some("en-US".into()),
            encoding: Some(encoding_name("utf-8")),
            privacy: "private",
            caps: st.indexers.capabilities(ix),
        }
    };
    resource_of(
        shape,
        fields,
        ix.enabled,
        ix.redirect,
        ix.priority,
        ix.app_profile_id,
        ix.download_client_id,
        ix.added,
        st.indexers.tag_ids(&ix.tags),
    )
}

/// A Cardigann definition as a template to fill in (`Cardigann.GetDefinition`).
fn cardigann_template(d: &CardigannDefinition, lang: &str) -> Value {
    let mut values = CARDIGANN_FIELDS.iter().map(|f| (f.name.to_string(), f.default.value())).collect::<Map<_, _>>();
    values.insert("definitionFile".into(), json!(d.id));
    let mut fields = to_schema(&IMPLEMENTATIONS[0], &values, lang);
    fields.extend(cardigann_settings(d).iter().enumerate().map(|(i, s)| cardigann_field(s, i, &d.name, None)));
    let caps = Capabilities::from_definition(d).unwrap_or_default();
    let shape = Shape {
        id: 0,
        name: &d.name,
        kind: "cardigann",
        definition_name: d.id.clone(),
        info_link_name: d.id.clone(),
        indexer_urls: d.links.clone(),
        legacy_urls: d.legacylinks.clone(),
        description: Some(d.description.clone()),
        language: Some(d.language.clone()),
        encoding: None,
        privacy: privacy_name(&d.kind),
        caps,
    };
    let mut v = resource_of(shape, fields, true, false, 25, 0, 0, 0.0, Vec::new());
    v["presets"] = json!([]);
    v
}

/// A Newznab or Torznab template of Prowlarr's (`DefaultDefinitions`): name, address, API path, categories,
/// description and language (Torznab's are its own).
struct NabTemplate {
    kind: &'static str,
    name: &'static str,
    url: &'static str,
    api_path: Option<&'static str>,
    categories: &'static [u32],
    description: Option<&'static str>,
    language: Option<&'static str>,
}

/// A Newznab template.
const fn usenet(name: &'static str, url: &'static str, categories: &'static [u32]) -> NabTemplate {
    NabTemplate { kind: "newznab", name, url, api_path: None, categories, description: None, language: None }
}

/// Every main category.
const ALL_CATS: &[u32] = &[1000, 2000, 3000, 4000, 5000, 6000, 7000, 8000];
/// Every main category but Other.
const NO_OTHER: &[u32] = &[1000, 2000, 3000, 4000, 5000, 6000, 7000];

/// Prowlarr's Newznab and Torznab templates, in its order.
const NAB_TEMPLATES: &[NabTemplate] = &[
    usenet("abNZB", "https://abnzb.com", &[2000, 3000, 4000, 5000, 6000, 7000, 8000]),
    usenet("altHUB", "https://api.althub.co.za", &[2000, 3000, 4000, 5000, 7000]),
    usenet("AnimeTosho (Usenet)", "https://feed.animetosho.org", &[2020, 5070]),
    usenet("DOGnzb", "https://api.dognzb.cr", ALL_CATS),
    usenet("DrunkenSlug", "https://drunkenslug.com", NO_OTHER),
    usenet("GingaDADDY", "https://www.gingadaddy.com", &[]),
    usenet("Miatrix", "https://www.miatrix.com", ALL_CATS),
    usenet("Newz69", "https://newz69.keagaming.com", ALL_CATS),
    usenet("NinjaCentral", "https://ninjacentral.co.za", ALL_CATS),
    usenet("Nzb.life", "https://api.nzb.life", ALL_CATS),
    usenet("NZBCat", "https://nzb.cat", NO_OTHER),
    usenet("NZBFinder", "https://nzbfinder.ws", &[2000, 3000, 5000, 6000, 7000]),
    usenet("NZBgeek", "https://api.nzbgeek.info", ALL_CATS),
    usenet("NzbNoob", "https://www.nzbnoob.com", &[2000, 3000, 4000, 5000, 6000, 7000, 8000]),
    usenet("NZBNDX", "https://www.nzbndx.com", ALL_CATS),
    usenet("NzbPlanet", "https://api.nzbplanet.net", ALL_CATS),
    usenet("NZBStars", "https://nzbstars.com", NO_OTHER),
    NabTemplate { api_path: Some("/api/v1/api"), ..usenet("Tabula Rasa", "https://www.tabula-rasa.pw", NO_OTHER) },
    usenet("Generic Newznab", "", &[]),
    NabTemplate {
        kind: "torznab",
        name: "AnimeTosho",
        url: "https://feed.animetosho.org",
        api_path: None,
        categories: &[2020, 5070],
        description: Some("Anime NZB/DDL mirror"),
        language: None,
    },
    NabTemplate {
        kind: "torznab",
        name: "Torrent Network",
        url: "https://tntracker.org",
        api_path: Some("/api/torznab/api"),
        categories: &[],
        description: Some("Torrent Network (TN) is a GERMAN Private site for TV / MOVIES / GENERAL"),
        language: Some("de-DE"),
    },
    NabTemplate {
        kind: "torznab",
        name: "Generic Torznab",
        url: "",
        api_path: None,
        categories: &[],
        description: Some("A Newznab-like api for torrents."),
        language: None,
    },
];

/// A Newznab/Torznab template as a resource (`Newznab.GetDefinition`, `Torznab.GetDefinition`).
fn nab_template(t: &NabTemplate, lang: &str) -> Value {
    let imp = implementation_of_kind(t.kind);
    let mut values: Map<String, Value> = imp.fields.iter().map(|f| (f.name.to_string(), f.default.value())).collect();
    values.insert("baseUrl".into(), json!(t.url));
    if let Some(p) = t.api_path {
        values.insert("apiPath".into(), json!(p));
    }
    let mut caps = Capabilities::default();
    for c in t.categories {
        if let Some(name) = crate::indexers::categories::name_of(*c) {
            caps.categories.add_mapping(&c.to_string(), &Category::new(*c, name), None);
        }
    }
    let shape = Shape {
        id: 0,
        name: t.name,
        kind: t.kind,
        definition_name: imp.implementation_name.into(),
        info_link_name: imp.implementation_name.into(),
        indexer_urls: vec![String::new()],
        legacy_urls: Vec::new(),
        description: Some(t.description.unwrap_or(nab_description(t.kind)).into()),
        language: Some(t.language.unwrap_or("en-US").into()),
        encoding: Some(encoding_name("utf-8")),
        privacy: "private",
        caps,
    };
    let mut v = resource_of(shape, to_schema(imp, &values, lang), true, t.kind == "newznab", 25, 0, 0, 0.0, Vec::new());
    v["presets"] = json!([]);
    v
}

// ---- from a request ----

/// An indexer as a request describes it (`IndexerResourceMapper.ToModel`), ready to check and save.
struct Model {
    /// The indexer it changes (0: a new one).
    id: i64,
    /// The indexer in Lantern's terms (settings opened; id 0 when new).
    ix: Indexer,
    /// The settings as Prowlarr's settings class holds them, for its checks.
    values: Map<String, Value>,
}

/// The text of a body property (`None` for null or missing).
fn text_prop(body: &Map<String, Value>, name: &str) -> Option<String> {
    prop(body, name).and_then(Value::as_str).map(str::to_string)
}

/// A whole number of a body property, `default` when missing.
fn int_prop(body: &Map<String, Value>, name: &str, default: i64) -> i64 {
    match prop(body, name) {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(default),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(default),
        _ => default,
    }
}

/// A yes-or-no body property, false when missing.
fn bool_prop(body: &Map<String, Value>, name: &str) -> bool {
    match prop(body, name) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// The resource's id in a body (0 when there is none).
fn id_of(body: &Map<String, Value>) -> i64 {
    int_prop(body, "id", 0)
}

/// A request's tag ids.
fn tags_of(body: &Map<String, Value>) -> Vec<i64> {
    let mut out = Vec::new();
    if let Some(Value::Array(a)) = prop(body, "tags") {
        for t in a.iter().filter_map(Value::as_i64) {
            if !out.contains(&t) {
                out.push(t);
            }
        }
    }
    out
}

/// The indexer a request body describes, the saved one (`existing`) giving the secrets it leaves masked.
fn to_model(st: &AppState, body: &Map<String, Value>, existing: Option<&Indexer>) -> Result<Model, ApiError> {
    let implementation = text_prop(body, "implementation").unwrap_or_default();
    let kind = kind_of_implementation(&implementation).ok_or_else(|| {
        fail(
            400,
            format!("Lantern can't add {implementation} indexers. Pick one from the schema: a Cardigann definition, Newznab or Torznab."),
        )
    })?;
    let imp = implementation_of_kind(kind);
    let fields: Vec<Value> = prop(body, "fields").and_then(Value::as_array).cloned().unwrap_or_default();
    let saved = existing.filter(|e| e.kind == kind).map(standard_values);
    let values = read_settings(imp, &fields, saved.as_ref())?;
    let mut settings: BTreeMap<String, String> = BTreeMap::new();
    let mut definition = String::new();
    let mut base_url = String::new();
    for (k, v) in &values {
        match k.as_str() {
            "definitionFile" => definition = text_of(v),
            "baseUrl" => base_url = text_of(v).trim().to_string(),
            name => {
                settings.insert(lantern_key(name).to_string(), text_of(v));
            }
        }
    }
    let ix_name = text_prop(body, "name").unwrap_or_default();
    if kind == "cardigann" && !definition.trim().is_empty() {
        let def = st.indexers.library.get(&definition);
        let standard: Vec<&str> = imp.fields.iter().map(|f| f.name).collect();
        for f in &fields {
            let Some(name) = f.get("name").and_then(Value::as_str) else { continue };
            if standard.contains(&name) {
                continue;
            }
            let value = f.get("value").cloned().unwrap_or(Value::Null);
            if name == "cardigannCaptcha" {
                settings.insert("CAPTCHA".into(), text_of(&value));
                continue;
            }
            let Some(d) = &def else { continue };
            let list = cardigann_settings(d);
            let s = list
                .iter()
                .find(|s| s.name == name)
                .ok_or_else(|| fail(500, format!("Unknown Cardigann setting. (Parameter '{name}')")))?;
            if s.is_info() {
                continue;
            }
            let old = existing.and_then(|e| e.settings.get(name));
            settings.insert(name.to_string(), setting_from_field(s, &value, old));
        }
    } else if kind != "cardigann" {
        // the preset a Newznab address belongs to, so its categories are known before the API answers
        if let Some(p) = crate::indexers::newznab::preset_at(&base_url) {
            definition = p.id.to_string();
            if kind == "newznab" {
                crate::indexers::fill_from_preset(&mut settings, p);
            }
        }
        if let Some(e) = existing.filter(|e| e.kind == kind) {
            if let Some(c) = e.settings.get("categories") {
                settings.entry("categories".into()).or_insert_with(|| c.clone());
            }
        }
    }
    let ix = Indexer {
        id: existing.map(|e| e.id).unwrap_or(0),
        kind: kind.to_string(),
        definition,
        name: ix_name,
        base_url,
        settings,
        enabled: bool_prop(body, "enable"),
        priority: int_prop(body, "priority", 25),
        tags: st.indexers.tag_labels(&tags_of(body)),
        redirect: bool_prop(body, "redirect"),
        app_profile_id: int_prop(body, "appProfileId", 0),
        download_client_id: int_prop(body, "downloadClientId", 0),
        added: existing.map(|e| e.added).unwrap_or(0.0),
        cookies: existing.map(|e| e.cookies.clone()).unwrap_or_default(),
        caps_xml: existing.map(|e| e.caps_xml.clone()).unwrap_or_default(),
        ..Indexer::default()
    };
    Ok(Model { id: id_of(body), ix, values })
}

/// The body Lantern's own indexer functions take for `ix` (with the tag ids of the request).
fn lantern_body(ix: &Indexer, tags: &[i64]) -> Value {
    json!({
        "kind": ix.kind,
        "definition": ix.definition,
        "name": ix.name,
        "base_url": ix.base_url,
        "settings": ix.settings,
        "enabled": ix.enabled,
        "priority": ix.priority,
        "tags": tags,
        "redirect": ix.redirect,
        "app_profile_id": ix.app_profile_id,
        "download_client_id": ix.download_client_id,
    })
}

// ---- checks ----

/// The checks every save runs on the resource (`SharedValidator` of `ProviderControllerBase` and
/// `IndexerController`): a name, unique; an implementation and settings class; an app profile that exists; a
/// priority from 1 to 50; a download client that exists (or none).
fn shared_checks(st: &AppState, body: &Map<String, Value>) -> Result<Vec<Option<validation::Failure>>, ApiError> {
    let name = text_prop(body, "name");
    let id = id_of(body);
    let mut out = vec![validation::not_empty("Name", name.as_deref())];
    if let Some(n) = name.as_deref() {
        if st.indexers.list()?.iter().any(|i| i.name.to_lowercase() == n.to_lowercase() && i.id != id) {
            out.push(Some(validation::Failure::new("Name", json!(n), "PredicateValidator", "Should be unique")));
        }
    }
    out.push(validation::not_empty("Implementation", text_prop(body, "implementation").as_deref()));
    out.push(validation::not_empty("ConfigContract", text_prop(body, "configContract").as_deref()));
    let profile = int_prop(body, "appProfileId", 0);
    match validation::greater_than("AppProfileId", profile, 0) {
        Some(f) => out.push(Some(f)),
        None => {
            if !super::apps::profiles::all(&st.db)?.iter().any(|p| p.id == profile) {
                out.push(Some(validation::Failure::new(
                    "AppProfileId",
                    json!(profile),
                    "AppProfileExistsValidator",
                    "App Profile does not exist",
                )));
            }
        }
    }
    out.push(validation::between("Priority", int_prop(body, "priority", 0), 1, 50));
    let client = int_prop(body, "downloadClientId", 0);
    if client != 0 && super::apps::provider::find::<super::apps::downloads::DownloadClients>(&st.db, client).ok().flatten().is_none() {
        out.push(Some(validation::Failure::new(
            "DownloadClientId",
            json!(client),
            "DownloadClientExistsValidator",
            "Download Client does not exist",
        )));
    }
    Ok(out)
}

/// A settings failure on `property` saying `message`.
fn failure(property: &str, message: &str, value: &Value, warning: bool) -> ProviderFailure {
    let mut f = ProviderFailure::new(property, message).with_value(value.clone());
    f.is_warning = warning;
    f
}

/// A setting's property name as Prowlarr's validator names it (`baseSettings.queryLimit` → `BaseSettings.QueryLimit`).
fn property_name(field: &str) -> String {
    field
        .split('.')
        .map(|p| {
            let mut c = p.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// .NET's `DateTime.TryParse` for the forms people type: a date, a date and time, or RFC 3339.
fn parse_date(s: &str) -> Option<chrono::NaiveDateTime> {
    let s = s.trim();
    if let Ok(d) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(d.naive_utc());
    }
    for f in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M", "%m/%d/%Y %H:%M:%S"] {
        if let Ok(d) = chrono::NaiveDateTime::parse_from_str(s, f) {
            return Some(d);
        }
    }
    for f in ["%Y-%m-%d", "%m/%d/%Y", "%Y/%m/%d", "%d %B %Y", "%B %d, %Y"] {
        if let Ok(d) = chrono::NaiveDate::parse_from_str(s, f) {
            return d.and_hms_opt(0, 0, 0);
        }
    }
    None
}

/// The checks of an indexer's settings (`CardigannSettingsValidator`, `NewznabSettingsValidator`,
/// `TorznabSettingsValidator`, with `IndexerCommonSettingsValidator` and `IndexerTorrentSettingsValidator`).
fn validate_settings(kind: &str, values: &Map<String, Value>) -> Vec<ProviderFailure> {
    let mut out = Vec::new();
    let get = |k: &str| values.get(k).cloned().unwrap_or(Value::Null);
    let text = |k: &str| values.get(k).and_then(Value::as_str).map(str::to_string);
    if kind == "cardigann" && text("definitionFile").is_none_or(|d| d.trim().is_empty()) {
        out.push(failure("DefinitionFile", "'Definition File' must not be empty.", &get("definitionFile"), false));
    }
    let lantern: BTreeMap<String, String> = values.iter().map(|(k, v)| (k.clone(), text_of(v))).collect();
    let (errors, warnings) = limits::validate(&lantern);
    let torrent = kind != "newznab";
    for (list, warning) in [(errors, false), (warnings, true)] {
        for (k, m) in list {
            if !torrent && k.starts_with("torrentBaseSettings.") {
                continue;
            }
            out.push(failure(&property_name(k), m, &get(k), warning));
        }
    }
    if kind == "cardigann" {
        return out;
    }
    // ValidRootUrl
    let url = text("baseUrl").unwrap_or_default();
    if url.trim().is_empty() {
        out.push(failure("BaseUrl", "'Base Url' must not be empty.", &get("baseUrl"), false));
    }
    if !(super::apps::provider::is_valid_url(&url) && url.to_lowercase().starts_with("http")) {
        out.push(failure("BaseUrl", "must be valid URL that starts with http(s)://", &get("baseUrl"), false));
    }
    // ValidUrlBase("/api")
    if let Some(p) = text("apiPath") {
        let l = p.to_lowercase();
        let rest = l.strip_prefix('/').unwrap_or(&l);
        let host_like = ["http://", "https://"].iter().any(|s| {
            rest.strip_prefix(s).is_some_and(|h| h.chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)))
        });
        if host_like {
            out.push(failure("ApiPath", "Must be a valid URL path (ie: '/api')", &get("apiPath"), false));
        }
    }
    // the API key of the sites that need one
    const API_KEY_ALLOW_LIST: [&str; 7] = ["nzbs.org", "nzb.life", "dognzb.cr", "nzbplanet.net", "nzbid.org", "nzbndx.com", "nzbindex.in"];
    if kind == "newznab" && API_KEY_ALLOW_LIST.iter().any(|h| url.to_lowercase().contains(h)) && text("apiKey").is_none_or(|k| k.is_empty())
    {
        out.push(failure("ApiKey", "'Api Key' must not be empty.", &get("apiKey"), false));
    }
    if let Some(p) = text("additionalParameters").filter(|p| !p.trim().is_empty()) {
        let re = regex::Regex::new(r"(&.+?=.+?)+").expect("a fixed pattern");
        if !re.is_match(&p) {
            out.push(failure(
                "AdditionalParameters",
                "'Additional Parameters' is not in the correct format.",
                &get("additionalParameters"),
                false,
            ));
        }
    }
    if kind == "newznab" {
        if let Some(v) = text("vipExpiration").filter(|v| !v.trim().is_empty()) {
            match parse_date(&v) {
                None => out.push(failure("VipExpiration", "Correctly formatted date is required", &get("vipExpiration"), false)),
                Some(d) if d <= chrono::Local::now().naive_local() => {
                    out.push(failure("VipExpiration", "Must be a future date", &get("vipExpiration"), true))
                }
                Some(_) => {}
            }
        }
    }
    out
}

/// What an indexer route answers when it can't do what was asked: an API error, or failed settings checks or a
/// failed test (a 400 with Prowlarr's list).
enum Problem {
    /// An error as the rest of the API answers it.
    Api(ApiError),
    /// The settings or the test failed.
    Failures(Vec<ProviderFailure>),
}

impl From<ApiError> for Problem {
    fn from(e: ApiError) -> Problem {
        Problem::Api(e)
    }
}

impl From<kit_server_core::Error> for Problem {
    fn from(e: kit_server_core::Error) -> Problem {
        Problem::Api(ApiError::Kit(e))
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        match self {
            Problem::Api(e) => e.into_response(),
            Problem::Failures(list) => {
                (StatusCode::BAD_REQUEST, Json(Value::Array(list.iter().map(ProviderFailure::json).collect()))).into_response()
            }
        }
    }
}

/// What an indexer route returns.
type Answer = Result<Response, Problem>;

/// Prowlarr's `VerifyValidationResult`: the failures as an error when one is not a warning (or, with
/// `include_warnings`, when there are any).
fn verify(list: Vec<ProviderFailure>, include_warnings: bool) -> Result<(), Problem> {
    let errors = list.iter().any(|f| !f.is_warning);
    if errors || (include_warnings && !list.is_empty()) {
        let list = if include_warnings { list } else { list.into_iter().filter(|f| !f.is_warning).collect() };
        return Err(Problem::Failures(list));
    }
    Ok(())
}

/// `IndexerController.Validate`: a Usenet indexer that can send apps to the site must have Redirect on, then the
/// settings' checks.
fn validate(m: &Model, include_warnings: bool) -> Result<(), Problem> {
    if m.ix.kind == "newznab" && !m.ix.redirect {
        return Err(Problem::Failures(vec![ProviderFailure::new("Redirect", "Redirect must be enabled for Usenet indexers")]));
    }
    verify(validate_settings(&m.ix.kind, &m.values), include_warnings)
}

/// Tests an indexer as Prowlarr's `TestConnection` does (a search for the newest releases must give some), and
/// for a saved one notes the outcome in its health (`IndexerFactory.Test`).
async fn run_test(st: &AppState, ix: &Indexer) -> Vec<ProviderFailure> {
    let r = st.indexers.test_indexer(ix).await;
    let out = match &r {
        Ok(0) => vec![ProviderFailure::new(
            "",
            "Query successful, but no results were returned from your indexer. This may be an issue with the indexer, your indexer category settings, or other indexer settings such as search freeleech only etc. See the FAQ for details.",
        )],
        Ok(_) => Vec::new(),
        Err(e) => vec![ProviderFailure::new("", format!("Unable to connect to indexer. {}", e.detail))],
    };
    if ix.id > 0 {
        if out.is_empty() {
            st.indexers.record_success(ix.id);
        } else if let Err(e) = &r {
            st.indexers.record_failure(ix.id, &e.detail);
        }
    }
    out
}

// ---- routes ----

/// `?forceSave=true` and the like.
fn flag(q: &HashMap<String, String>, name: &str) -> bool {
    q.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).is_some_and(|(_, v)| v.eq_ignore_ascii_case("true"))
}

/// A request body as a JSON object.
fn object(bytes: &[u8]) -> Result<Map<String, Value>, ApiError> {
    match body_json(bytes)? {
        Value::Object(m) => Ok(m),
        _ => Err(fail(400, "The request body must be a JSON object.")),
    }
}

/// An answer with a status, a JSON body and, when given, the resource's address.
fn answer(status: StatusCode, body: Value, location: Option<String>) -> Response {
    let mut r = (status, Json(body)).into_response();
    if let Some(l) = location.and_then(|l| HeaderValue::from_str(&l).ok()) {
        r.headers_mut().insert(header::LOCATION, l);
    }
    r
}

/// Prowlarr's 404 for an indexer that isn't there.
fn missing(id: i64) -> ApiError {
    super::not_found("IndexerDefinition", id)
}

/// Indexer `id`, or Prowlarr's 404.
fn find(st: &AppState, id: i64) -> Result<Indexer, ApiError> {
    st.indexers.get(id).map_err(|_| missing(id))
}

/// `GET /api/v1/indexer`: every indexer, by name.
async fn get_all(State(st): State<AppState>, _: V1) -> Answer {
    let lang = config::ui_language(&st.db);
    let mut list = st.indexers.list()?;
    list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.name.cmp(&b.name)));
    Ok(Json(Value::Array(list.iter().map(|i| resource(&st, i, &lang)).collect())).into_response())
}

/// `GET /api/v1/indexer/{id}`.
async fn get_one(State(st): State<AppState>, _: V1, Path(id): Path<i64>) -> Answer {
    let ix = find(&st, id)?;
    if ix.kind == "cardigann" && st.indexers.library.get(&ix.definition).is_none() {
        return Err(missing(id).into());
    }
    Ok(Json(resource(&st, &ix, &config::ui_language(&st.db))).into_response())
}

/// `POST /api/v1/indexer[?forceSave=true]`: checks the resource and, when it is on, its settings and a test
/// (`forceSave` only lets warnings through), then saves it (a Newznab/Torznab API is asked for its capabilities).
/// 201 with the indexer.
async fn create(State(st): State<AppState>, _: V1, Query(q): Query<HashMap<String, String>>, bytes: Bytes) -> Answer {
    let body = object(&bytes)?;
    let mut checks = shared_checks(&st, &body)?;
    checks.push(validation::not_null("Fields", prop(&body, "fields").unwrap_or(&Value::Null)));
    check(checks)?;
    let force = flag(&q, "forceSave");
    let mut m = to_model(&st, &body, None)?;
    m.ix.id = 0;
    if m.ix.enabled {
        validate(&m, !force)?;
        verify(run_test(&st, &m.ix).await, !force)?;
    }
    let id = st.indexers.add(&lantern_body(&m.ix, &tags_of(&body)))?;
    let ix = st.indexers.get(id)?;
    if ix.enabled {
        st.indexers.refresh_caps_of(std::slice::from_ref(&ix)).await;
    }
    let out = resource(&st, &st.indexers.get(id)?, &config::ui_language(&st.db));
    super::signalr::broadcast(&st, "indexer", "updated", Some(&out));
    Ok(answer(StatusCode::CREATED, out, Some(format!("/api/v1/indexer/{id}"))))
}

/// `PUT /api/v1/indexer/{id}[?forceSave=true]`: checks and (when it is on and not forced) tests the changed indexer,
/// then saves it. 202 with the indexer.
async fn update(
    State(st): State<AppState>,
    _: V1,
    Path(path_id): Path<i64>,
    Query(q): Query<HashMap<String, String>>,
    bytes: Bytes,
) -> Answer {
    let mut body = object(&bytes)?;
    if id_of(&body) == 0 {
        body.retain(|k, _| !k.eq_ignore_ascii_case("id"));
        body.insert("id".into(), json!(path_id));
    }
    let mut checks = shared_checks(&st, &body)?;
    checks.push(validation::valid_id(id_of(&body)));
    check(checks)?;
    validate_id(path_id)?;
    let force = flag(&q, "forceSave");
    let id = id_of(&body);
    let existing = st.indexers.get(id).ok();
    let m = to_model(&st, &body, existing.as_ref())?;
    if m.ix.enabled {
        validate(&m, !force)?;
        if !force {
            verify(run_test(&st, &m.ix).await, true)?;
        }
    }
    let Some(existing) = existing else { return Err(missing(m.id).into()) };
    if existing.kind != m.ix.kind || (m.ix.kind == "cardigann" && existing.definition != m.ix.definition) {
        return Err(fail(400, "An indexer can't change its kind or definition. Add a new one instead.").into());
    }
    st.indexers.update(id, &lantern_body(&m.ix, &tags_of(&body)))?;
    let ix = st.indexers.get(id)?;
    if ix.enabled {
        st.indexers.refresh_caps_of(std::slice::from_ref(&ix)).await;
    }
    let out = resource(&st, &st.indexers.get(id)?, &config::ui_language(&st.db));
    super::signalr::broadcast(&st, "indexer", "updated", Some(&out));
    Ok(answer(StatusCode::ACCEPTED, out, Some(format!("/api/v1/indexer/{id}"))))
}

/// Removes indexer `id` and its history (nothing to do when it isn't there, as in Prowlarr).
fn delete_one(st: &AppState, id: i64) -> Result<(), ApiError> {
    if st.indexers.get(id).is_ok() {
        st.indexers.remove(id)?;
        super::signalr::broadcast(st, "indexer", "deleted", Some(&json!({ "id": id })));
    }
    Ok(())
}

/// `DELETE /api/v1/indexer/{id}`: `{}`.
async fn remove(State(st): State<AppState>, _: V1, Path(id): Path<i64>) -> Answer {
    validate_id(id)?;
    delete_one(&st, id)?;
    Ok(Json(json!({})).into_response())
}

/// The ids of a bulk request.
fn bulk_ids(body: &Map<String, Value>) -> Result<Vec<i64>, ApiError> {
    match prop(body, "ids") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => a.iter().map(|v| v.as_i64().ok_or_else(|| fail(400, "Each id must be a number."))).collect(),
        Some(_) => Err(fail(400, "The ids must be a list of numbers.")),
    }
}

/// A number of a bulk request, `None` when it isn't given.
fn opt_num(body: &Map<String, Value>, name: &str) -> Option<Value> {
    prop(body, name).filter(|v| !v.is_null()).cloned()
}

/// `PUT /api/v1/indexer/bulk` `{"ids", "tags", "applyTags", "enable", "appProfileId", "priority",
/// "minimumSeeders", "seedRatio", "seedTime", "packSeedTime", "preferMagnetUrl"}`: changes several indexers at once
/// (the seeding settings only on torrent ones; `IndexerBulkResourceMapper`). 202 with them.
async fn bulk_update(State(st): State<AppState>, _: V1, bytes: Bytes) -> Answer {
    let body = object(&bytes)?;
    let ids = bulk_ids(&body)?;
    if ids.is_empty() {
        return Err(api_exception(400, Some("ids must be provided")).into());
    }
    let list: Vec<Indexer> = st.indexers.list()?.into_iter().filter(|i| ids.contains(&i.id)).collect();
    if list.len() != ids.len() {
        return Err(fail(500, format!("Expected query to return {} rows but returned {}", ids.len(), list.len())).into());
    }
    let tags = match prop(&body, "tags") {
        None | Some(Value::Null) => None,
        Some(_) => Some(tags_of(&body)),
    };
    let apply = prop(&body, "applyTags").map(text_of).unwrap_or_else(|| "add".into()).to_lowercase();
    for ix in &list {
        let mut tag_ids = st.indexers.tag_ids(&ix.tags);
        if let Some(t) = &tags {
            match apply.as_str() {
                "remove" | "1" => tag_ids.retain(|x| !t.contains(x)),
                "replace" | "2" => tag_ids = t.clone(),
                _ => {
                    for x in t {
                        if !tag_ids.contains(x) {
                            tag_ids.push(*x);
                        }
                    }
                }
            }
        }
        let mut change = json!({ "tags": tag_ids });
        if let Some(Value::Bool(b)) = opt_num(&body, "enable") {
            change["enabled"] = json!(b);
        }
        if let Some(p) = opt_num(&body, "appProfileId").and_then(|v| v.as_i64()) {
            change["app_profile_id"] = json!(p);
        }
        if let Some(p) = opt_num(&body, "priority").and_then(|v| v.as_i64()) {
            change["priority"] = json!(p);
        }
        if ix.protocol() == Protocol::Torrent {
            let mut s = Map::new();
            for (field, key) in [
                ("minimumSeeders", limits::APP_MINIMUM_SEEDERS),
                ("seedRatio", limits::SEED_RATIO),
                ("seedTime", limits::SEED_TIME),
                ("packSeedTime", limits::PACK_SEED_TIME),
                ("preferMagnetUrl", limits::PREFER_MAGNET_URL),
            ] {
                if let Some(v) = opt_num(&body, field) {
                    s.insert(key.into(), json!(text_of(&v)));
                }
            }
            if !s.is_empty() {
                change["settings"] = Value::Object(s);
            }
        }
        st.indexers.update(ix.id, &change)?;
    }
    let lang = config::ui_language(&st.db);
    let out: Vec<Value> = ids.iter().filter_map(|id| st.indexers.get(*id).ok()).map(|i| resource(&st, &i, &lang)).collect();
    for r in &out {
        super::signalr::broadcast(&st, "indexer", "updated", Some(r));
    }
    Ok(answer(StatusCode::ACCEPTED, Value::Array(out), None))
}

/// `DELETE /api/v1/indexer/bulk` `{"ids": [...]}`: `{}`.
async fn bulk_delete(State(st): State<AppState>, _: V1, bytes: Bytes) -> Answer {
    for id in bulk_ids(&object(&bytes)?)? {
        delete_one(&st, id)?;
    }
    Ok(Json(json!({})).into_response())
}

/// `GET /api/v1/indexer/schema`: every Cardigann definition, then Prowlarr's Newznab and Torznab templates (by
/// implementation name), each ready to fill in.
async fn schema(State(st): State<AppState>, _: V1) -> Answer {
    let lang = config::ui_language(&st.db);
    let mut list: Vec<Value> = st.indexers.library.all().iter().map(|d| cardigann_template(d, &lang)).collect();
    list.extend(NAB_TEMPLATES.iter().map(|t| nab_template(t, &lang)));
    Ok(Json(Value::Array(list)).into_response())
}

/// `POST /api/v1/indexer/test[?forceTest=true]`: checks the resource and its settings, then tests it without
/// saving. `{}`.
async fn test(State(st): State<AppState>, _: V1, Query(q): Query<HashMap<String, String>>, bytes: Bytes) -> Answer {
    let body = object(&bytes)?;
    check(shared_checks(&st, &body)?)?;
    let force = flag(&q, "forceTest");
    let existing = if id_of(&body) > 0 { st.indexers.get(id_of(&body)).ok() } else { None };
    let m = to_model(&st, &body, existing.as_ref())?;
    validate(&m, !force)?;
    verify(run_test(&st, &m.ix).await, true)?;
    Ok(Json(json!({})).into_response())
}

/// `POST /api/v1/indexer/testall`: tests every enabled indexer whose settings pass their checks;
/// `[{"id", "isValid", "validationFailures"}]`, a 400 when one failed.
async fn test_all(State(st): State<AppState>, _: V1) -> Answer {
    let mut out = Vec::new();
    let mut any_failed = false;
    for ix in st.indexers.list()?.into_iter().filter(|i| i.enabled) {
        let values: Map<String, Value> = standard_values(&ix);
        if validate_settings(&ix.kind, &values).iter().any(|f| !f.is_warning) {
            continue;
        }
        let failures: Vec<ProviderFailure> = run_test(&st, &ix).await.into_iter().filter(|f| !f.is_warning).collect();
        any_failed |= !failures.is_empty();
        out.push(json!({
            "id": ix.id,
            "isValid": failures.is_empty(),
            "validationFailures": failures.iter().map(ProviderFailure::plain_json).collect::<Vec<_>>(),
        }));
    }
    let status = if any_failed { StatusCode::BAD_REQUEST } else { StatusCode::OK };
    Ok((status, Json(Value::Array(out))).into_response())
}

/// `POST /api/v1/indexer/action/{name}`: `getUrls` gives the addresses to pick from (a Cardigann site's from the
/// resource, a Newznab/Torznab API's own address once it has answered); other actions answer `null`.
async fn action(State(st): State<AppState>, _: V1, Path(name): Path<String>, bytes: Bytes) -> Answer {
    let body = object(&bytes)?;
    if name != "getUrls" {
        return Ok(Json(Value::Null).into_response());
    }
    let implementation = text_prop(&body, "implementation").unwrap_or_default();
    let urls: Vec<String> = if kind_of_implementation(&implementation) == Some("cardigann") {
        prop(&body, "indexerUrls").and_then(Value::as_array).map(|a| a.iter().map(text_of).collect()).unwrap_or_default()
    } else {
        let existing = if id_of(&body) > 0 { st.indexers.get(id_of(&body)).ok() } else { None };
        vec![existing.filter(|e| !e.caps_xml.is_empty()).map(|e| e.base_url).unwrap_or_default()]
    };
    Ok(Json(json!({ "options": urls.iter().map(|u| json!({ "value": u, "name": u })).collect::<Vec<_>>() })).into_response())
}

/// `GET /api/v1/indexer/categories`: the standard top categories with their sub-categories
/// (`NewznabStandardCategory.ParentCats`).
async fn default_categories(_: V1) -> Json<Value> {
    let list: Vec<Value> = PARENTS
        .iter()
        .map(|(id, name, subs)| {
            json!({ "id": id, "name": name, "subCategories": subs.iter().map(|(s, n)| json!({ "id": s, "name": n, "subCategories": [] })).collect::<Vec<_>>() })
        })
        .collect();
    Json(Value::Array(list))
}

/// `GET /api/v1/indexerstatus`: the indexers on hold now (`GetBlockedProviders`), as `IndexerStatusResource`.
async fn statuses(State(st): State<AppState>, _: V1) -> Answer {
    let now = crate::auth::now();
    let list: Vec<Value> = st
        .indexers
        .list()?
        .iter()
        .filter(|i| i.status().is_disabled(now))
        .map(|i| {
            let s = i.status();
            let mut v = json!({ "indexerId": i.id });
            for (k, t) in
                [("disabledTill", s.disabled_till), ("mostRecentFailure", s.most_recent_failure), ("initialFailure", s.initial_failure)]
            {
                if let Some(t) = t {
                    v[k] = json!(super::date(t));
                }
            }
            v
        })
        .collect();
    Ok(Json(Value::Array(list)).into_response())
}

/// A time from a query (`2026-11-05`, `2026-11-05T10:30:00Z`) as seconds since 1970.
pub fn query_time(s: &str) -> Option<f64> {
    parse_date(s).map(|d| d.and_utc().timestamp() as f64)
}

/// One history event as the statistics read it.
struct Event {
    indexer_id: i64,
    kind: String,
    ok: bool,
    ms: i64,
    source: String,
    host: String,
}

/// Prowlarr's `CalculateAverageElapsedTime`: the average of the answer times above 0 (none: 0).
fn average(events: &[&Event]) -> i64 {
    let times: Vec<i64> = events.iter().map(|e| e.ms).filter(|m| *m > 0).collect();
    if times.is_empty() {
        0
    } else {
        times.iter().sum::<i64>() / times.len() as i64
    }
}

/// The statistics of `events` (`IndexerStatisticsService.IndexerStatistics`): per indexer, per user agent, per host.
fn statistics(events: &[Event], names: &HashMap<i64, String>) -> Value {
    let mut order: Vec<i64> = Vec::new();
    for e in events {
        if !order.contains(&e.indexer_id) {
            order.push(e.indexer_id);
        }
    }
    let mut indexers = Vec::new();
    for id in order {
        let Some(name) = names.get(&id) else { continue };
        let mine: Vec<&Event> = events.iter().filter(|e| e.indexer_id == id).collect();
        let count = |kind: &str, failed: bool| mine.iter().filter(|e| e.kind == kind && (!failed || !e.ok)).count();
        let queries: Vec<&Event> = mine.iter().copied().filter(|e| e.kind == "query" || e.kind == "rss").collect();
        let grabs: Vec<&Event> = mine.iter().copied().filter(|e| e.kind == "grab").collect();
        indexers.push(json!({
            "indexerId": id,
            "indexerName": name,
            "averageResponseTime": average(&queries),
            "averageGrabResponseTime": average(&grabs),
            "numberOfQueries": count("query", false),
            "numberOfGrabs": count("grab", false),
            "numberOfRssQueries": count("rss", false),
            "numberOfAuthQueries": count("auth", false),
            "numberOfFailedQueries": count("query", true),
            "numberOfFailedGrabs": count("grab", true),
            "numberOfFailedRssQueries": count("rss", true),
            "numberOfFailedAuthQueries": count("auth", true),
        }));
    }
    let grouped = |key: fn(&Event) -> &str, label: &str| {
        let mut keys: Vec<String> = Vec::new();
        for e in events.iter().filter(|e| e.kind != "auth") {
            if !keys.iter().any(|k| k == key(e)) {
                keys.push(key(e).to_string());
            }
        }
        keys.iter()
            .map(|k| {
                let mine: Vec<&Event> = events.iter().filter(|e| e.kind != "auth" && key(e) == k).collect();
                json!({
                    label: k,
                    "numberOfQueries": mine.iter().filter(|e| e.kind == "query" || e.kind == "rss").count(),
                    "numberOfGrabs": mine.iter().filter(|e| e.kind == "grab").count(),
                })
            })
            .collect::<Vec<_>>()
    };
    json!({
        "indexers": indexers,
        "userAgents": grouped(|e| &e.source, "userAgent"),
        "hosts": grouped(|e| &e.host, "host"),
    })
}

/// `GET /api/v1/indexerstats?startDate=&endDate=&indexers=&protocols=&tags=`: queries, grabs and answer times per
/// indexer, user agent and host, from the history between the two times (`IndexerStatsController`).
async fn stats(State(st): State<AppState>, _: V1, Query(q): Query<HashMap<String, String>>) -> Answer {
    let get = |k: &str| q.iter().find(|(x, _)| x.eq_ignore_ascii_case(k)).map(|(_, v)| v.trim().to_string()).filter(|v| !v.is_empty());
    let start = get("startDate").and_then(|s| query_time(&s)).unwrap_or(f64::MIN);
    let end = get("endDate").and_then(|s| query_time(&s)).unwrap_or_else(crate::auth::now);
    let mut all = st.indexers.list()?;
    if let Some(p) = get("protocols") {
        let wanted: Vec<String> = p.split(',').map(|x| x.trim().to_lowercase()).collect();
        all.retain(|i| wanted.iter().any(|w| *w == if i.protocol() == Protocol::Usenet { "usenet" } else { "torrent" }));
    }
    let mut ids: Vec<i64> = all.iter().map(|i| i.id).collect();
    if let Some(t) = get("tags") {
        let labels: Vec<String> = t.split(',').map(|x| x.trim().to_lowercase()).collect();
        let tagged: Vec<i64> =
            all.iter().filter(|i| super::tags::labels_of(&i.tags).iter().any(|l| labels.contains(l))).map(|i| i.id).collect();
        ids.retain(|i| tagged.contains(i));
    }
    if let Some(list) = get("indexers") {
        let wanted: Vec<i64> = list.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        ids.retain(|i| wanted.contains(i));
    }
    let names: HashMap<i64, String> = all.iter().map(|i| (i.id, i.name.clone())).collect();
    let events: Vec<Event> = super::history::between(&st.db, start, end)?
        .into_iter()
        .filter(|e| ids.contains(&e.indexer_id))
        .map(|e| Event {
            indexer_id: e.indexer_id,
            ok: e.ok,
            ms: if e.data.get("Cached").and_then(Value::as_str) == Some("1") { 0 } else { e.ms },
            source: e.data.get("Source").and_then(Value::as_str).unwrap_or("").to_string(),
            host: e.data.get("Host").and_then(Value::as_str).unwrap_or("").to_string(),
            kind: e.kind,
        })
        .collect();
    Ok(Json(statistics(&events, &names)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_titles_like_prowlarr() {
        // StringUtilFixture-style cases for NormalizeTitle
        assert_eq!(normalize_title("The.Show.Name.S01E02"), "show name s01e02");
        assert_eq!(normalize_title("Lantern Test Tracker"), "lantern test tracker");
        assert_eq!(normalize_title("AnimeTosho (Usenet)"), "animetosho usenet");
        assert_eq!(normalize_title("Nzb.life"), "nzb life");
        assert_eq!(normalize_title("Show & Tell: A Story"), "show tell story");
    }

    #[test]
    fn settings_classes_have_prowlarrs_field_order() {
        let names = |f: &[FieldSpec]| f.iter().map(|x| x.name).collect::<Vec<_>>();
        assert_eq!(
            names(CARDIGANN_FIELDS),
            [
                "definitionFile",
                "baseUrl",
                "baseSettings.queryLimit",
                "baseSettings.grabLimit",
                "baseSettings.limitsUnit",
                "torrentBaseSettings.appMinimumSeeders",
                "torrentBaseSettings.seedRatio",
                "torrentBaseSettings.seedTime",
                "torrentBaseSettings.packSeedTime",
                "torrentBaseSettings.preferMagnetUrl",
            ]
        );
        assert_eq!(
            names(NEWZNAB_FIELDS),
            [
                "baseUrl",
                "apiPath",
                "apiKey",
                "additionalParameters",
                "vipExpiration",
                "baseSettings.queryLimit",
                "baseSettings.grabLimit",
                "baseSettings.limitsUnit"
            ]
        );
        assert_eq!(TORZNAB_FIELDS.len(), 13);
        assert_eq!(TORZNAB_FIELDS[3].name, "torrentBaseSettings.appMinimumSeeders");
        assert_eq!(TORZNAB_FIELDS[8].name, "additionalParameters");
        assert!(TORZNAB_FIELDS.iter().enumerate().all(|(i, f)| f.order == i as i64));
    }

    #[test]
    fn newznab_settings_are_checked_like_prowlarr() {
        let v = |pairs: &[(&str, Value)]| {
            let mut m: Map<String, Value> = NEWZNAB_FIELDS.iter().map(|f| (f.name.to_string(), f.default.value())).collect();
            for (k, x) in pairs {
                m.insert(k.to_string(), x.clone());
            }
            m
        };
        let msgs = |m: &Map<String, Value>| {
            validate_settings("newznab", m).iter().map(|f| (f.property.clone(), f.message.clone())).collect::<Vec<_>>()
        };
        assert_eq!(
            msgs(&v(&[])),
            [
                ("BaseUrl".to_string(), "'Base Url' must not be empty.".to_string()),
                ("BaseUrl".to_string(), "must be valid URL that starts with http(s)://".to_string())
            ]
        );
        assert!(msgs(&v(&[("baseUrl", json!("https://api.nzbgeek.info"))])).is_empty());
        assert_eq!(
            msgs(&v(&[("baseUrl", json!("https://api.dognzb.cr"))])),
            [("ApiKey".to_string(), "'Api Key' must not be empty.".to_string())]
        );
        assert_eq!(
            msgs(&v(&[("baseUrl", json!("https://x.org")), ("apiPath", json!("https://x.org/api"))])),
            [("ApiPath".to_string(), "Must be a valid URL path (ie: '/api')".to_string())]
        );
        assert_eq!(
            msgs(&v(&[("baseUrl", json!("https://x.org")), ("additionalParameters", json!("dl=1"))])),
            [("AdditionalParameters".to_string(), "'Additional Parameters' is not in the correct format.".to_string())]
        );
        assert!(msgs(&v(&[("baseUrl", json!("https://x.org")), ("additionalParameters", json!("&dl=1"))])).is_empty());
        assert_eq!(
            msgs(&v(&[("baseUrl", json!("https://x.org")), ("vipExpiration", json!("soon"))])),
            [("VipExpiration".to_string(), "Correctly formatted date is required".to_string())]
        );
        let past = validate_settings("newznab", &v(&[("baseUrl", json!("https://x.org")), ("vipExpiration", json!("2001-01-01"))]));
        assert_eq!((past[0].message.as_str(), past[0].is_warning), ("Must be a future date", true));
        let limits = validate_settings("newznab", &v(&[("baseUrl", json!("https://x.org")), ("baseSettings.queryLimit", json!(0))]));
        assert_eq!((limits[0].property.as_str(), limits[0].message.as_str()), ("BaseSettings.QueryLimit", "Should be greater than zero"));
        // Cardigann needs its definition
        let c = validate_settings("cardigann", &Map::new());
        assert_eq!(c[0].message, "'Definition File' must not be empty.");
    }

    #[test]
    fn select_settings_travel_as_their_place() {
        let s = Setting {
            name: "sort".into(),
            kind: "select".into(),
            label: "Sort".into(),
            default: Some("added".into()),
            options: [("seeders".to_string(), "seeders".to_string()), ("added".to_string(), "created".to_string())].into_iter().collect(),
            ..Setting::default()
        };
        let f = cardigann_field(&s, 3, "X", None);
        assert_eq!(f["value"], 0, "added sorts before seeders");
        assert_eq!(
            f["selectOptions"],
            json!([{ "value": 0, "name": "created", "order": 0 }, { "value": 1, "name": "seeders", "order": 0 }])
        );
        assert_eq!(setting_from_field(&s, &json!(1), None), "seeders");
        assert_eq!(setting_from_field(&s, &json!("0"), None), "added");
        let saved: BTreeMap<String, String> = [("sort".to_string(), "seeders".to_string())].into_iter().collect();
        assert_eq!(cardigann_field(&s, 3, "X", Some(&saved))["value"], 1);
        let c = Setting { name: "freeleech".into(), kind: "checkbox".into(), default: Some("false".into()), ..Setting::default() };
        assert_eq!(cardigann_field(&c, 0, "X", None)["value"], false);
        assert_eq!(setting_from_field(&c, &json!(true), None), "true");
        let p = Setting { name: "password".into(), kind: "password".into(), ..Setting::default() };
        let saved: BTreeMap<String, String> = [("password".to_string(), "pw".to_string())].into_iter().collect();
        assert_eq!(cardigann_field(&p, 0, "X", Some(&saved))["value"], MASK);
        assert_eq!(setting_from_field(&p, &json!(MASK), Some(&"pw".to_string())), "pw");
        let i = Setting { name: "info".into(), kind: "info_category_8000".into(), ..Setting::default() };
        let f = cardigann_field(&i, 0, "Site", None);
        assert_eq!(f["type"], "info");
        assert_eq!(f["label"], "About Site Categories");
    }

    #[test]
    fn statistics_count_like_prowlarr() {
        let e = |id: i64, kind: &str, ok: bool, ms: i64, source: &str| Event {
            indexer_id: id,
            kind: kind.into(),
            ok,
            ms,
            source: source.into(),
            host: "h".into(),
        };
        let events = vec![
            e(1, "query", true, 100, "Sonarr"),
            e(1, "query", false, 300, "Sonarr"),
            e(1, "rss", true, 0, "Radarr"),
            e(1, "grab", true, 50, "Sonarr"),
            e(1, "auth", false, 10, ""),
            e(2, "query", true, 10, "Radarr"),
            e(9, "query", true, 10, "Radarr"),
        ];
        let names: HashMap<i64, String> = [(1, "One".to_string()), (2, "Two".to_string())].into_iter().collect();
        let s = statistics(&events, &names);
        assert_eq!(s["indexers"].as_array().unwrap().len(), 2, "an indexer that is gone is left out");
        let one = &s["indexers"][0];
        assert_eq!(one["averageResponseTime"], 200);
        assert_eq!(one["averageGrabResponseTime"], 50);
        assert_eq!((one["numberOfQueries"].clone(), one["numberOfFailedQueries"].clone()), (json!(2), json!(1)));
        assert_eq!((one["numberOfAuthQueries"].clone(), one["numberOfFailedAuthQueries"].clone()), (json!(1), json!(1)));
        assert_eq!(one["numberOfRssQueries"], 1);
        assert_eq!(s["userAgents"][0], json!({ "userAgent": "Sonarr", "numberOfQueries": 2, "numberOfGrabs": 1 }));
        assert_eq!(s["userAgents"][1], json!({ "userAgent": "Radarr", "numberOfQueries": 3, "numberOfGrabs": 0 }));
        assert_eq!(s["hosts"], json!([{ "host": "h", "numberOfQueries": 5, "numberOfGrabs": 1 }]));
    }
}
