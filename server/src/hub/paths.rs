//! Paths as Lantern, a download client and an app each see them. Lantern's downloads folder is `root`; a client
//! may see it elsewhere (its `root`), and an app sees its own folder at its import path. Paths are swapped only at
//! a whole folder name, so `/data/tv` never matches `/data/tv2`.

/// `p` with forward slashes and no slash at the end (`/` stays `/`).
pub fn clean(p: &str) -> String {
    let p = p.trim().replace('\\', "/");
    if p == "/" {
        return p;
    }
    p.trim_end_matches('/').to_string()
}

/// `path` with the folder `from` at its start swapped for `to`; `None` when it isn't inside `from`.
pub fn swap(path: &str, from: &str, to: &str) -> Option<String> {
    let (path, from, to) = (clean(path), clean(from), clean(to));
    if from.is_empty() {
        return None;
    }
    if path == from {
        return Some(to);
    }
    let rest = if from == "/" { path.strip_prefix('/') } else { path.strip_prefix(&from).and_then(|r| r.strip_prefix('/')) }?;
    Some(if to == "/" { format!("/{rest}") } else { format!("{to}/{rest}") })
}

/// `path` (as Lantern sees it) as a client whose view of `root` is `client_root` sees it (unchanged when that is empty).
pub fn to_client(path: &str, root: &str, client_root: &str) -> String {
    if client_root.trim().is_empty() {
        return clean(path);
    }
    swap(path, root, client_root).unwrap_or_else(|| clean(path))
}

/// `path` (as a client whose view of `root` is `client_root` sees it) as Lantern sees it.
pub fn from_client(path: &str, root: &str, client_root: &str) -> String {
    if client_root.trim().is_empty() {
        return clean(path);
    }
    swap(path, client_root, root).unwrap_or_else(|| clean(path))
}

/// `path` (as Lantern sees it) as an app sees it, whose folder `folder` it sees at `import_path` (unchanged when
/// that is empty).
pub fn to_app(path: &str, folder: &str, import_path: &str) -> String {
    if import_path.trim().is_empty() {
        return clean(path);
    }
    swap(path, folder, import_path).unwrap_or_else(|| clean(path))
}

/// `name` as a category and folder name: lower case letters, digits and dashes (`Sonarr 4K` → `sonarr-4k`).
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "app".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swaps_whole_folders_only() {
        assert_eq!(swap("/downloads/tv/Show", "/downloads", "/data/torrents").as_deref(), Some("/data/torrents/tv/Show"));
        assert_eq!(swap("/downloads2/x", "/downloads", "/data"), None);
        assert_eq!(swap("/downloads/", "/downloads", "/data").as_deref(), Some("/data"));
        assert_eq!(swap("/x/y", "/", "/mnt").as_deref(), Some("/mnt/x/y"));
        assert_eq!(to_client("/downloads/tv", "/downloads", ""), "/downloads/tv");
        assert_eq!(from_client("D:\\dl\\tv\\a.mkv", "/downloads", "D:/dl"), "/downloads/tv/a.mkv");
        assert_eq!(to_app("/downloads/sonarr/Show/a.mkv", "/downloads/sonarr", "/data/downloads/tv"), "/data/downloads/tv/Show/a.mkv");
        assert_eq!(slug("  Sonarr 4K!! "), "sonarr-4k");
        assert_eq!(slug("??"), "app");
    }
}
