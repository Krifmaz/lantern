// Ported from Prowlarr v2.7.0.5723 (src/Prowlarr.Api.V1/ProviderControllerBase.cs, ProviderResource.cs,
// ProviderBulkResource.cs, ProviderTestAllResult.cs, src/Prowlarr.Http/ClientSchema/SchemaBuilder.cs, Field.cs,
// src/NzbDrone.Core/Annotations/FieldDefinitionAttribute.cs, SelectOption.cs,
// src/NzbDrone.Core/ThingiProvider/ProviderFactory.cs, ProviderDefinition.cs,
// src/NzbDrone.Core/Validation/NzbDroneValidationResult.cs, NzbDroneValidationFailure.cs), GPL-3.0.

//! Prowlarr's generic provider API, for every kind of provider (applications now; download clients,
//! notifications and indexer proxies later): list, get, add, change, delete, bulk change and delete, the empty
//! templates (`/schema`), test one, test all and provider actions, with Prowlarr's resource (`fields`,
//! `implementation`, `configContract`, `infoLink`, `tags`) and its checks and answers.
//!
//! A kind of provider is a [`Family`]: its table, its implementations (each with its settings as
//! [`FieldSpec`]s, as Prowlarr's settings classes declare them), its own fields (an application's `syncLevel`),
//! its test and what happens after a change. [`routes`] gives every route for one family.

use super::super::{api_exception, body_json, check, fail, localization, not_found, validate_id, validation, ApiError, V1};
use crate::AppState;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use kit_server_core::{p, Db, RowExt, SqliteDb};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::future::Future;

/// What a settings value is in Prowlarr's settings class, which decides how a field's value is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    /// `string`.
    Text,
    /// `int`.
    Int,
    /// `int?`.
    OptInt,
    /// `double`.
    Double,
    /// `double?`.
    OptDouble,
    /// `bool`.
    Bool,
    /// `IEnumerable<int>`.
    IntList,
    /// `IEnumerable<string>`.
    TextList,
}

/// Prowlarr's `FieldType`, as the web app reads it (`textbox`, `select`, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    /// One line of text.
    Textbox,
    /// A number.
    Number,
    /// A password.
    Password,
    /// A switch.
    Checkbox,
    /// A choice from a list.
    Select,
    /// A folder.
    Path,
    /// A file.
    FilePath,
    /// Tags.
    Tag,
    /// A button.
    Action,
    /// An address.
    Url,
    /// A captcha.
    Captcha,
    /// An OAuth sign-in.
    OAuth,
    /// A device.
    Device,
    /// Tags from a list.
    TagSelect,
}

impl FieldType {
    /// The name the web app reads (`FirstCharToLower` of the enum name).
    pub fn name(self) -> &'static str {
        match self {
            FieldType::Textbox => "textbox",
            FieldType::Number => "number",
            FieldType::Password => "password",
            FieldType::Checkbox => "checkbox",
            FieldType::Select => "select",
            FieldType::Path => "path",
            FieldType::FilePath => "filePath",
            FieldType::Tag => "tag",
            FieldType::Action => "action",
            FieldType::Url => "url",
            FieldType::Captcha => "captcha",
            FieldType::OAuth => "oAuth",
            FieldType::Device => "device",
            FieldType::TagSelect => "tagSelect",
        }
    }
}

/// Prowlarr's `PrivacyLevel`: a password or API key is never sent back (`********` instead).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Privacy {
    /// Shown as it is.
    Normal,
    /// A password.
    Password,
    /// An API key.
    ApiKey,
    /// A user name.
    UserName,
}

impl Privacy {
    /// The name in JSON.
    fn name(self) -> &'static str {
        match self {
            Privacy::Normal => "normal",
            Privacy::Password => "password",
            Privacy::ApiKey => "apiKey",
            Privacy::UserName => "userName",
        }
    }

    /// Whether the value is masked.
    fn secret(self) -> bool {
        matches!(self, Privacy::Password | Privacy::ApiKey)
    }
}

/// Where a select field's choices come from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Options {
    /// Not a select field.
    None,
    /// The standard categories (`NewznabCategoryFieldConverter`).
    Categories,
    /// An enum's members: (value, name), in order.
    Enum(&'static [(i64, &'static str)]),
}

