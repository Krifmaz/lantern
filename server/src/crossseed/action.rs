// Ported from cross-seed v6.13.9 (src/action.ts, and getTorrentSavePath of src/torrent.ts), Apache-2.0.
//! What is done with a match (cross-seed's `performAction`): with action `save` its `.torrent` is saved in the
//! output folder; with `inject` it is added to a torrent client. With link folders (`linkDirs`) the match's files
//! are first linked from the searchee's files into a link folder on the same drive ([`get_link_dir`]), in a folder
//! named after the tracker unless `flatLinking`, with the link type of `linkType` ([`link_file`]); a `MATCH` keeps
//! its paths, other matches are paired file by file by size (and name, when sizes repeat). Without link folders a
//! match is added next to the searchee's files. A failed injection is saved for the inject job, and files linked
//! for it are removed again ([`unlink_metafile`]).

use super::clients::{Client, DirError, MetaRef, SearcheeOptions};
use super::config::RuntimeConfig;
use super::constants::{all_extensions, Action, Decision, InjectionResult, LinkType, MediaType, ABS_WIN_PATH_REGEX, MAX_PATH_BYTES};
use super::jsre::test;
use super::searchee::{create_searchee_from_path, get_root, get_root_folder, media_type_of, Label, Searchee};
use super::torrent::Metafile;
use super::utils::{format_as_list, strip_extension};
use kit_server_core::SqliteDb;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The test file made in a folder without a file to test linking with.
const LINK_DIR_SRC_NAME: &str = "linkDirSrc.cross-seed";
/// The test link made in a link folder.
const LINK_DIR_DEST_NAME: &str = "linkDirDest.cross-seed";
/// The test link made in a client's save folder.
const CLIENT_DEST_NAME: &str = "torrentClientDest.cross-seed";

/// What `performAction` needs: the clients, the database (the client torrent cache), the settings and the output
/// folder.
#[derive(Clone)]
pub struct Ctx {
    /// The torrent clients, in priority order.
    pub clients: Vec<Arc<Client>>,
    /// The database.
    pub db: SqliteDb,
    /// The settings.
    pub cfg: RuntimeConfig,
    /// `outputDir`: where `.torrent` files are saved.
    pub output_dir: PathBuf,
}

/// `ActionReturn`: what was done, and with which client and folder.
#[derive(Clone)]
pub struct ActionReturn {
    /// The client it went to (for `SUCCESS`, `ALREADY_EXISTS` and sometimes `TORRENT_NOT_COMPLETE`).
    pub client: Option<Arc<Client>>,
    /// The result.
    pub action_result: InjectionResult,
    /// The folder its files are in.
    pub destination_dir: Option<String>,
    /// Whether new files were linked for it.
    pub linked_new_files: bool,
}

impl ActionReturn {
    /// A result without a client.
    fn bare(action_result: InjectionResult, linked_new_files: bool) -> ActionReturn {
        ActionReturn { client: None, action_result, destination_dir: None, linked_new_files }
    }
}

/// `LinkResult`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LinkResult {
    /// Some file was already there.
    pub already_existed: bool,
    /// Some file was linked now.
    pub linked_new_files: bool,
}

/// `unwrapSymlinks`: follows a file's symbolic links to the file itself (a link in a folder of its path is kept).
pub fn unwrap_symlinks(path: &Path) -> io::Result<PathBuf> {
    let mut p = path.to_path_buf();
    for _ in 0..16 {
        if !std::fs::symlink_metadata(&p)?.file_type().is_symlink() {
            return Ok(p);
        }
        let target = std::fs::read_link(&p)?;
        p = absolute(&p.parent().unwrap_or(Path::new("/")).join(target));
    }
    Err(io::Error::other(format!("too many levels of symbolic links at {}", p.display())))
}

/// `path.resolve`: an absolute path with `.` and `..` worked out, without following links.
pub fn absolute(p: &Path) -> PathBuf {
    let base = if p.is_absolute() { PathBuf::new() } else { std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")) };
    let mut out = PathBuf::from("/");
    for c in base.join(p).components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::Normal(s) => out.push(s),
            _ => {}
        }
    }
    out
}

/// A copy-on-write copy (`COPYFILE_FICLONE_FORCE`): fails where the file system can't make one.
#[cfg(target_os = "linux")]
fn reflink(src: &Path, dest: &Path) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let from = std::fs::File::open(src)?;
    let to = std::fs::OpenOptions::new().write(true).create_new(true).open(dest)?;
    // SAFETY: both descriptors are open for the length of the call; FICLONE only reads them.
    let r = unsafe { libc::ioctl(to.as_raw_fd(), libc::FICLONE, from.as_raw_fd()) };
    if r != 0 {
        let e = io::Error::last_os_error();
        drop(to);
        let _ = std::fs::remove_file(dest);
        return Err(e);
    }
    Ok(())
}

/// Copy-on-write copies are made only on Linux here.
#[cfg(not(target_os = "linux"))]
fn reflink(_src: &Path, _dest: &Path) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "reflink is not supported here"))
}

