//! Lantern's own cases for the parts of autobrr's filters and macros that autobrr's test tables leave out
//! (`tests/upstream_autobrr.rs` runs those). The expected values are read from autobrr v1.90.0's
//! `internal/domain/filter.go` and `macros.go`.

use serde_json::{json, Value};
use server::releases::filter::Filter;
use server::releases::macros::Macro;
use server::releases::rejections::Val;
use server::releases::release::{IndexerMinimal, Release};
use server::releases::rls::Type;
use std::collections::BTreeMap;

/// A rejection value as JSON.
fn val_json(v: &Val) -> Value {
    match v {
        Val::Str(s) => json!(s),
        Val::List(l) => json!(l),
        Val::Int(n) => json!(n),
        Val::Uint(n) => json!(n),
        Val::Bool(b) => json!(b),
    }
}

/// The rejections of filter `f` on release `r`, as JSON.
fn rejects(f: &Filter, mut r: Release) -> Value {
    let (rr, ok) = f.check_filter(&mut r);
    assert_eq!(ok, rr.is_empty());
    Value::Array(rr.data.iter().map(|r| json!({ "key": r.key, "got": val_json(&r.got), "want": val_json(&r.want) })).collect())
}

/// A release with what `f` sets.
fn release(f: impl FnOnce(&mut Release)) -> Release {
    let mut r = Release::default();
    f(&mut r);
    r
}

#[test]
fn announce_types_bonus_and_freeleech_percent() {
    let f = Filter { announce_types: vec!["NEW".into()], ..Filter::default() };
    assert_eq!(rejects(&f, release(|r| r.announce_type = "NEW".into())), json!([]));
    assert_eq!(
        rejects(&f, release(|r| r.announce_type = "PROMO".into())),
        json!([{ "key": "match announce type", "got": "PROMO", "want": ["NEW"] }])
    );

    let f = Filter { bonus: vec!["Freeleech".into()], ..Filter::default() };
    assert_eq!(rejects(&f, release(|r| r.bonus = vec!["Freeleech".into()])), json!([]));
    assert_eq!(rejects(&f, Release::default()), json!([{ "key": "bonus", "got": [], "want": ["Freeleech"] }]));

    let f = Filter { freeleech_percent: "25,50-100".into(), ..Filter::default() };
    assert_eq!(rejects(&f, release(|r| r.freeleech_percent = 25)), json!([]));
    assert_eq!(rejects(&f, release(|r| r.freeleech_percent = 75)), json!([]));
    // as in autobrr, a range that doesn't hold ends the search
    let f = Filter { freeleech_percent: "50-100,25".into(), ..Filter::default() };
    assert_eq!(rejects(&f, release(|r| r.freeleech_percent = 25)), json!([{ "key": "freeleech percent", "got": 25, "want": "50-100,25" }]));
}

#[test]
fn containers_and_languages() {
    let f = Filter { containers: vec!["mkv".into()], ..Filter::default() };
    assert_eq!(rejects(&f, release(|r| r.container = "MKV".into())), json!([]));
    assert_eq!(rejects(&f, release(|r| r.container = "mp4".into())), json!([{ "key": "container", "got": "mp4", "want": ["mkv"] }]));

    let f = Filter { match_language: vec!["FRENCH".into()], except_language: vec!["GERMAN".into()], ..Filter::default() };
    assert_eq!(rejects(&f, release(|r| r.language = vec!["french".into()])), json!([]));
    assert_eq!(
        rejects(&f, release(|r| r.language = vec!["GERMAN".into()])),
        json!([
            { "key": "match language", "got": ["GERMAN"], "want": ["FRENCH"] },
            { "key": "except language", "got": ["GERMAN"], "want": ["GERMAN"] },
        ])
    );
}

#[test]
fn record_labels() {
    let f = Filter { match_record_labels: "Warp,Ninja*".into(), except_record_labels: "Ninja Tune".into(), ..Filter::default() };
    assert_eq!(rejects(&f, release(|r| r.record_label = "Warp".into())), json!([]));
    assert_eq!(
        rejects(&f, release(|r| r.record_label = "Ninja Tune".into())),
        json!([{ "key": "except record labels", "got": "Ninja Tune", "want": "Ninja Tune" }])
    );
    assert_eq!(
        rejects(&f, release(|r| r.record_label = "Sub Pop".into())),
        json!([{ "key": "match record labels", "got": "Sub Pop", "want": "Warp,Ninja*" }])
    );
    // RED and OPS announce no label: the site is asked later
    let mut r = release(|r| r.indexer.identifier = "redacted".into());
    let (rr, ok) = f.check_filter(&mut r);
    assert!(ok && rr.is_empty() && r.additional_record_label_check_required);
}

