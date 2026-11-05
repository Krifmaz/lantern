// Ported from autobrr v1.90.0 (internal/domain/filter.go FilterExternal, internal/filter/service.go
// RunExternalFilters, TestExternal, execCmd, webhook), GPL-2.0-or-later.
//! External filters: after a release passes a filter's own checks, a program or a web address gets the last
//! word. The program's exit code, or the web address's status code, must be the one the filter expects. A
//! program is run with its arguments split like a shell would, never through one.

use super::action::contains_any_macro;
use super::macros::Macro;
use super::rejections::RejectionReasons;
use super::release::{IndexerMinimal, Release};
use super::run::{look_path, write_temporary_file, ReleaseDownloader};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// The most output of a program or web address kept (autobrr's `externalOutputMaxBytes`).
pub const EXTERNAL_OUTPUT_MAX_BYTES: usize = 4096;
/// The error for a type autobrr doesn't know (autobrr's `ErrExternalFilterTypeUnsupported`).
pub const ERR_TYPE_UNSUPPORTED: &str = "unsupported external filter type";

/// For serde: leave out a zero.
fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// One external filter (autobrr's `FilterExternal`, with its JSON names).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilterExternal {
    /// Its id.
    pub id: i64,
    /// The name people see.
    pub name: String,
    /// Its place in the list.
    pub index: i64,
    /// `EXEC` or `WEBHOOK`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Runs at all.
    pub enabled: bool,
    /// EXEC: the program.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub exec_cmd: String,
    /// EXEC: its arguments (macros).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub exec_args: String,
    /// EXEC: the exit code that lets the release through.
    #[serde(skip_serializing_if = "is_zero")]
    pub exec_expect_status: i64,
    /// WEBHOOK: the address.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub webhook_host: String,
    /// WEBHOOK: the method (POST when empty).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub webhook_method: String,
    /// WEBHOOK: the body (macros).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub webhook_data: String,
    /// WEBHOOK: headers as `Name=value;Other=value`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub webhook_headers: String,
    /// WEBHOOK: the status code that lets the release through.
    #[serde(skip_serializing_if = "is_zero")]
    pub webhook_expect_status: i64,
    /// WEBHOOK: status codes that mean "try again", comma separated.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub webhook_retry_status: String,
    /// WEBHOOK: how many tries (1 when 0).
    #[serde(skip_serializing_if = "is_zero")]
    pub webhook_retry_attempts: i64,
    /// WEBHOOK: seconds between tries.
    #[serde(skip_serializing_if = "is_zero")]
    pub webhook_retry_delay_seconds: i64,
    /// `CONTINUE` (an error lets the release through) or `REJECT`.
    pub on_error: String,
    /// The filter it belongs to (not sent).
    #[serde(skip)]
    pub filter_id: i64,
}

impl FilterExternal {
    /// The fields whose macros could name the torrent.
    fn macro_fields(&self) -> [&str; 2] {
        [&self.exec_args, &self.webhook_data]
    }

    /// Whether it needs the torrent fetched (autobrr's `NeedTorrentDownloaded`).
    pub fn need_torrent_downloaded(&self) -> bool {
        contains_any_macro(&self.macro_fields(), &["TorrentPathName", "TorrentTmpFile", "TorrentDataRawBytes", "TorrentHash"])
    }

    /// Whether it needs the torrent written to a file (autobrr's `NeedTorrentTmpFile`).
    pub fn need_torrent_tmp_file(&self) -> bool {
        contains_any_macro(&self.macro_fields(), &["TorrentPathName", "TorrentTmpFile"])
    }
}

/// What a test run found (autobrr's `FilterExternalTestResult`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TestResult {
    /// `EXEC` or `WEBHOOK`.
    #[serde(rename = "type")]
    pub kind: String,
    /// No error and the expected status.
    pub success: bool,
    /// The exit or status code.
    pub status: i64,
    /// The code expected.
    pub expect_status: i64,
    /// What the program printed or the web address answered (at most 4096 bytes).
    pub output: String,
    /// What went wrong.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
    /// How long it took.
    pub duration_ms: i64,
}

/// `s` cut to [`EXTERNAL_OUTPUT_MAX_BYTES`].
fn capped(b: &[u8]) -> String {
    String::from_utf8_lossy(&b[..b.len().min(EXTERNAL_OUTPUT_MAX_BYTES)]).into_owned()
}

