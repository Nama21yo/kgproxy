use kgproxy::{config::Config, http::build_router};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "kgproxy=info,tower_http=info".into()),
        )
        .init();

    let config = Config::from_env()?;
    let listener = TcpListener::bind(config.bind_addr).await?;

    tracing::info!(addr = %listener.local_addr()?, "kgproxy listening");
    axum::serve(listener, build_router()).await?;

    Ok(())
}
