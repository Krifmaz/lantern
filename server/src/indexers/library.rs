//! The indexer definitions Lantern knows: the ones built into the image (`/app/definitions`) and the newer ones
//! it downloads into the data folder (`<data>/definitions/v11`), which win. Updated from Prowlarr/Indexers.

use super::definition::{Definition, SCHEMA};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// Where updates come from: the Prowlarr/Indexers repository as a tarball.
pub const DEFAULT_SOURCE: &str = "https://github.com/Prowlarr/Indexers/archive/refs/heads/master.tar.gz";

/// The definitions, by id.
#[derive(Clone, Default)]
pub struct Library {
    /// The folders read, later ones winning.
    dirs: Vec<PathBuf>,
    /// The folder updates are written to.
    data: PathBuf,
    /// id → definition.
    defs: Arc<RwLock<BTreeMap<String, Arc<Definition>>>>,
    /// Files that couldn't be read: (file, why).
    errors: Arc<RwLock<Vec<(String, String)>>>,
}

impl Library {
    /// A library reading `bundled` (may be missing) then `<data>/definitions/v11`.
    pub fn new(bundled: Option<PathBuf>, data_dir: &Path) -> Library {
        let data = data_dir.join("definitions").join(SCHEMA);
        let mut dirs = Vec::new();
        if let Some(b) = bundled {
            dirs.push(b);
        }
        dirs.push(data.clone());
        let lib = Library { dirs, data, defs: Default::default(), errors: Default::default() };
        lib.reload();
        lib
    }