/// Runs the program; its exit code and output (stdout then stderr, capped) (autobrr's `execCmd`).
pub async fn exec_cmd(ext: &FilterExternal, release: &Release) -> Result<(i64, String), (i64, String, String)> {
    let cmd =
        look_path(&ext.exec_cmd).map_err(|e| (0, String::new(), format!("exec failed, could not find program: {}: {e}", ext.exec_cmd)))?;
    let args = Macro::new(release).parse(&ext.exec_args).map_err(|e| (0, String::new(), format!("could not parse macro: {e}")))?;
    let args = super::shellquote::split(&args).map_err(|e| (0, String::new(), format!("could not parse into shell-words: {e}")))?;
    let out = tokio::process::Command::new(&cmd)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| (0, String::new(), format!("error running command: {e}")))?;
    let mut both = out.stdout.clone();
    both.extend_from_slice(&out.stderr);
    let output = capped(&both);
    match out.status.code() {
        Some(c) => Ok((i64::from(c), output)),
        None => Ok((-1, output)),
    }
}

/// Calls the web address; its status code and body (capped) (autobrr's `webhook`), trying again on the retry
/// status codes.
pub async fn webhook(http: &reqwest::Client, ext: &FilterExternal, release: &Release) -> Result<(i64, String), String> {
    if ext.webhook_host.is_empty() {
        return Err("external filter: missing host for webhook".into());
    }
    let data = Macro::new(release)
        .parse(&ext.webhook_data)
        .map_err(|e| format!("could not parse webhook data macro: {}: {e}", ext.webhook_data))?;
    let method = if ext.webhook_method.is_empty() { "POST" } else { ext.webhook_method.as_str() };
    let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| format!("could not build request for webhook: {e}"))?;
    let headers: Vec<(String, String)> = ext
        .webhook_headers
        .split(';')
        .filter(|_| !ext.webhook_headers.is_empty())
        .filter_map(|h| {
            let p: Vec<&str> = h.split('=').collect();
            (p.len() == 2).then(|| (p[0].to_string(), p[1].to_string()))
        })
        .collect();
    let attempts = if ext.webhook_retry_attempts == 0 { 1 } else { ext.webhook_retry_attempts };
    let delay = if ext.webhook_retry_delay_seconds > 0 {
        Duration::from_secs(ext.webhook_retry_delay_seconds as u64)
    } else {
        Duration::from_millis(100)
    };
    let retry: Vec<String> = if ext.webhook_retry_status.is_empty() {
        Vec::new()
    } else {
        ext.webhook_retry_status.replace(' ', "").split(',').map(str::to_string).collect()
    };
    let mut last = String::new();
    for i in 0..attempts.max(1) {
        if i > 0 {
            tokio::time::sleep(delay).await;
        }
        let mut req = http
            .request(method.clone(), &ext.webhook_host)
            .header("Content-Type", "application/json")
            .header("User-Agent", super::clients::user_agent());
        for (k, v) in &headers {
            req = req.header(k.as_str(), v.as_str());
        }
        if !ext.webhook_data.is_empty() && !data.is_empty() {
            req = req.body(data.clone());
        }
        let res = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                last = format!("could not make request for webhook: {}", super::clients::req_err(method.as_str(), &ext.webhook_host, &e));
                continue;
            }
        };
        let code = res.status().as_u16();
        let body = match res.bytes().await {
            Ok(b) => capped(&b),
            Err(e) => {
                last = format!("could not read response body: {e}");
                continue;
            }
        };
        if retry.contains(&code.to_string()) {
            last = format!("webhook got unwanted status code: {code}");
            continue;
        }
        return Ok((i64::from(code), body));
    }
    Err(last)
}

/// Runs the filter's enabled external filters in order (autobrr's `RunExternalFilters`): whether the release
/// passes; a rejection is added to `rr`.
pub async fn run_external_filters(
    externals: &[FilterExternal],
    release: &mut Release,
    downloads: &dyn ReleaseDownloader,
    http: &reqwest::Client,
    tmp_dir: &std::path::Path,
    rr: &mut RejectionReasons,
) -> Result<bool, String> {
    for ext in externals.iter().filter(|e| e.enabled) {
        if ext.need_torrent_downloaded() {
            let name = release.torrent_name.clone();
            downloads.download_release(release).await.map_err(|e| format!("could not download torrent file for release: {name}: {e}"))?;
            if ext.need_torrent_tmp_file() {
                write_temporary_file(release, tmp_dir).map_err(|e| format!("could not write torrent file for release: {name}: {e}"))?;
            }
        }
        match ext.kind.as_str() {
            "EXEC" => match exec_cmd(ext, release).await {
                Err((_, _, e)) => {
                    if ext.on_error == "CONTINUE" {
                        continue;
                    }
                    return Err(format!("error executing external command: {e}"));
                }
                Ok((code, _)) if code != ext.exec_expect_status => {
                    rr.add("external script exit code", code, ext.exec_expect_status);
                    return Ok(false);
                }
                Ok(_) => {}
            },
            "WEBHOOK" => match webhook(http, ext, release).await {
                Err(e) => {
                    if ext.on_error == "CONTINUE" {
                        continue;
                    }
                    return Err(format!("error executing external webhook: {e}"));
                }
                Ok((code, _)) if code != ext.webhook_expect_status => {
                    rr.add("external webhook status code", code, ext.webhook_expect_status);
                    return Ok(false);
                }
                Ok(_) => {}
            },
            _ => {}
        }
    }
    Ok(true)
}