/// `linkFile`: links `old` at `new` by `link_type`; `false` when `new` is already there.
pub fn link_file(old: &Path, new: &Path, link_type: LinkType) -> io::Result<bool> {
    let res = (|| {
        let original = unwrap_symlinks(old)?;
        match link_type {
            LinkType::Hardlink => std::fs::hard_link(&original, new),
            // resolved, as a symbolic link is read from where it is
            LinkType::Symlink => symlink(&original, &absolute(new)),
            LinkType::Reflink => reflink(&original, new),
            LinkType::ReflinkOrCopy => {
                if new.symlink_metadata().is_ok() {
                    return Err(io::Error::from(io::ErrorKind::AlreadyExists));
                }
                reflink(&original, new).or_else(|e| {
                    if e.kind() == io::ErrorKind::AlreadyExists {
                        return Err(e);
                    }
                    copy_new(&original, new)
                })
            }
        }
    })();
    match res {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}

/// A plain copy that refuses to overwrite.
fn copy_new(src: &Path, dest: &Path) -> io::Result<()> {
    let mut from = std::fs::File::open(src)?;
    let mut to = std::fs::OpenOptions::new().write(true).create_new(true).open(dest)?;
    io::copy(&mut from, &mut to)?;
    Ok(())
}

/// A symbolic link.
#[cfg(unix)]
fn symlink(original: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(original, link)
}

/// A symbolic link.
#[cfg(not(unix))]
fn symlink(original: &Path, link: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_file(original, link)
}

/// The device a path is on (0 where it can't be told).
fn dev(p: &Path) -> io::Result<u64> {
    let m = std::fs::metadata(p)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(m.dev())
    }
    #[cfg(not(unix))]
    {
        let _ = m;
        Ok(0)
    }
}

/// `findAFileWithExt`: the first file under `dir` with a known media ending.
pub fn find_a_file_with_ext(dir: &Path) -> Option<PathBuf> {
    let exts: Vec<&str> = all_extensions().collect();
    let entries = std::fs::read_dir(dir).ok()?;
    for e in entries.flatten() {
        let p = e.path();
        let Ok(t) = e.file_type() else { continue };
        let ext = p.extension().map(|x| format!(".{}", x.to_string_lossy())).unwrap_or_default();
        if t.is_file() && exts.contains(&ext.as_str()) {
            return Some(p);
        }
        if t.is_dir() {
            if let Some(f) = find_a_file_with_ext(&p) {
                return Some(f);
            }
        }
    }
    None
}

/// The link type for a test link: reflinks are tested as reflinks, the rest as hardlinks.
fn test_type(link_type: LinkType) -> LinkType {
    match link_type {
        LinkType::Reflink | LinkType::ReflinkOrCopy => link_type,
        _ => LinkType::Hardlink,
    }
}

/// `getLinkDir`: the link folder on the same drive as `path` (by device, else by a test link); for symlinks the
/// first one when none is.
pub fn get_link_dir(path: &Path, cfg: &RuntimeConfig) -> Option<String> {
    let st = std::fs::metadata(path).ok()?;
    if let Ok(path_dev) = dev(path) {
        if path_dev != 0 {
            let devs: Vec<Option<u64>> = cfg.link_dirs.iter().map(|d| dev(Path::new(d)).ok()).collect();
            let unique: std::collections::HashSet<_> = devs.iter().collect();
            if unique.len() == cfg.link_dirs.len() {
                for (i, d) in cfg.link_dirs.iter().enumerate() {
                    if devs[i] == Some(path_dev) {
                        return Some(d.clone());
                    }
                }
            }
        }
    }
    let mut src = if st.is_file() {
        Some(path.to_path_buf())
    } else if st.is_dir() {
        find_a_file_with_ext(path)
    } else {
        None
    };
    let mut temp: Option<PathBuf> = None;
    if src.is_none() {
        let t = if st.is_dir() { path.join(LINK_DIR_SRC_NAME) } else { path.parent().unwrap_or(Path::new("/")).join(LINK_DIR_SRC_NAME) };
        if std::fs::write(&t, "").is_ok() {
            src = Some(t.clone());
        }
        temp = Some(t);
    }
    let remove_temp = |t: &Option<PathBuf>| {
        if let Some(t) = t {
            let _ = std::fs::remove_file(t);
        }
    };
    if let Some(src) = &src {
        for d in &cfg.link_dirs {
            let test_path = Path::new(d).join(LINK_DIR_DEST_NAME);
            if link_file(src, &test_path, test_type(cfg.link_type)).is_ok() {
                let _ = std::fs::remove_file(&test_path);
                remove_temp(&temp);
                return Some(d.clone());
            }
        }
    }
    remove_temp(&temp);
    if cfg.link_type != LinkType::Symlink {
        tracing::error!(
            "cross-seed: Cannot find any linkDir from linkDirs on the same drive to {} {}",
            cfg.link_type.as_str(),
            path.display()
        );
        return None;
    }
    if cfg.link_dirs.len() > 1 {
        tracing::warn!(
            "cross-seed: Cannot find any linkDir from linkDirs on the same drive, using first linkDir for symlink: {}",
            path.display()
        );
    }
    cfg.link_dirs.first().cloned()
}

