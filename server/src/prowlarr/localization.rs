// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/Localization/LocalizationService.cs,
// src/Prowlarr.Api.V1/Localization/LocalizationController.cs), GPL-3.0. The strings in localization/*.json are
// Prowlarr's own (src/NzbDrone.Core/Localization/Core), copied as data under the same licence.

//! Prowlarr's words in every language its web app offers: `GET /api/v1/localization` answers the strings of the
//! language picked under Settings → UI (English for any string the language lacks), and
//! `GET /api/v1/localization/options` the languages to pick from.

use super::{config, ApiResult, APP_NAME, V1};
use crate::AppState;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use indexmap::IndexMap;
use serde_json::{json, Value};

/// The language every other one falls back to.
const DEFAULT_CULTURE: &str = "en";

/// The languages Prowlarr's web app offers: (name in the language, code).
pub const OPTIONS: [(&str, &str); 43] = [
    ("العربية", "ar"),
    ("Български", "bg"),
    ("বাংলা (বাংলাদেশ)", "bn"),
    ("Bosanski", "bs"),
    ("Català", "ca"),
    ("Čeština", "cs"),
    ("Dansk", "da"),
    ("Deutsch", "de"),
    ("English", "en"),
    ("Ελληνικά", "el"),
    ("Español", "es"),
    ("Español (Latino)", "es_MX"),
    ("Eesti", "et"),
    ("فارسی", "fa"),
    ("Suomi", "fi"),
    ("Français", "fr"),
    ("עִבְרִית", "he"),
    ("हिन्दी", "hi"),
    ("Hrvatski", "hr"),
    ("Magyar", "hu"),
    ("Indonesia", "id"),
    ("Íslenska", "is"),
    ("Italiano", "it"),
    ("日本語", "ja"),
    ("한국어", "ko"),
    ("Lietuvių", "lt"),
    ("Norsk bokmål", "nb_NO"),
    ("Nederlands", "nl"),
    ("Polski", "pl"),
    ("Português", "pt"),
    ("Português (Brasil)", "pt_BR"),
    ("Românește", "ro"),
    ("Русский", "ru"),
    ("Slovenčina", "sk"),
    ("српски", "sr"),
    ("Svenska", "sv"),
    ("தமிழ்", "ta"),
    ("ภาษาไทย", "th"),
    ("Türkçe", "tr"),
    ("Українська", "uk"),
    ("Tiếng Việt", "vi"),
    ("汉语 (简化字)", "zh_CN"),
    ("漢語 (繁体字)", "zh_TW"),
];

/// Prowlarr's string files, by file name, built into the server.
const FILES: [(&str, &str); 45] = [
    ("ar.json", include_str!("localization/ar.json")),
    ("bg.json", include_str!("localization/bg.json")),
    ("bn.json", include_str!("localization/bn.json")),
    ("bs.json", include_str!("localization/bs.json")),
    ("ca.json", include_str!("localization/ca.json")),
    ("cs.json", include_str!("localization/cs.json")),
    ("da.json", include_str!("localization/da.json")),
    ("de.json", include_str!("localization/de.json")),
    ("el.json", include_str!("localization/el.json")),
    ("en.json", include_str!("localization/en.json")),
    ("es.json", include_str!("localization/es.json")),
    ("es_MX.json", include_str!("localization/es_MX.json")),
    ("et.json", include_str!("localization/et.json")),
    ("fa.json", include_str!("localization/fa.json")),
    ("fi.json", include_str!("localization/fi.json")),
    ("fr.json", include_str!("localization/fr.json")),
    ("he.json", include_str!("localization/he.json")),
    ("hi.json", include_str!("localization/hi.json")),
    ("hr.json", include_str!("localization/hr.json")),
    ("hu.json", include_str!("localization/hu.json")),
    ("id.json", include_str!("localization/id.json")),
    ("is.json", include_str!("localization/is.json")),
    ("it.json", include_str!("localization/it.json")),
    ("ja.json", include_str!("localization/ja.json")),
    ("ko.json", include_str!("localization/ko.json")),
    ("lt.json", include_str!("localization/lt.json")),
    ("lv.json", include_str!("localization/lv.json")),
    ("nb_NO.json", include_str!("localization/nb_NO.json")),
    ("nl.json", include_str!("localization/nl.json")),
    ("pl.json", include_str!("localization/pl.json")),
    ("pt.json", include_str!("localization/pt.json")),
    ("pt_BR.json", include_str!("localization/pt_BR.json")),
    ("ro.json", include_str!("localization/ro.json")),
    ("ru.json", include_str!("localization/ru.json")),
    ("sk.json", include_str!("localization/sk.json")),
    ("sr.json", include_str!("localization/sr.json")),
    ("sv.json", include_str!("localization/sv.json")),
    ("ta.json", include_str!("localization/ta.json")),
    ("th.json", include_str!("localization/th.json")),
    ("tr.json", include_str!("localization/tr.json")),
    ("uk.json", include_str!("localization/uk.json")),
    ("vi.json", include_str!("localization/vi.json")),
    ("zh_CN.json", include_str!("localization/zh_CN.json")),
    ("zh_Hans.json", include_str!("localization/zh_Hans.json")),
    ("zh_TW.json", include_str!("localization/zh_TW.json")),
];

/// The localization routes.
pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/localization", get(strings)).route("/api/v1/localization/options", get(options))
}

/// The code of the language `ui_language` names when Prowlarr offers it, else English.
pub fn language_identifier(ui_language: &str) -> &'static str {
    OPTIONS.iter().find(|(_, v)| *v == ui_language).map(|(_, v)| *v).unwrap_or(DEFAULT_CULTURE)
}

