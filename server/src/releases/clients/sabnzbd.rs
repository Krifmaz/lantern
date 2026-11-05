// Ported from autobrr v1.90.0 (pkg/sabnzbd), GPL-2.0-or-later.
//! SABnzbd over its HTTP API at `/api` (`mode=addurl`, `mode=version`), with the API key in the query.

use serde_json::Value;
use std::time::Duration;

/// A SABnzbd client.
pub struct Client {
    addr: String,
    api_key: String,
    basic_user: String,
    basic_pass: String,
    http: reqwest::Client,
}

impl Client {
    /// A client for SABnzbd at `addr`; basic auth when both are set.
    pub fn new(addr: &str, api_key: &str, tls_skip_verify: bool, basic_user: &str, basic_pass: &str) -> Client {
        Client {
            addr: addr.into(),
            api_key: api_key.into(),
            basic_user: basic_user.into(),
            basic_pass: basic_pass.into(),
            http: super::http(Duration::from_secs(60), tls_skip_verify, false),
        }
    }

    /// GETs `/api` with `query`; the answer as JSON.
    async fn get(&self, query: &[(&str, &str)]) -> Result<Value, String> {
        let addr = super::super::downloader::join_path_url(&self.addr, "/api");
        let mut u = url::Url::parse(&addr).map_err(|e| e.to_string())?;
        {
            let mut q = u.query_pairs_mut();
            q.clear();
            let mut sorted = query.to_vec();
            sorted.sort_by(|a, b| a.0.cmp(b.0));
            for (k, v) in sorted {
                q.append_pair(k, v);
            }
        }
        let mut req = self.http.get(u.as_str()).header("User-Agent", super::user_agent());
        if !self.basic_user.is_empty() && !self.basic_pass.is_empty() {
            req = req.basic_auth(&self.basic_user, Some(&self.basic_pass));
        }
        let res = req.send().await.map_err(|e| super::req_err("Get", u.as_str(), &e))?;
        let text = res.text().await.map_err(|e| format!("could not read body: {e}"))?;
        if text.is_empty() {
            return Err("could not read body: EOF".into());
        }
        serde_json::from_str(&text).map_err(|e| format!("could not unmarshal body: {e}"))
    }

    /// Adds the NZB at `url` in `category` (`*` when empty); the queue ids, or SABnzbd's error.
    pub async fn add_from_url(&self, url: &str, category: &str) -> Result<Vec<String>, String> {
        let cat = if category.is_empty() { "*" } else { category };
        let v = self.get(&[("mode", "addurl"), ("name", url), ("output", "json"), ("apikey", &self.api_key), ("cat", cat)]).await?;
        Ok(v.get("nzo_ids")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default())
    }

    /// SABnzbd's version.
    pub async fn version(&self) -> Result<String, String> {
        let v = self.get(&[("mode", "version"), ("output", "json"), ("apikey", &self.api_key)]).await?;
        Ok(v.get("version").and_then(Value::as_str).unwrap_or("").to_string())
    }
}