/// `getLinkDirVirtual`: the one link folder for every episode of a season made of episodes.
pub fn get_link_dir_virtual(searchee: &Searchee, cfg: &RuntimeConfig) -> Option<String> {
    let first = searchee.files.first()?;
    let link_dir = get_link_dir(Path::new(&first.path), cfg)?;
    for f in &searchee.files[1..] {
        if get_link_dir(Path::new(&f.path), cfg).as_deref() != Some(link_dir.as_str()) {
            tracing::error!(
                "cross-seed: Cannot link files to multiple linkDirs for seasonFromEpisodes aggregation, source episodes are spread across multiple drives."
            );
            return None;
        }
    }
    Some(link_dir)
}

/// `testLinking`: whether files in `src_dir` can be linked into a link folder by `linkType`; an error in
/// cross-seed's words when they can't.
pub fn test_linking(src_dir: &Path, test_src_name: &str, test_dest_name: &str, cfg: &RuntimeConfig) -> Result<bool, String> {
    let mut temp: Option<PathBuf> = None;
    let res = (|| -> Result<bool, String> {
        let src = match find_a_file_with_ext(src_dir) {
            Some(f) => f,
            None => {
                let usable = src_dir.is_dir() && std::fs::read_dir(src_dir).is_ok();
                if !usable {
                    tracing::error!(
                        "cross-seed: cross-seed is unable to verify linking for {} (likely due to incorrect/insufficient volume mounts https://www.cross-seed.org/docs/tutorials/linking#configuring-linkdirs).",
                        src_dir.display()
                    );
                    return Ok(false);
                }
                let t = src_dir.join(test_src_name);
                if std::fs::write(&t, test_src_name).is_err() {
                    tracing::error!("cross-seed: Failed to create test file in {} for linking test.", src_dir.display());
                    return Ok(false);
                }
                temp = Some(t.clone());
                t
            }
        };
        let link_dir = get_link_dir(src_dir, cfg).ok_or_else(|| format!("No valid linkDir found for {}", src_dir.display()))?;
        let test_path = Path::new(&link_dir).join(test_dest_name);
        link_file(&src, &test_path, cfg.link_type).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(&test_path);
        Ok(true)
    })();
    if let Some(t) = temp {
        let _ = std::fs::remove_file(t);
    }
    res.map_err(|e| {
        tracing::error!("cross-seed: {e}");
        let dirs: Vec<String> = cfg.link_dirs.iter().map(|d| format!("\"{d}\"")).collect();
        format!(
            "Failed to create a test {} from {} in any linkDirs: [{}]. Ensure that {} is supported between these paths (hardlink/reflink requires same drive, partition, and volume).",
            cfg.link_type.as_str(),
            src_dir.display(),
            format_as_list(&dirs, false, "short", "unit"),
            cfg.link_type.as_str()
        )
    })
}

/// `linkAllFilesInMetafile`: links each of the match's files from the searchee's into `destination_dir`.
pub fn link_all_files_in_metafile(
    searchee: &Searchee,
    meta: &Metafile,
    decision: Decision,
    destination_dir: &Path,
    save_path: Option<&str>,
    ignore_missing: bool,
    link_type: LinkType,
) -> Result<LinkResult, String> {
    let mut available: Vec<&super::searchee::File> = searchee.files.iter().collect();
    let paths: Vec<(PathBuf, PathBuf)> = match (decision, save_path) {
        (Decision::Match, Some(sp)) => meta.files.iter().map(|f| (Path::new(sp).join(&f.path), destination_dir.join(&f.path))).collect(),
        _ => {
            let mut out = Vec::new();
            for nf in &meta.files {
                let mut matched: Vec<&super::searchee::File> = available.iter().copied().filter(|s| s.length == nf.length).collect();
                if matched.len() > 1 {
                    matched.retain(|s| s.name == nf.name);
                }
                let Some(first) = matched.first().copied() else { continue };
                if let Some(i) = available.iter().position(|a| std::ptr::eq(*a, first)) {
                    available.remove(i);
                }
                let src = match save_path {
                    Some(sp) => Path::new(sp).join(&first.path),
                    None => PathBuf::from(&first.path),
                };
                out.push((src, destination_dir.join(&nf.path)));
            }
            out
        }
    };
    let mut result = LinkResult::default();
    tracing::debug!("cross-seed: Linking {} from {} to {}", meta.name, searchee.name, destination_dir.display());
    let mut valid = Vec::new();
    for (src, dest) in paths {
        if dest.symlink_metadata().is_ok() {
            tracing::debug!("cross-seed: --- Skipping {} -> {}, already exists", src.display(), dest.display());
            result.already_existed = true;
            continue;
        }
        if src.exists() {
            valid.push((src, dest));
        } else if !ignore_missing {
            return Err(format!("Linking failed, {} not found.", src.display()));
        }
    }
    for (src, dest) in valid {
        if let Some(parent) = dest.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
        }
        match link_file(&src, &dest, link_type) {
            Ok(true) => result.linked_new_files = true,
            Ok(false) => {}
            Err(e) => {
                tracing::error!("cross-seed: --- Linking failed, {} -> {}: {e}", src.display(), dest.display());
                return Err(e.to_string());
            }
        }
    }
    Ok(result)
}