    /// Reads every folder again; the count of definitions.
    pub fn reload(&self) -> usize {
        let mut defs = BTreeMap::new();
        let mut errors = Vec::new();
        for dir in &self.dirs {
            let Ok(rd) = std::fs::read_dir(dir) else { continue };
            let mut files: Vec<PathBuf> =
                rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "yml" || e == "yaml")).collect();
            files.sort();
            for f in files {
                let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                match std::fs::read_to_string(&f).map_err(|e| e.to_string()).and_then(|t| Definition::parse(&t)) {
                    Ok(d) => {
                        defs.insert(d.id.clone(), Arc::new(d));
                    }
                    Err(e) => errors.push((name, e)),
                }
            }
        }
        let n = defs.len();
        *self.defs.write().unwrap_or_else(|e| e.into_inner()) = defs;
        *self.errors.write().unwrap_or_else(|e| e.into_inner()) = errors;
        n
    }

    /// Adds one definition (tests, a person's own file).
    pub fn insert(&self, d: Definition) {
        self.defs.write().unwrap_or_else(|e| e.into_inner()).insert(d.id.clone(), Arc::new(d));
    }

    /// The definition `id`, or the one that replaced it.
    pub fn get(&self, id: &str) -> Option<Arc<Definition>> {
        let defs = self.defs.read().unwrap_or_else(|e| e.into_inner());
        defs.get(id).cloned().or_else(|| defs.values().find(|d| d.replaces.iter().any(|r| r == id)).cloned())
    }

    /// Every definition, by id.
    pub fn all(&self) -> Vec<Arc<Definition>> {
        self.defs.read().unwrap_or_else(|e| e.into_inner()).values().cloned().collect()
    }

    /// How many there are.
    pub fn len(&self) -> usize {
        self.defs.read().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The files that couldn't be read.
    pub fn errors(&self) -> Vec<(String, String)> {
        self.errors.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Downloads the definitions from `source` (a .tar.gz of the Prowlarr/Indexers repository) into the data
    /// folder and reads them; the count.
    pub async fn update(&self, source: &str) -> Result<usize, String> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("Lantern/", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .map_err(|e| e.to_string())?;
        let resp = client.get(source).send().await.map_err(|e| format!("The indexer definitions can't be downloaded: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("The indexer definitions can't be downloaded: the server answered {}", resp.status()));
        }
        let bytes = resp.bytes().await.map_err(|e| format!("The indexer definitions download stopped: {e}"))?;
        let data = self.data.clone();
        let n = tokio::task::spawn_blocking(move || unpack(&bytes, &data)).await.map_err(|e| e.to_string())??;
        self.reload();
        Ok(n)
    }
}

/// Writes the `definitions/<SCHEMA>/*.yml` files of a .tar.gz into `dest` (replacing what was there); the count.
pub fn unpack(targz: &[u8], dest: &Path) -> Result<usize, String> {
    let tmp = dest.with_extension("new");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| format!("The definitions folder can't be made: {e}"))?;
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(targz));
    let want = format!("definitions/{SCHEMA}/");
    let mut n = 0;
    for entry in ar.entries().map_err(|e| format!("The definitions download isn't a .tar.gz: {e}"))? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.to_string_lossy().to_string();
        let Some(at) = path.find(&want) else { continue };
        let name = &path[at + want.len()..];
        if name.contains('/') || !(name.ends_with(".yml") || name.ends_with(".yaml")) {
            continue;
        }
        let mut text = String::new();
        if entry.read_to_string(&mut text).is_err() {
            continue;
        }
        std::fs::write(tmp.join(name), text).map_err(|e| format!("A definition can't be written: {e}"))?;
        n += 1;
    }
    if n == 0 {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(format!("The download has no definitions/{SCHEMA} files"));
    }
    let _ = std::fs::remove_dir_all(dest);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, dest).map_err(|e| format!("The new definitions can't be put in place: {e}"))?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpacks_the_schema_folder_only() {
        let dir = std::env::temp_dir().join(format!("lantern-lib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut tar = tar::Builder::new(Vec::new());
        let def = include_str!("../../tests/fixtures/definitions/lantern-test.yml");
        for (p, body) in [
            ("Indexers-master/definitions/v11/lantern-test.yml", def),
            ("Indexers-master/definitions/v10/old.yml", def),
            ("Indexers-master/README.md", "x"),
        ] {
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append_data(&mut h, p, body.as_bytes()).unwrap();
        }
        let raw = tar.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, &raw).unwrap();
        let bytes = gz.finish().unwrap();
        let lib = Library::new(None, &dir);
        assert_eq!(unpack(&bytes, &lib.data).unwrap(), 1);
        assert_eq!(lib.reload(), 1);
        assert!(lib.get("lantern-test").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// With `LANTERN_DEFINITIONS_DIR` set to a checkout's `definitions/v11`: every definition there loads and every
/// template in it renders (`cargo test every_real_definition -- --nocapture`).
#[cfg(test)]
mod real {
    use super::super::template::{render, Vars};
    use super::*;

    #[test]
    fn every_real_definition_loads_and_its_templates_render() {
        let Ok(dir) = std::env::var("LANTERN_DEFINITIONS_DIR") else { return };
        let lib = Library::new(Some(PathBuf::from(&dir)), &std::env::temp_dir().join("lantern-real-defs"));
        let errors = lib.errors();
        assert!(errors.is_empty(), "{} definitions don't load: {:?}", errors.len(), &errors[..errors.len().min(10)]);
        let mut bad = Vec::new();
        for d in lib.all() {
            let mut texts: Vec<String> = d.search.paths.iter().map(|p| p.path.clone()).collect();
            texts.push(d.search.rows.selector.clone());
            texts.extend(d.search.inputs.values().map(super::super::filters::yaml_text));
            texts.extend(d.search.fields.values().filter_map(|f| f.text.clone()));
            for t in texts {
                if let Err(e) = render(&t, &Vars::default()) {
                    bad.push(format!("{}: {e} in {t}", d.id));
                }
            }
        }
        let known = [
            "replace",
            "re_replace",
            "regexp",
            "split",
            "trim",
            "prepend",
            "append",
            "tolower",
            "toupper",
            "urldecode",
            "urlencode",
            "htmldecode",
            "htmlencode",
            "querystring",
            "dateparse",
            "timeparse",
            "timeago",
            "reltime",
            "fuzzytime",
            "validfilename",
            "diacritics",
            "validate",
            "hexdump",
            "strdump",
            "andmatch",
            "jsonjoinarray",
        ];
        for d in lib.all() {
            let mut all: Vec<&super::super::filters::Filter> = d.search.keywordsfilters.iter().collect();
            for f in d.search.fields.values() {
                all.extend(f.filters.iter());
            }
            for f in all {
                if !known.contains(&f.name.as_str()) {
                    bad.push(format!("{}: filter {}", d.id, f.name));
                }
            }
            for f in d.search.fields.values().filter(|f| !f.selector.is_empty() && !f.selector.contains("{{")) {
                if !f.selector.contains('{')
                    && !super::super::cardigann::valid_css(&f.selector)
                    && !d.search.paths.iter().any(|p| p.response.as_ref().is_some_and(|r| r.kind == "json"))
                    && d.id != "puntotorrent"
                {
                    bad.push(format!("{}: selector {}", d.id, f.selector));
                }
            }
        }
        assert!(bad.is_empty(), "{} templates fail: {:#?}", bad.len(), &bad[..bad.len().min(15)]);
        println!("{} definitions load", lib.len());
    }
}
