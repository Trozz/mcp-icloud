use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub mailbox: String,
    pub uidvalidity: u32,
    pub uid: u32,
}

/// Cache of raw message bodies. A (mailbox, UIDVALIDITY, UID) triple identifies immutable
/// content, so entries never go stale while the message exists; callers `remove` entries for
/// messages they move, delete or expunge. Flags and listings are never cached.
pub trait MessageCache: Send + Sync {
    fn get(&self, key: &CacheKey) -> Option<Arc<Vec<u8>>>;
    fn put(&self, key: CacheKey, raw: Arc<Vec<u8>>);
    fn remove(&self, key: &CacheKey);
}

pub struct LruMessageCache {
    max_bytes: usize,
    inner: Mutex<Inner>,
}

struct Inner {
    lru: lru::LruCache<CacheKey, Arc<Vec<u8>>>,
    bytes: usize,
}

impl LruMessageCache {
    /// `max_bytes == 0` disables caching.
    pub fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            inner: Mutex::new(Inner {
                lru: lru::LruCache::unbounded(),
                bytes: 0,
            }),
        }
    }

    #[cfg(test)]
    fn bytes(&self) -> usize {
        self.inner.lock().unwrap().bytes
    }
}

impl MessageCache for LruMessageCache {
    fn get(&self, key: &CacheKey) -> Option<Arc<Vec<u8>>> {
        let hit = self.inner.lock().expect("cache lock").lru.get(key).cloned();
        match hit {
            Some(_) => metrics::counter!("message_cache_hits_total").increment(1),
            None => metrics::counter!("message_cache_misses_total").increment(1),
        }
        hit
    }

    fn put(&self, key: CacheKey, raw: Arc<Vec<u8>>) {
        if self.max_bytes == 0 || raw.len() > self.max_bytes {
            return;
        }
        let mut inner = self.inner.lock().expect("cache lock");
        let size = raw.len();
        if let Some(old) = inner.lru.put(key, raw) {
            inner.bytes -= old.len();
        }
        inner.bytes += size;
        while inner.bytes > self.max_bytes {
            let Some((_, evicted)) = inner.lru.pop_lru() else {
                break;
            };
            inner.bytes -= evicted.len();
        }
    }

    fn remove(&self, key: &CacheKey) {
        let mut inner = self.inner.lock().expect("cache lock");
        if let Some(old) = inner.lru.pop(key) {
            inner.bytes -= old.len();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(uid: u32) -> CacheKey {
        CacheKey {
            mailbox: "INBOX".into(),
            uidvalidity: 1,
            uid,
        }
    }

    #[test]
    fn stores_and_returns() {
        let c = LruMessageCache::new(100);
        c.put(key(1), Arc::new(vec![1; 10]));
        assert_eq!(c.get(&key(1)).unwrap().len(), 10);
        assert!(c.get(&key(2)).is_none());
    }

    #[test]
    fn different_uidvalidity_is_a_different_entry() {
        let c = LruMessageCache::new(100);
        c.put(key(1), Arc::new(vec![1; 10]));
        assert!(
            c.get(&CacheKey {
                uidvalidity: 2,
                ..key(1)
            })
            .is_none()
        );
    }

    #[test]
    fn evicts_least_recently_used_over_budget() {
        let c = LruMessageCache::new(25);
        c.put(key(1), Arc::new(vec![0; 10]));
        c.put(key(2), Arc::new(vec![0; 10]));
        assert!(c.get(&key(1)).is_some());
        c.put(key(3), Arc::new(vec![0; 10]));
        assert!(c.get(&key(1)).is_some());
        assert!(c.get(&key(2)).is_none());
        assert!(c.get(&key(3)).is_some());
        assert_eq!(c.bytes(), 20);
    }

    #[test]
    fn replacing_an_entry_keeps_byte_count_right() {
        let c = LruMessageCache::new(100);
        c.put(key(1), Arc::new(vec![0; 10]));
        c.put(key(1), Arc::new(vec![0; 30]));
        assert_eq!(c.bytes(), 30);
    }

    #[test]
    fn remove_drops_the_entry_and_its_bytes() {
        let c = LruMessageCache::new(100);
        c.put(key(1), Arc::new(vec![0; 10]));
        c.put(key(2), Arc::new(vec![0; 10]));
        c.remove(&key(1));
        c.remove(&key(3));
        assert!(c.get(&key(1)).is_none());
        assert!(c.get(&key(2)).is_some());
        assert_eq!(c.bytes(), 10);
    }

    #[test]
    fn zero_budget_disables_and_oversized_is_skipped() {
        let off = LruMessageCache::new(0);
        off.put(key(1), Arc::new(vec![0; 1]));
        assert!(off.get(&key(1)).is_none());
        let small = LruMessageCache::new(5);
        small.put(key(1), Arc::new(vec![0; 6]));
        assert!(small.get(&key(1)).is_none());
        assert_eq!(small.bytes(), 0);
    }
}
