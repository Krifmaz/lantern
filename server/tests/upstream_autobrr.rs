// Ported from autobrr v1.90.0 (internal/domain/filter_test.go, release_test.go, releasetags_test.go,
// macros_test.go, rejections_test.go; pkg/wildcard/match_test.go; pkg/sanitize/sanitize_test.go), GPL-2.0-or-later.
//! autobrr's own tests for release parsing, filters, rejections, macros, wildcards and list tidying, run on
//! Lantern's port. The test tables were turned into JSON as they are (inputs and expected results unchanged) and
//! saved under `tests/fixtures/upstream/autobrr/`; every case of every table must give what autobrr's test wants.

use serde_json::{json, Value};
use server::releases::filter::{self, Filter, FilterDownloads};
use server::releases::macros::Macro;
use server::releases::rejections::{RejectionReasons, Val};
use server::releases::release::{self, IndexerMinimal, Release};
use server::releases::releasetags::{self, ReleaseTags};
use server::releases::rls::Type;
use server::releases::{sanitize, wildcard};
use std::collections::BTreeMap;

/// The cases of one saved test table.
fn table(path: &str) -> Vec<Value> {
    let p = format!("{}/tests/fixtures/upstream/autobrr/{path}.json", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p}: {e}"));
    let v: Value = serde_json::from_str(&text).expect("a JSON table");
    let cases = v.as_array().expect("a list of cases").clone();
    assert!(!cases.is_empty(), "{path} has no cases");
    cases
}

/// A text field.
fn st(v: &Value) -> String {
    v.as_str().unwrap_or_else(|| panic!("text wanted, got {v}")).to_string()
}

/// A list of texts (`null` is empty).
fn strs(v: &Value) -> Vec<String> {
    match v {
        Value::Null => Vec::new(),
        Value::Array(a) => a.iter().map(st).collect(),
        _ => panic!("a list wanted, got {v}"),
    }
}

/// A whole number.
fn int(v: &Value) -> i64 {
    v.as_i64().unwrap_or_else(|| panic!("a number wanted, got {v}"))
}

/// A Go constant by name.
fn constant(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        return s.to_string();
    }
    let c = v.get("__const").and_then(Value::as_str).unwrap_or_else(|| panic!("a constant wanted, got {v}"));
    match c {
        "FilterMaxDownloadsMinute" => "MINUTE",
        "FilterMaxDownloadsHour" => "HOUR",
        "FilterMaxDownloadsDay" => "DAY",
        "FilterMaxDownloadsWeek" => "WEEK",
        "FilterMaxDownloadsMonth" => "MONTH",
        "FilterMaxDownloadsEver" => "EVER",
        "FilterMaxDownloadsWindowRolling" => "ROLLING",
        "FilterMaxDownloadsWindowFixed" => "FIXED",
        "SizeUnitsBinary" => "binary",
        "rls.Movie" => "movie",
        "rls.Series" => "series",
        "rls.Episode" => "episode",
        "rls.Music" => "music",
        other => panic!("unknown constant {other}"),
    }
    .to_string()
}

/// The fields of a Go struct literal (keyed), without its type name.
fn fields(v: &Value) -> Vec<(&String, &Value)> {
    v.as_object().unwrap_or_else(|| panic!("a struct wanted, got {v}")).iter().filter(|(k, _)| *k != "__type").collect()
}

/// A Go map literal.
fn go_map(v: &Value) -> BTreeMap<String, String> {
    v.get("__map")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("a map wanted, got {v}"))
        .iter()
        .map(|(k, v)| (k.clone(), st(v)))
        .collect()
}

