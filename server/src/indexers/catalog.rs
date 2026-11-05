//! Everything that can be added on Indexers → Add, in one list: the torrent sites from the definitions and the
//! Usenet indexers Lantern knows ([`super::newznab::PRESETS`]), plus "another Newznab/Torznab indexer". The list
//! can be searched and narrowed by protocol, privacy and language, and says how many of each there are.

use super::definition::Definition;
use super::newznab::PRESETS;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

/// One thing that can be added.
#[derive(Debug, Clone, PartialEq)]
pub struct Site {
    /// The definition or preset id (empty for another Newznab/Torznab indexer).
    pub id: String,
    /// How Lantern talks to it: `cardigann`, `newznab` or `torznab`.
    pub kind: &'static str,
    /// `torrent` or `usenet`.
    pub protocol: &'static str,
    /// Its name.
    pub name: String,
    /// One line about it.
    pub description: String,
    /// `private`, `semi-private` or `public` (empty for another indexer).
    pub privacy: String,
    /// Its language (`en-US`; empty for another indexer).
    pub language: String,
    /// The address it is added with.
    pub url: String,
    /// The API path (Usenet presets; empty otherwise).
    pub api_path: String,
    /// The standard categories it carries (Usenet presets; the definition says for torrent sites).
    pub categories: Vec<u32>,
    /// Whether this is "another Newznab/Torznab indexer" rather than a known site.
    pub custom: bool,
}

impl Site {
    /// The JSON the API answers for it.
    pub fn json(&self) -> Value {
        json!({
            "id": self.id, "kind": self.kind, "protocol": self.protocol, "name": self.name,
            "description": self.description, "privacy": self.privacy, "language": self.language, "url": self.url,
            "api_path": self.api_path, "categories": self.categories, "custom": self.custom,
        })
    }
}

/// What to narrow the list to; an empty field means any.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Words found in the name, id or description.
    pub words: String,
    /// `torrent` or `usenet`.
    pub protocol: String,
    /// `private`, `semi-private` or `public`.
    pub privacy: String,
    /// A language (`en-US`), or just its first part (`en`) for every country.
    pub language: String,
}

impl Filter {
    /// Whether `site` is wanted. Another Newznab/Torznab indexer only shows when no privacy or language is asked.
    pub fn wants(&self, site: &Site) -> bool {
        let words = self.words.trim().to_lowercase();
        let lang = self.language.trim().to_lowercase();
        (words.is_empty()
            || site.name.to_lowercase().contains(&words)
            || site.id.to_lowercase().contains(&words)
            || site.description.to_lowercase().contains(&words))
            && (self.protocol.is_empty() || site.protocol == self.protocol)
            && (self.privacy.is_empty() || site.privacy == self.privacy)
            && (lang.is_empty() || {
                let l = site.language.to_lowercase();
                l == lang || l.split('-').next() == Some(lang.as_str())
            })
    }
}

/// Every site that can be added, by name (known sites first, then another Newznab and Torznab indexer).
pub fn sites(defs: &[Arc<Definition>]) -> Vec<Site> {
    let mut out: Vec<Site> = defs
        .iter()
        .map(|d| Site {
            id: d.id.clone(),
            kind: "cardigann",
            protocol: "torrent",
            name: d.name.clone(),
            description: d.description.clone(),
            privacy: d.kind.clone(),
            language: d.language.clone(),
            url: d.links.first().cloned().unwrap_or_default(),
            api_path: String::new(),
            categories: Vec::new(),
            custom: false,
        })
        .chain(PRESETS.iter().map(|p| Site {
            id: p.id.into(),
            kind: "newznab",
            protocol: "usenet",
            name: p.name.into(),
            description: p.description.into(),
            privacy: p.privacy.into(),
            language: p.language.into(),
            url: p.url.into(),
            api_path: p.api_path.into(),
            categories: p.categories.to_vec(),
            custom: false,
        }))
        .collect();
    out.sort_by_key(|s| s.name.to_lowercase());
    for (kind, protocol, name, description) in [
        ("newznab", "usenet", "Another Newznab indexer", "Any Usenet indexer with a Newznab API: enter its address and API key."),
        ("torznab", "torrent", "Another Torznab indexer", "Any torrent indexer with a Torznab API: enter its address and API key."),
    ] {
        out.push(Site {
            id: String::new(),
            kind,
            protocol,
            name: name.into(),
            description: description.into(),
            privacy: String::new(),
            language: String::new(),
            url: String::new(),
            api_path: "/api".into(),
            categories: Vec::new(),
            custom: true,
        });
    }
    out
}