/// The release a test run uses (autobrr's `newSampleRelease`).
pub fn sample_release() -> Release {
    let mut r = Release::new(IndexerMinimal {
        id: 0,
        name: "MockIndexer".into(),
        identifier: "mock".into(),
        identifier_external: "MockIndexer".into(),
    });
    r.parse_string("Best.Show.Ever.S18E21.1080p.AMZN.WEB-DL.DDP2.0.H.264-GROUP");
    r.filter_name = "TV".into();
    r.category = "TV".into();
    r.size = 1_500_000_000;
    r.torrent_id = "12345".into();
    r.info_url = "https://mock.local/torrents/12345".into();
    r.download_url = "https://mock.local/torrents/12345/download".into();
    r.uploader = "Uploader".into();
    r
}

/// Runs one external filter against the sample release (autobrr's `TestExternal`); an error for an unknown type.
pub async fn test_external(http: &reqwest::Client, ext: &FilterExternal) -> Result<TestResult, String> {
    let release = sample_release();
    let mut res = TestResult { kind: ext.kind.clone(), ..TestResult::default() };
    let start = Instant::now();
    match ext.kind.as_str() {
        "EXEC" => {
            match exec_cmd(ext, &release).await {
                Ok((code, out)) => {
                    res.status = code;
                    res.output = out;
                }
                Err((code, out, e)) => {
                    res.status = code;
                    res.output = out;
                    res.error = e;
                }
            }
            res.expect_status = ext.exec_expect_status;
        }
        "WEBHOOK" => {
            match webhook(http, ext, &release).await {
                Ok((code, body)) => {
                    res.status = code;
                    res.output = body;
                }
                Err(e) => res.error = e,
            }
            res.expect_status = ext.webhook_expect_status;
        }
        k => return Err(format!("external filter type '{k}' is not supported: {ERR_TYPE_UNSUPPORTED}")),
    }
    res.duration_ms = start.elapsed().as_millis() as i64;
    res.success = res.error.is_empty() && res.status == res.expect_status;
    Ok(res)
}

