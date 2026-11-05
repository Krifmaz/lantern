//! The update notice's side that talks to GitHub: the app's own releases (`<owner>/<repo>` from Cargo.toml's
//! `repository`), the file for this computer (the names `scripts/desktop-package.sh` makes), and the download,
//! checked against the release's SHA256SUMS. The rules and the words are kit-iced's `update`.

use kit_iced::update::{self, Feed, Release};
use serde_json::Value;
use std::path::PathBuf;

/// The repository the releases come from (`https://github.com/<owner>/<repo>`).
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// Where this app's releases live, for kit-iced's update rules.
pub fn feed() -> Feed {
    Feed {
        app: crate::title(),
        page: concat!(env!("CARGO_PKG_REPOSITORY"), "/releases"),
        downloads: concat!(env!("CARGO_PKG_REPOSITORY"), "/releases/download/"),
    }
}

/// GitHub's API address of the release list (`https://api.github.com/repos/<owner>/<repo>/releases`).
pub fn api_url(repository: &str) -> String {
    let repo = repository.trim_end_matches('/').trim_start_matches("https://github.com/");
    format!("https://api.github.com/repos/{repo}/releases")
}

/// The release file for a system (`std::env::consts::OS`) and version: `<App>-<version>-linux-x86_64.tar.gz`,
/// `-windows-x86_64.zip` or `-macos-universal.dmg` (`<App>` is the title without spaces); `None` for a system the
/// releases have no file for. The package script makes exactly these names.
pub fn file_for(os: &str, arch: &str, file: &str, version: &str) -> Option<String> {
    match (os, arch) {
        ("linux", "x86_64") => Some(format!("{file}-{version}-linux-x86_64.tar.gz")),
        ("windows", "x86_64") => Some(format!("{file}-{version}-windows-x86_64.zip")),
        ("macos", _) => Some(format!("{file}-{version}-macos-universal.dmg")),
        _ => None,
    }
}

/// The file for this computer at `version`.
pub fn this_computer(version: &str) -> Option<String> {
    file_for(std::env::consts::OS, std::env::consts::ARCH, crate::file_stem(), version)
}

/// Reads a GitHub release list into the newer release (none when this build is the newest); the error in words.
pub fn newer(list: &Value, current: &str) -> Result<Option<Release>, String> {
    if let Some(m) = list.get("message").and_then(Value::as_str) {
        return Err(format!("Couldn't check for updates: {m}"));
    }
    match update::full_release(list) {
        Some(r) => update::newest(&r, current, &feed(), this_computer),
        None => Ok(None),
    }
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(format!("{}/{}", crate::NAME, crate::VERSION))
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .unwrap_or_default()
}

/// Asks GitHub for the releases and answers the newer one, if any.
pub async fn check() -> Result<Option<Release>, String> {
    let resp = http()
        .get(api_url(REPOSITORY))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("Couldn't check for updates: {e}"))?;
    let list: Value = resp.json().await.map_err(|e| format!("Couldn't check for updates: {e}"))?;
    newer(&list, crate::VERSION)
}

/// Downloads the release's file for this computer and its SHA256SUMS into `dir`; answers the file's path and the
/// sums' text.
pub async fn download(r: Release, dir: PathBuf) -> Result<(PathBuf, Option<String>), String> {
    let Some(asset) = r.asset else { return Err(format!("The release has no file for this computer: see {}", r.page)) };
    let c = http();
    let fail = |e: reqwest::Error| format!("The download of {} failed: {e}", asset.name);
    let bytes = c.get(&asset.url).send().await.and_then(|r| r.error_for_status()).map_err(fail)?.bytes().await.map_err(fail)?;
    let sums = match &r.sums {
        Some(u) => Some(c.get(u).send().await.and_then(|r| r.error_for_status()).map_err(fail)?.text().await.map_err(fail)?),
        None => None,
    };
    tokio::fs::create_dir_all(&dir).await.map_err(|e| format!("{} can't be made: {e}", dir.display()))?;
    let path = dir.join(&asset.name);
    tokio::fs::write(&path, &bytes).await.map_err(|e| format!("{} can't be written: {e}", path.display()))?;
    Ok((path, sums))
}

