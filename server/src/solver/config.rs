// Ported from FlareSolverr v3.5.2 (src/utils.py get_config_*, src/flaresolverr.py, src/bottle_plugins/prometheus_plugin.py), MIT.
//! The solver's settings, read from the environment under FlareSolverr's own names, so a FlareSolverr
//! `docker-compose.yml` moves over unchanged.
//!
//! | Variable | Default | What |
//! | --- | --- | --- |
//! | `LOG_HTML` | `false` | log every page's HTML (at debug level) |
//! | `HEADLESS` | `true` | `false` shows the browser window (needs a screen) |
//! | `BROWSER_WAIT_TIMEOUT` | `1` | seconds each wait for a check to go away lasts before the box is ticked |
//! | `DISABLE_MEDIA` | `false` | don't load images, styles and fonts (a request's `disableMedia` wins) |
//! | `LANG` | unset | the language the browser asks pages for (`--accept-lang`) |
//! | `TZ` | unset | the browser's time zone |
//! | `PROXY_URL`, `PROXY_USERNAME`, `PROXY_PASSWORD` | unset | the proxy for requests that don't name one |
//! | `PROMETHEUS_ENABLED`, `PROMETHEUS_PORT` | `false`, `8192` | metrics on their own port |
//!
//! `HOST` and `PORT` are Lantern's `APP_SOLVER_PORT` (all addresses, 8191), and `LOG_LEVEL` and `LOG_FILE` are
//! Lantern's own logging (see docs/DECISIONS.md).

/// The solver's settings (see the module's words).
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// `LOG_HTML`: log each page's HTML at debug level.
    pub log_html: bool,
    /// `HEADLESS`: run the browser without a window.
    pub headless: bool,
    /// `BROWSER_WAIT_TIMEOUT`: how long, in seconds, each wait for a check to go away lasts.
    pub browser_wait_timeout: u64,
    /// `DISABLE_MEDIA`: block images, styles and fonts unless a request says otherwise.
    pub disable_media: bool,
    /// `LANG`: the language the browser asks pages for, when set.
    pub lang: Option<String>,
    /// `TZ`: the browser's time zone, when set.
    pub tz: Option<String>,
    /// `PROXY_URL`: the proxy for requests that don't name one.
    pub proxy_url: Option<String>,
    /// `PROXY_USERNAME`: the user name for that proxy.
    pub proxy_username: Option<String>,
    /// `PROXY_PASSWORD`: the password for that proxy.
    pub proxy_password: Option<String>,
    /// `PROMETHEUS_ENABLED`: serve metrics on [`Config::prometheus_port`].
    pub prometheus_enabled: bool,
    /// `PROMETHEUS_PORT`: the metrics port (8192).
    pub prometheus_port: u16,
}

impl Default for Config {
    fn default() -> Config {
        Config::from_vars(|_| None)
    }
}

impl Config {
    /// The settings from the process environment.
    pub fn from_env() -> Config {
        Config::from_vars(|k| std::env::var(k).ok())
    }

    /// The settings from `var` (a variable's value by name), with FlareSolverr's defaults and its rule that only
    /// `true` (any case) turns a switch on.
    pub fn from_vars(var: impl Fn(&str) -> Option<String>) -> Config {
        let switch = |k: &str, default: &str| var(k).unwrap_or_else(|| default.to_string()).to_lowercase() == "true";
        let number = |k: &str, default: u64| -> u64 {
            match var(k) {
                None => default,
                Some(v) => v.trim().parse().unwrap_or_else(|_| {
                    tracing::warn!("{k} must be a whole number, not \"{v}\"; using {default}");
                    default
                }),
            }
        };
        Config {
            log_html: switch("LOG_HTML", "false"),
            headless: switch("HEADLESS", "true"),
            browser_wait_timeout: number("BROWSER_WAIT_TIMEOUT", 1),
            disable_media: switch("DISABLE_MEDIA", "false"),
            lang: var("LANG"),
            tz: var("TZ"),
            proxy_url: var("PROXY_URL"),
            proxy_username: var("PROXY_USERNAME"),
            proxy_password: var("PROXY_PASSWORD"),
            prometheus_enabled: switch("PROMETHEUS_ENABLED", "false"),
            prometheus_port: number("PROMETHEUS_PORT", 8192).min(u16::MAX as u64) as u16,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn with(pairs: &[(&str, &str)]) -> Config {
        let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Config::from_vars(|k| m.get(k).cloned())
    }

    #[test]
    fn defaults_are_flaresolverrs() {
        let c = Config::default();
        assert!(!c.log_html && c.headless && !c.disable_media && !c.prometheus_enabled);
        assert_eq!((c.browser_wait_timeout, c.prometheus_port), (1, 8192));
        assert_eq!((c.lang, c.tz, c.proxy_url), (None, None, None));
    }

    #[test]
    fn only_true_turns_a_switch_on() {
        let c = with(&[("LOG_HTML", "TRUE"), ("HEADLESS", "no"), ("DISABLE_MEDIA", "1"), ("PROMETHEUS_ENABLED", "True")]);
        assert!(c.log_html && !c.headless && !c.disable_media && c.prometheus_enabled);
    }

    #[test]
    fn numbers_and_texts() {
        let c = with(&[
            ("BROWSER_WAIT_TIMEOUT", " 5 "),
            ("PROMETHEUS_PORT", "9000"),
            ("LANG", "fr_FR"),
            ("TZ", "Europe/Paris"),
            ("PROXY_URL", "http://p:8080"),
            ("PROXY_USERNAME", "u"),
            ("PROXY_PASSWORD", "pw"),
        ]);
        assert_eq!((c.browser_wait_timeout, c.prometheus_port), (5, 9000));
        assert_eq!((c.lang.as_deref(), c.tz.as_deref()), (Some("fr_FR"), Some("Europe/Paris")));
        assert_eq!(
            (c.proxy_url.as_deref(), c.proxy_username.as_deref(), c.proxy_password.as_deref()),
            (Some("http://p:8080"), Some("u"), Some("pw"))
        );
        assert_eq!(with(&[("BROWSER_WAIT_TIMEOUT", "1.5")]).browser_wait_timeout, 1);
    }
}