/// A settings default (what Prowlarr's settings class sets in its constructor).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Initial {
    /// No value (`null`).
    Null,
    /// Text.
    Text(&'static str),
    /// A whole number.
    Int(i64),
    /// A switch.
    Bool(bool),
    /// A list of numbers.
    Ints(&'static [i64]),
}

impl Initial {
    /// The default as JSON.
    pub fn value(self) -> Value {
        match self {
            Initial::Null => Value::Null,
            Initial::Text(t) => json!(t),
            Initial::Int(i) => json!(i),
            Initial::Bool(b) => json!(b),
            Initial::Ints(l) => json!(l),
        }
    }
}

/// One setting, as a `[FieldDefinition]` on a settings class property declares it.
#[derive(Debug, Clone, Copy)]
pub struct FieldSpec {
    /// Its order on the form.
    pub order: i64,
    /// Its name (the property's name in camelCase, `seedCriteria.seedRatio` when nested).
    pub name: &'static str,
    /// Its label (a localization key or the text itself).
    pub label: &'static str,
    /// The unit shown after it.
    pub unit: Option<&'static str>,
    /// Its help text (a localization key or the text itself).
    pub help_text: Option<&'static str>,
    /// What kind of value it holds.
    pub kind: ValueKind,
    /// How the web app shows it.
    pub field_type: FieldType,
    /// Only shown with advanced settings.
    pub advanced: bool,
    /// Whether it is a secret.
    pub privacy: Privacy,
    /// The example shown in an empty field.
    pub placeholder: Option<&'static str>,
    /// Its choices, for a select field.
    pub options: Options,
    /// Its value in a new provider.
    pub default: Initial,
}

impl FieldSpec {
    /// A text field with nothing else set, to build the others from.
    pub const BASE: FieldSpec = FieldSpec {
        order: 0,
        name: "",
        label: "",
        unit: None,
        help_text: None,
        kind: ValueKind::Text,
        field_type: FieldType::Textbox,
        advanced: false,
        privacy: Privacy::Normal,
        placeholder: None,
        options: Options::None,
        default: Initial::Null,
    };
}

/// One implementation of a family (a provider class in Prowlarr: `Sonarr`, `Radarr`, …).
pub struct Implementation {
    /// Its class name, stored as `implementation`.
    pub implementation: &'static str,
    /// The name people see (`implementationName`).
    pub implementation_name: &'static str,
    /// Its settings class (`configContract`).
    pub config_contract: &'static str,
    /// Its settings, in order.
    pub fields: &'static [FieldSpec],
    /// Its settings' checks (the settings class' validator).
    pub validate: fn(&Map<String, Value>) -> Vec<ProviderFailure>,
}

impl Implementation {
    /// Its settings as a new provider has them.
    pub fn defaults(&self) -> Map<String, Value> {
        self.fields.iter().map(|f| (f.name.to_string(), f.default.value())).collect()
    }
}

/// A saved provider (Prowlarr's `ProviderDefinition`).
#[derive(Debug, Clone, PartialEq)]
pub struct Definition {
    /// Its id (0 before it is saved).
    pub id: i64,
    /// Its name.
    pub name: Option<String>,
    /// Its implementation's class name.
    pub implementation: String,
    /// Its settings class.
    pub config_contract: String,
    /// Its settings, by field name.
    pub settings: Map<String, Value>,
    /// Its tags' ids.
    pub tags: Vec<i64>,
    /// The family's own fields (an application's `syncLevel`).
    pub extra: Map<String, Value>,
}

impl Definition {
    /// One setting (`Value::Null` when it is not set).
    pub fn setting(&self, name: &str) -> &Value {
        self.settings.get(name).unwrap_or(&Value::Null)
    }

    /// One setting as text ("" when it is not set).
    pub fn text(&self, name: &str) -> String {
        self.setting(name).as_str().unwrap_or("").to_string()
    }

    /// One setting as a list of numbers.
    pub fn ints(&self, name: &str) -> Vec<i64> {
        self.setting(name).as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default()
    }

    /// One setting as a switch.
    pub fn flag(&self, name: &str) -> bool {
        self.setting(name).as_bool().unwrap_or(false)
    }

    /// Its name ("" when it has none).
    pub fn label(&self) -> String {
        self.name.clone().unwrap_or_default()
    }
}

/// A failed check of a provider's settings or test, as Prowlarr's `NzbDroneValidationFailure` (or a plain
/// `ValidationFailure` from a test).
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderFailure {
    /// The setting it is about (`BaseUrl`, `ApiKey`; "" for the provider as a whole).
    pub property: String,
    /// What is wrong, in words.
    pub message: String,
    /// The value that was checked.
    pub attempted: Value,
    /// Only a warning.
    pub is_warning: bool,
}

impl ProviderFailure {
    /// A failure of `property` saying `message`.
    pub fn new(property: &str, message: impl Into<String>) -> ProviderFailure {
        ProviderFailure { property: property.into(), message: message.into(), attempted: Value::Null, is_warning: false }
    }

    /// The same failure with the value that was checked.
    pub fn with_value(mut self, v: Value) -> ProviderFailure {
        self.attempted = v;
        self
    }

    /// As Prowlarr's `NzbDroneValidationFailure` (what saving and testing answer).
    pub fn json(&self) -> Value {
        let mut v =
            json!({ "isWarning": self.is_warning, "propertyName": self.property, "errorMessage": self.message, "severity": "error" });
        if !self.attempted.is_null() {
            v["attemptedValue"] = self.attempted.clone();
        }
        v
    }

    /// As a plain `ValidationFailure` (what `/testall` lists).
    pub fn plain_json(&self) -> Value {
        let mut v = json!({ "propertyName": self.property, "errorMessage": self.message, "severity": "error" });
        if !self.attempted.is_null() {
            v["attemptedValue"] = self.attempted.clone();
        }
        v
    }
}

/// Prowlarr's `IsValidUrl` check (`Invalid Url: '<url>'`) on a text setting.
pub fn check_url(s: &Map<String, Value>, field: &str, property: &str) -> Option<ProviderFailure> {
    let v = s.get(field).cloned().unwrap_or(Value::Null);
    let text = v.as_str().map(str::to_string);
    let ok = text.as_deref().is_some_and(is_valid_url);
    (!ok).then(|| {
        let shown = text.unwrap_or_else(|| "{url}".into());
        ProviderFailure::new(property, format!("Invalid Url: '{shown}'")).with_value(v)
    })
}

/// .NET's `IsValidUrl`: an absolute, well-formed address with no spaces around it.
pub fn is_valid_url(s: &str) -> bool {
    if s.trim().is_empty() || s.starts_with(' ') || s.ends_with(' ') || s.chars().any(|c| c.is_whitespace() || c == '\\') {
        return false;
    }
    url::Url::parse(s).is_ok()
}

/// FluentValidation's `NotEmpty` on a setting: `'<Name>' must not be empty.` for null, "", spaces or [].
pub fn check_not_empty(s: &Map<String, Value>, field: &str, property: &str) -> Option<ProviderFailure> {
    let v = s.get(field).cloned().unwrap_or(Value::Null);
    let empty = match &v {
        Value::Null => true,
        Value::String(t) => t.trim().is_empty(),
        Value::Array(a) => a.is_empty(),
        _ => false,
    };
    empty.then(|| ProviderFailure::new(property, format!("'{}' must not be empty.", validation::display_name(property))).with_value(v))
}

/// A kind of provider: applications, download clients, notifications, indexer proxies.
pub trait Family: Send + Sync + 'static {
    /// The resource's path under `/api/v1/` (`applications`).
    const RESOURCE: &'static str;
    /// The table that keeps them.
    const TABLE: &'static str;
    /// The model's name in "not found" answers (`ApplicationDefinition`).
    const MODEL: &'static str;

    /// Every implementation.
    fn implementations() -> &'static [Implementation];

    /// Whether a provider is on (Prowlarr's `Enable`): only an enabled one is checked and tested when saved.
    fn enabled(def: &Definition) -> bool;

    /// The family's own fields from a request body (`ToModel`), the body's property names in any case.
    fn extra_from_body(body: &Map<String, Value>) -> Result<Map<String, Value>, ApiError>;

    /// Adds the family's own fields to a provider's resource (`ToResource`).
    fn extra_json(def: &Definition, out: &mut Map<String, Value>);

    /// The family's own fields of a new provider (in `/schema`).
    fn default_extra() -> Map<String, Value>;

    /// Changes the family's own fields of one provider from a bulk request.
    fn bulk_extra(def: &mut Definition, body: &Map<String, Value>) -> Result<(), ApiError>;

    /// Tests a provider (Prowlarr's `factory.Test`), recording success or failure for a saved one.
    fn test(st: &AppState, def: &Definition) -> impl Future<Output = Vec<ProviderFailure>> + Send;

    /// After a provider was added.
    fn added(st: &AppState, def: &Definition);

    /// After a provider was changed.
    fn updated(st: &AppState, def: &Definition);

    /// After a provider was deleted.
    fn deleted(st: &AppState, id: i64);

    /// After providers were changed at once.
    fn bulk_updated(st: &AppState, defs: &[Definition]);

    /// After providers were deleted at once.
    fn bulk_deleted(st: &AppState, ids: &[i64]);

    /// A provider's action (`RequestAction`), `null` when it has none.
    fn request_action(
        st: &AppState,
        def: &Definition,
        name: &str,
        query: &HashMap<String, String>,
    ) -> impl Future<Output = Result<Value, ApiError>> + Send;

    /// The wiki link of an implementation.
    fn info_link(def: &Definition) -> String {
        format!("https://wiki.servarr.com/prowlarr/supported#{}", def.implementation.to_lowercase().replace(' ', "-"))
    }
}

