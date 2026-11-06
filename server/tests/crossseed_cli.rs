//! `lantern cross-seed <command>`, run through its entry function on a temporary data folder: the commands that
//! need only the data folder, the torrent cache commands, `tree` and `diff`, the commands that run cross-seed
//! (their settings checked first, as cross-seed checks them) and the daemon's API. Nothing here reaches the
//! internet.

use kit_server_core::{p, Db, RowExt};
use serde_json::json;
use server::crossseed::cli::{run, Context};
use server::crossseed::torrent;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A fresh data folder `name` under the temporary folder, and the context the commands run in.
fn folder(name: &str) -> (PathBuf, Context) {
    let dir = std::env::temp_dir().join(format!("lantern-xscli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let web = dir.join("static");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html>").unwrap();
    let vars: HashMap<String, String> = HashMap::new();
    let ctx = Context {
        data_dir: dir.join("data"),
        static_dir: web,
        env: Arc::new(move |n| vars.get(n).cloned()),
        retry_delay: Duration::from_millis(10),
    };
    (dir, ctx)
}

/// Runs `cross-seed <args>`: the exit code, what it printed and what it printed as problems.
async fn cs(ctx: &Context, args: &[&str]) -> (i32, String, String) {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let code = run(&args, ctx, &mut out, &mut err).await;
    (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
}

/// The state on the same data folder, as the server would open it.
async fn state(ctx: &Context) -> server::AppState {
    server::AppState::open(&ctx.data_dir, ctx.static_dir.clone(), |_| None).await.unwrap()
}

/// A `.torrent` file with announce addresses.
fn with_trackers(t: Vec<u8>, announce: &str, list: &[&str]) -> Vec<u8> {
    let s = |x: &str| format!("{}:{x}", x.len());
    let t = torrent::with_top_key(&t, "announce", s(announce).as_bytes()).unwrap();
    let tiers: String = list.iter().map(|u| format!("l{}e", s(u))).collect();
    torrent::with_top_key(&t, "announce-list", format!("l{tiers}e").as_bytes()).unwrap()
}

#[tokio::test]
async fn the_command_line_is_read_as_commander_reads_it() {
    let (dir, ctx) = folder("args");
    let (code, out, _) = cs(&ctx, &["--version"]).await;
    assert_eq!((code, out.as_str()), (0, "6.13.9\n"));
    let (code, out, _) = cs(&ctx, &["--help"]).await;
    assert_eq!(code, 0);
    assert!(out.starts_with("Usage: cross-seed [options] [command]\n"), "{out}");
    assert!(out.contains("  update-torrent-cache-trackers <old-announce-url> <new-announce-url>"), "{out}");
    let (code, _, err) = cs(&ctx, &["nope"]).await;
    assert_eq!((code, err.as_str()), (1, "error: unknown command 'nope'\n(add --help for additional information)\n"));
    let (code, _, err) = cs(&ctx, &["diff", "a.torrent"]).await;
    assert_eq!((code, err.as_str()), (1, "error: missing required argument 'candidate'\n(add --help for additional information)\n"));
    let (code, _, err) = cs(&ctx, &["search", "--match-mode", "safe"]).await;
    assert_eq!(code, 1);
    assert!(
        err.starts_with("error: option '--match-mode <mode>' argument 'safe' is invalid. Allowed choices are strict, flexible, partial.")
    );
    let (code, out, _) = cs(&ctx, &["search", "--help"]).await;
    assert_eq!(code, 0);
    assert!(out.contains("--no-exclude-older") && !out.contains("--torrents"), "hidden as cross-seed hides it: {out}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn api_key_gen_config_and_the_caches_work_on_the_data_folder() {
    let (dir, ctx) = folder("minimal");
    // api-key: made when there is none (cross-seed's 24 random bytes), then shown again
    let (code, first, _) = cs(&ctx, &["api-key"]).await;
    assert_eq!(code, 0);
    let first = first.trim().to_string();
    assert_eq!(first.len(), 48);
    assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
    let (_, again, _) = cs(&ctx, &["api-key", "--api-key", "ignored-as-in-cross-seed-0000"]).await;
    assert_eq!(again.trim(), first);
    let (code, reset, _) = cs(&ctx, &["reset-api-key"]).await;
    assert_eq!(code, 0);
    assert_ne!(reset.trim(), first);
    assert_eq!(state(&ctx).await.settings.str("api_key"), reset.trim());

    // gen-config: the template values once, then "already exists"
    let (code, out, _) = cs(&ctx, &["gen-config"]).await;
    assert_eq!(code, 0);
    assert!(out.starts_with("Configuration file created at "), "{out}");
    assert!(ctx.data_dir.join("torrent_cache").is_dir());
    let saved = state(&ctx).await.settings.saved().await.unwrap();
    assert_eq!(saved.get("cross_seed_match_mode"), Some(&json!("flexible")));
    assert_eq!(saved.get("cross_seed_exclude_older"), Some(&json!("2 weeks")));
    let (_, out, _) = cs(&ctx, &["gen-config"]).await;
    assert_eq!(out, "Configuration file already exists.\n");

    // clear-cache and clear-client-cache
    let st = state(&ctx).await;
    st.db.execute("INSERT INTO cross_seeds (grab_id, indexer_id, title, info_hash, decision, at) VALUES (1,1,'a','','files_differ',1),(1,1,'b','abc','injected',1)", p![]).unwrap();
    st.db
        .execute("INSERT INTO cross_seed_timestamps (searchee, indexer_id, first_searched, last_searched) VALUES ('a',1,1,1)", p![])
        .unwrap();
    st.db.execute("INSERT INTO cross_seed_searches (grab_id, at) VALUES (1,1)", p![]).unwrap();
    st.db.execute("INSERT INTO cross_seed_data (path, title) VALUES ('/x','x')", p![]).unwrap();
    st.db.execute("INSERT INTO cross_seed_torrents (info_hash, name, file_path) VALUES ('h','n','/t.torrent')", p![]).unwrap();
    st.db
        .execute(
            "INSERT INTO cross_seed_client_searchees (info_hash, name, title, files, length, client_host, save_path, trackers) VALUES ('h','n','n','[]',1,'c','/','[]')",
            p![],
        )
        .unwrap();
    st.db.execute("INSERT INTO cross_seed_ensemble (path, ensemble, element) VALUES ('/e','show','1')", p![]).unwrap();
    let count = |t: &str| st.db.one(&format!("SELECT COUNT(*) AS n FROM {t}"), p![]).unwrap().unwrap().i("n");
    let (code, out, _) = cs(&ctx, &["clear-cache"]).await;
    assert_eq!((code, out.as_str()), (0, "Clearing cache...\n"));
    assert_eq!((count("cross_seeds"), count("cross_seed_timestamps"), count("cross_seed_searches")), (1, 0, 0));
    assert_eq!(count("cross_seed_data"), 1, "the client cache is kept");
    let (code, out, _) = cs(&ctx, &["clear-client-cache"]).await;
    assert_eq!((code, out.as_str()), (0, "Clearing client cache...\n"));
    for t in ["cross_seed_torrents", "cross_seed_client_searchees", "cross_seed_data", "cross_seed_ensemble"] {
        assert_eq!(count(t), 0, "{t}");
    }
    assert_eq!(count("cross_seeds"), 1, "decisions with a torrent stay");

    // clear-indexer-failures: the torrent indexers off hold
    let id = st.indexers.add(&json!({ "kind": "torznab", "name": "Site", "base_url": "http://127.0.0.1:9", "settings": {} })).unwrap();
    st.indexers.record_failure(id, "The site didn't answer");
    assert!(st.indexers.blocked_until(&st.indexers.get(id).unwrap()).is_some());
    let (code, out, _) = cs(&ctx, &["clear-indexer-failures"]).await;
    assert_eq!((code, out.as_str()), (0, ""));
    assert!(st.indexers.blocked_until(&st.indexers.get(id).unwrap()).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn update_torrent_cache_trackers_rewrites_announce_addresses() {
    let (dir, ctx) = folder("trackers");
    let cache = ctx.data_dir.join("torrent_cache");
    std::fs::create_dir_all(&cache).unwrap();
    let t = with_trackers(
        torrent::build("Movie.2020.1080p-GRP.mkv", &[("Movie.2020.1080p-GRP.mkv", 100)], "a"),
        "https://old.example/announce/key",
        &["https://old.example/announce/key", "https://other.example/announce"],
    );
    let before = torrent::Metafile::decode(&t).unwrap().info_hash;
    std::fs::write(cache.join("a.cached.torrent"), &t).unwrap();
    std::fs::write(cache.join("b.cached.torrent"), torrent::build("Other", &[("Other", 1)], "b")).unwrap();
    std::fs::write(cache.join("broken.torrent"), b"not bencode").unwrap();
    let (code, out, err) = cs(&ctx, &["update-torrent-cache-trackers", "old.example", "new.example"]).await;
    assert_eq!(code, 0);
    let a = cache.join("a.cached.torrent");
    assert_eq!(
        out,
        format!(
            "Found 3 files in cache, processing...\n#1: {}\n--- https://old.example/announce/key -> https://new.example/announce/key\n--- https://old.example/announce/key -> https://new.example/announce/key\n",
            a.display()
        )
    );
    assert!(err.starts_with(&format!("Error reading {}", cache.join("broken.torrent").display())), "{err}");
    let after = torrent::Metafile::decode(&std::fs::read(&a).unwrap()).unwrap();
    assert_eq!(after.info_hash, before, "the info dictionary is left alone");
    assert_eq!(after.trackers, ["new.example", "other.example"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn tree_and_diff_print_as_cross_seed_prints() {
    let (dir, ctx) = folder("tree");
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.torrent");
    let b = dir.join("b.torrent");
    let c = dir.join("c.torrent");
    let files = [("Show.S01E01.mkv", 700), ("Show.S01E02.mkv", 800)];
    std::fs::write(&a, with_trackers(torrent::build("Show.S01", &files, "a"), "https://t.example/a", &[])).unwrap();
    std::fs::write(&b, torrent::build("Show.S01", &files, "b")).unwrap();
    std::fs::write(&c, torrent::build("Show.S01", &[("Show.S01E01.mkv", 700), ("Show.S01E02.mkv", 801)], "c")).unwrap();
    let meta = torrent::Metafile::decode(&std::fs::read(&a).unwrap()).unwrap();
    let (code, out, _) = cs(&ctx, &["tree", a.to_str().unwrap()]).await;
    assert_eq!(code, 0);
    assert_eq!(
        out,
        format!(
            "Use `cross-seed diff` to compare two .torrent files
{{
  infoHash: '{}',
  length: 1500,
  name: 'Show.S01',
  title: 'Show.S01',
  pieceLength: {},
  files: [
    {{
      name: 'Show.S01E01.mkv',
      length: 700,
      path: 'Show.S01/Show.S01E01.mkv'
    }},
    {{
      name: 'Show.S01E02.mkv',
      length: 800,
      path: 'Show.S01/Show.S01E02.mkv'
    }}
  ],
  trackers: [ 't.example' ]
}}
",
            meta.info_hash, meta.piece_length
        )
    );
    let (code, out, _) = cs(&ctx, &["diff", a.to_str().unwrap(), b.to_str().unwrap()]).await;
    assert_eq!(code, 0);
    assert!(out.ends_with("\nTorrents are equal\n"), "{out}");
    assert!(out.starts_with("Use `cross-seed tree` on each .torrent file to display their full structure\n[\n  {"), "{out}");
    let (_, out, _) = cs(&ctx, &["diff", a.to_str().unwrap(), c.to_str().unwrap()]).await;
    assert!(out.contains("AssertionError [ERR_ASSERTION]: Expected values to be strictly deep-equal:\n+ actual - expected\n"), "{out}");
    assert!(out.contains("+     length: 800,\n-     length: 801,"), "{out}");
    assert!(out.contains("operator: 'deepStrictEqual'"), "{out}");
    let (code, _, err) = cs(&ctx, &["tree", dir.join("missing.torrent").to_str().unwrap()]).await;
    assert_eq!(code, 1);
    assert!(err.starts_with("Error: ENOENT: no such file or directory, open '"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn running_commands_check_the_settings_first() {
    let (dir, ctx) = folder("checks");
    // Lantern's defaults with no client: cross-seed's own complaints, then nothing runs
    let (code, out, err) = cs(&ctx, &["search", "--delay", "5"]).await;
    assert_eq!(code, 1);
    assert!(out.contains("Validating your configuration..."), "{out}");
    assert!(err.contains("delay must be 30 seconds to 1 hour."), "{err}");
    assert!(err.contains("You need to have a client configured for useClientTorrents."), "{err}");
    assert!(err.trim_end().ends_with("Your configuration is invalid, please see the errors above for details."), "{err}");
    // a folder that isn't there
    let (code, _, err) = cs(&ctx, &["search", "-A", "save", "--no-use-client-torrents", "--data-dirs", "/nonexistent-lantern-data"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("\tYour dataDir0 \"/nonexistent-lantern-data\" has invalid permissions: does not exist."), "{err}");
    // valid: the search runs (no indexers, so nothing is found) and the output folder is made
    let media = dir.join("media");
    std::fs::create_dir_all(&media).unwrap();
    let (code, out, err) = cs(&ctx, &["search", "-A", "save", "--no-use-client-torrents", "--data-dirs", media.to_str().unwrap()]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("Your configuration is valid!"), "{out}");
    assert!(ctx.data_dir.join("cross-seeds").is_dir());
    // one RSS scan (no indexers here, so nothing is read)
    let (code, out, err) = cs(&ctx, &["rss", "-A", "save", "--no-use-client-torrents", "--data-dirs", media.to_str().unwrap()]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("Your configuration is valid!"), "{out}");
    // the test notification, to a local stand-in
    let got: Arc<Mutex<Vec<serde_json::Value>>> = Arc::default();
    let seen = got.clone();
    let app = axum::Router::new().route(
        "/hook",
        axum::routing::post(move |axum::Json(v): axum::Json<serde_json::Value>| {
            let seen = seen.clone();
            async move {
                seen.lock().unwrap().push(v);
                "ok"
            }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hook = format!("http://{}/hook", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let (code, _, err) = cs(
        &ctx,
        &[
            "test-notification",
            "-A",
            "save",
            "--no-use-client-torrents",
            "--data-dirs",
            media.to_str().unwrap(),
            "--notification-webhook-urls",
            &hook,
        ],
    )
    .await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(*got.lock().unwrap(), [json!({ "title": "cross-seed", "body": "Test", "extra": { "event": "TEST" } })]);
    // inject from a folder given on the command line
    let inject = dir.join("inject");
    std::fs::create_dir_all(&inject).unwrap();
    let (code, _, err) = cs(
        &ctx,
        &[
            "inject",
            "-A",
            "save",
            "--no-use-client-torrents",
            "--data-dirs",
            media.to_str().unwrap(),
            "--inject-dir",
            inject.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(code, 0, "{err}");
    // the options are for this run only
    let st = state(&ctx).await;
    assert!(st.cross_seed.overlay().is_none());
    assert_eq!(st.settings.str("cross_seed_action"), "inject");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn restore_copies_the_torrent_cache_for_inject() {
    let (dir, ctx) = folder("restore");
    let st = state(&ctx).await;
    st.indexers.add(&json!({ "kind": "torznab", "name": "Site A", "base_url": "http://tracker.example", "settings": {} })).unwrap();
    let cache = ctx.data_dir.join("torrent_cache");
    std::fs::create_dir_all(&cache).unwrap();
    let known = with_trackers(
        torrent::build("Movie.2020.1080p-GRP.mkv", &[("Movie.2020.1080p-GRP.mkv", 100)], "a"),
        "http://tracker.example/announce",
        &[],
    );
    let unknown = torrent::build("Other.2021.720p-GRP.mkv", &[("Other.2021.720p-GRP.mkv", 50)], "b");
    let km = torrent::Metafile::decode(&known).unwrap();
    let um = torrent::Metafile::decode(&unknown).unwrap();
    std::fs::write(cache.join(format!("{}.cached.torrent", km.info_hash)), &known).unwrap();
    std::fs::write(cache.join(format!("{}.cached.torrent", um.info_hash)), &unknown).unwrap();
    std::fs::write(cache.join("deadbeef.cached.torrent"), b"broken").unwrap();
    let media = dir.join("media");
    std::fs::create_dir_all(&media).unwrap();
    let base = ["restore", "-A", "save", "--no-use-client-torrents", "--data-dirs", media.to_str().unwrap()];
    let (code, out, err) = cs(&ctx, &base).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("Found 3 .torrent files to restore from cache\n"), "{out}");
    assert!(
        out.contains(
            "Copied 3/3 .torrent files from cache to outputDir, run \"cross-seed inject\" to inject into client using your dataDirs\n"
        ),
        "{out}"
    );
    assert!(out.contains("Copied 2 .torrent files without filename metadata\n"), "{out}");
    assert!(err.contains("deadbeef.cached.torrent, filename metadata will be unknown"), "{err}");
    let mut names: Vec<String> = std::fs::read_dir(ctx.data_dir.join("cross-seeds"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let mut want = vec![
        format!("[movie][Site A]Movie.2020.1080p-GRP[{}].cached.torrent", km.info_hash),
        format!("[movie][UnknownTracker]Other.2021.720p-GRP[{}].cached.torrent", um.info_hash),
        "[unknown][UnknownTracker]deadbeef.cached.torrent".to_string(),
    ];
    want.sort();
    assert_eq!(names, want);
    // a second run copies nothing new
    let (_, out, _) = cs(&ctx, &base).await;
    assert!(out.contains("Copied 0/3"), "{out}");
    // an empty cache
    std::fs::remove_dir_all(&cache).unwrap();
    let (_, out, _) = cs(&ctx, &base).await;
    assert!(out.contains("No .torrent files found to restore from cache\n"), "{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// An output a test reads while the command still runs.
#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn the_daemon_serves_cross_seeds_api_with_the_given_port_and_key() {
    let (dir, mut ctx) = folder("daemon");
    // Lantern's own schedule is off: the daemon runs the jobs anyway, as cross-seed's daemon does
    ctx.env = Arc::new(|n| (n == "APP_CROSS_SEED").then(|| "false".to_string()));
    let media = dir.join("media");
    std::fs::create_dir_all(&media).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let key = "0123456789abcdef01234567";
    let args: Vec<String> = [
        "daemon",
        "-p",
        &port.to_string(),
        "--host",
        "127.0.0.1",
        "--api-key",
        key,
        "-A",
        "save",
        "--no-use-client-torrents",
        "--data-dirs",
        media.to_str().unwrap(),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let out = Buf::default();
    let shown = out.clone();
    let task_ctx = ctx.clone();
    let task = tokio::spawn(async move {
        let mut out = out;
        let mut err = Buf::default();
        run(&args, &task_ctx, &mut out, &mut err).await
    });
    let mut up = false;
    for _ in 0..200 {
        if String::from_utf8_lossy(&shown.0.lock().unwrap()).contains(&format!("Server is running on port {port}, ^C to stop.")) {
            up = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(up, "{}", String::from_utf8_lossy(&shown.0.lock().unwrap()));
    let http = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");
    assert_eq!(http.get(format!("{base}/api/ping")).send().await.unwrap().status(), 200);
    assert_eq!(http.get(format!("{base}/api/status?apikey={key}")).send().await.unwrap().status(), 200);
    assert_eq!(http.get(format!("{base}/api/status?apikey=wrong")).send().await.unwrap().status(), 401);
    let st = state(&ctx).await;
    let mut ran = false;
    for _ in 0..200 {
        let jobs = st.db.query("SELECT name FROM cross_seed_job_log", p![]).unwrap();
        if jobs.iter().any(|r| r.s("name") == "rss") && jobs.iter().any(|r| r.s("name") == "cleanup") {
            ran = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(ran, "the rss and cleanup jobs ran on schedule");
    task.abort();
    let _ = std::fs::remove_dir_all(&dir);
}
