use std::sync::Arc;

use kgproxy::{
    cache::RedisCache,
    config::Config,
    http::{AppState, build_router},
    logging::{ChannelLogger, postgres_pool},
    metrics::PostgresMetricsReader,
    origin::ReqwestDbpediaClient,
};
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
    let origin = ReqwestDbpediaClient::new(
        config.dbpedia_sparql_url.clone(),
        config.origin_timeout,
        config.max_origin_response_bytes,
    )?;
    let cache = RedisCache::new(&config.redis_url)?;
    let postgres = postgres_pool(&config.database_url)?;
    sqlx::migrate!("./migrations").run(&postgres).await?;
    let logger = ChannelLogger::spawn(postgres.clone(), 1024);
    let metrics = PostgresMetricsReader::new(postgres);
    let listener = TcpListener::bind(config.bind_addr).await?;

    tracing::info!(addr = %listener.local_addr()?, "kgproxy listening");
    axum::serve(
        listener,
        build_router(AppState::with_metrics(
            Arc::new(origin),
            Arc::new(cache),
            config.cache_ttl,
            config.max_outbound_concurrency,
            Arc::new(logger),
            Arc::new(metrics),
        )),
    )
    .await?;

    Ok(())
}
