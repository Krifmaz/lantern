//! The Torznab / Newznab answers Lantern gives apps: an indexer's capabilities (`t=caps`) and search results as
//! RSS. Sonarr, Radarr, Lidarr, Readarr, Jackett-style clients and other tools read these.

use super::categories;
use super::release::{Protocol, Release};
use chrono::Utc;

/// What one indexer (or all of them) can search for, for `t=caps`.
#[derive(Debug, Clone, Default)]
pub struct Caps {
    /// The indexer's name.
    pub title: String,
    /// search mode (`search`, `tv-search` …) → parameters.
    pub modes: Vec<(String, Vec<String>)>,
    /// The categories: (id, name), standard and the site's own (100000+).
    pub categories: Vec<(u32, String)>,
}

/// Escapes text for XML.
pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            c if (c as u32) < 0x20 && c != '\n' && c != '\t' && c != '\r' => {}
            c => o.push(c),
        }
    }
    o
}

/// The `t=caps` answer.
pub fn caps_xml(c: &Caps) -> String {
    let mut x = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<caps>\n");
    x.push_str(&format!("  <server title=\"{}\" />\n  <limits default=\"100\" max=\"100\" />\n  <searching>\n", esc(&c.title)));
    let has = |m: &str| c.modes.iter().find(|(k, _)| k == m);
    for (tag, mode) in [
        ("search", "search"),
        ("tv-search", "tv-search"),
        ("movie-search", "movie-search"),
        ("music-search", "music-search"),
        ("audio-search", "music-search"),
        ("book-search", "book-search"),
    ] {
        match has(mode) {
            Some((_, params)) => {
                let mut p = params.clone();
                if p.is_empty() {
                    p.push("q".into());
                }
                x.push_str(&format!("    <{tag} available=\"yes\" supportedParams=\"{}\" />\n", esc(&p.join(","))));
            }
            None => x.push_str(&format!("    <{tag} available=\"no\" supportedParams=\"q\" />\n")),
        }
    }
    x.push_str("  </searching>\n  <categories>\n");
    let mut parents: Vec<u32> = c.categories.iter().map(|(id, _)| categories::parent(*id)).filter(|p| *p < 100000).collect();
    parents.sort_unstable();
    parents.dedup();
    for p in parents {
        let name = categories::name_of(p).unwrap_or("Other");
        x.push_str(&format!("    <category id=\"{p}\" name=\"{}\">\n", esc(name)));
        let mut subs: Vec<&(u32, String)> =
            c.categories.iter().filter(|(id, _)| *id != p && *id < 100000 && categories::parent(*id) == p).collect();
        subs.sort_by_key(|(id, _)| *id);
        subs.dedup_by_key(|(id, _)| *id);
        for (id, n) in subs {
            x.push_str(&format!("      <subcat id=\"{id}\" name=\"{}\" />\n", esc(n)));
        }
        x.push_str("    </category>\n");
    }
    let mut own: Vec<&(u32, String)> = c.categories.iter().filter(|(id, _)| *id >= 100000).collect();
    own.sort_by_key(|(id, _)| *id);
    own.dedup_by_key(|(id, _)| *id);
    for (id, n) in own {
        x.push_str(&format!("    <category id=\"{id}\" name=\"{}\" />\n", esc(n)));
    }
    x.push_str("  </categories>\n</caps>\n");
    x
}