#[test]
fn release_tags_and_descriptions() {
    let f = Filter { except_release_tags: "lossless".into(), ..Filter::default() };
    assert_eq!(
        rejects(&f, release(|r| r.release_tags = "FLAC / Lossless".into())),
        json!([{ "key": "except release tags", "got": "FLAC / Lossless", "want": "lossless" }])
    );

    let f = Filter { match_description: "*black?metal*".into(), except_description: "pop".into(), ..Filter::default() };
    assert_eq!(rejects(&f, release(|r| r.description = "genre: black metal".into())), json!([]));
    assert_eq!(
        rejects(&f, release(|r| r.description = "genre: black metal, pop".into())),
        json!([{ "key": "except description", "got": "genre: black metal, pop", "want": "pop" }])
    );
    let f = Filter { use_regex_description: true, match_description: "^metal".into(), ..Filter::default() };
    assert_eq!(
        rejects(&f, release(|r| r.description = "black metal".into())),
        json!([{ "key": "match description: REGEX", "got": "black metal", "want": "^metal" }])
    );
    assert_eq!(rejects(&f, release(|r| r.description = "Metal and more".into())), json!([]));
}

#[test]
fn seeders_and_leechers() {
    let f = Filter { min_seeders: 5, max_seeders: 10, min_leechers: 1, max_leechers: 3, ..Filter::default() };
    let peers = |s: i64, l: i64| {
        release(|r| {
            r.seeders = s;
            r.leechers = l;
        })
    };
    assert_eq!(
        rejects(&f, peers(3, 0)),
        json!([{ "key": "min seeders", "got": 3, "want": 5 }, { "key": "min leechers", "got": 0, "want": 1 }])
    );
    assert_eq!(
        rejects(&f, peers(20, 4)),
        json!([{ "key": "max seeders", "got": 20, "want": 10 }, { "key": "max leechers", "got": 4, "want": 3 }])
    );
    assert_eq!(rejects(&f, peers(7, 2)), json!([]));
}