/// `unlinkMetafile`: removes the match's top folder (or file) from `destination_dir`, never the folder itself.
pub fn unlink_metafile(meta: &Metafile, destination_dir: &Path) {
    let mut roots = Vec::new();
    for f in &meta.files {
        match get_root(f) {
            Ok(r) => roots.push(destination_dir.join(r)),
            Err(e) => {
                tracing::error!("cross-seed: Unable to unlink {} in {}: {e}", meta.name, destination_dir.display());
                return;
            }
        }
    }
    let Ok(dest_meta) = std::fs::metadata(destination_dir) else { return };
    for root in roots {
        let Ok(m) = std::fs::symlink_metadata(&root) else { continue };
        if !root.starts_with(destination_dir) || absolute(&root) == absolute(destination_dir) {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if std::fs::metadata(&root).map(|r| r.ino() == dest_meta.ino()).unwrap_or(false) {
                continue;
            }
        }
        #[cfg(not(unix))]
        let _ = &dest_meta;
        tracing::debug!("cross-seed: Unlinking {}", root.display());
        let _ = if m.is_dir() { std::fs::remove_dir_all(&root) } else { std::fs::remove_file(&root) };
    }
}

/// `getTorrentSavePath`: `[mediaType][tracker]name[infoHash].torrent` in `dir`, the name shortened (with `...`)
/// so the whole path stays within 255 bytes.
pub fn get_torrent_save_path(meta: &Metafile, media: MediaType, tracker: &str, dir: &Path, cached: bool) -> PathBuf {
    let full = strip_extension(&meta.file_system_safe_name());
    let ext = if cached { ".cached.torrent" } else { ".torrent" };
    let name = |n: &str| format!("[{}][{tracker}]{n}[{}]{ext}", media.as_str(), meta.info_hash);
    let path = dir.join(name(&full));
    let len = path.to_string_lossy().len();
    if len <= MAX_PATH_BYTES {
        return path;
    }
    let chars: Vec<char> = full.chars().collect();
    let mut bytes = len + 3;
    let mut remove = 0;
    for c in chars.iter().rev() {
        remove += 1;
        bytes -= c.len_utf8();
        if bytes <= MAX_PATH_BYTES {
            break;
        }
    }
    let kept: String = chars[..chars.len().saturating_sub(remove)].iter().collect();
    dir.join(name(&format!("{kept}...")))
}

/// `saveToOutputDir`: saves the match's `.torrent` (or touches it when it is already there).
pub fn save_to_output_dir(meta: &Metafile, decision: Decision, searchee: &Searchee, tracker: &str, output_dir: &Path) {
    let res = (|| -> io::Result<()> {
        std::fs::create_dir_all(output_dir)?;
        let path = get_torrent_save_path(meta, media_type_of(&meta.title, &meta.files), tracker, output_dir, false);
        if path.exists() {
            let f = std::fs::OpenOptions::new().append(true).open(&path)?;
            let mtime = std::fs::metadata(&path)?.modified()?;
            f.set_times(std::fs::FileTimes::new().set_accessed(std::time::SystemTime::now()).set_modified(mtime))?;
            return Ok(());
        }
        std::fs::write(path, &meta.raw)
    })();
    if let Err(e) = res {
        tracing::error!(
            "cross-seed: Failed to save {} on {tracker} to outputDir while processing {} with {}: {e}",
            meta.name,
            decision.as_str(),
            searchee.name
        );
    }
}

/// `getSavePath`'s errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SavePathError {
    /// `INVALID_DATA`.
    InvalidData,
    /// `TORRENT_NOT_FOUND`.
    TorrentNotFound,
    /// `TORRENT_NOT_COMPLETE`.
    TorrentNotComplete,
}

impl SavePathError {
    /// cross-seed's name for it.
    fn as_str(self) -> &'static str {
        match self {
            SavePathError::InvalidData => "INVALID_DATA",
            SavePathError::TorrentNotFound => "TORRENT_NOT_FOUND",
            SavePathError::TorrentNotComplete => "TORRENT_NOT_COMPLETE",
        }
    }
}

/// The client the searchee is in: the only one, or the one at its host.
fn searchee_client(ctx: &Ctx, searchee: &Searchee) -> Option<Arc<Client>> {
    if ctx.clients.len() == 1 {
        return ctx.clients.first().cloned();
    }
    ctx.clients.iter().find(|c| Some(c.info.client_host.as_str()) == searchee.client_host.as_deref()).cloned()
}