#[cfg(test)]
mod tests {
    //! autobrr's internal/domain/release_torrent_test.go `TestFilterExternal_NeedTorrent`, and
    //! internal/filter/service.go's `RunExternalFilters`, `TestExternal` and output cap.
    use super::*;
    use crate::releases::run::BoxFut;
    use axum::routing::any;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn need_torrent() {
        let e = |args: &str, data: &str| FilterExternal { exec_args: args.into(), webhook_data: data.into(), ..FilterExternal::default() };
        let cases = [
            ("raw bytes in exec args", e("--data {{.TorrentDataRawBytes}}", ""), true, false),
            ("raw bytes in webhook data", e("", r#"{"data":"{{.TorrentDataRawBytes}}"}"#), true, false),
            ("path macro needs the file on disk", e("--file {{.TorrentPathName}}", ""), true, true),
            ("tmp file macro needs the file on disk", e("", r#"{"path":"{{.TorrentTmpFile}}"}"#), true, true),
            ("hash needs the contents only", e("--hash {{.TorrentHash}}", ""), true, false),
            ("unrelated macros need nothing", e("--name {{.TorrentName}}", r#"{"indexer":"{{.Indexer}}"}"#), false, false),
            ("empty", e("", ""), false, false),
        ];
        for (name, ext, bytes, file) in cases {
            assert_eq!(ext.need_torrent_downloaded(), bytes, "{name}");
            assert_eq!(ext.need_torrent_tmp_file(), file, "{name}");
        }
    }

    /// Never fetches anything.
    struct NoDownloads;
    impl ReleaseDownloader for NoDownloads {
        fn download_release<'a>(&'a self, _: &'a mut Release) -> BoxFut<'a, Result<(), String>> {
            Box::pin(async { Err("no".to_string()) })
        }
        fn resolve_magnet_uri<'a>(&'a self, _: &'a mut Release) -> BoxFut<'a, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
    }

    fn exec(cmd: &str, args: &str, expect: i64) -> FilterExternal {
        FilterExternal {
            kind: "EXEC".into(),
            enabled: true,
            exec_cmd: cmd.into(),
            exec_args: args.into(),
            exec_expect_status: expect,
            ..FilterExternal::default()
        }
    }

    async fn run(list: &[FilterExternal]) -> (Result<bool, String>, Vec<String>) {
        let mut r = sample_release();
        let mut rr = RejectionReasons::new();
        let tmp = std::env::temp_dir();
        let ok = run_external_filters(list, &mut r, &NoDownloads, &reqwest::Client::new(), &tmp, &mut rr).await;
        (ok, rr.lines())
    }

    #[tokio::test]
    async fn exec_exit_code_decides() {
        assert_eq!(run(&[exec("true", "", 0)]).await.0, Ok(true));
        let (ok, why) = run(&[exec("false", "", 0)]).await;
        assert_eq!(ok, Ok(false));
        assert_eq!(why.len(), 1);
        assert!(why[0].starts_with("[external script exit code] not matching: got 1 want: 0"), "{why:?}");
        let mut off = exec("false", "", 0);
        off.enabled = false;
        assert_eq!(run(&[off]).await.0, Ok(true), "a disabled one is skipped");
        let missing = exec("no-such-program-here", "", 0);
        assert!(run(std::slice::from_ref(&missing))
            .await
            .0
            .unwrap_err()
            .starts_with("error executing external command: exec failed, could not find program"));
        let cont = FilterExternal { on_error: "CONTINUE".into(), ..missing };
        assert_eq!(run(&[cont]).await.0, Ok(true), "CONTINUE lets the release through on an error");
    }

    /// A web address answering `status` and counting calls.
    async fn hook(status: u16) -> (String, Arc<AtomicUsize>) {
        let n = Arc::new(AtomicUsize::new(0));
        let c = n.clone();
        let app = axum::Router::new().route(
            "/hook",
            any(move |body: String| {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    (axum::http::StatusCode::from_u16(status).unwrap(), "x".repeat(5000) + &body)
                }
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (format!("http://{addr}/hook"), n)
    }

    #[tokio::test]
    async fn webhook_status_and_retries() {
        let (url, calls) = hook(200).await;
        let w = FilterExternal {
            kind: "WEBHOOK".into(),
            enabled: true,
            webhook_host: url.clone(),
            webhook_expect_status: 200,
            ..FilterExternal::default()
        };
        assert_eq!(run(std::slice::from_ref(&w)).await.0, Ok(true));
        let (code, body) = webhook(&reqwest::Client::new(), &w, &sample_release()).await.unwrap();
        assert_eq!((code, body.len()), (200, EXTERNAL_OUTPUT_MAX_BYTES), "the body is capped at 4096 bytes");
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        let (url, calls) = hook(503).await;
        let retry = FilterExternal { webhook_host: url, webhook_retry_status: "503, 502".into(), webhook_retry_attempts: 3, ..w.clone() };
        let e = run(&[retry]).await.0.unwrap_err();
        assert_eq!(e, "error executing external webhook: webhook got unwanted status code: 503");
        assert_eq!(calls.load(Ordering::SeqCst), 3);

        let (url, _) = hook(404).await;
        let (ok, why) = run(&[FilterExternal { webhook_host: url, ..w }]).await;
        assert_eq!(ok, Ok(false));
        assert!(why[0].starts_with("[external webhook status code] not matching: got 404 want: 200"), "{why:?}");
    }

    #[tokio::test]
    async fn test_external_reports() {
        let http = reqwest::Client::new();
        let r = test_external(&http, &exec("echo", "{{ .TorrentName }}", 0)).await.unwrap();
        assert!(r.success);
        assert_eq!(r.output, "Best.Show.Ever.S18E21.1080p.AMZN.WEB-DL.DDP2.0.H.264-GROUP\n");
        let r = test_external(&http, &exec("false", "", 0)).await.unwrap();
        assert!(!r.success);
        assert_eq!((r.status, r.expect_status), (1, 0));
        let e = test_external(&http, &FilterExternal { kind: "NOPE".into(), ..FilterExternal::default() }).await.unwrap_err();
        assert!(e.ends_with(ERR_TYPE_UNSUPPORTED));
        let v = serde_json::to_value(&r).unwrap();
        for k in ["type", "success", "status", "expect_status", "output", "duration_ms"] {
            assert!(v.get(k).is_some(), "{k}");
        }
        assert!(v.get("error").is_none(), "error is left out when empty");
    }
}