/// Checks a downloaded file against its release's SHA256SUMS; a file that does not match is deleted.
pub async fn verify(path: PathBuf, sums: Option<String>) -> Result<PathBuf, String> {
    tokio::task::spawn_blocking(move || {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let checked = update::hash_file(&path).and_then(|hash| update::verify(sums.as_deref(), &name, &hash));
        match checked {
            Ok(()) => Ok(path),
            Err(e) => {
                let _ = std::fs::remove_file(&path);
                Err(e)
            }
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Where a checked download is kept: the user's Downloads folder, else the system's temporary folder.
pub fn download_dir() -> PathBuf {
    dirs::download_dir().unwrap_or_else(std::env::temp_dir)
}

/// The toast after a checked download: where it is and what to do with it.
pub fn downloaded_words(path: &std::path::Path) -> String {
    format!("{} is downloaded and checked. Open it to install the new version.", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_releases_are_this_repositorys() {
        assert_eq!(api_url("https://github.com/Owner/notes"), "https://api.github.com/repos/Owner/notes/releases");
        let f = feed();
        assert!(f.page.starts_with("https://github.com/") && f.page.ends_with("/releases"));
        assert_eq!(f.downloads, format!("{}/releases/download/", REPOSITORY));
    }

    #[test]
    fn each_system_has_its_file() {
        assert_eq!(file_for("linux", "x86_64", "Notes", "1.2.3").as_deref(), Some("Notes-1.2.3-linux-x86_64.tar.gz"));
        assert_eq!(file_for("windows", "x86_64", "Notes", "1.2.3").as_deref(), Some("Notes-1.2.3-windows-x86_64.zip"));
        assert_eq!(file_for("macos", "aarch64", "Notes", "1.2.3").as_deref(), Some("Notes-1.2.3-macos-universal.dmg"));
        assert_eq!(file_for("linux", "aarch64", "Notes", "1.2.3"), None);
    }

    #[test]
    fn a_newer_full_release_is_offered() {
        let file = this_computer("99.0.0");
        let assets: Vec<Value> = file
            .iter()
            .chain(std::iter::once(&"SHA256SUMS".to_string()))
            .map(|n| json!({"name": n, "browser_download_url": format!("{}v99.0.0/{n}", feed().downloads), "size": 3}))
            .collect();
        let list = json!([
            {"tag_name": "v100.0.0", "prerelease": true, "assets": []},
            {"tag_name": "v99.0.0", "html_url": format!("{REPOSITORY}/releases/tag/v99.0.0"), "body": "Notes", "assets": assets},
        ]);
        let r = newer(&list, "0.1.0").unwrap().expect("v99.0.0 is newer");
        assert_eq!(r.version, "99.0.0");
        assert!(r.sums.is_some());
        assert_eq!(newer(&list, "99.0.0"), Ok(None), "the same version is not offered");
        assert_eq!(newer(&json!([]), "0.1.0"), Ok(None));
        assert_eq!(
            newer(&json!({"message": "API rate limit exceeded"}), "0.1.0"),
            Err("Couldn't check for updates: API rate limit exceeded".into())
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_download_must_match_its_sum() {
        let dir = std::env::temp_dir().join(format!("kit-desktop-verify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("Notes-1.0.0-linux-x86_64.tar.gz");
        std::fs::write(&f, b"abc").unwrap();
        let good = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let sums = format!("{good}  Notes-1.0.0-linux-x86_64.tar.gz\n");
        assert_eq!(verify(f.clone(), Some(sums)).await, Ok(f.clone()));
        let bad = format!("{}  Notes-1.0.0-linux-x86_64.tar.gz\n", "0".repeat(64));
        assert!(verify(f.clone(), Some(bad)).await.unwrap_err().contains("does not match"));
        assert!(!f.exists(), "a file that does not match is deleted");
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(downloaded_words(&f).ends_with("Open it to install the new version."));
    }
}