/// A search answer: `releases` as RSS, each link going through Lantern (`link_of` makes it).
pub fn results_xml(title: &str, self_url: &str, releases: &[Release], link_of: &dyn Fn(&Release) -> String) -> String {
    let mut x = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\" xmlns:atom=\"http://www.w3.org/2005/Atom\" xmlns:torznab=\"http://torznab.com/schemas/2015/feed\" xmlns:newznab=\"http://www.newznab.com/DTD/2010/feeds/attributes/\">\n<channel>\n",
    );
    x.push_str(&format!(
        "  <atom:link href=\"{}\" rel=\"self\" type=\"application/rss+xml\" />\n  <title>{}</title>\n  <description>{} through Lantern</description>\n  <link>{}</link>\n  <language>en-US</language>\n  <category>search</category>\n",
        esc(self_url),
        esc(title),
        esc(title),
        esc(self_url)
    ));
    for r in releases {
        let usenet = r.protocol == Protocol::Usenet;
        let ns = if usenet { "newznab" } else { "torznab" };
        let link = link_of(r);
        let date = r.date.unwrap_or_else(Utc::now).format("%a, %d %b %Y %H:%M:%S %z");
        x.push_str("  <item>\n");
        x.push_str(&format!("    <title>{}</title>\n", esc(&r.title)));
        x.push_str(&format!("    <guid>{}</guid>\n", esc(&r.guid)));
        x.push_str(&format!("    <jackettindexer id=\"{}\">{}</jackettindexer>\n", r.indexer_id, esc(&r.indexer)));
        x.push_str(&format!("    <prowlarrindexer id=\"{}\">{}</prowlarrindexer>\n", r.indexer_id, esc(&r.indexer)));
        if !r.details.is_empty() {
            x.push_str(&format!("    <comments>{}</comments>\n", esc(&r.details)));
        }
        x.push_str(&format!("    <pubDate>{date}</pubDate>\n"));
        if let Some(s) = r.size {
            x.push_str(&format!("    <size>{s}</size>\n"));
        }
        if !r.description.is_empty() {
            x.push_str(&format!("    <description>{}</description>\n", esc(&r.description)));
        }
        x.push_str(&format!("    <link>{}</link>\n", esc(&link)));
        for c in &r.categories {
            x.push_str(&format!("    <category>{c}</category>\n"));
        }
        let mime = if usenet { "application/x-nzb" } else { "application/x-bittorrent" };
        x.push_str(&format!("    <enclosure url=\"{}\" length=\"{}\" type=\"{mime}\" />\n", esc(&link), r.size.unwrap_or(0)));
        let mut attr = |k: &str, v: String| x.push_str(&format!("    <{ns}:attr name=\"{k}\" value=\"{}\" />\n", esc(&v)));
        for c in &r.categories {
            attr("category", c.to_string());
        }
        if let Some(s) = r.size {
            attr("size", s.to_string());
        }
        if let Some(f) = r.files {
            attr("files", f.to_string());
        }
        if let Some(g) = r.grabs {
            attr("grabs", g.to_string());
        }
        if !usenet {
            if let Some(s) = r.seeders {
                attr("seeders", s.to_string());
                attr("peers", (s + r.leechers.unwrap_or(0)).to_string());
            }
            if !r.infohash.is_empty() {
                attr("infohash", r.infohash.clone());
            }
            if !r.magnet.is_empty() {
                attr("magneturl", r.magnet.clone());
            }
            attr("downloadvolumefactor", r.download_factor.unwrap_or(1.0).to_string());
            attr("uploadvolumefactor", r.upload_factor.unwrap_or(1.0).to_string());
            if let Some(m) = r.minimum_ratio {
                attr("minimumratio", m.to_string());
            }
            if let Some(m) = r.minimum_seed_time {
                attr("minimumseedtime", m.to_string());
            }
        }
        if let Some(i) = r.imdb {
            attr("imdb", format!("{i:07}"));
            attr("imdbid", format!("tt{i:07}"));
        }
        if let Some(i) = r.tvdb {
            attr("tvdbid", i.to_string());
        }
        if let Some(i) = r.tmdb {
            attr("tmdbid", i.to_string());
        }
        if let Some(i) = r.tvmaze {
            attr("tvmazeid", i.to_string());
        }
        if !r.genre.is_empty() {
            attr("genre", r.genre.clone());
        }
        if let Some(y) = r.year {
            attr("year", y.to_string());
        }
        if !r.poster.is_empty() {
            attr("coverurl", r.poster.clone());
        }
        x.push_str("  </item>\n");
    }
    x.push_str("</channel>\n</rss>\n");
    x
}

/// A Torznab error answer (`<error code="100" description="…"/>`): 100 a bad API key, 201 a bad parameter, 203
/// a function the indexer doesn't have, 900 anything else.
pub fn error_xml(code: u16, description: &str) -> String {
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<error code=\"{code}\" description=\"{}\" />\n", esc(description))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_and_results_round_trip() {
        let c = Caps {
            title: "Test".into(),
            modes: vec![("search".into(), vec!["q".into()]), ("tv-search".into(), vec!["q".into(), "season".into(), "ep".into()])],
            categories: vec![(5040, "TV/HD".into()), (5000, "TV".into()), (100001, "Site cat".into())],
        };
        let x = caps_xml(&c);
        assert!(x.contains(r#"<tv-search available="yes" supportedParams="q,season,ep" />"#), "{x}");
        assert!(x.contains(r#"<category id="5000" name="TV">"#) && x.contains(r#"<subcat id="5040" name="TV/HD" />"#), "{x}");
        let r = Release {
            title: "A & B".into(),
            guid: "g".into(),
            link: "x".into(),
            size: Some(5),
            seeders: Some(3),
            leechers: Some(1),
            categories: vec![5040],
            ..Release::default()
        };
        let x = results_xml("Test", "http://l/1/api", &[r], &|_| "http://l/1/download?x=1&y=2".into());
        assert!(
            x.contains("<title>A &amp; B</title>") && x.contains(r#"name="peers" value="4""#) && x.contains("download?x=1&amp;y=2"),
            "{x}"
        );
        let back = super::super::newznab::parse_rss(&x, Protocol::Torrent);
        assert_eq!(back[0].title, "A & B");
        assert_eq!(back[0].seeders, Some(3));
    }
}
