//! Seeding rules and cleanup. A torrent has seeded enough when it reaches the `seed_ratio` or has seeded for
//! `seed_hours` (with neither set: when its client stopped it on its own limits); a Usenet download needs no
//! seeding. Once an app says it has imported a grab (`POST /api/hub/grabs/{id}/imported`) and the grab has seeded
//! enough, the `cleanup` setting decides: `off` keeps everything, `remove` takes it out of the client and keeps
//! the files, `delete` takes it out and deletes the files. A grab no app has imported is never cleaned up, and the
//! files of a grab that cross-seed copies seed from are always kept (the copies still need them).

use super::grabs::Grab;
use super::Hub;
use kit_server_core::{p, Db, Result};
use serde_json::json;

/// The `cleanup` setting's values.
pub const CLEANUP: [&str; 3] = ["off", "remove", "delete"];

impl Hub {
    /// Whether grab `g` has seeded enough under the seeding rules.
    pub fn seeded(&self, g: &Grab) -> bool {
        if g.protocol == "usenet" {
            return true;
        }
        let ratio = self.settings.float("seed_ratio");
        let hours = self.settings.float("seed_hours");
        if ratio <= 0.0 && hours <= 0.0 {
            return g.state == "completed";
        }
        (ratio > 0.0 && g.ratio >= ratio) || (hours > 0.0 && g.seed_secs as f64 >= hours * 3600.0)
    }

    /// Notes that grab `id`'s app has imported its files, so it may be cleaned up once it has seeded enough.
    pub fn imported(&self, id: i64) -> Result<Grab> {
        let g = self.get_grab(id)?;
        if !matches!(g.state.as_str(), "seeding" | "completed") {
            return Err(kit_server_core::err(409, "Only a finished grab can be marked as imported"));
        }
        self.db.execute("UPDATE hub_grabs SET imported=?, updated=? WHERE id=?", p![crate::auth::now(), crate::auth::now(), id])?;
        let g = self.get_grab(id)?;
        if let Ok(app) = self.app(g.app_id) {
            self.emit("grab.imported", &app, self.grab_json(&g, &app));
        }
        Ok(g)
    }

    /// Whether cross-seed copies added to a client seed from grab `id`'s files, so they must stay on disk.
    fn shares_files(&self, id: i64) -> bool {
        // no cross_seeds table (a hub on its own, as in some tests) means no copies
        self.db.query("SELECT 1 FROM cross_seeds WHERE grab_id=? AND decision='injected' LIMIT 1", p![id]).is_ok_and(|r| !r.is_empty())
    }

    /// Cleans up every imported grab that has seeded enough, as the `cleanup` setting says.
    pub(super) async fn clean_up(&self) -> Result<()> {
        let how = self.settings.str("cleanup");
        if how != "remove" && how != "delete" {
            return Ok(());
        }
        let rows = self.db.query(
            "SELECT * FROM hub_grabs WHERE state IN ('seeding','completed') AND imported>0 AND client_id<>0 AND client_ref<>'' ORDER BY id",
            p![],
        )?;
        for g in rows.iter().map(Grab::from_row) {
            if !self.seeded(&g) {
                continue;
            }
            let Ok(cfg) = self.client(g.client_id) else { continue };
            let backend = self.backend(&cfg)?;
            let delete = how == "delete" && !self.shares_files(g.id);
            if let Err(e) = backend.remove(&g.client_ref, delete).await {
                tracing::warn!("download hub: cleaning up {}: {e}", g.title);
                continue;
            }
            self.db.execute("UPDATE hub_grabs SET state='removed', speed=0, eta=0, updated=? WHERE id=?", p![crate::auth::now(), g.id])?;
            let g = self.get_grab(g.id)?;
            if let Ok(app) = self.app(g.app_id) {
                let mut v = self.grab_json(&g, &app);
                v["reason"] = json!(if delete { "cleaned up: removed with its files" } else { "cleaned up: removed, files kept" });
                self.emit("grab.removed", &app, v);
            }
        }
        Ok(())
    }
}
