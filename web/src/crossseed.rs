//! The Cross-seed page: how many finished torrents now seed on more sites, what happened to each copy found, and
//! Search now (admins).

use kit_leptos::api;
use kit_leptos::icons::icon;
use kit_leptos::toasts::toast;
use leptos::prelude::*;
use serde_json::Value;

/// A string field of a JSON object, "" when missing.
fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

/// The dot for an outcome: green when it seeds, amber when its files differ or it gave a magnet, red when it failed.
pub fn dot_of(decision: &str) -> &'static str {
    match decision {
        "injected" | "already_there" => "ok",
        "files_differ" | "magnet_only" => "warn",
        _ => "err",
    }
}

/// The line under the title: the numbers in words.
pub fn summary(stats: &Value) -> String {
    let n = |k: &str| stats.get(k).and_then(Value::as_i64).unwrap_or(0);
    let injected = match n("injected") {
        0 => "No torrent is cross-seeded yet".to_string(),
        1 => "1 copy seeding from files already downloaded".to_string(),
        i => format!("{i} copies seeding from files already downloaded"),
    };
    format!("{injected} · {} torrents searched · {} waiting their turn", n("searched"), n("waiting"))
}

/// The Cross-seed page.
#[component]
pub fn CrossSeed(admin: bool) -> impl IntoView {
    let (tick, set_tick) = signal(0u32);
    let data = LocalResource::new(move || {
        tick.track();
        async { api::get("/cross-seed?limit=100").await.map_err(|e| e.message) }
    });
    let run = move |_| {
        kit_leptos::rt::spawn(async move {
            match api::post_empty("/cross-seed/run").await {
                Ok(v) => toast(&s(&v, "message"), "info"),
                Err(e) => toast(&e.message, "error"),
            }
            set_tick.update(|t| *t += 1);
        })
    };
    let body = move || match data.get() {
        None => view! { <div class="empty"><span class="spinner"></span></div> }.into_any(),
        Some(Err(e)) => view! { <div class="error-box">{e}</div> }.into_any(),
        Some(Ok(v)) => {
            let off = v.get("enabled").and_then(Value::as_bool) == Some(false);
            let recent = v.get("recent").and_then(Value::as_array).cloned().unwrap_or_default();
            let list = if recent.is_empty() {
                view! { <div class="empty" inner_html=format!(r#"{}<p class="muted">Nothing found yet. Finished torrents are searched on your other torrent sites a few at a time.</p>"#, icon("search"))></div> }.into_any()
            } else {
                view! {
                    <div class="table-wrap"><table class="table" id="cross-seed-table">
                        <thead><tr><th>""</th><th>"Release"</th><th>"Site"</th><th>"What happened"</th></tr></thead>
                        <tbody>{recent.into_iter().map(|r| {
                            let detail = s(&r, "detail");
                            view! {
                                <tr>
                                    <td style="width:24px"><span class=format!("dot {}", dot_of(&s(&r, "decision")))></span></td>
                                    <td><div style="font-weight:650">{s(&r, "title")}</div></td>
                                    <td>{s(&r, "indexer")}</td>
                                    <td class="small">{s(&r, "words")}{(!detail.is_empty()).then(|| view! { <div class="faint">{detail}</div> })}</td>
                                </tr>
                            }
                        }).collect_view()}</tbody>
                    </table></div>
                }
                .into_any()
            };
            view! {
                <p class="muted" style="margin-top:0">{summary(v.get("stats").unwrap_or(&Value::Null))}</p>
                {off.then(|| view! { <div class="notice-banner" style="margin-bottom:12px"><span>"Cross-seed is off. Turn it on under Settings → Downloads → Cross-seed."</span></div> })}
                <div class="card flat">{list}</div>
            }
            .into_any()
        }
    };
    view! {
        <div class="row wrap between" style="margin-bottom:18px;gap:10px">
            <h1 style="margin:0">"Cross-seed"</h1>
            {admin.then(|| view! { <button class="btn" id="cross-seed-run" on:click=run inner_html=format!("{}Search now", icon("refresh"))></button> })}
        </div>
        {body}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn words_and_dots() {
        assert_eq!(
            summary(&json!({ "injected": 1, "searched": 3, "waiting": 0 })),
            "1 copy seeding from files already downloaded · 3 torrents searched · 0 waiting their turn"
        );
        assert!(summary(&json!({})).starts_with("No torrent"));
        assert_eq!((dot_of("injected"), dot_of("files_differ"), dot_of("failed")), ("ok", "warn", "err"));
    }
}