/// A `domain.Release` literal.
fn release_of(v: &Value) -> Release {
    let mut r = Release::default();
    for (k, x) in fields(v) {
        match k.as_str() {
            "TorrentName" => r.torrent_name = st(x),
            "Title" => r.title = st(x),
            "Category" => r.category = st(x),
            "Categories" => r.categories = strs(x),
            "Freeleech" => r.freeleech = x.as_bool().unwrap(),
            "FreeleechPercent" => r.freeleech_percent = int(x),
            "Bonus" => r.bonus = strs(x),
            "Group" => r.group = st(x),
            "ReleaseTags" => r.release_tags = st(x),
            "Size" => r.size = x.as_u64().unwrap(),
            "Tags" => r.tags = strs(x),
            "Uploader" => r.uploader = st(x),
            "Origin" => r.origin = st(x),
            "Description" => r.description = st(x),
            "Year" => r.year = int(x),
            "Season" => r.season = int(x),
            "Episode" => r.episode = int(x),
            "Source" => r.source = st(x),
            "Resolution" => r.resolution = st(x),
            "Codec" => r.codec = strs(x),
            "Container" => r.container = st(x),
            "HDR" => r.hdr = strs(x),
            "Audio" => r.audio = strs(x),
            "AudioChannels" => r.audio_channels = st(x),
            "AudioFormat" => r.audio_format = st(x),
            "Bitrate" => r.bitrate = st(x),
            "HasLog" => r.has_log = x.as_bool().unwrap(),
            "HasCue" => r.has_cue = x.as_bool().unwrap(),
            "LogScore" => r.log_score = int(x),
            "Language" => r.language = strs(x),
            "Cut" => r.cut = strs(x),
            "Edition" => r.edition = strs(x),
            "Other" => r.other = strs(x),
            "Hybrid" => r.hybrid = x.as_bool().unwrap(),
            "Proper" => r.proper = x.as_bool().unwrap(),
            "Repack" => r.repack = x.as_bool().unwrap(),
            "MediaProcessing" => r.media_processing = st(x),
            "Website" => r.website = st(x),
            "NormalizedHash" => r.normalized_hash = st(x),
            "Type" => r.r#type = Type::parse(&constant(x)),
            "Artists" => r.artists = st(x),
            "TorrentTmpFile" => r.torrent_tmp_file = st(x),
            "DownloadURL" => r.download_url = st(x),
            "InfoURL" => r.info_url = st(x),
            "FilterID" => r.filter_id = int(x),
            "RawVars" => r.raw_vars = go_map(x),
            "MetaIMDB" => r.meta_imdb = st(x),
            "MetaTMDB" => r.meta_tmdb = int(x),
            "MetaTVDB" => r.meta_tvdb = int(x),
            "MetaIGDB" => r.meta_igdb = int(x),
            "Indexer" => {
                let p = x.get("__pos").and_then(Value::as_array).expect("a positional IndexerMinimal");
                r.indexer = IndexerMinimal { id: int(&p[0]), name: st(&p[1]), identifier: st(&p[2]), identifier_external: st(&p[3]) };
            }
            other => panic!("unknown Release field {other}"),
        }
    }
    r
}