/// The table of a family.
pub fn schema(table: &str) -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS {table} (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL DEFAULT '',
            implementation TEXT NOT NULL,
            config_contract TEXT NOT NULL,
            settings TEXT NOT NULL DEFAULT '{{}}',
            tags TEXT NOT NULL DEFAULT '[]',
            extra TEXT NOT NULL DEFAULT '{{}}'
        );"
    )
}

/// A provider of a row.
fn of_row(r: &kit_server_core::Row) -> Definition {
    let obj = |k: &str| match serde_json::from_str::<Value>(&r.s(k)) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    };
    let name = r.s("name");
    Definition {
        id: r.i("id"),
        name: (!name.is_empty()).then_some(name),
        implementation: r.s("implementation"),
        config_contract: r.s("config_contract"),
        settings: obj("settings"),
        tags: serde_json::from_str::<Vec<i64>>(&r.s("tags")).unwrap_or_default(),
        extra: obj("extra"),
    }
}

/// Every provider of family `F`, by id.
pub fn all<F: Family>(db: &SqliteDb) -> kit_server_core::Result<Vec<Definition>> {
    Ok(db.query(&format!("SELECT * FROM {} ORDER BY id", F::TABLE), p![])?.iter().map(of_row).collect())
}

/// One provider of family `F`, `None` when there is none with `id`.
pub fn find<F: Family>(db: &SqliteDb, id: i64) -> kit_server_core::Result<Option<Definition>> {
    Ok(db.one(&format!("SELECT * FROM {} WHERE id=?", F::TABLE), p![id])?.map(|r| of_row(&r)))
}

/// Saves a new provider; its id.
pub fn insert<F: Family>(db: &SqliteDb, d: &Definition) -> kit_server_core::Result<i64> {
    db.execute(
        &format!("INSERT INTO {} (name, implementation, config_contract, settings, tags, extra) VALUES (?,?,?,?,?,?)", F::TABLE),
        p![
            d.label(),
            d.implementation.clone(),
            d.config_contract.clone(),
            Value::Object(d.settings.clone()).to_string(),
            json!(d.tags).to_string(),
            Value::Object(d.extra.clone()).to_string()
        ],
    )
}

/// Saves a changed provider.
pub fn update<F: Family>(db: &SqliteDb, d: &Definition) -> kit_server_core::Result<()> {
    db.execute(
        &format!("UPDATE {} SET name=?, implementation=?, config_contract=?, settings=?, tags=?, extra=? WHERE id=?", F::TABLE),
        p![
            d.label(),
            d.implementation.clone(),
            d.config_contract.clone(),
            Value::Object(d.settings.clone()).to_string(),
            json!(d.tags).to_string(),
            Value::Object(d.extra.clone()).to_string(),
            d.id
        ],
    )?;
    Ok(())
}

/// The implementation a provider names, in any case.
pub fn implementation_of<F: Family>(name: &str) -> Option<&'static Implementation> {
    F::implementations().iter().find(|i| i.implementation.eq_ignore_ascii_case(name))
}