/// `getSavePath`: the folder the searchee's files are in (`None` for a season made of episodes, whose file paths
/// are whole), checked to be there and finished.
async fn get_save_path(ctx: &Ctx, searchee: &mut Searchee, only_completed: bool) -> Result<Option<String>, SavePathError> {
    if let Some(path) = searchee.path.clone() {
        if !Path::new(&path).exists() {
            tracing::error!("cross-seed: Linking failed, {path} not found.");
            return Err(SavePathError::InvalidData);
        }
        let refreshed = create_searchee_from_path(Path::new(&path)).map_err(|_| SavePathError::TorrentNotFound)?;
        if only_completed && (searchee.mtime_ms != refreshed.mtime_ms || searchee.length != refreshed.length) {
            return Err(SavePathError::TorrentNotComplete);
        }
        return Ok(Some(super::searchee::dirname(&path)));
    }
    let Some(info_hash) = searchee.info_hash.clone() else {
        for f in &searchee.files {
            let Ok(m) = std::fs::metadata(&f.path) else {
                tracing::error!("cross-seed: Linking failed, {} not found.", f.path);
                return Err(SavePathError::InvalidData);
            };
            if only_completed {
                let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs_f64() * 1000.0);
                if searchee.mtime_ms.unwrap_or(0.0) < mtime.unwrap_or(0.0) || f.length != m.len() as i64 {
                    return Err(SavePathError::TorrentNotComplete);
                }
            }
        }
        return Ok(None);
    };
    let Some(client) = searchee_client(ctx, searchee) else { return Err(SavePathError::TorrentNotFound) };
    let save_path = if searchee.save_path.is_some() {
        if searchee.label != Label::Inject {
            let o = SearcheeOptions { new_searchees_only: true, refresh: Some(vec![info_hash.clone()]), ..SearcheeOptions::default() };
            let fresh = client.get_client_searchees(&ctx.db, &ctx.cfg, &o).await;
            let Some(r) = fresh.new_searchees.into_iter().find(|s| s.info_hash.as_deref() == Some(info_hash.as_str())) else {
                return Err(SavePathError::TorrentNotFound);
            };
            let label = searchee.label;
            *searchee = Searchee { label, ..r };
        }
        if only_completed && !client.is_torrent_complete(&info_hash).await.unwrap_or(false) {
            return Err(SavePathError::TorrentNotComplete);
        }
        searchee.save_path.clone().unwrap_or_default()
    } else {
        match client.get_download_dir(MetaRef::of_searchee(searchee), only_completed, &ctx.cfg).await {
            Ok(d) => d,
            Err(DirError::NotFound | DirError::UnknownError) => return Err(SavePathError::TorrentNotFound),
            Err(DirError::TorrentNotComplete) => return Err(SavePathError::TorrentNotComplete),
            Err(DirError::InvalidData) => return Err(SavePathError::InvalidData),
        }
    };
    let Some(first) = searchee.files.first() else { return Err(SavePathError::InvalidData) };
    let root_folder = match get_root_folder(first) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("cross-seed: Linking failed, {e}");
            return Err(SavePathError::InvalidData);
        }
    };
    let source = if searchee.files.len() == 1 {
        Path::new(&save_path).join(&first.path)
    } else if let Some(r) = root_folder {
        Path::new(&save_path).join(r)
    } else {
        PathBuf::from(&save_path)
    };
    if !source.exists() {
        tracing::error!("cross-seed: Linking failed, {} not found.", source.display());
        return Err(SavePathError::InvalidData);
    }
    Ok(Some(save_path))
}

/// `getClientAndDestinationDir`: the client to add to (for data, the first whose save folders are on the same
/// drive) and the folder to link into; the bool says the client already has the match there.
async fn get_client_and_destination_dir(
    ctx: &Ctx,
    client: Option<Arc<Client>>,
    searchee: &Searchee,
    save_path: Option<&str>,
    meta: &Metafile,
    tracker: &str,
) -> Option<(Arc<Client>, String, bool)> {
    let cfg = &ctx.cfg;
    let client = match client {
        Some(c) => c,
        None => {
            let src = match save_path {
                None => searchee.files.iter().map(|f| PathBuf::from(&f.path)).find(|p| p.exists())?,
                Some(sp) => searchee.files.iter().map(|f| Path::new(sp).join(&f.path)).find(|p| p.exists())?,
            };
            let src_dev = dev(&src).ok()?;
            let mut found = None;
            let mut error = String::new();
            for test_client in ctx.clients.iter().filter(|c| !c.info.readonly) {
                let dirs: std::collections::HashSet<String> =
                    test_client.get_all_download_dirs(&[], false, cfg).await.into_values().collect();
                if dirs.is_empty() {
                    error =
                        format!("No save paths found to test with for {}, add at least one torrent to the client.", test_client.info.label);
                    continue;
                }
                for d in dirs {
                    if src_dev != 0 && dev(Path::new(&d)).ok() == Some(src_dev) {
                        found = Some(test_client.clone());
                        break;
                    }
                    let test_path = Path::new(&d).join(CLIENT_DEST_NAME);
                    match link_file(&src, &test_path, test_type(cfg.link_type)) {
                        Ok(_) => {
                            let _ = std::fs::remove_file(&test_path);
                            found = Some(test_client.clone());
                            break;
                        }
                        Err(e) => error = e.to_string(),
                    }
                }
                if found.is_some() {
                    break;
                }
            }
            match found {
                Some(c) => c,
                None => {
                    tracing::debug!("cross-seed: {error}");
                    return None;
                }
            }
        }
    };
    match client.get_download_dir(MetaRef::of_meta(meta), false, cfg).await {
        Ok(d) => Some((client, d, true)),
        Err(DirError::InvalidData) => None,
        Err(_) => {
            let link_dir = match save_path {
                Some(sp) => get_link_dir(Path::new(sp), cfg),
                None => get_link_dir_virtual(searchee, cfg),
            }?;
            let dest = if cfg.flat_linking { link_dir } else { Path::new(&link_dir).join(tracker).to_string_lossy().into_owned() };
            Some((client, dest, false))
        }
    }
}