#[test]
fn every_macro_variable() {
    let r = Release {
        artists: "Artist".into(),
        audio: vec!["FLAC".into(), "Lossless".into()],
        audio_channels: "2.0".into(),
        audio_format: "FLAC".into(),
        bitrate: "Lossless".into(),
        bonus: vec!["Freeleech".into()],
        categories: vec!["Music".into()],
        category: "Music/Album".into(),
        codec: vec!["x264".into()],
        container: "mkv".into(),
        raw_cookie: "c=1".into(),
        description: "desc".into(),
        download_url: "https://dl".into(),
        episode: 2,
        filter_id: 7,
        filter_name: "F".into(),
        freeleech: true,
        freeleech_percent: 100,
        group: "GRP".into(),
        group_id: "g1".into(),
        hdr: vec!["DV".into(), "HDR10".into()],
        has_cue: true,
        has_log: true,
        implementation: "RSS".into(),
        indexer: IndexerMinimal { id: 1, name: "Name".into(), identifier: "ident".into(), identifier_external: "ext".into() },
        info_url: "https://info".into(),
        language: vec!["ENGLiSH".into()],
        leechers: 4,
        log_score: 100,
        magnet_uri: "magnet:?x".into(),
        meta_imdb: "tt0133093".into(),
        meta_tmdb: 603,
        meta_tvdb: 81189,
        meta_igdb: 1020,
        origin: "P2P".into(),
        other: vec!["REMUX".into()],
        pre_time: "10s".into(),
        protocol: "usenet".into(),
        proper: true,
        region: "R1".into(),
        release_tags: "FLAC / Lossless".into(),
        resolution: "1080p".into(),
        season: 1,
        seeders: 9,
        size: 3_834_225_472,
        skip_duplicate_profile_id: 3,
        skip_duplicate_profile_name: "dupe".into(),
        source: "WEB".into(),
        tags: vec!["rock".into(), "pop".into()],
        title: "Title".into(),
        torrent_data_raw_bytes: vec![1, 2],
        torrent_hash: "abc".into(),
        torrent_id: "42".into(),
        torrent_name: "Name".into(),
        torrent_tmp_file: "/tmp/x".into(),
        r#type: Type::Music,
        uploader: "up".into(),
        record_label: "rl".into(),
        website: "AMZN".into(),
        raw_vars: BTreeMap::from([("k".to_string(), "v".to_string())]),
        year: 2024,
        month: 5,
        day: 6,
        ..Release::default()
    };
    let want = [
        ("Artists", "Artist"),
        ("Audio", "[FLAC Lossless]"),
        ("AudioChannels", "2.0"),
        ("AudioFormat", "FLAC"),
        ("Bitrate", "Lossless"),
        ("Bonus", "[Freeleech]"),
        ("Categories", "[Music]"),
        ("Category", "Music/Album"),
        ("Codec", "[x264]"),
        ("Container", "mkv"),
        ("Cookie", "c=1"),
        ("Description", "desc"),
        ("DownloadUrl", "https://dl"),
        ("Episode", "2"),
        ("FilterID", "7"),
        ("FilterName", "F"),
        ("Freeleech", "true"),
        ("FreeleechPercent", "100"),
        ("Group", "GRP"),
        ("GroupID", "g1"),
        ("HDR", "DV, HDR10"),
        ("HasCue", "true"),
        ("HasLog", "true"),
        ("Implementation", "RSS"),
        ("Indexer", "ident"),
        ("IndexerIdentifier", "ident"),
        ("IndexerIdentifierExternal", "ext"),
        ("IndexerName", "Name"),
        ("InfoUrl", "https://info"),
        ("IsDuplicate", "false"),
        ("Language", "[ENGLiSH]"),
        ("Leechers", "4"),
        ("LogScore", "100"),
        ("MagnetURI", "magnet:?x"),
        ("MetaIMDB", "tt0133093"),
        ("MetaTMDB", "603"),
        ("MetaTVDB", "81189"),
        ("MetaIGDB", "1020"),
        ("Origin", "P2P"),
        ("Other", "[REMUX]"),
        ("PreTime", "10s"),
        ("Protocol", "usenet"),
        ("Proper", "true"),
        ("Region", "R1"),
        ("ReleaseTags", "FLAC / Lossless"),
        ("Repack", "false"),
        ("Resolution", "1080p"),
        ("Season", "1"),
        ("Seeders", "9"),
        ("Size", "3834225472"),
        ("SizeString", "3.8 GB"),
        ("SkipDuplicateProfileID", "3"),
        ("SkipDuplicateProfileName", "dupe"),
        ("Source", "WEB"),
        ("Tags", "rock, pop"),
        ("Title", "Title"),
        ("TorrentDataRawBytes", "[1 2]"),
        ("TorrentHash", "abc"),
        ("TorrentID", "42"),
        ("TorrentName", "Name"),
        ("TorrentPathName", "/tmp/x"),
        ("TorrentUrl", "https://dl"),
        ("TorrentTmpFile", "/tmp/x"),
        ("Type", "music"),
        ("Uploader", "up"),
        ("RecordLabel", "rl"),
        ("RawVars", "map[k:v]"),
        ("Website", "AMZN"),
        ("Year", "2024"),
        ("Month", "5"),
        ("Day", "6"),
    ];
    let m = Macro::new(&r);
    for (name, value) in want {
        assert_eq!(m.parse(&format!("{{{{ .{name} }}}}")).unwrap(), value, "{name}");
    }
    let now = chrono::Local::now();
    let num = |n: &str| m.parse(&format!("{{{{ .{n} }}}}")).unwrap().parse::<i64>().unwrap();
    assert_eq!(num("CurrentYear"), i64::from(chrono::Datelike::year(&now)));
    assert!((1..=12).contains(&num("CurrentMonth")) && (1..=31).contains(&num("CurrentDay")));
    assert!((0..24).contains(&num("CurrentHour")) && (0..60).contains(&num("CurrentMinute")));
    assert!((0..60).contains(&num("CurrentSecond")));
    assert!((num("CurrentTimeUnixMS") - now.timestamp_millis()).abs() < 60_000);
    assert!((num("CurrentTimeUnixMS") - num("CurrentUnixTimeMS")).abs() < 60_000);
    assert_eq!(m.parse("{{ .RawVars.missing }}").unwrap(), "<no value>");
}

#[test]
fn download_windows() {
    use chrono::{Local, TimeZone};
    // Wednesday 2026-11-04 15:42:10 local time
    let now = Local.with_ymd_and_hms(2026, 11, 4, 15, 42, 10).single().expect("a local time");
    let start = |unit: &str, window: &str, period: i64| {
        let f = Filter {
            max_downloads: 1,
            max_downloads_unit: unit.into(),
            max_downloads_window_type: window.into(),
            max_downloads_period: period,
            ..Filter::default()
        };
        f.downloads_window_start(now).map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
    };
    assert_eq!(start("MINUTE", "", 0).as_deref(), Some("2026-11-04 15:42:00"));
    assert_eq!(start("HOUR", "FIXED", 0).as_deref(), Some("2026-11-04 15:00:00"));
    assert_eq!(start("DAY", "", 0).as_deref(), Some("2026-11-04 00:00:00"));
    // SQLite's 'weekday 0', '-7 days': the Sunday before
    assert_eq!(start("WEEK", "", 0).as_deref(), Some("2026-11-01 00:00:00"));
    assert_eq!(start("MONTH", "", 0).as_deref(), Some("2026-11-01 00:00:00"));
    assert_eq!(start("EVER", "", 0), None);
    assert_eq!(start("HOUR", "ROLLING", 2).as_deref(), Some("2026-11-04 13:42:10"));
    assert_eq!(start("WEEK", "ROLLING", 1).as_deref(), Some("2026-10-28 15:42:10"));
    assert_eq!(start("MONTH", "ROLLING", 1).as_deref(), Some("2026-10-04 15:42:10"));
}