/// Prowlarr's `GetResourceFilename`: `pt_br` → `pt_BR.json`, `fr` → `fr.json`.
fn resource_filename(culture: &str) -> String {
    let parts: Vec<&str> = culture.split('_').collect();
    if parts.len() == 2 {
        format!("{}_{}.json", parts[0].to_lowercase(), parts[1].to_uppercase())
    } else {
        format!("{}.json", culture.to_lowercase())
    }
}

/// Copies the strings of one file into `dict` (names compared without regard to case, the first spelling kept);
/// a file Prowlarr doesn't have adds nothing.
fn copy_into(dict: &mut IndexMap<String, (String, String)>, file: &str) {
    let Some((_, text)) = FILES.iter().find(|(name, _)| *name == file) else { return };
    let Ok(strings) = serde_json::from_str::<IndexMap<String, String>>(text) else { return };
    for (k, v) in strings {
        let entry = dict.entry(k.to_lowercase()).or_insert_with(|| (k.clone(), String::new()));
        entry.1 = v;
    }
}

/// Every string in `ui_language`: English first, then the language's base (`pt` for `pt_BR`), then the language
/// itself, each replacing the one before.
pub fn dictionary(ui_language: &str) -> IndexMap<String, String> {
    let culture = language_identifier(ui_language).replace('-', "_").to_lowercase();
    let mut dict = IndexMap::new();
    copy_into(&mut dict, &format!("{DEFAULT_CULTURE}.json"));
    if culture.contains('_') {
        copy_into(&mut dict, &resource_filename(culture.split('_').next().unwrap_or("")));
    }
    copy_into(&mut dict, &resource_filename(&culture));
    dict.into_values().collect()
}

/// Prowlarr's `GetLocalizedString`: `phrase` in `ui_language` with its `{tokens}` filled in (`{appName}` is the
/// app's name); the phrase itself when there is no such string. `None` for an empty phrase (Prowlarr throws).
pub fn localized(ui_language: &str, phrase: &str, tokens: &[(&str, String)]) -> Option<String> {
    if phrase.is_empty() {
        return None;
    }
    let dict = dictionary(ui_language);
    let Some(value) = dict.iter().find(|(k, _)| k.eq_ignore_ascii_case(phrase)).map(|(_, v)| v.clone()) else {
        return Some(phrase.to_string());
    };
    let re = regex::Regex::new(r"(?i)\{([a-z0-9]+)\}").expect("a fixed pattern");
    Some(
        re.replace_all(&value, |c: &regex::Captures| {
            let name = &c[1];
            if name == "appName" && !tokens.iter().any(|(t, _)| *t == "appName") {
                return APP_NAME.to_string();
            }
            tokens.iter().find(|(t, _)| *t == name).map(|(_, v)| v.clone()).unwrap_or_else(|| format!("{{{name}}}"))
        })
        .into_owned(),
    )
}

/// `GET /api/v1/localization`: `{"Strings": {...}}` (Prowlarr writes this one answer without camelCase).
async fn strings(State(st): State<AppState>, _: V1) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "Strings": dictionary(&config::ui_language(&st.db)) })))
}

/// `GET /api/v1/localization/options`: `[{"name": "English", "value": "en"}, …]`.
async fn options(_: V1) -> Json<Value> {
    Json(Value::Array(OPTIONS.iter().map(|(n, v)| json!({ "name": n, "value": v })).collect()))
}

#[cfg(test)]
mod tests {
    // Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core.Test/Localization/LocalizationServiceFixture.cs), GPL-3.0.
    use super::*;

    #[test]
    fn should_get_string_in_dictionary_if_lang_exists_and_string_exists() {
        assert_eq!(localized("en", "BackupNow", &[]).as_deref(), Some("Backup Now"));
    }

    #[test]
    fn should_get_string_in_french() {
        assert_eq!(localized("fr", "BackupNow", &[]).as_deref(), Some("Sauvegarder maintenant"));
    }

    #[test]
    fn should_get_string_in_default_dictionary_if_unknown_language_and_string_exists() {
        assert_eq!(localized("", "BackupNow", &[]).as_deref(), Some("Backup Now"));
    }

    #[test]
    fn should_return_argument_if_string_doesnt_exists() {
        assert_eq!(localized("fr", "BadString", &[]).as_deref(), Some("BadString"));
        assert_eq!(localized("en", "BadString", &[]).as_deref(), Some("BadString"));
    }

    #[test]
    fn should_throw_if_empty_string_passed() {
        assert_eq!(localized("en", "", &[]), None);
    }

    #[test]
    fn fills_in_tokens() {
        let got = localized("en", "ApiKeyValidationHealthCheckMessage", &[("length", "20".into())]).unwrap();
        assert_eq!(got, "Please update your API key to be at least 20 characters long. You can do this via settings or the config file");
        assert!(localized("en", "IndexerHealthCheckNoIndexers", &[]).unwrap().contains("Lantern will not return"));
    }

    #[test]
    fn regional_languages_fall_back_to_their_base_then_english() {
        let br = dictionary("pt_BR");
        let en = dictionary("en");
        assert_eq!(br.len(), en.len().max(br.len()));
        assert!(br.contains_key("BackupNow"));
        // every option has a file
        for (_, v) in OPTIONS {
            assert!(FILES.iter().any(|(f, _)| *f == resource_filename(&v.to_lowercase())), "{v}");
        }
    }
}
