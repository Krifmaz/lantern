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
    server::spawn_definition_updates(st.clone());
    server::spawn_hub(st.clone());
    st.releases.start();
    server::crossseed::spawn(st.cross_seed.clone());
    let addr = SocketAddr::from(([0, 0, 0, 0], server::port()));
    tracing::info!("{name} {} listening on {addr}", server::VERSION);
    let solver_port = server::solver_port();
    if solver_port != 0 {
        // the FlareSolverr API on its own port, as apps expect it
        let solver_addr = SocketAddr::from(([0, 0, 0, 0], solver_port));
        let solver_app = server::solver::api::flaresolverr(st.solver.clone());
        match tokio::net::TcpListener::bind(solver_addr).await {
            Ok(l) => {
                tracing::info!("FlareSolverr API listening on {solver_addr}");
                tokio::spawn(async move {
                    let _ = axum::serve(l, solver_app).with_graceful_shutdown(server::shutdown_signal()).await;
                });
            }
            Err(e) => tracing::warn!(
                "The FlareSolverr API can't use port {solver_port}: {e}. Set APP_SOLVER_PORT to a free port, or 0 to turn it off."
            ),
        }
    }
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let solver = st.solver.clone();
    axum::serve(listener, server::app(st)).with_graceful_shutdown(server::shutdown_signal()).await?;
    solver.shutdown().await;
    Ok(())
}
