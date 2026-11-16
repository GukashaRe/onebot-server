pub mod actions;
pub mod auth;
pub mod bootstrap;
pub mod cache;
pub mod config;
pub mod error;
pub mod fanout;
pub mod http;
pub mod model;
pub mod protocol;
pub mod registry;
pub mod relations;
pub mod state;
pub mod store;
pub mod util;
pub mod ws;

use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use crate::{config::Config, http::router, store::Store};

pub async fn run() -> Result<()> {
    dotenvy::dotenv().ok();
    init_tracing();
    let config = Config::from_env()?;

    if config.register_key.is_none() {
        tracing::warn!(
            "REGISTER_KEY is not set: POST /register is disabled (open registration is off by default)"
        );
    }
    if config.admin_key.is_none() {
        tracing::warn!(
            "ADMIN_KEY is not set: server-operator token reset is disabled; self reset remains available"
        );
    }

    let pool = PgPoolOptions::new()
        .max_connections(20)
        .acquire_timeout(Duration::from_secs(10))
        .connect(&config.database_url)
        .await
        .context("failed to connect to PostgreSQL")?;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("failed to run database migrations")?;

    let store = Store::new(pool);
    let state = bootstrap::build_default_state(config.clone(), store);
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(config.listen_addr)
        .await
        .with_context(|| format!("failed to bind {}", config.listen_addr))?;

    tracing::info!(
        listen_addr = %config.listen_addr,
        ws_queue_capacity = config.ws_queue_capacity,
        rate_limit_default = config.rate_limit_default,
        "OneBot 11 prototype server started (plaintext HTTP/WS; TLS belongs at Nginx)"
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP server failed")?;
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("onebot_server=info,tower_http=info,axum=info"));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .try_init();
}

async fn shutdown_signal() {
    if tokio::signal::ctrl_c().await.is_ok() {
        tracing::info!("shutdown signal received");
    }
}
