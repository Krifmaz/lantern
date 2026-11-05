// Ported from autobrr/rls v0.10.0 (rls_test.go: TestParseRelease, rlsTests, buildRls, groupInfos), MIT.
//! rls's own release-name tests: every name in rls's `tests.yaml` (saved under
//! `tests/fixtures/upstream/rls/`) is read by Lantern's port and must give exactly what rls gives.

use server::releases::rls::{lex, parse::TagParser, taginfo, Release, TagType, Type};
use std::collections::BTreeMap;
use std::sync::Arc;

/// rls's test names and what each must give (field name to value, empty values left out).
fn rls_tests() -> Vec<(String, BTreeMap<String, String>)> {
    let text = include_str!("fixtures/upstream/rls/tests.yaml");
    let mut tests: Vec<(String, BTreeMap<String, String>)> = Vec::new();
    for line in text.lines() {
        if line.starts_with('"') {
            let n = line.rfind('"').expect("a closing quote");
            tests.push((unquote(&line[..=n]), BTreeMap::new()));
        } else if let Some(rest) = line.strip_prefix("  ") {
            let (k, v) = rest.split_once(": ").expect("a field");
            let v = if v.starts_with('"') { unquote(v) } else { v.to_string() };
            tests.last_mut().expect("a test").1.insert(k.to_string(), v);
        } else if !line.starts_with('#') && !line.is_empty() {
            panic!("unknown line {line:?}");
        }
    }
    tests
}

/// Go's `strconv.Unquote` for a double-quoted string.
fn unquote(s: &str) -> String {
    let inner = &s[1..s.len() - 1];
    let mut out = String::new();
    let mut c = inner.chars();
    while let Some(ch) = c.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match c.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('f') => out.push('\x0c'),
            Some('v') => out.push('\x0b'),
            Some('a') => out.push('\x07'),
            Some('b') => out.push('\x08'),
            Some('x') => {
                let h: String = c.by_ref().take(2).collect();
                out.push(char::from(u8::from_str_radix(&h, 16).expect("a hex byte")));
            }
            Some('u') => {
                let h: String = c.by_ref().take(4).collect();
                out.push(char::from_u32(u32::from_str_radix(&h, 16).expect("hex")).expect("a char"));
            }
            Some('U') => {
                let h: String = c.by_ref().take(8).collect();
                out.push(char::from_u32(u32::from_str_radix(&h, 16).expect("hex")).expect("a char"));
            }
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

/// rls's `groupInfos`: the extra groups its tests add.
fn group_infos() -> taginfo::Infos {
    let groups = [("CODEX", "game"), ("DARKSiDERS", "game"), ("D-Z0N3", "movie"), ("MrSeeN-SiMPLE", "")]
        .iter()
        .map(|(tag, typ)| Arc::new(taginfo::Taginfo::new(tag, "", "", "", typ, "").expect("a group")))
        .collect();
    [("group".to_string(), groups)].into_iter().collect()
}

/// rls's `buildRls`: a release as the test file writes it.
fn build_rls(r: &Release) -> BTreeMap<String, String> {
    let eps = r.series_episodes();
    let series_episodes =
        if eps.len() > 1 { eps.iter().map(|(s, e)| format!("S{s:02}E{e:02}")).collect::<Vec<_>>().join(" ") } else { String::new() };
    let int = |i: i32| if i == 0 { String::new() } else { i.to_string() };
    let unused: Vec<String> = r.unused().iter().map(|t| t.normalize()).collect();
    let fields = [
        ("type", r.r#type.as_str().to_string()),
        ("artist", r.artist.clone()),
        ("title", r.title.clone()),
        ("subtitle", r.subtitle.clone()),
        ("alt", r.alt.clone()),
        ("platform", r.platform.clone()),
        ("arch", r.arch.clone()),
        ("source", r.source.clone()),
        ("resolution", r.resolution.clone()),
        ("collection", r.collection.clone()),
        ("year", int(r.year)),
        ("month", int(r.month)),
        ("day", int(r.day)),
        ("series", int(r.series)),
        ("episode", int(r.episode)),
        ("seriesEpisodes", series_episodes),
        ("version", r.version.clone()),
        ("disc", r.disc.clone()),
        ("codec", r.codec.join(" ")),
        ("hdr", r.hdr.join(" ")),
        ("bitdepth", r.bit_depth.clone()),
        ("audio", r.audio.join(" ")),
        ("channels", r.channels.clone()),
        ("other", r.other.join(" ")),
        ("cut", r.cut.join(" ")),
        ("edition", r.edition.join(" ")),
        ("language", r.language.join(" ")),
        ("size", r.size.clone()),
        ("region", r.region.clone()),
        ("container", r.container.clone()),
        ("genre", r.genre.clone()),
        ("id", r.id.clone()),
        ("group", r.group.clone()),
        ("meta", r.meta.join(" ")),
        ("site", r.site.clone()),
        ("sum", r.sum.clone()),
        ("pass", r.pass.clone()),
        ("req", if r.req { "1".into() } else { String::new() }),
        ("ext", r.ext.clone()),
        ("unused", unused.join(" ")),
    ];
    fields.into_iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| (k.to_string(), v)).collect()
}

/// rls's TestParseRelease: every name in `tests.yaml` gives what rls gives.
#[test]
fn rls_parses_every_test_name_like_rls() {
    let p = TagParser::new(taginfo::all(&[group_infos()]), lex::default_lexers());
    let tests = rls_tests();
    assert!(tests.len() > 400, "the test file has {} names", tests.len());
    let mut failures = Vec::new();
    for (i, (s, exp)) in tests.iter().enumerate() {
        let r = p.parse_release(s.as_bytes());
        if !s.is_empty() && r.tags().is_empty() {
            failures.push(format!("test {i} {s:?}: no tags"));
            continue;
        }
        if r.original() != *s {
            failures.push(format!("test {i} {s:?}: original {:?}", r.original()));
        }
        let dates = r.tags().iter().filter(|t| t.is(&[TagType::Date])).count();
        if dates > 1 && r.r#type != Type::Magazine {
            failures.push(format!("test {i} {s:?}: {dates} date tags"));
        }
        let got = build_rls(&r);
        if &got != exp {
            failures.push(format!("test {i} {s:?}:\n  want {exp:?}\n  got  {got:?}"));
        }
    }
    assert!(failures.is_empty(), "{} of {} names differ:\n{}", failures.len(), tests.len(), failures.join("\n"));
}