/// Whether a provider's settings pass their checks (`Settings.Validate().IsValid`).
pub fn settings_valid<F: Family>(def: &Definition) -> bool {
    implementation_of::<F>(&def.implementation).is_some_and(|i| (i.validate)(&def.settings).iter().all(|f| f.is_warning))
}

/// The settings as Prowlarr's web app shows them (`SchemaBuilder.ToSchema`): every field with its label, help text
/// and value, secrets masked.
pub fn to_schema(imp: &Implementation, settings: &Map<String, Value>, lang: &str) -> Vec<Value> {
    let say = |t: &str| localization::localized(lang, t, &[]).unwrap_or_default();
    let mut specs: Vec<&FieldSpec> = imp.fields.iter().collect();
    specs.sort_by_key(|f| f.order);
    specs
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let mut v = Map::new();
            v.insert("order".into(), json!(i));
            v.insert("name".into(), json!(f.name));
            if !f.label.is_empty() {
                v.insert("label".into(), json!(say(f.label)));
            }
            if let Some(u) = f.unit {
                v.insert("unit".into(), json!(u));
            }
            if let Some(h) = f.help_text {
                v.insert("helpText".into(), json!(say(h)));
            }
            let mut value = settings.get(f.name).cloned().unwrap_or_else(|| f.default.value());
            if f.privacy.secret() && !value.is_null() && value != json!("") {
                value = json!(MASK);
            }
            if !value.is_null() {
                v.insert("value".into(), value);
            }
            v.insert("type".into(), json!(f.field_type.name()));
            v.insert("advanced".into(), json!(f.advanced));
            match f.options {
                Options::None => {}
                Options::Categories => {
                    v.insert("selectOptions".into(), Value::Array(super::categories::select_options()));
                }
                Options::Enum(members) => {
                    let list = members
                        .iter()
                        .map(|(value, name)| json!({ "value": value, "name": name, "order": value, "hint": format!("({value})") }))
                        .collect();
                    v.insert("selectOptions".into(), Value::Array(list));
                }
            }
            v.insert("privacy".into(), json!(f.privacy.name()));
            if let Some(p) = f.placeholder {
                v.insert("placeholder".into(), json!(p));
            }
            let is_float = f.field_type == FieldType::Number && matches!(f.kind, ValueKind::Double | ValueKind::OptDouble);
            v.insert("isFloat".into(), json!(is_float));
            Value::Object(v)
        })
        .collect()
}

/// What Prowlarr sends instead of a secret.
pub const MASK: &str = "********";

/// .NET's `ToString()` of a JSON value, as Prowlarr's converters read numbers from it.
fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        other => other.to_string(),
    }
}

/// One field's value from a request, as Prowlarr's `SchemaBuilder` converts it to the property's type.
fn convert(f: &FieldSpec, v: &Value) -> Result<Value, ApiError> {
    let cannot = |what: &str| fail(500, format!("The setting '{}' needs {what}.", f.name));
    Ok(match f.kind {
        ValueKind::Text => match v {
            Value::Null => Value::Null,
            Value::String(_) => v.clone(),
            _ => return Err(cannot("text")),
        },
        ValueKind::Int | ValueKind::OptInt => {
            let parsed = if v.is_null() { None } else { as_text(v).trim().parse::<i64>().ok().filter(|n| i32::try_from(*n).is_ok()) };
            match (parsed, f.kind) {
                (Some(n), _) => json!(n),
                (None, ValueKind::Int) => json!(0),
                (None, _) => Value::Null,
            }
        }
        ValueKind::Double | ValueKind::OptDouble => {
            let parsed = if v.is_null() { None } else { as_text(v).trim().parse::<f64>().ok() };
            match (parsed, f.kind) {
                (Some(n), _) => json!(n),
                (None, ValueKind::Double) => json!(0.0),
                (None, _) => Value::Null,
            }
        }
        ValueKind::Bool => match v {
            Value::Bool(_) => v.clone(),
            Value::Number(n) if n.as_i64() == Some(1) => json!(true),
            Value::Number(n) if n.as_i64() == Some(0) => json!(false),
            Value::Null => return Err(cannot("true or false")),
            _ => return Err(cannot("true or false")),
        },
        ValueKind::IntList => match v {
            Value::Null => json!([]),
            Value::Array(a) => {
                let mut out = Vec::new();
                for x in a {
                    out.push(x.as_i64().filter(|n| i32::try_from(*n).is_ok()).ok_or_else(|| cannot("a list of whole numbers"))?);
                }
                json!(out)
            }
            other => {
                let mut out = Vec::new();
                for part in as_text(other).split(',').filter(|p| !p.is_empty()) {
                    out.push(part.trim().parse::<i64>().map_err(|_| cannot("a list of whole numbers"))?);
                }
                json!(out)
            }
        },
        ValueKind::TextList => match v {
            Value::Null => json!([]),
            Value::Array(a) => json!(a.iter().map(|x| x.as_str().map(str::to_string)).collect::<Vec<_>>()),
            other => json!(as_text(other).split(',').filter(|p| !p.is_empty()).map(str::trim).collect::<Vec<_>>()),
        },
    })
}

/// The settings from a request's `fields` (`SchemaBuilder.ReadFromSchema`): each field the request names is read
/// (a masked secret keeps the saved one), the others keep a new provider's default.
pub fn read_settings(
    imp: &Implementation,
    fields: &[Value],
    existing: Option<&Map<String, Value>>,
) -> Result<Map<String, Value>, ApiError> {
    let mut out = imp.defaults();
    for f in imp.fields {
        let Some(given) = fields.iter().find(|x| x.get("name").and_then(Value::as_str) == Some(f.name)) else { continue };
        let value = given.get("value").cloned().unwrap_or(Value::Null);
        let masked = matches!(&value, Value::String(s) if s == MASK) || (!value.is_string() && as_text(&value) == MASK);
        if f.privacy.secret() && masked {
            if let Some(existing) = existing {
                out.insert(f.name.into(), existing.get(f.name).cloned().unwrap_or_else(|| f.default.value()));
                continue;
            }
        }
        out.insert(f.name.into(), convert(f, &value)?);
    }
    Ok(out)
}

