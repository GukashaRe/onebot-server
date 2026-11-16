use std::sync::Arc;

use crate::{
    cache::SessionCache, config::Config, protocol::ob11::Ob11Adapter, registry::ConnectionRegistry,
    state::AppState, store::Store,
};

pub fn build_state(
    config: Config,
    store: Store,
    adapter: Arc<dyn crate::protocol::ProtocolAdapter>,
) -> AppState {
    let registry = Arc::new(ConnectionRegistry::new());
    let cache = Arc::new(SessionCache::new(store.clone(), config.session_cache_ttl));
    let limiter = Arc::new(crate::state::RateLimiter::new());
    AppState {
        config: Arc::new(config),
        store,
        adapter,
        registry,
        cache,
        limiter,
    }
}

pub fn build_default_state(config: Config, store: Store) -> AppState {
    let adapter: Arc<dyn crate::protocol::ProtocolAdapter> = Arc::new(Ob11Adapter::new());
    build_state(config, store, adapter)
}
