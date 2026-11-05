//! The test tracker on its own (`cargo run --example mock_tracker -- 0.0.0.0:8999`), for the contract run with
//! real Sonarr, Radarr, Lidarr and Readarr (scripts/contract-arrs.sh). Sign in as `me` / `pw`.

#[path = "../tests/support/tracker.rs"]
mod tracker;

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "0.0.0.0:8999".into());
    let url = tracker::start_tracker_on(&addr).await;
    println!("test tracker on {url}");
    std::future::pending::<()>().await;
}
