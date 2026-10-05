//! Result cache — LRU of serialized query results keyed by (project,
//! canonical query hash, files data version, identity epoch and seq).
//!
//! A key changes whenever anything the result was computed from changes:
//! new files (data version), merges (identity seq), or the resolved date
//! range (folded into the query hash). Results that depend on person
//! properties carry a TTL instead, because `$set` moves none of those.
//! In-memory only, bounded in entries and (serialized) bytes, rebuilt on
//! restart. Values are kept as-is, so a cached result is bit-identical to
//! the computed one.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const MAX_CACHE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct CacheKey {
    pub project_id: String,
    pub query_hash: [u8; 32],
    pub data_version: u64,
    pub identity_epoch: u64,
    pub identity_seq: u64,
}

struct CacheEntry<V> {
    value: V,
    bytes: usize,
    last_access: u64,
    expires: Option<Instant>,
}

struct CacheState<V> {
    entries: HashMap<CacheKey, CacheEntry<V>>,
    response_bytes: usize,
    access_clock: u64,
}

impl<V> CacheState<V> {
    fn tick(&mut self) -> u64 {
        self.access_clock = self.access_clock.saturating_add(1);
        self.access_clock
    }

    fn remove(&mut self, key: &CacheKey) {
        if let Some(entry) = self.entries.remove(key) {
            self.response_bytes = self.response_bytes.saturating_sub(entry.bytes);
        }
    }
}

pub struct ResultCache<V = Vec<u8>> {
    state: Mutex<CacheState<V>>,
    max_entries: usize,
    max_bytes: usize,
}

impl<V: Clone> ResultCache<V> {
    pub fn new(max_entries: usize) -> Self {
        Self::with_limits(max_entries, MAX_CACHE_BYTES)
    }

    pub fn with_limits(max_entries: usize, max_bytes: usize) -> Self {
        ResultCache {
            state: Mutex::new(CacheState {
                entries: HashMap::new(),
                response_bytes: 0,
                access_clock: 0,
            }),
            max_entries,
            max_bytes,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CacheState<V>> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn get(&self, key: &CacheKey) -> Option<V> {
        let mut state = self.lock();
        let access = state.tick();
        let expired = state
            .entries
            .get(key)
            .and_then(|entry| entry.expires)
            .is_some_and(|expires| Instant::now() >= expires);
        if expired {
            state.remove(key);
            return None;
        }
        state.entries.get_mut(key).map(|entry| {
            entry.last_access = access;
            entry.value.clone()
        })
    }

    /// Insert `value`, accounted as `bytes` (its serialized size).
    pub fn put(&self, key: CacheKey, value: V, bytes: usize, ttl: Option<Duration>) {
        let mut state = self.lock();
        state.remove(&key);

        // One response that cannot fit must not evict the useful cache or leave
        // a stale value for the same key behind.
        if self.max_entries == 0 || bytes > self.max_bytes {
            return;
        }

        let response_len = bytes;
        let access = state.tick();
        state.entries.insert(
            key,
            CacheEntry {
                value,
                bytes,
                last_access: access,
                expires: ttl.map(|ttl| Instant::now() + ttl),
            },
        );
        state.response_bytes += response_len;

        while state.entries.len() > self.max_entries || state.response_bytes > self.max_bytes {
            let Some(oldest) = state
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_access)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            state.remove(&oldest);
        }
    }

    pub fn clear(&self) {
        let mut state = self.lock();
        state.entries.clear();
        state.response_bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: u8) -> CacheKey {
        CacheKey {
            project_id: "p".into(),
            query_hash: [id; 32],
            data_version: 1,
            identity_epoch: 0,
            identity_seq: 0,
        }
    }

    #[test]
    fn entry_limit_evicts_the_least_recently_used_value() {
        let cache = ResultCache::with_limits(2, 1024);
        cache.put(key(1), vec![1], vec![1].len(), None);
        cache.put(key(2), vec![2], vec![2].len(), None);
        assert_eq!(cache.get(&key(1)), Some(vec![1]));

        cache.put(key(3), vec![3], vec![3].len(), None);

        assert_eq!(cache.get(&key(1)), Some(vec![1]));
        assert_eq!(cache.get(&key(2)), None);
        assert_eq!(cache.get(&key(3)), Some(vec![3]));
    }

    #[test]
    fn response_bytes_are_bounded_independently_of_entry_count() {
        let cache = ResultCache::with_limits(10, 5);
        cache.put(key(1), vec![1; 3], vec![1; 3].len(), None);
        cache.put(key(2), vec![2; 3], vec![2; 3].len(), None);

        assert_eq!(cache.get(&key(1)), None);
        assert_eq!(cache.get(&key(2)), Some(vec![2; 3]));
    }

    #[test]
    fn replacement_updates_byte_accounting_and_oversize_values_are_not_cached() {
        let cache = ResultCache::with_limits(10, 5);
        cache.put(key(1), vec![1; 4], vec![1; 4].len(), None);
        cache.put(key(1), vec![1; 2], vec![1; 2].len(), None);
        cache.put(key(2), vec![2; 3], vec![2; 3].len(), None);

        assert_eq!(cache.get(&key(1)), Some(vec![1; 2]));
        assert_eq!(cache.get(&key(2)), Some(vec![2; 3]));

        cache.put(key(2), vec![9; 6], vec![9; 6].len(), None);
        assert_eq!(cache.get(&key(2)), None);
    }

    #[test]
    fn expired_entries_are_not_served() {
        let cache = ResultCache::with_limits(10, 1024);
        cache.put(key(1), vec![1], vec![1].len(), Some(Duration::ZERO));
        cache.put(key(2), vec![2], 1, Some(Duration::from_secs(60)));
        assert_eq!(cache.get(&key(1)), None);
        assert_eq!(cache.get(&key(2)), Some(vec![2]));
    }

    #[test]
    fn identity_and_data_versions_separate_entries() {
        let cache = ResultCache::with_limits(10, 1024);
        cache.put(key(1), vec![1], vec![1].len(), None);
        let mut merged = key(1);
        merged.identity_seq = 1;
        assert_eq!(cache.get(&merged), None);
        let mut newer = key(1);
        newer.data_version = 2;
        assert_eq!(cache.get(&newer), None);
        cache.clear();
        assert_eq!(cache.get(&key(1)), None);
    }
}