/// A property of a request body, its name in any case (Prowlarr reads them so).
pub fn prop<'a>(body: &'a Map<String, Value>, name: &str) -> Option<&'a Value> {
    body.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v)
}

/// A request body's tag ids, each once.
fn tags_of(v: Option<&Value>) -> Result<Vec<i64>, ApiError> {
    let mut out: Vec<i64> = Vec::new();
    match v {
        None | Some(Value::Null) => {}
        Some(Value::Array(a)) => {
            for t in a {
                let id = t.as_i64().ok_or_else(|| fail(400, "Each tag must be a tag's number."))?;
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        }
        Some(_) => return Err(fail(400, "The tags must be a list of tag numbers.")),
    }
    Ok(out)
}

/// The text of a body property (`None` for null or missing).
fn text_prop(body: &Map<String, Value>, name: &str) -> Option<String> {
    prop(body, name).and_then(Value::as_str).map(str::to_string)
}

/// A request body as a JSON object.
fn object(bytes: &[u8]) -> Result<Map<String, Value>, ApiError> {
    match body_json(bytes)? {
        Value::Object(m) => Ok(m),
        _ => Err(fail(400, "The request body must be a JSON object.")),
    }
}

/// The resource's id in a body (0 when there is none).
fn id_of(body: &Map<String, Value>) -> i64 {
    prop(body, "id").and_then(Value::as_i64).unwrap_or(0)
}

/// The checks every save runs on the resource (`SharedValidator`): a name, unique in the family, an
/// implementation and a settings class.
fn shared_checks<F: Family>(db: &SqliteDb, body: &Map<String, Value>) -> Result<Vec<Option<validation::Failure>>, ApiError> {
    let name = text_prop(body, "name");
    let id = id_of(body);
    let mut out = vec![validation::not_empty("Name", name.as_deref())];
    if let Some(n) = name.as_deref() {
        let taken = all::<F>(db)?.iter().any(|d| d.label().to_lowercase() == n.to_lowercase() && d.id != id);
        if taken {
            out.push(Some(validation::Failure::new("Name", json!(n), "PredicateValidator", "Should be unique")));
        }
    }
    out.push(validation::not_empty("Implementation", text_prop(body, "implementation").as_deref()));
    out.push(validation::not_empty("ConfigContract", text_prop(body, "configContract").as_deref()));
    Ok(out)
}

/// A provider from a request body (`ToModel`), with `existing` for the secrets it leaves masked.
fn to_model<F: Family>(body: &Map<String, Value>, existing: Option<&Definition>) -> Result<Definition, ApiError> {
    let implementation = text_prop(body, "implementation").unwrap_or_default();
    let contract = text_prop(body, "configContract").unwrap_or_default();
    let imp = F::implementations()
        .iter()
        .find(|i| i.config_contract == contract)
        .ok_or_else(|| fail(500, format!("Lantern doesn't know the settings '{contract}'. Use one from the schema.")))?;
    if implementation_of::<F>(&implementation).is_none() {
        return Err(fail(500, format!("Lantern doesn't know the implementation '{implementation}'. Use one from the schema.")));
    }
    let fields: Vec<Value> = prop(body, "fields").and_then(Value::as_array).cloned().unwrap_or_default();
    let settings = read_settings(imp, &fields, existing.map(|d| &d.settings))?;
    Ok(Definition {
        id: id_of(body),
        name: text_prop(body, "name"),
        implementation,
        config_contract: imp.config_contract.to_string(),
        settings,
        tags: tags_of(prop(body, "tags"))?,
        extra: F::extra_from_body(body)?,
    })
}

/// A provider as Prowlarr's resource (`ToResource`).
pub fn to_resource<F: Family>(def: &Definition, lang: &str) -> Value {
    let imp = implementation_of::<F>(&def.implementation);
    let mut v = Map::new();
    v.insert("id".into(), json!(def.id));
    if let Some(n) = &def.name {
        v.insert("name".into(), json!(n));
    }
    let fields = imp.map(|i| to_schema(i, &def.settings, lang)).unwrap_or_default();
    v.insert("fields".into(), Value::Array(fields));
    if let Some(i) = imp {
        v.insert("implementationName".into(), json!(i.implementation_name));
    }
    v.insert("implementation".into(), json!(def.implementation));
    v.insert("configContract".into(), json!(def.config_contract));
    v.insert("infoLink".into(), json!(F::info_link(def)));
    v.insert("tags".into(), json!(def.tags));
    F::extra_json(def, &mut v);
    Value::Object(v)
}

/// What a provider route answers when it can't do what was asked: an API error, or the failed checks of the
/// provider's settings or test (a 400 with Prowlarr's list).
#[derive(Debug)]
pub enum ProviderError {
    /// An error as the rest of the API answers it.
    Api(ApiError),
    /// The settings or the test failed.
    Failures(Vec<ProviderFailure>),
}

impl From<ApiError> for ProviderError {
    fn from(e: ApiError) -> ProviderError {
        ProviderError::Api(e)
    }
}

impl From<kit_server_core::Error> for ProviderError {
    fn from(e: kit_server_core::Error) -> ProviderError {
        ProviderError::Api(ApiError::Kit(e))
    }
}

impl IntoResponse for ProviderError {
    fn into_response(self) -> Response {
        match self {
            ProviderError::Api(e) => e.into_response(),
            ProviderError::Failures(list) => {
                (StatusCode::BAD_REQUEST, Json(Value::Array(list.iter().map(ProviderFailure::json).collect()))).into_response()
            }
        }
    }
}

/// What a provider route returns.
type Answer = Result<Response, ProviderError>;

/// Prowlarr's `VerifyValidationResult`: the failures as an error when one is not a warning (or, with
/// `include_warnings`, when there are any).
fn verify(list: Vec<ProviderFailure>, include_warnings: bool) -> Result<(), ProviderError> {
    let errors = list.iter().any(|f| !f.is_warning);
    if errors || (include_warnings && !list.is_empty()) {
        let list = if include_warnings { list } else { list.into_iter().filter(|f| !f.is_warning).collect() };
        return Err(ProviderError::Failures(list));
    }
    Ok(())
}

/// The settings checks of a provider (`Settings.Validate()`).
fn validate_settings<F: Family>(def: &Definition) -> Vec<ProviderFailure> {
    implementation_of::<F>(&def.implementation).map(|i| (i.validate)(&def.settings)).unwrap_or_default()
}

/// `?forceSave=true` and the like.
fn flag(q: &HashMap<String, String>, name: &str) -> bool {
    q.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).is_some_and(|(_, v)| v.eq_ignore_ascii_case("true"))
}