/// A `domain.Filter` literal.
fn filter_of(v: &Value) -> Filter {
    let mut f = Filter::default();
    for (k, x) in fields(v) {
        match k.as_str() {
            "Name" => f.name = st(x),
            "Enabled" => f.enabled = x.as_bool().unwrap(),
            "MinSize" => f.min_size = st(x),
            "MaxSize" => f.max_size = st(x),
            "MaxDownloads" => f.max_downloads = int(x),
            "MaxDownloadsUnit" => f.max_downloads_unit = constant(x),
            "MaxDownloadsPeriod" => f.max_downloads_period = int(x),
            "MaxDownloadsWindowType" => f.max_downloads_window_type = constant(x),
            "Downloads" => f.downloads = Some(FilterDownloads { period_count: int(&x["PeriodCount"]) }),
            "MatchReleases" => f.match_releases = st(x),
            "ExceptReleases" => f.except_releases = st(x),
            "UseRegex" => f.use_regex = x.as_bool().unwrap(),
            "MatchReleaseGroups" => f.match_release_groups = st(x),
            "ExceptReleaseGroups" => f.except_release_groups = st(x),
            "MatchReleaseTags" => f.match_release_tags = st(x),
            "UseRegexReleaseTags" => f.use_regex_release_tags = x.as_bool().unwrap(),
            "MatchDescription" => f.match_description = st(x),
            "Origins" => f.origins = strs(x),
            "ExceptOrigins" => f.except_origins = strs(x),
            "Freeleech" => f.freeleech = x.as_bool().unwrap(),
            "Shows" => f.shows = st(x),
            "Seasons" => f.seasons = st(x),
            "Episodes" => f.episodes = st(x),
            "Resolutions" => f.resolutions = strs(x),
            "Codecs" => f.codecs = strs(x),
            "Sources" => f.sources = strs(x),
            "MatchHDR" => f.match_hdr = strs(x),
            "ExceptHDR" => f.except_hdr = strs(x),
            "MatchOther" => f.match_other = strs(x),
            "ExceptOther" => f.except_other = strs(x),
            "Years" => f.years = st(x),
            "Months" => f.months = st(x),
            "Days" => f.days = st(x),
            "Artists" => f.artists = st(x),
            "Albums" => f.albums = st(x),
            "MatchReleaseTypes" => f.match_release_types = strs(x),
            "Formats" => f.formats = strs(x),
            "Quality" => f.quality = strs(x),
            "Media" => f.media = strs(x),
            "PerfectFlac" => f.perfect_flac = x.as_bool().unwrap(),
            "Cue" => f.cue = x.as_bool().unwrap(),
            "Log" => f.log = x.as_bool().unwrap(),
            "LogScore" => f.log_score = int(x),
            "MatchCategories" => f.match_categories = st(x),
            "ExceptCategories" => f.except_categories = st(x),
            "MatchUploaders" => f.match_uploaders = st(x),
            "ExceptUploaders" => f.except_uploaders = st(x),
            "Tags" => f.tags = st(x),
            "ExceptTags" => f.except_tags = st(x),
            "TagsMatchLogic" => f.tags_match_logic = st(x),
            "ExceptTagsMatchLogic" => f.except_tags_match_logic = st(x),
            other => panic!("unknown Filter field {other}"),
        }
    }
    f
}

/// A rejection value as JSON, the way the saved tables hold it.
fn val_json(v: &Val) -> Value {
    match v {
        Val::Str(s) => json!(s),
        Val::List(l) => json!(l),
        Val::Int(n) => json!(n),
        Val::Uint(n) => json!(n),
        Val::Bool(b) => json!(b),
    }
}

/// Rejections as JSON, comparable with a saved `RejectionReasons{data: …}` (a `nil` list is an empty one).
fn rejections_json(rr: &RejectionReasons) -> Value {
    Value::Array(
        rr.data
            .iter()
            .map(|r| {
                let mut o = json!({ "key": r.key, "got": val_json(&r.got), "want": val_json(&r.want) });
                if !r.format.is_empty() {
                    o["format"] = json!(r.format);
                }
                o
            })
            .collect(),
    )
}

/// A saved `RejectionReasons{data: …}` with `nil` lists as empty ones.
fn want_rejections(v: &Value) -> Value {
    let mut data = v["data"].clone();
    for r in data.as_array_mut().expect("rejection data") {
        for k in ["got", "want"] {
            if r[k].is_null() {
                r[k] = json!([]);
            }
        }
    }
    data
}

// ---- filter_test.go ----

#[test]
fn filter_check_filter() {
    for c in table("filter_test/TestFilter_CheckFilter") {
        let name = st(&c["name"]);
        let mut r = release_of(&c["fields"]);
        let f = filter_of(&c["args"]["filter"]);
        let title = r.torrent_name.clone();
        r.parse_string(&title);
        let (rr, ok) = f.check_filter(&mut r);
        assert_eq!(ok, c["want"].as_bool().unwrap(), "{name}: {}", rr.string());
        assert_eq!(rejections_json(&rr), want_rejections(&c["args"]["rejectionReasons"]), "{name}");
    }
}

#[test]
fn filter_check_filter1() {
    for c in table("filter_test/TestFilter_CheckFilter1") {
        let name = st(&c["name"]);
        let mut f = filter_of(&c["fields"]);
        f.sanitize();
        let mut r = release_of(&c["args"]["__pos"][0]);
        let title = r.torrent_name.clone();
        r.parse_string(&title);
        let (rr, ok) = f.check_filter(&mut r);
        assert_eq!(rejections_json(&rr), want_rejections(&c["rejectionReasons"]), "{name}");
        assert_eq!(ok, c["wantMatch"].as_bool().unwrap(), "{name}");
    }
}

