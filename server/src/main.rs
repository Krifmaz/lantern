//! The server binary: reads the environment, opens the data folder and serves the API and the web app.
//! `healthcheck` instead asks a running server's `/api/health` (the Docker image has no curl).

use std::net::SocketAddr;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let name = env!("CARGO_PKG_NAME");
    match std::env::args().nth(1).as_deref() {
        // `<app> --version`: the version and nothing else
        Some("--version" | "-V") => {
            println!("{name} {}", server::VERSION);
            return Ok(());
        }
        // `<app> healthcheck`: exit 0 when the server on APP_PORT answers /api/health, 1 otherwise
        Some("healthcheck") => std::process::exit(if server::healthcheck(server::port()).await { 0 } else { 1 }),
        _ => {}
    }
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let st = server::AppState::from_env().await?;
    let addr = SocketAddr::from(([0, 0, 0, 0], server::port()));
    tracing::info!("{name} {} listening on {addr}", server::VERSION);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, server::app(st)).with_graceful_shutdown(server::shutdown_signal()).await?;
    Ok(())
}
