use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use tokio::sync::RwLock;

use crate::{
    model::{Account, GroupMemberView, private_session_id},
    store::{Store, StoreError},
};

struct Entry<T> {
    value: T,
    expires_at: Instant,
}

impl<T> Entry<T> {
    fn fresh(value: T, ttl: Duration) -> Self {
        Self {
            value,
            expires_at: Instant::now() + ttl,
        }
    }

    fn is_fresh(&self) -> bool {
        Instant::now() < self.expires_at
    }
}

/// Short-lived process-local cache for fanout audience lookups. It is explicitly
/// not distributed: multi-instance connection tables are out of scope.
pub struct SessionCache {
    store: Store,
    ttl: Duration,
    groups: RwLock<HashMap<i64, Entry<Vec<GroupMemberView>>>>,
    private_pairs: RwLock<HashMap<String, Entry<Vec<Account>>>>,
}

impl SessionCache {
    pub fn new(store: Store, ttl: Duration) -> Self {
        Self {
            store,
            ttl,
            groups: RwLock::new(HashMap::new()),
            private_pairs: RwLock::new(HashMap::new()),
        }
    }

    pub async fn group_members(
        &self,
        group_id: i64,
    ) -> Result<Arc<Vec<GroupMemberView>>, StoreError> {
        {
            let cache = self.groups.read().await;
            if let Some(entry) = cache.get(&group_id)
                && entry.is_fresh()
            {
                return Ok(Arc::new(entry.value.clone()));
            }
        }

        let members = self.store.list_members(group_id).await?;
        let mut cache = self.groups.write().await;
        cache.insert(group_id, Entry::fresh(members.clone(), self.ttl));
        Ok(Arc::new(members))
    }

    /// Called after every membership mutation so the next fanout sees the new
    /// audience immediately rather than waiting for the TTL.
    pub async fn invalidate_group(&self, group_id: i64) {
        self.groups.write().await.remove(&group_id);
    }

    /// Private audiences are derivable from the ordered pair; we still cache the
    /// two account rows (including display names) to avoid repeated account
    /// lookups during rapid exchanges. There is no membership mutation to
    /// invalidate.
    pub async fn private_participants(
        &self,
        a: i64,
        b: i64,
    ) -> Result<Arc<Vec<Account>>, StoreError> {
        let key = private_session_id(a, b);
        {
            let cache = self.private_pairs.read().await;
            if let Some(entry) = cache.get(&key)
                && entry.is_fresh()
            {
                return Ok(Arc::new(entry.value.clone()));
            }
        }

        let accounts = self.store.get_accounts_by_ids(&[a, b]).await?;
        if accounts.len() != if a == b { 1 } else { 2 } {
            return Err(StoreError::NotFound(
                "private conversation account".to_owned(),
            ));
        }
        let mut cache = self.private_pairs.write().await;
        cache.insert(key, Entry::fresh(accounts.clone(), self.ttl));
        Ok(Arc::new(accounts))
    }

    pub async fn invalidate_private_pair(&self, a: i64, b: i64) {
        self.private_pairs
            .write()
            .await
            .remove(&private_session_id(a, b));
    }
}
