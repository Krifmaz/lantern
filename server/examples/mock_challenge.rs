//! The Cloudflare-style test site on its own (`cargo run --example mock_challenge -- 0.0.0.0:8998`), for the
//! check of the Docker image's browser (scripts/image-solver.sh).

#[path = "../tests/support/challenge.rs"]
mod challenge;

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "0.0.0.0:8998".into());
    let (url, _) = challenge::start_site_on(&addr).await;
    println!("test site on {url}");
    std::future::pending::<()>().await;
}