#[test]
fn filter_download_period() {
    for c in table("filter_test/TestFilter_DownloadPeriod") {
        let f = filter_of(&c["filter"]);
        let (n, unit) = f.download_period();
        assert_eq!((n, unit), (int(&c["wantN"]), st(&c["wantUnit"]).as_str()), "{}", c["name"]);
    }
}

#[test]
fn filter_validate_max_downloads() {
    for c in table("filter_test/TestFilter_ValidateMaxDownloads") {
        let f = filter_of(&c["filter"]);
        assert_eq!(f.validate().is_err(), c["wantErr"].as_bool().unwrap_or(false), "{}", c["name"]);
    }
}

#[test]
fn filter_validation() {
    for c in table("filter_test/Test_validation") {
        let f = filter_of(&c["filter"]);
        assert_eq!(f.validate().is_ok(), c["valid"].as_bool().unwrap(), "{}: {:?}", c["name"], f.validate());
    }
}

#[test]
fn filter_check_size() {
    for c in table("filter_test/Test_checkSizeFilter") {
        let f = filter_of(&c["filter"]);
        let mut rr = RejectionReasons::new();
        let got = f.check_release_size(c["releaseSize"].as_u64().unwrap(), &mut rr);
        match c.get("wantErr").and_then(Value::as_str) {
            Some(e) if !e.is_empty() => assert_eq!(got.as_ref().err().map(String::as_str), Some(e), "{}", c["name"]),
            _ => assert!(got.is_ok(), "{}: {got:?}", c["name"]),
        }
        assert_eq!(got.unwrap_or(false), c["want"].as_bool().unwrap(), "{}", c["name"]);
    }
}

#[test]
fn filter_is_perfect_flac() {
    for c in table("filter_test/TestFilter_IsPerfectFLAC") {
        let r = release_of(&c["release"]);
        let (why, ok) = Filter::default().is_perfect_flac(&r);
        assert_eq!(ok, c["wantOk"].as_bool().unwrap(), "{}", c["name"]);
        assert_eq!(why, strs(c.get("wantRejections").unwrap_or(&Value::Null)), "{}", c["name"]);
    }
}

/// Runs a table of `(args) -> want` cases of a list helper.
fn helper_table(path: &str, f: impl Fn(&Value) -> bool) {
    for c in table(path) {
        assert_eq!(f(&c["args"]), c["want"].as_bool().unwrap(), "{path} {}: {}", c["name"], c["args"]);
    }
}

#[test]
fn filter_list_helpers() {
    helper_table("filter_test/Test_containsMatch", |a| filter::contains_match(&strs(&a["tags"]), &strs(&a["filters"])));
    helper_table("filter_test/Test_containsAllMatch", |a| filter::contains_all_match(&strs(&a["tags"]), &strs(&a["filters"])));
    helper_table("filter_test/Test_contains", |a| filter::contains(&st(&a["tag"]), &st(&a["filter"])));
    helper_table("filter_test/Test_containsSlice", |a| filter::contains_slice(&st(&a["tag"]), &strs(&a["filters"])));
    helper_table("filter_test/Test_containsAny", |a| filter::contains_any(&strs(&a["tags"]), &st(&a["filter"])));
    helper_table("filter_test/Test_containsAll", |a| filter::contains_all(&strs(&a["tags"]), &st(&a["filter"])));
    helper_table("filter_test/Test_sliceContainsSlice", |a| filter::slice_contains_slice(&strs(&a["tags"]), &strs(&a["filters"])));
    helper_table("filter_test/Test_containsIntStrings", |a| filter::contains_int_strings(int(&a["value"]), &st(&a["filterList"])));
    helper_table("filter_test/Test_matchRegex", |a| filter::match_regex(&st(&a["tag"]), &st(&a["filter"])));
    helper_table("filter_test/Test_containsFuzzy", |a| filter::contains_fuzzy(&st(&a["tag"]), &st(&a["filter"])));
}

// ---- release_test.go ----

