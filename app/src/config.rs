use std::{env, net::SocketAddr, time::Duration};

use thiserror::Error;

const DEFAULT_BIND_ADDR: &str = "127.0.0.1:8080";
const DEFAULT_DBPEDIA_SPARQL_URL: &str = "https://dbpedia.org/sparql";
const DEFAULT_REDIS_URL: &str = "redis://127.0.0.1:6379/0";
const DEFAULT_DATABASE_URL: &str = "postgres://kgproxy:kgproxy-dev-password@127.0.0.1:5432/kgproxy";
const DEFAULT_CACHE_TTL_SECONDS: u64 = 604_800;
const DEFAULT_MAX_OUTBOUND_CONCURRENCY: usize = 2;
const DEFAULT_ORIGIN_TIMEOUT_MS: u64 = 2_000;
const DEFAULT_MAX_ORIGIN_RESPONSE_BYTES: usize = 100 * 1024;
const DEFAULT_CACHE_WARMER_ENABLED: bool = false;
const DEFAULT_CACHE_WARMER_INTERVAL_SECONDS: u64 = 3_600;
const DEFAULT_CACHE_WARMER_TOP_K: i64 = 25;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub bind_addr: SocketAddr,
    pub dbpedia_sparql_url: String,
    pub redis_url: String,
    pub database_url: String,
    pub cache_ttl: Duration,
    pub max_outbound_concurrency: usize,
    pub origin_timeout: Duration,
    pub max_origin_response_bytes: usize,
    pub cache_warmer_enabled: bool,
    pub cache_warmer_interval: Duration,
    pub cache_warmer_top_k: i64,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("invalid {name}: {value}")]
    InvalidValue { name: &'static str, value: String },
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let bind_addr = parse_env(
            "BIND_ADDR",
            DEFAULT_BIND_ADDR
                .parse()
                .expect("default bind address must be valid"),
        )?;
        let dbpedia_sparql_url = env::var("DBPEDIA_SPARQL_URL")
            .unwrap_or_else(|_| DEFAULT_DBPEDIA_SPARQL_URL.to_owned());
        let redis_url = env::var("REDIS_URL").unwrap_or_else(|_| DEFAULT_REDIS_URL.to_owned());
        let database_url =
            env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_owned());
        let cache_ttl =
            Duration::from_secs(parse_env("CACHE_TTL_SECONDS", DEFAULT_CACHE_TTL_SECONDS)?);
        let max_outbound_concurrency =
            parse_env("MAX_OUTBOUND_CONCURRENCY", DEFAULT_MAX_OUTBOUND_CONCURRENCY)?;
        let origin_timeout =
            Duration::from_millis(parse_env("ORIGIN_TIMEOUT_MS", DEFAULT_ORIGIN_TIMEOUT_MS)?);
        let max_origin_response_bytes = parse_env(
            "MAX_ORIGIN_RESPONSE_BYTES",
            DEFAULT_MAX_ORIGIN_RESPONSE_BYTES,
        )?;
        let cache_warmer_enabled = parse_env("CACHE_WARMER_ENABLED", DEFAULT_CACHE_WARMER_ENABLED)?;
        let cache_warmer_interval = Duration::from_secs(parse_env(
            "CACHE_WARMER_INTERVAL_SECONDS",
            DEFAULT_CACHE_WARMER_INTERVAL_SECONDS,
        )?);
        let cache_warmer_top_k = parse_env("CACHE_WARMER_TOP_K", DEFAULT_CACHE_WARMER_TOP_K)?;
        if cache_warmer_interval.is_zero() {
            return Err(ConfigError::InvalidValue {
                name: "CACHE_WARMER_INTERVAL_SECONDS",
                value: "0".to_owned(),
            });
        }
        if cache_warmer_top_k < 1 {
            return Err(ConfigError::InvalidValue {
                name: "CACHE_WARMER_TOP_K",
                value: cache_warmer_top_k.to_string(),
            });
        }

        Ok(Self {
            bind_addr,
            dbpedia_sparql_url,
            redis_url,
            database_url,
            cache_ttl,
            max_outbound_concurrency,
            origin_timeout,
            max_origin_response_bytes,
            cache_warmer_enabled,
            cache_warmer_interval,
            cache_warmer_top_k,
        })
    }
}

fn parse_env<T>(name: &'static str, default: T) -> Result<T, ConfigError>
where
    T: std::str::FromStr,
{
    match env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|_| ConfigError::InvalidValue { name, value }),
        Err(_) => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_match_mvp_capacity_plan() {
        let config = Config::from_env().expect("default config should load");

        assert_eq!(config.bind_addr, "127.0.0.1:8080".parse().unwrap());
        assert_eq!(config.dbpedia_sparql_url, "https://dbpedia.org/sparql");
        assert_eq!(config.redis_url, "redis://127.0.0.1:6379/0");
        assert_eq!(
            config.database_url,
            "postgres://kgproxy:kgproxy-dev-password@127.0.0.1:5432/kgproxy"
        );
        assert_eq!(config.cache_ttl, Duration::from_secs(604_800));
        assert_eq!(config.max_outbound_concurrency, 2);
        assert_eq!(config.origin_timeout, Duration::from_millis(2_000));
        assert_eq!(config.max_origin_response_bytes, 100 * 1024);
        assert!(!config.cache_warmer_enabled);
        assert_eq!(config.cache_warmer_interval, Duration::from_secs(3_600));
        assert_eq!(config.cache_warmer_top_k, 25);
    }
}