/// The UI language, for labels and help texts.
fn lang(st: &AppState) -> String {
    super::super::config::ui_language(&st.db)
}

/// An answer with a status, a JSON body and, when given, the resource's address.
fn answer(status: StatusCode, body: Value, location: Option<String>) -> Response {
    let mut r = (status, Json(body)).into_response();
    if let Some(l) = location.and_then(|l| HeaderValue::from_str(&l).ok()) {
        r.headers_mut().insert(header::LOCATION, l);
    }
    r
}

/// Every route of family `F` (Prowlarr's `ProviderControllerBase`).
pub fn routes<F: Family>() -> Router<AppState> {
    let base = format!("/api/v1/{}", F::RESOURCE);
    Router::new()
        .route(&base, get(get_all::<F>).post(create::<F>))
        .route(&format!("{base}/bulk"), put(bulk_update::<F>).delete(bulk_delete::<F>))
        .route(&format!("{base}/schema"), get(templates::<F>))
        .route(&format!("{base}/test"), post(test::<F>))
        .route(&format!("{base}/testall"), post(test_all::<F>))
        .route(&format!("{base}/action/{{name}}"), post(action::<F>))
        .route(&format!("{base}/{{id}}"), get(get_one::<F>).put(change::<F>).delete(remove::<F>))
}

/// `GET /api/v1/<resource>`: every provider, by name.
async fn get_all<F: Family>(State(st): State<AppState>, _: V1) -> Answer {
    let lang = lang(&st);
    let mut list = all::<F>(&st.db)?;
    list.sort_by(|a, b| a.label().to_lowercase().cmp(&b.label().to_lowercase()).then(a.label().cmp(&b.label())));
    Ok(Json(Value::Array(list.iter().map(|d| to_resource::<F>(d, &lang)).collect())).into_response())
}

/// `GET /api/v1/<resource>/{id}`.
async fn get_one<F: Family>(State(st): State<AppState>, _: V1, Path(id): Path<i64>) -> Answer {
    let d = find::<F>(&st.db, id)?.ok_or_else(|| not_found(F::MODEL, id))?;
    Ok(Json(to_resource::<F>(&d, &lang(&st))).into_response())
}

/// `POST /api/v1/<resource>[?forceSave=true]`: checks the resource and, when it is on, its settings and a test
/// (a test always runs for an enabled provider; `forceSave` only lets warnings through), then saves it. 201.
async fn create<F: Family>(State(st): State<AppState>, _: V1, Query(q): Query<HashMap<String, String>>, bytes: Bytes) -> Answer {
    let body = object(&bytes)?;
    let mut checks = shared_checks::<F>(&st.db, &body)?;
    checks.push(validation::not_null("Fields", prop(&body, "fields").unwrap_or(&Value::Null)));
    check(checks)?;
    let force = flag(&q, "forceSave");
    let mut def = to_model::<F>(&body, None)?;
    if F::enabled(&def) {
        verify(validate_settings::<F>(&def), !force)?;
        verify(F::test(&st, &def).await, !force)?;
    }
    def.id = insert::<F>(&st.db, &def)?;
    F::added(&st, &def);
    let base = format!("/api/v1/{}", F::RESOURCE);
    Ok(answer(StatusCode::CREATED, to_resource::<F>(&def, &lang(&st)), Some(format!("{base}/{}", def.id))))
}

/// `PUT /api/v1/<resource>/{id}[?forceSave=true]`: checks and (when it is on and not forced) tests the changed
/// provider, then saves it. 202.
async fn change<F: Family>(
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
    let mut checks = shared_checks::<F>(&st.db, &body)?;
    checks.push(validation::valid_id(id_of(&body)));
    check(checks)?;
    validate_id(path_id)?;
    let force = flag(&q, "forceSave");
    let existing = if id_of(&body) > 0 { find::<F>(&st.db, id_of(&body))? } else { None };
    let def = to_model::<F>(&body, existing.as_ref())?;
    if F::enabled(&def) {
        verify(validate_settings::<F>(&def), !force)?;
        if !force {
            verify(F::test(&st, &def).await, true)?;
        }
    }
    if existing.is_none() {
        return Err(not_found(F::MODEL, def.id).into());
    }
    update::<F>(&st.db, &def)?;
    F::updated(&st, &def);
    let base = format!("/api/v1/{}", F::RESOURCE);
    Ok(answer(StatusCode::ACCEPTED, to_resource::<F>(&def, &lang(&st)), Some(format!("{base}/{}", def.id))))
}