#[test]
fn release_parse() {
    for c in table("release_test/TestRelease_Parse") {
        let mut r = release_of(&c["fields"]);
        let title = r.torrent_name.clone();
        r.parse_string(&title);
        assert_eq!(r, release_of(&c["want"]), "{}", c["name"]);
    }
}

#[test]
fn release_map_vars() {
    for c in table("release_test/TestRelease_MapVars") {
        let mut r = release_of(&c["fields"]);
        let a = &c["args"];
        let unit = a.get("forceSizeUnit").map(st).unwrap_or_default();
        let units = a.get("sizeUnits").map(constant).unwrap_or_default();
        let _ = r.map_vars(&go_map(&a["varMap"]), &unit, &units);
        r.raw_vars.clear();
        assert_eq!(r, release_of(&c["want"]), "{}", c["name"]);
    }
}

#[test]
fn release_normalize_imdb_id() {
    for c in table("release_test/TestNormalizeIMDBID") {
        assert_eq!(release::normalize_imdb_id(&st(&c["id"])), st(&c["want"]), "{}", c["id"]);
    }
}

#[test]
fn release_to_binary_size_unit() {
    for c in table("release_test/Test_toBinarySizeUnit") {
        assert_eq!(release::to_binary_size_unit(&st(&c["size"])), st(&c["want"]), "{}", c["size"]);
    }
}

#[test]
fn release_split_any() {
    for c in table("release_test/TestSplitAny") {
        assert_eq!(release::split_any(&st(&c["args"]["s"]), &st(&c["args"]["seps"])), strs(&c["want"]), "{}", c["name"]);
    }
}

#[test]
fn release_parse_string_never_fails() {
    for c in table("release_test/TestRelease_ParseString") {
        let mut r = Release::default();
        r.parse_string(&st(&c["args"]["title"]));
        assert_eq!(r.torrent_name, st(&c["args"]["title"]));
    }
}

#[test]
fn release_unique_tags() {
    for c in table("release_test/Test_getUniqueTags") {
        let got = release::get_unique_tags(strs(&c["args"]["target"]), &strs(&c["args"]["source"]));
        assert_eq!(got, strs(&c["want"]), "{}", c["name"]);
    }
}

#[test]
fn release_hash() {
    for c in table("release_test/TestRelease_Hash") {
        let title = st(&c["fields"]["TorrentName"]);
        let mut r = Release { torrent_name: title.clone(), ..Release::default() };
        r.parse_string(&title);
        assert_eq!(r.normalized_hash, st(&c["want"]), "{}: {title}", c["name"]);
    }
}

// ---- releasetags_test.go ----

/// A `ReleaseTags` literal.
fn release_tags_of(v: &Value) -> ReleaseTags {
    let mut t = ReleaseTags::default();
    for (k, x) in fields(v) {
        match k.as_str() {
            "Audio" => t.audio = strs(x),
            "AudioBitrate" => t.audio_bitrate = st(x),
            "AudioFormat" => t.audio_format = st(x),
            "LogScore" => t.log_score = int(x),
            "HasLog" => t.has_log = x.as_bool().unwrap(),
            "HasCue" => t.has_cue = x.as_bool().unwrap(),
            "Bonus" => t.bonus = strs(x),
            "Channels" => t.channels = st(x),
            "Codec" => t.codec = st(x),
            "Container" => t.container = st(x),
            "HDR" => t.hdr = strs(x),
            "Origin" => t.origin = st(x),
            "Other" => t.other = strs(x),
            "Resolution" => t.resolution = st(x),
            "Source" => t.source = st(x),
            other => panic!("unknown ReleaseTags field {other}"),
        }
    }
    t
}

#[test]
fn release_tags_parse() {
    for c in table("releasetags_test/TestParseReleaseTags") {
        assert_eq!(releasetags::parse_release_tags(&strs(&c["args"]["tags"])), release_tags_of(&c["want"]), "{}", c["name"]);
    }
}

#[test]
fn release_tags_parse_string() {
    for c in table("releasetags_test/TestParseReleaseTagString") {
        let got = releasetags::parse_release_tag_string(&st(&c["args"]["tags"]));
        assert_eq!(got, release_tags_of(&c["want"]), "{}: {}", c["name"], c["args"]["tags"]);
    }
}