/// `logActionResult`: the outcome in cross-seed's words.
fn log_action_result(result: InjectionResult, meta: &Metafile, decision: Decision, searchee: &Searchee, tracker: &str) {
    let found = format!("Found {} on {tracker} by", meta.name);
    let source = format!("{} ({})", searchee.label.as_str(), searchee.name);
    let d = decision.as_str();
    match result {
        InjectionResult::Saved => tracing::info!("cross-seed: {found} {d} from {source} - saved"),
        InjectionResult::Success => tracing::info!("cross-seed: {found} {d} from {source} - injected"),
        InjectionResult::AlreadyExists => tracing::info!("cross-seed: {found} {d} from {source} - exists"),
        InjectionResult::TorrentNotComplete => tracing::warn!("cross-seed: {found} {d} from {source} - source is incomplete, saving..."),
        InjectionResult::Failure => tracing::error!("cross-seed: {found} {d} from {source} - failed to inject, saving..."),
    }
}

/// `performActionWithoutMutex`: saves or injects one match.
pub async fn perform_action_without_mutex(
    ctx: &Ctx,
    meta: &Metafile,
    decision: Decision,
    searchee_in: &Searchee,
    tracker: &str,
    inject_client: Option<&Arc<Client>>,
    only_completed: bool,
) -> ActionReturn {
    let cfg = &ctx.cfg;
    let mut searchee = searchee_in.clone();
    let log = |r: InjectionResult, s: &Searchee| log_action_result(r, meta, decision, s, tracker);
    let save = |s: &Searchee| save_to_output_dir(meta, decision, s, tracker, &ctx.output_dir);
    if cfg.action == Action::Save {
        log(InjectionResult::Saved, &searchee);
        save(&searchee);
        return ActionReturn::bare(InjectionResult::Saved, false);
    }
    let mut save_path: Option<String> = None;
    let mut destination_dir: Option<String> = None;
    let mut dir_from_client = false;
    let mut unlink_ok = false;
    let mut linked_new_files = false;

    let mut client: Option<Arc<Client>> = if ctx.clients.len() == 1 {
        ctx.clients.first().cloned()
    } else {
        ctx.clients.iter().find(|c| Some(c.info.client_host.as_str()) == searchee.client_host.as_deref() && !c.info.readonly).cloned()
    };
    let readonly_source = client.is_none() && searchee.client_host.is_some();
    if !cfg.link_dirs.is_empty() {
        match get_save_path(ctx, &mut searchee, only_completed).await {
            Err(SavePathError::TorrentNotComplete) => {
                log(InjectionResult::TorrentNotComplete, &searchee);
                save(&searchee);
                return ActionReturn {
                    client,
                    action_result: InjectionResult::TorrentNotComplete,
                    destination_dir: None,
                    linked_new_files,
                };
            }
            Err(e) => {
                tracing::error!("cross-seed: Failed to link files for {} from {}: {}", meta.name, searchee.name, e.as_str());
                log(InjectionResult::Failure, &searchee);
                save(&searchee);
                return ActionReturn::bare(InjectionResult::Failure, linked_new_files);
            }
            Ok(sp) => save_path = sp,
        }
        match get_client_and_destination_dir(ctx, client.clone(), &searchee, save_path.as_deref(), meta, tracker).await {
            Some((c, d, from_client)) => {
                client = Some(c);
                destination_dir = Some(d);
                dir_from_client = from_client;
            }
            None => client = None,
        }
    }
    let Some(client) = client else {
        tracing::error!("cross-seed: Failed to find a torrent client for {}", searchee.name);
        log(InjectionResult::Failure, &searchee);
        save(&searchee);
        return ActionReturn::bare(InjectionResult::Failure, linked_new_files);
    };
    for other in &ctx.clients {
        if other.info.client_host == client.info.client_host {
            continue;
        }
        match other.is_torrent_in_client(&meta.info_hash).await {
            Ok(false) => continue,
            Ok(true) => tracing::warn!(
                "cross-seed: Skipping {} injection into {} - already exists in {}",
                meta.name,
                client.info.client_host,
                other.info.client_host
            ),
            Err(e) => tracing::error!("cross-seed: Failed to check if {} exists in {}: {e}", meta.name, other.info.client_host),
        }
        log(InjectionResult::Failure, &searchee);
        return ActionReturn::bare(InjectionResult::Failure, linked_new_files);
    }
    if let Some(ic) = inject_client {
        if ic.info.client_host != client.info.client_host {
            tracing::warn!(
                "cross-seed: Skipping {} injection into {} - existing match is using {}",
                meta.name,
                client.info.client_host,
                ic.info.client_host
            );
            log(InjectionResult::Failure, &searchee);
            return ActionReturn::bare(InjectionResult::Failure, linked_new_files);
        }
    }
    if !cfg.link_dirs.is_empty() {
        let dest = destination_dir.clone().unwrap_or_default();
        match link_all_files_in_metafile(&searchee, meta, decision, Path::new(&dest), save_path.as_deref(), !only_completed, cfg.link_type)
        {
            Err(e) => {
                tracing::error!("cross-seed: Failed to link files for {} from {}: {e}", meta.name, searchee.name);
                log(InjectionResult::Failure, &searchee);
                save(&searchee);
                return ActionReturn::bare(InjectionResult::Failure, linked_new_files);
            }
            Ok(r) => {
                unlink_ok = !r.already_existed;
                linked_new_files = r.linked_new_files;
            }
        }
    } else if let Some(p) = &searchee.path {
        destination_dir = Some(super::searchee::dirname(p));
    } else if readonly_source {
        save_path = get_save_path(ctx, &mut searchee, only_completed).await.ok().flatten();
        let Some(sp) = save_path.clone() else {
            tracing::error!("cross-seed: Failed to find a save path for {}", searchee.name);
            log(InjectionResult::Failure, &searchee);
            save(&searchee);
            return ActionReturn::bare(InjectionResult::Failure, linked_new_files);
        };
        destination_dir = Some(sp);
    }

    let result = if dir_from_client {
        InjectionResult::AlreadyExists
    } else {
        // a match from a read-only client is added as data
        let as_data;
        let from: &Searchee = if readonly_source {
            as_data = Searchee { info_hash: None, ..searchee.clone() };
            &as_data
        } else {
            &searchee
        };
        client.inject(meta, from, decision, only_completed, destination_dir.as_deref(), cfg).await
    };
    log(result, &searchee);
    match result {
        InjectionResult::Success => {
            // the inject job may need to look at it again
            if super::clients::should_recheck(meta, decision, cfg) || searchee.info_hash.is_none() {
                save(&searchee);
            }
        }
        InjectionResult::AlreadyExists => {
            if linked_new_files {
                tracing::info!(
                    "cross-seed [{}]: Rechecking {} as new files were linked from {}",
                    client.info.label,
                    meta.name,
                    searchee.name
                );
                client.recheck_torrent(&meta.info_hash).await;
                client.spawn_resume(meta, decision, cfg);
            }
        }
        _ => {
            save(&searchee);
            if unlink_ok {
                if let Some(d) = &destination_dir {
                    unlink_metafile(meta, Path::new(d));
                    linked_new_files = false;
                }
            }
        }
    }
    if result == InjectionResult::Failure {
        return ActionReturn::bare(result, linked_new_files);
    }
    ActionReturn {
        client: Some(client),
        action_result: result,
        destination_dir: destination_dir.or(searchee.save_path.clone()),
        linked_new_files,
    }
}

