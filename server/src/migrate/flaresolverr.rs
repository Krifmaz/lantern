//! FlareSolverr → Lantern: FlareSolverr has no settings file, only environment variables and a port, so this reads
//! whatever a person pastes (a docker-compose service, an `.env` file, a `docker run` command or `docker inspect`
//! output) and says, for each setting it knows, what Lantern does instead. Lantern solves Cloudflare checks by
//! itself and answers the FlareSolverr API on its own port, so nothing is saved: the plan is the steps to take.

use super::{Action, Plan, Step};
use kit_server_core::{err, Result};
use std::collections::BTreeMap;

/// FlareSolverr's port, which Lantern's FlareSolverr API uses too.
pub const PORT: u16 = 8191;

/// The FlareSolverr settings Lantern knows, from `text`: name → value (the last one wins), and the host ports
/// mapped to the container's FlareSolverr port.
pub fn read(text: &str) -> (BTreeMap<String, String>, Vec<(u16, u16)>) {
    const KNOWN: [&str; 15] = [
        "PORT",
        "HOST",
        "LOG_LEVEL",
        "LOG_HTML",
        "LOG_FILE",
        "CAPTCHA_SOLVER",
        "TZ",
        "LANG",
        "HEADLESS",
        "BROWSER_TIMEOUT",
        "TEST_URL",
        "PROMETHEUS_ENABLED",
        "PROMETHEUS_PORT",
        "DISABLE_MEDIA",
        "PROXY_URL",
    ];
    let mut env = BTreeMap::new();
    // KEY=value, KEY: value, - KEY=value, -e KEY=value, "KEY=value"
    let pair = regex::Regex::new(r#"(?m)(?:^|[\s"'\[,-])([A-Z][A-Z0-9_]*)[ \t]*[=:][ \t]*(?:"([^"\r\n]*)"|'([^'\r\n]*)'|([^\s"',\]\\]*))"#)
        .expect("a valid pattern");
    for c in pair.captures_iter(text) {
        let k = &c[1];
        if KNOWN.contains(&k) {
            let v = c.get(2).or(c.get(3)).or(c.get(4)).map(|m| m.as_str()).unwrap_or("");
            env.insert(k.to_string(), v.trim().to_string());
        }
    }
    let port = env.get("PORT").and_then(|p| p.parse::<u16>().ok()).unwrap_or(PORT);
    let map = regex::Regex::new(r#"(?:^|[\s"'\[-])(?:[\d.]+:)?(\d{2,5}):(\d{2,5})(?:/tcp)?\b"#).expect("a valid pattern");
    let mut ports = Vec::new();
    for c in map.captures_iter(text) {
        if let (Ok(host), Ok(inside)) = (c[1].parse::<u16>(), c[2].parse::<u16>()) {
            if inside == port && !ports.contains(&(host, inside)) {
                ports.push((host, inside));
            }
        }
    }
    (env, ports)
}

/// The plan for FlareSolverr's settings in `text`; Lantern's FlareSolverr API is on `solver_port` (0: off).
pub fn plan(text: &str, solver_port: u16) -> Result<Plan> {
    let (env, ports) = read(text);
    if env.is_empty() && ports.is_empty() {
        return Err(err(
            400,
            "Nothing in that looked like FlareSolverr's settings. Paste its docker-compose service, .env file or docker run command.",
        ));
    }
    let mut plan = Plan { from: "FlareSolverr's settings".into(), dry_run: true, ..Default::default() };
    let mut step = |name: &str, action: Action, detail: String| plan.settings.push(Step::new(name, "FlareSolverr setting", action, detail));
    let off = solver_port == 0;
    let their_port = env.get("PORT").and_then(|p| p.parse::<u16>().ok()).unwrap_or(PORT);
    // the port apps reached FlareSolverr at: the host side of the mapping, else its own port
    let reached = ports.first().map(|(h, _)| *h).unwrap_or(their_port);
    if off {
        step(
            "Port",
            Action::Manual,
            format!("Lantern's FlareSolverr API is turned off (APP_SOLVER_PORT=0). Set APP_SOLVER_PORT={reached} so apps can reach it where FlareSolverr was."),
        );
    } else if reached == solver_port {
        step(
            "Port",
            Action::NothingNeeded,
            format!("Lantern answers the FlareSolverr API on port {solver_port}, where apps reached FlareSolverr."),
        );
    } else {
        step(
            "Port",
            Action::Manual,
            format!(
                "Apps reached FlareSolverr on port {reached}, and Lantern answers on {solver_port}. Map port {reached} to Lantern's {solver_port} (\"{reached}:{solver_port}\"), or set APP_SOLVER_PORT={reached}."
            ),
        );
    }
    for (k, v) in &env {
        let (action, detail) = match k.as_str() {
            "PORT" => continue,
            "HOST" => (Action::NothingNeeded, "Lantern listens on every address in its container.".to_string()),
            "LOG_LEVEL" | "LOG_HTML" | "LOG_FILE" => (Action::NothingNeeded, "Lantern keeps its own log; Diagnostics shows the solver's state.".to_string()),
            "CAPTCHA_SOLVER" => {
                if v.is_empty() || v.eq_ignore_ascii_case("none") {
                    (Action::NothingNeeded, "No captcha solver was set, and Lantern doesn't need one.".to_string())
                } else {
                    (Action::Skip, format!("Lantern doesn't send captchas to {v}. A site that shows one asks for a cookie from your browser instead."))
                }
            }
            "TZ" | "LANG" => (Action::Manual, format!("Put {k}={v} on Lantern's container too, if it isn't there, so times and words match.")),
            "HEADLESS" => (Action::NothingNeeded, "Lantern's browser always runs without a window.".to_string()),
            "BROWSER_TIMEOUT" => {
                (Action::NothingNeeded, "Each request says how long to wait (maxTimeout), as with FlareSolverr; Lantern starts its browser when needed.".to_string())
            }
            "TEST_URL" => (Action::NothingNeeded, "Lantern checks its browser when it starts it.".to_string()),
            "PROMETHEUS_ENABLED" | "PROMETHEUS_PORT" => (Action::Skip, "Lantern doesn't serve Prometheus numbers; Diagnostics shows the solver's state.".to_string()),
            "DISABLE_MEDIA" => (Action::NothingNeeded, "Lantern's browser loads only what a check needs.".to_string()),
            "PROXY_URL" => (
                Action::Manual,
                "Lantern doesn't take a proxy for the solver alone. Set it as the proxy for sites under Settings → Indexers, or send it with each request.".to_string(),
            ),
            _ => continue,
        };
        step(k, action, detail);
    }
    step(
        "Apps that used FlareSolverr",
        Action::Manual,
        format!(
            "Point them at Lantern's address on port {} (http://lantern:{}). Lantern's own indexers need nothing: Lantern solves for them by itself.",
            if off { reached } else { solver_port },
            if off { reached } else { solver_port }
        ),
    );
    plan.notes.push("Nothing is saved for FlareSolverr: Lantern solves Cloudflare checks by itself. Once the steps are done, FlareSolverr can be removed.".into());
    plan.summarize();
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPOSE: &str = r#"
services:
  flaresolverr:
    image: ghcr.io/flaresolverr/flaresolverr:latest
    environment:
      - LOG_LEVEL=${LOG_LEVEL:-info}
      - CAPTCHA_SOLVER=none
      - TZ=Europe/London
    ports:
      - "${PORT:-8191}:8191"
      - "8192:8191"
"#;

    #[test]
    fn compose_env_and_docker_run_are_read() {
        let (env, ports) = read(COMPOSE);
        assert_eq!(env.get("TZ").map(String::as_str), Some("Europe/London"));
        assert_eq!(env.get("CAPTCHA_SOLVER").map(String::as_str), Some("none"));
        assert_eq!(ports, vec![(8192, 8191)]);
        let (env, ports) = read("docker run -d -e LOG_LEVEL=debug -e \"TZ=Asia/Tokyo\" -p 9000:8191 ghcr.io/flaresolverr/flaresolverr");
        assert_eq!(env.get("LOG_LEVEL").map(String::as_str), Some("debug"));
        assert_eq!(env.get("TZ").map(String::as_str), Some("Asia/Tokyo"));
        assert_eq!(ports, vec![(9000, 8191)]);
        let (env, _) = read("PORT=8200\nHEADLESS=true\n# a comment\nUNRELATED=1\n");
        assert_eq!(env.get("PORT").map(String::as_str), Some("8200"));
        assert!(!env.contains_key("UNRELATED"));
        let (env, _) = read(r#"{"Env": ["LOG_LEVEL=info", "PATH=/usr/bin"]}"#);
        assert_eq!(env.get("LOG_LEVEL").map(String::as_str), Some("info"));
    }

    #[test]
    fn the_plan_says_what_to_do() {
        let p = plan(COMPOSE, 8191).unwrap();
        let port = p.settings.iter().find(|s| s.name == "Port").unwrap();
        assert_eq!(port.action, Action::Manual);
        assert!(port.detail.contains("\"8192:8191\""));
        assert!(p.settings.iter().any(|s| s.name == "TZ" && s.action == Action::Manual));
        let p = plan("PORT=8191", 8191).unwrap();
        assert_eq!(p.settings[0].action, Action::NothingNeeded);
        assert!(p.summary.contains("Nothing was changed"));
        assert_eq!(plan("hello there", 8191).unwrap_err().status.as_u16(), 400);
    }
}