#[test]
fn release_tags_clean() {
    for c in table("releasetags_test/Test_cleanReleaseTags") {
        assert_eq!(releasetags::clean_release_tags(&st(&c["args"]["tagString"])), st(&c["want"]), "{}", c["name"]);
    }
}

// ---- macros_test.go ----

#[test]
fn macros_parse() {
    for c in table("macros_test/TestMacros_Parse") {
        let m = Macro::new(&release_of(&c["release"]));
        let got = m.parse(&st(&c["args"]["text"]));
        let year = m.get("CurrentYear").cloned();
        assert_eq!(year.map(|y| y.text()), Some(chrono::Datelike::year(&chrono::Local::now()).to_string()));
        let want = match &c["want"] {
            Value::String(s) => s.clone(),
            // fmt.Sprintf("mock1-%v-race", currentTime.Year())
            w => st(&w["args"][0]).replace("%v", &chrono::Datelike::year(&chrono::Local::now()).to_string()),
        };
        if c["wantErr"].as_bool().unwrap() {
            assert!(got.is_err(), "{}: {got:?}", c["name"]);
        } else {
            assert!(got.is_ok(), "{}: {got:?}", c["name"]);
        }
        assert_eq!(got.unwrap_or_default(), want, "{}", c["name"]);
    }
}

// ---- rejections_test.go ----

/// A saved `RejectionReasons` from its `data`.
fn rejections_of(v: &Value) -> RejectionReasons {
    let mut rr = RejectionReasons::new();
    for r in v["data"].as_array().expect("rejection data") {
        let val = |x: &Value| match x {
            Value::String(s) => Val::Str(s.clone()),
            Value::Array(_) => Val::List(strs(x)),
            Value::Bool(b) => Val::Bool(*b),
            Value::Number(n) => Val::Int(n.as_i64().unwrap()),
            other => panic!("unknown rejection value {other}"),
        };
        match r.get("format").and_then(Value::as_str) {
            Some(f) => rr.addf(&st(&r["key"]), f, val(&r["got"]), val(&r["want"])),
            None => rr.add(&st(&r["key"]), val(&r["got"]), val(&r["want"])),
        }
    }
    rr
}

#[test]
fn rejections_string() {
    for c in table("rejections_test/TestRejectionReasons_String") {
        assert_eq!(rejections_of(&c["fields"]).string(), st(&c["want"]), "{}", c["name"]);
    }
    for c in table("rejections_test/TestRejectionReasons_StringTruncated") {
        assert_eq!(rejections_of(&c["fields"]).string_truncated(), st(&c["want"]), "{}", c["name"]);
    }
}

// ---- pkg/wildcard ----

#[test]
fn wildcard_match() {
    for c in table("wildcard_test/TestMatch") {
        assert_eq!(
            wildcard::is_match(&st(&c["pattern"]), &st(&c["text"])),
            c["matched"].as_bool().unwrap(),
            "{} {}",
            c["pattern"],
            c["text"]
        );
    }
    for c in table("wildcard_test/TestMatchSimple") {
        assert_eq!(wildcard::match_simple(&st(&c[0]), &st(&c[1])), c[2].as_bool().unwrap(), "{c}");
    }
    for c in table("wildcard_test/TestMatchSliceSimple") {
        assert_eq!(wildcard::match_slice_simple(&strs(&c[0]), &st(&c[1])), c[2].as_bool().unwrap(), "{c}");
    }
    for c in table("wildcard_test/matchSliceCases") {
        assert_eq!(wildcard::match_slice(&strs(&c[0]), &st(&c[1])), c[2].as_bool().unwrap(), "{c}");
    }
}

// ---- pkg/sanitize ----

#[test]
fn sanitize_strings() {
    for c in table("sanitize_test/TestStringAndFilterString") {
        let input = st(&c["input"]);
        assert_eq!(sanitize::string(&input), st(&c["expectedString"]), "{}", c["name"]);
        assert_eq!(sanitize::filter_string(&input), st(&c["expectedFilter"]), "{}", c["name"]);
    }
}