/// Whether `path` is a Windows path where this is not Windows, or the other way round (`ABS_WIN_PATH_REGEX`
/// against `path.sep`), which linking can't bridge.
pub fn is_cross_platform_path(path: &str) -> bool {
    test(&ABS_WIN_PATH_REGEX, path) == cfg!(unix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crossseed::searchee::File;
    use crate::crossseed::torrent::build;

    /// A fresh folder for one test.
    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lantern-xs-link-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(p: &Path, bytes: usize) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, vec![b'x'; bytes]).unwrap();
    }

    #[cfg(unix)]
    fn ino(p: &Path) -> u64 {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(p).unwrap().ino()
    }

    #[test]
    fn link_types_as_cross_seed_makes_them() {
        let d = dir("types");
        let src = d.join("src/a.mkv");
        write(&src, 10);
        // a symbolic link to the file is followed to the file itself first (unwrapSymlinks)
        let via = d.join("via.mkv");
        symlink(&src, &via).unwrap();
        assert_eq!(unwrap_symlinks(&via).unwrap(), src);

        let hard = d.join("hard.mkv");
        assert!(link_file(&via, &hard, LinkType::Hardlink).unwrap());
        #[cfg(unix)]
        assert_eq!(ino(&hard), ino(&src), "a hard link is the same file");
        assert!(!link_file(&via, &hard, LinkType::Hardlink).unwrap(), "EEXIST: already linked");

        let soft = d.join("soft.mkv");
        assert!(link_file(&via, &soft, LinkType::Symlink).unwrap());
        assert_eq!(std::fs::read_link(&soft).unwrap(), src, "a symlink to the resolved file");

        let copy = d.join("copy.mkv");
        assert!(link_file(&src, &copy, LinkType::ReflinkOrCopy).unwrap());
        assert_eq!(std::fs::read(&copy).unwrap(), std::fs::read(&src).unwrap());
        assert!(!link_file(&src, &copy, LinkType::ReflinkOrCopy).unwrap());
        // a forced reflink fails on a file system without them, and leaves nothing behind
        let refl = d.join("reflink.mkv");
        if link_file(&src, &refl, LinkType::Reflink).is_err() {
            assert!(!refl.exists());
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn links_every_file_of_a_match() {
        let d = dir("all");
        let save = d.join("downloads");
        write(&save.join("Show.S01/Show.S01E01.mkv"), 700);
        write(&save.join("Show.S01/Show.S01E02.mkv"), 800);
        let searchee = Searchee {
            info_hash: Some("a".repeat(40)),
            name: "Show.S01".into(),
            files: vec![
                File { name: "Show.S01E01.mkv".into(), path: "Show.S01/Show.S01E01.mkv".into(), length: 700 },
                File { name: "Show.S01E02.mkv".into(), path: "Show.S01/Show.S01E02.mkv".into(), length: 800 },
            ],
            ..Searchee::default()
        };
        // MATCH_SIZE_ONLY: other names, paired by size
        let other = Metafile::decode(&build("Show.S01.WEB", &[("e01.mkv", 700), ("e02.mkv", 800)], "b")).unwrap();
        let dest = d.join("links/Site B");
        let sp = save.to_string_lossy().into_owned();
        let r =
            link_all_files_in_metafile(&searchee, &other, Decision::MatchSizeOnly, &dest, Some(&sp), false, LinkType::Hardlink).unwrap();
        assert_eq!(r, LinkResult { already_existed: false, linked_new_files: true });
        assert_eq!(std::fs::metadata(dest.join("Show.S01.WEB/e01.mkv")).unwrap().len(), 700);
        assert_eq!(std::fs::metadata(dest.join("Show.S01.WEB/e02.mkv")).unwrap().len(), 800);
        // again: every file is there already
        let r =
            link_all_files_in_metafile(&searchee, &other, Decision::MatchSizeOnly, &dest, Some(&sp), false, LinkType::Hardlink).unwrap();
        assert_eq!(r, LinkResult { already_existed: true, linked_new_files: false });
        // MATCH keeps the paths of the torrent
        let same = Metafile::decode(&build("Show.S01", &[("Show.S01E01.mkv", 700), ("Show.S01E02.mkv", 800)], "c")).unwrap();
        let dest2 = d.join("links/Site C");
        link_all_files_in_metafile(&searchee, &same, Decision::Match, &dest2, Some(&sp), false, LinkType::Symlink).unwrap();
        assert!(dest2.join("Show.S01/Show.S01E02.mkv").symlink_metadata().unwrap().file_type().is_symlink());
        // a missing source fails, unless missing files are allowed (the inject job)
        let more = Metafile::decode(&build("Show.S01.X", &[("e01.mkv", 700), ("e09.mkv", 900)], "d")).unwrap();
        let missing =
            Searchee { files: vec![File { name: "gone.mkv".into(), path: "Show.S01/gone.mkv".into(), length: 900 }], ..searchee.clone() };
        let dest3 = d.join("links/Site D");
        let e =
            link_all_files_in_metafile(&missing, &more, Decision::MatchPartial, &dest3, Some(&sp), false, LinkType::Hardlink).unwrap_err();
        assert_eq!(e, format!("Linking failed, {} not found.", save.join("Show.S01/gone.mkv").display()));
        let r = link_all_files_in_metafile(&missing, &more, Decision::MatchPartial, &dest3, Some(&sp), true, LinkType::Hardlink).unwrap();
        assert!(!r.linked_new_files);
        // unlinkMetafile takes the match's folder away again, never the link folder
        unlink_metafile(&other, &dest);
        assert!(!dest.join("Show.S01.WEB").exists());
        assert!(dest.exists());
        assert!(save.join("Show.S01/Show.S01E01.mkv").exists(), "the source stays");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_link_folder_on_the_same_drive() {
        let d = dir("linkdir");
        write(&d.join("data/Show/a.mkv"), 5);
        let cfg = RuntimeConfig { link_dirs: vec![d.join("links").to_string_lossy().into_owned()], ..RuntimeConfig::default() };
        std::fs::create_dir_all(d.join("links")).unwrap();
        assert_eq!(get_link_dir(&d.join("data/Show"), &cfg).as_deref(), Some(cfg.link_dirs[0].as_str()));
        // no test files are left behind
        assert!(!d.join("links").join(LINK_DIR_DEST_NAME).exists());
        let none = RuntimeConfig { link_dirs: vec!["/nonexistent-link-dir".into()], ..RuntimeConfig::default() };
        assert_eq!(get_link_dir(&d.join("data/Show"), &none), None, "hardlinks need a folder on the same drive");
        let sym = RuntimeConfig { link_type: LinkType::Symlink, ..none.clone() };
        assert_eq!(get_link_dir(&d.join("data/Show"), &sym).as_deref(), Some("/nonexistent-link-dir"), "symlinks take the first");
        assert!(test_linking(&d.join("data"), "a.cross-seed", "b.cross-seed", &cfg).unwrap());
        let e = test_linking(&d.join("data"), "a.cross-seed", "b.cross-seed", &none).unwrap_err();
        assert_eq!(
            e,
            format!(
                "Failed to create a test hardlink from {} in any linkDirs: [\"/nonexistent-link-dir\"]. Ensure that hardlink is supported between these paths (hardlink/reflink requires same drive, partition, and volume).",
                d.join("data").display()
            )
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn saved_torrent_names() {
        let m = Metafile::decode(&build("Show.S01.1080p-GRP", &[("a.mkv", 1)], "z")).unwrap();
        let p = get_torrent_save_path(&m, MediaType::Season, "Site B", Path::new("/out"), false);
        assert_eq!(p, PathBuf::from(format!("/out/[pack][Site B]Show.S01.1080p-GRP[{}].torrent", m.info_hash)));
        let long = Metafile::decode(&build(&"N".repeat(300), &[("a.mkv", 1)], "z")).unwrap();
        let p = get_torrent_save_path(&long, MediaType::Movie, "T", Path::new("/out"), true);
        let shown = p.to_string_lossy().into_owned();
        assert!(shown.len() <= MAX_PATH_BYTES, "{}", shown.len());
        assert!(shown.ends_with(&format!("...[{}].cached.torrent", long.info_hash)));
        let d = dir("save");
        save_to_output_dir(&m, Decision::Match, &Searchee::default(), "Site B", &d);
        let saved = std::fs::read(get_torrent_save_path(&m, media_type_of(&m.title, &m.files), "Site B", &d, false)).unwrap();
        assert_eq!(saved, m.raw);
        let _ = std::fs::remove_dir_all(&d);
    }
}
