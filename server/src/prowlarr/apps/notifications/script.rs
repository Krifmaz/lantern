// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/Notifications/CustomScript/CustomScript.cs), GPL-3.0.

//! `CustomScript`: runs the script in the settings for each event, with what happened in `Prowlarr_*`
//! environment variables (the names scripts written for Prowlarr read). The script runs by itself, with no
//! arguments and no shell: nothing a person typed is ever read as a command.

use super::senders::SendError;
use super::Event;
use crate::prowlarr::apps::provider::Definition;
use crate::AppState;
use std::time::Duration;

/// The environment variables of an event.
fn environment(st: &AppState, ev: &Event) -> Vec<(String, String)> {
    let mut env: Vec<(&str, String)> = Vec::new();
    let instance = crate::prowlarr::config::instance_name(st);
    let app_url = st.settings.str("public_url");
    let event = match ev {
        Event::Grab(_) => "Grab",
        Event::HealthIssue(_) => "HealthIssue",
        Event::HealthRestored(_) => "HealthRestored",
        Event::ApplicationUpdate(_) => "ApplicationUpdate",
        Event::Test => "Test",
    };
    env.push(("Prowlarr_EventType", event.into()));
    env.push(("Prowlarr_InstanceName", instance));
    env.push(("Prowlarr_ApplicationUrl", app_url));
    match ev {
        Event::Grab(g) => {
            let r = &g.release;
            env.push(("Prowlarr_Release_Title", r.title.clone()));
            env.push(("Prowlarr_Release_Indexer", r.indexer.clone()));
            env.push(("Prowlarr_Release_Size", r.size.map(|s| s.to_string()).unwrap_or_default()));
            env.push((
                "Prowlarr_Release_Genres",
                r.genre.split(',').map(str::trim).filter(|g| !g.is_empty()).collect::<Vec<_>>().join("|"),
            ));
            let cats: Vec<&str> = r.categories.iter().filter_map(|c| crate::indexers::categories::name_of(*c)).collect();
            env.push(("Prowlarr_Release_Categories", cats.join("|")));
            env.push(("Prowlarr_Release_IndexerFlags", r.flags.join("|")));
            env.push(("Prowlarr_Release_PublishDate", r.date.map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string()).unwrap_or_default()));
            env.push(("Prowlarr_Download_Client", g.download_client_name.clone()));
            env.push(("Prowlarr_Download_Client_Type", g.download_client_type.clone()));
            env.push(("Prowlarr_Download_Id", g.download_id.clone()));
            env.push(("Prowlarr_Source", g.source.clone()));
            env.push(("Prowlarr_Host", g.host.clone()));
            env.push(("Prowlarr_Redirect", if g.redirect { "True".into() } else { "False".into() }));
        }
        Event::HealthIssue(f) | Event::HealthRestored(f) => {
            let part = if matches!(ev, Event::HealthIssue(_)) { "Issue" } else { "Restored" };
            let level = if f.kind == "error" { "Error" } else { "Warning" };
            let out: Vec<(String, String)> = vec![
                (format!("Prowlarr_Health_{part}_Level"), level.into()),
                (format!("Prowlarr_Health_{part}_Message"), f.message.clone()),
                (format!("Prowlarr_Health_{part}_Type"), f.source.into()),
                (format!("Prowlarr_Health_{part}_Wiki"), f.wiki_url.clone()),
            ];
            return env.into_iter().map(|(k, v)| (k.to_string(), v)).chain(out).collect();
        }
        Event::ApplicationUpdate(u) => {
            env.push(("Prowlarr_Update_Message", u.message.clone()));
            env.push(("Prowlarr_Update_NewVersion", u.new_version.clone()));
            env.push(("Prowlarr_Update_PreviousVersion", u.previous_version.clone()));
        }
        Event::Test => {}
    }
    env.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

/// Runs the script for `ev` (at most five minutes). For the test, a missing script or one that ends with an
/// error code fails.
pub async fn run(st: &AppState, def: &Definition, ev: &Event<'_>) -> Result<(), SendError> {
    let path = def.text("path");
    if !std::path::Path::new(&path).is_file() {
        return Err(SendError::new_pub("Path", "File does not exist"));
    }
    let mut cmd = tokio::process::Command::new(&path);
    cmd.envs(environment(st, ev))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd.kill_on_drop(true);
    let status = tokio::time::timeout(Duration::from_secs(300), cmd.status())
        .await
        .map_err(|_| SendError::new_pub("", "The script took longer than five minutes, so Lantern stopped it."))?
        .map_err(|e| SendError::new_pub("Path", format!("Lantern can't run the script: {e}")))?;
    match status.code() {
        Some(0) => Ok(()),
        Some(code) => Err(SendError::new_pub("", format!("Script exited with code: {code}"))),
        None => Err(SendError::new_pub("", "The script was stopped before it finished.")),
    }
}