/// `DELETE /api/v1/<resource>/{id}`: `{}`.
async fn remove<F: Family>(State(st): State<AppState>, _: V1, Path(id): Path<i64>) -> Answer {
    validate_id(id)?;
    st.db.execute(&format!("DELETE FROM {} WHERE id=?", F::TABLE), p![id])?;
    F::deleted(&st, id);
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

/// `PUT /api/v1/<resource>/bulk` `{"ids": [...], "tags": [...], "applyTags": "add"|"remove"|"replace", ...}`:
/// changes the tags and the family's own fields of several providers at once. 202 with them.
async fn bulk_update<F: Family>(State(st): State<AppState>, _: V1, bytes: Bytes) -> Answer {
    let body = object(&bytes)?;
    let ids = bulk_ids(&body)?;
    if ids.is_empty() {
        return Err(api_exception(400, Some("ids must be provided")).into());
    }
    let mut defs: Vec<Definition> = all::<F>(&st.db)?.into_iter().filter(|d| ids.contains(&d.id)).collect();
    if defs.len() != ids.len() {
        return Err(fail(500, format!("Expected query to return {} rows but returned {}", ids.len(), defs.len())).into());
    }
    let tags = match prop(&body, "tags") {
        None | Some(Value::Null) => None,
        v => Some(tags_of(v)?),
    };
    let apply = prop(&body, "applyTags").map(|v| as_text(v).to_lowercase()).unwrap_or_else(|| "add".into());
    for d in &mut defs {
        if let Some(tags) = &tags {
            match apply.as_str() {
                "remove" | "1" => d.tags.retain(|t| !tags.contains(t)),
                "replace" | "2" => d.tags = tags.clone(),
                _ => {
                    for t in tags {
                        if !d.tags.contains(t) {
                            d.tags.push(*t);
                        }
                    }
                }
            }
        }
        F::bulk_extra(d, &body)?;
    }
    for d in &defs {
        update::<F>(&st.db, d)?;
    }
    F::bulk_updated(&st, &defs);
    let lang = lang(&st);
    Ok(answer(StatusCode::ACCEPTED, Value::Array(defs.iter().map(|d| to_resource::<F>(d, &lang)).collect()), None))
}

/// `DELETE /api/v1/<resource>/bulk` `{"ids": [...]}`: `{}`.
async fn bulk_delete<F: Family>(State(st): State<AppState>, _: V1, bytes: Bytes) -> Answer {
    let ids = bulk_ids(&object(&bytes)?)?;
    for id in &ids {
        st.db.execute(&format!("DELETE FROM {} WHERE id=?", F::TABLE), p![*id])?;
    }
    F::bulk_deleted(&st, &ids);
    Ok(Json(json!({})).into_response())
}

/// `GET /api/v1/<resource>/schema`: a new provider of each implementation, by implementation name, each with its
/// presets (none yet).
async fn templates<F: Family>(State(st): State<AppState>, _: V1) -> Answer {
    let lang = lang(&st);
    let mut imps: Vec<&Implementation> = F::implementations().iter().collect();
    imps.sort_by(|a, b| a.implementation_name.cmp(b.implementation_name));
    let list = imps
        .iter()
        .map(|i| {
            let def = Definition {
                id: 0,
                name: None,
                implementation: i.implementation.into(),
                config_contract: i.config_contract.into(),
                settings: i.defaults(),
                tags: Vec::new(),
                extra: F::default_extra(),
            };
            let mut v = to_resource::<F>(&def, &lang);
            v["presets"] = json!([]);
            v
        })
        .collect();
    Ok(Json(Value::Array(list)).into_response())
}

/// `POST /api/v1/<resource>/test[?forceTest=true]`: checks the resource's name, implementation and settings and
/// tests it, without saving. `{}`.
async fn test<F: Family>(State(st): State<AppState>, _: V1, Query(q): Query<HashMap<String, String>>, bytes: Bytes) -> Answer {
    let body = object(&bytes)?;
    check(shared_checks::<F>(&st.db, &body)?)?;
    let force = flag(&q, "forceTest");
    let existing = if id_of(&body) > 0 { find::<F>(&st.db, id_of(&body))? } else { None };
    let def = to_model::<F>(&body, existing.as_ref())?;
    verify(validate_settings::<F>(&def), !force)?;
    verify(F::test(&st, &def).await, true)?;
    Ok(Json(json!({})).into_response())
}

/// `POST /api/v1/<resource>/testall`: tests every enabled provider whose settings pass their checks;
/// `[{"id", "isValid", "validationFailures"}]`, a 400 when one failed.
async fn test_all<F: Family>(State(st): State<AppState>, _: V1) -> Answer {
    let mut out = Vec::new();
    let mut any_failed = false;
    for d in all::<F>(&st.db)?.into_iter().filter(|d| settings_valid::<F>(d) && F::enabled(d)) {
        let failures: Vec<ProviderFailure> = F::test(&st, &d).await.into_iter().filter(|f| !f.is_warning).collect();
        any_failed |= !failures.is_empty();
        out.push(json!({
            "id": d.id,
            "isValid": failures.is_empty(),
            "validationFailures": failures.iter().map(ProviderFailure::plain_json).collect::<Vec<_>>(),
        }));
    }
    let status = if any_failed { StatusCode::BAD_REQUEST } else { StatusCode::OK };
    Ok((status, Json(Value::Array(out))).into_response())
}

/// `POST /api/v1/<resource>/action/{name}`: one of the provider's actions (the query string passed on).
async fn action<F: Family>(
    State(st): State<AppState>,
    _: V1,
    Path(name): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    bytes: Bytes,
) -> Answer {
    let body = object(&bytes)?;
    let existing = if id_of(&body) > 0 { find::<F>(&st.db, id_of(&body))? } else { None };
    let def = to_model::<F>(&body, existing.as_ref())?;
    Ok(Json(F::request_action(&st, &def, &name, &q).await?).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIELDS: &[FieldSpec] = &[
        FieldSpec { order: 0, name: "baseUrl", label: "Sonarr Server", default: Initial::Text("http://localhost:8989"), ..FieldSpec::BASE },
        FieldSpec { order: 1, name: "apiKey", label: "API Key", privacy: Privacy::ApiKey, ..FieldSpec::BASE },
        FieldSpec {
            order: 2,
            name: "syncCategories",
            label: "Sync Categories",
            kind: ValueKind::IntList,
            field_type: FieldType::Select,
            options: Options::Categories,
            default: Initial::Ints(&[5000, 5040]),
            ..FieldSpec::BASE
        },
        FieldSpec {
            order: 3,
            name: "flag",
            label: "Flag",
            kind: ValueKind::Bool,
            field_type: FieldType::Checkbox,
            default: Initial::Bool(true),
            ..FieldSpec::BASE
        },
        FieldSpec {
            order: 4,
            name: "seedRatio",
            label: "Ratio",
            kind: ValueKind::OptDouble,
            field_type: FieldType::Number,
            ..FieldSpec::BASE
        },
    ];

    fn none(_: &Map<String, Value>) -> Vec<ProviderFailure> {
        Vec::new()
    }

    const IMP: Implementation = Implementation {
        implementation: "Sonarr",
        implementation_name: "Sonarr",
        config_contract: "SonarrSettings",
        fields: FIELDS,
        validate: none,
    };

    #[test]
    fn settings_read_like_the_schema_builder() {
        // a field left out keeps the default; a masked key keeps the saved one; numbers in text are read
        let saved: Map<String, Value> = [("apiKey".to_string(), json!("secret"))].into_iter().collect();
        let fields = vec![
            json!({ "name": "apiKey", "value": MASK }),
            json!({ "name": "syncCategories", "value": "5000,5030" }),
            json!({ "name": "seedRatio", "value": "1.5" }),
        ];
        let s = read_settings(&IMP, &fields, Some(&saved)).unwrap();
        assert_eq!(s["baseUrl"], "http://localhost:8989");
        assert_eq!(s["apiKey"], "secret");
        assert_eq!(s["syncCategories"], json!([5000, 5030]));
        assert_eq!(s["seedRatio"], json!(1.5));
        assert_eq!(s["flag"], true);
        // a new provider saves the mask as it is, and a null list is empty
        let s = read_settings(&IMP, &[json!({ "name": "apiKey", "value": MASK }), json!({ "name": "syncCategories" })], None).unwrap();
        assert_eq!((s["apiKey"].clone(), s["syncCategories"].clone()), (json!(MASK), json!([])));
        assert!(read_settings(&IMP, &[json!({ "name": "flag", "value": "yes" })], None).is_err());
    }

    #[test]
    fn schema_masks_secrets_and_leaves_out_nulls() {
        let mut s = IMP.defaults();
        s.insert("apiKey".into(), json!("secret"));
        let f = to_schema(&IMP, &s, "en");
        assert_eq!(f[1]["value"], MASK);
        assert_eq!(f[1]["privacy"], "apiKey");
        assert!(f[4].get("value").is_none());
        assert_eq!(f[4]["isFloat"], true);
        assert_eq!(f[2]["selectOptions"][0]["value"], 1000);
        assert_eq!(
            f[0],
            json!({ "order": 0, "name": "baseUrl", "label": "Sonarr Server", "value": "http://localhost:8989", "type": "textbox", "advanced": false, "privacy": "normal", "isFloat": false })
        );
        let s = IMP.defaults();
        assert!(to_schema(&IMP, &s, "en")[1].get("value").is_none());
    }

    #[test]
    fn failures_take_prowlarrs_shapes() {
        let f = ProviderFailure::new("BaseUrl", "Invalid Url: 'x'").with_value(json!("x"));
        assert_eq!(
            f.json(),
            json!({ "isWarning": false, "propertyName": "BaseUrl", "errorMessage": "Invalid Url: 'x'", "attemptedValue": "x", "severity": "error" })
        );
        assert_eq!(
            ProviderFailure::new("", "Failed").plain_json(),
            json!({ "propertyName": "", "errorMessage": "Failed", "severity": "error" })
        );
        let s: Map<String, Value> = [("baseUrl".to_string(), json!("localhost:8989 x"))].into_iter().collect();
        assert_eq!(check_url(&s, "baseUrl", "BaseUrl").unwrap().message, "Invalid Url: 'localhost:8989 x'");
        assert_eq!(check_url(&Map::new(), "baseUrl", "BaseUrl").unwrap().message, "Invalid Url: '{url}'");
        let s: Map<String, Value> = [("baseUrl".to_string(), json!("http://sonarr:8989/sonarr"))].into_iter().collect();
        assert!(check_url(&s, "baseUrl", "BaseUrl").is_none());
        let s: Map<String, Value> = [("apiKey".to_string(), json!("")), ("syncCategories".to_string(), json!([]))].into_iter().collect();
        assert_eq!(check_not_empty(&s, "apiKey", "ApiKey").unwrap().message, "'Api Key' must not be empty.");
        assert_eq!(check_not_empty(&s, "syncCategories", "SyncCategories").unwrap().message, "'Sync Categories' must not be empty.");
    }
}
