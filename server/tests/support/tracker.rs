//! A small private tracker for tests and the contract run with real Sonarr/Radarr/Lidarr/Readarr: a sign-in form,
//! a results table (TV, a movie, an album and a book), a details page and the file. It matches
//! tests/fixtures/definitions/lantern-test.yml.
#![allow(dead_code)]

use axum::extract::Query;
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Redirect};
use axum::routing::{get, post};
use axum::{Form, Router};
use axum_extra::extract::cookie::{Cookie, CookieJar};
use std::collections::HashMap;

/// The tracker's results page: TV (freeleech), a movie with an IMDb link, an album, a book and an ad row.
pub const BROWSE: &str = r#"<html><body><a href="logout.php">Log out</a>
<table id="torrents"><tbody>
<tr><td class="cat"><a href="browse.php?cat=2">TV</a></td><td><a href="details.php?id=11">Show.Name.S01E02.1080p.WEB-DL-GRP</a>
  <img alt="Free"></td><td class="size">1.5 GB</td><td class="seeders">12</td><td class="leechers">3</td><td class="grabs">40</td>
  <td class="added">2026-11-04 10:30</td></tr>
<tr><td class="cat"><a href="browse.php?cat=1">Movies</a></td><td><a href="details.php?id=12">Movie.2025.2160p.UHD-GRP</a>
  <a href="https://www.imdb.com/title/tt0111161/">IMDb</a></td><td class="size">20,5 GB</td><td class="seeders">5</td><td class="leechers">0</td>
  <td class="grabs">7</td><td class="added">2026-11-03 08:00</td></tr>
<tr><td class="cat"><a href="browse.php?cat=3">Music</a></td><td><a href="details.php?id=13">Some Artist - Some Album (2025) [FLAC]</a></td>
  <td class="size">400 MB</td><td class="seeders">9</td><td class="leechers">1</td><td class="grabs">3</td><td class="added">2026-11-02 12:00</td></tr>
<tr><td class="cat"><a href="browse.php?cat=4">Books</a></td><td><a href="details.php?id=14">Some Author - Some Book (2024) [EPUB]</a></td>
  <td class="size">2 MB</td><td class="seeders">4</td><td class="leechers">0</td><td class="grabs">1</td><td class="added">2026-11-01 09:00</td></tr>
<tr><td colspan="7">an advert, not a release</td></tr>
</tbody></table></body></html>"#;

/// A tiny bencoded torrent.
pub const TORRENT: &[u8] =
    b"d8:announce14:http://t.test/4:infod6:lengthi1e4:name1:a12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaaee";

fn signed_in(jar: &CookieJar) -> bool {
    jar.get("sess").is_some_and(|c| c.value() == "ok")
}

/// The tracker: sign-in form, results that need the session, a details page and the file.
pub fn tracker() -> Router {
    Router::new()
        .route("/login.php", get(|| async { Html(r#"<form id="login" action="takelogin.php" method="post"><input type="hidden" name="token" value="t1"><input name="username"><input type="password" name="password"></form>"#) }))
        .route(
            "/takelogin.php",
            post(|jar: CookieJar, Form(f): Form<HashMap<String, String>>| async move {
                if f.get("username").map(String::as_str) == Some("me") && f.get("password").map(String::as_str) == Some("pw") && f.get("token").map(String::as_str) == Some("t1") {
                    (jar.add(Cookie::build(("sess", "ok")).path("/")), Redirect::to("/index.php")).into_response()
                } else {
                    Html(r#"<div class="error">Wrong username or password</div>"#).into_response()
                }
            }),
        )
        .route("/index.php", get(|jar: CookieJar| async move { if signed_in(&jar) { Html(r#"<a href="logout.php">Log out</a>"#).into_response() } else { Redirect::to("/login.php").into_response() } }))
        .route(
            "/browse.php",
            get(|jar: CookieJar, Query(q): Query<HashMap<String, String>>| async move {
                if !signed_in(&jar) {
                    return Redirect::to("/login.php").into_response();
                }
                // the definition sends the words with dots, the categories as c<id>=1 and the sort
                if q.get("sort").map(String::as_str) != Some("added") {
                    return (StatusCode::BAD_REQUEST, "no sort").into_response();
                }
                let words = q.get("search").cloned().unwrap_or_default();
                if words == "nothing.here" {
                    return Html(r#"<a href="logout.php">x</a><table id="torrents"><tbody></tbody></table>"#).into_response();
                }
                // only the rows of the categories asked for (all without any)
                let cats: Vec<&str> = ["1", "2", "3", "4"].into_iter().filter(|c| q.contains_key(&format!("c{c}"))).collect();
                let page = if cats.is_empty() {
                    BROWSE.to_string()
                } else {
                    let (head, rest) = BROWSE.split_once("<tbody>").unwrap();
                    let (rows, tail) = rest.split_once("</tbody>").unwrap();
                    let kept: Vec<String> = rows.split("<tr>").filter(|r| !r.trim().is_empty()).filter(|r| cats.iter().any(|c| r.contains(&format!("cat={c}\""))) || !r.contains("cat=")).map(|r| format!("<tr>{r}")).collect();
                    format!("{head}<tbody>{}</tbody>{tail}", kept.join(""))
                };
                Html(page).into_response()
            }),
        )
        .route(
            "/details.php",
            get(|jar: CookieJar, Query(q): Query<HashMap<String, String>>| async move {
                if !signed_in(&jar) {
                    return Redirect::to("/login.php").into_response();
                }
                Html(format!(r#"<a class="dl" href="download.php?id={}">Download</a>"#, q.get("id").cloned().unwrap_or_default())).into_response()
            }),
        )
        .route("/download.php", get(|jar: CookieJar| async move { if signed_in(&jar) { ([(header::CONTENT_TYPE, "application/x-bittorrent")], TORRENT).into_response() } else { Redirect::to("/login.php").into_response() } }))
}

pub async fn start_tracker() -> String {
    start_tracker_on("127.0.0.1:0").await
}

/// The tracker on `addr` (`0.0.0.0:8999`); its address.
pub async fn start_tracker_on(addr: &str) -> String {
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, tracker()).await.unwrap() });
    format!("http://{addr}/")
}
