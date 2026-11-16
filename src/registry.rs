use std::{
    collections::HashMap,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct ConnectionHandle {
    pub connection_id: u64,
    pub account_id: i64,
    pub sender: mpsc::Sender<String>,
    pub cancel: CancellationToken,
    pub slow: Arc<AtomicBool>,
}

#[derive(Default, Debug)]
pub struct PushReport {
    pub delivered: usize,
    pub slow_closed: usize,
}

/// In-memory account -> connections map. Every connection has its own bounded
/// queue; a full queue is never awaited by fanout.
pub struct ConnectionRegistry {
    connections: RwLock<HashMap<i64, HashMap<u64, ConnectionHandle>>>,
    next_id: AtomicU64,
    slow_disconnects: AtomicU64,
}

impl Default for ConnectionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ConnectionRegistry {
    pub fn new() -> Self {
        Self {
            connections: RwLock::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            slow_disconnects: AtomicU64::new(0),
        }
    }

    pub fn next_connection_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn register(&self, handle: ConnectionHandle) {
        let account_id = handle.account_id;
        let connection_id = handle.connection_id;
        let mut connections = self
            .connections
            .write()
            .expect("connection registry poisoned");
        connections
            .entry(account_id)
            .or_default()
            .insert(connection_id, handle);
    }

    pub fn unregister(&self, account_id: i64, connection_id: u64) {
        let mut connections = self
            .connections
            .write()
            .expect("connection registry poisoned");
        if let Some(account_connections) = connections.get_mut(&account_id) {
            account_connections.remove(&connection_id);
            if account_connections.is_empty() {
                connections.remove(&account_id);
            }
        }
    }

    /// Non-blocking fanout primitive. Locks are only held for short in-memory
    /// operations; delivery uses try_send. A full or closed queue marks the
    /// connection slow, cancels its token, and immediately drops it from the
    /// registry.
    pub fn push(&self, account_id: i64, text: String) -> PushReport {
        let handles: Vec<ConnectionHandle> = {
            let connections = self
                .connections
                .read()
                .expect("connection registry poisoned");
            connections
                .get(&account_id)
                .map(|entries| entries.values().cloned().collect())
                .unwrap_or_default()
        };

        let mut report = PushReport::default();
        let mut to_drop = Vec::new();
        for handle in handles {
            if handle.slow.load(Ordering::Relaxed) {
                to_drop.push(handle.connection_id);
                continue;
            }
            match handle.sender.try_send(text.clone()) {
                Ok(()) => {
                    report.delivered += 1;
                }
                Err(mpsc::error::TrySendError::Full(_)) => {
                    handle.slow.store(true, Ordering::Relaxed);
                    handle.cancel.cancel();
                    to_drop.push(handle.connection_id);
                    report.slow_closed += 1;
                    self.slow_disconnects.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(
                        account_id,
                        connection_id = handle.connection_id,
                        "slow WebSocket consumer; closing instead of blocking fanout"
                    );
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    handle.cancel.cancel();
                    to_drop.push(handle.connection_id);
                }
            }
        }

        if !to_drop.is_empty() {
            let mut connections = self
                .connections
                .write()
                .expect("connection registry poisoned");
            if let Some(account_connections) = connections.get_mut(&account_id) {
                for id in to_drop {
                    account_connections.remove(&id);
                }
                if account_connections.is_empty() {
                    connections.remove(&account_id);
                }
            }
        }
        report
    }

    /// Actively disconnect every live connection for an account (token reset).
    pub fn kick_account(&self, account_id: i64) -> usize {
        let removed = {
            let mut connections = self
                .connections
                .write()
                .expect("connection registry poisoned");
            connections.remove(&account_id).unwrap_or_default()
        };
        let count = removed.len();
        for handle in removed.into_values() {
            handle.slow.store(true, Ordering::Relaxed);
            handle.cancel.cancel();
        }
        count
    }

    pub fn active_connections(&self) -> usize {
        self.connections
            .read()
            .expect("connection registry poisoned")
            .values()
            .map(HashMap::len)
            .sum()
    }

    pub fn slow_disconnects(&self) -> u64 {
        self.slow_disconnects.load(Ordering::Relaxed)
    }
}