/// How many known sites there are in all, per protocol, privacy and language (another indexer isn't counted).
pub fn counts(all: &[Site]) -> Value {
    let known: Vec<&Site> = all.iter().filter(|s| !s.custom).collect();
    let tally = |f: fn(&Site) -> &str| {
        let mut m: BTreeMap<String, usize> = BTreeMap::new();
        for s in &known {
            let k = f(s);
            if !k.is_empty() {
                *m.entry(k.to_string()).or_default() += 1;
            }
        }
        m
    };
    json!({
        "all": known.len(),
        "protocol": tally(|s| s.protocol),
        "privacy": tally(|s| &s.privacy),
        "language": tally(|s| &s.language),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(id: &str, kind: &str, language: &str) -> Arc<Definition> {
        Arc::new(Definition {
            id: id.into(),
            name: id.to_uppercase(),
            description: format!("{id} tracker"),
            language: language.into(),
            kind: kind.into(),
            links: vec![format!("https://{id}.example/")],
            ..Default::default()
        })
    }

    #[test]
    fn torrent_and_usenet_in_one_list_with_filters_and_counts() {
        let all = sites(&[def("alpha", "private", "en-US"), def("beta", "public", "fr-FR"), def("gamma", "semi-private", "en-GB")]);
        assert_eq!(all.len(), 3 + PRESETS.len() + 2);
        let geek = all.iter().find(|s| s.id == "nzbgeek").unwrap();
        assert_eq!(
            (geek.kind, geek.protocol, geek.url.as_str(), geek.api_path.as_str()),
            ("newznab", "usenet", "https://api.nzbgeek.info", "/api")
        );
        assert!(geek.categories.contains(&5000) && geek.categories.contains(&2000));
        assert_eq!(all.iter().find(|s| s.id == "tabularasa").unwrap().api_path, "/api/v1/api");
        // sorted by name, the two "another" entries last
        assert!(all[..all.len() - 2].windows(2).all(|w| w[0].name.to_lowercase() <= w[1].name.to_lowercase()));
        assert!(all[all.len() - 2..].iter().all(|s| s.custom));

        let pick = |f: Filter| all.iter().filter(|s| f.wants(s)).map(|s| s.name.clone()).collect::<Vec<_>>();
        let usenet = pick(Filter { protocol: "usenet".into(), ..Default::default() });
        assert_eq!(usenet.len(), PRESETS.len() + 1);
        assert_eq!(pick(Filter { protocol: "torrent".into(), privacy: "public".into(), ..Default::default() }), ["BETA"]);
        assert_eq!(pick(Filter { language: "en".into(), protocol: "torrent".into(), ..Default::default() }), ["ALPHA", "GAMMA"]);
        assert_eq!(pick(Filter { language: "fr-FR".into(), ..Default::default() }), ["BETA"]);
        assert_eq!(pick(Filter { words: "geek".into(), ..Default::default() }), ["NZBgeek"]);
        assert_eq!(pick(Filter { privacy: "semi-private".into(), ..Default::default() }), ["GAMMA"]);

        let c = counts(&all);
        assert_eq!(c["all"], json!(3 + PRESETS.len()));
        assert_eq!(c["protocol"]["torrent"], json!(3));
        assert_eq!(c["protocol"]["usenet"], json!(PRESETS.len()));
        assert_eq!(c["language"]["fr-FR"], json!(1));
        assert_eq!(c["privacy"]["public"], json!(2), "beta and AnimeTosho");
    }
}
