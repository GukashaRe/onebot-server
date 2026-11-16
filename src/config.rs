use std::{
    env,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

use anyhow::{Context, Result};

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub listen_addr: SocketAddr,
    pub register_key: Option<String>,
    pub admin_key: Option<String>,
    pub ws_queue_capacity: usize,
    pub rate_limit_default: i32,
    pub public_base_url: Option<String>,
    pub session_cache_ttl: Duration,
    pub ws_revalidate_interval: Duration,
    pub history_default_limit: i64,
    pub history_max_limit: i64,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let database_url = env::var("DATABASE_URL")
            .context("DATABASE_URL is required (PostgreSQL connection string)")?;
        let listen_addr: SocketAddr = env::var("LISTEN_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:8080".to_owned())
            .parse()
            .context("LISTEN_ADDR must be a socket address such as 127.0.0.1:8080")?;

        let register_key = non_empty(env::var("REGISTER_KEY").ok());
        let admin_key = non_empty(env::var("ADMIN_KEY").ok());

        let ws_queue_capacity = parse_u64("WS_QUEUE_CAPACITY", 256).clamp(1, 65_536) as usize;
        let rate_limit_default = parse_i32("RATE_LIMIT_DEFAULT", 20).max(1);
        let session_cache_ttl = Duration::from_secs(parse_u64("SESSION_CACHE_TTL_SECS", 30).max(1));
        let ws_revalidate_interval =
            Duration::from_secs(parse_u64("WS_REVALIDATE_SECS", 60).max(5));
        let history_default_limit = parse_u64("HISTORY_DEFAULT_LIMIT", 20).clamp(1, 500) as i64;
        let history_max_limit = parse_u64("HISTORY_MAX_LIMIT", 100).clamp(1, 500) as i64;

        let public_base_url = non_empty(env::var("PUBLIC_BASE_URL").ok())
            .map(|url| url.trim_end_matches('/').to_owned());

        Ok(Self {
            database_url,
            listen_addr,
            register_key,
            admin_key,
            ws_queue_capacity,
            rate_limit_default,
            public_base_url,
            session_cache_ttl,
            ws_revalidate_interval,
            history_default_limit,
            history_max_limit,
        })
    }

    /// Best-effort direct URL used when a request lacks a Host header.
    pub fn fallback_base_url(&self) -> String {
        if let Some(base) = &self.public_base_url {
            return base.clone();
        }
        let host = match self.listen_addr.ip() {
            IpAddr::V4(ip) if ip == Ipv4Addr::UNSPECIFIED => "127.0.0.1".to_owned(),
            IpAddr::V4(ip) => ip.to_string(),
            IpAddr::V6(ip) if ip.is_unspecified() => "[::1]".to_owned(),
            IpAddr::V6(ip) => format!("[{ip}]"),
        };
        format!("http://{host}:{}", self.listen_addr.port())
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let v = v.trim().to_owned();
        if v.is_empty() { None } else { Some(v) }
    })
}

fn parse_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

fn parse_i32(name: &str, default: i32) -> i32 {
    env::var(name)
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(default)
}
